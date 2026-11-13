//! Insertion-ordered `Map` / `Set` with ECMAScript semantics, including LIVE iteration
//! while the collection is mutated (`for (const x of set) { set.delete(..); set.add(..) }`),
//! plus `Object.keys` ordering (`OrdinaryOwnPropertyKeys`).
//!
//! Semantics (checked by `of-vectors`, order suite, and by the unit tests below, whose
//! expected values were produced with node v24.21.0 on 2026-11-05):
//! - `set(k, v)` on an existing key keeps its position; `delete` + `set` moves it to the end.
//! - Iteration visits entries in insertion order; entries added during iteration ARE
//!   visited; entries deleted before being reached are skipped; deleting and re-adding
//!   the current entry re-queues it at the end.
//! - **`clear()` does not end a live iteration**: the iterator continues with whatever is
//!   added after the clear (`new Set([1,2,3])`, clear + add 9 + add 10 inside the first
//!   loop body, visits `1,9,10`). ARCHITECTURE §12 G7(a), PORTING_GUIDE §5.2 item 1.
//! - An iteration that has returned "done" stays done, even if entries are added later
//!   (V8 detaches an exhausted iterator from its table).
//!
//! Layout: an entry vector with tombstones + a lookup index. A [`Cursor`] is a position
//! in the entry vector. Compaction (dropping tombstones) happens only inside `insert`
//! (heap mode: when tombstones outnumber live entries; inline mode: when the inline
//! array is full). It bumps an epoch and logs how positions moved; a cursor from an
//! earlier epoch is remapped through the logged moves on its next use, so it stays valid
//! across any sequence of mutations (as V8's ordered hash tables do). The log keeps the
//! last [`MAX_REMAPS`] compactions and is dropped by [`OrdMap::end_tick`] and by clones
//! (cursors never outlive a tick). A cursor older than the log is a Rust-side bug and
//! panics loudly (never a silent early finish).
//!
//! One type serves both `JsMap` (`N = 0`: heap storage, hashed index) and `SmallJsMap`
//! (`N > 0`: up to `N` slots inline with linear search, spilling to heap + index above
//! that; ARCHITECTURE §8.2). Spilling preserves slot positions, so it is invisible to
//! cursors. The index uses the inline Fx hasher by default ([`crate::fx`]); it is never
//! iterated, so the hasher cannot affect any order.

use crate::fx::FxBuildHasher;
use std::collections::HashMap;
use std::hash::{BuildHasher, Hash};

/// How many compactions a suspended cursor may lag behind before it is reported as a bug.
pub const MAX_REMAPS: usize = 8;

/// Heap-mode compaction threshold (unchanged from the vector-checked original).
const HEAP_MIN_TOMBSTONES: usize = 16;

pub struct OrdMap<K, V, S = FxBuildHasher, const N: usize = 0> {
    inline: [Option<(K, V)>; N],
    /// Used inline slots (live + tombstones); 0 once spilled.
    inline_len: usize,
    heap: Vec<Option<(K, V)>>,
    /// Always true when `N == 0`.
    spilled: bool,
    /// key -> slot position; maintained only while spilled.
    index: HashMap<K, usize, S>,
    live: usize,
    epoch: u64,
    /// `remaps[i]` maps the slot positions of epoch `epoch - remaps.len() + i` to the next
    /// epoch: `remap[old_pos]` = number of live entries before `old_pos` (length old+1).
    remaps: Vec<Vec<u32>>,
}

/// Live-iteration cursor (see module docs). `Default` is a fresh cursor at position 0,
/// epoch 0, which is only valid for a map that never compacted; prefer [`OrdMap::cursor`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cursor {
    pos: usize,
    epoch: u64,
    done: bool,
}

impl<K: Hash + Eq + Clone, V, S: BuildHasher + Default, const N: usize> Default for OrdMap<K, V, S, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Hash + Eq + Clone, V, S: BuildHasher + Default, const N: usize> OrdMap<K, V, S, N> {
    pub fn new() -> Self {
        OrdMap {
            inline: std::array::from_fn(|_| None),
            inline_len: 0,
            heap: Vec::new(),
            spilled: N == 0,
            index: HashMap::with_hasher(S::default()),
            live: 0,
            epoch: 0,
            remaps: Vec::new(),
        }
    }

