//! Tick-loop semantics (FILE_MAP §4 P1 mandatory design tests): inbox order == call order,
//! init over a live-growing `uninit`, spawn-phase gating and the per-exec re-read
//! (Q-SPAWNFLIP), init on inactive executions, compaction, TS-exact post-throw state, stuck
//! games, MIRV pin/unpin, clone identity. Driven by the test-only `ProbeExec`.
//!
//! Owner: **P1** (FILE_MAP §4 P1 design tests).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use crate::api::intents::{Intent, StampedIntent};
use crate::exec::probe::{self, Phase, ProbeExec};
use crate::exec::{ExecCmd, WarheadRef};
use crate::ids::{ExecId, ExecKind};

fn game(in_spawn_phase: bool) -> Game {
    let mut st = State::default();
    st.start_tick = if in_spawn_phase { None } else { Some(0) };
    Game::from_state(st, "g")
}

fn turn(n: u64) -> Turn {
    Turn::empty(n)
}

fn probe_id(g: &Game, tag: u32) -> ExecId {
    let ar = &g.execs.ar.probe;
    let idx = (0..ar.items.len() as u32).find(|&i| ar.is_allocated(i) && ar.get(i).tag == tag).expect("probe tag");
    ExecId { kind: ExecKind::Probe, idx }
}

fn tags(g: &Game, ids: &[ExecId]) -> Vec<u32> {
    ids.iter().map(|id| g.execs.ar.probe.get(id.idx).tag).collect()
}

#[test]
fn inbox_order_and_live_growing_init_pass() {
    probe::set_hook(|p, cx, ph, t| match (p.tag, ph, t) {
        (1, Phase::Init, 0) => {
            cx.st.add_execution(ProbeExec::exec(10, true, false));
            cx.st.add_execution(ProbeExec::exec(11, true, false));
        }
        (10, Phase::Init, 0) => {
            cx.st.add_execution(ProbeExec::exec(12, true, false));
        }
        (1, Phase::Tick, 1) => {
            cx.st.add_execution(ProbeExec::exec(20, true, false));
        }
        (11, Phase::Tick, 1) => {
            cx.st.add_execution(ProbeExec::exec(21, true, false));
        }
        _ => {}
    });
    let mut g = game(false);
    g.step_with_execs(&turn(0), [ProbeExec::exec(1, true, false)]).unwrap();
    // Executions added during init are visited in the same pass, in push order.
    assert_eq!(probe::take_log(), ["init 1@0", "init 10@0", "init 11@0", "init 12@0"]);
    assert_eq!(tags(&g, &g.execs.live), [1, 10, 11, 12]);
    assert!(g.state.uninit.is_empty());
    let ev = g.step(&turn(1)).unwrap();
    assert_eq!(ev.tick, 2);
    // Tick pass in live order; the tick-pass additions are inited after it, in call order.
    assert_eq!(probe::take_log(), ["tick 1@1", "tick 10@1", "tick 11@1", "tick 12@1", "init 20@1", "init 21@1"]);
    assert_eq!(tags(&g, &g.execs.live), [1, 10, 11, 12, 20, 21]);
    probe::clear_hook();
}

#[test]
fn spawn_phase_gating_and_per_exec_reread() {
    probe::set_hook(|p, cx, ph, t| {
        if p.tag == 1 && ph == Phase::Tick && t == 1 {
            cx.end_spawn_phase(); // like SpawnTimerExecution.ts:15
        }
    });
    let mut g = game(true);
    // A: active during spawn; B: not.
    g.step_with_execs(&turn(0), [ProbeExec::exec(1, true, true), ProbeExec::exec(2, true, false)]).unwrap();
    assert_eq!(probe::take_log(), ["init 1@0"]);
    assert_eq!(tags(&g, &g.execs.live), [1]);
    assert_eq!(tags(&g, &g.state.uninit), [2]);
    // Put a non-spawn-phase execution D into `live` behind A (only a restored game can have one):
    // inSpawnPhase() is re-read per execution, so D ticks once A has ended the phase.
    let d = g.execs.ar.insert(ProbeExec::exec(3, true, false));
    g.execs.live.push(d);
    g.step(&turn(1)).unwrap();
    assert_eq!(g.state.start_tick, Some(1));
    // B's init is gated by the phase as of its own turn in the init pass (now over).
    assert_eq!(probe::take_log(), ["tick 1@1", "tick 3@1", "init 2@1"]);
    assert_eq!(tags(&g, &g.execs.live), [1, 3, 2]);
    probe::clear_hook();
}

