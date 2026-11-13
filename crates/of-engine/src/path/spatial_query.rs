//! `SpatialQuery` (`pathfinding/spatial/SpatialQuery.ts`). Owner: **P4b**.
//! The per-player `reachableComponents` memo is `Player.reachable_components_memo` (exact).
//! Scratch: `sc.spatial_bounded` (`boundedAStar`), `sc.traversal` (`bfsNearest`).
//!
//! **`closestShoreByWater` writes state.** It runs `PathFinding.Water(gm).findPath(..)`
//! (`:217`): that refreshes the game's water chain slot (`sharedWaterChain`) and, on the
//! HPA chain, fills `AbstractGraph._pathCache` segments, which later queries can hit
//! (history-dependent after a water nuke, ARCHITECTURE §3.8). The exact entry point is
//! therefore [`closest_shore_by_water_cx`] (`&mut Ctx`). The frozen S0 signature
//! [`closest_shore_by_water`] takes `&mut Rcx` (read-only state), so it runs the same query
//! on a copy of the segment visibility and drops the fills: identical results for that
//! call, but a later HPA query inside the same graph version after a terrain edit can
//! diverge from TS. Request R3 in `docs/requests/P4b.md` asks for the `Ctx` signature at
//! the TransportShipUtils call sites; until then P7 should call the `_cx` variant.
//!
//! The reachable-components memo is exact (keyed on `tileChangeVersion` + `waterVersion`,
//! and only `size`/`has` of the set are read), so a reader with `&State` answers from a
//! valid memo or recomputes without storing (P2 owns where it is refreshed, `state/player.rs`).
#![allow(clippy::collapsible_if)] // nested ifs kept as the TS writes them

use crate::ctx::{Ctx, Rcx};
use crate::ids::{Owner, SmallId, TileRef};
use crate::map::GameMap;
use crate::path::MapRead;
use crate::path::astar_water_bounded::{self, BoundedAStarConfig, SearchBounds};
use crate::path::pathfinder::{ChainKind, WaterStepper, chain_kind_for};
use crate::path::stepper::{StepFinder, StepperState};
use crate::path::types::{PathResult, Src};
use crate::scratch::traversal::TraversalScratch;
use crate::state::State;
use crate::water::abstract_graph::SegVisibility;

/// `REFINE_MAX_SEARCH_AREA = 100 * 100` (`:14`).
pub const REFINE_MAX_SEARCH_AREA: u32 = 100 * 100;

/// `bfsNearest(from, maxDist, predicate)` (`:45-97`): LIFO flood over the Manhattan
/// diamond (terrain ignored), neighbours N, S, W, E; strict `<` keeps the first candidate
/// at the minimum distance.
pub fn bfs_nearest<M: MapRead>(
    scratch: &mut TraversalScratch,
    map: &M,
    from: TileRef,
    max_dist: f64,
    mut predicate: impl FnMut(TileRef) -> bool,
) -> Option<TileRef> {
    if !map.is_valid_ref(f64::from(from)) {
        return None;
    }
    // `tileTraversalScratch(game)` + `bumpTraversalGeneration(scratch)` (`:57-58`)
    scratch.ensure(map.width() as usize * map.height() as usize);
    let generation = scratch.bump_generation();
    let TraversalScratch { visited, stack, .. } = scratch;
    stack.clear();

    let mut best: Option<TileRef> = None;
    let mut best_dist = f64::INFINITY;

    // `mark(t)` (`:69-79`)
    macro_rules! mark {
        ($t:expr) => {{
            let t: TileRef = $t;
            visited[t as usize] = generation; // in-range: t is a valid tile (from, or a neighbour)
            stack.push(t);
            if predicate(t) {
                let dist = f64::from(map.manhattan_dist(from, t));
                if dist < best_dist {
                    best = Some(t);
                    best_dist = dist;
                }
            }
        }};
    }

    if max_dist >= 0.0 {
        mark!(from);
        if best_dist == 0.0 {
            return best;
        }
    }
    let mut nbuf = [0; 4];
    while let Some(curr) = stack.pop() {
        // `map.forEachNeighbor(curr, visit)` (`:85-92`)
        let n = map.neighbors4(curr, &mut nbuf);
        for &nb in &nbuf[..n] {
            if visited[nb as usize] != generation && f64::from(map.manhattan_dist(from, nb)) <= max_dist {
                mark!(nb);
            }
        }
        if best_dist <= 1.0 {
            break;
        }
    }
    best
}

