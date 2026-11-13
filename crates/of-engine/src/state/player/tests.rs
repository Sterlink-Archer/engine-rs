//! Tests for `state::player`. Owner: **P2**.
//!
//! T0/T2 tests that run on P2's code alone (no map, no config, no `GameImpl` bodies):
//! - `tests/PlayerImpl.test.ts` "units() type filtering" (units placed straight in the arena);
//! - the exact unit memos and the `verify-memos` guard (G11);
//! - gold/troops arithmetic and strictness, relations, embargoes, emojis, quick chats,
//!   donations, teams, `numUnitsConstructed` order, the constructor, the CS row.
//!
//! The vitest cases that need `buildUnit`/`conquer`/a map (`PlayerImpl.test.ts` upgrade and
//! spawn cases, `UnitMemoInvalidation.test.ts`) need P1's `Setup.ts` port and P3's config and
//! map; they are S2 integration work (ledger task in the P2 report).

// Test code: a failed `expect` is the assertion, and catching panics is how a TS throw is
// observed (the lint allows these only in path/stepper.rs and runner.rs for the engine itself).
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::disallowed_methods)]

use super::*;
use crate::api::types::{PlayerInfo, PlayerType};
use crate::exec::ExecStore;
use crate::state::alliance::Alliance;
use crate::state::stats::tests::state_with;
use crate::state::unit::Unit;
use crate::tileset::PlayerTileOrder;

fn caught_ts_throw(f: impl FnOnce()) -> Option<String> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(()) => None,
        Err(p) => match of_det::throw::as_ts_throw(&*p) {
            Some(t) => Some(format!("{t:?}")),
            None => std::panic::resume_unwind(p),
        },
    }
}

fn three() -> State {
    state_with(vec![
        PlayerInfo::new("Alice", PlayerType::Human, Some("c1".into()), "pid_a"),
        PlayerInfo::new("Bob", PlayerType::Human, Some("c2".into()), "pid_b"),
        PlayerInfo::new("Nat", PlayerType::Nation, None, "pid_n"),
    ])
}
const A: SmallId = SmallId(1);
const B: SmallId = SmallId(2);
const N: SmallId = SmallId(3);

/// Give `p` `n` fake owned tiles (only `_tiles.size` matters to `isAlive`).
fn give_tiles(st: &mut State, p: SmallId, n: u32) {
    // A private position index sized for the fake land indexes (the State's own index is
    // empty in a map-less test state).
    let mut ord = PlayerTileOrder::new(n as usize + 1);
    let land: Vec<u32> = (0..=n).collect();
    let hdr = &mut st.player_data_mut(p).tiles;
    for t in 0..n {
        ord.add(hdr, &land, t);
    }
}

fn add_unit(st: &mut State, owner: SmallId, t: UnitType, level: i32) -> UnitRef {
    let id = st.next_unit_id;
    st.next_unit_id += 1;
    let r = st.units.alloc(Unit { id, unit_type: t, owner, active: true, level, targetable: true, ..Unit::default() });
    let p = st.player_data_mut(owner);
    p.units.push(r);
    p.my_units_version += 1;
    r
}

#[test]
fn constructor_matches_ts_field_initialisers() {
    let st = three();
    let p = st.player_data(A);
    assert_eq!(p.rng, Prng::new(simple_hash("pid_a")));
    assert_eq!((p.marked_traitor_tick, p.marked_doomsday_clock_tick, p.rotted_at_tick), (-1, -1, -1));
    assert_eq!((p.last_delete_unit_tick, p.last_embargo_all_tick), (-1, -1));
    assert_eq!(p.largest_cluster_bbox, Tri::Absent);
    // `toInt(startTroops)`: floors, clamps ±Inf, throws on NaN.
    let info = PlayerInfo::new("x", PlayerType::Bot, None, "x");
    assert_eq!(Player::new(A, PlayerInfoIdx(0), &info, 2.9, 0, None).troops, 2);
    assert_eq!(Player::new(A, PlayerInfoIdx(0), &info, f64::INFINITY, 0, None).troops, 9_007_199_254_740_991);
    assert!(
        caught_ts_throw(|| {
            let _ = Player::new(A, PlayerInfoIdx(0), &info, f64::NAN, 0, None);
        })
        .is_some()
    );
}

