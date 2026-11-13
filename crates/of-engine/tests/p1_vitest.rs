//! P1 rung T0: ported upstream vitests (FILE_MAP §4 row P1). Owner: **P1**.
//!
//! Each module is one upstream file; each test keeps the upstream name and assertions. They
//! drive the whole stack (map load, players, units, executions). They were `#[ignore]`d in
//! the P1 branch until the S1 packages were merged; at merge1 all 28 pass and run by default. Deviations from upstream are noted per test: `TestConfig` overrides that are
//! not `GameConfig` keys cannot be applied to `EngineConfig` (see `common/mod.rs`).
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{TestGame, player_info, setup, setup_full};
use of_engine::api::types::{PlayerInfo, PlayerType, UnitType};
use of_engine::exec::Exec;
use of_engine::exec::alliance::{break_::BreakAllianceExec, reject::AllianceRejectExec, request::AllianceRequestExec};
use of_engine::exec::spawn::SpawnExec;
use of_engine::ids::{AllianceId, SmallId, UnitRef};
use of_engine::state::stats::StatNum;
use of_engine::state::unit::UnitParams;
use serde_json::json;

const NEEDS_DIP: &str = "S2: needs P2 Player/Stats, P3 map+config, P9 alliance executions";

fn alliances(g: &TestGame, p: SmallId) -> Vec<AllianceId> {
    g.g.state.player(p).alliances().as_slice().to_vec()
}

fn id_of(g: &TestGame, p: SmallId) -> String {
    g.g.state.player(p).player_id().to_owned()
}

/// `tests/PlayerAllianceList.test.ts`.
mod player_alliance_list {
    use super::*;

    /// `beforeEach` (`:38-55`).
    fn before() -> (TestGame, SmallId, SmallId, SmallId) {
        let mut g = setup(
            "plains",
            json!({"infiniteGold": true, "instantBuild": true, "infiniteTroops": true}),
            vec![
                player_info("player1", PlayerType::Human),
                player_info("player2", PlayerType::Human),
                player_info("player3", PlayerType::Human),
            ],
        );
        let (p1, p2, p3) = (g.p("player1"), g.p("player2"), g.p("player3"));
        for (p, y) in [(p1, 0), (p2, 1), (p3, 2)] {
            let t = g.r(0, y);
            g.cx().player_mut(p).conquer(t);
        }
        (g, p1, p2, p3)
    }

    /// `ally(a, b)` (`:20-25`): counter-requests, one tick each.
    fn ally(g: &mut TestGame, a: SmallId, b: SmallId) {
        let (ai, bi) = (id_of(g, a), id_of(g, b));
        g.add_execution(Exec::AllianceRequest(AllianceRequestExec::new(a, bi)));
        g.tick();
        g.add_execution(Exec::AllianceRequest(AllianceRequestExec::new(b, ai)));
        g.tick();
    }

    /// `breakAlliance(a, b)` (`:31-35`): init tick + run tick.
    fn break_alliance(g: &mut TestGame, a: SmallId, b: SmallId) {
        let bi = id_of(g, b);
        g.add_execution(Exec::BreakAlliance(BreakAllianceExec::new(a, bi)));
        g.tick();
        g.tick();
    }

    #[test]
    fn forming_an_alliance_adds_it_to_both_participants_lists() {
        let (mut g, p1, p2, _) = before();
        assert!(alliances(&g, p1).is_empty() && alliances(&g, p2).is_empty());
        ally(&mut g, p1, p2);
        let (l1, l2) = (alliances(&g, p1), alliances(&g, p2));
        assert_eq!((l1.len(), l2.len()), (1, 1));
        assert_eq!(l1[0], l2[0]); // same underlying alliance object
        assert_eq!(g.g.state.alliance(l1[0]).other(p1), p2);
        assert_eq!(g.g.state.alliance(l2[0]).other(p2), p1);
    }

    #[test]
    fn alliances_agrees_with_is_allied_with_and_alliance_with() {
        let (mut g, p1, p2, p3) = before();
        ally(&mut g, p1, p2);
        let st = &g.g.state;
        assert!(st.player(p1).is_allied_with(p2));
        assert_eq!(st.player(p1).alliance_with(p2), Some(alliances(&g, p1)[0]));
        assert!(!st.player(p1).is_allied_with(p3));
        assert!(alliances(&g, p3).is_empty());
    }

