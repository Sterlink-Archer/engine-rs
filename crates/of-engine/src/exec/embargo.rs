//! `EmbargoExecution` (`engine/src/execution/EmbargoExecution.ts`). Owner: **P9**.
//!
//! Fields: the `Embargo v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1).

#[allow(unused_imports)]
use crate::exec::prelude::*;

use crate::canon::{Canon, CanonCx, CanonError, Cs, Uncanon, UncanonCx};
use crate::exec::ai_attack::p9cs;

/// `EmbargoExecution` data.
#[derive(StateClone, Debug)]
pub struct EmbargoExec {
    pub active: bool,
    pub target: Option<SmallId>,
    pub player: SmallId,
    pub target_id: String,
    pub action: EmbargoAction,
}

impl EmbargoExec {
    /// `new EmbargoExecution(player, targetID, action)` (`:17-21`).
    pub fn new(player: SmallId, target_id: String, action: EmbargoAction) -> Self {
        EmbargoExec { active: true, target: None, player, target_id, action }
    }
}

impl ExecBehavior for EmbargoExec {
    fn is_active(&self) -> bool {
        self.active
    }
    /// `activeDuringSpawnPhase()` returns `false` (`EmbargoExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    /// TS: `:23-30`.
    fn init(&mut self, cx: &mut Ctx, ticks: i32) {
        let _ = ticks;
        if !cx.st.has_player(&self.target_id) {
            self.active = false;
            return;
        }
        self.target = Some(cx.st.player_by_id(&self.target_id));
    }
    /// TS: `:32-37`.
    fn tick(&mut self, cx: &mut Ctx, ticks: i32) {
        let _ = ticks;
        // `this.target` is set whenever the execution is still active after init.
        let target = self.target.ts_expect("EmbargoExecution.ts:33 target");
        match self.action {
            EmbargoAction::Start => cx.player_mut(self.player).add_embargo(target, false),
            EmbargoAction::Stop => cx.player_mut(self.player).stop_embargo(target),
        }
        self.active = false;
    }
}

/// `EmbargoExecutionSnapshot`, `Embargo v1 (execution)` field order.
impl Canon for EmbargoExec {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        Cs::Obj(vec![
            ("active".into(), self.active.canon(cx)),
            ("target".into(), self.target.canon(cx)),
            ("player".into(), self.player.canon(cx)),
            ("targetID".into(), self.target_id.canon(cx)),
            ("action".into(), p9cs::embargo_action_cs(self.action)),
        ])
    }
}

impl Uncanon for EmbargoExec {
    fn uncanon(v: &Cs, _cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        let f = |k| p9cs::field(v, k);
        Ok(EmbargoExec {
            active: p9cs::bool_(f("active")?)?,
            target: p9cs::opt_small_id(f("target")?)?,
            player: p9cs::small_id(f("player")?)?,
            target_id: p9cs::str_(f("targetID")?)?,
            action: p9cs::embargo_action(f("action")?)?,
        })
    }
}
