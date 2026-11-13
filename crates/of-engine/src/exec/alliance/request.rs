//! `AllianceRequestExecution` (`engine/src/execution/alliance/AllianceRequestExecution.ts`). Owner: **P9**.
//!
//! Fields: the `AllianceRequest v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1).

#[allow(unused_imports)]
use crate::exec::prelude::*;

use crate::canon::{Canon, CanonCx, CanonError, Cs, Uncanon, UncanonCx};
use crate::exec::ai_attack::p9cs;
use crate::exec::nuke_alliance::{NukeAllianceCheck, would_nuke_break_alliance};
use crate::state::alliance_request::RequestStatus;

/// `AllianceRequestExecution` data.
#[derive(StateClone, Debug)]
pub struct AllianceRequestExec {
    pub active: bool,
    /// `mg` set by `init`
    pub initialized: bool,
    pub req: Option<ReqIdx>,
    pub requestor: SmallId,
    pub recipient_id: String,
}

impl AllianceRequestExec {
    /// `new AllianceRequestExecution(requestor, recipientID)` (`:22-25`).
    pub fn new(requestor: SmallId, recipient_id: String) -> Self {
        // TS: :18-19 `req = null`, `active = true`
        AllianceRequestExec { active: true, initialized: false, req: None, requestor, recipient_id }
    }

    /// `cancelNukesBetweenAlliedPlayers(recipient)` (`:94-176`). The `neutralized` /
    /// `cancelledWarheadLaunchers` tallies only feed the two `displayMessage` calls of
    /// `:153-175` (view, ARCHITECTURE §5), so only the deletions are ported.
    pub fn cancel_nukes_between_allied_players(&mut self, cx: &mut Ctx, recipient: SmallId) {
        let players = [self.requestor, recipient];
        for launcher in players {
            let other = if launcher == self.requestor { recipient } else { self.requestor };
            // TS: :103-108 `launcher.units([...])` is a fresh copy, so deleting is safe
            let units = cx.st.player(launcher).units_of(&[
                UnitType::AtomBomb,
                UnitType::HydrogenBomb,
                UnitType::Mirv,
                UnitType::MirvWarhead,
            ]);
            for unit in units {
                let u = cx.st.unit(unit);
                if !u.is_active() || u.reached_target() {
                    continue;
                }
                // TS: :111 `targetTile` is `TileRef | undefined`; every test below is a JS
                // truthiness test, so tile 0 counts as absent.
                let target_tile = u.target_tile().filter(|&t| t != 0);
                let ty = u.unit_type();
                if ty == UnitType::Mirv {
                    // TS: :118-121 `unit.targetPlayer() ?? (targetTile ? owner(targetTile) : null)`
                    let target = u.target_player().or_else(|| target_tile.map(|t| cx.st.owner(t)));
                    if target != Some(other) {
                        continue;
                    }
                } else if ty == UnitType::MirvWarhead {
                    // TS: :123 `if (!targetTile || this.mg.owner(targetTile) !== other) continue`
                    match target_tile {
                        Some(t) if cx.st.owner(t) == other => {}
                        _ => continue,
                    }
                } else {
                    // TS: :125-137
                    let Some(target_tile) = target_tile else {
                        continue;
                    };
                    let magnitude = cx.st.config().nuke_magnitudes(ty);
                    let mut allies = JsSet::new();
                    allies.insert(other.0); // TS: :132 `new Set([other.smallID()])`
                    let threshold = cx.st.config().nuke_alliance_break_threshold();
                    let breaks = would_nuke_break_alliance(
                        cx.st,
                        NukeAllianceCheck { target_tile, magnitude, ally_small_ids: Some(&allies), threshold },
                    );
                    if !breaks {
                        continue;
                    }
                }
                // TS: :140 `unit.delete(false)`
                cx.unit_mut(unit).delete(Some(false), None);
            }
        }
    }
}

