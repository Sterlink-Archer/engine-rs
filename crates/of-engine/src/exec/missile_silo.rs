//! `MissileSiloExecution` (`engine/src/execution/MissileSiloExecution.ts`). Owner: **P8**.
//!
//! Fields: the `MissileSilo v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1).

#[allow(unused_imports)]
use crate::exec::prelude::*;

/// `MissileSiloExecution` data.
#[derive(StateClone, Debug)]
pub struct MissileSiloExec {
    pub active: bool,
    /// `mg !== undefined` (set by `init`).
    pub initialized: bool,
    pub silo: UnitRef,
}

impl MissileSiloExec {
    /// `new MissileSiloExecution(silo)`.
    pub fn new(silo: UnitRef) -> Self {
        // TS: MissileSiloExecution.ts:12-18
        MissileSiloExec { active: true, initialized: false, silo }
    }
}

impl ExecBehavior for MissileSiloExec {
    fn is_active(&self) -> bool {
        self.active
    }
    /// `activeDuringSpawnPhase()` returns `false` (`MissileSiloExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    /// `init(mg, ticks)` (`:20-22`): `this.mg = mg`.
    fn init(&mut self, _cx: &mut Ctx, _ticks: i32) {
        self.initialized = true;
    }
    /// `tick(ticks)` (`:24-46`).
    fn tick(&mut self, cx: &mut Ctx, _ticks: i32) {
        if cx.st.unit(self.silo).is_under_construction() {
            return;
        }

        if !cx.st.unit(self.silo).is_active() {
            self.active = false;
            return;
        }

        // frontTime is the time the earliest missile fired.
        // TS: `missileTimerQueue()[0]` — `undefined` on an empty queue.
        let Some(&front_time) = cx.st.unit(self.silo).missile_timer_queue().first() else {
            return;
        };

        // TS: `SiloCooldown() - (mg.ticks() - frontTime)` (number arithmetic).
        let cooldown = cx.st.config().silo_cooldown() - f64::from(cx.st.ticks() - front_time);

        if cooldown <= 0.0 {
            cx.unit_mut(self.silo).reload_missile();
        }
    }
}

// CS v1 `MissileSilo v1 (execution)` record (`MissileSiloExecution.ts:549-555`). Written at
// merge1 by the integrator on behalf of P8.
crate::impl_canon_record!(MissileSiloExec { "active" => active, "initialized" => initialized, "silo" => silo });

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::State;

    #[test]
    fn new_and_init_match_ts() {
        let u = UnitRef::default();
        let mut e = MissileSiloExec::new(u);
        assert!(e.active && !e.initialized && e.is_active() && !e.active_during_spawn_phase());
        assert_eq!(e.silo, u);
        let mut st = State::default();
        let mut sc = crate::scratch::Scratch::new();
        let mut cx = Ctx::new(&mut st, &mut sc, None);
        e.init(&mut cx, 0);
        assert!(e.initialized && e.active);
    }
}
