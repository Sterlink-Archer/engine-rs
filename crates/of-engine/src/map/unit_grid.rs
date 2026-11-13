//! `UnitGrid` (`engine-lib/src/game/UnitGrid.ts`). Owner: **P3**. ARCHITECTURE §3.7.
//!
//! TS: a `ceil(H/100) × ceil(W/100)` grid of `Map<UnitType, Set<Unit>>` (`:5-17`). Two orders
//! are state (CS G.unitGrid, `:19-35`): per-cell **type order** (first insertion; emptied sets
//! keep their key) and **in-set order** (insertion; a move is remove + append, `:90-105`).
//! Rust: flat cell headers + an intrusive doubly-linked list per (cell, type), with links per
//! unit slot in arrays parallel to `Arena<Unit>`; plus a DefensePost count per cell (§11).

use crate::StateClone;
use crate::api::types::UnitType;
use crate::arena::Arena;
use crate::ids::{SmallId, TileRef, UnitRef};
use crate::state::unit::Unit;
use of_det::js;

/// `"none"` link sentinel.
pub const NIL: u32 = u32::MAX;

/// One grid cell (`Map<UnitType, Set<Unit>>`).
#[derive(StateClone, Debug, PartialEq, Eq)]
pub struct CellHdr {
    /// Types in first-insertion order (`Map` key order); `n_types` are valid.
    pub type_order: [u8; 16],
    pub n_types: u8,
    /// Per `UnitType` index: first / last unit slot of the in-set list, or [`NIL`].
    pub head: [u32; 16],
    pub tail: [u32; 16],
    /// Live DefensePosts in this cell (answers `hasUnitNearby(DefensePost)` fast, §11 item 2).
    pub n_defense_posts: u16,
}

impl Default for CellHdr {
    fn default() -> Self {
        CellHdr { type_order: [0; 16], n_types: 0, head: [NIL; 16], tail: [NIL; 16], n_defense_posts: 0 }
    }
}

/// Per unit slot: the intrusive list links and the cell the unit is filed under.
#[derive(StateClone, Debug, PartialEq, Eq)]
pub struct UnitLink {
    pub prev: u32,
    pub next: u32,
    /// Cell index, or [`NIL`] when not in the grid.
    pub cell: u32,
    /// Generation of the unit filed in this slot (non-snapshot): turns a list slot back into
    /// its [`UnitRef`] for queries, the snapshot and the GC walk.
    pub generation: u32,
}

impl Default for UnitLink {
    fn default() -> Self {
        UnitLink { prev: NIL, next: NIL, cell: NIL, generation: 0 }
    }
}

/// `UnitGrid<Unit>` data.
///
/// `Refs` is hand-written (below): it reports every unit filed in the grid, in snapshot order.
/// TS keeps a unit in the grid until `removeUnit`, and a deleted unit that is moved again is
/// re-filed (`updateUnitCell` → `addUnit`), so the GC must never free a filed unit, and CS
/// G.unitGrid names it (dead-but-referenced units are rows, SPEC §1.2).
#[derive(StateClone, Debug, Default)]
#[refs(manual)]
pub struct UnitGrid {
    /// `cellSize = 100` (`:7`).
    pub cell_size: u32,
    /// Grid width / height in cells.
    pub cols: u32,
    pub rows: u32,
    /// Row-major cells (`grid[y][x]`).
    pub cells: Vec<CellHdr>,
    /// Indexed by unit arena slot.
    pub links: Vec<UnitLink>,
    /// non-snapshot: the map width (TS keeps the `gm` reference, `:9`); `gm.x/y(tile)`.
    pub map_w: u32,
    /// non-snapshot (t409 H5): the sum of every cell's `n_defense_posts`, i.e. the number of
    /// units filed in any DefensePost set. 0 answers `hasUnitNearby(DefensePost)` without a scan.
    pub n_defense_posts_total: u32,
}

/// One `nearbyUnits` result (`{ unit, distSquared }`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NearbyUnit {
    pub unit: UnitRef,
    pub dist_squared: f64,
}

