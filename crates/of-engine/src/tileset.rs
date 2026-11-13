//! Port of `engine-lib/src/game/TileSet.ts` (ARCHITECTURE §3.6.2).
//!
//! The TS `TileSet` is an insertion-ordered set of tile refs: a dense `Uint32Array` with
//! `TOMBSTONE` holes plus an open-addressing index (`TileSet.ts:27-242`). Its observable
//! semantics are those of the dense array walked by index with a re-read length
//! (`forEach`/`values`, `:166-189`):
//! - iteration in insertion order; entries added during iteration are visited;
//! - deleted entries are skipped; delete + re-add moves the value to the end;
//! - `clear()` resets the dense array (`:157-163`), so a running iteration continues at
//!   its dense *index* in the fresh array: it sees the entries added after the clear
//!   **from that index on**. This differs from a JS `Set` (which would restart at the
//!   first new entry): `new TileSet([1,2,3])`, `forEach` that clears and adds 9, 10 on
//!   its first call visits `1,10` (TileSet.ts @5b75bd3 run under node v24.21.0, see the
//!   tests). Ported literally.
//!
//! Dense positions, tombstones, capacities and the hash layout are not observable:
//! compaction (`:208-218`) preserves live order and TS defers it while an iterator runs
//! (`iterDepth`). This port never compacts inside a tick at all: compaction runs only in
//! end-of-tick maintenance ([`OrderedTileSet::maybe_compact`],
//! [`PlayerTileOrder::maybe_compact`]) when tombstones exceed 25 % of the live count
//! (review 1 #6), so no cursor can ever see a position shift.
//!
//! Two layouts with identical semantics:
//! - [`OrderedTileSet`]: a standalone set (attack borders, the `NukeExecution` blast set
//!   `:108`, the `GameMapImpl.bfs` result `:503`): dense `Vec<u32>` + Fx lookup index.
//! - [`PlayerTileOrder`] + [`TileListHdr`]: players' `_tiles` / `_borderTiles`. A tile is
//!   in at most one player's `_tiles` (and at most one `_borderTiles`): `conquer` deletes
//!   it from the previous owner before adding (`GameImpl.ts:987-1009`), and border status
//!   is only ever added to the current owner (`:1042-1052`). So one State-level position
//!   array per set kind, indexed by **land index**, replaces every per-player hash table:
//!   membership is O(1) without hashing and a fork clones it with a memcpy. Every write to
//!   that array goes through the frozen accessor [`PlayerTileOrder::pos_set`]
//!   (ARCHITECTURE §8.5), so copy-on-write chunks can be added later without touching
//!   ported code.
//!
//! Callers iterate with a [`TileCursor`] that re-reads the length every step (the loop
//! body may mutate `State`), never with a borrowed Rust iterator.
//!
//! Owner: **P1** (FILE_MAP §1 row 6).

use crate::StateClone;
use crate::ids::TileRef;
use crate::jsorder::LookupMap;

/// Deleted dense slots hold this sentinel (`TileSet.ts:4-7`: no tile ref can equal it).
pub const TOMBSTONE: u32 = 0xffff_ffff;
/// "Not in any list" in a [`PlayerTileOrder`] position array.
pub const NO_POS: u32 = u32::MAX;

/// A running `forEach`/`for..of` over a tile set: the dense index (`TileSet.ts:178`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TileCursor {
    i: usize,
}

#[inline]
fn next_in(dense: &[u32], c: &mut TileCursor) -> Option<TileRef> {
    while c.i < dense.len() {
        let v = dense[c.i]; // in-range: loop condition
        c.i += 1;
        if v != TOMBSTONE {
            return Some(v);
        }
    }
    None
}

#[inline]
fn needs_compaction(dense_len: usize, live: usize) -> bool {
    (dense_len - live) * 4 > live
}

// ---------------------------------------------------------------------------------------

/// Standalone `TileSet`.
#[derive(StateClone, Debug, Default)]
pub struct OrderedTileSet {
    dense: Vec<u32>,
    /// tile -> dense position; lookup-only (never iterated; order lives in `dense`).
    index: LookupMap<u32, u32>,
}

