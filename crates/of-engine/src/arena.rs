//! `Arena<T, H>`: generational slots with a LIFO free list and a mark-sweep hook
//! (ARCHITECTURE §3.2, §3.11). Used for units, attacks, rail clusters and railroads.
//!
//! - A "deleted" TS object stays allocated (and readable) for as long as anything can
//!   reach it: TS executions keep reading deleted units (`NukeExecution.nuke`, …). Only
//!   the v1 GC frees slots, via [`Arena::sweep`], for objects that are both inactive and
//!   unmarked.
//! - Slot positions are **never observable**: CS keys and JS map keys use TS ids
//!   (PORTING_GUIDE §5.1). Iterating an arena in slot order ([`Arena::iter_slots`]) is
//!   for GC/debug only, never for simulation order.
//! - Every access checks the handle's generation. A mismatch (a stale handle into a
//!   reused slot) is a Rust bug: a plain panic → `Fatal::Bug`, never a TS behaviour.
//!
//! Owner: **P1** (FILE_MAP §2 foundations).

use crate::StateClone;
use crate::ids::ArenaHandle;
use crate::plain::PlainState;
use std::marker::PhantomData;

impl<T> PlainState for PhantomData<T> {}

#[derive(StateClone, Debug)]
struct Slot<T> {
    generation: u32,
    val: Option<T>,
}

/// Generational arena of `T`, addressed by handle type `H`.
#[derive(StateClone, Debug)]
#[refs(manual)]
pub struct Arena<T, H> {
    slots: Vec<Slot<T>>,
    /// LIFO free list (deterministic slot reuse; reuse is unobservable anyway).
    free: Vec<u32>,
    live: u32,
    _h: PhantomData<H>,
}

/// An arena is storage, not a reference: its objects are reached by handle (the GC worklist,
/// the CS drain queues), so as a field it reports nothing (canon/refs.rs).
impl<T, H> crate::canon::Refs for Arena<T, H> {
    #[inline]
    fn visit_refs(&self, _v: &mut dyn crate::canon::RefVisitor) {}
}

impl<T: Clone + PlainState, H: ArenaHandle + PlainState> Default for Arena<T, H> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone + PlainState, H: ArenaHandle + PlainState> Arena<T, H> {
    pub fn new() -> Self {
        Arena { slots: Vec::new(), free: Vec::new(), live: 0, _h: PhantomData }
    }

    /// Number of allocated (not freed) objects, including TS-deleted ones not yet swept.
    #[inline]
    pub fn len(&self) -> usize {
        self.live as usize
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.live == 0
    }
    /// Slot count (allocated + free), the size of a [`Marks`] for this arena.
    #[inline]
    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }

    /// Store `val` and return its handle.
    pub fn alloc(&mut self, val: T) -> H {
        self.live += 1;
        if let Some(slot) = self.free.pop() {
            let s = &mut self.slots[slot as usize]; // in-range: free list holds valid slots
            debug_assert!(s.val.is_none());
            s.val = Some(val);
            H::from_parts(slot, s.generation)
        } else {
            let slot = u32::try_from(self.slots.len()).expect("arena slot fits u32");
            self.slots.push(Slot { generation: 0, val: Some(val) });
            H::from_parts(slot, 0)
        }
    }

    #[cold]
    #[inline(never)]
    fn stale(h: H) -> ! {
        panic!("of-engine arena: stale or freed handle {h:?} (Rust bug, not a TS behaviour)")
    }

    /// The object behind `h`. Panics (Rust bug) on a stale handle.
    #[inline]
    pub fn get(&self, h: H) -> &T {
        match self.slots.get(h.slot() as usize) {
            Some(Slot { generation, val: Some(v) }) if *generation == h.generation() => v,
            _ => Self::stale(h),
        }
    }
    #[inline]
    pub fn get_mut(&mut self, h: H) -> &mut T {
        match self.slots.get_mut(h.slot() as usize) {
            Some(Slot { generation, val: Some(v) }) if *generation == h.generation() => v,
            _ => Self::stale(h),
        }
    }
    /// `None` for a stale/freed handle (for assertions and the GC; sim code uses `get`).
    #[inline]
    pub fn try_get(&self, h: H) -> Option<&T> {
        match self.slots.get(h.slot() as usize) {
            Some(Slot { generation, val: Some(v) }) if *generation == h.generation() => Some(v),
            _ => None,
        }
    }
    #[inline]
    pub fn contains(&self, h: H) -> bool {
        self.try_get(h).is_some()
    }

    /// Free one object (GC only) and return it. The slot's generation advances, so any
    /// remaining handle to it is detected as stale.
    pub fn free(&mut self, h: H) -> T {
        let s = match self.slots.get_mut(h.slot() as usize) {
            Some(s) if s.generation == h.generation() && s.val.is_some() => s,
            _ => Self::stale(h),
        };
        let v = s.val.take().expect("checked above"); // rust-invariant: matched Some
        s.generation = s.generation.wrapping_add(1);
        self.free.push(h.slot());
        self.live -= 1;
        v
    }

    /// Mark-sweep (ARCHITECTURE §3.11): free every allocated object whose slot is not
    /// marked and for which `can_free` (the "inactive" test) holds. Slots are visited in
    /// ascending order, so the free list — and thus later slot reuse — is deterministic.
    /// Returns the number freed.
    pub fn sweep(&mut self, marks: &Marks, mut can_free: impl FnMut(&T) -> bool) -> usize {
        let mut n = 0;
        for i in 0..self.slots.len() {
            let s = &mut self.slots[i]; // in-range: i < len
            let free_it = match &s.val {
                Some(v) => !marks.get(i) && can_free(v),
                None => false,
            };
            if free_it {
                s.val = None;
                s.generation = s.generation.wrapping_add(1);
                self.free.push(i as u32);
                self.live -= 1;
                n += 1;
            }
        }
        n
    }

    /// Mutate every allocated object in **slot order**: unobservable maintenance only (e.g.
    /// end-of-tick `TileSet` compaction), never simulation logic.
    pub fn for_each_mut(&mut self, mut f: impl FnMut(&mut T)) {
        for s in &mut self.slots {
            if let Some(v) = s.val.as_mut() {
                f(v);
            }
        }
    }

    /// Allocated objects in **slot order** (not TS order): GC marking and debugging only.
    pub fn iter_slots(&self) -> impl Iterator<Item = (H, &T)> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.val.as_ref().map(|v| (H::from_parts(i as u32, s.generation), v)))
    }
}

