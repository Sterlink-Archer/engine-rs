//! `Alliance` / `MutableAlliance` (`engine/src/game/AllianceImpl.ts`). Owner: **P1**.
//!
//! Stored in `State.alliances`, index == TS id (`nextAllianceID++`, `GameImpl.ts:483-490`).
//! Pure readers are inherent; methods that reach the game (`expire`, `extend`) are on `Ctx`.

use crate::StateClone;
use crate::api::types::Tick;
use crate::ctx::Ctx;
use crate::ids::{AllianceId, SmallId};
use crate::state::State;

/// `AllianceImpl` data, `Alliance v1` snapshot order.
#[derive(StateClone, Debug, Default, PartialEq, Eq)]
pub struct Alliance {
    pub id: AllianceId,
    pub requestor: SmallId,
    pub recipient: SmallId,
    pub created_at: Tick,
    pub expires_at: Tick,
    pub extension_requested_requestor: bool,
    pub extension_requested_recipient: bool,
}

impl Alliance {
    /// `new AllianceImpl(mg, requestor, recipient, createdAt, id)` (`AllianceImpl.ts:15-30`):
    /// both extension flags start `false`; `expiresAt_ = createdAt + allianceDuration` is
    /// computed by the caller (`GameImpl.acceptAllianceRequest`, which holds the config).
    pub fn new(id: AllianceId, requestor: SmallId, recipient: SmallId, created_at: Tick, expires_at: Tick) -> Self {
        Alliance {
            id,
            requestor,
            recipient,
            created_at,
            expires_at,
            extension_requested_requestor: false,
            extension_requested_recipient: false,
        }
    }
    /// `other(player)` (`:32-37`): the recipient if `player` is the requestor, else the
    /// requestor (also for a non-member `player`, as in TS).
    pub fn other(&self, player: SmallId) -> SmallId {
        if self.requestor == player {
            return self.recipient;
        }
        self.requestor
    }
    pub fn requestor(&self) -> SmallId {
        self.requestor
    }
    pub fn recipient(&self) -> SmallId {
        self.recipient
    }
    pub fn created_at(&self) -> Tick {
        self.created_at
    }
    pub fn expires_at(&self) -> Tick {
        self.expires_at
    }
    pub fn id(&self) -> AllianceId {
        self.id
    }
    /// `addExtensionRequest(player)` (`:54-66`); the `AllianceExtension` update is view work
    /// (ARCHITECTURE §5).
    pub fn add_extension_request(&mut self, player: SmallId) {
        if self.requestor == player {
            self.extension_requested_requestor = true;
        } else if self.recipient == player {
            self.extension_requested_recipient = true;
        }
    }
    /// `bothAgreedToExtend()` (`:68-72`).
    pub fn both_agreed_to_extend(&self) -> bool {
        self.extension_requested_requestor && self.extension_requested_recipient
    }
    /// `onlyOneAgreedToExtend()` (`:74-81`).
    pub fn only_one_agreed_to_extend(&self) -> bool {
        self.extension_requested_requestor != self.extension_requested_recipient
    }
    /// `agreedToExtend(player)` (`:83-88`).
    pub fn agreed_to_extend(&self, player: SmallId) -> bool {
        (self.requestor == player && self.extension_requested_requestor)
            || (self.recipient == player && self.extension_requested_recipient)
    }
}

// `AllianceSnapshot` (`AllianceImpl.ts:103-113`, dump record `Alliance v1`), SPEC §1 D section.
crate::impl_canon_record!(Alliance {
    "id" => id,
    "requestor" => requestor,
    "recipient" => recipient,
    "createdAt" => created_at,
    "expiresAt" => expires_at,
    "extensionRequestedRequestor" => extension_requested_requestor,
    "extensionRequestedRecipient" => extension_requested_recipient,
});

impl State {
    #[inline]
    pub fn alliance(&self, a: AllianceId) -> &Alliance {
        &self.alliances[a.0 as usize] // in-range: ids are indexes into alliances
    }
    #[inline]
    pub fn alliance_mut(&mut self, a: AllianceId) -> &mut Alliance {
        &mut self.alliances[a.0 as usize] // in-range: ids are indexes into alliances
    }
}

impl Ctx<'_> {
    /// `alliance.expire()` (`AllianceImpl.ts:50-52`) → `mg.expireAlliance(this)`.
    pub fn alliance_expire(&mut self, a: AllianceId) {
        self.expire_alliance(a);
    }
    /// `alliance.extend()` (`AllianceImpl.ts:93-97`).
    pub fn alliance_extend(&mut self, a: AllianceId) {
        let expires_at = self.st.ticks() + self.st.config().alliance_duration();
        let al = self.st.alliance_mut(a);
        al.extension_requested_requestor = false;
        al.extension_requested_recipient = false;
        al.expires_at = expires_at;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canon::{Canon, CanonCx, Cs};
    use crate::exec::ExecStore;

    const A: SmallId = SmallId(1);
    const B: SmallId = SmallId(2);
    const C: SmallId = SmallId(3);

    #[test]
    fn other_and_extension_flags() {
        let mut al = Alliance::new(AllianceId(4), A, B, 10, 310);
        assert_eq!((al.other(A), al.other(B)), (B, A));
        // TS `other`: anyone but the requestor gets the requestor.
        assert_eq!(al.other(C), A);
        assert!(!al.both_agreed_to_extend() && !al.only_one_agreed_to_extend());
        al.add_extension_request(C); // neither side: no flag
        assert!(!al.agreed_to_extend(A) && !al.agreed_to_extend(B) && !al.agreed_to_extend(C));
        al.add_extension_request(B);
        assert!(al.only_one_agreed_to_extend() && al.agreed_to_extend(B) && !al.agreed_to_extend(A));
        al.add_extension_request(A);
        assert!(al.both_agreed_to_extend() && !al.only_one_agreed_to_extend());
        // A self-alliance cannot exist; with requestor == recipient TS sets only the requestor flag.
        let mut s = Alliance::new(AllianceId(0), A, A, 0, 0);
        s.add_extension_request(A);
        assert!(s.extension_requested_requestor && !s.extension_requested_recipient);
    }

    #[test]
    fn alliance_canon_is_the_snapshot_record() {
        // The AllianceId leaf (P13, `canon/leaves.rs`) resolves the id against `st.alliances`
        // and queues its D row, so the state must hold alliances 0..=4 (merge1).
        let mut st = State::default();
        for i in 0..=4 {
            st.alliances.push(Alliance::new(AllianceId(i), A, B, 0, 0));
        }
        let ex = ExecStore::default();
        let mut cx = CanonCx::new(&st, &ex);
        let mut al = Alliance::new(AllianceId(4), A, B, 10, 310);
        al.extension_requested_recipient = true;
        assert_eq!(
            al.canon(&mut cx),
            Cs::Obj(vec![
                ("id".into(), Cs::Int(4)),
                ("requestor".into(), Cs::Int(1)),
                ("recipient".into(), Cs::Int(2)),
                ("createdAt".into(), Cs::Int(10)),
                ("expiresAt".into(), Cs::Int(310)),
                ("extensionRequestedRequestor".into(), Cs::Bool(false)),
                ("extensionRequestedRecipient".into(), Cs::Bool(true)),
            ])
        );
    }
}