#[test]
fn init_runs_on_inactive_and_compaction_frees() {
    probe::clear_hook();
    let mut g = game(false);
    g.step_with_execs(&turn(0), [ProbeExec::exec(1, false, false), Exec::NoOp(crate::exec::noop::NoOpExec::new())])
        .unwrap();
    // TS init has no isActive check; both were then compacted out of `live` and freed.
    assert_eq!(probe::take_log(), ["init 1@0"]);
    assert!(g.execs.live.is_empty() && g.state.uninit.is_empty());
    assert_eq!(g.execs.ar.len(), 0);
}

#[test]
fn spawn_phase_compaction_keeps_non_spawn_execs() {
    probe::clear_hook();
    let mut g = game(true);
    // In the spawn phase, compaction keeps `!activeDuringSpawnPhase() || isActive()`.
    let keep = g.execs.ar.insert(ProbeExec::exec(1, false, false));
    let drop = g.execs.ar.insert(ProbeExec::exec(2, false, true));
    g.execs.live.extend([keep, drop]);
    g.step(&turn(0)).unwrap();
    assert_eq!(tags(&g, &g.execs.live), [1]);
    assert!(!g.execs.ar.is_allocated(drop));
}

#[test]
fn tick_throw_leaves_ts_post_throw_state() {
    probe::set_hook(|p, cx, ph, t| {
        if p.tag == 1 && ph == Phase::Tick && p.ticks == 1 {
            cx.st.add_execution(ProbeExec::exec(30, true, false));
            crate::ctx::ts_throw!("boom at {t}");
        }
    });
    let mut g = game(false);
    g.step_with_execs(&turn(0), [ProbeExec::exec(1, true, false), ProbeExec::exec(2, true, false)]).unwrap();
    probe::take_log();
    let fp_live = g.execs.live.clone();
    let err = g.step(&turn(1)).unwrap_err();
    assert_eq!(err, Fatal::Tick { tick: 1, msg: "boom at 1".into() });
    // `_ticks` not incremented; the push before the throw reached unInitExecs; live unchanged;
    // the later execution never ticked; the game is not stuck.
    assert_eq!(g.state.ticks, 1);
    assert_eq!(tags(&g, &g.state.uninit), [30]);
    assert_eq!(g.execs.live, fp_live);
    assert_eq!(probe::take_log(), ["tick 1@1"]);
    assert!(g.stuck.is_none() && g.state.exec_inbox.is_empty() && g.state.exec_cmds.is_empty());
    // It keeps stepping: the same tick number runs again, and 30 is inited.
    g.step(&turn(2)).unwrap();
    assert_eq!(probe::take_log(), ["tick 1@1", "tick 2@1", "init 30@1"]);
    assert_eq!(g.state.ticks, 2);
    probe::clear_hook();
}

#[test]
fn init_throw_leaves_uninit_and_live_untouched() {
    probe::set_hook(|p, cx, ph, t| {
        if p.tag == 2 && ph == Phase::Init && t == 0 && p.inits == 1 {
            cx.st.add_execution(ProbeExec::exec(9, true, false));
            crate::ctx::ts_throw!("init throw");
        }
    });
    let mut g = game(false);
    let err = g
        .step_with_execs(
            &turn(0),
            [ProbeExec::exec(1, true, false), ProbeExec::exec(2, true, false), ProbeExec::exec(3, true, false)],
        )
        .unwrap_err();
    assert!(matches!(err, Fatal::Tick { tick: 0, .. }));
    assert_eq!(probe::take_log(), ["init 1@0", "init 2@0"]);
    // TS: `execs.push(...inited)` and `unInitExecs = unInited` never ran.
    assert!(g.execs.live.is_empty());
    assert_eq!(tags(&g, &g.state.uninit), [1, 2, 3, 9]);
    // Next tick: every one is (re-)inited, as in TS.
    g.step(&turn(1)).unwrap();
    assert_eq!(probe::take_log(), ["init 1@0", "init 2@0", "init 3@0", "init 9@0"]);
    assert_eq!(g.execs.ar.probe.get(probe_id(&g, 1).idx).inits, 2);
    probe::clear_hook();
}

