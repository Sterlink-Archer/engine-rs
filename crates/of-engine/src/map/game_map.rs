//! `GameMap` (`engine-api/src/game/GameMap.ts` interface, `engine-lib/src/game/GameMapImpl.ts`).
//! Owner: **P3**. ARCHITECTURE §3.6.1, §8.5.
//!
//! - Terrain byte: bit 7 land, bit 6 shoreline, bit 5 ocean, bits 0-4 magnitude
//!   (`GameMapImpl.ts:26-35`). Shared `Arc<[u8]>`, copy-on-write on water edits; every terrain
//!   write bumps `terrain_gen` (gates cross-fork water segment sharing, §3.8).
//! - State `u16`: bits 0-11 owner, 13 fallout, 14 defense bonus (`:37-41`). Private, behind the
//!   frozen accessors of [`TileStateBuf`] (§8.5), so chunked COW can be added later.
//! - The minimap's state buffer is the `Zero` variant: the tick path never writes minimap
//!   state (only `restoreSnapshot` does, which `from_canon` replaces).
//! - `bfsSearch`/`bfs` use the per-map scratch `sc.bfs_main` / `sc.bfs_mini` with a `busy`
//!   flag: a nested call on the same map throws `"Re-entrant bfsSearch is not supported"`.

use crate::StateClone;
use crate::api::types::{Cell, TerrainType};
use crate::ids::TileRef;
use crate::jsorder::JsMap;
use crate::scratch::bfs::BfsScratch;
use crate::tileset::OrderedTileSet;
use std::sync::Arc;

/// `PLAYER_ID_MASK` (`GameMapImpl.ts:38`).
pub const PLAYER_ID_MASK: u16 = 0xfff;
/// `FALLOUT_BIT` (`:39`).
pub const FALLOUT_BIT: u16 = 13;
/// `DEFENSE_BONUS_BIT` (`:40`).
pub const DEFENSE_BONUS_BIT: u16 = 14;

/// The per-tile state buffer (`state: Uint16Array`, `GameMapImpl.ts:15`).
///
/// **Frozen at S0** (ARCHITECTURE §8.5): every write goes through
/// [`TileStateBuf::set_owner_id`], [`TileStateBuf::set_fallout`],
/// [`TileStateBuf::set_defense_bonus`] (or [`TileStateBuf::set_raw`] for the CS loader);
/// reads through [`TileStateBuf::get`]. The buffer is private.
#[derive(StateClone, Debug)]
pub enum TileStateBuf {
    /// The main map: one `u16` per tile.
    Full(Vec<u16>),
    /// The minimap: never written on the tick path, so it is all zeros of this length.
    Zero(usize),
}

impl Default for TileStateBuf {
    fn default() -> Self {
        TileStateBuf::Zero(0)
    }
}

