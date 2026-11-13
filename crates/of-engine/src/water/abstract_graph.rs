//! `AbstractGraph` + `AbstractGraphBuilder` (`pathfinding/algorithms/AbstractGraph.ts`). Owner: **P4a**.
//!
//! One immutable [`WaterGraph`] per water-graph version, shared by `Arc` across forks. The
//! per-edge segment cache (`_pathCache`, `:105-136`) is split (ARCHITECTURE §3.8):
//! - shared values `seg_shared` (2 × edges `OnceLock`s), filled only while
//!   `mini.terrain_gen == built_at_mini_gen` (a pure function of graph + build-time terrain);
//! - per-game [`SegVisibility`]: the `filled` bitset (a hit only if this game's bit is set)
//!   and the post-edit `overlay`. A rebuild resets it (TS `_initPathCache`).
//!
//! The graph's `_waterComponents` (`:44,138-149`) is **not** captured: every graph in TS
//! points at the one live `ConnectedComponents`, so `getComponentId`/`getComponentSize` are
//! answered from `state.water.components` (`WaterManager::graph_component_id`).

use crate::StateClone;
use crate::ids::TileRef;
use crate::jsorder::{JsSet, LookupMap};
use crate::map::GameMap;
use crate::scratch::water::BfsGridScratch;
use crate::water::bfs_grid::{self, Visit};
use crate::water::components::Components;
use crate::water::map_view::WaterMap;
use std::sync::{Arc, OnceLock};

/// `AbstractNode` (`:6-12`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AbstractNode {
    pub id: u32,
    pub x: u32,
    pub y: u32,
    pub tile: TileRef,
    pub component_id: u32,
}

/// `AbstractEdge` (`:14-21`): `node_a < node_b`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AbstractEdge {
    pub id: u32,
    pub node_a: u32,
    pub node_b: u32,
    pub cost: f64,
    pub cluster_x: u32,
    pub cluster_y: u32,
}

/// `Cluster` (`:23-27`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphCluster {
    pub x: u32,
    pub y: u32,
    pub node_ids: Vec<u32>,
}

/// One built `AbstractGraph` (immutable after build).
#[derive(Debug, Default)]
pub struct WaterGraph {
    pub cluster_size: u32,
    pub clusters_x: u32,
    pub clusters_y: u32,
    pub nodes: Vec<AbstractNode>,
    pub edges: Vec<AbstractEdge>,
    /// Per node: incident edge ids in TS order (`getNodeEdges`).
    pub node_edges: Vec<Vec<u32>>,
    /// `getEdgeBetween` index (lookup-only): `(lo << 32 | hi)` → edge id, filled after
    /// `_sortEdges`. Used only for `a != b`, where the TS scan (`:79-90`) can match only the
    /// unique `{a, b}` edge.
    pub edge_between: LookupMap<u64, u32>,
    /// `_clusters` by cluster key.
    pub clusters: Vec<Option<GraphCluster>>,
    /// `mini.terrain_gen` when built (gates `seg_shared` writes).
    pub built_at_mini_gen: u32,
    /// Shared segment values, `2 * edge_id + dir`.
    pub seg_shared: Box<[OnceLock<Arc<[u32]>>]>,
}

impl crate::plain::PlainState for WaterGraph {}
crate::refs_leaf!(WaterGraph);

/// `AbstractGraphBuilder.CLUSTER_SIZE`.
pub const CLUSTER_SIZE: u32 = 32;

#[inline]
fn pair_key(lo: u32, hi: u32) -> u64 {
    (u64::from(lo) << 32) | u64::from(hi)
}

impl WaterGraph {
    /// `new AbstractGraphBuilder(miniMap, CLUSTER_SIZE, undefined, undefined, components,
    /// builderBFS).build()` (`:244-328`): a full build on shared, already-initialised
    /// components (the `WaterManager` constructor path, `WaterManager.ts:78-86`).
    pub fn build(mini: &GameMap, components: &Components, bfs: &mut BfsGridScratch) -> WaterGraph {
        Self::build_with(mini, CLUSTER_SIZE, None, None, Some(components), bfs)
    }