    #[inline]
    fn is_heap(&self) -> bool {
        N == 0 || self.spilled
    }
    #[inline]
    fn slots(&self) -> &[Option<(K, V)>] {
        if self.is_heap() { &self.heap } else { &self.inline[..self.inline_len] }
    }
    #[inline]
    fn slots_mut(&mut self) -> &mut [Option<(K, V)>] {
        if self.is_heap() { &mut self.heap } else { &mut self.inline[..self.inline_len] }
    }
    #[inline]
    fn find(&self, k: &K) -> Option<usize> {
        if self.is_heap() {
            self.index.get(k).copied()
        } else {
            self.inline[..self.inline_len].iter().position(|e| matches!(e, Some((kk, _)) if kk == k))
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.live
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.live == 0
    }
    /// True once the map no longer fits inline (always true for `N == 0`).
    #[inline]
    pub fn is_spilled(&self) -> bool {
        self.is_heap()
    }
    #[inline]
    pub fn contains_key(&self, k: &K) -> bool {
        self.find(k).is_some()
    }
    pub fn get(&self, k: &K) -> Option<&V> {
        let i = self.find(k)?;
        self.slots()[i].as_ref().map(|(_, v)| v)
    }
    pub fn get_mut(&mut self, k: &K) -> Option<&mut V> {
        let i = self.find(k)?;
        self.slots_mut()[i].as_mut().map(|(_, v)| v)
    }

    /// `map.set(k, v)`. Returns the previous value (an existing key keeps its position).
    pub fn insert(&mut self, k: K, v: V) -> Option<V> {
        if let Some(i) = self.find(&k) {
            let slot = self.slots_mut()[i].as_mut().expect("index points at a live slot");
            return Some(std::mem::replace(&mut slot.1, v));
        }
        if self.is_heap() {
            let tomb = self.heap.len() - self.live;
            if tomb > HEAP_MIN_TOMBSTONES && tomb > self.live {
                self.compact_heap();
            }
        } else if self.inline_len == N {
            if self.live < N {
                self.compact_inline();
            } else {
                self.spill();
            }
        }
        if self.is_heap() {
            self.index.insert(k.clone(), self.heap.len());
            self.heap.push(Some((k, v)));
        } else {
            self.inline[self.inline_len] = Some((k, v));
            self.inline_len += 1;
        }
        self.live += 1;
        None
    }

    /// `map.delete(k)`. Returns the removed value (JS returns `true` iff present).
    pub fn remove(&mut self, k: &K) -> Option<V> {
        let i = self.find(k)?;
        if self.is_heap() {
            self.index.remove(k);
        }
        self.live -= 1;
        self.slots_mut()[i].take().map(|(_, v)| v)
    }

    /// `map.clear()`. Every entry becomes a tombstone, so a live cursor keeps its place
    /// and goes on to visit entries inserted after the clear (ECMAScript semantics).
    pub fn clear(&mut self) {
        for s in self.slots_mut() {
            *s = None;
        }
        self.index.clear();
        self.live = 0;
    }

    fn push_remap(&mut self, r: Vec<u32>) {
        if self.remaps.len() == MAX_REMAPS {
            self.remaps.remove(0);
        }
        self.remaps.push(r);
        self.epoch += 1;
    }

    fn compact_heap(&mut self) {
        let mut remap = Vec::with_capacity(self.heap.len() + 1);
        let mut out = Vec::with_capacity(self.live);
        let mut n = 0u32;
        for e in self.heap.drain(..) {
            remap.push(n);
            if let Some((k, v)) = e {
                self.index.insert(k.clone(), n as usize);
                out.push(Some((k, v)));
                n += 1;
            }
        }
        remap.push(n);
        self.heap = out;
        self.push_remap(remap);
    }

    fn compact_inline(&mut self) {
        let mut remap = Vec::with_capacity(self.inline_len + 1);
        let mut n = 0usize;
        for i in 0..self.inline_len {
            remap.push(n as u32);
            if self.inline[i].is_some() {
                if n != i {
                    self.inline[n] = self.inline[i].take();
                }
                n += 1;
            }
        }
        remap.push(n as u32);
        self.inline_len = n;
        self.push_remap(remap);
    }

    /// Move the inline slots (tombstones included, so positions do not change) to the heap.
    fn spill(&mut self) {
        let mut heap = Vec::with_capacity((2 * N).max(4));
        for i in 0..self.inline_len {
            let e = self.inline[i].take();
            if let Some((k, _)) = &e {
                self.index.insert(k.clone(), i);
            }
            heap.push(e);
        }
        self.heap = heap;
        self.inline_len = 0;
        self.spilled = true;
    }

    /// Drop the compaction log. Call at a tick boundary: cursors never outlive a tick.
    pub fn end_tick(&mut self) {
        self.remaps.clear();
    }

    /// Iterate live entries in order (no mutation possible during this borrow).
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.slots().iter().filter_map(|e| e.as_ref().map(|(k, v)| (k, v)))
    }
    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.iter().map(|(k, _)| k)
    }
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.iter().map(|(_, v)| v)
    }
    /// Mutate values in place, in order (keys and order cannot change).
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut V> {
        self.slots_mut().iter_mut().filter_map(|e| e.as_mut().map(|(_, v)| v))
    }
    /// The first live key (`map.keys().next().value`), e.g. the LRU eviction victim of
    /// `WaterPathMemo` (`pathfinding/PathFinder.ts:79-138`). Ledger t088.
    pub fn first_live(&self) -> Option<&K> {
        self.keys().next()
    }

    /// Start a live iteration.
    pub fn cursor(&self) -> Cursor {
        Cursor { pos: 0, epoch: self.epoch, done: false }
    }

    fn resync(&self, c: &mut Cursor) {
        if c.epoch == self.epoch {
            return;
        }
        assert!(c.epoch < self.epoch, "of-det OrdMap: cursor from another map (epoch {} > {})", c.epoch, self.epoch);
        let lag = (self.epoch - c.epoch) as usize;
        assert!(
            lag <= self.remaps.len(),
            "of-det OrdMap: stale cursor lags {lag} compactions but only {} are logged \
             (cursors must not outlive end_tick() or more than {MAX_REMAPS} compactions)",
            self.remaps.len()
        );
        for r in &self.remaps[self.remaps.len() - lag..] {
            c.pos = r.get(c.pos).copied().unwrap_or(*r.last().expect("remap has len+1 entries")) as usize;
        }
        c.epoch = self.epoch;
    }

    /// Advance a live iteration: returns the next live entry's key (cloned) and moves past it.
    pub fn next_live(&self, c: &mut Cursor) -> Option<K> {
        if c.done {
            return None;
        }
        self.resync(c);
        let slots = self.slots();
        while c.pos < slots.len() {
            let i = c.pos;
            c.pos += 1;
            if let Some((k, _)) = &slots[i] {
                return Some(k.clone());
            }
        }
        c.done = true;
        None
    }
}