impl OrderedTileSet {
    pub fn new() -> Self {
        Self::default()
    }
    /// `size`.
    #[inline]
    pub fn size(&self) -> usize {
        self.index.len()
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }
    /// `has(value)`.
    #[inline]
    pub fn has(&self, t: TileRef) -> bool {
        self.index.contains_key(&t)
    }
    /// `add(value)`. Returns true if newly added (an existing value keeps its position).
    pub fn add(&mut self, t: TileRef) -> bool {
        debug_assert_ne!(t, TOMBSTONE);
        let p = u32::try_from(self.dense.len()).expect("dense length fits u32");
        // One probe (t409 H5). No panic point between the two writes (PORTING_GUIDE §11).
        if !self.index.insert_if_absent(t, p) {
            return false;
        }
        self.dense.push(t);
        true
    }
    /// `delete(value)`: tombstones the dense slot (`:139-155`).
    pub fn delete(&mut self, t: TileRef) -> bool {
        match self.index.remove(&t) {
            Some(p) => {
                self.dense[p as usize] = TOMBSTONE; // in-range: index holds valid positions
                true
            }
            None => false,
        }
    }
    /// `clear()` (`:157-163`): fresh dense array; a running cursor keeps its index.
    pub fn clear(&mut self) {
        self.dense.clear();
        self.index.clear();
    }
    /// Start an iteration.
    #[inline]
    pub fn cursor(&self) -> TileCursor {
        TileCursor::default()
    }
    /// Next live value at or after the cursor; re-reads the length (entries appended
    /// during iteration are visited).
    #[inline]
    pub fn next_live(&self, c: &mut TileCursor) -> Option<TileRef> {
        next_in(&self.dense, c)
    }
    /// Borrowing iteration (only when nothing can mutate the set meanwhile).
    pub fn iter(&self) -> impl Iterator<Item = TileRef> + '_ {
        self.dense.iter().copied().filter(|&v| v != TOMBSTONE)
    }
    /// `[...set]`.
    pub fn to_vec(&self) -> Vec<TileRef> {
        self.iter().collect()
    }
    /// Used dense slots including tombstones (not observable; for tests and maintenance).
    #[inline]
    pub fn dense_len(&self) -> usize {
        self.dense.len()
    }
    /// End-of-tick maintenance only: drop tombstones when they exceed 25 % of the live
    /// count. Order is preserved; never call while a cursor may be live.
    pub fn maybe_compact(&mut self) -> bool {
        if !needs_compaction(self.dense.len(), self.size()) {
            return false;
        }
        self.dense.retain(|&v| v != TOMBSTONE);
        for (i, &v) in self.dense.iter().enumerate() {
            self.index.insert(v, i as u32);
        }
        true
    }
}

impl FromIterator<TileRef> for OrderedTileSet {
    /// `new TileSet(values)` (`:37-43`).
    fn from_iter<I: IntoIterator<Item = TileRef>>(it: I) -> Self {
        let mut s = OrderedTileSet::new();
        for t in it {
            s.add(t);
        }
        s
    }
}

// ---------------------------------------------------------------------------------------

/// Per-player header of one `TileSet` in [`PlayerTileOrder`] layout: the dense insertion
/// order with tombstones. Membership lives in the shared position array.
#[derive(StateClone, Debug, Default, PartialEq, Eq)]
pub struct TileListHdr {
    dense: Vec<u32>,
    live: u32,
}

impl TileListHdr {
    /// `size` (`numTilesOwned` for `_tiles`).
    #[inline]
    pub fn size(&self) -> usize {
        self.live as usize
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.live == 0
    }
    #[inline]
    pub fn cursor(&self) -> TileCursor {
        TileCursor::default()
    }
    #[inline]
    pub fn next_live(&self, c: &mut TileCursor) -> Option<TileRef> {
        next_in(&self.dense, c)
    }
    pub fn iter(&self) -> impl Iterator<Item = TileRef> + '_ {
        self.dense.iter().copied().filter(|&v| v != TOMBSTONE)
    }
    pub fn to_vec(&self) -> Vec<TileRef> {
        self.iter().collect()
    }
    #[inline]
    pub fn dense_len(&self) -> usize {
        self.dense.len()
    }
}

/// The State-level position index for one set kind (`pos_tiles` or `pos_border`):
/// `pos[land_index(tile)]` is the tile's position in its owner's [`TileListHdr`], or
/// [`NO_POS`]. `land_index` (`statics.land_index`, shared) maps a tile ref to its land
/// index; only land tiles can be owned or be border tiles.
#[derive(StateClone, Debug, Default)]
pub struct PlayerTileOrder {
    pos: Vec<u32>,
}

impl PlayerTileOrder {
    /// One slot per land tile.
    pub fn new(num_land_tiles: usize) -> Self {
        PlayerTileOrder { pos: vec![NO_POS; num_land_tiles] }
    }

    /// Read accessor.
    #[inline]
    pub fn pos_get(&self, li: u32) -> u32 {
        self.pos[li as usize] // in-range: li comes from statics.land_index
    }

    /// **The only writer of the position array** (frozen at S0, ARCHITECTURE §8.5). State
    /// exposes it as `pos_tiles_set` / `pos_border_set`.
    #[inline]
    pub fn pos_set(&mut self, li: u32, p: u32) {
        self.pos[li as usize] = p; // in-range: li comes from statics.land_index
    }

    /// Land index of `t`, or `None` for a non-land tile (TS sets of land tiles simply do
    /// not contain it).
    #[inline]
    fn li_opt(land_index: &[u32], t: TileRef) -> Option<u32> {
        let li = land_index[t as usize]; // in-range: t is a valid tile ref
        (li != NO_POS).then_some(li)
    }

    #[inline]
    fn li(land_index: &[u32], t: TileRef) -> u32 {
        let li = land_index[t as usize]; // in-range: t is a valid tile ref
        assert_ne!(li, NO_POS, "tile {t} is not a land tile (PlayerTileOrder holds land tiles only)");
        li
    }

