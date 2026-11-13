//! `AStarWaterHierarchical` (`pathfinding/algorithms/AStar.WaterHierarchical.ts`). Owner: **P4a**.
//!
//! There is one HPA per game and it always routes on the **current** graph
//! (`state.water.graph`) and the live components (§3.8). Its internals (abstract A*, the
//! three bounded local A*s, the source resolver, the tile BFS) are scratch: `sc.hpa`.
//!
//! - `options.cachePaths` is always `true` (the only constructor call sites,
//!   `WaterManager.ts:87-91,203-207`), so the segment cache is always consulted.
//! - TS `findPath(from: number | number[], to)`: the only caller is the chain's
//!   `ComponentCheckTransformer`, which passes a number for one valid source and an array
//!   for two or more (`ComponentCheckTransformer.ts:31-33`). So `from.len() == 1` is the
//!   TS scalar (single-source) and `from.len() >= 2` the array (multi-source).
//! - The three `AStarWaterBounded` instances belong to P4b (`path::astar_water_bounded`).
//!   TS sizes them `maxSearchArea` = 32², 96², 260² (`:33-47`) and `searchBounded` returns
//!   `null` when the bounds exceed it (`AStar.WaterBounded.ts:117-119`, before any other
//!   effect except an unobservable stamp bump). The frozen P4b `search_bounded` signature
//!   has no `max_search_area`, so the HPA applies that check itself before calling it
//!   ([`LocalKind::max_search_area`]); see docs/requests/P4a.md.

use crate::ctx::{Rcx, TsExpect};
use crate::ids::TileRef;
use crate::path::astar_water_bounded::{BoundedAStarConfig, SearchBounds};
use crate::scratch::path::AStarWaterBoundedScratch;
use crate::scratch::water::HpaScratch;
use crate::water::abstract_graph::{AbstractNode, SegVisibility, WaterGraph};
use crate::water::astar_abstract::{self, AbstractGraphAStarConfig};
use crate::water::bfs_grid::{self, Visit};
use crate::water::map_view::WaterMap;

/// Which of the HPA's three `AStarWaterBounded` instances (`:12-14`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LocalKind {
    /// `localAStar`: one cluster.
    Local,
    /// `localAStarMultiCluster`: 3 × 3 clusters.
    Multi,
    /// `localAStarShortPath`: the multi-source short-path box.
    Short,
}

impl LocalKind {
    /// The instance's `maxSearchArea` (`:33-47`).
    pub(crate) fn max_search_area(self, cluster_size: u32) -> u64 {
        let cs = u64::from(cluster_size);
        match self {
            LocalKind::Local => cs * cs,
            LocalKind::Multi => (cs * 3) * (cs * 3),
            LocalKind::Short => 260 * 260, // `shortPathSize = 260` (`:45`)
        }
    }
}

/// The bounded local search the HPA delegates to (`selectedAStar.searchBounded`). Production
/// passes P4b's `search_bounded`; the unit tests pass a literal port of it.
pub(crate) type LocalSearch<'l> =
    dyn FnMut(LocalKind, &mut AStarWaterBoundedScratch, &[TileRef], TileRef, SearchBounds) -> Option<Vec<TileRef>> + 'l;

/// One HPA query's inputs: the HPA's `map` (the minimap), the current graph, the per-game
/// segment visibility and the scratch.
pub(crate) struct Hpa<'a, 'l, M: WaterMap + ?Sized> {
    pub map: &'a M,
    pub graph: &'a WaterGraph,
    /// `mini.terrain_gen` now (decides shared vs overlay segment fills).
    pub mini_terrain_gen: u32,
    pub seg_vis: &'a mut SegVisibility,
    pub sc: &'a mut HpaScratch,
    pub local: &'a mut LocalSearch<'l>,
}

