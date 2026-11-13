//! Tests for `state::unit`. Owner: **P2**.
//!
//! T2 module tests of the `UnitImpl` methods that reach neither the map (P3) nor P1's
//! `GameImpl` bodies: the `isInCombat` view side effect (ARCHITECTURE §5, G5), the partial
//! state merges, trajectory clamping, the missile queue, `setTroops`, and the CS row.
//! `buildUnit`/`delete`/`setOwner` need `addUnit`/`removeUnit`/`bumpUnitsVersion` (P1) and the
//! unit config (P3); they are covered by the integration scenarios (FILE_MAP §4 T3).

// Test code: a failed `expect` is the assertion, and catching panics is how a TS throw is
// observed (the lint allows these only in path/stepper.rs and runner.rs for the engine itself).
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::disallowed_methods)]

use super::*;
use crate::api::types::{PlayerInfo, PlayerType};
use crate::canon::CanonCx;
use crate::exec::ExecStore;
use crate::scratch::Scratch;
use crate::state::stats::tests::state_with;

fn caught_ts_throw(f: impl FnOnce()) -> Option<String> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(()) => None,
        Err(p) => match of_det::throw::as_ts_throw(&*p) {
            Some(t) => Some(format!("{t:?}")),
            None => std::panic::resume_unwind(p),
        },
    }
}

fn one_player() -> State {
    state_with(vec![PlayerInfo::new("Bob", PlayerType::Human, Some("c1".into()), "pid1")])
}

/// A unit allocated straight into the arena and listed in its owner's `_units`.
fn add_unit(st: &mut State, t: UnitType, f: impl FnOnce(&mut Unit)) -> UnitRef {
    let id = st.next_unit_id;
    st.next_unit_id += 1;
    let mut u =
        Unit { id, unit_type: t, owner: SmallId(1), active: true, level: 1, targetable: true, ..Unit::default() };
    f(&mut u);
    let r = st.units.alloc(u);
    let p = st.player_data_mut(SmallId(1));
    p.units.push(r);
    p.my_units_version += 1;
    r
}

fn warship(st: &mut State) -> UnitRef {
    add_unit(st, UnitType::Warship, |u| {
        u.warship_state = Some(WarshipState {
            state: WarshipMode::Patrolling,
            patrol_tile: Some(5),
            retreat_port: None,
            is_in_combat: None,
            last_combat_tick: -100,
            veterancy: 0,
            veterancy_progress: 0,
        })
    })
}

fn nuke(st: &mut State, traj: Vec<TileRef>) -> UnitRef {
    add_unit(st, UnitType::AtomBomb, |u| {
        u.nuke_state = Some(NukeState { trajectory: traj, trajectory_index: 0, targeted_by_sam: false, wait_ticks: 0 })
    })
}

#[test]
fn unit_touched_writes_is_in_combat_only_on_warships() {
    let mut st = one_player();
    let w = warship(&mut st);
    let city = add_unit(&mut st, UnitType::City, |_| {});
    st.ticks = 10;
    assert_eq!(st.unit_data(w).warship_state.as_ref().expect("ws").is_in_combat, None);
    st.unit_touched(w);
    assert_eq!(st.unit_data(w).warship_state.as_ref().expect("ws").is_in_combat, Some(false));
    st.unit_data_mut(w).warship_state.as_mut().expect("ws").last_combat_tick = 7; // 10 - 7 <= 3
    st.unit_touched(w);
    assert_eq!(st.unit_data(w).warship_state.as_ref().expect("ws").is_in_combat, Some(true));
    st.unit_touched(city); // no warship state: nothing to write
    assert!(st.unit_data(city).warship_state.is_none());
}

#[test]
fn warship_state_getter_writes_and_throws_on_non_warships() {
    let mut st = one_player();
    let w = warship(&mut st);
    let city = add_unit(&mut st, UnitType::City, |_| {});
    let mut sc = Scratch::default();
    st.ticks = 3;
    let mut cx = Ctx::new(&mut st, &mut sc, None);
    assert_eq!(cx.unit_mut(w).warship_state().is_in_combat, Some(false));
    let msg = caught_ts_throw(|| {
        cx.unit_mut(city).warship_state();
    });
    assert!(msg.expect("throws").contains("warshipState called on non-warship unit"));
    assert!(
        caught_ts_throw(|| {
            let _ = cx.st.unit(city).is_in_combat();
        })
        .is_some()
    );
}