/// `closestShore(owner, tile, maxDist = 50)` (`:103-117`).
pub fn closest_shore(rcx: &mut Rcx, owner: Owner, tile: TileRef, max_dist: u32) -> Option<TileRef> {
    let st: &State = rcx.st;
    closest_shore_m(&mut rcx.sc.traversal, &st.map, owner, tile, max_dist)
}

/// [`closest_shore`] over any [`MapRead`].
pub fn closest_shore_m<M: MapRead>(
    scratch: &mut TraversalScratch,
    map: &M,
    owner: Owner,
    tile: TileRef,
    max_dist: u32,
) -> Option<TileRef> {
    let owner_id = owner.0;
    let is_valid_tile = |t: TileRef| {
        if !map.is_shore(t) || !map.is_land(t) {
            return false;
        }
        map.owner_id(t) == owner_id
    };
    bfs_nearest(scratch, map, tile, f64::from(max_dist), is_valid_tile)
}

/// `closestReachableShore(targetOwner, attacker, tile, maxDist = 50)` (`:129-158`).
pub fn closest_reachable_shore(
    rcx: &mut Rcx,
    target_owner: Owner,
    attacker: SmallId,
    tile: TileRef,
    max_dist: u32,
) -> Option<TileRef> {
    let st: &State = rcx.st;
    let target_id = target_owner.0;
    // Water components adjacent to the attacker's own shoreline.
    let reachable = reachable_components(rcx, attacker);
    if reachable.is_empty() {
        return None;
    }
    let is_valid_tile = |rcx: &mut Rcx, t: TileRef| -> bool {
        let map = &st.map;
        if !MapRead::is_shore(map, t) || !MapRead::is_land(map, t) {
            return false;
        }
        if MapRead::owner_id(map, t) != target_id {
            return false;
        }
        match rcx.get_water_component(t) {
            Some(c) => reachable.contains(&c),
            None => false,
        }
    };
    // TS: :153-155
    if f64::from(max_dist) >= 0.0 && MapRead::is_valid_ref(&st.map, f64::from(tile)) && is_valid_tile(rcx, tile) {
        return Some(tile);
    }
    // The predicate reads water components through `rcx`, so the traversal scratch is lent
    // out for the flood (scratch: an unwind only costs a re-allocation).
    let mut trav = std::mem::take(&mut rcx.sc.traversal);
    let r = bfs_nearest(&mut trav, &st.map, tile, f64::from(max_dist), |t| is_valid_tile(rcx, t));
    rcx.sc.traversal = trav;
    r
}

/// The reachable water components of `player` (`reachableComponents`, `:160-184`), in
/// first-seen order (`Set` insertion order; callers only use `size` and `has`).
pub fn reachable_components(rcx: &mut Rcx, player: SmallId) -> Vec<u32> {
    let st: &State = rcx.st;
    let p = st.player(player);
    let tile_version = p.tile_change_version();
    let water_version = st.map.water_version;
    if let Some(m) = &st.players[player.player_index()].reachable_components_memo {
        if m.tile_version == tile_version && m.water_version == water_version {
            return m.components.iter().copied().collect();
        }
    }
    let mut components: Vec<u32> = Vec::new();
    // `attacker.borderTiles().forEach(..)`: TileSet order.
    for t in p.border_tiles().iter() {
        if !MapRead::is_shore(&st.map, t) || !MapRead::is_land(&st.map, t) {
            continue;
        }
        if let Some(c) = rcx.get_water_component(t) {
            if !components.contains(&c) {
                components.push(c);
            }
        }
    }
    components
}

/// The `shores` of `closestShoreByWater` (`:197-215`): `None` when the query returns null
/// before the water search.
fn shores_for(rcx: &mut Rcx, owner: Owner, target: TileRef) -> Option<(Vec<TileRef>, u32)> {
    if !owner.is_player() {
        return None;
    }
    let st: &State = rcx.st;
    // Target must be water or shore (land adjacent to water)
    if !MapRead::is_water(&st.map, target) && !MapRead::is_shore(&st.map, target) {
        return None;
    }
    let target_component = rcx.get_water_component(target)?;
    let mut shores: Vec<TileRef> = Vec::new();
    for t in st.player(owner).border_tiles().iter() {
        // `isValidTile(t)` (`:203-207`)
        if !MapRead::is_shore(&st.map, t) || !MapRead::is_land(&st.map, t) {
            continue;
        }
        if rcx.get_water_component(t) == Some(target_component) {
            shores.push(t);
        }
    }
    if shores.is_empty() {
        return None;
    }
    Some((shores, target_component))
}