    #[test]
    fn breaking_an_alliance_removes_it_from_both_lists() {
        let (mut g, p1, p2, _) = before();
        ally(&mut g, p1, p2);
        assert_eq!(alliances(&g, p1).len(), 1);
        break_alliance(&mut g, p1, p2);
        assert!(alliances(&g, p1).is_empty() && alliances(&g, p2).is_empty());
        assert!(!g.g.state.player(p1).is_allied_with(p2));
    }

    #[test]
    fn expiring_an_alliance_removes_it_from_both_lists() {
        let (mut g, p1, p2, _) = before();
        ally(&mut g, p1, p2);
        assert_eq!(alliances(&g, p1).len(), 1);
        let a = alliances(&g, p1)[0];
        g.cx().alliance_expire(a); // player1.alliances()[0].expire()
        assert!(alliances(&g, p1).is_empty() && alliances(&g, p2).is_empty());
        assert!(!g.g.state.player(p1).is_allied_with(p2));
    }

    #[test]
    fn a_player_tracks_multiple_alliances_independently() {
        let (mut g, p1, p2, p3) = before();
        ally(&mut g, p1, p2);
        ally(&mut g, p1, p3);
        let l1 = alliances(&g, p1);
        assert_eq!(l1.len(), 2);
        let others: Vec<SmallId> = l1.iter().map(|&a| g.g.state.alliance(a).other(p1)).collect();
        assert!(others.contains(&p2) && others.contains(&p3));
        break_alliance(&mut g, p1, p2);
        let l1 = alliances(&g, p1);
        assert_eq!(l1.len(), 1);
        assert_eq!(g.g.state.alliance(l1[0]).other(p1), p3);
        assert!(alliances(&g, p2).is_empty());
        assert_eq!(alliances(&g, p3).len(), 1);
    }

    #[test]
    fn remove_all_alliances_clears_the_player_and_every_partner() {
        let (mut g, p1, p2, p3) = before();
        ally(&mut g, p1, p2);
        ally(&mut g, p1, p3);
        assert_eq!(alliances(&g, p1).len(), 2);
        g.cx().player_mut(p1).remove_all_alliances();
        assert!(alliances(&g, p1).is_empty() && alliances(&g, p2).is_empty() && alliances(&g, p3).is_empty());
    }

    /// Same scenario through `removeAlliancesByPlayerSilently` (`GameImpl.ts:1121-1138`),
    /// which `removeAllAlliances` is not: it snapshots the list before detaching.
    #[test]
    fn remove_alliances_by_player_silently_detaches_every_alliance() {
        let (mut g, p1, p2, p3) = before();
        ally(&mut g, p1, p2);
        ally(&mut g, p1, p3);
        g.cx().remove_alliances_by_player_silently(p1);
        assert!(alliances(&g, p1).is_empty() && alliances(&g, p2).is_empty() && alliances(&g, p3).is_empty());
        let _ = NEEDS_DIP;
    }
}

/// `tests/AllianceRequestExecution.test.ts`.
mod alliance_request_execution {
    use super::*;

    fn before() -> (TestGame, SmallId, SmallId) {
        let mut g = setup(
            "plains",
            json!({"infiniteGold": true, "instantBuild": true, "infiniteTroops": true}),
            vec![
                player_info("player1", PlayerType::Human),
                player_info("player2", PlayerType::Human),
                player_info("player3", PlayerType::Nation),
            ],
        );
        let (p1, p2) = (g.p("player1"), g.p("player2"));
        let (t0, t1) = (g.r(0, 0), g.r(0, 1));
        g.cx().player_mut(p1).conquer(t0);
        g.cx().player_mut(p2).conquer(t1);
        (g, p1, p2)
    }

    #[test]
    fn can_create_alliance_by_counter_request() {
        let (mut g, p1, p2) = before();
        g.add_execution(Exec::AllianceRequest(AllianceRequestExec::new(p1, "player2".into())));
        g.tick();
        g.add_execution(Exec::AllianceRequest(AllianceRequestExec::new(p2, "player1".into())));
        g.tick();
        assert!(g.g.state.player(p1).is_allied_with(p2));
        assert!(g.g.state.player(p2).is_allied_with(p1));
    }

