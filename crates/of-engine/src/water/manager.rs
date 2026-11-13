//! `WaterManager` (`engine/src/game/WaterManager.ts`). Owner: **P4a**. ARCHITECTURE §3.8.
//!
//! State (CS W): the scalars and the two pending Sets; the current graph `Arc` (the HPA always
//! routes on it), the live components `Arc` (COW), the per-game segment visibility and the
//! water chain slots (P4b's `ChainSlots`, `PathFinder.ts:42-45`). Stamp arrays and the builder
//! BFS are scratch (`sc.water_mgr`). The full main + minimap edit sequence (setWater,
//! setOcean, the magnitude BFS → `setMagnitude`, `:493-881`) is ported for both maps (G10).
//!
//! The algorithms are written once over [`WaterMapMut`] (see `water::map_view`); the `Ctx` /
//! `Rcx` entry points below split the borrows of `State` and call them with the real maps.

use crate::StateClone;
use crate::ctx::{Ctx, Rcx};
use crate::ids::TileRef;
use crate::jsorder::{JsMap, JsSet, LookupMap, LookupSet};
use crate::map::GameMap;
use crate::path::pathfinder::ChainSlots;
use crate::scratch::water::{BfsGridScratch, WaterMgrScratch};
use crate::water::abstract_graph::{CLUSTER_SIZE, SegVisibility, WaterGraph};
use crate::water::components::{Components, ComponentsSnapshot};
use crate::water::map_view::{WaterMap, WaterMapMut};
use std::sync::Arc;

/// `WATER_GRAPH_REBUILD_INTERVAL` (`WaterManager.ts:20`): at most one rebuild every 20 ticks.
pub const WATER_GRAPH_REBUILD_INTERVAL: i32 = 20;
/// `MAX_MAG_DIST = 62` (`:24`): magnitude = ceil(dist / 2) capped at 31.
pub const MAX_MAG_DIST: u32 = 62;
/// `TERRAIN_LAND_MASK` (`:31`).
const TERRAIN_LAND_MASK: u8 = 0x80;
/// `TERRAIN_MAG_MASK` (`:32`).
const TERRAIN_MAG_MASK: u8 = 0x1f;
/// `IMPASSABLE_MAG` (`:33`).
const IMPASSABLE_MAG: u8 = 31;

/// `WaterManager` data, `WaterManager v1` snapshot order, then non-snapshot state.
#[derive(StateClone, Debug, Default)]
pub struct WaterManager {
    /// `_waterGraphVersion` (`:52`).
    pub water_graph_version: u32,
    pub water_graph_dirty: bool,
    pub water_graph_last_rebuild_tick: i32,
    /// `_pendingWaterTiles: Set<TileRef>` (`:56`).
    pub pending_water_tiles: JsSet<TileRef>,
    /// `_dirtyMiniTiles: Set<TileRef>` (`:57`).
    pub dirty_mini_tiles: JsSet<TileRef>,
    /// `_miniWaterCC` (`None` when `disableNavMesh`).
    pub components: Option<Arc<Components>>,
    /// non-snapshot: `_miniWaterGraph` (+ its HPA); starts as `statics.graph0`.
    pub graph: Option<Arc<WaterGraph>>,
    /// non-snapshot: per-game `_pathCache` visibility.
    pub seg_vis: SegVisibility,
    /// non-snapshot: `_waterChainCache` (P4b, `PathFinder.ts:42-45,145-162`).
    pub chains: ChainSlots,
}

/// `WaterManagerState` (`:889-910`): the snapshot/restore record (CS W.water).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WaterManagerSnapshot {
    pub water_graph_version: u32,
    pub water_graph_dirty: bool,
    pub water_graph_last_rebuild_tick: i32,
    pub pending_water_tiles: Vec<TileRef>,
    pub dirty_mini_tiles: Vec<TileRef>,
    /// `null` until the minimap's `waterVersion() > 0` (`:104-107`).
    pub components: Option<ComponentsSnapshot>,
}

/// `CraterBounds` (`:35-40`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CraterBounds {
    pub min_x: i64,
    pub max_x: i64,
    pub min_y: i64,
    pub max_y: i64,
}

/// The version-0 water data built once per map (the `WaterManager` constructor body,
/// `:74-92`, on the pristine minimap): `new ConnectedComponents(miniMap).initialize()` and
/// the full graph build on those components. P3's `MapStatics::load` stores the result as
/// `graph0` / `components0` (ARCHITECTURE §3.8, review 1 #19).
pub fn build_version0(mini: &GameMap) -> (Arc<WaterGraph>, Arc<Components>) {
    let mut bfs = BfsGridScratch::default();
    let mut queue = Vec::new();
    build_version0_with(mini, &mut bfs, &mut queue)
}