#[test]
fn update_warship_state_merges_and_drops_is_in_combat() {
    let mut st = one_player();
    let w = warship(&mut st);
    let mut sc = Scratch::default();
    st.ticks = 50;
    let mut cx = Ctx::new(&mut st, &mut sc, None);
    // Same values: early return, the stored state (and its isInCombat) is untouched.
    cx.unit_mut(w).touch();
    cx.unit_mut(w).update_warship_state(WarshipStateUpdate { patrol_tile: Some(Some(5)), ..Default::default() });
    assert_eq!(cx.st.unit_data(w).warship_state.as_ref().expect("ws").is_in_combat, Some(false));
    // A change: the rebuilt object has no isInCombat key, then the trailing toUpdate writes it.
    cx.unit_mut(w).update_warship_state(WarshipStateUpdate {
        state: Some(WarshipMode::Retreating),
        retreat_port: Some(Some(9)),
        ..Default::default()
    });
    let ws = cx.st.unit_data(w).warship_state.clone().expect("ws");
    assert_eq!(
        (ws.state, ws.patrol_tile, ws.retreat_port, ws.is_in_combat),
        (WarshipMode::Retreating, Some(5), Some(9), Some(false))
    );
    // `patrolTile: undefined` present in the update overrides the stored tile.
    cx.unit_mut(w).update_warship_state(WarshipStateUpdate { patrol_tile: Some(None), ..Default::default() });
    assert_eq!(cx.st.unit_data(w).warship_state.as_ref().expect("ws").patrol_tile, None);
    // `isInCombat: true` marks combat (lastCombatTick = ticks) even when nothing else changes.
    cx.unit_mut(w).update_warship_state(WarshipStateUpdate { is_in_combat: Some(Some(true)), ..Default::default() });
    let ws = cx.st.unit_data(w).warship_state.clone().expect("ws");
    assert_eq!((ws.last_combat_tick, ws.is_in_combat), (50, Some(true)));
}

#[test]
fn nuke_state_updates_and_trajectory_clamp() {
    let mut st = one_player();
    let n = nuke(&mut st, vec![1, 2, 3]);
    let empty = nuke(&mut st, vec![]);
    let city = add_unit(&mut st, UnitType::City, |_| {});
    let mut sc = Scratch::default();
    let mut cx = Ctx::new(&mut st, &mut sc, None);
    cx.unit_mut(n).set_trajectory_index(7);
    assert_eq!(cx.st.unit(n).trajectory_index(), 2);
    cx.unit_mut(n).set_trajectory_index(-4);
    assert_eq!(cx.st.unit(n).trajectory_index(), 0);
    cx.unit_mut(empty).set_trajectory_index(0); // max = -1
    assert_eq!(cx.st.unit(empty).trajectory_index(), -1);
    cx.unit_mut(n).update_nuke_state(NukeStateUpdate {
        wait_ticks: Some(4),
        trajectory: Some(vec![9]),
        ..Default::default()
    });
    let ns = cx.st.unit(n).nuke_state().clone();
    assert_eq!((ns.wait_ticks, ns.trajectory), (4, vec![1, 2, 3])); // trajectory in the update is ignored
    cx.unit_mut(n).set_targeted_by_sam(true);
    assert!(cx.st.unit(n).targeted_by_sam());
    assert_eq!(cx.st.unit(city).trajectory_index(), 0);
    assert!(cx.st.unit(city).trajectory().is_empty());
    assert!(
        caught_ts_throw(|| cx.unit_mut(city).set_trajectory_index(1)).expect("throws").contains("setTrajectoryIndex")
    );
    assert!(caught_ts_throw(|| cx.unit_mut(city).set_targeted_by_sam(true)).is_some());
    assert!(
        caught_ts_throw(|| {
            let _ = cx.st.unit(city).nuke_state();
        })
        .expect("throws")
        .contains("nukeState called on non-nuke unit")
    );
}

#[test]
fn transport_ship_state_value_reads_unit_troops() {
    let mut st = one_player();
    let t = add_unit(&mut st, UnitType::TransportShip, |u| {
        u.troops = 123.0;
        u.transport_ship_state = Some(TransportShipState { is_retreating: false, troops: 0.0 });
    });
    let city = add_unit(&mut st, UnitType::City, |_| {});
    let mut sc = Scratch::default();
    let mut cx = Ctx::new(&mut st, &mut sc, None);
    assert_eq!(cx.st.unit(t).transport_ship_state_value(), TransportShipState { is_retreating: false, troops: 123.0 });
    cx.unit_mut(t)
        .update_transport_ship_state(TransportShipStateUpdate { is_retreating: Some(true), troops: Some(5.0) });
    assert_eq!(cx.st.unit_data(t).transport_ship_state, Some(TransportShipState { is_retreating: true, troops: 0.0 }));
    assert!(
        caught_ts_throw(|| {
            let _ = cx.st.unit(city).transport_ship_state();
        })
        .expect("throws")
        .contains("transportShipState called on non-transport-ship unit")
    );
}