/// `findPath(from, to)` (`:66-77`) on the current graph; `from` may be several sources.
/// Segment fills go to `seg_vis` (per game), hence `&mut`.
pub fn find_path(rcx: &mut Rcx, seg_vis: &mut SegVisibility, from: &[TileRef], to: TileRef) -> Option<Vec<TileRef>> {
    with_hpa(rcx, seg_vis, |h| h.find_path(from, to))
}

/// `findPathSingle(from, to)` (`:145-301`).
pub fn find_path_single(
    rcx: &mut Rcx,
    seg_vis: &mut SegVisibility,
    from: TileRef,
    to: TileRef,
) -> Option<Vec<TileRef>> {
    with_hpa(rcx, seg_vis, |h| h.find_path_single(from, to))
}

/// Builds the per-query [`Hpa`] from the game state: the minimap, `state.water.graph`, P4b's
/// bounded A* with the TS default config (`heuristicWeight 3`, `maxIterations 100_000`,
/// `AStar.WaterBounded.ts:49-50`; the HPA passes no config).
fn with_hpa<R>(
    rcx: &mut Rcx,
    seg_vis: &mut SegVisibility,
    f: impl FnOnce(&mut Hpa<'_, '_, crate::map::GameMap>) -> R,
) -> R {
    let st = rcx.st;
    let Some(graph) = st.water.graph.as_deref() else {
        // A TS chain is HPA only when `miniWaterHPA()` exists (`PathFinder.ts:52`), i.e. the
        // nav mesh is enabled, and then a graph always exists.
        panic!("of-engine: HPA query without a water graph (Rust bug: chain kind Hpa needs a graph)");
    };
    let mini = &st.mini;
    let cluster_size = graph.cluster_size;
    // Each of the HPA's three `AStarWaterBounded` instances has its own `maxSearchArea`
    // (`AStar.WaterHierarchical.ts:33-47`); P4b's `search_bounded_cap` sizes the scratch to it
    // (requests P4a → P4b #1 / P4b R2; merge1: the frozen `search_bounded` panicked on an
    // unsized scratch, first seen at RS-SMOKE-01 tick 335).
    let mut local =
        |k: LocalKind, s: &mut AStarWaterBoundedScratch, start: &[TileRef], goal: TileRef, b: SearchBounds| {
            crate::path::astar_water_bounded::search_bounded_cap(
                s,
                mini,
                u32::try_from(k.max_search_area(cluster_size)).unwrap_or(u32::MAX), // <= 260²
                BoundedAStarConfig::default(),
                start,
                goal,
                b,
            )
        };
    let mut h =
        Hpa { map: mini, graph, mini_terrain_gen: mini.terrain_gen, seg_vis, sc: &mut rcx.sc.hpa, local: &mut local };
    f(&mut h)
}

impl<M: WaterMap + ?Sized> Hpa<'_, '_, M> {
    #[inline]
    fn cluster_size(&self) -> u32 {
        self.graph.cluster_size
    }

    /// `selectedAStar.searchBounded(start, goal, bounds)` with the `maxSearchArea` gate.
    fn search_bounded(
        &mut self,
        kind: LocalKind,
        start: &[TileRef],
        goal: TileRef,
        b: SearchBounds,
    ) -> Option<Vec<TileRef>> {
        // TS: AStar.WaterBounded.ts:111-119
        let bw = i64::from(b.max_x) - i64::from(b.min_x) + 1;
        let bh = i64::from(b.max_y) - i64::from(b.min_y) + 1;
        let num_local_nodes = bw * bh;
        if num_local_nodes > kind.max_search_area(self.cluster_size()) as i64 {
            return None;
        }
        let s = match kind {
            LocalKind::Local => &mut self.sc.local,
            LocalKind::Multi => &mut self.sc.local_multi,
            LocalKind::Short => &mut self.sc.local_short,
        };
        (self.local)(kind, s, start, goal, b)
    }