pub(crate) fn build_version0_with<M: WaterMap + ?Sized>(
    mini: &M,
    bfs: &mut BfsGridScratch,
    queue: &mut Vec<i32>,
) -> (Arc<WaterGraph>, Arc<Components>) {
    let mut cc = Components::new_with(mini);
    cc.initialize_with(mini, true, queue);
    let graph = WaterGraph::build_with(mini, CLUSTER_SIZE, None, None, Some(&cc), bfs);
    (Arc::new(graph), Arc::new(cc))
}

impl WaterManager {
    /// `new WaterManager(map, miniMap, disableNavMesh)` (`:69-93`) from the shared statics.
    /// `None`/`None` is the `disableNavMesh` manager.
    pub fn new(graph0: Option<Arc<WaterGraph>>, components0: Option<Arc<Components>>) -> Self {
        let seg_vis = graph0.as_deref().map(SegVisibility::for_graph).unwrap_or_default();
        WaterManager {
            water_graph_version: 0,
            water_graph_dirty: false,
            water_graph_last_rebuild_tick: 0,
            pending_water_tiles: JsSet::default(),
            dirty_mini_tiles: JsSet::default(),
            components: components0,
            graph: graph0,
            seg_vis,
            chains: ChainSlots::default(),
        }
    }

    /// `waterGraphVersion()` (`:217-219`).
    pub fn water_graph_version(&self) -> u32 {
        self.water_graph_version
    }

    /// `queueTile(tile)` (`:139-141`).
    pub fn queue_tile(&mut self, tile: TileRef) {
        self.pending_water_tiles.insert(tile);
    }

    /// `graph.getComponentId(tile)` (`AbstractGraph.ts:142-144`): every graph reads the one
    /// live `ConnectedComponents` (`_waterComponents?.getComponentId(tile) ?? 0`). This is
    /// the chain's `componentCheckFn` (`PathFinder.ts:60`) for old and new chains alike.
    pub fn graph_component_id(&self, tile: TileRef) -> u32 {
        self.components.as_deref().map_or(0, |c| c.get_component_id(tile))
    }

    /// `graph.getComponentSize(componentId)` (`AbstractGraph.ts:147-149`).
    pub fn graph_component_size(&self, component_id: u32) -> u32 {
        self.components.as_deref().map_or(0, |c| c.get_component_size(component_id))
    }

    /// `snapshot()` (`:95-109`). `mini_water_version` is `miniMap.waterVersion()`.
    pub fn snapshot(&self, mini_water_version: u32) -> WaterManagerSnapshot {
        WaterManagerSnapshot {
            water_graph_version: self.water_graph_version,
            water_graph_dirty: self.water_graph_dirty,
            water_graph_last_rebuild_tick: self.water_graph_last_rebuild_tick,
            pending_water_tiles: self.pending_water_tiles.iter().copied().collect(),
            dirty_mini_tiles: self.dirty_mini_tiles.iter().copied().collect(),
            components: if mini_water_version > 0 {
                self.components.as_deref().and_then(|c| c.snapshot())
            } else {
                None
            },
        }
    }

    /// `restoreSnapshot(s)` (`:119-137`) on a manager just built for the restored maps: the
    /// graph is **rebuilt** from the restored components (TS restore semantics, SPEC §1 GAP:
    /// a stale graph is not reproduced), with fresh segment visibility.
    pub fn restore_snapshot(&mut self, mini: &GameMap, s: &WaterManagerSnapshot, bfs: &mut BfsGridScratch) {
        self.restore_snapshot_with(mini, s, bfs)
    }

    pub(crate) fn restore_snapshot_with<M: WaterMap + ?Sized>(
        &mut self,
        mini: &M,
        s: &WaterManagerSnapshot,
        bfs: &mut BfsGridScratch,
    ) {
        self.water_graph_version = s.water_graph_version;
        self.water_graph_dirty = s.water_graph_dirty;
        self.water_graph_last_rebuild_tick = s.water_graph_last_rebuild_tick;
        self.pending_water_tiles = s.pending_water_tiles.iter().copied().collect();
        self.dirty_mini_tiles = s.dirty_mini_tiles.iter().copied().collect();
        if let (Some(cs), Some(cc)) = (s.components.as_ref(), self.components.as_mut()) {
            Arc::make_mut(cc).restore_snapshot(cs);
            let graph = WaterGraph::build_with(mini, CLUSTER_SIZE, None, None, Some(&**cc), bfs);
            self.seg_vis.reset(&graph); // HPA.setGraph: path caches live on the new graph
            self.graph = Some(Arc::new(graph));
        }
    }