/// `cellSize = 100` (`:7`).
const CELL_SIZE: u32 = 100;

impl crate::canon::Refs for UnitGrid {
    fn visit_refs(&self, v: &mut dyn crate::canon::RefVisitor) {
        for c in 0..self.cells.len() {
            self.for_each_in_cell(c, |_, u| v.visit_unit(u));
        }
    }
}

impl UnitGrid {
    /// `new UnitGrid(gm)` (`:9-21`) for a `width × height` map:
    /// `ceil(height / 100)` rows of `ceil(width / 100)` empty cells.
    pub fn new(width: u32, height: u32) -> Self {
        let cols = width.div_ceil(CELL_SIZE);
        let rows = height.div_ceil(CELL_SIZE);
        UnitGrid {
            cell_size: CELL_SIZE,
            cols,
            rows,
            cells: vec![CellHdr::default(); (cols * rows) as usize],
            links: Vec::new(),
            map_w: width,
            n_defense_posts_total: 0,
        }
    }

    #[inline]
    fn link(&self, slot: u32) -> UnitLink {
        self.links.get(slot as usize).cloned().unwrap_or_default()
    }
    #[inline]
    fn link_mut(&mut self, slot: u32) -> &mut UnitLink {
        let i = slot as usize;
        if self.links.len() <= i {
            self.links.resize(i + 1, UnitLink::default());
        }
        &mut self.links[i]
    }
    #[inline]
    fn handle(&self, slot: u32) -> UnitRef {
        UnitRef { slot, generation: self.links[slot as usize].generation } // in-range: slot is in a list
    }
    /// `getGridCoords(x, y)` → a cell index, `None` when `!isValidCell` (`:51-53, :107-114`).
    fn cell_of(&self, map_w: u32, tile: TileRef) -> Option<u32> {
        let gx = (tile % map_w) / self.cell_size;
        let gy = (tile / map_w) / self.cell_size;
        (gx < self.cols && gy < self.rows).then_some(gy * self.cols + gx)
    }
    /// The map width the grid was built for (`gm.width()`).
    #[inline]
    fn map_w_hint(&self) -> u32 {
        self.map_w
    }
    /// The query methods take `map_w` explicitly (frozen S0 signature); it must be the grid's.
    #[inline]
    fn check_w(&self, map_w: u32) {
        debug_assert_eq!(map_w, self.map_w, "UnitGrid built for another map width");
    }

    /// Visit the units of cell `c`, types in Map key order, each list in Set order.
    fn for_each_in_cell(&self, c: usize, mut f: impl FnMut(UnitType, UnitRef)) {
        let hdr = &self.cells[c]; // in-range: c < cells.len()
        for k in 0..usize::from(hdr.n_types) {
            let t = UnitType::ALL[usize::from(hdr.type_order[k])];
            let mut s = hdr.head[t.index()];
            while s != NIL {
                f(t, self.handle(s));
                s = self.links[s as usize].next; // in-range: list member
            }
        }
    }

    /// `snapshot(unitRef)` (`:23-35`): per cell (row-major), each type's units in Set order,
    /// emptied sets included (their Map key stays).
    #[allow(dead_code)] // CS G.unitGrid (P13)
    pub(crate) fn snapshot(&self) -> Vec<Vec<(UnitType, Vec<UnitRef>)>> {
        (0..self.cells.len())
            .map(|c| {
                let hdr = &self.cells[c];
                (0..usize::from(hdr.n_types))
                    .map(|k| {
                        let t = UnitType::ALL[usize::from(hdr.type_order[k])];
                        let mut v = Vec::new();
                        let mut s = hdr.head[t.index()];
                        while s != NIL {
                            v.push(self.handle(s));
                            s = self.links[s as usize].next;
                        }
                        (t, v)
                    })
                    .collect()
            })
            .collect()
    }