    #[test]
    fn can_reject_alliance_request() {
        let (mut g, p1, p2) = before();
        g.add_execution(Exec::AllianceRequest(AllianceRequestExec::new(p1, "player2".into())));
        g.tick();
        g.add_execution(Exec::AllianceReject(AllianceRejectExec::new("player1".into(), p2)));
        g.tick();
        assert!(!g.g.state.player(p1).is_allied_with(p2));
        assert!(!g.g.state.player(p2).is_allied_with(p1));
        assert!(g.g.state.player(p1).outgoing_alliance_requests().is_empty());
    }

    /// Upstream patches `allianceRequestDuration = () => 5` and ticks 6 times; the port ticks
    /// `allianceRequestDuration() + 1` times with the real config.
    #[test]
    fn alliance_request_expires() {
        let (mut g, p1, p2) = before();
        g.add_execution(Exec::AllianceRequest(AllianceRequestExec::new(p1, "player2".into())));
        g.tick();
        assert_eq!(g.g.state.player(p1).outgoing_alliance_requests().len(), 1);
        let d = g.g.state.config().alliance_request_duration() as usize;
        g.ticks(d + 1);
        assert!(g.g.state.player(p1).outgoing_alliance_requests().is_empty());
        assert!(!g.g.state.player(p1).is_allied_with(p2));
        assert!(!g.g.state.player(p2).is_allied_with(p1));
    }

    // "alliance request is revoked immediately if requester launches a nuke" patches
    // `nukeAllianceBreakThreshold = () => 0` on the config object; not portable onto
    // `EngineConfig` (P8 owns the nuke path; M-NUKE-03 covers it at L2).
}

/// `tests/ConquerGold.test.ts`.
mod conquer_gold {
    use super::*;

    /// `addPlayerWithGold` (`:9-19`).
    fn add_player_with_gold(g: &mut TestGame, id: &str, t: PlayerType, gold: i64) -> SmallId {
        let p = g.cx().add_player(PlayerInfo::new(id, t, None, id), None);
        assert_eq!(g.p(id), p);
        g.cx().player_mut(p).add_gold(gold, None);
        p
    }

    #[test]
    fn conquer_gold_amount_full_for_bot_and_nation_half_for_human() {
        for (t, gold, want) in
            [(PlayerType::Bot, 1000, 1000), (PlayerType::Nation, 2000, 2000), (PlayerType::Human, 1000, 500)]
        {
            let mut g = setup("ocean_and_land", json!({}), vec![]);
            let p = add_player_with_gold(&mut g, "x", t, gold);
            let st = &g.g.state;
            assert_eq!(st.config().conquer_gold_amount(&st.player(p)), want, "{t:?}");
        }
    }

    /// `Conquest gold transfer` `beforeEach` (`:44-61`).
    fn before() -> (TestGame, SmallId) {
        let mut g = setup("ocean_and_land", json!({}), vec![]);
        let info = PlayerInfo::new("conqueror", PlayerType::Human, None, "conqueror");
        let c = g.cx().add_player(info.clone(), None);
        let t = g.r(0, 10);
        g.add_execution(Exec::Spawn(SpawnExec::new("test_game", info, Some(f64::from(t)), false)));
        (g, c)
    }

    #[test]
    fn conqueror_receives_all_gold_of_a_bot_or_nation() {
        for (t, gold) in [(PlayerType::Bot, 1000), (PlayerType::Nation, 800)] {
            let (mut g, c) = before();
            let v = add_player_with_gold(&mut g, "v", t, gold);
            let before = g.g.state.player(c).gold();
            g.cx().conquer_player(c, v);
            assert_eq!(g.g.state.player(c).gold(), before + gold, "{t:?}");
            assert_eq!(g.g.state.player(v).gold(), 0);
        }
    }

    #[test]
    fn conqueror_receives_half_of_a_human_who_has_attacked() {
        let (mut g, c) = before();
        let v = g
            .cx()
            .add_player(PlayerInfo::new("victim", PlayerType::Human, Some("victim_client".into()), "victim"), None);
        g.cx().player_mut(v).add_gold(1000, None);
        // `game.stats().attack(victim, game.terraNullius(), 100)`
        g.g.state.stats_cx().attack(v, SmallId::TERRA_NULLIUS, StatNum::Num(100.0));
        let before = g.g.state.player(c).gold();
        g.cx().conquer_player(c, v);
        assert_eq!(g.g.state.player(c).gold(), before + 500);
        assert_eq!(g.g.state.player(v).gold(), 0);
    }