impl TileStateBuf {
    /// A zeroed buffer of `n` tiles (`new Uint16Array(n)`).
    pub fn new_full(n: usize) -> Self {
        TileStateBuf::Full(vec![0; n])
    }
    /// The all-zero minimap buffer.
    pub fn new_zero(n: usize) -> Self {
        TileStateBuf::Zero(n)
    }
    #[inline]
    pub fn len(&self) -> usize {
        match self {
            TileStateBuf::Full(v) => v.len(),
            TileStateBuf::Zero(n) => *n,
        }
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// `state[ref]`.
    #[inline]
    pub fn get(&self, t: TileRef) -> u16 {
        match self {
            TileStateBuf::Full(v) => v[t as usize], // in-range: t is a valid tile ref
            TileStateBuf::Zero(n) => {
                debug_assert!((t as usize) < *n);
                0
            }
        }
    }
    #[inline]
    fn full_mut(&mut self) -> &mut Vec<u16> {
        match self {
            TileStateBuf::Full(v) => v,
            TileStateBuf::Zero(_) => {
                panic!("of-engine: write to the minimap tile state (Rust bug, ARCHITECTURE §3.6.1)")
            }
        }
    }
    /// `state[ref] = (state[ref] & ~PLAYER_ID_MASK) | id` (`GameMapImpl.ts:216-224`); the
    /// `> PLAYER_ID_MASK` throw is the caller's ([`GameMap::set_owner_id`]).
    #[inline]
    pub fn set_owner_id(&mut self, t: TileRef, id: u16) {
        let v = self.full_mut();
        let i = t as usize;
        v[i] = (v[i] & !PLAYER_ID_MASK) | id; // in-range: t is a valid tile ref
    }
    /// Set or clear the fallout bit (`:230-246`; the counter is the caller's).
    #[inline]
    pub fn set_fallout(&mut self, t: TileRef, value: bool) {
        let v = self.full_mut();
        let i = t as usize;
        if value {
            v[i] |= 1 << FALLOUT_BIT; // in-range: t is a valid tile ref
        } else {
            v[i] &= !(1 << FALLOUT_BIT); // in-range: t is a valid tile ref
        }
    }
    /// Set or clear the defense-bonus bit (`:279-285`).
    #[inline]
    pub fn set_defense_bonus(&mut self, t: TileRef, value: bool) {
        let v = self.full_mut();
        let i = t as usize;
        if value {
            v[i] |= 1 << DEFENSE_BONUS_BIT; // in-range: t is a valid tile ref
        } else {
            v[i] &= !(1 << DEFENSE_BONUS_BIT); // in-range: t is a valid tile ref
        }
    }
    /// Raw write of a whole state word (CS loader / `updateTile` only).
    #[inline]
    pub fn set_raw(&mut self, t: TileRef, state: u16) {
        self.full_mut()[t as usize] = state; // in-range: t is a valid tile ref
    }
    /// Zero-copy read of the whole buffer (`tileStateBuffer()`); `None` for the minimap.
    pub fn as_slice(&self) -> Option<&[u16]> {
        match self {
            TileStateBuf::Full(v) => Some(v),
            TileStateBuf::Zero(_) => None,
        }
    }
}

/// `GameMapImpl` data (`GameMap v1` snapshot fields derive from it: width, height, edit
/// lists from `pristine_edits` + `terrain`, fallout/defense lists from `state`, counters).
#[derive(StateClone, Debug, Default)]
pub struct GameMap {
    /// `width_`. // int: map dimension ≤ 65535.
    pub w: u32,
    /// `height_`.
    pub h: u32,
    /// `pristineTerrain: Map<TileRef, number> | null` (`:161-175`): original byte per edited
    /// tile, first-edit order (CS `editTiles`/`editOriginal`).
    pub pristine_edits: JsMap<TileRef, u8>,
    /// `terrain: Uint8Array` (shared, COW on edit); CS `editCurrent` = its bytes at the edits.
    pub terrain: Arc<[u8]>,
    /// `state: Uint16Array` (private behind accessors); CS `fallout`/`defense` tile lists.
    state: TileStateBuf,
    /// `numLandTiles_`.
    pub num_land_tiles: u32,
    /// `waterVersion_` (`:155`).
    pub water_version: u32,
    /// `_numTilesWithFallout` (`:8`).
    pub num_tiles_with_fallout: u32,
    /// non-snapshot: +1 on every terrain write (per lineage; gen 0 = pristine).
    pub terrain_gen: u32,
}

/// `IS_LAND_BIT` (`:27`).
const IS_LAND_BIT: u8 = 7;
/// `SHORELINE_BIT` (`:28`).
const SHORELINE_BIT: u8 = 6;
/// `OCEAN_BIT` (`:29`).
const OCEAN_BIT: u8 = 5;
/// `MAGNITUDE_MASK = 0x1f` (`:30`).
const MAGNITUDE_MASK: u8 = 0x1f;
/// `IMPASSABLE_MAGNITUDE = 31` (`:35`).
const IMPASSABLE_MAGNITUDE: u8 = 31;

/// TS `finally` of `bfsSearch` (`:496-499`): clears the queue and the re-entrancy guard on
/// every exit, unwinding included.
struct BfsGuard<'s>(&'s mut BfsScratch);
impl Drop for BfsGuard<'_> {
    fn drop(&mut self) {
        self.0.exit();
    }
}

impl GameMap {
    /// `new GameMapImpl(width, height, terrainData, numLandTiles)` (`:43-62`); throws on a
    /// length mismatch. `mini = true` builds the minimap with a `Zero` state buffer.
    pub fn new(width: u32, height: u32, terrain: Arc<[u8]>, num_land_tiles: u32, mini: bool) -> Self {
        let n = u64::from(width) * u64::from(height);
        if terrain.len() as u64 != n {
            of_det::ts_throw!("Terrain data length {} doesn't match dimensions {width}x{height}", terrain.len());
        }
        // in-range: n = terrain.len(), a usize
        let n = n as usize;
        GameMap {
            w: width,
            h: height,
            pristine_edits: JsMap::default(),
            terrain,
            state: if mini { TileStateBuf::new_zero(n) } else { TileStateBuf::new_full(n) },
            num_land_tiles,
            water_version: 0,
            num_tiles_with_fallout: 0,
            terrain_gen: 0,
        }
    }
    /// The state buffer (read-only; writes go through the methods below).
    #[inline]
    pub fn state_buf(&self) -> &TileStateBuf {
        &self.state
    }
    /// The state buffer for the frozen write accessors (CS loader, P13).
    #[inline]
    pub fn state_buf_mut(&mut self) -> &mut TileStateBuf {
        &mut self.state
    }

