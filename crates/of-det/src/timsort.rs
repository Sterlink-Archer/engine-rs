//! V8 `Array.prototype.sort` (Torque TimSort, `third_party/v8/builtins/array-sort.tq`,
//! V8 13.6.233.17 as shipped in Node v24.21.0), ported call-for-call from the verified
//! Python reference `conformance/vectors/check_py/v8_timsort.py`.
//!
//! Why call-for-call: 659 of the golden sort cases use comparators that are INCONSISTENT
//! (NaN from `Inf - Inf` or NaN keys). Their output depends on exactly which pairs V8
//! compares, in which order; `of-vectors` checks the full comparator call trace of 4,613
//! cases, not just the output. For consistent comparators any stable sort gives the same
//! output; [`sort_by`] is still the V8 algorithm so that comparator side effects (and
//! counts) match too.
//!
//! Comparator results are JS numbers: `NaN` is treated as `+0` (`SortCompareUserFn`),
//! and every branch tests `order < 0` / `order >= 0` (so `-0` is "not less").
//!
//! Holes/undefined preprocessing is omitted: engine arrays are dense, never holey.

const MIN_GALLOP_WINS: usize = 7;
const TEMP_SIZE: usize = 32;

/// Sort `v` in place exactly like V8's `Array.prototype.sort(cmp)`.
pub fn sort_by<T: Clone, F: FnMut(&T, &T) -> f64>(v: &mut [T], cmp: F) {
    let n = v.len();
    if n < 2 {
        return;
    }
    let mut st = TimSort { work: v, cmp, min_gallop: MIN_GALLOP_WINS, runs: Vec::new(), temp: Vec::new() };
    st.sort(n);
}

/// JS `arr.sort((a, b) => key(a) - key(b))` (ascending numeric key, the engine's most
/// common comparator shape).
pub fn sort_by_key_asc<T: Clone, K: FnMut(&T) -> f64>(v: &mut [T], mut key: K) {
    sort_by(v, |a, b| key(a) - key(b))
}

/// Sort a copy (like `[...arr].sort(cmp)`).
pub fn sorted_by<T: Clone, F: FnMut(&T, &T) -> f64>(v: &[T], cmp: F) -> Vec<T> {
    let mut out = v.to_vec();
    sort_by(&mut out, cmp);
    out
}

struct TimSort<'a, T, F> {
    work: &'a mut [T],
    cmp: F,
    min_gallop: usize,
    runs: Vec<(usize, usize)>, // (base, length)
    temp: Vec<T>,
}

#[derive(PartialEq, Eq)]
enum MergeExit {
    Succeed,
    CopyRest,
}

/// FixedArray copy within one slice (overlap-safe in the same direction as array-sort.tq).
#[inline]
fn copy_within<T: Clone>(a: &mut [T], src: usize, dst: usize, len: usize) {
    if len == 0 {
        return;
    }
    if src < dst {
        let mut i = len;
        while i > 0 {
            i -= 1;
            a[dst + i] = a[src + i].clone();
        }
    } else {
        for i in 0..len {
            a[dst + i] = a[src + i].clone();
        }
    }
}

impl<'a, T: Clone, F: FnMut(&T, &T) -> f64> TimSort<'a, T, F> {
    #[inline]
    fn compare(&mut self, a: &T, b: &T) -> f64 {
        let v = (self.cmp)(a, b);
        if v.is_nan() { 0.0 } else { v }
    }

    fn ensure_temp(&mut self, requested: usize) {
        let min_size = requested.max(TEMP_SIZE);
        if self.temp.len() < min_size {
            // Contents are always written before they are read.
            self.temp.clear();
            self.temp.resize(min_size, self.work[0].clone());
        }
    }

    fn binary_insertion_sort(&mut self, low: usize, start_arg: usize, high: usize) {
        let mut start = if low == start_arg { start_arg + 1 } else { start_arg };
        while start < high {
            let mut left = low;
            let mut right = start;
            let pivot = self.work[right].clone();
            while left < right {
                let mid = left + ((right - left) >> 1);
                let m = self.work[mid].clone();
                let order = self.compare(&pivot, &m);
                if order < 0.0 {
                    right = mid;
                } else {
                    left = mid + 1;
                }
            }
            let mut p = start;
            while p > left {
                self.work[p] = self.work[p - 1].clone();
                p -= 1;
            }
            self.work[left] = pivot;
            start += 1;
        }
    }

