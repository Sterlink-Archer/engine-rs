//! Exact port of `packages/engine-lib/src/PseudoRandom.ts` (sfc32, splitmix32 seeding,
//! 12 warm-up draws). All state is int32; outputs that JS returns as numbers are f64 here
//! so that callers keep the exact JS arithmetic (`nextInt` floors f64 products, etc.).
//! Checked by `of-vectors` against `prng.json.zst`.

use crate::js::{pad_start_zero, to_int32, to_string_radix};

/// The sfc32 generator. Equality and hashing look at the four state words only (the
/// `prng-trace` counters are instrumentation, not state).
#[derive(Clone, Debug)]
pub struct PseudoRandom {
    s0: i32,
    s1: i32,
    s2: i32,
    s3: i32,
    #[cfg(feature = "prng-trace")]
    trace_tag: u32,
    #[cfg(feature = "prng-trace")]
    draws: u64,
}

impl PartialEq for PseudoRandom {
    fn eq(&self, o: &Self) -> bool {
        self.get_state() == o.get_state()
    }
}
impl Eq for PseudoRandom {}
impl std::hash::Hash for PseudoRandom {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        self.get_state().hash(h)
    }
}

/// `prng-trace` (ARCHITECTURE §4.5, review 0 #13; ledger t088 item (f)): every draw of
/// every `PseudoRandom` is reported to a thread-local hook as
/// `(owner tag, draw index, value, caller location)`, so `conform --prng-trace` can find
/// the first divergent draw per nation and name the behaviour file that made it.
/// Compiled out entirely without the feature.
#[cfg(feature = "prng-trace")]
pub mod trace {
    use std::cell::RefCell;
    use std::panic::Location;

    /// One draw (one sfc32 step).
    #[derive(Clone, Copy, Debug)]
    pub struct Draw {
        /// Set by the owner with `set_trace_tag` (e.g. a nation's smallID); 0 = untagged.
        pub tag: u32,
        /// 0-based draw counter of this generator.
        pub index: u64,
        /// The raw `t >>> 0` of the step.
        pub value: u32,
        /// The engine call site that drew (through `#[track_caller]`).
        pub location: &'static Location<'static>,
    }

    pub type Hook = Box<dyn FnMut(&Draw)>;
    thread_local! {
        // Debug instrumentation only (feature-gated); not simulation state.
        static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) };
    }

    /// Install (or with `None`, remove) this thread's draw hook.
    pub fn set_hook(h: Option<Hook>) {
        HOOK.with(|c| *c.borrow_mut() = h);
    }

    pub(crate) fn emit(d: Draw) {
        HOOK.with(|c| {
            if let Some(h) = c.borrow_mut().as_mut() {
                h(&d)
            }
        });
    }
}

/// Error for `randElement([])` / `randFromSet(empty)` (the engine throws; stream untouched).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmptyChoice;

const POW36_8: f64 = 2_821_109_907_456.0; // 36^8

impl PseudoRandom {
    /// `new PseudoRandom(seed)`: seed is truncated by `seed | 0`.
    pub fn new(seed: f64) -> Self {
        Self::from_seed_i32(to_int32(seed))
    }

    /// Constructor with the already-truncated int32 seed.
    pub fn from_seed_i32(seed: i32) -> Self {
        let mut h = seed;
        let mut split = || {
            h = h.wrapping_add(0x9e37_79b9_u32 as i32);
            let mut t = h ^ ((h as u32) >> 16) as i32;
            t = t.wrapping_mul(0x21f0_aaad);
            t ^= ((t as u32) >> 15) as i32;
            t = t.wrapping_mul(0x735a_2d97);
            t ^ ((t as u32) >> 15) as i32
        };
        let s0 = split();
        let s1 = split();
        let s2 = split();
        let s3 = split();
        let mut r = Self::raw(s0, s1, s2, s3);
        // Warm-up draws are part of construction: not traced, not counted.
        for _ in 0..12 {
            r.step();
        }
        r
    }

    #[inline]
    fn raw(s0: i32, s1: i32, s2: i32, s3: i32) -> Self {
        PseudoRandom {
            s0,
            s1,
            s2,
            s3,
            #[cfg(feature = "prng-trace")]
            trace_tag: 0,
            #[cfg(feature = "prng-trace")]
            draws: 0,
        }
    }

    /// Tag this generator's draws for `prng-trace` (no-op without the feature).
    #[inline]
    pub fn set_trace_tag(&mut self, _tag: u32) {
        #[cfg(feature = "prng-trace")]
        {
            self.trace_tag = _tag;
        }
    }

    /// Number of draws made so far (always 0 without `prng-trace`).
    #[inline]
    pub fn draw_count(&self) -> u64 {
        #[cfg(feature = "prng-trace")]
        {
            self.draws
        }
        #[cfg(not(feature = "prng-trace"))]
        {
            0
        }
    }

    /// `getState()`: the four int32 words.
    #[inline]
    pub fn get_state(&self) -> [i32; 4] {
        [self.s0, self.s1, self.s2, self.s3]
    }

    /// `PseudoRandom.fromState(state)`: each word through `| 0`.
    pub fn from_state_f64(state: [f64; 4]) -> Self {
        Self::raw(to_int32(state[0]), to_int32(state[1]), to_int32(state[2]), to_int32(state[3]))
    }
    #[inline]
    pub fn from_state(state: [i32; 4]) -> Self {
        Self::raw(state[0], state[1], state[2], state[3])
    }

    /// One sfc32 step; returns `t >>> 0` (so `next() == next_u32() / 2^32` exactly).
    #[inline]
    #[cfg_attr(feature = "prng-trace", track_caller)]
    pub fn next_u32(&mut self) -> u32 {
        let t = self.step();
        #[cfg(feature = "prng-trace")]
        {
            trace::emit(trace::Draw {
                tag: self.trace_tag,
                index: self.draws,
                value: t,
                location: std::panic::Location::caller(),
            });
            self.draws += 1;
        }
        t
    }