    /// Every terrain write goes through here: COW on the shared terrain and `terrain_gen += 1`
    /// (ARCHITECTURE §3.6.1, §3.8).
    #[inline]
    fn terrain_mut(&mut self) -> &mut [u8] {
        self.terrain_gen = self.terrain_gen.wrapping_add(1);
        Arc::make_mut(&mut self.terrain)
    }
    #[inline]
    fn tb(&self, r: TileRef) -> u8 {
        self.terrain[r as usize] // in-range: r is a valid tile ref (callers pass map refs)
    }

    pub fn num_tiles_with_fallout(&self) -> u32 {
        self.num_tiles_with_fallout
    }
    /// `ref(x, y)`: throws `Invalid coordinates: …` on invalid input (`:67-72`).
    pub fn ref_(&self, x: f64, y: f64) -> TileRef {
        match self.try_ref(x, y) {
            Some(r) => r,
            None => of_det::ts_throw!(
                "Invalid coordinates: {},{}",
                of_det::throw::js_number_to_string(x),
                of_det::throw::js_number_to_string(y)
            ),
        }
    }
    /// `isValidCoord` + `ref` without a throw (`None` where TS would throw).
    pub fn try_ref(&self, x: f64, y: f64) -> Option<TileRef> {
        if !self.is_valid_coord(x, y) {
            return None;
        }
        // TS: `yToRef[y] + x`; exact: 0 <= x < w, 0 <= y < h integers.
        Some((y as u32) * self.w + x as u32)
    }
    /// `Number.isInteger(ref) && ref >= 0 && ref < width * height` (`:74-78`).
    pub fn is_valid_ref(&self, r: f64) -> bool {
        is_integer(r) && r >= 0.0 && r < f64::from(self.w) * f64::from(self.h)
    }
    /// `x(ref) = ref % w`.
    #[inline]
    pub fn x(&self, r: TileRef) -> u32 {
        r % self.w
    }
    /// `y(ref) = (ref / w) | 0`.
    #[inline]
    pub fn y(&self, r: TileRef) -> u32 {
        r / self.w
    }
    pub fn cell(&self, r: TileRef) -> Cell {
        Cell::new(f64::from(self.x(r)), f64::from(self.y(r)))
    }
    #[inline]
    pub fn width(&self) -> u32 {
        self.w
    }
    #[inline]
    pub fn height(&self) -> u32 {
        self.h
    }
    pub fn num_land_tiles(&self) -> u32 {
        self.num_land_tiles
    }
    /// `num_tiles = width * height` (`forEachTile` bound).
    #[inline]
    pub fn num_tiles(&self) -> u32 {
        self.w * self.h
    }
    /// `forEachTile(fn)` (`:394-398`): refs `0 .. width*height` in order.
    #[allow(dead_code)] // TS API (`GameImpl.ts:1559`); callers arrive with their packages
    pub(crate) fn for_each_tile(&self, mut f: impl FnMut(TileRef)) {
        for r in 0..self.num_tiles() {
            f(r);
        }
    }
    /// `isValidCoord(x, y)` (`Number.isInteger` checks, `:102-111`).
    pub fn is_valid_coord(&self, x: f64, y: f64) -> bool {
        is_integer(x) && is_integer(y) && x >= 0.0 && x < f64::from(self.w) && y >= 0.0 && y < f64::from(self.h)
    }
    #[inline]
    pub fn is_land(&self, r: TileRef) -> bool {
        self.tb(r) & (1 << IS_LAND_BIT) != 0
    }
    #[inline]
    pub fn is_impassable(&self, r: TileRef) -> bool {
        self.is_land(r) && (self.tb(r) & MAGNITUDE_MASK) == IMPASSABLE_MAGNITUDE
    }
    /// `isOceanShore(ref)` (`:126-137`).
    pub fn is_ocean_shore(&self, r: TileRef) -> bool {
        if !self.is_land(r) {
            return false;
        }
        let w = self.w;
        let x = r % w;
        if x != 0 && self.is_ocean(r - 1) {
            return true;
        }
        if x != w - 1 && self.is_ocean(r + 1) {
            return true;
        }
        if r >= w && self.is_ocean(r - w) {
            return true;
        }
        if r < (self.h - 1) * w && self.is_ocean(r + w) {
            return true;
        }
        false
    }
    #[inline]
    pub fn is_ocean(&self, r: TileRef) -> bool {
        self.tb(r) & (1 << OCEAN_BIT) != 0
    }
    #[inline]
    pub fn is_shoreline(&self, r: TileRef) -> bool {
        self.tb(r) & (1 << SHORELINE_BIT) != 0
    }
    #[inline]
    pub fn magnitude(&self, r: TileRef) -> u8 {
        self.tb(r) & MAGNITUDE_MASK
    }
    #[inline]
    pub fn terrain_byte(&self, r: TileRef) -> u8 {
        self.tb(r)
    }
    pub fn water_version(&self) -> u32 {
        self.water_version
    }
    /// `editedTerrainTiles()` (`:166-168`): first-edit order.
    pub fn edited_terrain_tiles(&self) -> Vec<TileRef> {
        self.pristine_edits.iter().map(|(&k, _)| k).collect()
    }
    /// `noteTerrainEdit(ref)` (`:170-175`): remember the original byte on first edit.
    fn note_terrain_edit(&mut self, r: TileRef) {
        if !self.pristine_edits.contains_key(&r) {
            let b = self.tb(r);
            self.pristine_edits.insert(r, b);
        }
    }
    /// `setWater(ref)` (`:177-183`): terrain write (COW, `terrain_gen += 1`, `noteTerrainEdit`).
    pub fn set_water(&mut self, r: TileRef) {
        if !self.is_land(r) || self.is_impassable(r) {
            return;
        }
        self.note_terrain_edit(r);
        self.water_version += 1;
        self.terrain_mut()[r as usize] = 0; // in-range: valid ref
        self.num_land_tiles -= 1;
    }
    /// `setShorelineBit(ref)` (`:185-188`).
    pub fn set_shoreline_bit(&mut self, r: TileRef) {
        self.note_terrain_edit(r);
        self.terrain_mut()[r as usize] |= 1 << SHORELINE_BIT; // in-range: valid ref
    }
    /// `clearShorelineBit(ref)` (`:190-193`).
    pub fn clear_shoreline_bit(&mut self, r: TileRef) {
        self.note_terrain_edit(r);
        self.terrain_mut()[r as usize] &= !(1 << SHORELINE_BIT); // in-range: valid ref
    }
    /// `setOcean(ref)` (`:195-198`).
    pub fn set_ocean(&mut self, r: TileRef) {
        self.note_terrain_edit(r);
        self.terrain_mut()[r as usize] |= 1 << OCEAN_BIT; // in-range: valid ref
    }
    /// `setMagnitude(ref, value)` (`:200-205`); minimap magnitude is sim-relevant (G10).
    pub fn set_magnitude(&mut self, r: TileRef, value: u8) {
        self.note_terrain_edit(r);
        let t = self.terrain_mut();
        let i = r as usize; // in-range: valid ref
        t[i] = (t[i] & !MAGNITUDE_MASK) | (value & MAGNITUDE_MASK);
    }
    #[inline]
    pub fn owner_id(&self, r: TileRef) -> u16 {
        self.state.get(r) & PLAYER_ID_MASK
    }
    #[inline]
    pub fn has_owner(&self, r: TileRef) -> bool {
        self.owner_id(r) != 0
    }
    /// `setOwnerID(ref, id)`: throws `Player ID … exceeds maximum value 4095` (`:216-224`).
    pub fn set_owner_id(&mut self, r: TileRef, player_id: u16) {
        if player_id > PLAYER_ID_MASK {
            of_det::ts_throw!("Player ID {player_id} exceeds maximum value {PLAYER_ID_MASK}");
        }
        self.state.set_owner_id(r, player_id);
    }
    #[inline]
    pub fn has_fallout(&self, r: TileRef) -> bool {
        self.state.get(r) & (1 << FALLOUT_BIT) != 0
    }
    /// `setFallout(ref, value)` (`:230-243`, keeps `_numTilesWithFallout`).
    pub fn set_fallout(&mut self, r: TileRef, value: bool) {
        let existing = self.has_fallout(r);
        if value {
            if !existing {
                self.num_tiles_with_fallout += 1;
                self.state.set_fallout(r, true);
            }
        } else if existing {
            self.num_tiles_with_fallout -= 1;
            self.state.set_fallout(r, false);
        }
    }
    /// `isOnEdgeOfMap(ref)` (`:248-260`): the map boundary or an impassable 4-neighbour.
    pub fn is_on_edge_of_map(&self, r: TileRef) -> bool {
        let w = self.w;
        let x = r % w;
        if x == 0 || x == w - 1 || r < w || r >= (self.h - 1) * w {
            return true;
        }
        self.is_impassable(r - 1) || self.is_impassable(r + 1) || self.is_impassable(r - w) || self.is_impassable(r + w)
    }
    /// `isBorder(ref)` (`:262-273`).
    pub fn is_border(&self, r: TileRef) -> bool {
        let w = self.w;
        let x = r % w;
        let owner = self.owner_id(r);
        if x != 0 && self.owner_id(r - 1) != owner {
            return true;
        }
        if x != w - 1 && self.owner_id(r + 1) != owner {
            return true;
        }
        if r >= w && self.owner_id(r - w) != owner {
            return true;
        }
        if r < (self.h - 1) * w && self.owner_id(r + w) != owner {
            return true;
        }
        false
    }
    #[inline]
    pub fn has_defense_bonus(&self, r: TileRef) -> bool {
        self.state.get(r) & (1 << DEFENSE_BONUS_BIT) != 0
    }
    /// `setDefenseBonus` (no callers at the pin; ported for fidelity).
    pub fn set_defense_bonus(&mut self, r: TileRef, value: bool) {
        self.state.set_defense_bonus(r, value);
    }
    #[inline]
    pub fn is_water(&self, r: TileRef) -> bool {
        !self.is_land(r)
    }
    #[inline]
    pub fn is_shore(&self, r: TileRef) -> bool {
        self.is_land(r) && self.is_shoreline(r)
    }
    /// `cost(ref)`: `magnitude < 10 ? 2 : 1` (`:296-297`).
    #[inline]
    pub fn cost(&self, r: TileRef) -> u32 {
        if self.magnitude(r) < 10 { 2 } else { 1 }
    }
    /// `terrainType(ref)` (`:302-312`).
    pub fn terrain_type(&self, r: TileRef) -> TerrainType {
        if self.is_land(r) {
            let m = self.magnitude(r);
            if m >= IMPASSABLE_MAGNITUDE {
                return TerrainType::Impassable;
            }
            if m < 10 {
                return TerrainType::Plains;
            }
            if m < 20 {
                return TerrainType::Highland;
            }
            return TerrainType::Mountain;
        }
        TerrainType::Ocean
    }
    /// `neighbors(ref)` (N, S, W, E; `:314-325`).
    pub fn neighbors(&self, r: TileRef) -> Vec<TileRef> {
        let mut buf = [0; 4];
        let n = self.neighbors4(r, &mut buf);
        buf[..n].to_vec()
    }
    /// `neighbors4(ref, out)`: writes N, S, W, E; returns the count (`:337-347`).
    #[inline]
    pub fn neighbors4(&self, r: TileRef, out: &mut [TileRef; 4]) -> usize {
        let w = self.w;
        let x = r % w;
        let mut n = 0;
        if r >= w {
            out[n] = r - w;
            n += 1;
        }
        if r < (self.h - 1) * w {
            out[n] = r + w;
            n += 1;
        }
        if x != 0 {
            out[n] = r - 1;
            n += 1;
        }
        if x != w - 1 {
            out[n] = r + 1;
            n += 1;
        }
        n
    }
    /// `forEachNeighbor(ref, callback)` (`GameMapImpl.ts:327-335`): N, S, W, E, the same order
    /// as `neighbors4`. Callers: `AttackExecution.ts:476,486`, `WarshipExecution.ts:723`,
    /// `SpatialQuery.ts:92`, `SharedWaterCache.ts:111` (through `GameImpl.ts:1539`).
    #[inline]
    pub fn for_each_neighbor(&self, r: TileRef, mut f: impl FnMut(TileRef)) {
        let mut buf = [0; 4];
        let n = self.neighbors4(r, &mut buf);
        for &t in &buf[..n] {
            f(t);
        }
    }
    /// `neighbors8(ref, out)` (`:349-369`): dx-major order.
    #[inline]
    pub fn neighbors8(&self, r: TileRef, out: &mut [TileRef; 8]) -> usize {
        let w = self.w;
        let x = r % w;
        let has_n = r >= w;
        let has_s = r < (self.h - 1) * w;
        let mut n = 0;
        let mut push = |t: TileRef| {
            out[n] = t;
            n += 1;
        };
        if x != 0 {
            if has_n {
                push(r - 1 - w);
            }
            push(r - 1);
            if has_s {
                push(r - 1 + w);
            }
        }
        if has_n {
            push(r - w);
        }
        if has_s {
            push(r + w);
        }
        if x != w - 1 {
            if has_n {
                push(r + 1 - w);
            }
            push(r + 1);
            if has_s {
                push(r + 1 + w);
            }
        }
        n
    }
    /// `forEachNeighborWithDiag` order (dx-major, `:371-392`; identical to `neighbors8`),
    /// written into `out`; returns the count.
    #[inline]
    pub fn neighbors_with_diag(&self, r: TileRef, out: &mut [TileRef; 8]) -> usize {
        self.neighbors8(r, out)
    }
    /// `forEachNeighborWithDiag(ref, callback)` (`:371-392`).
    #[inline]
    #[allow(dead_code)] // TS API (`AttackImpl.ts:158`, P6)
    pub(crate) fn for_each_neighbor_with_diag(&self, r: TileRef, mut f: impl FnMut(TileRef)) {
        let mut buf = [0; 8];
        let n = self.neighbors8(r, &mut buf);
        for &t in &buf[..n] {
            f(t);
        }
    }
    /// `|x1 - x2| + |y1 - y2|` (`:400-404`).
    pub fn manhattan_dist(&self, c1: TileRef, c2: TileRef) -> u32 {
        self.x(c1).abs_diff(self.x(c2)) + self.y(c1).abs_diff(self.y(c2))
    }
    /// `dx * dx + dy * dy` (`:405-409`).
    pub fn euclidean_dist_squared(&self, c1: TileRef, c2: TileRef) -> u32 {
        let dx = self.x(c1).abs_diff(self.x(c2));
        let dy = self.y(c1).abs_diff(self.y(c2));
        dx * dx + dy * dy
    }
    /// `circleSearch(tile, radius, filter?)` (`:410-432`): insertion-ordered result, columns
    /// (x) outer, rows (y) inner. No filter = `|_, _| true`.
    pub fn circle_search(
        &self,
        tile: TileRef,
        radius: f64,
        mut filter: impl FnMut(TileRef, f64) -> bool,
    ) -> OrderedTileSet {
        let cx = f64::from(self.x(tile));
        let cy = f64::from(self.y(tile));
        let mut tiles = OrderedTileSet::new();
        let min_x = of_det::js::max(0.0, cx - radius);
        let max_x = of_det::js::min(f64::from(self.w) - 1.0, cx + radius);
        let min_y = of_det::js::max(0.0, cy - radius);
        let max_y = of_det::js::min(f64::from(self.h) - 1.0, cy + radius);
        // TS indexes `yToRef[j]` with the loop values; a fractional start would read `undefined`
        // (NaN tiles). Every caller passes an integer nuke magnitude (`NukeAlliance.ts:25,74`).
        if min_x <= max_x && min_y <= max_y {
            assert!(
                min_x.fract() == 0.0 && min_y.fract() == 0.0,
                "circleSearch with a fractional radius {radius} (unported TS NaN-tile behaviour)"
            );
        }
        let r2 = radius * radius;
        let mut i = min_x;
        while i <= max_x {
            let mut j = min_y;
            while j <= max_y {
                // in-range: 0 <= i < w, 0 <= j < h integers
                let t = (j as u32) * self.w + i as u32;
                let d2 = f64::from(self.euclidean_dist_squared(tile, t));
                j += 1.0;
                if d2 > r2 {
                    continue;
                }
                if filter(t, d2) {
                    tiles.add(t);
                }
            }
            i += 1.0;
        }
        tiles
    }
    /// `bfsSearch(tile, filter, onVisit?)` (`:472-500`) on this map's scratch (`sc.bfs_main` for
    /// the main map, `sc.bfs_mini` for the minimap). `on_visit` returning `false` stops it (no
    /// `onVisit` = `|_| true`; TS `void` returns count as "continue").
    pub fn bfs_search(
        &self,
        sc: &mut BfsScratch,
        tile: TileRef,
        mut filter: impl FnMut(&GameMap, TileRef) -> bool,
        mut on_visit: impl FnMut(TileRef) -> bool,
    ) -> bool {
        // TS: `if (this.inBfs) throw …; this.inBfs = true; try { … } finally { … }`
        sc.enter();
        let g = BfsGuard(sc);
        let sc = &mut *g.0;
        if !filter(self, tile) {
            return true;
        }
        let gen_ = sc.bump_generation(self.num_tiles() as usize);
        sc.queue.clear();
        sc.visited[tile as usize] = gen_ as u16; // in-range: valid ref; gen < 0xffff
        sc.queue.push(tile);
        let w = self.w;
        let last_row = (self.h - 1) * w;
        let mut head = 0;
        while head < sc.queue.len() {
            let curr = sc.queue[head];
            head += 1;
            if !on_visit(curr) {
                return false;
            }
            // expandNeighbors (`:458-470`): N, S, W, E through tryPushNeighbor (`:447-456`).
            let x = curr % w;
            let mut try_push = |n: TileRef, sc: &mut BfsScratch| {
                if u32::from(sc.visited[n as usize]) != gen_ && filter(self, n) {
                    sc.visited[n as usize] = gen_ as u16;
                    sc.queue.push(n);
                }
            };
            if curr >= w {
                try_push(curr - w, sc);
            }
            if curr < last_row {
                try_push(curr + w, sc);
            }
            if x != 0 {
                try_push(curr - 1, sc);
            }
            if x != w - 1 {
                try_push(curr + 1, sc);
            }
        }
        true
    }
    /// `bfs(tile, filter)` (`:502-508`).
    pub fn bfs(
        &self,
        sc: &mut BfsScratch,
        tile: TileRef,
        filter: impl FnMut(&GameMap, TileRef) -> bool,
    ) -> OrderedTileSet {
        let mut result = OrderedTileSet::new();
        self.bfs_search(sc, tile, filter, |t| {
            result.add(t);
            true
        });
        result
    }
    /// `tileStateBuffer()`: the live buffer (`None` for the minimap, whose state is all zeros).
    pub fn tile_state_buffer(&self) -> Option<&[u16]> {
        self.state.as_slice()
    }
    /// `tileState(tile)`.
    #[inline]
    pub fn tile_state(&self, t: TileRef) -> u16 {
        self.state.get(t)
    }
    /// `updateTile(tile, packed)` (`:523-551`; restore/client path): returns whether terrain changed.
    pub fn update_tile(&mut self, t: TileRef, packed: u32) -> bool {
        let state = (packed & 0xffff) as u16;
        let terrain_byte = ((packed >> 16) & 0xff) as u8;

        let existing_fallout = self.has_fallout(t);
        self.state.set_raw(t, state);
        let new_fallout = self.has_fallout(t);
        if existing_fallout && !new_fallout {
            self.num_tiles_with_fallout -= 1;
        }
        if !existing_fallout && new_fallout {
            self.num_tiles_with_fallout += 1;
        }

        let terrain_changed = self.tb(t) != terrain_byte;
        if terrain_changed {
            let was_land = self.is_land(t);
            self.note_terrain_edit(t);
            self.terrain_mut()[t as usize] = terrain_byte; // in-range: valid ref
            let is_now_land = terrain_byte & (1 << IS_LAND_BIT) != 0;
            if was_land != is_now_land {
                self.water_version += 1;
            }
            if was_land && !is_now_land {
                self.num_land_tiles -= 1;
            } else if !was_land && is_now_land {
                self.num_land_tiles += 1;
            }
        }
        terrain_changed
    }
}