impl<K: Clone, V: Clone, S: Clone, const N: usize> Clone for OrdMap<K, V, S, N> {
    fn clone(&self) -> Self {
        OrdMap {
            inline: self.inline.clone(),
            inline_len: self.inline_len,
            heap: self.heap.clone(),
            spilled: self.spilled,
            index: self.index.clone(),
            live: self.live,
            epoch: self.epoch,
            remaps: Vec::new(), // not carried by clones (ARCHITECTURE §12 G7(c))
        }
    }
    /// Reuses `heap`/`index` capacity (pooled forks, ARCHITECTURE §8.1). Ledger t088.
    fn clone_from(&mut self, src: &Self) {
        self.inline.clone_from_slice(&src.inline);
        self.inline_len = src.inline_len;
        self.heap.clone_from(&src.heap);
        self.spilled = src.spilled;
        self.index.clone_from(&src.index);
        self.live = src.live;
        self.epoch = src.epoch;
        self.remaps.clear();
    }
}

impl<K: std::fmt::Debug, V: std::fmt::Debug, S, const N: usize> std::fmt::Debug for OrdMap<K, V, S, N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let slots: &[Option<(K, V)>] =
            if N == 0 || self.spilled { &self.heap } else { &self.inline[..self.inline_len] };
        f.debug_map().entries(slots.iter().filter_map(|e| e.as_ref().map(|(k, v)| (k, v)))).finish()
    }
}