    /// `findPath(from, to)` (`:66-77`).
    pub(crate) fn find_path(&mut self, from: &[TileRef], to: TileRef) -> Option<Vec<TileRef>> {
        match from {
            [f] => self.find_path_single(*f, to),
            _ => self.find_path_multi_source(from, to),
        }
    }

    /// `findPathMultiSource(sources, target)` (`:79-105`).
    fn find_path_multi_source(&mut self, sources: &[TileRef], target: TileRef) -> Option<Vec<TileRef>> {
        // TS: :84-85 (an empty array is truthy, but searchBounded never returns one)
        if let Some(p) = self.try_short_path_multi_source(sources, target) {
            return Some(p);
        }
        let target_node = self.get_cluster_node(target)?;
        let node_to_source = self.resolve_sources_to_nodes(sources);
        if node_to_source.is_empty() {
            return None;
        }
        // TS: :96 `[...nodeToSource.keys()]` (insertion order)
        let node_ids: Vec<u32> = node_to_source.iter().map(|e| e.0).collect();
        let node_path = astar_abstract::find_path(
            &mut self.sc.abstract_astar,
            self.graph,
            AbstractGraphAStarConfig::default(),
            &node_ids,
            target_node.id,
        )?;
        // TS: :101 `nodeToSource.get(nodePath[0])!`
        let first = node_path.first().copied().ts_expect("AStar.WaterHierarchical.ts:101 nodePath[0]");
        let winning_source =
            node_to_source.iter().find(|e| e.0 == first).map(|e| e.1).ts_expect("AStar.WaterHierarchical.ts:101");
        self.find_path_single(winning_source, target)
    }

    /// `tryShortPathMultiSource(sources, target)` (`:107-143`).
    fn try_short_path_multi_source(&mut self, sources: &[TileRef], target: TileRef) -> Option<Vec<TileRef>> {
        const SHORT_PATH_THRESHOLD: u32 = 120;
        const PADDING: i64 = 10;
        let map = self.map;
        let candidates: Vec<TileRef> =
            sources.iter().copied().filter(|&s| map.manhattan_dist(s, target) <= SHORT_PATH_THRESHOLD).collect();
        if candidates.is_empty() {
            return None;
        }
        let to_x = i64::from(map.x(target));
        let to_y = i64::from(map.y(target));
        let (mut min_x, mut max_x, mut min_y, mut max_y) = (to_x, to_x, to_y, to_y);
        for &s in &candidates {
            let sx = i64::from(map.x(s));
            let sy = i64::from(map.y(s));
            min_x = min_x.min(sx);
            max_x = max_x.max(sx);
            min_y = min_y.min(sy);
            max_y = max_y.max(sy);
        }
        let bounds = SearchBounds {
            min_x: (min_x - PADDING).max(0) as u32,
            max_x: (i64::from(map.width()) - 1).min(max_x + PADDING) as u32,
            min_y: (min_y - PADDING).max(0) as u32,
            max_y: (i64::from(map.height()) - 1).min(max_y + PADDING) as u32,
        };
        self.search_bounded(LocalKind::Short, &candidates, target, bounds)
    }