    fn count_and_make_run(&mut self, low_arg: usize, high: usize) -> usize {
        let low = low_arg + 1;
        if low == high {
            return 1;
        }
        let mut run_length = 2;
        let (a, b) = (self.work[low].clone(), self.work[low - 1].clone());
        let order = self.compare(&a, &b);
        let is_descending = order < 0.0;
        let mut previous = a;
        let mut idx = low + 1;
        while idx < high {
            let current = self.work[idx].clone();
            let order = self.compare(&current, &previous);
            if is_descending {
                if order >= 0.0 {
                    break;
                }
            } else if order < 0.0 {
                break;
            }
            previous = current;
            run_length += 1;
            idx += 1;
        }
        if is_descending {
            self.work[low_arg..low_arg + run_length].reverse();
        }
        run_length
    }

    /// GallopLeft over `arr` (work if `in_temp == false`, else temp).
    fn gallop_left(&mut self, in_temp: bool, key: &T, base: usize, length: usize, hint: usize) -> usize {
        let mut last_ofs: isize = 0;
        let mut offset: isize = 1;
        let hint_i = hint as isize;
        let len_i = length as isize;
        let at = |s: &Self, i: isize| -> T {
            if in_temp {
                s.temp[(base as isize + i) as usize].clone()
            } else {
                s.work[(base as isize + i) as usize].clone()
            }
        };
        let x = at(self, hint_i);
        let order = self.compare(&x, key);
        if order < 0.0 {
            let max_ofs = len_i - hint_i;
            while offset < max_ofs {
                let x = at(self, hint_i + offset);
                if self.compare(&x, key) >= 0.0 {
                    break;
                }
                last_ofs = offset;
                offset = (offset << 1) + 1;
                if offset <= 0 {
                    offset = max_ofs;
                }
            }
            if offset > max_ofs {
                offset = max_ofs;
            }
            last_ofs += hint_i;
            offset += hint_i;
        } else {
            let max_ofs = hint_i + 1;
            while offset < max_ofs {
                let x = at(self, hint_i - offset);
                if self.compare(&x, key) < 0.0 {
                    break;
                }
                last_ofs = offset;
                offset = (offset << 1) + 1;
                if offset <= 0 {
                    offset = max_ofs;
                }
            }
            if offset > max_ofs {
                offset = max_ofs;
            }
            let tmp = last_ofs;
            last_ofs = hint_i - offset;
            offset = hint_i - tmp;
        }
        last_ofs += 1;
        while last_ofs < offset {
            let m = last_ofs + ((offset - last_ofs) >> 1);
            let x = at(self, m);
            if self.compare(&x, key) < 0.0 {
                last_ofs = m + 1;
            } else {
                offset = m;
            }
        }
        offset as usize
    }

    fn gallop_right(&mut self, in_temp: bool, key: &T, base: usize, length: usize, hint: usize) -> usize {
        let mut last_ofs: isize = 0;
        let mut offset: isize = 1;
        let hint_i = hint as isize;
        let len_i = length as isize;
        let at = |s: &Self, i: isize| -> T {
            if in_temp {
                s.temp[(base as isize + i) as usize].clone()
            } else {
                s.work[(base as isize + i) as usize].clone()
            }
        };
        let x = at(self, hint_i);
        let order = self.compare(key, &x);
        if order < 0.0 {
            let max_ofs = hint_i + 1;
            while offset < max_ofs {
                let x = at(self, hint_i - offset);
                if self.compare(key, &x) >= 0.0 {
                    break;
                }
                last_ofs = offset;
                offset = (offset << 1) + 1;
                if offset <= 0 {
                    offset = max_ofs;
                }
            }
            if offset > max_ofs {
                offset = max_ofs;
            }
            let tmp = last_ofs;
            last_ofs = hint_i - offset;
            offset = hint_i - tmp;
        } else {
            let max_ofs = len_i - hint_i;
            while offset < max_ofs {
                let x = at(self, hint_i + offset);
                if self.compare(key, &x) < 0.0 {
                    break;
                }
                last_ofs = offset;
                offset = (offset << 1) + 1;
                if offset <= 0 {
                    offset = max_ofs;
                }
            }
            if offset > max_ofs {
                offset = max_ofs;
            }
            last_ofs += hint_i;
            offset += hint_i;
        }
        last_ofs += 1;
        while last_ofs < offset {
            let m = last_ofs + ((offset - last_ofs) >> 1);
            let x = at(self, m);
            if self.compare(key, &x) < 0.0 {
                offset = m;
            } else {
                last_ofs = m + 1;
            }
        }
        offset as usize
    }