// ---- `PlayerImpl.test.ts` "units() type filtering" ----

fn mixed_units(st: &mut State) -> Vec<UnitRef> {
    vec![
        add_unit(st, A, UnitType::City, 1),
        add_unit(st, A, UnitType::MissileSilo, 1),
        add_unit(st, A, UnitType::City, 1),
        add_unit(st, A, UnitType::DefensePost, 1),
    ]
}

#[test]
fn units_single_type_in_insertion_order_and_fresh_copies() {
    let mut st = three();
    let u = mixed_units(&mut st);
    let p = st.player(A);
    assert_eq!(p.units_of(&[UnitType::City]), vec![u[0], u[2]]);
    let mut a = p.units_of(&[UnitType::City]);
    a.clear(); // the caller's copy
    assert_eq!(p.units_of(&[UnitType::City]).len(), 2);
    assert_eq!(p.units().as_slice(), u.as_slice()); // units() is the live array
}

#[test]
fn units_two_three_and_array_forms_are_unions_in_insertion_order() {
    let mut st = three();
    let u = mixed_units(&mut st);
    let p = st.player(A);
    assert_eq!(p.units_of(&[UnitType::City, UnitType::MissileSilo]), vec![u[0], u[1], u[2]]);
    assert_eq!(p.units_of(&[UnitType::City, UnitType::DefensePost, UnitType::MissileSilo]), u);
    assert_eq!(p.units_of(&[UnitType::City, UnitType::City]), vec![u[0], u[2]]);
    assert_eq!(p.units_of(&[UnitType::DefensePost, UnitType::City, UnitType::MissileSilo, UnitType::City]), u);
    assert!(p.units_of(&[UnitType::Port]).is_empty());
}

// ---- exact memos (ARCHITECTURE §3.3, G11) ----

#[test]
fn unit_counts_are_level_weighted_and_construction_counts_once() {
    let mut st = three();
    let c1 = add_unit(&mut st, A, UnitType::City, 3);
    let _c2 = add_unit(&mut st, A, UnitType::City, 1);
    st.unit_data_mut(c1).under_construction = true;
    let p = st.player(A);
    assert_eq!(p.unit_count(UnitType::City), 4);
    assert_eq!(p.units_owned(UnitType::City), 2); // 1 (under construction) + level 1
    assert_eq!(p.unit_count(UnitType::Port), 0);
}

#[test]
fn filled_memos_answer_like_a_recompute_until_the_version_moves() {
    let mut st = three();
    let mut sc = Scratch::default();
    let c = add_unit(&mut st, A, UnitType::City, 2);
    add_unit(&mut st, A, UnitType::Port, 1);
    {
        let mut cx = Ctx::new(&mut st, &mut sc, None);
        cx.player_mut(A).fill_unit_memos();
    }
    let v = st.player_data(A).my_units_version;
    assert!(st.player_data(A).my_units_memo.hdr[UnitType::City.index()].valid);
    assert_eq!(st.player_data(A).my_unit_count_memo[UnitType::City.index()].version, Some(v));
    // Hits (checked against a recompute under `verify-memos`).
    assert_eq!(st.player(A).units_of(&[UnitType::City]), vec![c]);
    assert_eq!(st.player(A).unit_count(UnitType::City), 2);
    assert_eq!(st.player(A).units_owned(UnitType::Port), 1);
    // Every `_units` change bumps the version, so the memo is bypassed afterwards.
    let c2 = add_unit(&mut st, A, UnitType::City, 1);
    assert_eq!(st.player(A).units_of(&[UnitType::City]), vec![c, c2]);
    assert_eq!(st.player(A).unit_count(UnitType::City), 3);
}