    /// The general builder: `oldGraph` + `dirtyMiniTiles` select the partial rebuild
    /// (`:280-282`; the caller passes `dirty` only when non-empty, `WaterManager.ts:192`);
    /// `shared_components = None` builds and initialises private components (`:261-277`).
    pub(crate) fn build_with<M: WaterMap + ?Sized>(
        map: &M,
        cluster_size: u32,
        old_graph: Option<&WaterGraph>,
        dirty_mini_tiles: Option<&JsSet<TileRef>>,
        shared_components: Option<&Components>,
        bfs: &mut BfsGridScratch,
    ) -> WaterGraph {
        let own;
        let components = match shared_components {
            Some(c) => c,
            None => {
                // TS: :262 + :275-277
                let mut c = Components::new_with(map);
                let mut q = Vec::new();
                c.initialize_with(map, true, &mut q);
                own = c;
                &own
            }
        };
        let mut b = Builder::new(map, cluster_size, old_graph, dirty_mini_tiles, components, bfs);
        b.build()
    }

    /// `nodeCount` (chain kind: `< 100` → simple chain, `PathFinder.ts:52-58`).
    pub fn node_count(&self) -> u32 {
        self.nodes.len() as u32
    }
    /// `edgeCount`.
    pub fn edge_count(&self) -> u32 {
        self.edges.len() as u32
    }
    /// `getNode(id)`.
    pub fn get_node(&self, id: u32) -> Option<&AbstractNode> {
        self.nodes.get(id as usize)
    }
    /// `getEdge(id)`.
    pub fn get_edge(&self, id: u32) -> Option<&AbstractEdge> {
        self.edges.get(id as usize)
    }
    /// `getNodeEdges(nodeId)` as edge ids (`:68-77`; every listed id is a live edge).
    pub fn get_node_edges(&self, node_id: u32) -> &[u32] {
        self.node_edges.get(node_id as usize).map_or(&[], |v| v.as_slice())
    }
    /// `getEdgeBetween(nodeA, nodeB)` (`:79-90`).
    pub fn get_edge_between(&self, node_a: u32, node_b: u32) -> Option<&AbstractEdge> {
        let edge_ids = self.node_edges.get(node_a as usize)?;
        if node_a == node_b {
            // TS scan: the first incident edge always matches (`edge.nodeA === nodeA || …`).
            return edge_ids.first().and_then(|&e| self.edges.get(e as usize));
        }
        let (lo, hi) = if node_a < node_b { (node_a, node_b) } else { (node_b, node_a) };
        self.edge_between.get(&pair_key(lo, hi)).and_then(|&e| self.edges.get(e as usize))
    }
    /// `getOtherNode(edge, nodeId)` (`:92-94`).
    pub fn get_other_node(&self, edge: &AbstractEdge, node_id: u32) -> u32 {
        if edge.node_a == node_id { edge.node_b } else { edge.node_a }
    }
    /// `getClusterKey(clusterX, clusterY)` (`:151-153`).
    pub fn get_cluster_key(&self, cluster_x: u32, cluster_y: u32) -> u32 {
        cluster_y * self.clusters_x + cluster_x
    }
    /// `getCluster(clusterX, clusterY)` with JS index semantics: the key is computed from
    /// possibly out-of-range coordinates (`getNearbyClusterNodes` passes `x - 1` at the
    /// left edge, which lands on the previous row's last cluster) and only a negative or
    /// too-large key is `undefined`.
    pub fn get_cluster_i(&self, cluster_x: i64, cluster_y: i64) -> Option<&GraphCluster> {
        let key = cluster_y * i64::from(self.clusters_x) + cluster_x;
        if key < 0 {
            return None;
        }
        self.clusters.get(key as usize).and_then(|c| c.as_ref())
    }
    /// `getCluster(clusterX, clusterY)` (`:155-157`).
    pub fn get_cluster(&self, cluster_x: u32, cluster_y: u32) -> Option<&GraphCluster> {
        self.get_cluster_i(i64::from(cluster_x), i64::from(cluster_y))
    }
    /// `getClusterNodes(..)` as node ids (`:159-163`).
    pub fn get_cluster_nodes(&self, cluster_x: u32, cluster_y: u32) -> &[u32] {
        self.get_cluster(cluster_x, cluster_y).map_or(&[], |c| c.node_ids.as_slice())
    }
    /// `getNearbyClusterNodes(..)` as node ids (`:165-180`), with the JS key wrap at the
    /// left/right map edges (see [`WaterGraph::get_cluster_i`]).
    pub fn get_nearby_cluster_nodes(&self, cluster_x: u32, cluster_y: u32) -> Vec<u32> {
        let mut nodes = Vec::new();
        for dy in -1i64..=1 {
            for dx in -1i64..=1 {
                if let Some(c) = self.get_cluster_i(i64::from(cluster_x) + dx, i64::from(cluster_y) + dy) {
                    nodes.extend_from_slice(&c.node_ids);
                }
            }
        }
        nodes
    }
    /// Number of `_pathCache` slots (`_edges.length * 2`, `:135`).
    pub fn seg_slots(&self) -> usize {
        self.edges.len() * 2
    }
}

