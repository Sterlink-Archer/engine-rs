//! `RetreatExecution` (`engine/src/execution/RetreatExecution.ts`). Owner: **P6**.
//!
//! Fields: the `Retreat v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1).

#[allow(unused_imports)]
use crate::exec::prelude::*;

/// `cancelDelay = 20` (`:11`).
const CANCEL_DELAY: i32 = 20;

/// `RetreatExecution` data.
#[derive(StateClone, Debug)]
pub struct RetreatExec {
    pub active: bool,
    pub initialized: bool,
    pub retreat_ordered: bool,
    pub start_tick: Option<i32>,
    pub player: SmallId,
    pub attack_id: String,
}

impl RetreatExec {
    /// `new RetreatExecution(player, attackID)`.
    pub fn new(player: SmallId, attack_id: String) -> Self {
        // TS field initialisers (`:14-16`): `active = true`, `retreatOrdered = false`,
        // `startTick` unset.
        RetreatExec { active: true, initialized: false, retreat_ordered: false, start_tick: None, player, attack_id }
    }
    /// `owner()` (`:40-42`).
    pub fn owner(&self) -> SmallId {
        self.player
    }
}

impl ExecBehavior for RetreatExec {
    fn is_active(&self) -> bool {
        self.active
    }
    /// `activeDuringSpawnPhase()` returns `false` (`RetreatExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    /// `init(mg, ticks)` (`:23-26`): `startTick = mg.ticks()`.
    fn init(&mut self, cx: &mut Ctx, _ticks: i32) {
        self.initialized = true;
        self.start_tick = Some(cx.st.ticks());
    }
    /// `tick(ticks)` (`:28-38`).
    fn tick(&mut self, cx: &mut Ctx, _ticks: i32) {
        if !self.retreat_ordered {
            cx.player_mut(self.player).order_retreat(&self.attack_id);
            self.retreat_ordered = true;
        }
        // TS: `this.mg.ticks() >= this.startTick + cancelDelay`. An unset `startTick` would be
        // `undefined + 20 = NaN` (comparison false); a tick always follows init here.
        let due = match self.start_tick {
            Some(s) => cx.st.ticks() >= s + CANCEL_DELAY,
            None => false,
        };
        if due {
            cx.player_mut(self.player).execute_retreat(&self.attack_id);
            self.active = false;
        }
    }
}

// CS v1 export (`Retreat v1 (execution)` record, dump field order).
crate::impl_canon_record!(RetreatExec { "active" => active, "initialized" => initialized, "retreatOrdered" => retreat_ordered, "startTick" => start_tick, "player" => player, "attackID" => attack_id });

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructor_defaults_and_init_stamps_start_tick() {
        let mut e = RetreatExec::new(SmallId(4), "0000abcd".into());
        assert!(e.active && !e.initialized && !e.retreat_ordered && e.start_tick.is_none());
        assert_eq!(e.owner(), SmallId(4));
        let mut st = crate::state::State::default();
        st.ticks = 37;
        let mut sc = crate::scratch::Scratch::new();
        let mut cx = Ctx::new(&mut st, &mut sc, None);
        e.init(&mut cx, 37);
        assert!(e.initialized);
        assert_eq!(e.start_tick, Some(37));
        assert!(!e.active_during_spawn_phase());
    }

    /// CS export follows the `Retreat v1 (execution)` dump order; an unset `startTick` is null.
    #[test]
    fn canon_record_order() {
        use crate::canon::{Canon, CanonCx, Cs};
        let st = crate::state::State::default();
        let ex = crate::exec::ExecStore::default();
        let mut cx = CanonCx::new(&st, &ex);
        let e = RetreatExec::new(SmallId(2), "00000abc".into());
        assert_eq!(
            e.canon(&mut cx),
            Cs::Obj(vec![
                ("active".into(), Cs::Bool(true)),
                ("initialized".into(), Cs::Bool(false)),
                ("retreatOrdered".into(), Cs::Bool(false)),
                ("startTick".into(), Cs::Null),
                ("player".into(), Cs::Int(2)),
                ("attackID".into(), Cs::Str("00000abc".into())),
            ])
        );
    }
}