    /// `hdr.has(t)`.
    #[inline]
    pub fn has(&self, hdr: &TileListHdr, land_index: &[u32], t: TileRef) -> bool {
        let Some(li) = Self::li_opt(land_index, t) else { return false };
        let p = self.pos_get(li);
        p != NO_POS && hdr.dense.get(p as usize) == Some(&t)
    }

    /// `hdr.add(t)`; true if newly added. The tile must not be in another list of this
    /// kind (structural invariant above; a violation is a Rust bug).
    pub fn add(&mut self, hdr: &mut TileListHdr, land_index: &[u32], t: TileRef) -> bool {
        let li = Self::li(land_index, t);
        let p = self.pos_get(li);
        if p != NO_POS {
            assert_eq!(
                hdr.dense.get(p as usize),
                Some(&t),
                "tile {t} is already in another player's list of this kind"
            );
            return false;
        }
        let np = u32::try_from(hdr.dense.len()).expect("dense length fits u32");
        // No panic point between the paired writes (PORTING_GUIDE §11).
        hdr.dense.push(t);
        hdr.live += 1;
        self.pos_set(li, np);
        true
    }

    /// `hdr.delete(t)`; true if it was present.
    pub fn delete(&mut self, hdr: &mut TileListHdr, land_index: &[u32], t: TileRef) -> bool {
        let Some(li) = Self::li_opt(land_index, t) else { return false };
        let p = self.pos_get(li);
        if p == NO_POS || hdr.dense.get(p as usize) != Some(&t) {
            return false;
        }
        hdr.dense[p as usize] = TOMBSTONE; // in-range: checked by the get above
        hdr.live -= 1;
        self.pos_set(li, NO_POS);
        true
    }

    /// `hdr.clear()`: fresh dense array (a running cursor keeps its index, as in TS).
    pub fn clear(&mut self, hdr: &mut TileListHdr, land_index: &[u32]) {
        for i in 0..hdr.dense.len() {
            let t = hdr.dense[i]; // in-range: loop bound
            if t != TOMBSTONE {
                self.pos_set(Self::li(land_index, t), NO_POS);
            }
        }
        hdr.dense.clear();
        hdr.live = 0;
    }

    /// End-of-tick maintenance only (never with a live cursor): drop tombstones when they
    /// exceed 25 % of the live count, rewriting positions. Order is preserved.
    pub fn maybe_compact(&mut self, hdr: &mut TileListHdr, land_index: &[u32]) -> bool {
        if !needs_compaction(hdr.dense.len(), hdr.size()) {
            return false;
        }
        hdr.dense.retain(|&v| v != TOMBSTONE);
        for i in 0..hdr.dense.len() {
            let t = hdr.dense[i]; // in-range: loop bound
            self.pos_set(Self::li(land_index, t), i as u32);
        }
        true
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // tests may catch panics to assert them
mod tests {
    use super::*;

    #[test]
    fn compaction_threshold_is_25_percent() {
        let mut s: OrderedTileSet = (0..100).collect();
        for t in 0..20 {
            s.delete(t);
        }
        assert!(!s.maybe_compact()); // 20 tombstones = 25 % of 80 live: not above
        assert_eq!(s.dense_len(), 100);
        s.delete(20); // 21 > 25 % of 79
        assert!(s.maybe_compact());
        assert_eq!(s.dense_len(), 79);
        assert_eq!(s.to_vec(), (21..100).collect::<Vec<_>>());
        assert!(s.has(50) && !s.has(20));
        assert!(s.delete(99) && s.add(99) && s.to_vec().last() == Some(&99));
    }

    #[test]
    fn player_tile_order_two_players_share_positions() {
        // 6 tiles; tiles 0..5 are land with land index = tile, tile 5 is water.
        let land: Vec<u32> = vec![0, 1, 2, 3, 4, NO_POS];
        let mut ord = PlayerTileOrder::new(5);
        let (mut a, mut b) = (TileListHdr::default(), TileListHdr::default());
        assert!(ord.add(&mut a, &land, 3));
        assert!(ord.add(&mut a, &land, 1));
        assert!(ord.add(&mut b, &land, 0));
        assert!(!ord.add(&mut a, &land, 3));
        assert!(ord.has(&a, &land, 3) && !ord.has(&b, &land, 3) && ord.has(&b, &land, 0));
        // conquer: delete from the previous owner, then add to the new one
        assert!(ord.delete(&mut a, &land, 3));
        assert!(ord.add(&mut b, &land, 3));
        assert_eq!((a.to_vec(), b.to_vec()), (vec![1], vec![0, 3]));
        assert!(!ord.delete(&mut a, &land, 3));
        assert!(!ord.has(&a, &land, 5) && !ord.delete(&mut a, &land, 5)); // water: never a member
        let r = std::panic::catch_unwind(move || {
            let mut ord = ord;
            ord.add(&mut a, &land, 0) // tile 0 belongs to b: structural invariant
        });
        assert!(r.is_err());
    }
}
