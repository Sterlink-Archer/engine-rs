//! `AllianceRejectExecution` (`engine/src/execution/alliance/AllianceRejectExecution.ts`). Owner: **P9**.
//!
//! Fields: the `AllianceReject v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1).

#[allow(unused_imports)]
use crate::exec::prelude::*;

use crate::canon::{Canon, CanonCx, CanonError, Cs, Uncanon, UncanonCx};
use crate::exec::ai_attack::p9cs;

/// `AllianceRejectExecution` data.
#[derive(StateClone, Debug)]
pub struct AllianceRejectExec {
    pub active: bool,
    pub requestor_id: String,
    pub recipient: SmallId,
}

impl AllianceRejectExec {
    /// `new AllianceRejectExecution(requestorID, recipient)` (`:15-18`).
    pub fn new(requestor_id: String, recipient: SmallId) -> Self {
        AllianceRejectExec { active: true, requestor_id, recipient }
    }
}

impl ExecBehavior for AllianceRejectExec {
    fn is_active(&self) -> bool {
        self.active
    }
    /// `activeDuringSpawnPhase()` returns `false` (`alliance/AllianceRejectExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    /// TS: `:20-47`.
    fn init(&mut self, cx: &mut Ctx, ticks: i32) {
        let _ = ticks;
        if !cx.st.has_player(&self.requestor_id) {
            self.active = false; // TS: :21-27
            return;
        }
        let requestor = cx.st.player_by_id(&self.requestor_id);
        if cx.st.player(requestor).is_friendly(self.recipient, false) {
            // TS: :30-33 already allied: warn only
        } else {
            // TS: :35-37 `outgoingAllianceRequests().find(ar => ar.recipient() === this.recipient)`
            let request = cx
                .st
                .player(requestor)
                .outgoing_alliance_requests()
                .into_iter()
                .find(|&ar| cx.st.alliance_request(ar).recipient() == self.recipient);
            if let Some(request) = request {
                cx.reject_alliance_request(request); // TS: :43
            }
        }
        self.active = false; // TS: :46
    }
    /// TS: `:49` empty.
    fn tick(&mut self, cx: &mut Ctx, ticks: i32) {
        let _ = (cx, ticks);
    }
}

/// `AllianceRejectExecutionSnapshot` (`:59-65`).
impl Canon for AllianceRejectExec {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        Cs::Obj(vec![
            ("active".into(), self.active.canon(cx)),
            ("requestorID".into(), self.requestor_id.canon(cx)),
            ("recipient".into(), self.recipient.canon(cx)),
        ])
    }
}

impl Uncanon for AllianceRejectExec {
    fn uncanon(v: &Cs, _cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        Ok(AllianceRejectExec {
            active: p9cs::bool_(p9cs::field(v, "active")?)?,
            requestor_id: p9cs::str_(p9cs::field(v, "requestorID")?)?,
            recipient: p9cs::small_id(p9cs::field(v, "recipient")?)?,
        })
    }
}
