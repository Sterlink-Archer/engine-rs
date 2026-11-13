//! Unit-test fixtures that build `State` without the S1 constructors of other packages
//! (`Player::new` is P2's, `GameMap::new`/`MapStatics::load` are P3's). Owner: **P1**
//! (FILE_MAP §5 rule 8: shared test fixtures belong to P1/P13).
//!
//! [`raw_player`] writes the field initialisers of `PlayerImpl` (`PlayerImpl.ts:135-220`)
//! directly, with gold/troops passed in, so P1's state methods can be tested on data alone.
//! It never stands in for P2's constructor in a conformance path.
#![cfg(test)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use crate::api::types::{PlayerInfo, Team};
use crate::ids::{PlayerInfoIdx, SmallId};
use crate::jsarr::JsArr;
use crate::jsval::Tri;
use crate::map::MapStatics;
use crate::state::State;
use crate::state::player::Player;
use std::sync::Arc;

/// A `State` whose statics describe a `w × h` all-land map (land index = tile index).
pub(crate) fn state_with_map(w: u32, h: u32) -> State {
    let n = (w * h) as usize;
    let statics = MapStatics {
        width: w,
        height: h,
        num_land_tiles: w * h,
        land_index: (0..n as u32).collect::<Vec<_>>().into(),
        ..Default::default()
    };
    let mut st = State::default();
    st.statics = Arc::new(statics);
    // `GameImpl` field initialisers (`GameImpl.ts:148-149`); `State::default()` has zeros.
    st.next_player_id = 1;
    st.next_unit_id = 1;
    st.test_reset_tile_order(n);
    st
}

/// Append a player with `info` (smallID = `nextPlayerID++`), like `addPlayer` minus the
/// config reads; `PlayerImpl` field initialisers otherwise.
pub(crate) fn raw_player(st: &mut State, info: PlayerInfo, team: Option<Team>) -> SmallId {
    let small_id = SmallId(u16::try_from(st.next_player_id).unwrap());
    let idx = PlayerInfoIdx(st.infos.len() as u32);
    let p = Player {
        small_id,
        info: idx,
        team,
        last_tile_change: 0,
        tile_change_version: 0,
        rng: of_det::Prng::new(of_det::util::simple_hash(&info.id)),
        gold: 0,
        troops: 0,
        trade_gold: 0,
        train_gold: 0,
        piracy_gold: 0,
        gold_earned: 0,
        marked_traitor_tick: -1,
        marked_doomsday_clock_tick: -1,
        rotted_at_tick: -1,
        betrayal_count: 0,
        embargoes: Default::default(),
        tiles: Default::default(),
        border_tiles: Default::default(),
        units: JsArr::new(),
        my_units_version: 0,
        past_outgoing_alliance_requests: Vec::new(),
        expired_alliances: Vec::new(),
        targets: Vec::new(),
        outgoing_emojis: Vec::new(),
        outgoing_quick_chats: Default::default(),
        sent_donations: Vec::new(),
        relations: Default::default(),
        last_delete_unit_tick: -1,
        last_embargo_all_tick: -1,
        incoming_attacks: JsArr::new(),
        outgoing_attacks: JsArr::new(),
        outgoing_land_attacks: Vec::new(),
        alliances: JsArr::new(),
        spawn_tile: None,
        is_disconnected: false,
        disconnect_snapshot: None,
        largest_cluster_bbox: Tri::Absent,
        num_units_constructed: Vec::new(),
        my_units_memo: Default::default(),
        my_unit_count_memo: Default::default(),
        my_units_owned_memo: Default::default(),
        nearby_memo: None,
        reachable_components_memo: None,
        conn4: false,
        comps: Default::default(),
    };
    Arc::make_mut(&mut st.infos).push(info);
    st.players.push(p);
    st.next_player_id += 1;
    small_id
}

/// Give player `p` tile `t` in its `_tiles` set (data only: no map write).
pub(crate) fn give_tile(st: &mut State, p: SmallId, t: u32) {
    let (pos, hdr, li) = st.tiles_parts_mut(p);
    pos.add(hdr, li, t);
}