    #[test]
    fn conqueror_receives_no_gold_from_a_human_who_never_attacked() {
        let (mut g, c) = before();
        let v = add_player_with_gold(&mut g, "afk", PlayerType::Human, 1000);
        let before = g.g.state.player(c).gold();
        g.cx().conquer_player(c, v);
        assert_eq!(g.g.state.player(c).gold(), before);
        assert_eq!(g.g.state.player(v).gold(), 1000);
    }
}

/// `tests/core/game/StructureVersion.test.ts`.
mod structure_version {
    use super::*;

    fn build(g: &mut TestGame, p: SmallId, t: UnitType, x: u32, y: u32) -> UnitRef {
        let tile = g.r(x, y);
        g.cx().player_mut(p).build_unit(t, tile, UnitParams::default())
    }

    #[test]
    fn moves_for_rail_structures_and_terrain_only() {
        let mut g = setup("plains", json!({}), vec![player_info("player", PlayerType::Human)]);
        let p = g.p("player");
        let start = g.g.state.structure_version();
        let post = build(&mut g, p, UnitType::DefensePost, 5, 5);
        assert_eq!(g.g.state.structure_version(), start);
        let city = build(&mut g, p, UnitType::City, 20, 20);
        assert_eq!(g.g.state.structure_version(), start + 1);
        build(&mut g, p, UnitType::Port, 30, 30);
        build(&mut g, p, UnitType::Factory, 40, 40);
        assert_eq!(g.g.state.structure_version(), start + 3);
        g.cx().unit_mut(post).delete(Some(false), None);
        assert_eq!(g.g.state.structure_version(), start + 3);
        g.cx().unit_mut(city).delete(Some(false), None);
        assert_eq!(g.g.state.structure_version(), start + 4);
        let t = g.r(60, 60);
        g.cx().set_water(t);
        assert_eq!(g.g.state.structure_version(), start + 5);
    }

    #[test]
    fn moves_when_a_structures_construction_state_flips() {
        let mut g = setup("plains", json!({}), vec![player_info("player", PlayerType::Human)]);
        let p = g.p("player");
        let city = build(&mut g, p, UnitType::City, 20, 20);
        let post = build(&mut g, p, UnitType::DefensePost, 5, 5);
        let start = g.g.state.structure_version();
        g.cx().unit_mut(city).set_under_construction(true);
        assert_eq!(g.g.state.structure_version(), start + 1);
        g.cx().unit_mut(city).set_under_construction(true);
        assert_eq!(g.g.state.structure_version(), start + 1);
        g.cx().unit_mut(city).set_under_construction(false);
        assert_eq!(g.g.state.structure_version(), start + 2);
        g.cx().unit_mut(post).set_under_construction(true);
        g.cx().unit_mut(post).set_under_construction(false);
        assert_eq!(g.g.state.structure_version(), start + 2);
    }

    #[test]
    fn moves_when_queued_water_conversions_flush() {
        let mut g = setup("plains", json!({"waterNukes": true}), vec![player_info("player", PlayerType::Human)]);
        let start = g.g.state.structure_version();
        g.tick();
        assert_eq!(g.g.state.structure_version(), start);
        let t = g.r(60, 60);
        g.cx().queue_water_conversion(t);
        assert_eq!(g.g.state.structure_version(), start);
        g.tick();
        assert_eq!(g.g.state.structure_version(), start + 1);
        assert!(g.g.state.map.is_water(t));
    }
}

/// `tests/UnitMemoInvalidation.test.ts`, "unit memo invalidation" (`:21-93`). The water-
/// version, nearby(), water-path-memo and spawn-tile sections belong to P3/P2/P4b.
mod unit_memo_invalidation {
    use super::*;

    fn before() -> (TestGame, SmallId, SmallId) {
        let mut g = setup(
            "plains",
            json!({"infiniteGold": true, "instantBuild": true}),
            vec![
                PlayerInfo::new("player1", PlayerType::Human, Some("c1".into()), "p1"),
                PlayerInfo::new("player2", PlayerType::Human, Some("c2".into()), "p2"),
            ],
        );
        let (p1, p2) = (g.p("p1"), g.p("p2"));
        let (t0, t1) = (g.r(0, 0), g.r(10, 10));
        g.cx().player_mut(p1).conquer(t0);
        g.cx().player_mut(p2).conquer(t1);
        (g, p1, p2)
    }