/// `Number.isInteger(x)`.
#[inline]
fn is_integer(x: f64) -> bool {
    x.is_finite() && x.trunc() == x
}

// ---- distance predicates (`GameMapImpl.ts:665-788`), for `bfs`/`bfsSearch` filters ----
// Callers: `execution/Util.ts:21` (P6), `PlayerImpl.ts:1947-1949` (P2),
// `NationNukeBehavior.ts:999,1026` (P11); they arrive with those packages.

/// `euclDistFN(root, dist, center = false)` (`:665-686`).
#[allow(dead_code)]
pub(crate) fn eucl_dist_fn(root: TileRef, dist: f64, center: bool) -> impl Fn(&GameMap, TileRef) -> bool + Copy {
    let dist2 = dist * dist;
    move |gm: &GameMap, n: TileRef| {
        if !center {
            f64::from(gm.euclidean_dist_squared(root, n)) <= dist2
        } else {
            let root_x = f64::from(gm.x(root)) - 0.5;
            let root_y = f64::from(gm.y(root)) - 0.5;
            let dx = f64::from(gm.x(n)) - root_x;
            let dy = f64::from(gm.y(n)) - root_y;
            dx * dx + dy * dy <= dist2
        }
    }
}

/// `manhattanDistFN(root, dist, center = false)` (`:688-704`).
#[allow(dead_code)]
pub(crate) fn manhattan_dist_fn(root: TileRef, dist: f64, center: bool) -> impl Fn(&GameMap, TileRef) -> bool + Copy {
    move |gm: &GameMap, n: TileRef| {
        if !center {
            f64::from(gm.manhattan_dist(root, n)) <= dist
        } else {
            let root_x = f64::from(gm.x(root)) - 0.5;
            let root_y = f64::from(gm.y(root)) - 0.5;
            let dx = (f64::from(gm.x(n)) - root_x).abs();
            let dy = (f64::from(gm.y(n)) - root_y).abs();
            dx + dy <= dist
        }
    }
}