#[test]
fn rust_panic_is_bug_and_sticks() {
    probe::set_hook(|p, _cx, ph, _t| {
        if p.tag == 1 && ph == Phase::Init {
            panic!("index out of bounds (simulated)");
        }
    });
    let mut g = game(false);
    let err = g.step_with_execs(&turn(0), [ProbeExec::exec(1, true, false)]).unwrap_err();
    assert!(matches!(&err, Fatal::Bug { msg } if msg.contains("simulated")), "{err:?}");
    assert_eq!(g.stuck, Some(err.clone()));
    assert_eq!(g.step(&turn(1)).unwrap_err(), err);
    probe::clear_hook();
}

#[test]
fn unknown_sender_maps_to_noop() {
    probe::clear_hook();
    let mut g = game(true);
    let t = Turn {
        turn_number: 0,
        intents: vec![StampedIntent { client_id: "nobody".into(), intent: Intent::Spawn { tile: 5 } }],
        hash: None,
    };
    g.step(&t).unwrap();
    // NoOp is not active during spawn: it waits in uninit.
    assert_eq!(g.state.uninit.len(), 1);
    assert_eq!(g.state.uninit[0].kind, ExecKind::NoOp);
}

#[test]
fn mirv_bind_pin_detach_cancel_unpin() {
    // Tag 1 plays MIRV: spawns warhead 100 at tick 0 (tick pass), detaches it at tick 2 (the
    // warhead deactivates itself), cancels + unpins at tick 3 and clears its list.
    probe::set_hook(|p, cx, ph, t| match (p.tag, ph, t) {
        (1, Phase::Tick, 1) => {
            let k = p.warheads.len() as u32;
            let pending = cx.st.add_execution(ProbeExec::exec(100, true, false));
            p.warheads.push(WarheadRef::Pending(pending));
            cx.st.exec_cmds.push(ExecCmd::BindWarhead { k, pending });
        }
        (100, Phase::Tick, 2) => p.active = false,
        (1, Phase::Tick, 3) => {
            for &w in &p.warheads {
                cx.st.exec_cmds.push(ExecCmd::CancelNuke(w));
            }
            for &w in &p.warheads {
                cx.st.exec_cmds.push(ExecCmd::Unpin(w));
            }
            p.warheads.clear();
        }
        _ => {}
    });
    let mut g = game(false);
    g.step_with_execs(&turn(0), [ProbeExec::exec(1, true, false)]).unwrap();
    g.step(&turn(1)).unwrap();
    let w = probe_id(&g, 100);
    let m = probe_id(&g, 1);
    assert_eq!(g.execs.ar.probe.get(m.idx).warheads, [WarheadRef::Id(w)]);
    assert_eq!(g.execs.ar.holds(w), 2, "listed + pinned");
    g.step(&turn(2)).unwrap();
    // Compacted out of live, still allocated: a detached warhead (CS E row via the MIRV).
    assert_eq!(tags(&g, &g.execs.live), [1]);
    assert!(g.execs.ar.is_allocated(w));
    assert_eq!(g.execs.ar.holds(w), 1);
    probe::take_log();
    g.step(&turn(3)).unwrap();
    assert_eq!(probe::take_log(), ["tick 1@3", "cancel 100"]);
    assert!(!g.execs.ar.is_allocated(w), "last hold dropped");
    probe::clear_hook();
}

