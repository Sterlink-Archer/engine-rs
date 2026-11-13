//! `QuickChatExecution` (`engine/src/execution/QuickChatExecution.ts`). Owner: **P9**.
//!
//! Fields: the `QuickChat v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1).
//! `displayChat` (`:48-63`) and the `console.log` (`:66-68`) are view-only and dropped
//! (ARCHITECTURE §5); `getMessageFromKey` (`:85-88`) only feeds them.

#[allow(unused_imports)]
use crate::exec::prelude::*;

use crate::canon::{Canon, CanonCx, CanonError, Cs, Uncanon, UncanonCx};
use crate::exec::ai_attack::p9cs;

/// `QuickChatExecution` data.
#[derive(StateClone, Debug)]
pub struct QuickChatExec {
    pub active: bool,
    pub initialized: bool,
    pub recipient: Option<SmallId>,
    pub sender: SmallId,
    pub recipient_id: String,
    pub quick_chat_key: String,
    pub target: Option<String>,
}

impl QuickChatExec {
    /// `new QuickChatExecution(sender, recipientID, quickChatKey, target)` (`:18-23`).
    pub fn new(sender: SmallId, recipient_id: String, quick_chat_key: String, target: Option<String>) -> Self {
        QuickChatExec {
            active: true,
            initialized: false,
            recipient: None,
            sender,
            recipient_id,
            quick_chat_key,
            target,
        }
    }
}

impl ExecBehavior for QuickChatExec {
    fn is_active(&self) -> bool {
        self.active
    }
    /// `activeDuringSpawnPhase()` returns `false` (`QuickChatExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    /// TS: `:25-36`.
    fn init(&mut self, cx: &mut Ctx, ticks: i32) {
        let _ = ticks;
        self.initialized = true; // TS: :26 `this.mg = mg`
        if !cx.st.has_player(&self.recipient_id) {
            self.active = false;
            return;
        }
        self.recipient = Some(cx.st.player_by_id(&self.recipient_id));
    }
    /// TS: `:38-71`.
    fn tick(&mut self, cx: &mut Ctx, ticks: i32) {
        let _ = ticks;
        let recipient = self.recipient.ts_expect("QuickChatExecution.ts:39 recipient");
        if !cx.st.player(self.sender).can_send_quick_chat(recipient) {
            self.active = false;
            return;
        }
        cx.player_mut(self.sender).record_quick_chat(recipient); // TS: :46
        self.active = false;
    }
}

/// `QuickChatExecutionSnapshot`, `QuickChat v1 (execution)` field order; `target` is
/// `str?undef` (absent when `undefined`).
impl Canon for QuickChatExec {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        let target = match &self.target {
            Some(t) => Cs::Str(t.clone()),
            None => Cs::Absent,
        };
        Cs::Obj(vec![
            ("active".into(), self.active.canon(cx)),
            ("initialized".into(), self.initialized.canon(cx)),
            ("recipient".into(), self.recipient.canon(cx)),
            ("sender".into(), self.sender.canon(cx)),
            ("recipientID".into(), self.recipient_id.canon(cx)),
            ("quickChatKey".into(), self.quick_chat_key.canon(cx)),
            ("target".into(), target),
        ])
    }
}

impl Uncanon for QuickChatExec {
    fn uncanon(v: &Cs, _cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        let f = |k| p9cs::field(v, k);
        let target = match v.get("target") {
            None | Some(Cs::Absent) => None,
            Some(t) => Some(p9cs::str_(t)?),
        };
        Ok(QuickChatExec {
            active: p9cs::bool_(f("active")?)?,
            initialized: p9cs::bool_(f("initialized")?)?,
            recipient: p9cs::opt_small_id(f("recipient")?)?,
            sender: p9cs::small_id(f("sender")?)?,
            recipient_id: p9cs::str_(f("recipientID")?)?,
            quick_chat_key: p9cs::str_(f("quickChatKey")?)?,
            target,
        })
    }
}