/// `rectDistFN(root, dist, center = false)` (`:706-726`).
#[allow(dead_code)]
pub(crate) fn rect_dist_fn(root: TileRef, dist: f64, center: bool) -> impl Fn(&GameMap, TileRef) -> bool + Copy {
    move |gm: &GameMap, n: TileRef| {
        let (dx, dy) = if !center {
            ((f64::from(gm.x(n)) - f64::from(gm.x(root))).abs(), (f64::from(gm.y(n)) - f64::from(gm.y(root))).abs())
        } else {
            let root_x = f64::from(gm.x(root)) - 0.5;
            let root_y = f64::from(gm.y(root)) - 0.5;
            ((f64::from(gm.x(n)) - root_x).abs(), (f64::from(gm.y(n)) - root_y).abs())
        };
        dx <= dist && dy <= dist
    }
}

/// `isometricDistFN(root, dist, center = false)` (`:739-759`, with `isInIsometricTile`, `:728-737`).
#[allow(dead_code)]
pub(crate) fn isometric_dist_fn(root: TileRef, dist: f64, center: bool) -> impl Fn(&GameMap, TileRef) -> bool + Copy {
    move |gm: &GameMap, n: TileRef| {
        if !center {
            f64::from(gm.manhattan_dist(root, n)) <= dist
        } else {
            let root_x = f64::from(gm.x(root)) - 0.5;
            let root_y = f64::from(gm.y(root)) - 0.5;
            // isInIsometricTile(center, tile, yOffset = 0, distance)
            let dx = (f64::from(gm.x(n)) - root_x).abs();
            let dy = (f64::from(gm.y(n)) - (root_y + 0.0)).abs();
            dx + dy * 2.0 <= dist + 1.0
        }
    }
}