/// `verify-memos` turns a stale hit (a `_units` change without a version bump — which TS
/// never does) into a loud failure instead of a silent divergence.
#[cfg(feature = "verify-memos")]
#[test]
fn verify_memos_catches_a_missing_version_bump() {
    let mut st = three();
    let mut sc = Scratch::default();
    add_unit(&mut st, A, UnitType::City, 1);
    {
        let mut cx = Ctx::new(&mut st, &mut sc, None);
        cx.player_mut(A).fill_unit_memos();
    }
    let extra =
        st.units.alloc(Unit { id: 99, unit_type: UnitType::City, owner: A, active: true, level: 1, ..Unit::default() });
    st.player_data_mut(A).units.push(extra); // no `_myUnitsVersion++`
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| st.player(A).units_of(&[UnitType::City])));
    assert!(r.is_err(), "verify-memos must reject the stale memo");
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| st.player(A).unit_count(UnitType::City)));
    assert!(r.is_err());
}

#[test]
fn units_constructed_keeps_first_construction_order() {
    let mut st = three();
    let mut sc = Scratch::default();
    let mut cx = Ctx::new(&mut st, &mut sc, None);
    let mut p = cx.player_mut(A);
    p.record_unit_constructed(UnitType::Port);
    p.record_unit_constructed(UnitType::City);
    p.record_unit_constructed(UnitType::Port);
    assert_eq!(p.r().data().num_units_constructed, vec![(UnitType::Port, 2), (UnitType::City, 1)]);
    assert_eq!((p.r().units_constructed(UnitType::Port), p.r().units_constructed(UnitType::Warship)), (2, 0));
}

// ---- resources ----

#[test]
fn gold_and_troops_follow_bigint_semantics() {
    let mut st = three();
    let mut sc = Scratch::default();
    let mut cx = Ctx::new(&mut st, &mut sc, None);
    let mut p = cx.player_mut(A);
    p.add_gold(100, Some(3));
    assert_eq!((p.r().gold(), p.r().gold_earned()), (100, 100));
    assert_eq!(p.remove_gold(0), 0);
    assert_eq!(p.remove_gold(-5), 0);
    assert_eq!(p.remove_gold(250), 100); // minInt(gold, toRemove)
    assert_eq!(p.r().gold(), 0);
    p.set_troops(10.9);
    assert_eq!(p.r().troops(), 10.0);
    p.add_troops(2.5);
    assert_eq!(p.r().troops(), 12.0);
    p.add_troops(-3.5); // → removeTroops(3.5) → toInt(3.5) = 3
    assert_eq!(p.r().troops(), 9.0);
    assert_eq!(p.remove_troops(100.0), 9.0);
    assert_eq!(p.remove_troops(-1.0), 0.0);
    assert!(caught_ts_throw(|| p.set_troops(f64::NAN)).is_some());
    // NaN is not `<= 0`, so it reaches `toInt(NaN)` and throws.
    assert!(
        caught_ts_throw(|| {
            p.remove_troops(f64::NAN);
        })
        .is_some()
    );
    p.add_trade_gold(3);
    p.add_train_gold(4);
    p.add_piracy_gold(5);
    let d = p.r().data();
    assert_eq!((d.trade_gold, d.train_gold, d.piracy_gold), (3, 4, 5));
}

#[test]
fn donations_move_resources_and_record_stats() {
    let mut st = three();
    let mut sc = Scratch::default();
    st.ticks = 7;
    st.player_data_mut(A).gold = 1000;
    st.player_data_mut(A).troops = 50;
    let mut cx = Ctx::new(&mut st, &mut sc, None);
    assert!(!cx.player_mut(A).donate_gold(A, 10));
    assert!(!cx.player_mut(A).donate_gold(B, 0));
    assert!(cx.player_mut(A).donate_gold(B, 300));
    assert!(cx.player_mut(A).donate_troops(B, 20.7)); // toInt → 20
    assert!(!cx.player_mut(A).donate_troops(B, -1.0));
    let (a, b) = (cx.st.player(A), cx.st.player(B));
    assert_eq!((a.gold(), b.gold(), b.gold_earned()), (700, 300, 300));
    assert_eq!((a.troops(), b.troops()), (30.0, 20.0));
    assert_eq!(a.data().sent_donations, vec![Donation { recipient: B, tick: 7 }, Donation { recipient: B, tick: 7 }]);
    // `goldDonationReceived` created the recipient's stats record.
    assert!(cx.st.stats.get_player_stats(Some("c2")).is_some());
    // Nothing left to give: removed === 0n → false, no donation recorded.
    cx.st.player_data_mut(A).gold = 0;
    assert!(!cx.player_mut(A).donate_gold(B, 5));
    assert_eq!(cx.st.player(A).data().sent_donations.len(), 2);
}

