//! Tests for `state::stats`. Owner: **P2**.
//!
//! - T0: the assertions of `tests/Stats.test.ts` and `tests/StatsTiles.test.ts` that call
//!   `StatsImpl` directly (`stats-full`), plus the FILE_MAP §4 mandatory design tests (lite
//!   build and full build): the floor asymmetry of send + cancel, and the clientID-gated
//!   strict `_bigint` conversion.
//! - Rendering: [`dump`] prints `stats()` as JSON with sorted keys and bigints as `<n>n`, so
//!   expected values read like the vitest `toStrictEqual` literals (key order is not
//!   observable: CSE1 sorts object keys).

// Test code: a failed `expect` is the assertion, and catching panics is how a TS throw is
// observed (the lint allows these only in path/stepper.rs and runner.rs for the engine itself).
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::disallowed_methods)]

use super::*;
use crate::api::types::PlayerInfo;
use crate::ids::PlayerInfoIdx;
use std::sync::Arc;

/// A `State` whose players are `infos` in smallID order (no map, no config calls).
pub(crate) fn state_with(infos: Vec<PlayerInfo>) -> State {
    let mut st = State::default();
    for (i, info) in infos.into_iter().enumerate() {
        let sid = SmallId::from_player_index(i);
        let idx = PlayerInfoIdx(u32::try_from(i).expect("small test"));
        let p = Player::new(sid, idx, &info, 0.0, 0, None);
        Arc::make_mut(&mut st.infos).push(info);
        st.players.push(p);
    }
    st
}

/// `setup(.., [new PlayerInfo("boat dude", Human, "client1", "player_1_id"), .. "client2" ..])`
/// plus a nation (no clientID) as player 3.
fn two_clients() -> (State, SmallId, SmallId, SmallId) {
    let st = state_with(vec![
        PlayerInfo::new("boat dude", PlayerType::Human, Some("client1".into()), "player_1_id"),
        PlayerInfo::new("boat dude", PlayerType::Human, Some("client2".into()), "player_2_id"),
        PlayerInfo::new("nation", PlayerType::Nation, None, "nation_id"),
    ]);
    (st, SmallId(1), SmallId(2), SmallId(3))
}

fn caught_ts_throw(f: impl FnOnce()) -> Option<String> {
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    match r {
        Ok(()) => None,
        Err(p) => match of_det::throw::as_ts_throw(&*p) {
            Some(t) => Some(format!("{t:?}")),
            None => std::panic::resume_unwind(p),
        },
    }
}

fn lite_sent(st: &State, cid: &str) -> Option<i64> {
    st.stats.get_player_stats(Some(cid)).map(|p| p.attacks_sent)
}

// ---- mandatory design tests (both builds) ----

#[test]
fn send_then_cancel_with_fractional_troops_leaves_minus_one() {
    // ARCHITECTURE §3.10: floor(2.5) + floor(-2.5) = 2 - 3 = -1, and conquerPlayer tests `=== 0n`.
    let (mut st, p1, p2, _) = two_clients();
    st.stats_cx().attack(p1, p2, StatNum::Num(2.5));
    assert_eq!(st.stats.attacks_sent(Some("client1")), 2);
    st.stats_cx().attack_cancel(p1, p2, StatNum::Num(2.5));
    assert_eq!(st.stats.attacks_sent(Some("client1")), -1);
    // An integral send + cancel nets to zero.
    let (mut st, p1, p2, _) = two_clients();
    st.stats_cx().attack(p1, p2, StatNum::Num(7.0));
    st.stats_cx().attack_cancel(p1, p2, StatNum::Num(7.0));
    assert_eq!(st.stats.attacks_sent(Some("client1")), 0);
    // A bigint argument negates as a bigint.
    let (mut st, p1, p2, _) = two_clients();
    st.stats_cx().attack(p1, p2, StatNum::Big(5));
    st.stats_cx().attack_cancel(p1, p2, StatNum::Big(5));
    assert_eq!(st.stats.attacks_sent(Some("client1")), 0);
}

