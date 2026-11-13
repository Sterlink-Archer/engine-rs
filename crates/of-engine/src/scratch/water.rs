//! Water scratch (owner: **P4a**; the fields of `Scratch` are declared at S0 by P1, the
//! contents of these types are filled by P4a).
//!
//! TS scratch objects covered (ARCHITECTURE §3.8):
//! - `WaterManager` stamp arrays `_waterDistArr/_waterStampArr/_waterStamp` and
//!   `_miniDistArr/_miniStampArr/_miniStamp` (u16, `WaterManager.ts:59-67`) and
//!   `_builderBFS` (`:47`, `:77`, shared with `AbstractGraphBuilder`, `AbstractGraph.ts:260`)
//!   → [`WaterMgrScratch`] (`sc.water_mgr`). P4a documents TS's own u16 wrap handling; if
//!   it is not single-pass safe, `prepare_step` must apply the step-start reset (G14).
//! - The HPA internals: `tileBFS`, `AbstractGraphAStar`, the three `AStarWaterBounded`
//!   instances (local, multi-cluster, short-path) and `SourceResolver`
//!   (`AStar.WaterHierarchical.ts:25-50, 60-64`) → [`HpaScratch`] (`sc.hpa`), one field per
//!   TS instance so nesting stays legal exactly where TS nests.

use super::StepReset;
use super::path::AStarWaterBoundedScratch;

/// `sc.water_mgr`.
///
/// **u16 stamp wrap (G14), documented as required:** `bumpFullMapStamp`/`bumpMiniStamp`
/// (`WaterManager.ts:613-629`) zero their stamp array and restart at 1 once the stamp reaches
/// `0xffff`. Every pass (`recomputeMagnitudesInBox`, the shoreline dedup) takes a fresh stamp
/// and only reads `stampArr[t] === stamp` / `distArr[t]` for tiles stamped in that same pass.
/// Stamps only grow between wraps, so a stamp left in the array by an earlier pass (of this or
/// any other game on the thread) is always smaller than the next one. TS's own handling is
/// therefore single-pass safe and sharing one scratch per thread is unobservable: no
/// step-start reset is needed. The arrays are re-created (stamp 0) when the map size differs
/// (`:377-381`, `:571-575`), exactly as TS does on its first use.
#[derive(Debug, Default)]
pub struct WaterMgrScratch {
    pub water_dist: Vec<u16>,
    pub water_stamp_arr: Vec<u16>,
    pub water_stamp: u32,
    pub mini_dist: Vec<u16>,
    pub mini_stamp_arr: Vec<u16>,
    pub mini_stamp: u32,
    /// `_builderBFS` (`BFSGrid`), also the graph builder's `sharedTileBFS`.
    pub builder_bfs: BfsGridScratch,
    /// `ConnectedComponents.queue` (`ConnectedComponents.ts:28,79`): the flood-fill work
    /// queue, alive only during `initialize()` in TS.
    pub cc_queue: Vec<i32>,
}

/// A `BFSGrid` instance (`pathfinding/algorithms/BFS.Grid.ts:1-12`): `stamp` starts at 1;
/// `visitedStamp`/`queue`/`dist` are sized to the grid on first use.
#[derive(Debug)]
pub struct BfsGridScratch {
    pub stamp: u32,
    pub visited_stamp: Vec<u32>,
    pub queue: Vec<i32>,
    pub dist: Vec<u16>,
}

impl Default for BfsGridScratch {
    fn default() -> Self {
        // TS: BFS.Grid.ts:2 `private stamp = 1`
        BfsGridScratch { stamp: 1, visited_stamp: Vec::new(), queue: Vec::new(), dist: Vec::new() }
    }
}

/// An `AbstractGraphAStar` instance (`AStar.AbstractGraph.ts:10-40`, f32 `gScore`). TS builds
/// a new one per graph (`AStar.WaterHierarchical.ts:62`); stamps are relative per query, so
/// one thread-wide instance, grown to the current graph's node count, is unobservable.
#[derive(Debug)]
pub struct AbstractAStarScratch {
    pub stamp: u32,
    pub closed_stamp: Vec<u32>,
    pub g_score_stamp: Vec<u32>,
    pub g_score: Vec<f32>,
    pub came_from: Vec<i32>,
    pub start_node: Vec<i32>,
    pub queue: crate::water::priority_queue::MinHeap,
}

impl Default for AbstractAStarScratch {
    fn default() -> Self {
        // TS: AStar.AbstractGraph.ts:11 `private stamp = 1`
        AbstractAStarScratch {
            stamp: 1,
            closed_stamp: Vec::new(),
            g_score_stamp: Vec::new(),
            g_score: Vec::new(),
            came_from: Vec::new(),
            start_node: Vec::new(),
            queue: crate::water::priority_queue::MinHeap::default(),
        }
    }
}

/// A `SourceResolver` instance (`AStar.WaterHierarchical.ts:414-418`). It holds only the map
/// and the graph (no buffers): nothing to keep here.
#[derive(Debug, Default)]
pub struct SourceResolverScratch {}

/// `sc.hpa`: the per-game `AStarWaterHierarchical`'s search buffers.
#[derive(Debug, Default)]
pub struct HpaScratch {
    pub tile_bfs: BfsGridScratch,
    pub abstract_astar: AbstractAStarScratch,
    pub local: AStarWaterBoundedScratch,
    pub local_multi: AStarWaterBoundedScratch,
    pub local_short: AStarWaterBoundedScratch,
    pub source_resolver: SourceResolverScratch,
}

impl StepReset for WaterMgrScratch {}
impl StepReset for HpaScratch {}