/// Per-game visibility over `seg_shared` (state; cloned; reset on rebuild).
#[derive(StateClone, Debug, Default)]
pub struct SegVisibility {
    /// 2 × edges bits.
    pub filled: Vec<u64>,
    /// Fills made after a terrain edit under the same graph (rare).
    pub overlay: LookupMap<u32, Arc<[u32]>>,
}

impl SegVisibility {
    /// Visibility for a freshly built graph (`_initPathCache`).
    pub fn for_graph(graph: &WaterGraph) -> Self {
        let mut s = SegVisibility::default();
        s.reset(graph);
        s
    }

    /// `getCachedPath(edgeId, fromNodeId)` (`:109-122`): hit only when this game's bit is set.
    pub fn get_cached_path<'g>(&'g self, graph: &'g WaterGraph, edge_id: u32, from_node_id: u32) -> Option<&'g [u32]> {
        let edge = graph.edges.get(edge_id as usize)?;
        let direction = if from_node_id == edge.node_a { 0 } else { 1 };
        let idx = (edge_id as usize) * 2 + direction;
        let word = self.filled.get(idx / 64)?;
        if word & (1u64 << (idx % 64)) == 0 {
            return None; // `_pathCache[idx] ?? null` with nothing stored by this game
        }
        if let Some(p) = self.overlay.get(&(idx as u32)) {
            return Some(p);
        }
        graph.seg_shared.get(idx).and_then(|c| c.get()).map(|p| &p[..])
    }

    /// `setCachedPath(edgeId, fromNodeId, path)` (`:124-131`): shared iff the gens match.
    pub fn set_cached_path(
        &mut self,
        graph: &WaterGraph,
        mini_terrain_gen: u32,
        edge_id: u32,
        from_node_id: u32,
        path: &[u32],
    ) {
        let Some(edge) = graph.edges.get(edge_id as usize) else {
            return; // TS: :126
        };
        let direction = if from_node_id == edge.node_a { 0 } else { 1 };
        let idx = (edge_id as usize) * 2 + direction;
        let value: Arc<[u32]> = Arc::from(path);
        if mini_terrain_gen == graph.built_at_mini_gen
            && let Some(cell) = graph.seg_shared.get(idx)
        {
            // Pure function of (graph, build-time terrain): every sharer computes the same
            // value, so a cell another fork filled first holds an equal segment.
            let _ = cell.get_or_init(|| value);
            self.overlay.remove(&(idx as u32));
        } else {
            self.overlay.insert(idx as u32, value);
        }
        let wi = idx / 64;
        if self.filled.len() <= wi {
            self.filled.resize(wi + 1, 0);
        }
        self.filled[wi] |= 1u64 << (idx % 64); // in-range: resized above
    }

