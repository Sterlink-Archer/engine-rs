//! `AllianceExtensionExecution` (`engine/src/execution/alliance/AllianceExtensionExecution.ts`). Owner: **P9**.
//!
//! Fields: the `AllianceExtension v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1).
//! All work happens in `init`; `isActive()` is always `false`. The two `displayMessage`
//! calls (`:51-68`, `:72-80`) are view-only and dropped (ARCHITECTURE §5).

#[allow(unused_imports)]
use crate::exec::prelude::*;

use crate::canon::{Canon, CanonCx, CanonError, Cs, Uncanon, UncanonCx};
use crate::exec::ai_attack::p9cs;

/// `AllianceExtensionExecution` data.
#[derive(StateClone, Debug)]
pub struct AllianceExtensionExec {
    pub from: SmallId,
    /// `toID: PlayerID`
    pub to_id: String,
}

impl AllianceExtensionExec {
    /// `new AllianceExtensionExecution(from, toID)` (`:13-16`).
    pub fn new(from: SmallId, to_id: String) -> Self {
        AllianceExtensionExec { from, to_id }
    }
}

impl ExecBehavior for AllianceExtensionExec {
    /// TS: `:88-90` `return false`.
    fn is_active(&self) -> bool {
        false
    }
    /// `activeDuringSpawnPhase()` returns `false` (`alliance/AllianceExtensionExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    /// TS: `:18-82`.
    fn init(&mut self, cx: &mut Ctx, ticks: i32) {
        let _ = ticks;
        if !cx.st.has_player(&self.to_id) {
            return; // TS: :19-24 console.warn + return
        }
        let to = cx.st.player_by_id(&self.to_id);
        // TS: :27-32
        if !cx.st.player(self.from).is_alive() || !cx.st.player(to).is_alive() {
            return;
        }
        // TS: :34-40 `if (!alliance)`
        let Some(alliance) = cx.st.player(self.from).alliance_with(to) else {
            return;
        };
        // TS: :43 check whether this is a new request (before adding it)
        let was_only_one_agreed = cx.st.alliance(alliance).only_one_agreed_to_extend();
        // TS: :46
        cx.st.alliance_mut(alliance).add_extension_request(self.from);
        if cx.st.alliance(alliance).both_agreed_to_extend() {
            // TS: :49 (messages :51-68 dropped)
            cx.alliance_extend(alliance);
        } else if cx.st.alliance(alliance).only_one_agreed_to_extend() && !was_only_one_agreed {
            // TS: :69-81: only a message (dropped)
        }
    }
    /// TS: `:84-86` no-op.
    fn tick(&mut self, cx: &mut Ctx, ticks: i32) {
        let _ = (cx, ticks);
    }
}

/// `AllianceExtensionExecutionSnapshot` (`:96-101`).
impl Canon for AllianceExtensionExec {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        Cs::Obj(vec![("from".into(), self.from.canon(cx)), ("toID".into(), self.to_id.canon(cx))])
    }
}

impl Uncanon for AllianceExtensionExec {
    fn uncanon(v: &Cs, _cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        Ok(AllianceExtensionExec {
            from: p9cs::small_id(p9cs::field(v, "from")?)?,
            to_id: p9cs::str_(p9cs::field(v, "toID")?)?,
        })
    }
}
