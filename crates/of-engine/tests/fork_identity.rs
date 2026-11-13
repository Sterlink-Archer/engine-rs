//! `fork_identity` skeleton (FILE_MAP §3 S0 step 5; ARCHITECTURE §10, §12 G12): a fork and its
//! source fed the same turns stay identical, whether the fork is pooled (`clone_into` /
//! `fork_into` into a dirty `Game`) or fresh (`clone`), and whatever GC runs on one side only.
//!
//! At S0 the world is the empty one of `empty_world.rs` plus NoOp executions injected at
//! pseudo-random ticks (in the spawn phase they queue in `uninit`; after it they are inited
//! and compacted), so the comparison is `fingerprint64`. P13 extends this to K digests
//! (`canonical_state`), random fork + GC points over 200 ticks on real scenarios, and the
//! water-nuke variant (FILE_MAP §4 P13 row).

use of_engine::Game;
use of_engine::api::intents::Turn;
use of_engine::ctx::Ctx;
use of_engine::exec::Exec;
use of_engine::exec::noop::NoOpExec;
use of_engine::exec::win_check::WinCheckExec;
use of_engine::scratch::Scratch;
use of_engine::state::State;

/// Deterministic xorshift for the injection schedule.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// The executions injected at tick `t` (same for every copy).
fn extras(t: u64) -> Vec<Exec> {
    let mut r = Rng(0x9e37_79b9_7f4a_7c15 ^ t.wrapping_mul(0x2545_f491_4f6c_dd1d));
    (0..r.next() % 3).map(|_| Exec::NoOp(NoOpExec::new())).collect()
}

fn step(g: &mut Game, t: u64) {
    g.step_with_execs(&Turn::empty(t), extras(t)).expect("step");
}

fn end_spawn_phase(g: &mut Game) {
    let mut sc = Scratch::new();
    Ctx::new(&mut g.state, &mut sc, None).end_spawn_phase();
}

/// A game at tick `n < 20`, in the spawn phase, with WinCheck waiting in `uninit`.
fn run_to(n: u64) -> Game {
    assert!(n < 20, "WinCheck's init is S1 work: stay in the spawn phase");
    let mut g = Game::from_state(State::default(), "s0fork1");
    let init = [Exec::WinCheck(WinCheckExec { active: false, initialized: false, checked_ranked_spawns: false })];
    g.step_with_execs(&Turn::empty(0), init).expect("tick 0");
    for t in 1..n {
        step(&mut g, t);
    }
    g
}

#[test]
fn fork_at_50_matches_for_50_ticks() {
    // NoOps only: the spawn phase ends at tick 20, after which any queued S1 execution
    // (WinCheck, …) would be inited, and its `init` is still a `todo!()`.
    let mut src = Game::from_state(State::default(), "s0fork1");
    step(&mut src, 0);
    for t in 1..50 {
        if t == 20 {
            end_spawn_phase(&mut src);
        }
        step(&mut src, t);
    }
    assert_eq!(src.state.ticks, 50);

    // Pooled target that already holds an unrelated, larger game.
    let mut pooled = run_to(5);
    for t in 0..80 {
        pooled.step_with_execs(&Turn::empty(t), [Exec::NoOp(NoOpExec::new())]).ok();
    }
    src.fork_into(&mut pooled);
    let mut fresh = src.clone();
    assert_eq!(src.fingerprint64(), pooled.fingerprint64());
    assert_eq!(src.fingerprint64(), fresh.fingerprint64());

    let mut gc = Rng(7);
    for t in 50..100 {
        step(&mut src, t);
        step(&mut pooled, t);
        step(&mut fresh, t);
        // GC at random points on one side only: unobservable.
        if gc.next() & 3 == 0 {
            let execs = std::mem::take(&mut pooled.execs);
            pooled.state.collect_garbage(&execs);
            pooled.execs = execs;
        }
        let f = src.fingerprint64();
        assert_eq!(f, pooled.fingerprint64(), "pooled fork diverged at tick {t}");
        assert_eq!(f, fresh.fingerprint64(), "fresh fork diverged at tick {t}");
        assert_eq!(src.state.uninit, pooled.state.uninit);
        assert_eq!(src.execs.live, fresh.execs.live);
    }
    assert_eq!(src.state.ticks, 100);
}

#[test]
fn spawn_phase_fork_keeps_uninit_queue() {
    // Forks taken while executions wait in `uninit` (spawn phase) carry the queue.
    let mut src = run_to(10);
    let n = src.state.uninit.len();
    assert!(n >= 1, "WinCheck waits in uninit");
    let mut b = Game::default();
    src.clone_into(&mut b);
    for t in 10..20 {
        step(&mut src, t);
        step(&mut b, t);
        assert_eq!(src.fingerprint64(), b.fingerprint64(), "tick {t}");
    }
    assert!(src.state.uninit.len() >= n);
    assert_eq!(src.state.uninit, b.state.uninit);
}
