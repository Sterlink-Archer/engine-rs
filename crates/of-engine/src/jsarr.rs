//! `JsArr<T>`: a TS array **field** that TS deletes from by reassignment,
//! `this.x = this.x.filter(..)` (ARCHITECTURE §4.7, PORTING_GUIDE §5.1/§5.3).
//!
//! JS semantics being reproduced: a `for..of` over the array keeps walking the array
//! **object** it started on. Pushes to that same object are visited; a reassignment makes
//! the field point at a *new* object, so the running loop keeps visiting every element of
//! the old one, unchanged, and never sees later pushes (they go to the new object).
//! A Rust `retain` under a cursor that re-reads the length would instead skip the element
//! after each removal — the review-0 #1 divergence.
//!
//! Known fields: `Player.units`, `Player.incoming_attacks`, `Player.outgoing_attacks`,
//! `Player.alliances`, `State.alliance_requests` (`UnitImpl.ts:275,353`;
//! `AttackImpl.ts:61-71`; `GameImpl.ts:469,507,1143-1144`).
//!
//! Rules:
//! - removal is **only** [`JsArr::filter_reassign`] (never a bare `retain`/`remove`);
//! - a loop whose body can (transitively) remove from the field iterates
//!   [`JsArr::iter_snapshot`], annotated `// iter-snapshot: filter-reassign`;
//! - a loop whose body provably cannot remove may index the live slice, annotated
//!   `// iter-live: <reason>`.
//!
//! The one corner a snapshot cannot see is a push **followed by** a reassignment inside a
//! single loop step (the push went to the old object, which the snapshot never read).
//! A site where that is possible needs a written proof or custom code, annotated
//! `// iter-snapshot: push-then-reassign <argument>`.
//!
//! Owner: **P1** (FILE_MAP §2 foundations).

use crate::StateClone;
use crate::ids::{AllianceId, AttackRef, ReqIdx, UnitRef};
use crate::plain::PlainState;

/// A JS array field that is filter-reassigned. `generation` counts reassignments: equal `generation`
/// means "the same JS array object".
#[derive(StateClone, Debug, PartialEq, Eq)]
pub struct JsArr<T> {
    v: Vec<T>,
    generation: u32,
}

impl<T> Default for JsArr<T> {
    fn default() -> Self {
        JsArr { v: Vec::new(), generation: 0 }
    }
}

impl<T: Copy + PartialEq + PlainState> JsArr<T> {
    pub fn new() -> Self {
        JsArr { v: Vec::new(), generation: 0 }
    }
    /// `arr.push(x)`: mutates the current array object.
    #[inline]
    pub fn push(&mut self, x: T) {
        self.v.push(x);
    }
    /// `this.x = this.x.filter(keep)`: a NEW array object (order kept).
    pub fn filter_reassign(&mut self, mut keep: impl FnMut(&T) -> bool) {
        self.v.retain(|x| keep(x));
        self.generation = self.generation.wrapping_add(1);
    }
    /// `this.x = <new array>` with the given contents (also a new object).
    pub fn reassign(&mut self, items: impl IntoIterator<Item = T>) {
        self.v.clear();
        self.v.extend(items);
        self.generation = self.generation.wrapping_add(1);
    }
    #[inline]
    pub fn len(&self) -> usize {
        self.v.len()
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.v.is_empty()
    }
    /// `arr[i]`: `None` out of range (JS `undefined`).
    #[inline]
    pub fn get(&self, i: usize) -> Option<T> {
        self.v.get(i).copied()
    }
    /// The live array (read-only). Index loops over it re-read `len()` per step.
    #[inline]
    pub fn as_slice(&self) -> &[T] {
        &self.v
    }
    /// `arr.includes(x)` / `arr.indexOf(x) !== -1` for handle types (`===` on values).
    #[inline]
    pub fn contains(&self, x: &T) -> bool {
        self.v.contains(x)
    }
    /// How many reassignments this field has seen (identity of the current JS object).
    #[inline]
    pub fn generation(&self) -> u32 {
        self.generation
    }

    /// Start a `for (const x of this.field)` over the current array object, for a loop
    /// whose body may reassign the field. Takes a buffer from `pool`; return it with
    /// [`SnapIter::finish`].
    pub fn iter_snapshot(&self, pool: &mut SnapPool) -> SnapIter<T>
    where
        T: SnapElem,
    {
        let mut buf = pool.take();
        buf.extend(self.v.iter().map(|x| x.to_bits()));
        SnapIter { buf, i: 0, gen0: self.generation, _t: std::marker::PhantomData }
    }
}

/// Element types that can live in the shared snapshot pool (handles: `Copy`, ≤ 64 bits).
pub trait SnapElem: Copy {
    fn to_bits(self) -> u64;
    fn from_bits(b: u64) -> Self;
}