    /// `tick(currentTick)` (`:147-215`) over any map pair (the body of [`Ctx::water_tick`]).
    pub(crate) fn tick_with<M: WaterMapMut + ?Sized>(
        &mut self,
        sc: &mut WaterMgrScratch,
        map: &mut M,
        mini: &mut M,
        disable_nav_mesh: impl FnOnce() -> bool,
        current_tick: i32,
    ) -> Vec<TileRef> {
        let mut changed_tiles: Vec<TileRef> = Vec::new();
        let mut converted_this_tick = false;
        // TS: :151
        if !self.pending_water_tiles.is_empty() {
            converted_this_tick = self.flush_pending_with(sc, map, mini, &mut changed_tiles);
        }
        // TS: :175-184 (left-to-right short-circuit)
        if self.water_graph_dirty
            && !disable_nav_mesh()
            && !converted_this_tick
            && i64::from(current_tick) - i64::from(self.water_graph_last_rebuild_tick)
                >= i64::from(WATER_GRAPH_REBUILD_INTERVAL)
        {
            self.rebuild_graph_with(&mut sc.builder_bfs, mini, current_tick);
        }
        changed_tiles
    }

    /// `tick` `:151-172` over any map pair (see [`Ctx::water_flush_pending`]).
    pub(crate) fn flush_pending_with<M: WaterMapMut + ?Sized>(
        &mut self,
        sc: &mut WaterMgrScratch,
        map: &mut M,
        mini: &mut M,
        changed: &mut Vec<TileRef>,
    ) -> bool {
        let mut converted: Vec<TileRef> = Vec::new();
        // iter-live: the loop body never touches `_pendingWaterTiles`
        for &tile in self.pending_water_tiles.iter() {
            // TS: :155-165 tile may have been conquered between queueing and flushing
            if map.is_land(tile) && !map.has_owner(tile) && !map.is_impassable(tile) {
                if map.has_fallout(tile) {
                    map.set_fallout(tile, false);
                }
                map.set_water(tile);
                converted.push(tile);
            }
        }
        self.pending_water_tiles.clear();
        if !converted.is_empty() {
            self.finalize_water_changes(sc, map, mini, &converted, changed);
            return true;
        }
        false
    }

    /// `tick` `:185-211` over any minimap (see [`Ctx::water_rebuild_graph`]).
    pub(crate) fn rebuild_graph_with<M: WaterMap + ?Sized>(
        &mut self,
        bfs: &mut BfsGridScratch,
        mini: &M,
        current_tick: i32,
    ) {
        self.water_graph_dirty = false;
        self.water_graph_last_rebuild_tick = current_tick;
        let dirty = if self.dirty_mini_tiles.is_empty() { None } else { Some(&self.dirty_mini_tiles) };
        let graph =
            WaterGraph::build_with(mini, CLUSTER_SIZE, self.graph.as_deref(), dirty, self.components.as_deref(), bfs);
        self.dirty_mini_tiles.clear();
        // TS: :199-208 `setGraph` (or a new HPA): the HPA now routes on the new graph and the
        // segment cache starts empty (`_initPathCache` of the new graph).
        self.seg_vis.reset(&graph);
        self.graph = Some(Arc::new(graph));
        self.water_graph_version += 1;
    }

    /// `getWaterComponent(tile)` (`:229-257`) over any map pair.
    pub(crate) fn water_component_with<M: WaterMap + ?Sized>(&self, map: &M, mini: &M, tile: TileRef) -> Option<u32> {
        if self.graph.is_none() {
            return Some(0); // permissive fallback (disableNavMesh)
        }
        let mini_x = map.x(tile) / 2; // Math.floor (non-negative)
        let mini_y = map.y(tile) / 2;
        let mini_tile = mini.ref_(f64::from(mini_x), f64::from(mini_y));
        if mini.is_water(mini_tile) {
            return Some(self.graph_component_id(mini_tile));
        }
        let mut nb = [0; 4];
        let mut nb2 = [0; 4];
        let k = mini.neighbors4(mini_tile, &mut nb);
        for &n in &nb[..k] {
            if mini.is_water(n) {
                return Some(self.graph_component_id(n));
            }
        }
        for &n in &nb[..k] {
            let k2 = mini.neighbors4(n, &mut nb2);
            for &n2 in &nb2[..k2] {
                if mini.is_water(n2) {
                    return Some(self.graph_component_id(n2));
                }
            }
        }
        None
    }