    /// `_initPathCache()` (`:133-136`).
    pub fn reset(&mut self, graph: &WaterGraph) {
        self.filled.clear();
        self.filled.resize(graph.seg_slots().div_ceil(64), 0);
        self.overlay.clear();
    }
}

/// `{ cost, clusterX, clusterY }` of `oldEdgeCosts` (`:239-242`).
#[derive(Clone, Copy)]
struct OldEdge {
    cost: f64,
    cluster_x: u32,
    cluster_y: u32,
}

/// `AbstractGraphBuilder` build state (`:219-263`).
struct Builder<'a, M: WaterMap + ?Sized> {
    map: &'a M,
    cluster_size: u32,
    width: u32,
    height: u32,
    clusters_x: u32,
    clusters_y: u32,
    tile_bfs: &'a mut BfsGridScratch,
    water_components: &'a Components,
    old_graph: Option<&'a WaterGraph>,
    dirty_mini_tiles: Option<&'a JsSet<TileRef>>,
    graph: WaterGraph,
    /// `tileToNode` (lookup-only: get/set, `:333-347`).
    tile_to_node: LookupMap<TileRef, u32>,
    next_node_id: u32,
    next_edge_id: u32,
    /// `edgeBetween` (lookup-only: get/set, `:540-568`): (lo, hi) → edge index.
    edge_between: LookupMap<u64, u32>,
    /// `cleanClusters` (`has` only).
    clean_clusters: Option<Vec<bool>>,
    /// `oldEdgeCosts` (lookup-only: get/set, `:683-704`).
    old_edge_costs: Option<LookupMap<u64, OldEdge>>,
}

impl<'a, M: WaterMap + ?Sized> Builder<'a, M> {
    fn new(
        map: &'a M,
        cluster_size: u32,
        old_graph: Option<&'a WaterGraph>,
        dirty_mini_tiles: Option<&'a JsSet<TileRef>>,
        water_components: &'a Components,
        tile_bfs: &'a mut BfsGridScratch,
    ) -> Self {
        let width = map.width();
        let height = map.height();
        Builder {
            map,
            cluster_size,
            width,
            height,
            clusters_x: width.div_ceil(cluster_size), // TS: Math.ceil(width / clusterSize)
            clusters_y: height.div_ceil(cluster_size),
            tile_bfs,
            water_components,
            old_graph,
            dirty_mini_tiles,
            graph: WaterGraph::default(),
            tile_to_node: LookupMap::default(),
            next_node_id: 0,
            next_edge_id: 0,
            edge_between: LookupMap::default(),
            clean_clusters: None,
            old_edge_costs: None,
        }
    }

    /// `build()` (`:265-328`).
    fn build(&mut self) -> WaterGraph {
        self.graph = WaterGraph {
            cluster_size: self.cluster_size,
            clusters_x: self.clusters_x,
            clusters_y: self.clusters_y,
            ..WaterGraph::default()
        };
        // TS: :280-282
        if self.old_graph.is_some() && self.dirty_mini_tiles.is_some_and(|d| !d.is_empty()) {
            self.compute_partial_rebuild_info();
        }
        // TS: :285-290 pre-create all clusters
        let total = (self.clusters_x as usize) * (self.clusters_y as usize);
        self.graph.clusters = vec![None; total];
        for cy in 0..self.clusters_y {
            for cx in 0..self.clusters_x {
                let key = self.graph.get_cluster_key(cx, cy) as usize;
                self.graph.clusters[key] = Some(GraphCluster { x: cx, y: cy, node_ids: Vec::new() });
            }
        }
        // TS: :294-298 gateway nodes
        for cy in 0..self.clusters_y {
            for cx in 0..self.clusters_x {
                self.process_cluster(cx, cy);
            }
        }
        // TS: :303-310 intra-cluster edges, then renumber
        for cy in 0..self.clusters_y {
            for cx in 0..self.clusters_x {
                match self.graph.get_cluster(cx, cy) {
                    Some(c) if !c.node_ids.is_empty() => {}
                    _ => continue,
                }
                self.build_cluster_connections(cx, cy);
            }
        }
        self.sort_edges();
        // TS: :320 `_initPathCache()` (the per-game half is `SegVisibility::reset`)
        let slots = self.graph.seg_slots();
        self.graph.seg_shared = (0..slots).map(|_| OnceLock::new()).collect();
        self.graph.built_at_mini_gen = self.map.terrain_gen();
        // TS: :323 `setWaterComponents`: not captured (module doc)
        std::mem::take(&mut self.graph)
    }