    fn city(g: &mut TestGame, p: SmallId, t: UnitType) -> UnitRef {
        let tile = g.r(0, 0);
        g.cx().player_mut(p).build_unit(t, tile, UnitParams::default())
    }

    #[test]
    fn build_and_delete_move_all_counts() {
        let (mut g, p1, _) = before();
        assert_eq!(g.g.state.player(p1).unit_count(UnitType::City), 0);
        assert_eq!(g.g.state.unit_count(UnitType::City), 0);
        let c = city(&mut g, p1, UnitType::City);
        let st = &g.g.state;
        assert_eq!(st.player(p1).unit_count(UnitType::City), 1);
        assert_eq!(st.player(p1).units_owned(UnitType::City), 1);
        assert_eq!(st.player(p1).units_of(&[UnitType::City]), [c]);
        assert_eq!(st.unit_count(UnitType::City), 1);
        assert_eq!(st.units_of(&[UnitType::City]), [c]);
        g.cx().unit_mut(c).delete(Some(false), None);
        let st = &g.g.state;
        assert_eq!(st.player(p1).unit_count(UnitType::City), 0);
        assert!(st.player(p1).units_of(&[UnitType::City]).is_empty());
        assert_eq!(st.unit_count(UnitType::City), 0);
        assert!(st.units_of(&[UnitType::City]).is_empty());
        assert_eq!(st.unit_by_id(st.unit(c).id()), None); // removeUnit: `_unitMap.delete`
    }

    #[test]
    fn level_changes_move_the_level_weighted_counts_both_directions() {
        let (mut g, p1, _) = before();
        let c = city(&mut g, p1, UnitType::City);
        assert_eq!(g.g.state.player(p1).unit_count(UnitType::City), 1);
        g.cx().unit_mut(c).increase_level();
        assert_eq!(g.g.state.unit(c).level(), 2);
        assert_eq!(g.g.state.player(p1).unit_count(UnitType::City), 2);
        assert_eq!(g.g.state.player(p1).units_owned(UnitType::City), 2);
        assert_eq!(g.g.state.unit_count(UnitType::City), 2);
        g.cx().unit_mut(c).decrease_level(None); // non-terminal
        assert!(g.g.state.unit(c).is_active());
        assert_eq!(g.g.state.player(p1).unit_count(UnitType::City), 1);
        assert_eq!(g.g.state.player(p1).units_owned(UnitType::City), 1);
        assert_eq!(g.g.state.unit_count(UnitType::City), 1);
        g.cx().unit_mut(c).decrease_level(None); // terminal: level 0 deletes the unit
        assert_eq!(g.g.state.player(p1).unit_count(UnitType::City), 0);
        assert_eq!(g.g.state.unit_count(UnitType::City), 0);
    }

    #[test]
    fn capture_moves_the_counts_between_players() {
        let (mut g, p1, p2) = before();
        let port = city(&mut g, p1, UnitType::Port);
        assert_eq!(g.g.state.player(p1).unit_count(UnitType::Port), 1);
        assert_eq!(g.g.state.player(p2).unit_count(UnitType::Port), 0);
        g.cx().unit_mut(port).set_owner(p2);
        let st = &g.g.state;
        assert_eq!(st.player(p1).unit_count(UnitType::Port), 0);
        assert!(st.player(p1).units_of(&[UnitType::Port]).is_empty());
        assert_eq!(st.player(p2).unit_count(UnitType::Port), 1);
        assert_eq!(st.player(p2).units_of(&[UnitType::Port]), [port]);
        assert_eq!(st.unit_count(UnitType::Port), 1);
        // Game-level order follows `_players` order after the capture.
        assert_eq!(st.units_of(&[UnitType::Port]), [port]);
    }

    #[test]
    fn under_construction_toggle_moves_units_owned() {
        let (mut g, p1, _) = before();
        let c = city(&mut g, p1, UnitType::City);
        g.cx().unit_mut(c).increase_level();
        assert_eq!(g.g.state.player(p1).units_owned(UnitType::City), 2);
        g.cx().unit_mut(c).set_under_construction(true);
        assert_eq!(g.g.state.player(p1).units_owned(UnitType::City), 1);
        g.cx().unit_mut(c).set_under_construction(false);
        assert_eq!(g.g.state.player(p1).units_owned(UnitType::City), 2);
    }

