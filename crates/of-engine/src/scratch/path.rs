//! Path-finding scratch (owner: **P4b**; the fields of `Scratch` are declared at S0 by P1,
//! the contents of these types are filled by P4b).
//!
//! TS scratch objects covered (ARCHITECTURE §3.8): the simple water chain's `AStarWater`
//! (`PathFinder.ts:53`; `MinHeap`, `AStar.Water.ts:48`), the `AStarWaterBounded` of
//! `SmoothingWaterTransformer` (`:33`) and of `SpatialQuery` (`:33`, lazily), the rail
//! chain (`_railChainCache`, `PathFinder.ts:171-183`; `AStar.Rail.ts`, `BucketQueue`) and
//! the station path finder's `AStar` (`PathFinder.Station.ts:13-14`). Each one keeps its
//! own buffers, re-stamped per query (`PriorityQueue.ts:169-177`).
//!
//! **Sharing is unobservable.** TS keeps one instance per chain / per `SpatialQuery` /
//! per `StationPathFinder`; Rust keeps one per thread. Every query bumps the instance's
//! stamp first and only reads entries carrying the current stamp, and the wrap at
//! `0xffffffff` refills the arrays (`AStar.Water.ts:52-57`), so a buffer's history never
//! reaches a result. The two size-dependent semantics are kept: `AStarWaterBounded`
//! refuses a box larger than its `maxSearchArea` (`AStar.WaterBounded.ts:117-119`), so the
//! capacity is part of the instance ([`AStarWaterBoundedScratch::ensure`]), and the
//! `BucketQueue` clamps to `maxPriority` (rebuilt when it differs, `astar::AStarBuffers`).
//! None of these scratch objects has a TS `finally`, so `after_caught_throw` has nothing
//! to undo.

use super::StepReset;
use crate::path::astar::AStarBuffers;
use crate::water::priority_queue::MinHeap;

/// An `AStarWater` instance (`pathfinding/algorithms/AStar.Water.ts:20-47`).
#[derive(Debug, Default)]
pub struct AStarWaterScratch {
    pub stamp: u32,
    pub closed_stamp: Vec<u32>,
    pub g_score_stamp: Vec<u32>,
    pub g_score: Vec<u32>,
    pub came_from: Vec<i32>,
    /// `new MinHeap(numNodes)`; the capacity only sizes TS's resize warning.
    pub queue: Option<MinHeap>,
}

impl AStarWaterScratch {
    /// Size the buffers for a `num_nodes` map (a fresh TS instance: zeroed, `stamp = 1`).
    pub fn ensure(&mut self, num_nodes: usize) {
        if self.closed_stamp.len() != num_nodes || self.queue.is_none() {
            self.closed_stamp = vec![0; num_nodes];
            self.g_score_stamp = vec![0; num_nodes];
            self.g_score = vec![0; num_nodes];
            self.came_from = vec![0; num_nodes];
            self.queue = Some(crate::path::astar::new_min_heap(num_nodes));
            self.stamp = 1;
        }
    }
}

/// An `AStarWaterBounded` instance (`pathfinding/algorithms/AStar.WaterBounded.ts:29-58`)
/// (also used by P4a's HPA, `sc.hpa.local*`).
#[derive(Debug, Default)]
pub struct AStarWaterBoundedScratch {
    /// The constructor's `maxSearchArea` (== `closedStamp.length`, semantic: `:117-119`).
    pub max_search_area: u32,
    pub stamp: u32,
    pub closed_stamp: Vec<u32>,
    pub g_score_stamp: Vec<u32>,
    pub g_score: Vec<u32>,
    pub came_from: Vec<i32>,
    /// `new MinHeap(maxSearchArea * 4)`.
    pub queue: Option<MinHeap>,
}

impl AStarWaterBoundedScratch {
    /// `new AStarWaterBounded(map, maxSearchArea)`: every TS instance has a fixed capacity,
    /// and each `Scratch` field stands for one instance kind, so the capacity is set on
    /// first use (a different value re-creates the buffers, as a new TS instance would).
    pub fn ensure(&mut self, max_search_area: u32) {
        if self.max_search_area != max_search_area || self.queue.is_none() {
            let n = max_search_area as usize;
            self.max_search_area = max_search_area;
            self.closed_stamp = vec![0; n];
            self.g_score_stamp = vec![0; n];
            self.g_score = vec![0; n];
            self.came_from = vec![0; n];
            self.queue = Some(crate::path::astar::new_min_heap(n * 4));
            self.stamp = 1;
        }
    }
}

/// The rail chain's `AStarRail` (`pathfinding/algorithms/AStar.Rail.ts`): one generic `AStar`.
#[derive(Debug, Default)]
pub struct AStarRailScratch {
    pub astar: AStarBuffers,
}

/// The station path finder's generic `AStar` (`PathFinder.Station.ts:13-14`).
#[derive(Debug, Default)]
pub struct AStarStationScratch {
    pub astar: AStarBuffers,
}

impl StepReset for AStarWaterScratch {}
impl StepReset for AStarWaterBoundedScratch {}
impl StepReset for AStarRailScratch {}
impl StepReset for AStarStationScratch {}