    /// `hasWaterComponent(tile, component)` (`:259-297`) over any map pair.
    pub(crate) fn has_water_component_with<M: WaterMap + ?Sized>(
        &self,
        map: &M,
        mini: &M,
        tile: TileRef,
        component: u32,
    ) -> bool {
        if self.graph.is_none() {
            return true;
        }
        let mini_x = map.x(tile) / 2;
        let mini_y = map.y(tile) / 2;
        let mini_tile = mini.ref_(f64::from(mini_x), f64::from(mini_y));
        if mini.is_water(mini_tile) && self.graph_component_id(mini_tile) == component {
            return true;
        }
        let mut nb = [0; 4];
        let mut nb2 = [0; 4];
        let k = mini.neighbors4(mini_tile, &mut nb);
        for &n in &nb[..k] {
            if mini.is_water(n) && self.graph_component_id(n) == component {
                return true;
            }
        }
        for &n in &nb[..k] {
            let k2 = mini.neighbors4(n, &mut nb2);
            for &n2 in &nb2[..k2] {
                if mini.is_water(n2) && self.graph_component_id(n2) == component {
                    return true;
                }
            }
        }
        false
    }

    /// `getWaterComponentSize(tile)` (`:307-312`) over any map pair.
    pub(crate) fn water_component_size_with<M: WaterMap + ?Sized>(
        &self,
        map: &M,
        mini: &M,
        tile: TileRef,
    ) -> Option<u32> {
        let component_id = self.water_component_with(map, mini, tile)?;
        if self.graph.is_none() {
            return Some(0);
        }
        Some(self.graph_component_size(component_id) * 4)
    }

    /// `finalizeWaterChanges(convertedTiles, changedTiles)` (`:314-611`).
    fn finalize_water_changes<M: WaterMapMut + ?Sized>(
        &mut self,
        sc: &mut WaterMgrScratch,
        map: &mut M,
        mini: &mut M,
        converted_tiles: &[TileRef],
        changed_tiles: &mut Vec<TileRef>,
    ) {
        // TS: :318 `new Set(convertedTiles)` (the tiles come from a Set: already unique)
        let mut converted: Vec<TileRef> = Vec::with_capacity(converted_tiles.len());
        let mut converted_has: LookupSet<TileRef> = LookupSet::default(); // lookup-only: `converted.has`
        for &t in converted_tiles {
            if converted_has.insert(t) {
                converted.push(t);
            }
        }
        if converted.is_empty() {
            return;
        }
        let w = map.width();
        let total_tiles = w * map.height();
        // TS: :327 `changed` (Set, drained in insertion order at :608-610)
        let mut changed: JsSet<TileRef> = JsSet::default();
        for &t in &converted {
            changed.insert(t);
        }
        // TS: :332-343 cardinal neighbours N, S, W, E written from `start`
        let push_neighbors = |tile: TileRef, out: &mut [TileRef; 8], mut start: usize| -> usize {
            if tile >= w {
                out[start] = tile - w;
                start += 1;
            }
            if tile < total_tiles - w {
                out[start] = tile + w;
                start += 1;
            }
            let x = tile % w;
            if x > 0 {
                out[start] = tile - 1;
                start += 1;
            }
            if x < w - 1 {
                out[start] = tile + 1;
                start += 1;
            }
            start
        };
        let mut nb = [0u32; 8];

        // ── 1. Propagate ocean bit (`:350-372`)
        let mut ocean_queue: Vec<TileRef> = Vec::new();
        for &tile in &converted {
            let end = push_neighbors(tile, &mut nb, 0);
            for &n in &nb[..end] {
                if !converted_has.contains(&n) && map.is_ocean(n) {
                    map.set_ocean(tile);
                    ocean_queue.push(tile);
                    break;
                }
            }
        }
        let mut o_head = 0;
        while o_head < ocean_queue.len() {
            let tile = ocean_queue[o_head];
            o_head += 1;
            let end = push_neighbors(tile, &mut nb, 0);
            for &n in &nb[..end] {
                if map.is_water(n) && !map.is_ocean(n) {
                    map.set_ocean(n);
                    changed.insert(n);
                    ocean_queue.push(n);
                }
            }
        }

        // ── 2. Magnitude BFS (`:377-399`)
        let tt = total_tiles as usize;
        if sc.water_dist.len() != tt {
            sc.water_dist = vec![0; tt];
            sc.water_stamp_arr = vec![0; tt];
            sc.water_stamp = 0;
        }
        let groups = compute_crater_groups(&converted, w, MAX_MAG_DIST);
        for g in &groups {
            let stamp = bump_stamp(&mut sc.water_stamp, &mut sc.water_stamp_arr);
            recompute_magnitudes_in_box(
                map,
                g,
                &mut sc.water_stamp_arr,
                &mut sc.water_dist,
                stamp,
                true, // impassable terrain is void, not coastline
                Some(&mut changed),
            );
        }

        // ── 3. Fix shoreline bits (`:408-461`)
        let shore_stamp = bump_stamp(&mut sc.water_stamp, &mut sc.water_stamp_arr);
        let mut tiles_to_check: Vec<TileRef> = Vec::new();
        {
            let arr = &mut sc.water_stamp_arr;
            let mut push_to_check = |tile: TileRef| {
                let ti = tile as usize; // in-range: valid main-map refs
                if u32::from(arr[ti]) != shore_stamp {
                    arr[ti] = shore_stamp as u16;
                    tiles_to_check.push(tile);
                }
            };
            for &tile in &converted {
                push_to_check(tile);
                let end = push_neighbors(tile, &mut nb, 0);
                for i in 0..end {
                    push_to_check(nb[i]);
                    let end2 = push_neighbors(nb[i], &mut nb, end);
                    for &t in &nb[end..end2] {
                        push_to_check(t);
                    }
                }
            }
        }
        for &tile in &tiles_to_check {
            if map.is_impassable(tile) {
                if map.is_shoreline(tile) {
                    map.clear_shoreline_bit(tile);
                    changed.insert(tile);
                }
                continue;
            }
            let tile_is_land = map.is_land(tile);
            let mut has_opposite = false;
            let end = push_neighbors(tile, &mut nb, 0);
            for &n in &nb[..end] {
                if map.is_impassable(n) {
                    continue;
                }
                if map.is_land(n) != tile_is_land {
                    has_opposite = true;
                    break;
                }
            }
            let old_shoreline = map.is_shoreline(tile);
            if has_opposite {
                if !old_shoreline {
                    map.set_shoreline_bit(tile);
                    changed.insert(tile);
                }
            } else if old_shoreline {
                map.clear_shoreline_bit(tile);
                changed.insert(tile);
            }
        }

        // ── 4. Update minimap terrain (`:466-496`)
        let mut mini_tiles_to_check: JsSet<TileRef> = JsSet::default();
        let mut converted_mini_tiles: JsSet<TileRef> = JsSet::default();
        for &tile in &converted {
            let mini_x = map.x(tile) / 2;
            let mini_y = map.y(tile) / 2;
            if mini.is_valid_coord(f64::from(mini_x), f64::from(mini_y)) {
                mini_tiles_to_check.insert(mini.ref_(f64::from(mini_x), f64::from(mini_y)));
            }
        }
        for &mini_tile in mini_tiles_to_check.iter() {
            if !mini.is_land(mini_tile) {
                continue;
            }
            let fx = mini.x(mini_tile) * 2;
            let fy = mini.y(mini_tile) * 2;
            let mut is_water = false;
            'outer: for dy in 0..2u32 {
                for dx in 0..2u32 {
                    let (x, y) = (f64::from(fx + dx), f64::from(fy + dy));
                    if map.is_valid_coord(x, y) && map.is_water(map.ref_(x, y)) {
                        is_water = true;
                        break 'outer;
                    }
                }
            }
            if is_water {
                mini.set_water(mini_tile); // no-op on an impassable mini tile (still "converted")
                converted_mini_tiles.insert(mini_tile);
            }
        }