impl ExecBehavior for AllianceRequestExec {
    fn is_active(&self) -> bool {
        self.active
    }
    /// `activeDuringSpawnPhase()` returns `false` (`alliance/AllianceRequestExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    /// TS: `:27-67`.
    fn init(&mut self, cx: &mut Ctx, ticks: i32) {
        let _ = ticks;
        self.initialized = true; // TS: :28 `this.mg = mg`
        if !cx.st.has_player(&self.recipient_id) {
            return; // TS: :29-34 warn + return (stays active; `tick` expires it)
        }
        let recipient = cx.st.player_by_id(&self.recipient_id);
        if !cx.st.player(self.requestor).can_send_alliance_request(recipient) {
            self.active = false; // TS: :38-40
            return;
        }
        // TS: :42-44 `recipient.outgoingAllianceRequests().find(r => r.recipient() === this.requestor)`
        let incoming = cx
            .st
            .player(recipient)
            .outgoing_alliance_requests()
            .into_iter()
            .find(|&r| cx.st.alliance_request(r).recipient() == self.requestor);
        match incoming {
            Some(incoming) => {
                // TS: :45-62 accept the pending reverse request instead
                self.active = false;
                cx.accept_alliance_request(incoming);
                cx.player_mut(self.requestor).update_relation(recipient, 100.0);
                cx.player_mut(recipient).update_relation(self.requestor, 100.0);
                if cx.st.player(self.requestor).has_embargo_against(recipient) {
                    cx.player_mut(self.requestor).end_temporary_embargo(recipient);
                }
                if cx.st.player(recipient).has_embargo_against(self.requestor) {
                    cx.player_mut(recipient).end_temporary_embargo(self.requestor);
                }
                self.cancel_nukes_between_allied_players(cx, recipient);
            }
            None => {
                // TS: :64
                self.req = cx.player_mut(self.requestor).create_alliance_request(recipient);
            }
        }
    }
    /// TS: `:69-84`.
    fn tick(&mut self, cx: &mut Ctx, ticks: i32) {
        let _ = ticks;
        // TS: :70-76 `this.req?.status() === "accepted" || ... === "rejected"`
        if let Some(r) = self.req {
            let s = cx.st.alliance_request(r).status();
            if s == RequestStatus::Accepted || s == RequestStatus::Rejected {
                self.active = false;
                return;
            }
        }
        // TS: :77-83 `this.mg.ticks() - (this.req?.createdAt() ?? 0) > allianceRequestDuration()`
        let created = self.req.map_or(0, |r| cx.st.alliance_request(r).created_at());
        if cx.st.ticks() - created > cx.st.config().alliance_request_duration() {
            if let Some(r) = self.req {
                cx.reject_alliance_request(r); // TS: :81 `this.req?.reject()`
            }
            self.active = false;
        }
    }
}

/// `AllianceRequestExecutionSnapshot` (`:178-186`); `req` is the SPEC §1.2 request key.
impl Canon for AllianceRequestExec {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        let req = match self.req {
            // P13 leaf: emits the SPEC §1.2 key AND queues the D row (merge1; the p9cs shim
            // only emitted the key, so a request referenced only here had no D row).
            Some(r) => cx.request_key(r),
            None => Cs::Null,
        };
        Cs::Obj(vec![
            ("active".into(), self.active.canon(cx)),
            ("initialized".into(), self.initialized.canon(cx)),
            ("req".into(), req),
            ("requestor".into(), self.requestor.canon(cx)),
            ("recipientID".into(), self.recipient_id.canon(cx)),
        ])
    }
}

impl Uncanon for AllianceRequestExec {
    fn uncanon(v: &Cs, cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        let f = |k| p9cs::field(v, k);
        let req = f("req")?;
        Ok(AllianceRequestExec {
            active: p9cs::bool_(f("active")?)?,
            initialized: p9cs::bool_(f("initialized")?)?,
            req: if req.is_null() { None } else { Some(p9cs::req_from(req, cx)?) },
            requestor: p9cs::small_id(f("requestor")?)?,
            recipient_id: p9cs::str_(f("recipientID")?)?,
        })
    }
}