    /// `this.grid[gy][gx].get(type) ?? set(type, new Set())` + `Set.add(unit)` on cell `c`.
    fn set_add(&mut self, c: u32, t: UnitType, u: UnitRef) {
        let ti = t.index();
        let hdr = &mut self.cells[c as usize]; // in-range: cell_of
        if !hdr.type_order[..usize::from(hdr.n_types)].contains(&(ti as u8)) {
            // `Map.set` of a new key: appended to the key order.
            hdr.type_order[usize::from(hdr.n_types)] = ti as u8;
            hdr.n_types += 1;
        }
        let l = self.link(u.slot);
        if l.cell != NIL {
            // `Set.add` of a member is a no-op. TS cannot file one unit under two cells: every
            // tile change goes through `move()` → `updateUnitCell` (ARCHITECTURE §3.7).
            assert!(
                l.cell == c && l.generation == u.generation,
                "UnitGrid: unit slot {} already filed in cell {} (adding to {c})",
                u.slot,
                l.cell
            );
            return;
        }
        let tail = self.cells[c as usize].tail[ti];
        *self.link_mut(u.slot) = UnitLink { prev: tail, next: NIL, cell: c, generation: u.generation };
        if tail == NIL {
            self.cells[c as usize].head[ti] = u.slot;
        } else {
            self.links[tail as usize].next = u.slot; // in-range: list member
        }
        let hdr = &mut self.cells[c as usize];
        hdr.tail[ti] = u.slot;
        if t == UnitType::DefensePost {
            hdr.n_defense_posts += 1;
            self.n_defense_posts_total += 1;
        }
    }
    /// `Set.delete(unit)` on cell `c`'s `t` set (no-op when not a member; the key stays).
    fn set_delete(&mut self, c: u32, t: UnitType, u: UnitRef) {
        let l = self.link(u.slot);
        if l.cell != c || l.generation != u.generation {
            return;
        }
        let ti = t.index();
        if l.prev == NIL {
            self.cells[c as usize].head[ti] = l.next;
        } else {
            self.links[l.prev as usize].next = l.next;
        }
        if l.next == NIL {
            self.cells[c as usize].tail[ti] = l.prev;
        } else {
            self.links[l.next as usize].prev = l.prev;
        }
        *self.link_mut(u.slot) = UnitLink::default();
        if t == UnitType::DefensePost {
            self.cells[c as usize].n_defense_posts -= 1;
            self.n_defense_posts_total -= 1;
        }
    }

    /// `addUnit(unit)` (`:56-68`): appends to the unit's cell/type set.
    pub fn add_unit(&mut self, units: &Arena<Unit, UnitRef>, u: UnitRef) {
        let unit = units.get(u);
        let (tile, t) = (unit.tile, unit.unit_type);
        let map_w = self.map_w_hint();
        if let Some(c) = self.cell_of(map_w, tile) {
            self.set_add(c, t, u);
        }
    }
    /// `removeUnit(unit)` (`:71-74`): by the unit's current tile.
    pub fn remove_unit(&mut self, units: &Arena<Unit, UnitRef>, u: UnitRef) {
        let tile = units.get(u).tile;
        self.remove_unit_by_tile(units, u, tile);
    }
    /// `removeUnitByTile(unit, tile)` (`:76-85`).
    pub fn remove_unit_by_tile(&mut self, units: &Arena<Unit, UnitRef>, u: UnitRef, tile: TileRef) {
        let t = units.get(u).unit_type;
        let map_w = self.map_w_hint();
        if let Some(c) = self.cell_of(map_w, tile) {
            // TS: `grid[gy][gx].get(type)?.delete(unit)`; a missing key is a no-op too.
            if self.cells[c as usize].type_order[..usize::from(self.cells[c as usize].n_types)]
                .contains(&(t.index() as u8))
            {
                self.set_delete(c, t, u);
            }
        }
    }
    /// `updateUnitCell(unit)` (`:90-105`): remove from the old cell, append to the new one.
    pub fn update_unit_cell(&mut self, units: &Arena<Unit, UnitRef>, u: UnitRef) {
        let unit = units.get(u);
        let (new_tile, old_tile) = (unit.tile, unit.last_tile);
        if new_tile == old_tile {
            return;
        }
        let map_w = self.map_w_hint();
        let cs = self.cell_size;
        let (gx, gy) = ((old_tile % map_w) / cs, (old_tile / map_w) / cs);
        let (ngx, ngy) = ((new_tile % map_w) / cs, (new_tile / map_w) / cs);
        if gx != ngx || gy != ngy {
            self.remove_unit_by_tile(units, u, old_tile);
            self.add_unit(units, u);
        }
    }