#[test]
fn missile_queue_cooldown_and_set_troops() {
    let mut st = one_player();
    let silo = add_unit(&mut st, UnitType::MissileSilo, |_| {});
    let mut sc = Scratch::default();
    st.ticks = 11;
    let mut cx = Ctx::new(&mut st, &mut sc, None);
    assert!(!cx.st.unit(silo).is_in_cooldown());
    cx.unit_mut(silo).launch();
    assert!(cx.st.unit(silo).is_in_cooldown()); // length 1 === level 1
    assert_eq!(cx.st.unit(silo).ticks_left_in_cooldown(), Some(11));
    cx.unit_mut(silo).reload_missile();
    cx.unit_mut(silo).reload_missile(); // shift() on [] is a no-op
    assert_eq!(cx.st.unit(silo).ticks_left_in_cooldown(), None);
    // `Math.max(0, troops)`: negative clamps, -0 becomes +0, NaN stays NaN.
    cx.unit_mut(silo).set_troops(-5.0);
    assert_eq!(cx.st.unit(silo).troops(), 0.0);
    cx.unit_mut(silo).set_troops(-0.0);
    assert!(cx.st.unit(silo).troops().is_sign_positive());
    cx.unit_mut(silo).set_troops(f64::NAN);
    assert!(cx.st.unit(silo).troops().is_nan());
    cx.unit_mut(silo).set_troops(7.5);
    assert_eq!(cx.st.unit(silo).troops(), 7.5);
}

#[test]
fn deletion_marks_and_simple_setters() {
    let mut st = one_player();
    let city = add_unit(&mut st, UnitType::City, |_| {});
    let mut sc = Scratch::default();
    st.ticks = 20;
    let mut cx = Ctx::new(&mut st, &mut sc, None);
    assert!(!cx.st.unit(city).is_marked_for_deletion());
    cx.st.unit_data_mut(city).deletion_at = Some(25);
    assert!(cx.st.unit(city).is_marked_for_deletion());
    assert!(!cx.st.unit(city).is_overdue_deletion()); // 20 - 25 > 0 is false
    cx.st.ticks = 26;
    assert!(cx.st.unit(city).is_overdue_deletion());
    cx.st.unit_data_mut(city).active = false;
    assert!(!cx.st.unit(city).is_overdue_deletion()); // inactive → false
    cx.unit_mut(city).clear_pending_deletion();
    assert!(!cx.st.unit(city).is_marked_for_deletion());
    cx.unit_mut(city).set_targetable(false);
    cx.unit_mut(city).set_loaded(true);
    cx.unit_mut(city).set_tile_target(Some(4));
    cx.unit_mut(city).set_reached_target();
    let d = cx.st.unit_data(city);
    assert_eq!((d.targetable, d.loaded, d.target_tile, d.reached_target), (false, Some(true), Some(4), true));
}

#[test]
fn delete_of_an_inactive_unit_throws_with_the_ts_message() {
    let mut st = one_player();
    let city = add_unit(&mut st, UnitType::City, |u| u.active = false);
    let mut sc = Scratch::default();
    let mut cx = Ctx::new(&mut st, &mut sc, None);
    let msg = caught_ts_throw(|| cx.unit_mut(city).delete(None, None)).expect("throws");
    assert!(msg.contains("cannot delete Unit:City,owner:Bob not active"), "{msg}");
}

#[test]
fn unit_hash_matches_ts_formula() {
    let mut st = one_player();
    let city = add_unit(&mut st, UnitType::City, |u| u.tile = 42);
    let id = st.unit_data(city).id;
    assert_eq!(st.unit(city).hash(), 42.0 + simple_hash("City") * f64::from(id));
}

#[test]
fn canon_row_follows_the_dump_order_and_undef_rules() {
    let mut st = one_player();
    let w = warship(&mut st);
    let ex = ExecStore::default();
    let mut cx = CanonCx::new(&st, &ex);
    let Cs::Obj(fields) = st.unit_data(w).canon(&mut cx) else { panic!("object") };
    let keys: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        keys,
        [
            "id",
            "type",
            "owner",
            "tile",
            "lastTile",
            "active",
            "targetTile",
            "targetPlayer",
            "targetUnit",
            "health",
            "transportShipState",
            "warshipState",
            "nukeState",
            "reachedTarget",
            "wasDestroyedByEnemy",
            "destroyer",
            "lastSetSafeFromPirates",
            "underConstruction",
            "lastOwner",
            "troops",
            "missileTimerQueue",
            "hasTrainStation",
            "level",
            "targetable",
            "loaded",
            "trainType",
            "deletionAt",
            "samLauncherState",
        ]
    );
    let ws = &fields[11].1;
    let Cs::Obj(wf) = ws else { panic!("warship object") };
    // retreatPort and isInCombat are undefined → absent.
    assert_eq!(wf[2], ("retreatPort".to_owned(), Cs::Absent));
    assert_eq!(wf[3], ("isInCombat".to_owned(), Cs::Absent));
    assert_eq!(fields[1].1, Cs::Str("Warship".into()));
}