/// `closestShoreByWater(owner, target)` (`:190-224`), exact: the water search runs through
/// a `PathFinding.Water(gm)` stepper on the game (slot refresh and segment fills kept).
pub fn closest_shore_by_water_cx(cx: &mut Ctx, owner: Owner, target: TileRef) -> Option<TileRef> {
    let (shores, _) = shores_for(&mut cx.rcx(), owner, target)?;
    // `PathFinding.Water(gm).findPath(shores, target)`: an array, even with one shore.
    let mut stepper = WaterStepper::new(cx);
    let path = stepper.find_path_src(cx, Src::Many(&shores), target)?;
    if path.is_empty() {
        return None;
    }
    let st: &State = cx.st;
    Some(refine_start_tile(&mut cx.sc.spatial_bounded, &st.map, &path, &shores))
}

/// Exact memo of [`closest_shore_by_water`] results (P3 perf, ledger t066), kept in `State`
/// (non-snapshot, cloned with forks, never exported).
///
/// Why it is exact: with `shores` computed (by `shores_for`, outside the memo), the rest of the
/// read-only query is a pure function of `(shores, target)` and the water terrain: the chain
/// kind and HPA graph (`water_graph_version`), the main and mini terrain (`terrain_gen`, bumped
/// by every terrain write) and the HPA segment values, which are a pure function of graph +
/// build-time terrain while `mini.terrain_gen == graph.built_at_mini_gen` (`water::
/// abstract_graph` docs). The memo is used only in that state, keyed on all of the above plus
/// the exact `shores` list; any other state bypasses it. Its scratch-only inputs
/// (`spatial_bounded`, a copy of `seg_vis`) never reach a result.
///
/// Bounded FIFO (entries and stored shore tiles), so fork cost stays small.
#[derive(crate::StateClone, Debug, Default)]
pub struct ShoreMemo {
    /// `map.terrain_gen`, `mini.terrain_gen`, `water_graph_version` of every entry.
    pub map_gen: u32,
    pub mini_gen: u32,
    pub graph_version: u32,
    pub entries: Vec<ShoreMemoEntry>,
    /// Next entry to overwrite once full.
    pub next: u32,
    /// Shore tiles stored over all entries.
    pub tiles: u32,
}

/// One memoised `closestShoreByWater(owner, target)` with its exact `shores` input.
#[derive(crate::StateClone, Debug, Default)]
pub struct ShoreMemoEntry {
    pub target: TileRef,
    pub shores: Vec<TileRef>,
    pub result: Option<TileRef>,
}

const SHORE_MEMO_ENTRIES: usize = 512;
const SHORE_MEMO_TILES: u32 = 1 << 17;

type ShoreKey = [u32; 3];

impl ShoreMemo {
    /// The memo key for `st`, or `None` when HPA segment values may be history-dependent.
    fn state_key(st: &State) -> Option<ShoreKey> {
        if let Some(g) = &st.water.graph
            && g.built_at_mini_gen != st.mini.terrain_gen
        {
            return None;
        }
        Some([st.map.terrain_gen, st.mini.terrain_gen, st.water.water_graph_version])
    }
    fn get(&self, key: ShoreKey, target: TileRef, shores: &[TileRef]) -> Option<Option<TileRef>> {
        if [self.map_gen, self.mini_gen, self.graph_version] != key {
            return None;
        }
        self.entries.iter().find(|e| e.target == target && e.shores == shores).map(|e| e.result)
    }
    fn put(&mut self, key: ShoreKey, target: TileRef, shores: &[TileRef], result: Option<TileRef>) {
        let n = shores.len() as u32;
        if [self.map_gen, self.mini_gen, self.graph_version] != key || self.tiles + n > SHORE_MEMO_TILES {
            self.entries.clear();
            [self.map_gen, self.mini_gen, self.graph_version] = key;
            self.next = 0;
            self.tiles = 0;
        }
        if n > SHORE_MEMO_TILES / 4 {
            return;
        }
        let e = ShoreMemoEntry { target, shores: shores.to_vec(), result };
        self.tiles += n;
        if self.entries.len() < SHORE_MEMO_ENTRIES {
            self.entries.push(e);
        } else {
            let i = self.next as usize; // in-range: next < SHORE_MEMO_ENTRIES
            self.tiles -= self.entries[i].shores.len() as u32;
            self.entries[i] = e;
            self.next = (self.next + 1) % SHORE_MEMO_ENTRIES as u32;
        }
    }
}