// ---- relations ----

#[test]
fn relations_clamp_decay_and_sort() {
    let mut st = three();
    let mut sc = Scratch::default();
    give_tiles(&mut st, B, 1);
    give_tiles(&mut st, N, 1);
    let mut cx = Ctx::new(&mut st, &mut sc, None);
    let mut p = cx.player_mut(A);
    p.update_relation(N, 30.0);
    p.update_relation(B, -250.0); // within(-250, -100, 100)
    assert_eq!(p.r().data().relations.get(&B), Some(&-100.0));
    assert_eq!(p.r().relation(B), Relation::Hostile);
    assert_eq!(p.r().relation(N), Relation::Neutral);
    assert_eq!(p.r().relation(SmallId(9)), Relation::Neutral); // `?? 0`
    // Insertion order (N, B) is kept; the sort is by raw value.
    assert_eq!(p.r().all_relations_sorted(), vec![(B, Relation::Hostile), (N, Relation::Neutral)]);
    p.decay_relations();
    assert_eq!(p.r().data().relations.get(&N), Some(&(30.0 - 0.05)));
    assert_eq!(p.r().data().relations.get(&B), Some(&(-100.0 + 0.05)));
    p.update_relation(N, -29.88); // ≈0.07 → |0.02 after decay| < 0.1 → 0
    p.decay_relations();
    assert_eq!(p.r().data().relations.get(&N), Some(&0.0));
    let keys: Vec<SmallId> = p.r().data().relations.keys().copied().collect();
    assert_eq!(keys, vec![N, B]);
    assert!(
        caught_ts_throw(|| p.update_relation(A, 1.0)).expect("throws").contains("cannot update relation with self")
    );
    assert!(
        caught_ts_throw(|| {
            let _ = p.r().relation(A);
        })
        .expect("throws")
        .contains(
            "cannot get relation with self: Player:{name:Alice,clientID:c1,isAlive:false,troops:0,numTileOwned:0}]"
        )
    );
}

#[test]
fn dead_players_are_dropped_from_sorted_relations() {
    let mut st = three();
    let mut sc = Scratch::default();
    give_tiles(&mut st, N, 2);
    let mut cx = Ctx::new(&mut st, &mut sc, None);
    cx.player_mut(A).update_relation(B, 60.0); // B owns no tiles: dead
    cx.player_mut(A).update_relation(N, 60.0);
    assert_eq!(cx.st.player(A).all_relations_sorted(), vec![(N, Relation::Friendly)]);
    assert_eq!(cx.st.player(A).player_profile().relations, vec![(N, Relation::Friendly)]);
}

// ---- embargoes ----

#[test]
fn embargo_rules() {
    let mut st = three();
    let mut sc = Scratch::default();
    st.ticks = 5;
    let mut cx = Ctx::new(&mut st, &mut sc, None);
    let mut p = cx.player_mut(A);
    p.add_embargo(B, true);
    assert!(p.r().has_embargo_against(B) && !p.r().has_manual_embargo_against(B));
    p.cx.st.ticks = 9;
    p.add_embargo(B, false); // a temporary embargo is replaced in place
    assert_eq!(p.r().get_embargoes(), vec![Embargo { target: B, created_at: 9, is_temporary: false }]);
    p.add_embargo(B, true); // a manual one is never overwritten
    assert!(p.r().has_manual_embargo_against(B));
    p.end_temporary_embargo(B); // manual: kept
    assert!(p.r().has_embargo_against(B));
    assert!(!p.r().can_trade(B));
    assert!(!cx.st.player(B).can_trade(A)); // either side's embargo blocks trade
    cx.player_mut(A).stop_embargo(B);
    assert!(cx.st.player(A).can_trade(B));
    assert!(!cx.st.player(A).can_trade(A)); // same id
}

