//! `BreakAllianceExecution` (`engine/src/execution/alliance/BreakAllianceExecution.ts`). Owner: **P9**.
//!
//! Fields: the `BreakAlliance v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1).

#[allow(unused_imports)]
use crate::exec::prelude::*;

use of_det::ts_throw;

use crate::canon::{Canon, CanonCx, CanonError, Cs, Uncanon, UncanonCx};
use crate::exec::ai_attack::p9cs;

/// `BreakAllianceExecution` data.
#[derive(StateClone, Debug)]
pub struct BreakAllianceExec {
    pub active: bool,
    pub recipient: Option<SmallId>,
    pub initialized: bool,
    pub requestor: SmallId,
    pub recipient_id: String,
}

impl BreakAllianceExec {
    /// `new BreakAllianceExecution(requestor, recipientID)` (`:17-20`).
    pub fn new(requestor: SmallId, recipient_id: String) -> Self {
        // TS: :13-15 field initialisers
        BreakAllianceExec { active: true, recipient: None, initialized: false, requestor, recipient_id }
    }
}

impl ExecBehavior for BreakAllianceExec {
    fn is_active(&self) -> bool {
        self.active
    }
    /// `activeDuringSpawnPhase()` returns `false` (`alliance/BreakAllianceExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    /// TS: `:22-32`.
    fn init(&mut self, cx: &mut Ctx, ticks: i32) {
        let _ = ticks;
        if !cx.st.has_player(&self.recipient_id) {
            self.active = false;
            return;
        }
        self.recipient = Some(cx.st.player_by_id(&self.recipient_id));
        self.initialized = true; // TS: :31 `this.mg = mg`
    }
    /// TS: `:34-60`.
    fn tick(&mut self, cx: &mut Ctx, ticks: i32) {
        let _ = ticks;
        // TS: :35-41 (`requestor` is never null)
        let recipient = match (self.initialized, self.recipient) {
            (true, Some(r)) => r,
            _ => ts_throw!("Not initialized"),
        };
        match cx.st.player(self.requestor).alliance_with(recipient) {
            None => {} // TS: :43-44 warn only
            Some(alliance) => {
                cx.player_mut(self.requestor).break_alliance(alliance); // TS: :46
                cx.player_mut(recipient).update_relation(self.requestor, -100.0); // TS: :47
                // TS: :49-53 `nearby().filter(n => n.isPlayer() && !n.isOnSameTeam(this.recipient!))`
                let neighbors: Vec<SmallId> = cx
                    .st
                    .player(self.requestor)
                    .nearby()
                    .into_iter()
                    .filter(|&n| n.is_player() && !cx.st.player(n).is_on_same_team(recipient))
                    .collect();
                for neighbor in neighbors {
                    cx.player_mut(neighbor).update_relation(self.requestor, -40.0); // TS: :56
                }
            }
        }
        self.active = false; // TS: :59
    }
}

/// `BreakAllianceExecutionSnapshot` (`:70-78`).
impl Canon for BreakAllianceExec {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        Cs::Obj(vec![
            ("active".into(), self.active.canon(cx)),
            ("recipient".into(), self.recipient.canon(cx)),
            ("initialized".into(), self.initialized.canon(cx)),
            ("requestor".into(), self.requestor.canon(cx)),
            ("recipientID".into(), self.recipient_id.canon(cx)),
        ])
    }
}

impl Uncanon for BreakAllianceExec {
    fn uncanon(v: &Cs, _cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        let f = |k| p9cs::field(v, k);
        Ok(BreakAllianceExec {
            active: p9cs::bool_(f("active")?)?,
            recipient: p9cs::opt_small_id(f("recipient")?)?,
            initialized: p9cs::bool_(f("initialized")?)?,
            requestor: p9cs::small_id(f("requestor")?)?,
            recipient_id: p9cs::str_(f("recipientID")?)?,
        })
    }
}