/// `hexDistFN(root, dist, center = false)` (`:761-781`).
#[allow(dead_code)]
pub(crate) fn hex_dist_fn(root: TileRef, dist: f64, center: bool) -> impl Fn(&GameMap, TileRef) -> bool + Copy {
    move |gm: &GameMap, n: TileRef| {
        let (dx, dy) = if !center {
            ((f64::from(gm.x(n)) - f64::from(gm.x(root))).abs(), (f64::from(gm.y(n)) - f64::from(gm.y(root))).abs())
        } else {
            let root_x = f64::from(gm.x(root)) - 0.5;
            let root_y = f64::from(gm.y(root)) - 0.5;
            ((f64::from(gm.x(n)) - root_x).abs(), (f64::from(gm.y(n)) - root_y).abs())
        };
        dx <= dist && dy <= dist && dx + dy <= dist * 1.5
    }
}

/// `andFN(x, y)` (`:783-788`): short-circuit `x(gm, tile) && y(gm, tile)`.
#[allow(dead_code)]
pub(crate) fn and_fn(
    mut x: impl FnMut(&GameMap, TileRef) -> bool,
    mut y: impl FnMut(&GameMap, TileRef) -> bool,
) -> impl FnMut(&GameMap, TileRef) -> bool {
    move |gm: &GameMap, t: TileRef| x(gm, t) && y(gm, t)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tile_state_bits_match_gamemapimpl() {
        // GameMapImpl.ts:37-41: bits 0-11 owner, 13 fallout, 14 defense bonus.
        let mut b = TileStateBuf::new_full(4);
        b.set_owner_id(1, 0xabc);
        b.set_fallout(1, true);
        b.set_defense_bonus(1, true);
        assert_eq!(b.get(1), 0xabc | (1 << 13) | (1 << 14));
        b.set_owner_id(1, 5); // keeps the high bits: (s & ~0xfff) | id
        assert_eq!(b.get(1), 5 | (1 << 13) | (1 << 14));
        b.set_fallout(1, false);
        b.set_defense_bonus(1, false);
        assert_eq!(b.get(1), 5);
        assert_eq!(b.get(0), 0);
        let z = TileStateBuf::new_zero(4);
        assert_eq!((z.get(3), z.len(), z.as_slice().is_none()), (0, 4, true));
    }

    #[test]
    #[allow(clippy::disallowed_methods)] // asserting a Rust-bug panic
    fn minimap_state_write_is_a_rust_bug() {
        let r = std::panic::catch_unwind(|| {
            let mut z = TileStateBuf::new_zero(4);
            z.set_fallout(0, true);
        });
        assert!(r.is_err());
    }
}