        // ── 4b. Minimap ocean + magnitude for converted tiles (`:507-592`)
        if !converted_mini_tiles.is_empty() {
            let mini_w = mini.width();
            let mini_h = mini.height();
            let push_mini_neighbors = |tile: TileRef, out: &mut [TileRef; 4]| -> usize {
                let x = tile % mini_w;
                let y = (tile - x) / mini_w;
                let mut count = 0;
                if y > 0 {
                    out[count] = tile - mini_w;
                    count += 1;
                }
                if y < mini_h - 1 {
                    out[count] = tile + mini_w;
                    count += 1;
                }
                if x > 0 {
                    out[count] = tile - 1;
                    count += 1;
                }
                if x < mini_w - 1 {
                    out[count] = tile + 1;
                    count += 1;
                }
                count
            };
            let mut mini_nb = [0u32; 4];
            // 4b-i (`:530-556`)
            let mut mini_ocean_queue: Vec<TileRef> = Vec::new();
            for &mt in converted_mini_tiles.iter() {
                let nc = push_mini_neighbors(mt, &mut mini_nb);
                let near_ocean = mini_nb[..nc].iter().any(|&n| mini.is_ocean(n));
                if near_ocean {
                    mini.set_ocean(mt);
                    mini_ocean_queue.push(mt);
                }
            }
            let mut mo_head = 0;
            while mo_head < mini_ocean_queue.len() {
                let tile = mini_ocean_queue[mo_head];
                mo_head += 1;
                let nc = push_mini_neighbors(tile, &mut mini_nb);
                for &n in &mini_nb[..nc] {
                    if mini.is_water(n) && !mini.is_ocean(n) {
                        mini.set_ocean(n);
                        mini_ocean_queue.push(n);
                    }
                }
            }
            // 4b-ii (`:570-591`)
            let mini_total = (mini_w as usize) * (mini_h as usize);
            if sc.mini_dist.len() != mini_total {
                sc.mini_dist = vec![0; mini_total];
                sc.mini_stamp_arr = vec![0; mini_total];
                sc.mini_stamp = 0;
            }
            let mini_conv: Vec<TileRef> = converted_mini_tiles.iter().copied().collect();
            let mini_groups = compute_crater_groups(&mini_conv, mini_w, MAX_MAG_DIST);
            for g in &mini_groups {
                let stamp = bump_stamp(&mut sc.mini_stamp, &mut sc.mini_stamp_arr);
                recompute_magnitudes_in_box(mini, g, &mut sc.mini_stamp_arr, &mut sc.mini_dist, stamp, false, None);
            }
        }