    /// `_sortEdges()` (`:194-203`): `(nodeA, nodeB)` order, ids renumbered, incident lists
    /// rebuilt in id order.
    fn sort_edges(&mut self) {
        let g = &mut self.graph;
        of_det::js_sort_by(&mut g.edges, |a, b| {
            // TS: `a.nodeA - b.nodeA || a.nodeB - b.nodeB`
            let d = f64::from(a.node_a) - f64::from(b.node_a);
            if d != 0.0 { d } else { f64::from(a.node_b) - f64::from(b.node_b) }
        });
        for l in g.node_edges.iter_mut() {
            l.clear();
        }
        let mut eb = LookupMap::with_capacity(g.edges.len());
        for i in 0..g.edges.len() {
            let e = &mut g.edges[i];
            e.id = i as u32;
            let (a, b) = (e.node_a, e.node_b);
            g.node_edges[a as usize].push(i as u32); // in-range: edge endpoints are nodes
            g.node_edges[b as usize].push(i as u32);
            eb.insert(pair_key(a, b), i as u32);
        }
        g.edge_between = eb;
    }

    /// `getOrCreateNode(x, y)` (`:330-349`).
    fn get_or_create_node(&mut self, x: u32, y: u32) -> u32 {
        let tile = self.map.ref_(f64::from(x), f64::from(y));
        if let Some(&id) = self.tile_to_node.get(&tile) {
            return id;
        }
        let id = self.next_node_id;
        self.next_node_id += 1;
        let node = AbstractNode { id, x, y, tile, component_id: self.water_components.get_component_id(tile) };
        // TS: `_addNode`: `_nodes[id] = node; _nodeEdgeIds[id] = []` (ids are dense)
        self.graph.nodes.push(node);
        self.graph.node_edges.push(Vec::new());
        self.tile_to_node.insert(tile, id);
        id
    }

    /// `addNodeToCluster(cx, cy, node)` (`:351-363`): dedup with `includes`.
    fn add_node_to_cluster(&mut self, cluster_x: u32, cluster_y: u32, node_id: u32) {
        let key = self.graph.get_cluster_key(cluster_x, cluster_y) as usize;
        if let Some(Some(c)) = self.graph.clusters.get_mut(key)
            && !c.node_ids.contains(&node_id)
        {
            c.node_ids.push(node_id);
        }
    }

    /// `processCluster(cx, cy)` (`:365-390`).
    fn process_cluster(&mut self, cx: u32, cy: u32) {
        let base_x = cx * self.cluster_size;
        let base_y = cy * self.cluster_size;
        if cx < self.clusters_x - 1 {
            let edge_x = (base_x + self.cluster_size - 1).min(self.width - 1);
            let nodes = self.find_nodes_on_vertical_edge(edge_x, base_y);
            for n in nodes {
                self.add_node_to_cluster(cx, cy, n);
                self.add_node_to_cluster(cx + 1, cy, n);
            }
        }
        if cy < self.clusters_y - 1 {
            let edge_y = (base_y + self.cluster_size - 1).min(self.height - 1);
            let nodes = self.find_nodes_on_horizontal_edge(edge_y, base_x);
            for n in nodes {
                self.add_node_to_cluster(cx, cy, n);
                self.add_node_to_cluster(cx, cy + 1, n);
            }
        }
    }