// ---- teams and alliances ----

#[test]
fn teams_friendliness_and_alliances() {
    let mut st = three();
    st.player_data_mut(A).team = Some("Red".into());
    st.player_data_mut(B).team = Some("Red".into());
    st.player_data_mut(N).team = Some(colored_teams::BOT.into());
    let pa = st.player(A);
    assert!(pa.is_on_same_team(B) && !pa.is_on_same_team(A) && !pa.is_on_same_team(N));
    assert!(pa.is_friendly(A, false) && pa.is_friendly(B, false));
    st.player_data_mut(B).is_disconnected = true;
    assert!(!st.player(A).is_friendly(B, false) && st.player(A).is_friendly(B, true));
    // An alliance with N (pushed as data: P1 owns the alliance constructors).
    let id = AllianceId(0);
    st.alliances.push(Alliance {
        id,
        requestor: A,
        recipient: N,
        created_at: 0,
        expires_at: 100,
        extension_requested_requestor: false,
        extension_requested_recipient: false,
    });
    st.player_data_mut(A).alliances.push(id);
    st.player_data_mut(N).alliances.push(id);
    assert_eq!(st.player(A).alliance_with(N), Some(id));
    assert!(st.player(N).is_allied_with(A) && !st.player(A).is_allied_with(A) && !st.player(A).is_allied_with(B));
    assert!(st.player(A).is_friendly(N, false));
}

// ---- communication ----

#[test]
fn emojis_and_quick_chats() {
    let mut st = three();
    let mut sc = Scratch::default();
    st.ticks = 4;
    let mut cx = Ctx::new(&mut st, &mut sc, None);
    let mut p = cx.player_mut(A);
    p.send_emoji(EmojiRecipient::AllPlayers, "👍");
    p.send_emoji(EmojiRecipient::Player(B), "x");
    assert!(
        caught_ts_throw(|| p.send_emoji(EmojiRecipient::Player(A), "x"))
            .expect("throws")
            .contains("Cannot send emoji to oneself")
    );
    let e = &p.r().data().outgoing_emojis;
    assert_eq!(e.len(), 2);
    assert_eq!((e[0].recipient_id, e[0].created_at, e[1].sender_id), (EmojiRecipient::AllPlayers, 4, A));
    assert!(!p.r().can_send_emoji(EmojiRecipient::Player(A)));
    assert!(!p.r().can_send_quick_chat(A));
    assert!(p.r().can_send_quick_chat(B)); // never sent
    p.record_quick_chat(B);
    assert_eq!(p.r().data().outgoing_quick_chats.get(&B), Some(&4));
}

// ---- small state ----

#[test]
fn doomsday_traitor_and_disconnect_fields() {
    let mut st = three();
    let mut sc = Scratch::default();
    give_tiles(&mut st, A, 3);
    st.ticks = 100;
    let mut cx = Ctx::new(&mut st, &mut sc, None);
    let mut p = cx.player_mut(A);
    assert_eq!(p.r().get_traitor_remaining_ticks(), 0); // never marked: no config read
    p.enter_doomsday_clock();
    p.cx.st.ticks = 120;
    p.enter_doomsday_clock(); // keeps the first mark
    assert!(p.r().in_doomsday_clock());
    assert_eq!(p.r().doomsday_clock_ticks(), 20);
    assert!(!p.r().is_decaying());
    p.mark_rotted();
    p.cx.st.ticks = 150;
    assert!(p.r().is_decaying()); // 150 - 120 <= 30
    p.cx.st.ticks = 151;
    assert!(!p.r().is_decaying());
    p.clear_doomsday_clock();
    assert!(!p.r().in_doomsday_clock() && p.r().data().rotted_at_tick == -1);
    let snap = DisconnectSnapshot { current_tick: 3, team_tiles: 4, total_land: 5, was_alive: true };
    p.mark_disconnected(true, Some(snap.clone()));
    p.mark_disconnected(true, Some(DisconnectSnapshot::default())); // first snapshot kept
    assert_eq!(p.r().disconnect_snapshot(), Some(&snap));
    assert_eq!(p.r().disconnected_at_tick(), Some(3));
    p.mark_disconnected(false, None);
    assert_eq!(p.r().disconnect_snapshot(), None);
    p.set_largest_cluster_bounding_box(None);
    assert_eq!(p.r().data().largest_cluster_bbox, Tri::Null); // null ≠ absent
    p.set_spawn_tile(77);
    assert!(p.r().has_spawned());
    p.record_delete_unit();
    p.record_embargo_all();
    assert_eq!((p.r().data().last_delete_unit_tick, p.r().data().last_embargo_all_tick), (151, 151));
}