#[test]
fn nan_stat_throws_only_for_a_client_player() {
    let (mut st, p1, _p2, nation) = two_clients();
    // Non-clientID player: `_makePlayerStats` returns undefined before `_bigint` runs.
    assert_eq!(caught_ts_throw(|| st.stats_cx().gold_work(nation, StatNum::Num(f64::NAN))), None);
    assert_eq!(caught_ts_throw(|| st.stats_cx().attack(nation, SmallId(0), StatNum::Num(f64::INFINITY))), None);
    assert!(st.stats.lite.is_empty());
    // ClientID player: `BigInt(Math.floor(NaN))` throws (RangeError), in the lite build too.
    let msg = caught_ts_throw(|| st.stats_cx().gold_work(p1, StatNum::Num(f64::NAN)));
    assert!(msg.is_some(), "NaN gold for a clientID player must throw");
    let msg = caught_ts_throw(|| st.stats_cx().attack(p1, SmallId(0), StatNum::Num(f64::NEG_INFINITY)));
    assert!(msg.is_some(), "-Inf troops for a clientID player must throw");
    // The record was created before the conversion threw (TS statement order).
    assert_eq!(lite_sent(&st, "client1"), Some(0));
}

#[test]
fn attack_on_a_non_client_target_converts_only_for_the_client_side() {
    let (mut st, p1, _p2, nation) = two_clients();
    // nation attacks client1 with NaN: the sender has no record, the target does → throws.
    assert!(caught_ts_throw(|| st.stats_cx().attack(nation, p1, StatNum::Num(f64::NAN))).is_some());
    // client1 attacks the nation: SENT converts (throws on NaN); the target side never runs.
    let (mut st, p1, _p2, nation) = two_clients();
    st.stats_cx().attack(p1, nation, StatNum::Num(3.0));
    assert_eq!(lite_sent(&st, "client1"), Some(3));
    assert!(st.stats.get_player_stats(None).is_none());
}

#[test]
fn gold_before_converts_outside_the_client_gate() {
    // `if (_bigint(goldBefore) < DONATION_BROKE_GOLD_THRESHOLD)` runs for every player.
    let (mut st, _p1, _p2, nation) = two_clients();
    assert!(
        caught_ts_throw(|| st.stats_cx().gold_donation_received(nation, StatNum::Big(1), StatNum::Num(f64::NAN)))
            .is_some()
    );
    let (mut st, _p1, _p2, nation) = two_clients();
    st.stats_cx().gold_donation_received(nation, StatNum::Big(1), StatNum::Big(0));
    assert!(st.stats.lite.is_empty());
}

#[test]
fn lite_records_are_created_by_every_entry_point_and_migrate() {
    let (mut st, p1, p2, _) = two_clients();
    st.stats_cx().betray(p2);
    assert_eq!(lite_sent(&st, "client2"), Some(0));
    assert_eq!(st.stats.attacks_sent(Some("nobody")), 0);
    assert_eq!(st.stats.attacks_sent(None), 0);
    st.stats_cx().attack(p1, p2, StatNum::Num(10.0));
    st.stats_cx().migrate_client_id("client1", "new_client1");
    assert_eq!(lite_sent(&st, "client1"), None);
    assert_eq!(lite_sent(&st, "new_client1"), Some(10));
    // Same id: no-op; unknown id: no-op.
    st.stats_cx().migrate_client_id("new_client1", "new_client1");
    st.stats_cx().migrate_client_id("ghost", "x");
    assert_eq!(lite_sent(&st, "new_client1"), Some(10));
    assert_eq!(lite_sent(&st, "x"), None);
}

// ---- stats-full: the vitest ports ----

#[cfg(feature = "stats-full")]
mod full {
    use super::*;
    use crate::canon::Cs;

