//! `AllianceRequest` (`engine/src/game/AllianceRequestImpl.ts`). Owner: **P1**.
//!
//! Stored append-only in `State.alliance_reqs` (index = creation order, [`ReqIdx`]);
//! `State.alliance_requests` (the TS `GameImpl.allianceRequests` array) is filter-reassigned
//! on accept/reject (`GameImpl.ts:468-518`). `accept()`/`reject()` call back into the game
//! (`:45-52`) → `Ctx::accept_alliance_request`/`reject_alliance_request`.

use crate::StateClone;
use crate::api::types::Tick;
use crate::canon::{Canon, CanonCx, Cs};
use crate::ctx::{Ctx, ts_throw};
use crate::ids::{AllianceId, ReqIdx, SmallId};
use crate::plain_copy;
use crate::state::State;
use crate::state::alliance::Alliance;

/// `"pending" | "accepted" | "rejected"`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RequestStatus {
    #[default]
    Pending,
    Accepted,
    Rejected,
}
plain_copy!(RequestStatus);

impl RequestStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            RequestStatus::Pending => "pending",
            RequestStatus::Accepted => "accepted",
            RequestStatus::Rejected => "rejected",
        }
    }
}

/// `AllianceRequestImpl` data, `AllianceRequest v1` snapshot order.
#[derive(StateClone, Debug, Default, PartialEq, Eq)]
pub struct AllianceRequest {
    pub requestor: SmallId,
    pub recipient: SmallId,
    /// `tickCreated`.
    pub created_at: Tick,
    pub status: RequestStatus,
}

impl AllianceRequest {
    /// `new AllianceRequestImpl(requestor, recipient, tickCreated, game)`: `status_ = "pending"`
    /// (`AllianceRequestImpl.ts:19-27`).
    pub fn new(requestor: SmallId, recipient: SmallId, created_at: Tick) -> Self {
        AllianceRequest { requestor, recipient, created_at, status: RequestStatus::Pending }
    }
    pub fn status(&self) -> RequestStatus {
        self.status
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
    /// The SPEC §1.2 key `"<requestor>:<recipient>:<createdAt>"` (decimal smallIDs and tick).
    pub fn canon_key(&self) -> String {
        format!("{}:{}:{}", self.requestor.0, self.recipient.0, self.created_at)
    }
}

/// The D-section row (SPEC §1 D: `{key, requestor, recipient, createdAt, status}`, N6 adds
/// `key`); the rest is `AllianceRequestSnapshot` order (`AllianceRequestImpl.ts:63-70`).
impl Canon for AllianceRequest {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        Cs::Obj(vec![
            ("key".to_owned(), Cs::Str(self.canon_key())),
            ("requestor".to_owned(), self.requestor.canon(cx)),
            ("recipient".to_owned(), self.recipient.canon(cx)),
            ("createdAt".to_owned(), self.created_at.canon(cx)),
            ("status".to_owned(), self.status.canon(cx)),
        ])
    }
}

impl State {
    #[inline]
    pub fn alliance_request(&self, r: ReqIdx) -> &AllianceRequest {
        &self.alliance_reqs[r.0 as usize] // in-range: ReqIdx comes from create_alliance_request
    }
    /// `allianceRequests` (the live JS array) in list order.
    pub fn alliance_requests(&self) -> &[ReqIdx] {
        self.alliance_requests.as_slice()
    }
}

