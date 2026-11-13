//! `RecomputeRailClusterExecution` (`engine/src/execution/RecomputeRailClusterExecution.ts`). Owner: **P5**.
//!
//! Fields: the `RecomputeRailCluster v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1).

#[allow(unused_imports)]
use crate::exec::prelude::*;

/// `RecomputeRailClusterExecution` data.
#[derive(StateClone, Debug)]
pub struct RecomputeRailClusterExec {}

#[allow(clippy::new_without_default)] // a TS no-argument constructor; `Default` would be a placeholder
impl RecomputeRailClusterExec {
    /// `new RecomputeRailClusterExecution(railNetwork)` (the network is `cx.st.rail`).
    pub fn new() -> Self {
        RecomputeRailClusterExec {}
    }
}

impl ExecBehavior for RecomputeRailClusterExec {
    /// `isActive()` is always `true` (`:7-9`).
    fn is_active(&self) -> bool {
        true
    }
    /// `activeDuringSpawnPhase()` returns `false` (`RecomputeRailClusterExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    /// `init(mg, ticks) {}` (`:15`).
    fn init(&mut self, _cx: &mut Ctx, _ticks: i32) {}
    /// `tick(ticks)`: `railNetwork.recomputeClusters()` (`:17-19`).
    fn tick(&mut self, cx: &mut Ctx, _ticks: i32) {
        cx.rail_recompute_clusters();
    }
}