    #[test]
    fn units_of_type_hands_out_a_copy_not_the_memo() {
        let (mut g, p1, _) = before();
        city(&mut g, p1, UnitType::City);
        let mut a = g.g.state.player(p1).units_of(&[UnitType::City]);
        a.clear(); // caller mutates its copy
        assert_eq!(g.g.state.player(p1).units_of(&[UnitType::City]).len(), 1);
        let mut b = g.g.state.units_of(&[UnitType::City]);
        b.clear();
        assert_eq!(g.g.state.units_of(&[UnitType::City]).len(), 1);
    }
}

/// `tests/core/game/GameImpl.test.ts`, the traitor and spawn-immunity cases (`:27-174`). The
/// takeover/convert/`applySingleplayerConfig` cases test hooks that are out of scope
/// (ARCHITECTURE §1). `vi.spyOn(canSendAllianceRequest).mockReturnValue(true)` is ported as
/// a direct `createAllianceRequest` call, which is what the execution does when that gate
/// passes.
mod game_impl {
    use super::*;
    use of_engine::exec::attack::AttackExec;

    fn before() -> (TestGame, SmallId, SmallId) {
        let mut g = setup(
            "ocean_and_land",
            json!({"infiniteGold": true, "instantBuild": true, "infiniteTroops": true}),
            vec![],
        );
        let ai = PlayerInfo::new("attacker dude", PlayerType::Human, None, "attacker_id");
        let di = PlayerInfo::new("defender dude", PlayerType::Human, None, "defender_id");
        let a = g.cx().add_player(ai.clone(), None);
        let d = g.cx().add_player(di.clone(), None);
        let (ds, as_) = (g.r(0, 15), g.r(0, 14));
        g.add_execution(Exec::Spawn(SpawnExec::new("game_id", ai, Some(f64::from(as_)), false)));
        g.add_execution(Exec::Spawn(SpawnExec::new("game_id", di, Some(f64::from(ds)), false)));
        (g, a, d)
    }

    fn ally_mocked(g: &mut TestGame, a: SmallId, d: SmallId) {
        g.cx().create_alliance_request(a, d);
        g.tick();
        g.cx().create_alliance_request(d, a);
        g.tick();
        assert!(g.g.state.player(a).alliance_with(d).is_some());
        assert!(g.g.state.player(d).alliance_with(a).is_some());
    }

    fn attack_until_done(g: &mut TestGame, a: SmallId, d: SmallId) {
        let did = id_of(g, d);
        g.add_execution(Exec::Attack(AttackExec::new(Some(100.0), a, Some(did), None, true)));
        loop {
            g.tick();
            if g.g.state.player(a).outgoing_attacks().is_empty() {
                break;
            }
        }
    }

    #[test]
    fn dont_become_traitor_when_betraying_inactive_player() {
        let (mut g, a, d) = before();
        ally_mocked(&mut g, a, d);
        g.cx().player_mut(d).mark_disconnected(true, None);
        g.ticks(2);
        let al = g.g.state.player(a).alliance_with(d).expect("allied");
        g.cx().player_mut(a).break_alliance(al);
        attack_until_done(&mut g, a, d);
        assert!(!g.g.state.player(a).is_traitor());
        assert!(g.g.state.player(a).alliance_with(d).is_none());
    }

    #[test]
    fn do_become_traitor_when_betraying_active_player() {
        let (mut g, a, d) = before();
        ally_mocked(&mut g, a, d);
        g.ticks(2);
        let al = g.g.state.player(a).alliance_with(d).expect("allied");
        g.cx().player_mut(a).break_alliance(al);
        g.tick();
        attack_until_done(&mut g, a, d);
        assert!(g.g.state.player(a).is_traitor());
        assert!(g.g.state.player(a).alliance_with(d).is_none());
    }

