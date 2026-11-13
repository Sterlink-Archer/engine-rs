//! `NoOpExecution` (`engine/src/execution/NoOpExecution.ts`). Owner: **P1**. Fully ported.

#[allow(unused_imports)]
use crate::exec::prelude::*;

/// `NoOpExecution` (no fields; `NoOp v1` snapshot is `{}`).
#[derive(StateClone, Debug, Default)]
pub struct NoOpExec {}

impl NoOpExec {
    /// `new NoOpExecution()`.
    pub fn new() -> Self {
        NoOpExec {}
    }
}

impl ExecBehavior for NoOpExec {
    /// `isActive()` returns `false` (`NoOpExecution.ts:2-4`).
    fn is_active(&self) -> bool {
        false
    }
    /// `activeDuringSpawnPhase()` returns `false` (`:5-7`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    /// `init(mg, ticks): void {}` (`:8`).
    fn init(&mut self, _cx: &mut Ctx, _ticks: i32) {}
    /// `tick(ticks): void {}` (`:9`).
    fn tick(&mut self, _cx: &mut Ctx, _ticks: i32) {}
}

// CS v1 `NoOp v1 (execution)`: the empty record `{}` (`NoOpExecution.ts`; merge1).
impl crate::canon::Canon for NoOpExec {
    fn canon(&self, _cx: &mut crate::canon::CanonCx<'_>) -> crate::canon::Cs {
        crate::canon::Cs::Obj(Vec::new())
    }
}