    /// `findNodesOnVerticalEdge(x, baseY)` (`:392-426`): one node at the middle of every
    /// water span whose right neighbour is water too.
    fn find_nodes_on_vertical_edge(&mut self, x: u32, base_y: u32) -> Vec<u32> {
        let mut nodes = Vec::new();
        let max_y = (base_y + self.cluster_size).min(self.height);
        let mut span_start: i64 = -1;
        for y in base_y..max_y {
            let tile = self.map.ref_(f64::from(x), f64::from(y));
            let is_entrance = self.map.is_water(tile)
                && x + 1 < self.map.width()
                && self.map.is_water(self.map.ref_(f64::from(x + 1), f64::from(y)));
            if is_entrance {
                if span_start == -1 {
                    span_start = i64::from(y);
                }
            } else {
                self.try_add_vertical(&mut span_start, x, y, &mut nodes);
            }
        }
        self.try_add_vertical(&mut span_start, x, max_y, &mut nodes);
        nodes
    }

    fn try_add_vertical(&mut self, span_start: &mut i64, x: u32, y: u32, nodes: &mut Vec<u32>) {
        if *span_start == -1 {
            return;
        }
        let span_length = i64::from(y) - *span_start;
        let mid_y = *span_start + span_length.div_euclid(2); // Math.floor (non-negative)
        *span_start = -1;
        let n = self.get_or_create_node(x, mid_y as u32);
        nodes.push(n);
    }

    /// `findNodesOnHorizontalEdge(y, baseX)` (`:428-462`).
    fn find_nodes_on_horizontal_edge(&mut self, y: u32, base_x: u32) -> Vec<u32> {
        let mut nodes = Vec::new();
        let max_x = (base_x + self.cluster_size).min(self.width);
        let mut span_start: i64 = -1;
        for x in base_x..max_x {
            let tile = self.map.ref_(f64::from(x), f64::from(y));
            let is_entrance = self.map.is_water(tile)
                && y + 1 < self.map.height()
                && self.map.is_water(self.map.ref_(f64::from(x), f64::from(y + 1)));
            if is_entrance {
                if span_start == -1 {
                    span_start = i64::from(x);
                }
            } else {
                self.try_add_horizontal(&mut span_start, x, y, &mut nodes);
            }
        }
        self.try_add_horizontal(&mut span_start, max_x, y, &mut nodes);
        nodes
    }

    fn try_add_horizontal(&mut self, span_start: &mut i64, x: u32, y: u32, nodes: &mut Vec<u32>) {
        if *span_start == -1 {
            return;
        }
        let span_length = i64::from(x) - *span_start;
        let mid_x = *span_start + span_length.div_euclid(2);
        *span_start = -1;
        let n = self.get_or_create_node(mid_x as u32, y);
        nodes.push(n);
    }

    /// `buildClusterConnections(cx, cy)` (`:464-521`).
    fn build_cluster_connections(&mut self, cx: u32, cy: u32) {
        let cluster_key = (cy * self.clusters_x + cx) as usize;
        if self.clean_clusters.as_ref().is_some_and(|c| c.get(cluster_key).copied().unwrap_or(false)) {
            self.build_cluster_connections_from_cache(cx, cy);
            return;
        }
        let Some(cluster) = self.graph.get_cluster(cx, cy) else {
            return;
        };
        let nodes: Vec<AbstractNode> = cluster.node_ids.iter().map(|&id| self.graph.nodes[id as usize]).collect();
        let cluster_min_x = cx * self.cluster_size;
        let cluster_min_y = cy * self.cluster_size;
        let cluster_max_x = (self.width - 1).min(cluster_min_x + self.cluster_size - 1);
        let cluster_max_y = (self.height - 1).min(cluster_min_y + self.cluster_size - 1);
        for i in 0..nodes.len() {
            let from_node = nodes[i];
            let mut target_nodes: Vec<AbstractNode> = Vec::new();
            for nj in nodes.iter().skip(i + 1) {
                if nodes[i].component_id != nj.component_id {
                    continue;
                }
                target_nodes.push(*nj);
            }
            if target_nodes.is_empty() {
                continue;
            }
            let reachable = self.find_all_reachable_nodes_in_bounds(
                from_node.tile,
                &target_nodes,
                cluster_min_x,
                cluster_max_x,
                cluster_min_y,
                cluster_max_y,
            );
            // TS: `for (const [targetId, cost] of reachable.entries())` (insertion order)
            for (target_id, cost) in reachable {
                self.add_or_update_edge(from_node.id, target_id, f64::from(cost), cx, cy);
            }
        }
    }