    /// `findPathSingle(from, to)` (`:145-301`).
    pub(crate) fn find_path_single(&mut self, from: TileRef, to: TileRef) -> Option<Vec<TileRef>> {
        let map = self.map;
        let cs = self.cluster_size();
        let dist = map.manhattan_dist(from, to);
        // TS: :149-161 early exit for very short distances
        if dist <= cs {
            let cluster_x = map.x(from) / cs;
            let cluster_y = map.y(from) / cs;
            if let Some(p) = self.find_local_path(from, to, cluster_x, cluster_y, true) {
                return Some(p);
            }
        }
        let start_node = self.find_nearest_node(from);
        let end_node = self.find_nearest_node(to);
        let start_node = start_node?;
        let end_node = end_node?;
        if start_node.id == end_node.id {
            // TS: :176-183
            let cluster_x = start_node.x / cs;
            let cluster_y = start_node.y / cs;
            return self.find_local_path(from, to, cluster_x, cluster_y, true);
        }
        let node_path = astar_abstract::find_path(
            &mut self.sc.abstract_astar,
            self.graph,
            AbstractGraphAStarConfig::default(),
            &[start_node.id],
            end_node.id,
        )?;
        let mut initial_path: Vec<TileRef> = Vec::new();
        // TS: :206-225 start → first node
        let first_node_tile = self.node(node_path[0]).tile; // in-range: a found path is non-empty
        let start_cluster_x = map.x(from) / cs;
        let start_cluster_y = map.y(from) / cs;
        let start_segment = self.find_local_path(from, first_node_tile, start_cluster_x, start_cluster_y, false)?;
        initial_path.extend_from_slice(&start_segment);
        // TS: :228-274 through the abstract nodes
        for pair in node_path.windows(2) {
            let (from_node_id, to_node_id) = (pair[0], pair[1]);
            let edge = *self.graph.get_edge_between(from_node_id, to_node_id)?;
            let from_tile = self.node(from_node_id).tile;
            let to_tile = self.node(to_node_id).tile;
            // TS: :244-255 `cachePaths` is always true; a hit needs a non-empty path
            if let Some(cached) = self.seg_vis.get_cached_path(self.graph, edge.id, from_node_id)
                && !cached.is_empty()
            {
                initial_path.extend_from_slice(&cached[1..]);
                continue;
            }
            let segment_path = self.find_local_path(from_tile, to_tile, edge.cluster_x, edge.cluster_y, false)?;
            initial_path.extend_from_slice(&segment_path[1..]);
            self.seg_vis.set_cached_path(self.graph, self.mini_terrain_gen, edge.id, from_node_id, &segment_path);
        }
        // TS: :277-295 last node → end
        let last_node_tile = self.node(node_path[node_path.len() - 1]).tile;
        let end_cluster_x = map.x(to) / cs;
        let end_cluster_y = map.y(to) / cs;
        let end_segment = self.find_local_path(last_node_tile, to, end_cluster_x, end_cluster_y, false)?;
        initial_path.extend_from_slice(&end_segment[1..]);
        Some(initial_path)
    }

    /// `this.graph.getNode(id)!` for ids from the abstract A* (always valid).
    fn node(&self, id: u32) -> AbstractNode {
        *self.graph.get_node(id).ts_expect("AStar.WaterHierarchical.ts:207 getNode(..)!")
    }

    /// `findNearestNode(tile)` (`:303-345`): BFS over water from `tile` to the first tile that
    /// is one of its cluster's nodes, rejecting tiles outside the cluster.
    fn find_nearest_node(&mut self, tile: TileRef) -> Option<AbstractNode> {
        let map = self.map;
        let cs = self.cluster_size();
        let x = map.x(tile);
        let y = map.y(tile);
        let cluster_x = x / cs;
        let cluster_y = y / cs;
        let min_x = cluster_x * cs;
        let min_y = cluster_y * cs;
        let max_x = (map.width() - 1).min(min_x + cs - 1);
        let max_y = (map.height() - 1).min(min_y + cs - 1);
        let graph = self.graph;
        let cluster = graph.get_cluster(cluster_x, cluster_y)?;
        if cluster.node_ids.is_empty() {
            return None;
        }
        let candidate_nodes: Vec<AbstractNode> = cluster
            .node_ids
            .iter()
            .map(|&id| *graph.get_node(id).ts_expect("AStar.WaterHierarchical.ts:321"))
            .collect();
        let max_distance = cs * cs;
        bfs_grid::search(
            &mut self.sc.tile_bfs,
            map.width(),
            map.height(),
            &[tile],
            max_distance,
            |t| map.is_water(t),
            |t, _dist| {
                let tile_x = map.x(t);
                let tile_y = map.y(t);
                for node in &candidate_nodes {
                    if node.x == tile_x && node.y == tile_y {
                        return Visit::Found(*node);
                    }
                }
                if tile_x < min_x || tile_x > max_x || tile_y < min_y || tile_y > max_y {
                    return Visit::Reject;
                }
                Visit::Expand
            },
        )
    }