    /// The raw sfc32 step (`PseudoRandom.ts` `next()` body without the division).
    #[inline(always)]
    fn step(&mut self) -> u32 {
        let t = self.s0.wrapping_add(self.s1).wrapping_add(self.s3);
        self.s3 = self.s3.wrapping_add(1);
        self.s0 = self.s1 ^ ((self.s1 as u32) >> 9) as i32;
        self.s1 = self.s2.wrapping_add(self.s2.wrapping_shl(3));
        self.s2 = self.s2.wrapping_shl(21) | ((self.s2 as u32) >> 11) as i32;
        self.s2 = self.s2.wrapping_add(t);
        t as u32
    }

    /// `next()`: float in [0, 1).
    #[inline]
    #[cfg_attr(feature = "prng-trace", track_caller)]
    pub fn next(&mut self) -> f64 {
        self.next_u32() as f64 / 4_294_967_296.0
    }

    /// `nextInt(min, max)`: `Math.floor(next() * (floor(max) - floor(min))) + floor(min)`.
    #[inline]
    #[cfg_attr(feature = "prng-trace", track_caller)]
    pub fn next_int(&mut self, min: f64, max: f64) -> f64 {
        let lo = min.floor();
        let hi = max.floor();
        (self.next() * (hi - lo)).floor() + lo
    }

    /// `nextInt` for the common integer case; same draws and result as [`Self::next_int`].
    #[inline]
    #[cfg_attr(feature = "prng-trace", track_caller)]
    pub fn next_int_i64(&mut self, min: i64, max: i64) -> i64 {
        self.next_int(min as f64, max as f64) as i64
    }

    /// `nextFloat(min, max)`: `next() * (max - min) + min`.
    #[inline]
    #[cfg_attr(feature = "prng-trace", track_caller)]
    pub fn next_float(&mut self, min: f64, max: f64) -> f64 {
        self.next() * (max - min) + min
    }

    /// `nextID()`: 8 base-36 chars.
    #[cfg_attr(feature = "prng-trace", track_caller)]
    pub fn next_id(&mut self) -> String {
        let n = (self.next() * POW36_8).floor();
        pad_start_zero(&to_string_radix(n, 36), 8)
    }

    /// `randElement(arr)` returning the chosen INDEX. Empty -> error, no draw consumed.
    #[inline]
    #[cfg_attr(feature = "prng-trace", track_caller)]
    pub fn rand_index(&mut self, len: usize) -> Result<usize, EmptyChoice> {
        if len == 0 {
            return Err(EmptyChoice);
        }
        Ok(self.next_int(0.0, len as f64) as usize)
    }

    /// `randElement(arr)`.
    #[cfg_attr(feature = "prng-trace", track_caller)]
    pub fn rand_element<'a, T>(&mut self, arr: &'a [T]) -> Result<&'a T, EmptyChoice> {
        let i = self.rand_index(arr.len())?;
        Ok(&arr[i])
    }

    /// `randFromSet(set)`: index into the set's insertion order (callers pass the
    /// iteration-ordered items, e.g. from `OrdSet::iter`).
    #[cfg_attr(feature = "prng-trace", track_caller)]
    pub fn rand_from_iter<T, I: ExactSizeIterator<Item = T>>(&mut self, mut it: I) -> Result<T, EmptyChoice> {
        let i = self.rand_index(it.len())?;
        Ok(it.nth(i).expect("index < len"))
    }

    /// `chance(odds)`: `nextInt(0, odds) === 0`.
    #[inline]
    #[cfg_attr(feature = "prng-trace", track_caller)]
    pub fn chance(&mut self, odds: f64) -> bool {
        self.next_int(0.0, odds) == 0.0
    }

    /// `shuffleArray(array)`: Fisher-Yates on a copy.
    #[cfg_attr(feature = "prng-trace", track_caller)]
    pub fn shuffle_array<T: Clone>(&mut self, array: &[T]) -> Vec<T> {
        let mut result = array.to_vec();
        self.shuffle_in_place(&mut result);
        result
    }

    /// In-place variant with identical draws.
    #[cfg_attr(feature = "prng-trace", track_caller)]
    pub fn shuffle_in_place<T>(&mut self, result: &mut [T]) {
        let n = result.len();
        if n < 2 {
            return;
        }
        let mut i = n - 1;
        while i > 0 {
            let j = self.next_int(0.0, (i + 1) as f64) as usize;
            result.swap(i, j);
            i -= 1;
        }
    }
}

#[cfg(all(test, feature = "prng-trace"))]
mod trace_tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    #[test]
    fn draws_are_reported_with_caller() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let l2 = log.clone();
        trace::set_hook(Some(Box::new(move |d: &trace::Draw| {
            l2.lock().unwrap().push((d.tag, d.index, d.value, d.location.line()))
        })));
        let mut p = PseudoRandom::new(123.0);
        let mut q = p.clone();
        p.set_trace_tag(7);
        assert_eq!(p.draw_count(), 0, "warm-up is not counted");
        let line = line!() + 1;
        let v = p.next_u32();
        p.next_int(0.0, 10.0);
        trace::set_hook(None);
        let log = log.lock().unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0], (7, 0, v, line));
        assert_eq!((log[1].0, log[1].1), (7, 1));
        assert_eq!(p.draw_count(), 2);
        // instrumentation does not change the stream or equality
        assert_eq!(q.next_u32(), v);
        q.next_int(0.0, 10.0);
        assert_eq!(p, q);
    }
}