        // ── 5. Mark the water graph dirty (`:597-604`)
        if !converted_mini_tiles.is_empty() {
            let mini_conv: Vec<TileRef> = converted_mini_tiles.iter().copied().collect();
            if let Some(cc) = self.components.as_mut() {
                Arc::make_mut(cc).add_water_tiles_impl(&mini_conv); // COW (forks keep theirs)
            }
            self.water_graph_dirty = true;
            for &mt in &mini_conv {
                self.dirty_mini_tiles.insert(mt);
            }
        }
        // TS: :608-610 drain `changed` into the output
        changed_tiles.extend(changed.iter().copied());
    }
}

/// `bumpFullMapStamp()` / `bumpMiniStamp()` (`:613-629`): u16 stamps; at `0xffff` the array
/// is zeroed and the stamp restarts at 1 (single-pass safe, see `WaterMgrScratch`).
fn bump_stamp(stamp: &mut u32, arr: &mut [u16]) -> u32 {
    *stamp += 1;
    if *stamp >= 0xffff {
        arr.fill(0);
        *stamp = 1;
    }
    *stamp
}

/// `computeCraterGroups(converted, w, maxMagDist)` (`:645-748`).
pub(crate) fn compute_crater_groups(converted: &[TileRef], w: u32, max_mag_dist: u32) -> Vec<CraterBounds> {
    let cell_size = i64::from(max_mag_dist);
    let cells_x = i64::from(w).div_euclid(cell_size) + i64::from(i64::from(w) % cell_size != 0); // Math.ceil
    // TS: :654-669 per-cell bounds, cells in first-tile order (iterated below)
    let mut cells: JsMap<i64, CraterBounds> = JsMap::default();
    for &tile in converted {
        let tx = i64::from(tile % w);
        let ty = (i64::from(tile) - tx) / i64::from(w);
        let key = ty.div_euclid(cell_size) * cells_x + tx.div_euclid(cell_size);
        match cells.get_mut(&key) {
            None => {
                cells.insert(key, CraterBounds { min_x: tx, max_x: tx, min_y: ty, max_y: ty });
            }
            Some(b) => {
                if tx < b.min_x {
                    b.min_x = tx;
                }
                if tx > b.max_x {
                    b.max_x = tx;
                }
                if ty < b.min_y {
                    b.min_y = ty;
                }
                if ty > b.max_y {
                    b.max_y = ty;
                }
            }
        }
    }
    // TS: :673-702 union-find over occupied cells (8-neighbourhood)
    let keys: Vec<i64> = cells.keys().copied().collect();
    let mut parent: LookupMap<i64, i64> = LookupMap::with_capacity(keys.len()); // lookup-only: get/set
    for &k in &keys {
        parent.insert(k, k);
    }
    fn find(parent: &mut LookupMap<i64, i64>, k: i64) -> i64 {
        let pget = |p: &LookupMap<i64, i64>, x: i64| *p.get(&x).unwrap_or(&x);
        let mut root = k;
        while pget(parent, root) != root {
            root = pget(parent, root);
        }
        let mut cur = k;
        while pget(parent, cur) != root {
            let next = pget(parent, cur);
            parent.insert(cur, root);
            cur = next;
        }
        root
    }
    for &key in &keys {
        let cx = key % cells_x;
        let cy = (key - cx) / cells_x;
        for dy in -1i64..=1 {
            for dx in -1i64..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let nx = cx + dx;
                let ny = cy + dy;
                if nx < 0 || nx >= cells_x || ny < 0 {
                    continue;
                }
                let nk = ny * cells_x + nx;
                if !cells.contains_key(&nk) {
                    continue;
                }
                let ra = find(&mut parent, key);
                let rb = find(&mut parent, nk);
                if ra != rb {
                    parent.insert(ra.max(rb), ra.min(rb));
                }
            }
        }
    }
    // TS: :705-718 merge cell bounds per root (Map iteration order)
    let mut by_root: JsMap<i64, CraterBounds> = JsMap::default();
    for &key in &keys {
        let b = *cells.get(&key).unwrap_or_else(|| unreachable!("key from cells"));
        let root = find(&mut parent, key);
        match by_root.get_mut(&root) {
            None => {
                by_root.insert(root, b);
            }
            Some(g) => {
                if b.min_x < g.min_x {
                    g.min_x = b.min_x;
                }
                if b.max_x > g.max_x {
                    g.max_x = b.max_x;
                }
                if b.min_y < g.min_y {
                    g.min_y = b.min_y;
                }
                if b.max_y > g.max_y {
                    g.max_y = b.max_y;
                }
            }
        }
    }
    let mut groups: Vec<CraterBounds> = by_root.values().copied().collect();
    // TS: :722-746 greedy merges while the union box is cheaper
    let pad = 4 * i64::from(max_mag_dist);
    let box_area = |g: &CraterBounds| (g.max_x - g.min_x + pad) * (g.max_y - g.min_y + pad);
    let mut merged = true;
    while merged {
        merged = false;
        let mut i = 0;
        while i < groups.len() && !merged {
            for j in (i + 1)..groups.len() {
                let a = groups[i];
                let b = groups[j];
                let union = CraterBounds {
                    min_x: a.min_x.min(b.min_x),
                    max_x: a.max_x.max(b.max_x),
                    min_y: a.min_y.min(b.min_y),
                    max_y: a.max_y.max(b.max_y),
                };
                if box_area(&union) < box_area(&a) + box_area(&b) {
                    groups[i] = union;
                    groups.remove(j); // TS: splice(j, 1)
                    merged = true;
                    break;
                }
            }
            i += 1;
        }
    }
    groups
}