impl Ctx<'_> {
    /// `GameImpl.createAllianceRequest(requestor, recipient)` (`GameImpl.ts:440-466`). The
    /// `console.log` lines and the `AllianceRequest` update are dropped (ARCHITECTURE §5).
    pub fn create_alliance_request(&mut self, requestor: SmallId, recipient: SmallId) -> Option<ReqIdx> {
        // TS: :444-447
        if self.st.player(requestor).is_allied_with(recipient) {
            return None;
        }
        // TS: :448-455 `recipient.incomingAllianceRequests().find((ar) => ar.requestor() === requestor)`
        let dup = self
            .st
            .player(recipient)
            .incoming_alliance_requests()
            .into_iter()
            .find(|&r| self.st.alliance_request(r).requestor == requestor);
        if dup.is_some() {
            return None;
        }
        // TS: :456-462
        let corresponding = self
            .st
            .player(requestor)
            .incoming_alliance_requests()
            .into_iter()
            .find(|&r| self.st.alliance_request(r).requestor == recipient);
        if let Some(r) = corresponding {
            self.accept_alliance_request(r); // TS: correspondingReq.accept()
            return None;
        }
        // TS: :463-465
        #[allow(clippy::expect_used)] // rust-invariant: < 2^32 requests per game
        let idx = ReqIdx(u32::try_from(self.st.alliance_reqs.len()).expect("request index fits u32"));
        self.st.alliance_reqs.push(AllianceRequest::new(requestor, recipient, self.st.ticks));
        self.st.alliance_requests.push(idx);
        Some(idx)
    }
    /// `request.accept()` (`AllianceRequestImpl.ts:45-48`: `status_ = "accepted"` first) →
    /// `GameImpl.acceptAllianceRequest` (`GameImpl.ts:468-505`). Returns the new alliance's id
    /// (TS returns nothing; a throw leaves the request filtered out and accepted, as in TS).
    pub fn accept_alliance_request(&mut self, r: ReqIdx) -> AllianceId {
        self.st.alliance_reqs[r.0 as usize].status = RequestStatus::Accepted; // in-range: ReqIdx
        // TS: :469-471 (filter reassignment, ARCHITECTURE §4.7)
        self.st.alliance_requests.filter_reassign(|&ar| ar != r);
        let req = self.st.alliance_request(r);
        let (requestor, recipient) = (req.requestor, req.recipient);
        // TS: :476-481
        if self.st.player(requestor).alliance_with(recipient).is_some() {
            let name = self.st.player(recipient).name().to_owned();
            ts_throw!("cannot accept alliance request, already allied with {name}");
        }
        // TS: :484-490 `new AllianceImpl(this, requestor, recipient, this._ticks, this.nextAllianceID++)`
        let id = AllianceId(self.st.next_alliance_id);
        self.st.next_alliance_id += 1;
        let created_at = self.st.ticks;
        let expires_at = created_at + self.st.config().alliance_duration(); // AllianceImpl.ts:28
        debug_assert_eq!(self.st.alliances.len(), id.0 as usize, "alliance ids are indexes");
        self.st.alliances.push(Alliance::new(id, requestor, recipient, created_at, expires_at));
        // TS: :491-492
        self.st.player_data_mut(requestor).alliances.push(id);
        self.st.player_data_mut(recipient).alliances.push(id);
        // TS: :493-494
        self.st.stats_cx().alliance_formed(requestor);
        self.st.stats_cx().alliance_formed(recipient);
        // TS: :495-497
        self.st.player_data_mut(requestor).past_outgoing_alliance_requests.push(r);
        id
    }
    /// `request.reject()` (`AllianceRequestImpl.ts:50-53`) → `GameImpl.rejectAllianceRequest`
    /// (`GameImpl.ts:507-518`).
    pub fn reject_alliance_request(&mut self, r: ReqIdx) {
        self.st.alliance_reqs[r.0 as usize].status = RequestStatus::Rejected; // in-range: ReqIdx
        self.st.alliance_requests.filter_reassign(|&ar| ar != r);
        let requestor = self.st.alliance_request(r).requestor;
        self.st.player_data_mut(requestor).past_outgoing_alliance_requests.push(r);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::api::types::{PlayerInfo, PlayerType};
    use crate::exec::ExecStore;
    use crate::scratch::Scratch;
    use crate::testkit::{raw_player, state_with_map};

    fn two_players() -> (State, SmallId, SmallId) {
        let mut st = state_with_map(2, 2);
        let a = raw_player(&mut st, PlayerInfo::new("a", PlayerType::Human, None, "a"), None);
        let b = raw_player(&mut st, PlayerInfo::new("b", PlayerType::Human, None, "b"), None);
        (st, a, b)
    }

    /// Push a pending request the way `createAllianceRequest` does after its checks.
    fn push_req(st: &mut State, from: SmallId, to: SmallId) -> ReqIdx {
        let r = ReqIdx(st.alliance_reqs.len() as u32);
        st.alliance_reqs.push(AllianceRequest::new(from, to, st.ticks));
        st.alliance_requests.push(r);
        r
    }

    #[test]
    fn reject_filters_list_and_records_past_request() {
        let (mut st, a, b) = two_players();
        st.ticks = 5;
        let r0 = push_req(&mut st, a, b);
        let r1 = push_req(&mut st, b, a);
        let r2 = push_req(&mut st, a, b);
        let gen0 = st.alliance_requests.generation();
        let mut sc = Scratch::new();
        let mut cx = Ctx::new(&mut st, &mut sc, None);
        cx.reject_alliance_request(r1);
        assert_eq!(cx.st.alliance_requests(), [r0, r2]);
        // `this.allianceRequests = this.allianceRequests.filter(..)`: a new array object.
        assert_ne!(cx.st.alliance_requests.generation(), gen0);
        assert_eq!(cx.st.alliance_request(r1).status(), RequestStatus::Rejected);
        assert_eq!(cx.st.player_data(b).past_outgoing_alliance_requests, [r1]);
        assert!(cx.st.player_data(a).past_outgoing_alliance_requests.is_empty());
        // The request object stays readable after removal (pastOutgoingAllianceRequests).
        assert_eq!(cx.st.alliance_request(r1).created_at(), 5);
    }

    #[test]
    fn request_canon_row_and_key() {
        let (mut st, a, b) = two_players();
        st.ticks = 42;
        let r = push_req(&mut st, b, a);
        let ex = ExecStore::default();
        let mut cx = CanonCx::new(&st, &ex);
        assert_eq!(r.canon(&mut cx), Cs::Str("2:1:42".into()));
        assert_eq!(
            st.alliance_request(r).canon(&mut cx),
            Cs::Obj(vec![
                ("key".into(), Cs::Str("2:1:42".into())),
                ("requestor".into(), Cs::Int(2)),
                ("recipient".into(), Cs::Int(1)),
                ("createdAt".into(), Cs::Int(42)),
                ("status".into(), Cs::Str("pending".into())),
            ])
        );
    }
}
