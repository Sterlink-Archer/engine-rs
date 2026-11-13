//! `SpawnTimerExecution` (`engine/src/execution/SpawnTimerExecution.ts`). Owner: **P6**.
//!
//! Fields: the `SpawnTimer v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1).
//!
//! `isActive()` is `this.mg.inSpawnPhase()` (`:19-21`), but the frozen S0 signature
//! `ExecBehavior::is_active(&self)` cannot read the game, and the frozen field list (`{initialized}`;
//! `exec/mod.rs:566` builds the struct literally) leaves no room for a phase mirror. Shim:
//! `is_active` returns `true` and `tick` re-checks the phase itself, so the **simulation** is
//! identical (TS ticks it only during the phase; here a post-phase tick is a no-op). The one
//! residual difference is compaction (`GameImpl.ts:757-773`): TS drops the execution at the end
//! of the tick that ended the phase, this port keeps it listed, which shifts CS `G.execs`.
//! Filed as `docs/requests/P6.md` #1 (a non-snapshot `phase_over` field, set by `tick` right
//! after `endSpawnPhase()`; exact because outside singleplayer, where no SpawnTimer exists
//! (`GameRunner.ts:184-186`), only this execution ends the phase).

#[allow(unused_imports)]
use crate::exec::prelude::*;

/// `SpawnTimerExecution` data.
#[derive(StateClone, Debug)]
pub struct SpawnTimerExec {
    pub initialized: bool,
}

#[allow(clippy::new_without_default)] // a TS no-argument constructor; `Default` would be a placeholder
impl SpawnTimerExec {
    /// `new SpawnTimerExecution()`.
    pub fn new() -> Self {
        SpawnTimerExec { initialized: false }
    }
}

impl ExecBehavior for SpawnTimerExec {
    /// `isActive()` = `this.mg.inSpawnPhase()` (`:19-21`): shimmed to `true` (module doc,
    /// request #1). Tick pass: TS ticks it exactly while the phase lasts; here a post-phase
    /// tick returns at once. Compaction: TS drops it once the phase is over; here it stays.
    fn is_active(&self) -> bool {
        true
    }
    /// `isActive()` = `this.mg.inSpawnPhase()` (`SpawnTimerExecution.ts:19-21`), the exact form
    /// the driver's tick gate and compaction call (`ExecBehavior::is_active_in`, request P6 #1,
    /// merge1). TS compaction drops the execution at the end of the tick that ended the phase.
    fn is_active_in(&self, st: &crate::state::State) -> bool {
        st.in_spawn_phase()
    }
    /// `activeDuringSpawnPhase()` returns `true` (`SpawnTimerExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        true
    }
    /// `init(mg)` (`:4-6`).
    fn init(&mut self, _cx: &mut Ctx, _ticks: i32) {
        self.initialized = true;
    }
    /// `tick()` (`:8-12`).
    fn tick(&mut self, cx: &mut Ctx, _ticks: i32) {
        // TS only ticks it while `isActive()` (= in the spawn phase) holds; see `is_active`.
        if !cx.st.in_spawn_phase() {
            return;
        }
        if f64::from(cx.st.ticks()) > cx.st.config().num_spawn_phase_turns() {
            cx.end_spawn_phase();
        }
    }
}
// CS v1 export (`SpawnTimer v1 (execution)` record, dump field order).
crate::impl_canon_record!(SpawnTimerExec { "initialized" => initialized });
