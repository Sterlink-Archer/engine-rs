//! `TargetPlayerExecution` (`engine/src/execution/TargetPlayerExecution.ts`). Owner: **P9**.
//!
//! Fields: the `TargetPlayer v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1).

#[allow(unused_imports)]
use crate::exec::prelude::*;

use crate::canon::{Canon, CanonCx, CanonError, Cs, Uncanon, UncanonCx};
use crate::exec::ai_attack::p9cs;

/// `TargetPlayerExecution` data.
#[derive(StateClone, Debug)]
pub struct TargetPlayerExec {
    pub active: bool,
    pub target: Option<SmallId>,
    pub requestor: SmallId,
    pub target_id: String,
}

impl TargetPlayerExec {
    /// `new TargetPlayerExecution(requestor, targetID)` (`:17-20`).
    pub fn new(requestor: SmallId, target_id: String) -> Self {
        TargetPlayerExec { active: true, target: None, requestor, target_id }
    }
}

impl ExecBehavior for TargetPlayerExec {
    fn is_active(&self) -> bool {
        self.active
    }
    /// `activeDuringSpawnPhase()` returns `false` (`TargetPlayerExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    /// TS: `:22-30`.
    fn init(&mut self, cx: &mut Ctx, ticks: i32) {
        let _ = ticks;
        if !cx.st.has_player(&self.target_id) {
            self.active = false;
            return;
        }
        self.target = Some(cx.st.player_by_id(&self.target_id));
    }
    /// TS: `:32-38`.
    fn tick(&mut self, cx: &mut Ctx, ticks: i32) {
        let _ = ticks;
        let target = self.target.ts_expect("TargetPlayerExecution.ts:33 target");
        if cx.st.player(self.requestor).can_target(target) {
            cx.player_mut(self.requestor).target(target);
            cx.player_mut(target).update_relation(self.requestor, -40.0);
        }
        self.active = false;
    }
}

/// `TargetPlayerExecutionSnapshot`, `TargetPlayer v1 (execution)` field order.
impl Canon for TargetPlayerExec {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        Cs::Obj(vec![
            ("active".into(), self.active.canon(cx)),
            ("target".into(), self.target.canon(cx)),
            ("requestor".into(), self.requestor.canon(cx)),
            ("targetID".into(), self.target_id.canon(cx)),
        ])
    }
}

impl Uncanon for TargetPlayerExec {
    fn uncanon(v: &Cs, _cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        let f = |k| p9cs::field(v, k);
        Ok(TargetPlayerExec {
            active: p9cs::bool_(f("active")?)?,
            target: p9cs::opt_small_id(f("target")?)?,
            requestor: p9cs::small_id(f("requestor")?)?,
            target_id: p9cs::str_(f("targetID")?)?,
        })
    }
}