/// `recomputeMagnitudesInBox(map, bounds, stampArr, distArr, stamp, passableCoastOnly,
/// changed)` (`:776-886`). Reads terrain bytes directly, like TS; every read is of the live
/// bytes (the TS array is written in place by `setMagnitude` during the final loop).
fn recompute_magnitudes_in_box<M: WaterMapMut + ?Sized>(
    map: &mut M,
    bounds: &CraterBounds,
    stamp_arr: &mut [u16],
    dist_arr: &mut [u16],
    stamp: u32,
    passable_coast_only: bool,
    mut changed: Option<&mut JsSet<TileRef>>,
) {
    let w = i64::from(map.width());
    let h = i64::from(map.height());
    let m = i64::from(MAX_MAG_DIST);
    // Dirty box (`:791-794`) and seed box (`:796-799`).
    let d_min_x = (bounds.min_x - m).max(0);
    let d_max_x = (w - 1).min(bounds.max_x + m);
    let d_min_y = (bounds.min_y - m).max(0);
    let d_max_y = (h - 1).min(bounds.max_y + m);
    let s_min_x = (bounds.min_x - m * 2).max(0);
    let s_max_x = (w - 1).min(bounds.max_x + m * 2);
    let s_min_y = (bounds.min_y - m * 2).max(0);
    let s_max_y = (h - 1).min(bounds.max_y + m * 2);
    let is_coast_byte =
        |b: u8| (b & TERRAIN_LAND_MASK) != 0 && (!passable_coast_only || (b & TERRAIN_MAG_MASK) != IMPASSABLE_MAG);
    let st = stamp as u16; // stamps are < 0xffff
    let wu = w as usize;
    // TS: :806-824 seed from coastline water tiles inside the seed box
    let mut queue: Vec<usize> = Vec::new();
    {
        let terrain = map.terrain();
        for y in s_min_y..=s_max_y {
            let row_start = (y * w) as usize;
            for x in s_min_x..=s_max_x {
                let tile = row_start + x as usize;
                if (terrain[tile] & TERRAIN_LAND_MASK) != 0 {
                    continue;
                }
                if stamp_arr[tile] == st {
                    continue;
                }
                let is_coast = (y > 0 && is_coast_byte(terrain[tile - wu]))
                    || (y < h - 1 && is_coast_byte(terrain[tile + wu]))
                    || (x > 0 && is_coast_byte(terrain[tile - 1]))
                    || (x < w - 1 && is_coast_byte(terrain[tile + 1]));
                if is_coast {
                    stamp_arr[tile] = st;
                    dist_arr[tile] = 0;
                    queue.push(tile);
                }
            }
        }
        // TS: :827-865 BFS through water, clipped to the seed box
        let mut head = 0;
        while head < queue.len() {
            let tile = queue[head];
            head += 1;
            let next_dist = dist_arr[tile] as u32 + 1;
            let x = (tile % wu) as i64;
            let y = ((tile - x as usize) / wu) as i64;
            let mut visit = |n: usize, queue: &mut Vec<usize>| {
                if (terrain[n] & TERRAIN_LAND_MASK) == 0 && stamp_arr[n] != st {
                    stamp_arr[n] = st;
                    dist_arr[n] = next_dist as u16; // TS: Uint16Array store
                    queue.push(n);
                }
            };
            if y > s_min_y {
                visit(tile - wu, &mut queue);
            }
            if y < s_max_y {
                visit(tile + wu, &mut queue);
            }
            if x > s_min_x {
                visit(tile - 1, &mut queue);
            }
            if x < s_max_x {
                visit(tile + 1, &mut queue);
            }
        }
    }
    // TS: :868-885 update magnitudes only for dirty-box tiles
    for y in d_min_y..=d_max_y {
        let row_start = (y * w) as usize;
        for x in d_min_x..=d_max_x {
            let tile = row_start + x as usize;
            let b = map.terrain()[tile];
            if (b & TERRAIN_LAND_MASK) != 0 {
                continue;
            }
            let new_mag: u8 = if stamp_arr[tile] == st {
                // Math.min(Math.ceil(dist / 2), 31) on non-negative ints
                (u32::from(dist_arr[tile]).div_ceil(2)).min(31) as u8
            } else {
                31
            };
            if (b & TERRAIN_MAG_MASK) != new_mag {
                map.set_magnitude(tile as TileRef, new_mag);
                if let Some(c) = changed.as_deref_mut() {
                    c.insert(tile as TileRef);
                }
            }
        }
    }
}