#[test]
fn mark_traitor_counts_betrayals_and_records_stats() {
    let mut st = three();
    let mut sc = Scratch::default();
    st.ticks = 12;
    let mut cx = Ctx::new(&mut st, &mut sc, None);
    cx.player_mut(A).mark_traitor();
    cx.player_mut(N).mark_traitor(); // nations count too, but have no stats record
    assert_eq!((cx.st.player(A).betrayals(), cx.st.player(N).betrayals()), (1, 1));
    assert_eq!(cx.st.player(A).data().marked_traitor_tick, 12);
    assert!(cx.st.stats.get_player_stats(Some("c1")).is_some());
    assert_eq!(cx.st.stats.lite.len(), 1);
}

#[test]
fn targets_filter_and_transitive_dedupe() {
    let mut st = three();
    // `target()` calls GameImpl.target (P1, view-only): fill `targets_` directly.
    st.player_data_mut(A).targets.push(Target { tick: 0, target: B });
    st.player_data_mut(A).targets.push(Target { tick: 0, target: B });
    // targets() needs `targetDuration()` (P3); transitive dedupe is checked on the raw list.
    let mut set = OwnerSet::new();
    for t in st.player_data(A).targets.iter().map(|t| t.target) {
        set.add(t);
    }
    set.add(N);
    set.add(B);
    assert_eq!(set.order, vec![B, N]);
}

// ---- CS row ----

#[test]
fn canon_row_follows_the_dump_order() {
    let mut st = three();
    st.player_data_mut(A).relations.insert(B, 2.5);
    st.player_data_mut(A).outgoing_quick_chats.insert(B, 6);
    let ex = ExecStore::default();
    let mut cx = CanonCx::new(&st, &ex);
    let Cs::Obj(fields) = st.player_data(A).canon(&mut cx) else { panic!("object") };
    let keys: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        keys,
        [
            "smallID",
            "info",
            "team",
            "lastTileChange",
            "tileChangeVersion",
            "random",
            "gold",
            "troops",
            "tradeGold",
            "trainGold",
            "piracyGold",
            "goldEarned",
            "markedTraitorTick",
            "markedDoomsdayClockTick",
            "rottedAtTick",
            "betrayalCount",
            "embargoes",
            "tiles",
            "borderTiles",
            "units",
            "unitsVersion",
            "pastOutgoingAllianceRequests",
            "expiredAlliances",
            "targets",
            "outgoingEmojis",
            "outgoingQuickChats",
            "sentDonations",
            "relations",
            "lastDeleteUnitTick",
            "lastEmbargoAllTick",
            "incomingAttacks",
            "outgoingAttacks",
            "outgoingLandAttacks",
            "alliances",
            "spawnTile",
            "isDisconnected",
            "disconnectSnapshot",
            "largestClusterBoundingBox",
            "numUnitsConstructed",
        ]
    );
    assert_eq!(fields[37].1, Cs::Absent); // never assigned
    assert_eq!(fields[27].1, Cs::Arr(vec![Cs::Arr(vec![Cs::Int(2), Cs::Num(2.5)])]));
    assert_eq!(fields[25].1, Cs::Arr(vec![Cs::Arr(vec![Cs::Int(2), Cs::Int(6)])]));
    let Cs::Obj(info) = &fields[1].1 else { panic!("info") };
    let ik: Vec<&str> = info.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        ik,
        ["name", "playerType", "clientID", "id", "isLobbyCreator", "clanTag", "friends", "teamIndex", "nationFlag"]
    );
    assert_eq!(info[1].1, Cs::Str("HUMAN".into()));
}