    /// `getCellsInRange(tile, range)` (`:117-140`), in f64 as TS computes it.
    fn cells_in_range(&self, map_w: u32, tile: TileRef, range: f64) -> (f64, f64, f64, f64) {
        let x = f64::from(tile % map_w);
        let y = f64::from(tile / map_w);
        let cs = f64::from(self.cell_size);
        let (gx, gy) = (js::floor(x / cs), js::floor(y / cs));
        let start_x = js::max(0.0, gx - js::ceil((range - (x % cs)) / cs));
        let end_x = js::min(f64::from(self.cols) - 1.0, gx + js::ceil((range - (cs - (x % cs))) / cs));
        let start_y = js::max(0.0, gy - js::ceil((range - (y % cs)) / cs));
        let end_y = js::min(f64::from(self.rows) - 1.0, gy + js::ceil((range - (cs - (y % cs))) / cs));
        (start_x, end_x, start_y, end_y)
    }
    /// `for (cy = startGridY; cy <= endGridY; cy++) for (cx = startGridX; cx <= endGridX; cx++)`.
    fn for_each_cell_in_range(&self, map_w: u32, tile: TileRef, range: f64, mut f: impl FnMut(usize) -> bool) {
        let (sx, ex, sy, ey) = self.cells_in_range(map_w, tile, range);
        let mut cy = sy;
        while cy <= ey {
            let mut cx = sx;
            while cx <= ex {
                // in-range: 0 <= cx <= cols-1, 0 <= cy <= rows-1 integers (max/min/ceil above)
                if !f((cy as usize) * self.cols as usize + cx as usize) {
                    return;
                }
                cx += 1.0;
            }
            cy += 1.0;
        }
    }
    /// The units of cell `c`'s `t` set, in Set order; `false` from `f` stops.
    fn for_each_of_type(&self, c: usize, t: UnitType, mut f: impl FnMut(UnitRef) -> bool) -> bool {
        let mut s = self.cells[c].head[t.index()];
        while s != NIL {
            let u = self.handle(s);
            s = self.links[s as usize].next;
            if !f(u) {
                return false;
            }
        }
        true
    }
    /// `dx * dx + dy * dy` between a unit's tile and `(x, y)`.
    #[inline]
    fn dist_sq(map_w: u32, a: TileRef, x: u32, y: u32) -> f64 {
        let dx = f64::from(a % map_w) - f64::from(x);
        let dy = f64::from(a / map_w) - f64::from(y);
        dx * dx + dy * dy
    }