    /// `addOrUpdateEdge(a, b, cost, clusterX, clusterY)` (`:528-570`).
    fn add_or_update_edge(&mut self, node_id_a: u32, node_id_b: u32, cost: f64, cluster_x: u32, cluster_y: u32) {
        let (lo, hi) = if node_id_a < node_id_b { (node_id_a, node_id_b) } else { (node_id_b, node_id_a) };
        let key = pair_key(lo, hi);
        if let Some(&idx) = self.edge_between.get(&key) {
            let e = &mut self.graph.edges[idx as usize]; // in-range: indices of pushed edges
            if cost < e.cost {
                e.cost = cost;
                e.cluster_x = cluster_x;
                e.cluster_y = cluster_y;
            }
            return;
        }
        let id = self.next_edge_id;
        self.next_edge_id += 1;
        let edge = AbstractEdge { id, node_a: lo, node_b: hi, cost, cluster_x, cluster_y };
        self.edge_between.insert(key, id);
        // TS: `_addEdge`: `_edges[id] = edge` (dense) + both incident lists
        self.graph.edges.push(edge);
        self.graph.node_edges[lo as usize].push(id);
        self.graph.node_edges[hi as usize].push(id);
    }

    /// `findAllReachableNodesInBounds(from, targets, minX, maxX, minY, maxY)` (v36 `:572-625`):
    /// one BFS over water from `from`, returning `(targetId, dist)` in discovery order.
    fn find_all_reachable_nodes_in_bounds(
        &mut self,
        from: TileRef,
        target_nodes: &[AbstractNode],
        min_x: u32,
        max_x: u32,
        min_y: u32,
        max_y: u32,
    ) -> Vec<(u32, u32)> {
        let map = self.map;
        // `tileToNodeId` (lookup-only; tiles of distinct nodes are distinct)
        let mut tile_to_node_id: LookupMap<TileRef, u32> = LookupMap::with_capacity(target_nodes.len());
        for node in target_nodes {
            tile_to_node_id.insert(node.tile, node.id);
        }
        // rules-v36 (t713, upstream #6295 394e150db "Fix boats failing or detouring at river bends"): the bounds check
        // limits the search; a river can wind far inside a cluster, so the cap is clusterSize^2 (v36
        // AbstractGraph.ts:585-586), no longer 4 x the largest Manhattan distance to a target (pin :594).
        let max_distance = self.cluster_size * self.cluster_size;
        let mut reachable: Vec<(u32, u32)> = Vec::new(); // a Map set once per target
        let mut found_count = 0usize;
        let (w, h) = (map.width(), map.height());
        let _ = bfs_grid::search(
            self.tile_bfs,
            w,
            h,
            &[from],
            max_distance,
            |tile| map.is_water(tile),
            |tile, dist| {
                let x = map.x(tile);
                let y = map.y(tile);
                let node_id = tile_to_node_id.get(&tile).copied();
                let is_start_or_target = tile == from || node_id.is_some();
                if !is_start_or_target && (x < min_x || x > max_x || y < min_y || y > max_y) {
                    return Visit::Reject;
                }
                if let Some(id) = node_id {
                    reachable.push((id, dist));
                    found_count += 1;
                    if found_count == target_nodes.len() {
                        return Visit::Found(dist);
                    }
                }
                Visit::Expand
            },
        );
        reachable
    }