/// Insertion-ordered `Set` (same semantics as [`OrdMap`]).
pub struct OrdSet<K, S = FxBuildHasher, const N: usize = 0> {
    map: OrdMap<K, (), S, N>,
}

impl<K: Clone, S: Clone, const N: usize> Clone for OrdSet<K, S, N> {
    fn clone(&self) -> Self {
        OrdSet { map: self.map.clone() }
    }
    fn clone_from(&mut self, src: &Self) {
        self.map.clone_from(&src.map);
    }
}

impl<K: std::fmt::Debug, S, const N: usize> std::fmt::Debug for OrdSet<K, S, N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let m = &self.map;
        let slots: &[Option<(K, ())>] = if N == 0 || m.spilled { &m.heap } else { &m.inline[..m.inline_len] };
        f.debug_set().entries(slots.iter().filter_map(|e| e.as_ref().map(|(k, _)| k))).finish()
    }
}

impl<K: Hash + Eq + Clone, S: BuildHasher + Default, const N: usize> Default for OrdSet<K, S, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Hash + Eq + Clone, S: BuildHasher + Default, const N: usize> OrdSet<K, S, N> {
    pub fn new() -> Self {
        OrdSet { map: OrdMap::new() }
    }
    #[inline]
    pub fn len(&self) -> usize {
        self.map.len()
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
    #[inline]
    pub fn is_spilled(&self) -> bool {
        self.map.is_spilled()
    }
    #[inline]
    pub fn contains(&self, k: &K) -> bool {
        self.map.contains_key(k)
    }
    /// `set.add(k)`; returns true if newly inserted.
    pub fn insert(&mut self, k: K) -> bool {
        self.map.insert(k, ()).is_none()
    }
    /// `set.delete(k)`.
    pub fn remove(&mut self, k: &K) -> bool {
        self.map.remove(k).is_some()
    }
    /// `set.clear()`; live cursors continue (see [`OrdMap::clear`]).
    pub fn clear(&mut self) {
        self.map.clear()
    }
    pub fn end_tick(&mut self) {
        self.map.end_tick()
    }
    pub fn iter(&self) -> impl Iterator<Item = &K> {
        self.map.keys()
    }
    pub fn first_live(&self) -> Option<&K> {
        self.map.first_live()
    }
    pub fn cursor(&self) -> Cursor {
        self.map.cursor()
    }
    pub fn next_live(&self, c: &mut Cursor) -> Option<K> {
        self.map.next_live(c)
    }
}

impl<K: Hash + Eq + Clone, S: BuildHasher + Default, const N: usize> FromIterator<K> for OrdSet<K, S, N> {
    fn from_iter<I: IntoIterator<Item = K>>(it: I) -> Self {
        let mut s = OrdSet::new();
        for k in it {
            s.insert(k);
        }
        s
    }
}

impl<K: Hash + Eq + Clone, V, S: BuildHasher + Default, const N: usize> FromIterator<(K, V)> for OrdMap<K, V, S, N> {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(it: I) -> Self {
        let mut m = OrdMap::new();
        for (k, v) in it {
            m.insert(k, v);
        }
        m
    }
}

/// Is `s` an array index (canonical numeric string of a uint32 < 2^32 - 1)?
pub fn is_array_index(s: &str) -> Option<u32> {
    if s.is_empty() || s.len() > 10 {
        return None;
    }
    if s.len() > 1 && s.starts_with('0') {
        return None;
    }
    if !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let v: u64 = s.parse().ok()?;
    if v < 4_294_967_295 { Some(v as u32) } else { None }
}

/// `Object.keys(o)` order for an object whose keys were assigned in `insertion` order
/// (repeats allowed): array indices ascending first, then string keys by first insertion.
pub fn object_keys_order<S: AsRef<str>>(insertion: &[S]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut idx: Vec<(u32, String)> = Vec::new();
    let mut strs: Vec<String> = Vec::new();
    for k in insertion {
        let k = k.as_ref();
        if !seen.insert(k.to_string()) {
            continue;
        }
        match is_array_index(k) {
            Some(n) => idx.push((n, k.to_string())),
            None => strs.push(k.to_string()),
        }
    }
    idx.sort_by_key(|(n, _)| *n); // array indices are distinct: a stable/unstable sort agree
    idx.into_iter().map(|(_, s)| s).chain(strs).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    type Small<K, const N: usize> = OrdSet<K, FxBuildHasher, N>;

    /// Drive a live iteration; `body(set, visited_value, step)` may mutate.
    fn run<const N: usize>(s: &mut Small<i64, N>, mut body: impl FnMut(&mut Small<i64, N>, i64, usize)) -> Vec<i64> {
        let mut c = s.cursor();
        let mut seen = vec![];
        while let Some(v) = s.next_live(&mut c) {
            seen.push(v);
            body(s, v, seen.len());
            assert!(seen.len() < 10_000, "runaway iteration");
        }
        seen
    }

    fn live_iteration_with_compaction_n<const N: usize>() {
        let mut s: Small<i64, N> = (0..40).collect();
        let mut done = false;
        let seen = run(&mut s, |s, v, _| {
            if v == 5 && !done {
                done = true;
                for k in 6..40 {
                    s.remove(&k);
                }
                for k in 100..110 {
                    s.insert(k); // heap: first insert compacts (34 tombstones > 6 live)
                }
                s.remove(&3); // already visited
                s.remove(&5); // current
                s.insert(5); // re-queued at the end
            }
        });
        let want: Vec<i64> = (0..6).chain(100..110).chain([5]).collect();
        assert_eq!(seen, want);
        let fin: Vec<i64> = s.iter().copied().collect();
        assert_eq!(fin, [0, 1, 2, 4].into_iter().chain(100..110).chain([5]).collect::<Vec<_>>());
    }

    #[test]
    fn live_iteration_with_compaction() {
        live_iteration_with_compaction_n::<0>();
        live_iteration_with_compaction_n::<4>();
        live_iteration_with_compaction_n::<64>();
    }

    /// node: `s=new Set([1,2,3]); for (x of s) { first: s.clear(); s.add(9); s.add(10) }` -> 1,9,10
    fn clear_keeps_cursor_n<const N: usize>() {
        let mut s: Small<i64, N> = [1, 2, 3].into_iter().collect();
        let seen = run(&mut s, |s, _, step| {
            if step == 1 {
                s.clear();
                s.insert(9);
                s.insert(10);
            }
        });
        assert_eq!(seen, [1, 9, 10]);
        // node: Set([1,2,3,4]), at x===2 (once): clear(); add(2); add(5) -> visits 1,2,2,5; final 2,5
        let mut s: Small<i64, N> = [1, 2, 3, 4].into_iter().collect();
        let mut once = true;
        let seen = run(&mut s, |s, v, _| {
            if v == 2 && once {
                once = false;
                s.clear();
                s.insert(2);
                s.insert(5);
            }
        });
        assert_eq!(seen, [1, 2, 2, 5]);
        assert_eq!(s.iter().copied().collect::<Vec<_>>(), [2, 5]);
        // node: Map(0..39); at k===3 (once): delete 4..39, set 100..129, delete 100..124,
        // set 200..239, clear(), set 7 -> visits 0,1,2,3,7 (several compactions in one body)
        let mut s: Small<i64, N> = (0..40).collect();
        let mut once = true;
        let seen = run(&mut s, |s, v, _| {
            if v == 3 && once {
                once = false;
                for i in 4..40 {
                    s.remove(&i);
                }
                for i in 100..130 {
                    s.insert(i);
                }
                for i in 100..125 {
                    s.remove(&i);
                }
                for i in 200..240 {
                    s.insert(i);
                }
                s.clear();
                s.insert(7);
            }
        });
        assert_eq!(seen, [0, 1, 2, 3, 7]);
    }

    #[test]
    fn clear_keeps_cursor() {
        clear_keeps_cursor_n::<0>();
        clear_keeps_cursor_n::<2>();
        clear_keeps_cursor_n::<8>();
    }

    /// node: an exhausted Set iterator stays `{done: true}` after a later `add`.
    #[test]
    fn done_is_final() {
        let mut s: OrdSet<u32> = [1, 2].into_iter().collect();
        let mut c = s.cursor();
        while s.next_live(&mut c).is_some() {}
        s.insert(3);
        assert_eq!(s.next_live(&mut c), None);
    }

    #[test]
    fn small_spill_and_inline_compaction() {
        let mut m: OrdMap<u32, u32, FxBuildHasher, 4> = OrdMap::new();
        // churn inside the inline capacity never spills
        for round in 0..100u32 {
            m.insert(round, round);
            if round >= 2 {
                m.remove(&(round - 2));
            }
        }
        assert!(!m.is_spilled());
        assert_eq!(m.keys().copied().collect::<Vec<_>>(), [98, 99]);
        // a 5th live entry spills, order kept, lookups via the index
        for k in [1000, 1001, 1002] {
            m.insert(k, k);
        }
        assert!(m.is_spilled());
        assert_eq!(m.keys().copied().collect::<Vec<_>>(), [98, 99, 1000, 1001, 1002]);
        assert_eq!(m.get(&1001), Some(&1001));
        assert_eq!(m.insert(99, 7), Some(99));
        assert_eq!(m.keys().copied().collect::<Vec<_>>(), [98, 99, 1000, 1001, 1002]);
        assert_eq!(m.first_live(), Some(&98));
    }

    #[test]
    fn spill_during_iteration_keeps_cursor() {
        let mut s: Small<i64, 3> = [1, 2, 3].into_iter().collect();
        let seen = run(&mut s, |s, v, _| {
            if v == 1 {
                s.remove(&2);
                s.insert(4); // full inline with a tombstone: inline compaction (epoch bump)
                s.insert(5); // full, no tombstone: spill (positions kept)
            }
        });
        assert_eq!(seen, [1, 3, 4, 5]);
    }

    #[test]
    fn clone_from_reuses_capacity_and_drops_log() {
        let mut src: OrdMap<u32, u32> = (0..100).map(|i| (i, i)).collect();
        for i in 0..90 {
            src.remove(&i);
        }
        src.insert(500, 1); // compaction -> remap log entry
        let mut dst: OrdMap<u32, u32> = (0..1000).map(|i| (i, i)).collect();
        let cap = dst.heap.capacity();
        let ptr = dst.heap.as_ptr();
        dst.clone_from(&src);
        assert_eq!(dst.heap.as_ptr(), ptr);
        assert_eq!(dst.heap.capacity(), cap);
        assert!(dst.remaps.is_empty());
        assert_eq!(dst.keys().copied().collect::<Vec<_>>(), src.keys().copied().collect::<Vec<_>>());
        assert_eq!(dst.get(&95), Some(&95));
        assert_eq!(src.clone().remaps.len(), 0);
    }

    #[test]
    #[should_panic(expected = "stale cursor")]
    fn cursor_outliving_end_tick_is_loud() {
        let mut s: OrdSet<u32> = (0..40).collect();
        let mut c = s.cursor();
        s.next_live(&mut c);
        for i in 0..39 {
            s.remove(&i);
        }
        s.insert(100); // compaction
        s.end_tick();
        s.next_live(&mut c);
    }
}