    fn render(v: &Cs, out: &mut String) {
        match v {
            Cs::Absent => out.push_str("undefined"),
            Cs::Null => out.push_str("null"),
            Cs::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Cs::Int(i) => out.push_str(&i.to_string()),
            Cs::Num(x) => out.push_str(&of_det::throw::js_number_to_string(*x)),
            Cs::Big(b) => {
                out.push_str(&b.to_string());
                out.push('n');
            }
            Cs::Str(s) => {
                out.push('"');
                out.push_str(s);
                out.push('"');
            }
            Cs::Arr(a) => {
                out.push('[');
                for (i, x) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    render(x, out);
                }
                out.push(']');
            }
            Cs::Obj(o) => {
                let mut o: Vec<_> = o.iter().filter(|(_, v)| !matches!(v, Cs::Absent)).collect();
                o.sort_by(|a, b| a.0.cmp(&b.0)); // test-only rendering of a key-sorted object
                out.push('{');
                for (i, (k, x)) in o.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(k);
                    out.push(':');
                    render(x, out);
                }
                out.push('}');
            }
            Cs::Typed(_) => out.push_str("<typed>"),
        }
    }

    /// `stats.stats()` rendered with sorted keys.
    pub(super) fn dump(st: &State) -> String {
        let data = Cs::Obj(st.stats.full.data.iter().map(|(k, v)| (k.clone(), v.to_cs())).collect());
        let mut s = String::new();
        render(&data, &mut s);
        s
    }

    fn rec<'s>(st: &'s State, cid: &str) -> &'s PlayerStatsRec {
        st.stats.full_record(cid).expect("record")
    }

    macro_rules! case {
        ($name:ident, |$st:ident, $p1:ident, $p2:ident| $body:block, $want:expr) => {
            #[test]
            fn $name() {
                let (mut $st, $p1, $p2, _) = two_clients();
                $body
                assert_eq!(dump(&$st), $want);
            }
        };
    }

    // `tests/Stats.test.ts`
    case!(
        attack,
        |st, p1, p2| {
            st.stats_cx().attack(p1, p2, StatNum::Num(1.0));
        },
        "{client1:{attacks:[1n]},client2:{attacks:[0n,1n]}}"
    );
    case!(
        attack_cancel,
        |st, p1, p2| {
            st.stats_cx().attack_cancel(p1, p2, StatNum::Num(1.0));
        },
        "{client1:{attacks:[-1n,0n,1n]},client2:{attacks:[0n,-1n]}}"
    );
    case!(
        attack_max_incoming_ignores_non_player,
        |st, p1, p2| {
            let _ = (p1, p2);
            st.stats_cx().attack_max_incoming(SmallId(0), StatNum::Num(50.0));
        },
        "{}"
    );
    case!(
        betray,
        |st, p1, p2| {
            let _ = p2;
            st.stats_cx().betray(p1);
        },
        "{client1:{betrayals:1n}}"
    );
    case!(
        boat_send_trade,
        |st, p1, p2| {
            st.stats_cx().boat_send_trade(p1, p2);
        },
        "{client1:{boats:{trade:[1n]}}}"
    );
    case!(
        boat_arrive_trade,
        |st, p1, p2| {
            st.stats_cx().boat_arrive_trade(p1, p2, StatNum::Num(1.0));
        },
        "{client1:{boats:{trade:[0n,1n]},gold:[0n,0n,1n]},client2:{gold:[0n,0n,1n]}}"
    );
    case!(
        boat_captured_trade,
        |st, p1, p2| {
            st.stats_cx().boat_captured_trade(p1, p2, StatNum::Num(1.0));
        },
        "{client1:{boats:{trade:[0n,0n,1n]},gold:[0n,0n,0n,1n]}}"
    );
    case!(
        boat_destroy_trade,
        |st, p1, p2| {
            st.stats_cx().boat_destroy_trade(p1, p2);
        },
        "{client1:{boats:{trade:[0n,0n,0n,1n]}}}"
    );
    case!(
        boat_send_troops,
        |st, p1, p2| {
            st.stats_cx().boat_send_troops(p1, p2, StatNum::Num(1.0));
        },
        "{client1:{boats:{trans:[1n]}}}"
    );
    case!(
        boat_arrive_troops,
        |st, p1, p2| {
            st.stats_cx().boat_arrive_troops(p1, p2, StatNum::Num(1.0));
        },
        "{client1:{boats:{trans:[0n,1n]}}}"
    );
    case!(
        boat_destroy_troops,
        |st, p1, p2| {
            st.stats_cx().boat_destroy_troops(p1, p2, StatNum::Num(1.0));
        },
        "{client1:{boats:{trans:[0n,0n,0n,1n]}}}"
    );
    case!(
        bomb_launch,
        |st, p1, p2| {
            st.stats_cx().bomb_launch(p1, p2, UnitType::AtomBomb);
        },
        "{client1:{bombs:{abomb:[1n]}}}"
    );
    case!(
        bomb_land,
        |st, p1, p2| {
            st.stats_cx().bomb_land(p1, p2, UnitType::HydrogenBomb);
        },
        "{client1:{bombs:{hbomb:[0n,1n]}}}"
    );
    case!(
        bomb_intercept,
        |st, p1, p2| {
            let _ = p2;
            st.stats_cx().bomb_intercept(p1, UnitType::MirvWarhead, StatNum::Num(1.0));
        },
        "{client1:{bombs:{mirvw:[0n,0n,1n]}}}"
    );
    case!(
        gold_work,
        |st, p1, p2| {
            let _ = p2;
            st.stats_cx().gold_work(p1, StatNum::Num(1.0));
        },
        "{client1:{gold:[1n]}}"
    );
    case!(
        train_trade_gold,
        |st, p1, p2| {
            st.stats_cx().train_self_trade(p1, StatNum::Num(2.0));
            st.stats_cx().train_external_trade(p2, StatNum::Num(1.0));
        },
        "{client1:{gold:[0n,0n,0n,0n,2n]},client2:{gold:[0n,0n,0n,0n,0n,1n]}}"
    );
    case!(
        unit_build,
        |st, p1, p2| {
            let _ = p2;
            st.stats_cx().unit_build(p1, UnitType::City);
        },
        "{client1:{units:{city:[1n]}}}"
    );
    case!(
        unit_capture,
        |st, p1, p2| {
            let _ = p2;
            st.stats_cx().unit_capture(p1, UnitType::DefensePost);
        },
        "{client1:{units:{defp:[0n,0n,1n]}}}"
    );
    case!(
        unit_destroy,
        |st, p1, p2| {
            let _ = p2;
            st.stats_cx().unit_destroy(p1, UnitType::MissileSilo);
        },
        "{client1:{units:{silo:[0n,1n]}}}"
    );
    case!(
        unit_lose,
        |st, p1, p2| {
            let _ = p2;
            st.stats_cx().unit_lose(p1, UnitType::Port);
        },
        "{client1:{units:{port:[0n,0n,0n,1n]}}}"
    );
    case!(
        player_killed,
        |st, p1, p2| {
            st.stats_cx().player_killed(p1, 10);
            st.stats_cx().player_killed(p2, 40);
        },
        "{client1:{killedAt:10n},client2:{killedAt:40n}}"
    );

    #[test]
    fn records_the_largest_single_incoming_attack_not_the_sum() {
        let (mut st, p1, p2, _) = two_clients();
        for x in [10.0, 50.0, 20.0] {
            st.stats_cx().attack(p1, p2, StatNum::Num(x));
            st.stats_cx().attack_max_incoming(p2, StatNum::Num(x));
        }
        let recv = rec(&st, "client2").attacks.clone().expect("attacks");
        assert_eq!((recv[ATTACK_INDEX_RECV], recv[ATTACK_INDEX_MAX_RECV]), (80, 50));
    }

    #[test]
    fn attack_cancel_does_not_lower_the_recorded_maximum() {
        let (mut st, p1, p2, _) = two_clients();
        st.stats_cx().attack(p1, p2, StatNum::Num(50.0));
        st.stats_cx().attack_max_incoming(p2, StatNum::Num(50.0));
        st.stats_cx().attack_cancel(p1, p2, StatNum::Num(50.0));
        let recv = rec(&st, "client2").attacks.clone().expect("attacks");
        assert_eq!((recv[ATTACK_INDEX_RECV], recv[ATTACK_INDEX_MAX_RECV]), (0, 50));
    }

    #[test]
    fn attack_alone_records_no_maximum() {
        let (mut st, p1, p2, _) = two_clients();
        st.stats_cx().attack(p1, p2, StatNum::Num(50.0));
        let recv = rec(&st, "client2").attacks.clone().expect("attacks");
        assert_eq!(recv[ATTACK_INDEX_RECV], 50);
        assert_eq!(recv.get(ATTACK_INDEX_MAX_RECV), None);
    }

    #[test]
    fn gold_war_counts_conquests_by_player_type() {
        let (mut st, p1, p2, nation) = two_clients();
        st.stats_cx().gold_war(p1, p2, StatNum::Num(1.0));
        assert_eq!(dump(&st), "{client1:{conquests:[1n],gold:[0n,1n]}}");
        st.stats_cx().gold_war(p1, p2, StatNum::Num(1.0));
        assert_eq!(dump(&st), "{client1:{conquests:[2n],gold:[0n,2n]}}");
        st.stats_cx().gold_war(p1, nation, StatNum::Big(3));
        assert_eq!(dump(&st), "{client1:{conquests:[2n,1n],gold:[0n,5n]}}");
    }

    #[test]
    fn record_kill_only_for_human_client_victims() {
        let (mut st, p1, p2, nation) = two_clients();
        st.stats_cx().record_kill(p1, p2, StatNum::Num(30.0));
        st.stats_cx().record_kill(p1, p2, StatNum::Num(35.0));
        st.stats_cx().record_kill(p1, nation, StatNum::Num(36.0));
        let kills = rec(&st, "client1").kills.clone().expect("kills");
        assert_eq!(
            kills,
            vec![KillRec { victim: "client2".into(), tick: 30 }, KillRec { victim: "client2".into(), tick: 35 }]
        );
        assert!(st.stats.full_record("client2").is_none());
    }

    #[test]
    fn record_final_tiles_spawn_tile_and_first_write_wins_fields() {
        let (mut st, p1, p2, _) = two_clients();
        st.stats_cx().record_final_tiles(p1, StatNum::Num(42.0));
        assert_eq!(rec(&st, "client1").final_tiles, Some(42));
        st.stats_cx().record_spawn_tile(p1, 7);
        st.stats_cx().record_spawn_tile(p1, 9); // last write wins
        assert_eq!(rec(&st, "client1").spawn_tile, Some(9));
        st.stats_cx().record_killed_by(p1, None);
        st.stats_cx().record_killed_by(p1, Some("client2")); // first write wins, null counts
        assert_eq!(rec(&st, "client1").killed_by, crate::jsval::Tri::Null);
        st.stats_cx().record_death_position(p2, 3);
        st.stats_cx().record_death_position(p2, 1);
        assert_eq!(rec(&st, "client2").death_position, Some(3.0));
    }

    #[test]
    fn alliance_stats() {
        let (mut st, p1, p2, _) = two_clients();
        st.stats_cx().alliance_formed(p1);
        st.stats_cx().alliance_ended(p1, StatNum::Num(100.0), Some(AllianceEndCounter::BrokenByOther));
        st.stats_cx().alliance_ended(p1, StatNum::Num(40.0), Some(AllianceEndCounter::Expired));
        st.stats_cx().alliance_ended(p2, StatNum::Num(7.0), None);
        st.stats_cx().record_alliances_at_end(p1, 2, StatNum::Num(30.0));
        st.stats_cx().record_alliances_at_end(p1, 2, StatNum::Num(30.0)); // written, not added
        assert_eq!(dump(&st), "{client1:{alliances:[1n,1n,1n,2n,0n,100n]},client2:{alliances:[0n,0n,0n,0n,0n,7n]}}");
    }

    #[test]
    fn gold_donation_received_counts_broke_recipients() {
        let (mut st, p1, _, _) = two_clients();
        st.stats_cx().gold_donation_received(p1, StatNum::Big(100), StatNum::Big(49_999));
        st.stats_cx().gold_donation_received(p1, StatNum::Big(5), StatNum::Big(50_000));
        assert_eq!(dump(&st), "{client1:{donations:[2n,1n],gold:[0n,0n,0n,0n,0n,0n,105n]}}");
    }

    #[test]
    fn migrate_client_id_moves_the_record() {
        let (mut st, p1, p2, _) = two_clients();
        st.stats_cx().attack(p1, p2, StatNum::Num(10.0));
        let before = rec(&st, "client1").clone();
        st.stats_cx().migrate_client_id("client1", "new_client1");
        assert!(st.stats.full_record("client1").is_none());
        assert_eq!(rec(&st, "new_client1"), &before);
    }

    #[test]
    fn a_throw_leaves_the_zero_extended_array_like_ts() {
        let (mut st, p1, _, _) = two_clients();
        assert!(caught_ts_throw(|| st.stats_cx().train_external_trade(p1, StatNum::Num(f64::NAN))).is_some());
        assert_eq!(dump(&st), "{client1:{gold:[0n,0n,0n,0n,0n,0n]}}");
    }

    // ---- `tests/StatsTiles.test.ts` (tick sampling) ----

    fn sample(st: &mut State, p: SmallId, tiles: f64, troops: f64, alliances: i32) {
        st.stats_cx().record_tick_sample(p, StatNum::Num(tiles), StatNum::Num(troops), alliances);
    }
    fn tiles_of(st: &State) -> Vec<i64> {
        rec(st, "client1").tiles.clone().expect("tiles")
    }

    #[test]
    fn keeps_the_worst_drawdown_even_after_a_later_higher_peak() {
        let (mut st, p1, _, _) = two_clients();
        for t in [1000.0, 100.0, 1100.0] {
            sample(&mut st, p1, t, 0.0, 0);
        }
        assert_eq!(tiles_of(&st), vec![1100, 1000, 100]);
    }

    #[test]
    fn keeps_the_proportionally_worst_decline_not_the_most_recent() {
        let (mut st, p1, _, _) = two_clients();
        for t in [1000.0, 500.0, 1000.0, 900.0] {
            sample(&mut st, p1, t, 0.0, 0);
        }
        let t = tiles_of(&st);
        assert_eq!((t[TILE_INDEX_DRAWDOWN_PEAK], t[TILE_INDEX_DRAWDOWN_TROUGH]), (1000, 500));
    }

    #[test]
    fn records_no_depth_for_a_player_that_only_grows() {
        let (mut st, p1, _, _) = two_clients();
        for t in [10.0, 100.0, 1000.0] {
            sample(&mut st, p1, t, 0.0, 0);
        }
        let t = tiles_of(&st);
        assert_eq!(t[TILE_INDEX_DRAWDOWN_PEAK], t[TILE_INDEX_DRAWDOWN_TROUGH]);
    }

    #[test]
    fn records_peak_troops_and_peak_concurrent_alliances() {
        let (mut st, p1, _, _) = two_clients();
        sample(&mut st, p1, 10.0, 500.0, 3);
        sample(&mut st, p1, 10.0, 100.0, 1);
        let r = rec(&st, "client1");
        assert_eq!(r.peak_troops, Some(500));
        assert_eq!(r.alliances.as_ref().expect("alliances")[ALLIANCE_INDEX_PEAK_CONCURRENT], 3);
    }

    #[test]
    fn still_tracks_drawdown_after_a_leading_zero_tile_sample() {
        let (mut st, p1, _, _) = two_clients();
        for t in [0.0, 5000.0, 1.0] {
            sample(&mut st, p1, t, 0.0, 0);
        }
        let t = tiles_of(&st);
        assert_eq!((t[TILE_INDEX_DRAWDOWN_PEAK], t[TILE_INDEX_DRAWDOWN_TROUGH]), (5000, 1));
    }

    #[test]
    fn replaces_a_stored_drawdown_once_a_later_decline_is_worse() {
        let (mut st, p1, _, _) = two_clients();
        for t in [100.0, 50.0, 1000.0, 100.0] {
            sample(&mut st, p1, t, 0.0, 0);
        }
        let t = tiles_of(&st);
        assert_eq!((t[TILE_INDEX_DRAWDOWN_PEAK], t[TILE_INDEX_DRAWDOWN_TROUGH]), (1000, 100));
    }

    #[test]
    fn continues_after_a_restore_that_drops_the_sample_cache() {
        let (mut st, p1, _, _) = two_clients();
        sample(&mut st, p1, 100.0, 200.0, 1);
        sample(&mut st, p1, 50.0, 150.0, 1);
        // TS restore: `_sampleCache` is a WeakMap, so restored records start without a cache.
        for r in st.stats.full.data.values_mut() {
            r.sample_cache = None;
        }
        sample(&mut st, p1, 200.0, 500.0, 3);
        sample(&mut st, p1, 10.0, 300.0, 2);
        let r = rec(&st, "client1");
        assert_eq!(r.tiles.clone().expect("tiles"), vec![200, 200, 10]);
        assert_eq!(r.peak_troops, Some(500));
        assert_eq!(r.alliances.as_ref().expect("alliances")[ALLIANCE_INDEX_PEAK_CONCURRENT], 3);
    }

    #[test]
    fn big_and_large_inputs_take_the_fallback_path() {
        let (mut st, p1, _, _) = two_clients();
        st.stats_cx().record_tick_sample(p1, StatNum::Big(100_000_000), StatNum::Big(500_000_000), 2);
        st.stats_cx().record_tick_sample(p1, StatNum::Big(20_000_000), StatNum::Big(100_000_000), 1);
        let r = rec(&st, "client1");
        assert_eq!(r.tiles.clone().expect("tiles"), vec![100_000_000, 100_000_000, 20_000_000]);
        assert_eq!(r.peak_troops, Some(500_000_000));
    }

    #[test]
    fn fractional_samples_take_the_fallback_path() {
        let (mut st, p1, _, _) = two_clients();
        sample(&mut st, p1, 10.8, 20.4, 1);
        let r = rec(&st, "client1");
        assert_eq!(r.tiles.clone().expect("tiles"), vec![10, 10, 10]);
        assert_eq!(r.peak_troops, Some(20));
    }

    #[test]
    fn ignores_players_without_a_client_id() {
        let (mut st, _, _, nation) = two_clients();
        sample(&mut st, nation, 500.0, 1000.0, 0);
        assert_eq!(dump(&st), "{}");
    }

    /// `matches the original reference all-bigint sampler across randomized mixed inputs`:
    /// Mulberry32(1337), 3 players × 600 ticks, a cache-dropping "restore" at tick 300.
    #[test]
    fn matches_the_reference_all_bigint_sampler() {
        #[derive(Default, Clone)]
        struct Ref {
            tiles: Option<Vec<i128>>,
            peak_troops: Option<i128>,
            alliances: Option<Vec<i128>>,
        }
        fn ref_sample(p: &mut Ref, tiles: StatNum, troops: StatNum, alliance_count: i32) {
            let big = |x: StatNum| -> i128 {
                match x {
                    StatNum::Big(b) => i128::from(b),
                    StatNum::Num(v) => i128::from(of_det::bigint::from_floor_strict(v)),
                }
            };
            let t = big(tiles);
            let pt = p.tiles.get_or_insert_with(|| vec![0, 0, 0]);
            if t > pt[0] {
                pt[0] = t;
            }
            let (peak, ddp, ddt) = (pt[0], pt[1], pt[2]);
            if ddp == 0 || (peak - t) * ddp > (ddp - ddt) * peak {
                pt[1] = peak;
                pt[2] = t;
            }
            let tr = big(troops);
            if p.peak_troops.is_none_or(|x| tr > x) {
                p.peak_troops = Some(tr);
            }
            let pa = p.alliances.get_or_insert_with(|| vec![0, 0]);
            while pa.len() <= ALLIANCE_INDEX_PEAK_CONCURRENT {
                pa.push(0);
            }
            let a = i128::from(alliance_count);
            if a > pa[ALLIANCE_INDEX_PEAK_CONCURRENT] {
                pa[ALLIANCE_INDEX_PEAK_CONCURRENT] = a;
            }
        }
        // Mulberry32 (the test's `rand`).
        let mut s: i32 = 1337;
        let mut rand = || -> f64 {
            use of_det::js::imul;
            s = s.wrapping_add(0x6d2b_79f5);
            let su = s as u32;
            let mut t = imul((su ^ (su >> 15)) as i32, 1 | s);
            let tu = t as u32;
            t = (t.wrapping_add(imul((tu ^ (tu >> 7)) as i32, 61 | t))) ^ t;
            let tu = t as u32;
            f64::from(tu ^ (tu >> 14)) / 4_294_967_296.0
        };
        let sample_pool = |rand: &mut dyn FnMut() -> f64, fixed_peak: f64| -> StatNum {
            let roll = rand();
            if roll < 0.05 {
                return StatNum::Num(0.0);
            }
            if roll < 0.15 {
                return StatNum::Num(fixed_peak);
            }
            if roll < 0.45 {
                return StatNum::Num((rand() * 50_000.0).floor());
            }
            if roll < 0.6 {
                return StatNum::Big((rand() * 50_000.0).floor() as i64);
            }
            if roll < 0.75 {
                let a = (rand() * 50_000.0).floor();
                return StatNum::Num(a + rand());
            }
            if roll < 0.88 {
                return StatNum::Num(90_000_000.0 + (rand() * 10_000_000.0).floor());
            }
            StatNum::Big(90_000_000 + (rand() * 50_000_000.0).floor() as i64)
        };
        let mut st = state_with(vec![
            PlayerInfo::new("a", PlayerType::Human, Some("p1".into()), "id1"),
            PlayerInfo::new("b", PlayerType::Human, Some("p2".into()), "id2"),
            PlayerInfo::new("c", PlayerType::Human, Some("p3".into()), "id3"),
        ]);
        let mut refs = vec![Ref::default(); 3];
        let peaks = [12_345.0, 45_678.0, 89_000.0];
        for tick in 0..600 {
            if tick == 300 {
                for r in st.stats.full.data.values_mut() {
                    r.sample_cache = None;
                }
            }
            for (i, (rf, peak)) in refs.iter_mut().zip(peaks).enumerate() {
                let tiles = if tick < 5 && i == 0 { StatNum::Num(0.0) } else { sample_pool(&mut rand, peak) };
                let troops = sample_pool(&mut rand, 50_000.0);
                let alliances = (rand() * 8.0).floor() as i32;
                st.stats_cx().record_tick_sample(SmallId::from_player_index(i), tiles, troops, alliances);
                ref_sample(rf, tiles, troops, alliances);
            }
        }
        for (i, cid) in ["p1", "p2", "p3"].iter().enumerate() {
            let r = rec(&st, cid);
            let to128 = |v: &Option<Vec<i64>>| v.as_ref().map(|v| v.iter().map(|x| i128::from(*x)).collect::<Vec<_>>());
            assert_eq!(to128(&r.tiles), refs[i].tiles, "tiles {cid}");
            assert_eq!(r.peak_troops.map(i128::from), refs[i].peak_troops, "peakTroops {cid}");
            assert_eq!(to128(&r.alliances), refs[i].alliances, "alliances {cid}");
        }
    }
}