impl Ctx<'_> {
    /// `game.queueWaterConversion(tile)` → `WaterManager.queueTile(tile)` (`:139-145`).
    pub fn water_queue_tile(&mut self, tile: TileRef) {
        self.st.water.queue_tile(tile);
    }
    /// `WaterManager.tick(currentTick)` (`:147-215`): flush pending conversions (both maps)
    /// and the throttled graph rebuild; returns the converted main-map tiles. The caller bumps
    /// `structure_version` if it is non-empty (`GameImpl.ts:657-662`).
    ///
    /// S0 (P1) ported the control flow literally so the tick loop runs on an unedited map; the
    /// two branch bodies are P4a's ([`Ctx::water_flush_pending`], [`Ctx::water_rebuild_graph`]).
    pub fn water_tick(&mut self, current_tick: i32) -> Vec<TileRef> {
        // `disableNavMesh` is the ctor argument, i.e. `config.disableNavMesh()`; read lazily at
        // its TS position in the `&&` chain.
        let st = &mut *self.st;
        let cfg = &st.cfg;
        st.water.tick_with(&mut self.sc.water_mgr, &mut st.map, &mut st.mini, || cfg.disable_nav_mesh(), current_tick)
    }
    /// `tick` `:152-175`: convert every still-eligible pending tile (`isLand && !hasOwner &&
    /// !isImpassable`, clearing fallout first), clear the set, and run `finalizeWaterChanges`
    /// on the converted tiles (pushing changed main-map tiles to `changed`). Returns
    /// `convertedThisTick`.
    #[allow(clippy::ptr_arg)] // P4a pushes onto it
    pub fn water_flush_pending(&mut self, changed: &mut Vec<TileRef>) -> bool {
        let st = &mut *self.st;
        st.water.flush_pending_with(&mut self.sc.water_mgr, &mut st.map, &mut st.mini, changed)
    }
    /// `tick` `:187-212`: `_waterGraphDirty = false`, `_waterGraphLastRebuildTick = currentTick`,
    /// rebuild the graph from `_dirtyMiniTiles`, clear them, swap the HPA graph,
    /// `_waterGraphVersion++`.
    pub fn water_rebuild_graph(&mut self, current_tick: i32) {
        let st = &mut *self.st;
        st.water.rebuild_graph_with(&mut self.sc.water_mgr.builder_bfs, &st.mini, current_tick);
    }
}

impl Rcx<'_> {
    /// `getWaterComponent(tile): number | null` (`:229-257`, main-map tile → minimap component).
    pub fn water_component(&mut self, tile: TileRef) -> Option<u32> {
        self.st.water.water_component_with(&self.st.map, &self.st.mini, tile)
    }
    /// `hasWaterComponent(tile, component)` (`:259-305`).
    pub fn has_water_component(&mut self, tile: TileRef, component: u32) -> bool {
        self.st.water.has_water_component_with(&self.st.map, &self.st.mini, tile, component)
    }
    /// `getWaterComponentSize(tile): number | null` (`:307-320`).
    pub fn water_component_size(&mut self, tile: TileRef) -> Option<u32> {
        self.st.water.water_component_size_with(&self.st.map, &self.st.mini, tile)
    }
}