/// [`closest_shore_by_water`] through the exact [`ShoreMemo`] in `cx.st` (same result).
pub fn closest_shore_by_water_memo(cx: &mut Ctx, owner: Owner, target: TileRef) -> Option<TileRef> {
    let (shores, _) = shores_for(&mut cx.rcx(), owner, target)?;
    let key = ShoreMemo::state_key(cx.st);
    if let Some(k) = key
        && let Some(r) = cx.st.shore_memo.get(k, target, &shores)
    {
        #[cfg(feature = "verify-memos")]
        assert_eq!(r, closest_shore_by_water_from(&mut cx.rcx(), &shores, target), "verify-memos: shore memo stale");
        return r;
    }
    let r = closest_shore_by_water_from(&mut cx.rcx(), &shores, target);
    if let Some(k) = key {
        cx.st.shore_memo.put(k, target, &shores, r);
    }
    r
}

/// `closestShoreByWater(owner, target)` on read-only state (frozen S0 signature): see the
/// module doc for what it cannot keep. Prefer [`closest_shore_by_water_cx`].
pub fn closest_shore_by_water(rcx: &mut Rcx, owner: Owner, target: TileRef) -> Option<TileRef> {
    let (shores, _) = shores_for(rcx, owner, target)?;
    closest_shore_by_water_from(rcx, &shores, target)
}

/// The part of [`closest_shore_by_water`] after `shores_for` (`:217-223`).
fn closest_shore_by_water_from(rcx: &mut Rcx, shores: &[TileRef], target: TileRef) -> Option<TileRef> {
    let st: &State = rcx.st;
    // `sharedWaterChain(gm)`: the slot a refresh would build has this kind (the kind only
    // changes with the graph version, `path::pathfinder::chain_kind_for`).
    let kind = chain_kind_for(st);
    let mut seg = st.water.seg_vis.clone();
    let mut stepper = StepperState::default();
    let path =
        stepper.find_path(&mut RcxWaterFinder { rcx: &mut *rcx, seg: &mut seg, kind }, Src::Many(shores), target)?;
    if path.is_empty() {
        return None;
    }
    Some(refine_start_tile(&mut rcx.sc.spatial_bounded, &st.map, &path, shores))
}

/// A `PathFinding.Water` stepper over read-only state (see [`closest_shore_by_water`]).
struct RcxWaterFinder<'r, 'a, 's> {
    rcx: &'r mut Rcx<'a>,
    seg: &'s mut SegVisibility,
    kind: ChainKind,
}

impl StepFinder for RcxWaterFinder<'_, '_, '_> {
    fn pre_check(&mut self, from: u32, to: u32) -> Option<PathResult<u32>> {
        let map = &self.rcx.st.map;
        if !MapRead::is_valid_ref(map, f64::from(from)) || !MapRead::is_valid_ref(map, f64::from(to)) {
            Some(PathResult::NotFound)
        } else {
            None
        }
    }
    fn has_pre_check(&self) -> bool {
        true
    }
    fn distance(&mut self, a: u32, b: u32) -> Option<f64> {
        Some(f64::from(MapRead::manhattan_dist(&self.rcx.st.map, a, b)))
    }
    fn find_path(&mut self, from: Src<'_>, to: u32) -> Option<Vec<u32>> {
        crate::path::builder::water_chain_find_path_src(self.rcx, self.seg, self.kind, from, to)
    }
    fn after_caught_throw(&mut self) {
        self.rcx.sc.after_caught_throw();
    }
}

