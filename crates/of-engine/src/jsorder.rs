//! Container vocabulary for JS `Map`/`Set` ports (PORTING_GUIDE §5.1).
//!
//! | TS | Rust |
//! |---|---|
//! | `Map`/`Set` that is ever iterated (incl. by the snapshot) | [`JsMap`] / [`JsSet`] (insertion order, live cursors) |
//! | … and per-entity, usually tiny | [`SmallJsMap`] / [`SmallJsSet`] (`N` inline slots) |
//! | `Map`/`Set` used only for `get/has/set/delete` | [`LookupMap`] / [`LookupSet`] (Fx-hashed, **no iteration API**), annotated `// lookup-only: <grep evidence>` |
//!
//! All of the ordered containers live in `of-det` (owned by the conformance workflow);
//! this module only re-exports them and adds the lookup-only wrappers.
//!
//! Owner: **P1** (FILE_MAP §2 foundations).

// This module is the sanctioned home of the std hash containers: they are wrapped with
// no iteration API (PORTING_GUIDE §12).
#![allow(clippy::disallowed_types)]

pub use of_det::ordmap::{Cursor, OrdMap, OrdSet};
pub use of_det::{JsMap, JsSet, SmallJsMap, SmallJsSet};

use crate::plain::PlainState;
use of_det::fx::FxBuildHasher;
use std::collections::{HashMap, HashSet};
use std::hash::Hash;

/// A lookup-only map (TS `Map` never iterated, never spread, never counted by order and not
/// in the snapshot). There is deliberately no way to iterate it, so hash order can never
/// reach the simulation.
#[derive(Debug)]
pub struct LookupMap<K, V> {
    m: HashMap<K, V, FxBuildHasher>,
}

impl<K: Clone, V: Clone> Clone for LookupMap<K, V> {
    fn clone(&self) -> Self {
        LookupMap { m: self.m.clone() }
    }
    fn clone_from(&mut self, src: &Self) {
        self.m.clone_from(&src.m);
    }
}

impl<K: PlainState, V: PlainState> PlainState for LookupMap<K, V> {}

/// GC pass only: marking is a set union, so hash order cannot leak. On the canon pass a
/// lookup map reports nothing (its order is not state; it is never in the snapshot).
impl<K: crate::canon::Refs, V: crate::canon::Refs> crate::canon::Refs for LookupMap<K, V> {
    fn visit_refs(&self, v: &mut dyn crate::canon::RefVisitor) {
        if v.canon_pass() {
            return;
        }
        #[allow(clippy::disallowed_methods)] // order-independent GC marking (see above)
        for (k, x) in self.m.iter() {
            k.visit_refs(v);
            x.visit_refs(v);
        }
    }
}

impl<K: Hash + Eq, V> Default for LookupMap<K, V> {
    fn default() -> Self {
        LookupMap { m: HashMap::default() }
    }
}

impl<K: Hash + Eq, V> LookupMap<K, V> {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_capacity(n: usize) -> Self {
        LookupMap { m: HashMap::with_capacity_and_hasher(n, FxBuildHasher::default()) }
    }
    #[inline]
    pub fn len(&self) -> usize {
        self.m.len()
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.m.is_empty()
    }
    #[inline]
    pub fn get(&self, k: &K) -> Option<&V> {
        self.m.get(k)
    }
    #[inline]
    pub fn get_mut(&mut self, k: &K) -> Option<&mut V> {
        self.m.get_mut(k)
    }
    #[inline]
    pub fn contains_key(&self, k: &K) -> bool {
        self.m.contains_key(k)
    }
    /// `map.set(k, v)`; returns the previous value.
    #[inline]
    pub fn insert(&mut self, k: K, v: V) -> Option<V> {
        self.m.insert(k, v)
    }
    /// `map.delete(k)`.
    #[inline]
    pub fn remove(&mut self, k: &K) -> Option<V> {
        self.m.remove(k)
    }
    /// `if (!map.has(k)) map.set(k, v)` with one hash probe (t409 H5); true if inserted.
    #[inline]
    pub fn insert_if_absent(&mut self, k: K, v: V) -> bool {
        match self.m.entry(k) {
            std::collections::hash_map::Entry::Occupied(_) => false,
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(v);
                true
            }
        }
    }
    pub fn clear(&mut self) {
        self.m.clear()
    }
}

/// A lookup-only set (see [`LookupMap`]).
#[derive(Debug)]
pub struct LookupSet<K> {
    s: HashSet<K, FxBuildHasher>,
}

impl<K: Clone> Clone for LookupSet<K> {
    fn clone(&self) -> Self {
        LookupSet { s: self.s.clone() }
    }
    fn clone_from(&mut self, src: &Self) {
        self.s.clone_from(&src.s);
    }
}

impl<K: PlainState> PlainState for LookupSet<K> {}

/// GC pass only (see `LookupMap`'s impl).
impl<K: crate::canon::Refs> crate::canon::Refs for LookupSet<K> {
    fn visit_refs(&self, v: &mut dyn crate::canon::RefVisitor) {
        if v.canon_pass() {
            return;
        }
        #[allow(clippy::disallowed_methods)] // order-independent GC marking
        for k in self.s.iter() {
            k.visit_refs(v);
        }
    }
}

impl<K: Hash + Eq> Default for LookupSet<K> {
    fn default() -> Self {
        LookupSet { s: HashSet::default() }
    }
}

impl<K: Hash + Eq> LookupSet<K> {
    pub fn new() -> Self {
        Self::default()
    }
    #[inline]
    pub fn len(&self) -> usize {
        self.s.len()
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.s.is_empty()
    }
    #[inline]
    pub fn contains(&self, k: &K) -> bool {
        self.s.contains(k)
    }
    /// `set.add(k)`; true if newly inserted.
    #[inline]
    pub fn insert(&mut self, k: K) -> bool {
        self.s.insert(k)
    }
    /// `set.delete(k)`.
    #[inline]
    pub fn remove(&mut self, k: &K) -> bool {
        self.s.remove(k)
    }
    pub fn clear(&mut self) {
        self.s.clear()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lookup_map_basics_and_clone_from() {
        let mut m: LookupMap<u32, u32> = LookupMap::new();
        assert_eq!(m.insert(1, 10), None);
        assert_eq!(m.insert(1, 11), Some(10));
        assert_eq!(m.get(&1), Some(&11));
        assert!(m.remove(&1).is_some() && m.is_empty());
        let mut src: LookupMap<u32, u32> = LookupMap::with_capacity(8);
        src.insert(5, 50);
        m.clone_from(&src);
        assert_eq!(m.get(&5), Some(&50));
        let mut s: LookupSet<u32> = LookupSet::new();
        assert!(s.insert(3) && !s.insert(3) && s.contains(&3) && s.remove(&3) && !s.contains(&3));
    }
}
