//! `Scratch`: one field per TS scratch object (ARCHITECTURE §3.8, review 0 #7 + review 1 #16).
//!
//! - Map-sized search buffers (BFS visited arrays, A* queues, traversal stamps) are not
//!   state: they are never cloned and never part of CS. TS keeps them per game or per
//!   map instance; Rust keeps **one `Scratch` per thread**, passed down by `&mut` inside
//!   [`crate::ctx::Ctx`]/[`crate::ctx::Rcx`]. No `RefCell`, no thread-local on the hot path:
//!   the compiler checks the borrows, and a TS "Re-entrant …" throw is an explicit `busy`
//!   flag that raises `ts_throw!` where TS throws.
//! - **Fields are declared by P1 (this file) and filled by their owners** in their own
//!   files below (FILE_MAP §5 rule 5). Adding a field is a one-line P1 PR.
//! - [`Scratch::prepare`] runs at the start of every `Game::step`: each field's
//!   [`StepReset::prepare_step`] applies the step-start stamp-wrap reset (G14) and clears
//!   any guard left set by a throw.
//! - [`Scratch::after_caught_throw`] runs at the one in-tick catch boundary (the
//!   path-finder stepper, PORTING_GUIDE §11) to undo what TS's `finally` blocks undo.
//!
//! | field | TS object | owner |
//! |---|---|---|
//! | `snap_pool` | (Rust only) `JsArr::iter_snapshot` buffers | P1 |
//! | `traversal` | `TileTraversalScratch` (`game/TileTraversalScratch.ts`) | P3 |
//! | `bfs_main`, `bfs_mini` | `GameMapImpl.bfsVisited/bfsQueue/inBfs` of the main map / minimap | P3 |
//! | `water_mgr` | `WaterManager` stamp arrays + `_builderBFS` | P4a |
//! | `hpa` | `AStarWaterHierarchical` internals | P4a |
//! | `astar_water` | simple water chain `AStarWater` (`PathFinder.ts:53`) | P4b |
//! | `smoothing_local` | `SmoothingWaterTransformer.localAStar` (`:33`) | P4b |
//! | `spatial_bounded` | `SpatialQuery.boundedAStar` (`:33`) | P4b |
//! | `astar_rail` | `_railChainCache` rail chain (`PathFinder.ts:171-183`) | P4b |
//! | `astar_station` | `StationPathFinder.aStar` (`PathFinder.Station.ts:14`) | P4b |
//! | `clusters` | `PlayerExecution.removeClusters` per-call arrays | P6 |
//!
//! Owner: **P1** (FILE_MAP §2 foundations).

pub mod bfs;
pub mod clusters;
pub mod path;
pub mod traversal;
pub mod water;

use crate::jsarr::SnapPool;

/// Hooks every scratch field implements (defaults: nothing to do).
pub trait StepReset {
    /// Start of every step: stamp-wrap reset (G14) and clearing of stale guards.
    fn prepare_step(&mut self) {}
    /// After a `TsThrow` was caught inside the tick (the stepper boundary): reset what a TS
    /// `finally` would have reset on the way out (e.g. a `busy` guard).
    fn after_caught_throw(&mut self) {}
}

/// All scratch of one thread. Cheap to create (every buffer starts empty and is sized by
/// its owner on first use).
#[derive(Debug, Default)]
pub struct Scratch {
    pub snap_pool: SnapPool,
    pub traversal: traversal::TraversalScratch,
    pub bfs_main: bfs::BfsScratch,
    pub bfs_mini: bfs::BfsScratch,
    pub water_mgr: water::WaterMgrScratch,
    pub hpa: water::HpaScratch,
    pub astar_water: path::AStarWaterScratch,
    pub smoothing_local: path::AStarWaterBoundedScratch,
    pub spatial_bounded: path::AStarWaterBoundedScratch,
    pub astar_rail: path::AStarRailScratch,
    pub astar_station: path::AStarStationScratch,
    pub clusters: clusters::ClusterScratch,
}

impl Scratch {
    pub fn new() -> Self {
        Self::default()
    }

    /// The step-start hook (called by `Game::step` before the tick; ARCHITECTURE §3.8).
    pub fn prepare(&mut self) {
        let Scratch {
            snap_pool: _,
            traversal,
            bfs_main,
            bfs_mini,
            water_mgr,
            hpa,
            astar_water,
            smoothing_local,
            spatial_bounded,
            astar_rail,
            astar_station,
            clusters,
        } = self; // exhaustive: a new field must be added here too
        traversal.prepare_step();
        bfs_main.prepare_step();
        bfs_mini.prepare_step();
        water_mgr.prepare_step();
        hpa.prepare_step();
        astar_water.prepare_step();
        smoothing_local.prepare_step();
        spatial_bounded.prepare_step();
        astar_rail.prepare_step();
        astar_station.prepare_step();
        clusters.prepare_step();
    }

    /// Called right after the stepper's `catch_unwind` caught a `TsThrow` (PORTING_GUIDE §11).
    pub fn after_caught_throw(&mut self) {
        let Scratch {
            snap_pool: _,
            traversal,
            bfs_main,
            bfs_mini,
            water_mgr,
            hpa,
            astar_water,
            smoothing_local,
            spatial_bounded,
            astar_rail,
            astar_station,
            clusters,
        } = self;
        traversal.after_caught_throw();
        bfs_main.after_caught_throw();
        bfs_mini.after_caught_throw();
        water_mgr.after_caught_throw();
        hpa.after_caught_throw();
        astar_water.after_caught_throw();
        smoothing_local.after_caught_throw();
        spatial_bounded.after_caught_throw();
        astar_rail.after_caught_throw();
        astar_station.after_caught_throw();
        clusters.after_caught_throw();
    }
}

// The one sanctioned thread-local in the crate: fetched ONCE per `Game::step`
// (ARCHITECTURE §3.8), never on the hot path.
thread_local! {
    #[allow(clippy::disallowed_types)]
    static THREAD_SCRATCH: std::cell::RefCell<Option<Box<Scratch>>> = const { std::cell::RefCell::new(None) };
}

/// Run `f` with this thread's `Scratch` (created on first use). Nested use is a Rust bug
/// and panics (`RefCell` borrow); a panic inside `f` keeps the scratch (its guards are
/// reset by the next [`Scratch::prepare`]).
pub fn with_thread_scratch<R>(f: impl FnOnce(&mut Scratch) -> R) -> R {
    THREAD_SCRATCH.with(|cell| {
        let mut slot = cell.borrow_mut();
        let sc = slot.get_or_insert_with(Box::default);
        f(sc)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prepare_resets_guards_and_stamps() {
        let mut sc = Scratch::new();
        sc.bfs_main.busy = true;
        sc.traversal.ensure(4);
        sc.traversal.generation = traversal::GEN_RESET_ABOVE + 1;
        sc.prepare();
        assert!(!sc.bfs_main.busy);
        assert_eq!(sc.traversal.generation, 0);
        sc.bfs_mini.enter();
        sc.after_caught_throw();
        assert!(!sc.bfs_mini.busy);
    }
    #[test]
    fn thread_scratch_persists_per_thread() {
        with_thread_scratch(|sc| sc.traversal.generation = 42);
        assert_eq!(with_thread_scratch(|sc| sc.traversal.generation), 42);
        std::thread::spawn(|| assert_eq!(with_thread_scratch(|sc| sc.traversal.generation), 0)).join().expect("thread");
    }
}
