//! Exact version-keyed unit memos (ARCHITECTURE §3.3, review 1 #9). Owner: P1 (the
//! game-level `unitsByTypeMemo`/`unitCountMemo`, `GameImpl.ts:349-428`); P2 uses the same
//! types for `PlayerImpl.myUnitsMemo` & co (`PlayerImpl.ts:166-175, 486-497`).
//!
//! Layout: one backing `Vec<UnitRef>` and 16 per-type headers. A miss for any type at
//! version `v` rebuilds **all 16** lists in one pass over the source list, in source
//! order. This equals TS's per-type lazy fill because every mutation of the source bumps
//! the version, so a filter result at version `v` does not depend on when it is computed.
//! The `verify-memos` feature (P2) recomputes on every hit and asserts equality (G11).
//! Hits hand out a **copy**, like TS's `memo.list.slice()`.

use crate::StateClone;
use crate::api::types::UnitType;
use crate::ids::UnitRef;

/// One cached value with the version it was computed at (`{ version, count }`).
#[derive(StateClone, Debug, Default, PartialEq, Eq)]
pub struct Memo<T> {
    /// `None` = no entry yet (`Map.get` → `undefined`).
    pub version: Option<u32>,
    pub value: T,
}

/// Header of one unit type's list inside a [`TypeMemo`].
#[derive(StateClone, Debug, Default, PartialEq, Eq)]
pub struct TypeMemoHdr {
    pub off: u32,
    pub len: u32,
    pub version: u32,
    pub valid: bool,
}

/// All 16 per-type unit lists of one source list (`Map<UnitType, {version, list}>`).
#[derive(StateClone, Debug, Default, PartialEq, Eq)]
pub struct TypeMemo {
    pub units: Vec<UnitRef>,
    pub hdr: [TypeMemoHdr; 16],
}

impl TypeMemo {
    /// The cached list for `t` at `version`, if valid (TS: `memo !== undefined &&
    /// memo.version === version`). Callers copy it (`memo.units.slice()`).
    pub fn get(&self, t: UnitType, version: u32) -> Option<&[UnitRef]> {
        let h = &self.hdr[t.index()]; // in-range: 16 headers, UnitType::COUNT == 16
        if !h.valid || h.version != version {
            return None;
        }
        let (off, len) = (h.off as usize, h.len as usize);
        self.units.get(off..off + len)
    }
    /// Rebuild all 16 lists from `source` (in source order) at `version`; `type_of` maps a
    /// unit to its type. The result per type equals TS's lazy `filter((u) => u.type() === t)`
    /// at the same version (module docs).
    pub fn rebuild(
        &mut self,
        source: impl Iterator<Item = UnitRef>,
        version: u32,
        mut type_of: impl FnMut(UnitRef) -> UnitType,
    ) {
        let items: Vec<(UnitRef, usize)> = source.map(|u| (u, type_of(u).index())).collect();
        let mut counts = [0u32; UnitType::COUNT];
        for &(_, t) in &items {
            counts[t] += 1; // in-range: index() < 16
        }
        let mut off = 0u32;
        let mut next = [0u32; UnitType::COUNT];
        for t in 0..UnitType::COUNT {
            self.hdr[t] = TypeMemoHdr { off, len: counts[t], version, valid: true };
            next[t] = off;
            off += counts[t];
        }
        self.units.clear();
        self.units.resize(items.len(), UnitRef::default());
        for &(u, t) in &items {
            self.units[next[t] as usize] = u; // in-range: offsets partition 0..items.len()
            next[t] += 1;
        }
    }
    /// Invalidate everything (e.g. on restore).
    pub fn clear(&mut self) {
        for h in self.hdr.iter_mut() {
            h.valid = false;
        }
        self.units.clear();
    }
}

impl<T: Copy> Memo<T> {
    /// The cached value at `version` (`memo !== undefined && memo.version === version`).
    pub fn get(&self, version: u32) -> Option<T> {
        if self.version == Some(version) { Some(self.value) } else { None }
    }
    /// `memo.set(type, { version, count })`.
    pub fn set(&mut self, version: u32, value: T) {
        self.version = Some(version);
        self.value = value;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(slot: u32) -> UnitRef {
        UnitRef { slot, generation: 0 }
    }

    #[test]
    fn rebuild_matches_per_type_filter_in_source_order() {
        let src = [
            (r(0), UnitType::City),
            (r(1), UnitType::Port),
            (r(2), UnitType::City),
            (r(3), UnitType::Factory),
            (r(4), UnitType::City),
        ];
        let mut m = TypeMemo::default();
        assert!(m.get(UnitType::City, 0).is_none());
        m.rebuild(src.iter().map(|x| x.0), 7, |u| src[u.slot as usize].1);
        for t in [UnitType::City, UnitType::Port, UnitType::Factory, UnitType::Warship] {
            let want: Vec<UnitRef> = src.iter().filter(|x| x.1 == t).map(|x| x.0).collect();
            assert_eq!(m.get(t, 7).map(<[UnitRef]>::to_vec), Some(want), "{t:?}");
        }
        // A different version misses (TS: `memo.version === this._unitsVersion`).
        assert!(m.get(UnitType::City, 8).is_none());
        m.clear();
        assert!(m.get(UnitType::City, 7).is_none());
        // Rebuild from an empty source: valid, empty lists.
        m.rebuild(std::iter::empty(), 9, |_| UnitType::City);
        assert_eq!(m.get(UnitType::City, 9), Some(&[][..]));
    }

    #[test]
    fn scalar_memo_hit_and_miss() {
        let mut m: Memo<i32> = Memo::default();
        assert_eq!(m.get(0), None); // `Map.get` → undefined, even at version 0
        m.set(3, 12);
        assert_eq!((m.get(3), m.get(4)), (Some(12), None));
    }
}