/// A slot bitset for [`Arena::sweep`] (GC marks). Scratch-like: rebuilt per collection.
#[derive(Clone, Debug, Default)]
pub struct Marks {
    words: Vec<u64>,
}

impl Marks {
    /// Clear and size for `n` slots (keeps capacity).
    pub fn reset(&mut self, n: usize) {
        self.words.clear();
        self.words.resize(n.div_ceil(64), 0);
    }
    #[inline]
    pub fn set(&mut self, slot: u32) {
        let i = slot as usize;
        if let Some(w) = self.words.get_mut(i / 64) {
            *w |= 1 << (i % 64);
        } else {
            panic!("Marks::set: slot {slot} beyond reset size");
        }
    }
    /// Mark the slot of a handle.
    #[inline]
    pub fn mark<H: ArenaHandle>(&mut self, h: H) {
        self.set(h.slot());
    }
    #[inline]
    pub fn get(&self, slot: usize) -> bool {
        self.words.get(slot / 64).is_some_and(|w| w & (1 << (slot % 64)) != 0)
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // tests may catch panics to assert them
mod tests {
    use super::*;
    use crate::ids::UnitRef;

    #[test]
    fn alloc_free_reuse_and_stale_detection() {
        let mut a: Arena<String, UnitRef> = Arena::new();
        let x = a.alloc("x".into());
        let y = a.alloc("y".into());
        assert_eq!((a.get(x).as_str(), a.get(y).as_str(), a.len()), ("x", "y", 2));
        assert_eq!(a.free(x), "x");
        assert!(!a.contains(x));
        let z = a.alloc("z".into()); // LIFO reuse of x's slot, new generation
        assert_eq!(z.slot, x.slot);
        assert_ne!(z.generation, x.generation);
        assert_eq!(a.get(z), "z");
        assert!(std::panic::catch_unwind(|| a.get(x).len()).is_err());
    }

    #[test]
    fn sweep_frees_unmarked_inactive_only() {
        let mut a: Arena<(u32, bool), UnitRef> = Arena::new(); // (id, active)
        let hs: Vec<_> = (0..6).map(|i| a.alloc((i, i % 2 == 0))).collect();
        let mut m = Marks::default();
        m.reset(a.slot_count());
        m.mark(hs[1]); // inactive but referenced: kept
        let n = a.sweep(&m, |&(_, active)| !active);
        assert_eq!(n, 2); // ids 3 and 5
        assert!(a.contains(hs[1]) && !a.contains(hs[3]) && !a.contains(hs[5]));
        assert_eq!(a.len(), 4);
        // deterministic reuse: highest freed slot first (LIFO over ascending sweep)
        assert_eq!(a.alloc((9, true)).slot, hs[5].slot);
    }

    #[test]
    fn clone_from_reuses_slots_vec() {
        let mut src: Arena<Vec<u32>, UnitRef> = Arena::new();
        for i in 0..10 {
            src.alloc(vec![i; 4]);
        }
        let mut dst: Arena<Vec<u32>, UnitRef> = Arena::new();
        for _ in 0..20 {
            dst.alloc(vec![0; 64]);
        }
        let p = dst.slots.as_ptr();
        dst.clone_from(&src);
        assert_eq!(dst.slots.as_ptr(), p);
        assert_eq!(dst.len(), 10);
        assert_eq!(dst.iter_slots().map(|(_, v)| v[0]).collect::<Vec<_>>(), (0..10).collect::<Vec<_>>());
    }
}