    /// `computePartialRebuildInfo()` (`:639-705`).
    fn compute_partial_rebuild_info(&mut self) {
        let (Some(dirty), Some(old_graph)) = (self.dirty_mini_tiles, self.old_graph) else {
            return; // rust-invariant: checked by the caller (`:280`)
        };
        let total = (self.clusters_x as usize) * (self.clusters_y as usize);
        // TS: :644-651 primary dirty cluster keys (Set; only `has` is observable downstream)
        let mut primary: Vec<u32> = Vec::new();
        let mut primary_seen = vec![false; total];
        for &tile in dirty.iter() {
            let x = self.map.x(tile);
            let y = self.map.y(tile);
            let cx = x / self.cluster_size; // Math.floor (non-negative)
            let cy = y / self.cluster_size;
            let k = cy * self.clusters_x + cx;
            if let Some(s) = primary_seen.get_mut(k as usize)
                && !*s
            {
                *s = true;
                primary.push(k);
            }
        }
        // TS: :655-673 expand by the 1-ring
        let mut expanded = vec![false; total];
        for key in primary {
            let cy = key / self.clusters_x;
            let cx = key - cy * self.clusters_x;
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let nx = i64::from(cx) + dx;
                    let ny = i64::from(cy) + dy;
                    if nx >= 0 && nx < i64::from(self.clusters_x) && ny >= 0 && ny < i64::from(self.clusters_y) {
                        expanded[(ny * i64::from(self.clusters_x) + nx) as usize] = true; // in-range: bounds checked
                    }
                }
            }
        }
        // TS: :676-680
        self.clean_clusters = Some(expanded.iter().map(|&d| !d).collect());
        // TS: :683-704 (minTile, maxTile) → cheapest old edge
        let mut costs: LookupMap<u64, OldEdge> = LookupMap::with_capacity(old_graph.edges.len());
        for edge in &old_graph.edges {
            let (Some(na), Some(nb)) = (old_graph.get_node(edge.node_a), old_graph.get_node(edge.node_b)) else {
                continue;
            };
            let tile_min = na.tile.min(nb.tile);
            let tile_max = na.tile.max(nb.tile);
            let k = pair_key(tile_min, tile_max);
            let better = match costs.get(&k) {
                None => true,
                Some(ex) => edge.cost < ex.cost,
            };
            if better {
                costs.insert(k, OldEdge { cost: edge.cost, cluster_x: edge.cluster_x, cluster_y: edge.cluster_y });
            }
        }
        self.old_edge_costs = Some(costs);
    }

    /// `buildClusterConnectionsFromCache(cx, cy)` (`:712-743`): keeps the old edge's
    /// ORIGINAL cluster attribution.
    fn build_cluster_connections_from_cache(&mut self, cx: u32, cy: u32) {
        let Some(cluster) = self.graph.get_cluster(cx, cy) else {
            return;
        };
        let nodes: Vec<AbstractNode> = cluster.node_ids.iter().map(|&id| self.graph.nodes[id as usize]).collect();
        for i in 0..nodes.len() {
            for j in (i + 1)..nodes.len() {
                if nodes[i].component_id != nodes[j].component_id {
                    continue;
                }
                let tile_min = nodes[i].tile.min(nodes[j].tile);
                let tile_max = nodes[i].tile.max(nodes[j].tile);
                let entry = self.old_edge_costs.as_ref().and_then(|m| m.get(&pair_key(tile_min, tile_max)).copied());
                if let Some(e) = entry {
                    self.add_or_update_edge(nodes[i].id, nodes[j].id, e.cost, e.cluster_x, e.cluster_y);
                }
            }
        }
    }
}