    /// Upstream sets `setSpawnImmunityDuration(100)` on TestConfig; the port uses
    /// `EngineConfig`'s own `spawnImmunityDuration()` and asserts the same thing only when it
    /// exceeds the ticks elapsed since the spawn phase ended.
    #[test]
    fn singleplayer_late_human_spawn_gets_spawn_immunity() {
        let mut g = setup_full("plains", json!({"gameType": "Singleplayer"}), vec![], false, vec![]);
        let past = g.g.state.config().num_spawn_phase_turns() as usize + 20;
        g.ticks(past);
        let info = PlayerInfo::new("late human", PlayerType::Human, Some("late_client_id".into()), "late_player_id");
        let t = g.r(5, 5);
        g.add_execution(Exec::Spawn(SpawnExec::new("game_id", info, Some(f64::from(t)), false)));
        g.tick();
        let start_before = g.g.state.start_tick;
        g.tick();
        assert!(g.g.state.player(g.p("late_player_id")).has_spawned());
        // `spawnUpdates[SpawnPhaseEnd]` has length 1 ⇔ endSpawnPhase ran on this tick.
        assert!(start_before.is_none() && g.g.state.start_tick.is_some());
        if g.g.state.config().spawn_immunity_duration() > 0 {
            assert!(g.g.state.is_spawn_immunity_active());
        }
    }
}

/// `tests/core/GameRunner.test.ts` ("Nation spawn ordering with random spawn").
mod game_runner {
    use super::*;
    use of_engine::api::types::{Cell, Nation};
    use of_engine::exec::nation::NationExec;

    /// `createTestGame(randomSpawn, nationCells)` (`:15-53`).
    fn create_test_game(random_spawn: bool, cells: &[(f64, f64)]) -> (TestGame, PlayerInfo, Vec<(PlayerInfo, Nation)>) {
        let mut g = setup_full("plains", json!({"randomSpawn": random_spawn}), vec![], false, vec![]);
        let human = PlayerInfo::new("human", PlayerType::Human, Some("client_1".into()), "human_id");
        g.cx().add_player(human.clone(), None);
        let mut nations = Vec::new();
        for (i, &(x, y)) in cells.iter().enumerate() {
            let (name, id) = if cells.len() == 1 {
                ("TestNation".to_owned(), "nation_id".to_owned())
            } else {
                (format!("Nation{i}"), format!("nation_{i}"))
            };
            let info = PlayerInfo::new(name, PlayerType::Nation, None, id);
            let nation = Nation { spawn_cell: Some(Cell::new(x, y)), player_info: info.clone() };
            g.cx().add_player(info.clone(), None);
            nations.push((info, nation));
        }
        (g, human, nations)
    }

    fn spawned_alive(g: &TestGame, id: &str) -> (bool, bool) {
        let p = g.g.state.player(g.p(id));
        (p.has_spawned(), p.is_alive())
    }

    #[test]
    fn nation_spawns_in_singleplayer_with_random_spawn() {
        let (mut g, human, nations) = create_test_game(true, &[(50.0, 50.0)]);
        g.add_execution(Exec::Nation(NationExec::new("test_game_id", nations[0].1.clone())));
        g.add_execution(Exec::Spawn(SpawnExec::new("test_game_id", human.clone(), None, false)));
        g.ticks(4);
        assert!(spawned_alive(&g, &human.id).0);
        assert_eq!(spawned_alive(&g, &nations[0].0.id), (true, true));
    }

    #[test]
    fn multiple_nations_spawn_in_singleplayer_with_random_spawn() {
        let cells: Vec<(f64, f64)> = (0..5).map(|i| (20.0 + f64::from(i) * 15.0, 20.0 + f64::from(i) * 15.0)).collect();
        let (mut g, human, nations) = create_test_game(true, &cells);
        for (_, n) in &nations {
            g.add_execution(Exec::Nation(NationExec::new("test_game_id", n.clone())));
        }
        g.add_execution(Exec::Spawn(SpawnExec::new("test_game_id", human.clone(), None, false)));
        g.ticks(8);
        assert!(spawned_alive(&g, &human.id).0);
        for (info, _) in &nations {
            assert_eq!(spawned_alive(&g, &info.id), (true, true), "{}", info.id);
        }
    }

    #[test]
    fn nation_spawns_in_singleplayer_without_random_spawn() {
        let (mut g, _, nations) = create_test_game(false, &[(50.0, 50.0)]);
        g.add_execution(Exec::Nation(NationExec::new("test_game_id", nations[0].1.clone())));
        g.ticks(4);
        assert_eq!(spawned_alive(&g, &nations[0].0.id), (true, true));
    }
}