impl SnapElem for UnitRef {
    #[inline]
    fn to_bits(self) -> u64 {
        ((self.generation as u64) << 32) | self.slot as u64
    }
    #[inline]
    fn from_bits(b: u64) -> Self {
        UnitRef { slot: b as u32, generation: (b >> 32) as u32 }
    }
}
impl SnapElem for AttackRef {
    #[inline]
    fn to_bits(self) -> u64 {
        ((self.generation as u64) << 32) | self.slot as u64
    }
    #[inline]
    fn from_bits(b: u64) -> Self {
        AttackRef { slot: b as u32, generation: (b >> 32) as u32 }
    }
}
impl SnapElem for AllianceId {
    #[inline]
    fn to_bits(self) -> u64 {
        self.0 as u64
    }
    #[inline]
    fn from_bits(b: u64) -> Self {
        AllianceId(b as u32)
    }
}
impl SnapElem for ReqIdx {
    #[inline]
    fn to_bits(self) -> u64 {
        self.0 as u64
    }
    #[inline]
    fn from_bits(b: u64) -> Self {
        ReqIdx(b as u32)
    }
}
impl SnapElem for u32 {
    #[inline]
    fn to_bits(self) -> u64 {
        self as u64
    }
    #[inline]
    fn from_bits(b: u64) -> Self {
        b as u32
    }
}

/// Reusable snapshot buffers (lives in `Scratch` as `sc.snap_pool`; never cloned).
#[derive(Debug, Default)]
pub struct SnapPool {
    free: Vec<Vec<u64>>,
}

impl SnapPool {
    fn take(&mut self) -> Vec<u64> {
        self.free.pop().unwrap_or_default()
    }
    fn give(&mut self, mut b: Vec<u64>) {
        b.clear();
        self.free.push(b);
    }
}

/// A running `for..of` over one JS array object (see [`JsArr::iter_snapshot`]).
#[must_use = "call finish() to return the buffer to the pool"]
pub struct SnapIter<T> {
    buf: Vec<u64>,
    i: usize,
    gen0: u32,
    _t: std::marker::PhantomData<T>,
}

impl<T: SnapElem + PartialEq + PlainState> SnapIter<T> {
    /// The next element of the array object the loop started on. While `arr` has not been
    /// reassigned since the start (`generation == gen0`), it *is* that object, so elements pushed
    /// to it during the loop are picked up here, exactly as JS visits them.
    pub fn next(&mut self, arr: &JsArr<T>) -> Option<T> {
        if arr.generation == self.gen0 && arr.v.len() > self.buf.len() {
            let n = self.buf.len();
            self.buf.extend(arr.v[n..].iter().map(|x| x.to_bits()));
        }
        let b = *self.buf.get(self.i)?;
        self.i += 1;
        Some(T::from_bits(b))
    }
    /// End the loop (also after an early `break`) and recycle the buffer.
    pub fn finish(self, pool: &mut SnapPool) {
        pool.give(self.buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(slot: u32) -> UnitRef {
        UnitRef { slot, generation: 7 }
    }

    /// JS: `for (const x of p.units) { if (x === 1) p.units = p.units.filter(y => y !== 2) }`
    /// visits 0,1,2,3 (the old array object), not 0,1,3.
    #[test]
    fn delete_during_loop_visits_old_array() {
        let mut arr: JsArr<UnitRef> = JsArr::new();
        for i in 0..4 {
            arr.push(u(i));
        }
        let mut pool = SnapPool::default();
        let mut it = arr.iter_snapshot(&mut pool);
        let mut seen = vec![];
        while let Some(x) = it.next(&arr) {
            seen.push(x.slot);
            if x.slot == 1 {
                arr.filter_reassign(|y| y.slot != 2);
            }
            if x.slot == 2 {
                arr.filter_reassign(|y| y.slot != 0); // delete an already-visited one too
            }
        }
        it.finish(&mut pool);
        assert_eq!(seen, [0, 1, 2, 3]);
        assert_eq!(arr.as_slice(), &[u(1), u(3)]);
        assert_eq!(arr.generation(), 2);
    }

    /// JS: pushes to the SAME array object during `for..of` are visited
    /// (e.g. `units.push(captured)` while iterating, no reassignment yet).
    #[test]
    fn push_before_reassign_is_visited_push_after_is_not() {
        let mut arr: JsArr<u32> = JsArr::new();
        arr.push(10);
        arr.push(11);
        let mut pool = SnapPool::default();
        let mut it = arr.iter_snapshot(&mut pool);
        let mut seen = vec![];
        while let Some(x) = it.next(&arr) {
            seen.push(x);
            match x {
                10 => arr.push(12),                      // same object: visited
                11 => arr.filter_reassign(|&y| y != 10), // new object [11, 12]
                12 => arr.push(13),                      // pushed to the NEW object: not visited
                _ => {}
            }
        }
        it.finish(&mut pool);
        assert_eq!(seen, [10, 11, 12]);
        assert_eq!(arr.as_slice(), &[11, 12, 13]);
    }

    #[test]
    fn pool_recycles_and_nested_loops_are_independent() {
        let mut arr: JsArr<u32> = JsArr::new();
        arr.reassign([1, 2, 3]);
        let mut pool = SnapPool::default();
        let mut outer = arr.iter_snapshot(&mut pool);
        let mut pairs = vec![];
        while let Some(a) = outer.next(&arr) {
            let mut inner = arr.iter_snapshot(&mut pool);
            while let Some(b) = inner.next(&arr) {
                pairs.push((a, b));
                if a == 1 && b == 1 {
                    arr.filter_reassign(|&y| y != 3);
                }
            }
            inner.finish(&mut pool);
        }
        outer.finish(&mut pool);
        // outer walks [1,2,3]; inner #1 walks [1,2,3] (reassigned during it), later inners [1,2]
        assert_eq!(pairs, [(1, 1), (1, 2), (1, 3), (2, 1), (2, 2), (3, 1), (3, 2)]);
        assert_eq!(pool.free.len(), 2);
    }
}