    /// `nearbyUnits(tile, searchRange, types, predicate?, includeUnderConstruction = false)`
    /// (`:155-222`): cells in TS order, then types in the given order, then Set order. The
    /// single-type TS overload is `types = &[t]` (same iteration). The predicate sees
    /// `(unit, distSquared)`.
    #[allow(clippy::too_many_arguments)]
    pub fn nearby_units(
        &self,
        units: &Arena<Unit, UnitRef>,
        map_w: u32,
        tile: TileRef,
        search_range: f64,
        types: &[UnitType],
        mut predicate: impl FnMut(UnitRef, f64) -> bool,
        include_under_construction: bool,
    ) -> Vec<NearbyUnit> {
        self.check_w(map_w);
        let mut nearby = Vec::new();
        let (x, y) = (tile % map_w, tile / map_w);
        let range_squared = search_range * search_range;
        self.for_each_cell_in_range(map_w, tile, search_range, |c| {
            for &t in types {
                self.for_each_of_type(c, t, |u| {
                    let unit = units.get(u);
                    if !unit.active {
                        return true;
                    }
                    if !include_under_construction && unit.under_construction {
                        return true;
                    }
                    let dist_squared = Self::dist_sq(map_w, unit.tile, x, y);
                    if dist_squared > range_squared {
                        return true;
                    }
                    if !predicate(u, dist_squared) {
                        return true;
                    }
                    nearby.push(NearbyUnit { unit: u, dist_squared });
                    true
                });
            }
            true
        });
        nearby
    }
    /// `unitIsInRange(unit, tile, rangeSquared, playerId?, includeUnderConstruction)` (`:224-244`).
    fn unit_is_in_range(
        unit: &Unit,
        map_w: u32,
        x: u32,
        y: u32,
        range_squared: f64,
        player: Option<SmallId>,
        include_under_construction: bool,
    ) -> bool {
        if !unit.active {
            return false;
        }
        if !include_under_construction && unit.under_construction {
            return false;
        }
        if let Some(p) = player
            && unit.owner != p
        {
            return false;
        }
        Self::dist_sq(map_w, unit.tile, x, y) <= range_squared
    }
    /// `hasUnitNearby(tile, searchRange, type, playerId?, includeUnderConstruction = false)`
    /// (`:247-279`); `player` is the resolved owner filter.
    #[allow(clippy::too_many_arguments)]
    pub fn has_unit_nearby(
        &self,
        units: &Arena<Unit, UnitRef>,
        map_w: u32,
        tile: TileRef,
        search_range: f64,
        t: UnitType,
        player: Option<SmallId>,
        include_under_construction: bool,
    ) -> bool {
        self.check_w(map_w);
        // t409 H5: no unit is filed in any DefensePost set, so no cell's set yields a unit.
        if t == UnitType::DefensePost && self.n_defense_posts_total == 0 {
            #[cfg(feature = "verify-memos")]
            assert!(
                self.cells.iter().all(|c| c.head[t.index()] == NIL && c.n_defense_posts == 0),
                "verify-memos: n_defense_posts_total stale"
            );
            return false;
        }
        let (x, y) = (tile % map_w, tile / map_w);
        let range_squared = search_range * search_range;
        let mut found = false;
        self.for_each_cell_in_range(map_w, tile, search_range, |c| {
            self.for_each_of_type(c, t, |u| {
                if Self::unit_is_in_range(units.get(u), map_w, x, y, range_squared, player, include_under_construction)
                {
                    found = true;
                    return false;
                }
                true
            })
        });
        found
    }
    /// `anyUnitNearby(tile, searchRange, types, predicate, playerId?, includeUnderConstruction)`
    /// (`:282-320`).
    #[allow(clippy::too_many_arguments)]
    pub fn any_unit_nearby(
        &self,
        units: &Arena<Unit, UnitRef>,
        map_w: u32,
        tile: TileRef,
        search_range: f64,
        types: &[UnitType],
        mut predicate: impl FnMut(UnitRef) -> bool,
        player: Option<SmallId>,
        include_under_construction: bool,
    ) -> bool {
        self.check_w(map_w);
        let (x, y) = (tile % map_w, tile / map_w);
        let range_squared = search_range * search_range;
        let mut found = false;
        self.for_each_cell_in_range(map_w, tile, search_range, |c| {
            for &t in types {
                let go_on = self.for_each_of_type(c, t, |u| {
                    if !Self::unit_is_in_range(
                        units.get(u),
                        map_w,
                        x,
                        y,
                        range_squared,
                        player,
                        include_under_construction,
                    ) {
                        return true;
                    }
                    if predicate(u) {
                        found = true;
                        return false;
                    }
                    true
                });
                if !go_on {
                    return false;
                }
            }
            true
        });
        found
    }
}