    /// `findLocalPath(from, to, clusterX, clusterY, multiCluster)` (`:354-409`).
    fn find_local_path(
        &mut self,
        from: TileRef,
        to: TileRef,
        cluster_x: u32,
        cluster_y: u32,
        multi_cluster: bool,
    ) -> Option<Vec<TileRef>> {
        let cs = i64::from(self.cluster_size());
        let w = i64::from(self.map.width());
        let h = i64::from(self.map.height());
        let (cx, cy) = (i64::from(cluster_x), i64::from(cluster_y));
        let (min_x, min_y, max_x, max_y) = if multi_cluster {
            (
                ((cx - 1) * cs).max(0),
                ((cy - 1) * cs).max(0),
                (w - 1).min((cx + 2) * cs - 1),
                (h - 1).min((cy + 2) * cs - 1),
            )
        } else {
            let mx = cx * cs;
            let my = cy * cs;
            (mx, my, (w - 1).min(mx + cs - 1), (h - 1).min(my + cs - 1))
        };
        let kind = if multi_cluster { LocalKind::Multi } else { LocalKind::Local };
        let bounds =
            SearchBounds { min_x: min_x as u32, max_x: max_x as u32, min_y: min_y as u32, max_y: max_y as u32 };
        let mut path = self.search_bounded(kind, &[from], to, bounds)?;
        if path.is_empty() {
            return None;
        }
        // TS: :401-406 fix endpoints (BoundedAStar clamps tiles to the bounds)
        if path[0] != from {
            path.insert(0, from);
        }
        if path[path.len() - 1] != to {
            path.push(to);
        }
        Some(path)
    }

    /// `SourceResolver.resolveSourcesToNodes(sources)` (`:426-447`): `Map<nodeId, source>` in
    /// insertion order, keeping the closest source per node (`dist < prevDist`, first wins
    /// ties). Entries: `(node_id, source, dist)`.
    fn resolve_sources_to_nodes(&self, sources: &[TileRef]) -> Vec<(u32, TileRef, u32)> {
        let mut out: Vec<(u32, TileRef, u32)> = Vec::new();
        for &source in sources {
            let Some(node) = self.get_cluster_node(source) else {
                continue;
            };
            let x = self.map.x(source);
            let y = self.map.y(source);
            let dist = node.x.abs_diff(x) + node.y.abs_diff(y);
            match out.iter_mut().find(|e| e.0 == node.id) {
                None => out.push((node.id, source, dist)),
                Some(e) => {
                    if dist < e.2 {
                        e.1 = source; // `Map.set` on an existing key keeps its position
                        e.2 = dist;
                    }
                }
            }
        }
        out
    }

    /// `SourceResolver.getClusterNode(tile)` (`:449-474`) = `resolveTarget`: the closest node
    /// of the tile's cluster by Manhattan distance (first wins ties).
    fn get_cluster_node(&self, tile: TileRef) -> Option<AbstractNode> {
        let cs = self.cluster_size();
        let x = self.map.x(tile);
        let y = self.map.y(tile);
        let cluster = self.graph.get_cluster(x / cs, y / cs)?;
        if cluster.node_ids.is_empty() {
            return None;
        }
        let mut best: Option<AbstractNode> = None;
        let mut best_dist = f64::INFINITY;
        for &node_id in &cluster.node_ids {
            let Some(node) = self.graph.get_node(node_id) else {
                continue;
            };
            let dist = f64::from(node.x.abs_diff(x) + node.y.abs_diff(y));
            if dist < best_dist {
                best_dist = dist;
                best = Some(*node);
            }
        }
        best
    }
}