    fn merge_low(&mut self, base_a: usize, length_a: usize, base_b: usize, length_b: usize) {
        let mut length_a = length_a;
        let mut length_b = length_b;
        self.ensure_temp(length_a);
        for i in 0..length_a {
            self.temp[i] = self.work[base_a + i].clone();
        }
        let mut dest = base_a;
        let mut cursor_temp = 0usize;
        let mut cursor_b = base_b;

        self.work[dest] = self.work[cursor_b].clone();
        dest += 1;
        cursor_b += 1;

        let exit = 'body: {
            length_b -= 1;
            if length_b == 0 {
                break 'body MergeExit::Succeed;
            }
            if length_a == 1 {
                break 'body MergeExit::CopyRest;
            }
            let mut min_gallop = self.min_gallop;
            loop {
                let mut nof_wins_a = 0usize;
                let mut nof_wins_b = 0usize;
                loop {
                    let (b, t) = (self.work[cursor_b].clone(), self.temp[cursor_temp].clone());
                    let order = self.compare(&b, &t);
                    if order < 0.0 {
                        self.work[dest] = b;
                        dest += 1;
                        cursor_b += 1;
                        nof_wins_b += 1;
                        length_b -= 1;
                        nof_wins_a = 0;
                        if length_b == 0 {
                            break 'body MergeExit::Succeed;
                        }
                        if nof_wins_b >= min_gallop {
                            break;
                        }
                    } else {
                        self.work[dest] = t;
                        dest += 1;
                        cursor_temp += 1;
                        nof_wins_a += 1;
                        length_a -= 1;
                        nof_wins_b = 0;
                        if length_a == 1 {
                            break 'body MergeExit::CopyRest;
                        }
                        if nof_wins_a >= min_gallop {
                            break;
                        }
                    }
                }
                min_gallop += 1;
                let mut first_iteration = true;
                while nof_wins_a >= MIN_GALLOP_WINS || nof_wins_b >= MIN_GALLOP_WINS || first_iteration {
                    first_iteration = false;
                    min_gallop = if min_gallop < 2 { 1 } else { min_gallop - 1 };
                    self.min_gallop = min_gallop;

                    let key = self.work[cursor_b].clone();
                    nof_wins_a = self.gallop_right(true, &key, cursor_temp, length_a, 0);
                    if nof_wins_a > 0 {
                        for i in 0..nof_wins_a {
                            self.work[dest + i] = self.temp[cursor_temp + i].clone();
                        }
                        dest += nof_wins_a;
                        cursor_temp += nof_wins_a;
                        length_a -= nof_wins_a;
                        if length_a == 1 {
                            break 'body MergeExit::CopyRest;
                        }
                        if length_a == 0 {
                            break 'body MergeExit::Succeed;
                        }
                    }
                    self.work[dest] = self.work[cursor_b].clone();
                    dest += 1;
                    cursor_b += 1;
                    length_b -= 1;
                    if length_b == 0 {
                        break 'body MergeExit::Succeed;
                    }

                    let key = self.temp[cursor_temp].clone();
                    nof_wins_b = self.gallop_left(false, &key, cursor_b, length_b, 0);
                    if nof_wins_b > 0 {
                        copy_within(self.work, cursor_b, dest, nof_wins_b);
                        dest += nof_wins_b;
                        cursor_b += nof_wins_b;
                        length_b -= nof_wins_b;
                        if length_b == 0 {
                            break 'body MergeExit::Succeed;
                        }
                    }
                    self.work[dest] = self.temp[cursor_temp].clone();
                    dest += 1;
                    cursor_temp += 1;
                    length_a -= 1;
                    if length_a == 1 {
                        break 'body MergeExit::CopyRest;
                    }
                }
                min_gallop += 1;
                self.min_gallop = min_gallop;
            }
        };
        match exit {
            MergeExit::Succeed => {
                for i in 0..length_a {
                    self.work[dest + i] = self.temp[cursor_temp + i].clone();
                }
            }
            MergeExit::CopyRest => {
                copy_within(self.work, cursor_b, dest, length_b);
                self.work[dest + length_b] = self.temp[cursor_temp].clone();
            }
        }
    }

    fn merge_high(&mut self, base_a: usize, length_a: usize, base_b: usize, length_b: usize) {
        let mut length_a = length_a;
        let mut length_b = length_b;
        self.ensure_temp(length_b);
        for i in 0..length_b {
            self.temp[i] = self.work[base_b + i].clone();
        }
        // Signed cursors: they may step to base-1 at the very end.
        let mut dest: isize = (base_b + length_b - 1) as isize;
        let mut cursor_temp: isize = length_b as isize - 1;
        let mut cursor_a: isize = (base_a + length_a - 1) as isize;

        self.work[dest as usize] = self.work[cursor_a as usize].clone();
        dest -= 1;
        cursor_a -= 1;

        let exit = 'body: {
            length_a -= 1;
            if length_a == 0 {
                break 'body MergeExit::Succeed;
            }
            if length_b == 1 {
                break 'body MergeExit::CopyRest;
            }
            let mut min_gallop = self.min_gallop;
            loop {
                let mut nof_wins_a = 0usize;
                let mut nof_wins_b = 0usize;
                loop {
                    let (t, a) = (self.temp[cursor_temp as usize].clone(), self.work[cursor_a as usize].clone());
                    let order = self.compare(&t, &a);
                    if order < 0.0 {
                        self.work[dest as usize] = a;
                        dest -= 1;
                        cursor_a -= 1;
                        nof_wins_a += 1;
                        length_a -= 1;
                        nof_wins_b = 0;
                        if length_a == 0 {
                            break 'body MergeExit::Succeed;
                        }
                        if nof_wins_a >= min_gallop {
                            break;
                        }
                    } else {
                        self.work[dest as usize] = t;
                        dest -= 1;
                        cursor_temp -= 1;
                        nof_wins_b += 1;
                        length_b -= 1;
                        nof_wins_a = 0;
                        if length_b == 1 {
                            break 'body MergeExit::CopyRest;
                        }
                        if nof_wins_b >= min_gallop {
                            break;
                        }
                    }
                }
                min_gallop += 1;
                let mut first_iteration = true;
                while nof_wins_a >= MIN_GALLOP_WINS || nof_wins_b >= MIN_GALLOP_WINS || first_iteration {
                    first_iteration = false;
                    min_gallop = if min_gallop < 2 { 1 } else { min_gallop - 1 };
                    self.min_gallop = min_gallop;

                    let key = self.temp[cursor_temp as usize].clone();
                    let k = self.gallop_right(false, &key, base_a, length_a, length_a - 1);
                    nof_wins_a = length_a - k;
                    if nof_wins_a > 0 {
                        dest -= nof_wins_a as isize;
                        cursor_a -= nof_wins_a as isize;
                        copy_within(self.work, (cursor_a + 1) as usize, (dest + 1) as usize, nof_wins_a);
                        length_a -= nof_wins_a;
                        if length_a == 0 {
                            break 'body MergeExit::Succeed;
                        }
                    }
                    self.work[dest as usize] = self.temp[cursor_temp as usize].clone();
                    dest -= 1;
                    cursor_temp -= 1;
                    length_b -= 1;
                    if length_b == 1 {
                        break 'body MergeExit::CopyRest;
                    }

                    let key = self.work[cursor_a as usize].clone();
                    let k = self.gallop_left(true, &key, 0, length_b, length_b - 1);
                    nof_wins_b = length_b - k;
                    if nof_wins_b > 0 {
                        dest -= nof_wins_b as isize;
                        cursor_temp -= nof_wins_b as isize;
                        for i in 0..nof_wins_b {
                            self.work[(dest + 1) as usize + i] = self.temp[(cursor_temp + 1) as usize + i].clone();
                        }
                        length_b -= nof_wins_b;
                        if length_b == 1 {
                            break 'body MergeExit::CopyRest;
                        }
                        if length_b == 0 {
                            break 'body MergeExit::Succeed;
                        }
                    }
                    self.work[dest as usize] = self.work[cursor_a as usize].clone();
                    dest -= 1;
                    cursor_a -= 1;
                    length_a -= 1;
                    if length_a == 0 {
                        break 'body MergeExit::Succeed;
                    }
                }
                min_gallop += 1;
                self.min_gallop = min_gallop;
            }
        };
        match exit {
            MergeExit::Succeed => {
                if length_b > 0 {
                    let d0 = (dest - (length_b as isize - 1)) as usize;
                    for i in 0..length_b {
                        self.work[d0 + i] = self.temp[i].clone();
                    }
                }
            }
            MergeExit::CopyRest => {
                dest -= length_a as isize;
                cursor_a -= length_a as isize;
                copy_within(self.work, (cursor_a + 1) as usize, (dest + 1) as usize, length_a);
                self.work[dest as usize] = self.temp[cursor_temp as usize].clone();
            }
        }
    }

    fn merge_at(&mut self, i: usize) {
        let stack_size = self.runs.len();
        let (mut base_a, mut length_a) = self.runs[i];
        let (base_b, mut length_b) = self.runs[i + 1];
        self.runs[i].1 = length_a + length_b;
        if i + 3 == stack_size {
            self.runs[i + 1] = self.runs[i + 2];
        }
        self.runs.pop();

        let key = self.work[base_b].clone();
        let k = self.gallop_right(false, &key, base_a, length_a, 0);
        base_a += k;
        length_a -= k;
        if length_a == 0 {
            return;
        }
        let key_left = self.work[base_a + length_a - 1].clone();
        length_b = self.gallop_left(false, &key_left, base_b, length_b, length_b - 1);
        if length_b == 0 {
            return;
        }
        if length_a <= length_b {
            self.merge_low(base_a, length_a, base_b, length_b);
        } else {
            self.merge_high(base_a, length_a, base_b, length_b);
        }
    }

    #[inline]
    fn run_len(&self, n: usize) -> usize {
        self.runs[n].1
    }

    /// `run_length(n - 2) > run_length(n - 1) + run_length(n)`; true for n < 2.
    fn invariant_established(&self, n: usize) -> bool {
        if n < 2 {
            return true;
        }
        self.run_len(n - 2) > self.run_len(n - 1) + self.run_len(n)
    }

    fn merge_collapse(&mut self) {
        while self.runs.len() > 1 {
            let mut n = self.runs.len() - 2;
            if !self.invariant_established(n + 1) || !self.invariant_established(n) {
                if self.run_len(n - 1) < self.run_len(n + 1) {
                    n -= 1;
                }
                self.merge_at(n);
            } else if self.run_len(n) <= self.run_len(n + 1) {
                self.merge_at(n);
            } else {
                break;
            }
        }
    }

    fn merge_force_collapse(&mut self) {
        while self.runs.len() > 1 {
            let mut n = self.runs.len() - 2;
            if n > 0 && self.run_len(n - 1) < self.run_len(n + 1) {
                n -= 1;
            }
            self.merge_at(n);
        }
    }

    fn sort(&mut self, length: usize) {
        let mut remaining = length;
        let mut low = 0;
        let min_run = compute_min_run_length(remaining);
        while remaining != 0 {
            let mut current = self.count_and_make_run(low, low + remaining);
            if current < min_run {
                let forced = min_run.min(remaining);
                self.binary_insertion_sort(low, low + current, low + forced);
                current = forced;
            }
            self.runs.push((low, current));
            self.merge_collapse();
            low += current;
            remaining -= current;
        }
        self.merge_force_collapse();
    }
}

fn compute_min_run_length(n_arg: usize) -> usize {
    let mut n = n_arg;
    let mut r = 0;
    while n >= 64 {
        r |= n & 1;
        n >>= 1;
    }
    n + r
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sorts_and_is_stable() {
        let mut v: Vec<(i32, usize)> = (0..1000).map(|i| (((i * 7919) % 13) as i32, i)).collect();
        sort_by(&mut v, |a, b| (a.0 - b.0) as f64);
        for w in v.windows(2) {
            assert!(w[0].0 < w[1].0 || (w[0].0 == w[1].0 && w[0].1 < w[1].1));
        }
    }
}