/// `refineStartTile(path, shores, gm)` (`:226-316`) on the main map, with
/// `getBoundedAStar()` = `sc.spatial_bounded` (capacity `REFINE_MAX_SEARCH_AREA`).
pub fn refine_start_tile<M: MapRead>(
    bounded: &mut crate::scratch::path::AStarWaterBoundedScratch,
    map: &M,
    path: &[TileRef],
    shores: &[TileRef],
) -> TileRef {
    const CANDIDATE_RADIUS: u32 = 20;
    const MIN_WAYPOINT_DIST: usize = 50;
    const MAX_WAYPOINT_DIST: usize = 200;
    const PADDING: i64 = 10;
    let area_cap = i64::from(REFINE_MAX_SEARCH_AREA);

    if path.len() <= MIN_WAYPOINT_DIST {
        return path[0];
    }
    let best_tile = path[0];
    let candidates: Vec<TileRef> =
        shores.iter().copied().filter(|&s| map.manhattan_dist(s, best_tile) <= CANDIDATE_RADIUS).collect();
    if candidates.len() <= 1 {
        return best_tile;
    }

    // Precompute candidate bounds
    let mut cand_min_x = i64::from(map.x(candidates[0]));
    let mut cand_max_x = cand_min_x;
    let mut cand_min_y = i64::from(map.y(candidates[0]));
    let mut cand_max_y = cand_min_y;
    for &c in &candidates[1..] {
        let sx = i64::from(map.x(c));
        let sy = i64::from(map.y(c));
        cand_min_x = cand_min_x.min(sx);
        cand_max_x = cand_max_x.max(sx);
        cand_min_y = cand_min_y.min(sy);
        cand_max_y = cand_max_y.max(sy);
    }

    // Binary search for furthest waypoint that keeps bounds within limit (`:264-287`)
    let mut lo = MIN_WAYPOINT_DIST as i64;
    let mut hi = MAX_WAYPOINT_DIST.min(path.len() - 1) as i64;
    let mut best_waypoint_idx = lo;
    let mut i = 0;
    while i < 5 && lo <= hi {
        let mid = (lo + hi) >> 1;
        let wp = path[mid as usize];
        let wp_x = i64::from(map.x(wp));
        let wp_y = i64::from(map.y(wp));
        let min_x = cand_min_x.min(wp_x) - PADDING;
        let max_x = cand_max_x.max(wp_x) + PADDING;
        let min_y = cand_min_y.min(wp_y) - PADDING;
        let max_y = cand_max_y.max(wp_y) + PADDING;
        let area = (max_x - min_x + 1) * (max_y - min_y + 1);
        if area <= area_cap {
            best_waypoint_idx = mid;
            lo = mid + 1;
        } else {
            hi = mid - 1;
        }
        i += 1;
    }

    let waypoint = path[best_waypoint_idx as usize];
    let wp_x = i64::from(map.x(waypoint));
    let wp_y = i64::from(map.y(waypoint));
    let w = i64::from(map.width());
    let h = i64::from(map.height());
    let min_x = 0.max(cand_min_x.min(wp_x) - PADDING);
    let max_x = (w - 1).min(cand_max_x.max(wp_x) + PADDING);
    let min_y = 0.max(cand_min_y.min(wp_y) - PADDING);
    let max_y = (h - 1).min(cand_max_y.max(wp_y) + PADDING);
    let bounds_area = (max_x - min_x + 1) * (max_y - min_y + 1);
    if bounds_area > area_cap {
        return best_tile;
    }
    let bounds = SearchBounds { min_x: min_x as u32, max_x: max_x as u32, min_y: min_y as u32, max_y: max_y as u32 };
    let refined = astar_water_bounded::search_bounded_m(
        bounded,
        map,
        REFINE_MAX_SEARCH_AREA,
        BoundedAStarConfig::default(),
        &candidates,
        waypoint,
        bounds,
    );
    // `refinedPath?.[0] ?? bestTile`
    refined.and_then(|p| p.first().copied()).unwrap_or(best_tile)
}

/// `GameMap` instantiation of [`refine_start_tile`] (used by P7's TransportShipUtils).
pub fn refine_start_tile_on(
    bounded: &mut crate::scratch::path::AStarWaterBoundedScratch,
    map: &GameMap,
    path: &[TileRef],
    shores: &[TileRef],
) -> TileRef {
    refine_start_tile(bounded, map, path, shores)
}