#[test]
fn freeing_a_mirv_releases_its_warheads_and_pending_refs_resolve() {
    // The MIRV spawns and, in the same call, cancels a warhead through its Pending ref; a second
    // warhead stays listed until the MIRV itself goes inactive and is freed.
    probe::set_hook(|p, cx, ph, t| match (p.tag, ph, t) {
        (1, Phase::Tick, 1) => {
            for tag in [100, 101] {
                let k = p.warheads.len() as u32;
                let pending = cx.st.add_execution(ProbeExec::exec(tag, true, false));
                p.warheads.push(WarheadRef::Pending(pending));
                cx.st.exec_cmds.push(ExecCmd::BindWarhead { k, pending });
            }
            cx.st.exec_cmds.push(ExecCmd::CancelNuke(p.warheads[0]));
        }
        (1, Phase::Tick, 2) => p.active = false,
        _ => {}
    });
    let mut g = game(false);
    g.step_with_execs(&turn(0), [ProbeExec::exec(1, true, false)]).unwrap();
    g.step(&turn(1)).unwrap();
    let w0 = probe_id(&g, 100);
    let w1 = probe_id(&g, 101);
    assert!(g.execs.ar.probe.get(w0.idx).cancelled);
    // w0: cancelled before its init (TS inits it anyway), then compacted out; still pinned.
    assert!(g.execs.ar.is_allocated(w0) && !g.execs.live.contains(&w0));
    assert_eq!(g.execs.ar.holds(w1), 2);
    g.step(&turn(2)).unwrap();
    // The MIRV is freed: its holds go, so w0 (unlisted) is freed and w1 keeps its listing.
    assert!(!g.execs.ar.is_allocated(w0));
    assert_eq!(g.execs.ar.holds(w1), 1);
    assert_eq!(tags(&g, &g.execs.live), [101]);
    probe::clear_hook();
}

#[test]
fn clone_into_reuses_and_stays_identical() {
    probe::set_hook(|p, cx, ph, t| {
        if p.tag == 1 && ph == Phase::Tick && t % 7 == 0 {
            cx.st.add_execution(ProbeExec::exec(1000 + t as u32, t % 2 == 0, false));
        }
    });
    let mut a = game(false);
    a.step_with_execs(&turn(0), [ProbeExec::exec(1, true, false)]).unwrap();
    for i in 1..50 {
        a.step(&turn(i)).unwrap();
    }
    let mut b = Game::default();
    a.fork_into(&mut b);
    assert_eq!(a.fingerprint64(), b.fingerprint64());
    for i in 50..100 {
        a.step(&turn(i)).unwrap();
        b.step(&turn(i)).unwrap();
        assert_eq!(a.fingerprint64(), b.fingerprint64(), "tick {i}");
        assert_eq!(tags(&a, &a.execs.live), tags(&b, &b.execs.live));
    }
    // Pooled re-fork reuses the live Vec buffer.
    let p = b.execs.live.as_ptr();
    let cap = b.execs.live.capacity();
    a.clone_into(&mut b);
    if a.execs.live.len() <= cap {
        assert_eq!(b.execs.live.as_ptr(), p);
    }
    probe::clear_hook();
}

/// SPEC §3.2.6 exec-boundary hook (request P13 #1): the probe sees every execution the tick
/// pass runs, with its `live` index, after that execution's tick (its `addExecution`s already
/// in `uninit`), and nothing for skipped (inactive) ones.
#[test]
fn step_with_probe_runs_after_each_executed_tick() {
    probe::set_hook(|p, cx, ph, _t| {
        if p.tag == 2 && ph == Phase::Tick {
            cx.st.add_execution(ProbeExec::exec(9, true, false));
        }
    });
    let mut g = game(false);
    g.step_with_execs(
        &turn(0),
        [ProbeExec::exec(1, true, false), ProbeExec::exec(5, false, false), ProbeExec::exec(2, true, false)],
    )
    .unwrap();
    // All three are live after the init pass; tag 5 is inactive and is compacted out.
    assert_eq!(tags(&g, &g.execs.live), [1, 2]);
    let mut seen: Vec<(usize, ExecKind, usize)> = Vec::new();
    g.step_with_probe(&turn(1), |i, k, st| seen.push((i, k, st.uninit.len()))).unwrap();
    // Index 0 = tag 1 (uninit empty), index 1 = tag 2 (its child already drained into uninit).
    assert_eq!(seen, [(0, ExecKind::Probe, 0), (1, ExecKind::Probe, 1)]);
    // A plain step after a probed one is unchanged.
    let ev = g.step(&turn(2)).unwrap();
    assert_eq!(ev.tick, 3);
}
