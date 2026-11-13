//! `fork_identity` (FILE_MAP §4 P13 mandatory design test; ARCHITECTURE §10, §12 G12): random
//! fork points and GC at random points over 200 ticks, **K-equal every tick** between the
//! source and each fork (pooled `clone_into`/`fork_into` targets and a fresh `clone`).
//!
//! The comparison digest is `Game::canon_k` (the real K_t) when the exporter can produce it,
//! and otherwise — until the row owners' `Canon` impls (P2 Player/Unit, P5 RailNetwork, P1
//! NoOp, …) land — the **section K** of everything P13 exports itself: the K projections of
//! G (with `execTypes`), T, M and W, combined like K. Both sides must agree on which digest
//! they produce (a fork that could and a source that could not would be a divergence too).
//!
//! The world is the S0 one (`fork_identity.rs`): the empty state with NoOp executions
//! injected at pseudo-random ticks; the spawn phase ends at tick 20, after which the NoOps are
//! inited and compacted. GC runs on one side only, at random ticks: it must be unobservable.
//! The water-nuke variant needs a water map, P3/P4a/P8 and the owners' impls: it is an open
//! item of the P13 report (ledger), not a stub here.
//!
//! Owner: **P13**.

use of_canon::Cs;
use of_engine::Game;
use of_engine::api::intents::Turn;
use of_engine::canon::sections::{g_section, m_section, t_section};
use of_engine::canon::{CanonCx, CanonMode};
use of_engine::ctx::Ctx;
use of_engine::exec::Exec;
use of_engine::exec::noop::NoOpExec;
use of_engine::scratch::Scratch;
use of_engine::state::State;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn extras(t: u64) -> Vec<Exec> {
    let mut r = Rng(0x9e37_79b9_7f4a_7c15 ^ t.wrapping_mul(0x2545_f491_4f6c_dd1d));
    (0..r.next() % 3).map(|_| Exec::NoOp(NoOpExec::new())).collect()
}

fn step(g: &mut Game, t: u64) {
    if t == 20 {
        let mut sc = Scratch::new();
        Ctx::new(&mut g.state, &mut sc, None).end_spawn_phase();
    }
    g.step_with_execs(&Turn::empty(t), extras(t)).expect("step");
}

/// The section K: G (with `execTypes` of the listed executions), T, M and W in their K
/// projection, combined with the K magic.
fn section_k(g: &Game) -> u64 {
    let mut cx = CanonCx::with_mode(&g.state, &g.execs, CanonMode::Key);
    let out = g_section(&mut cx);
    assert!(cx.errors.is_empty(), "{:?}", cx.errors);
    let mut gf = out.fields;
    gf.push(("execTypes".into(), Cs::Arr(cx.exec_order.iter().map(|id| Cs::Str(id.kind.as_str().into())).collect())));
    let w = Cs::Obj(vec![("water".into(), out.water), ("sharedWaterCache".into(), out.shared_water)]);
    let mut cs = of_canon::CanonState { tick: g.state.ticks as u32, sec: std::array::from_fn(|_| Cs::Null) };
    cs.sec[0] = Cs::Obj(gf);
    cs.sec[1] = t_section(&g.state, CanonMode::Key);
    cs.sec[2] = m_section(&g.state);
    cs.sec[8] = w;
    let k = of_canon::digest::key_projection(&cs);
    of_engine::canon::export::digest_k_projected(&k).expect("digest").all
}

/// `(K or the reason it is not available yet, section K)`.
fn digests(g: &Game) -> (Result<u64, String>, u64) {
    (g.canon_k().map_err(|e| e.0), section_k(g))
}

#[test]
fn random_forks_and_gc_keep_k_equal_for_200_ticks() {
    let mut src = Game::from_state(State::default(), "p13fork");
    let mut r = Rng(0xdead_beef_cafe_f00d);
    // Three fork points in (0, 200); a pooled target that already holds a larger, unrelated
    // game, one fork_into target and one fresh clone.
    let mut forks: Vec<(String, Game)> = Vec::new();
    let mut fork_at: Vec<u64> = (0..3).map(|_| 1 + r.next() % 150).collect();
    of_engine::of_det::js_sort_by(&mut fork_at, |a, b| *a as f64 - *b as f64);
    let mut real_k = 0usize;
    for t in 0..200u64 {
        if fork_at.contains(&t) {
            let mut pooled = Game::from_state(State::default(), "other");
            for u in 0..60 {
                pooled.step_with_execs(&Turn::empty(u), [Exec::NoOp(NoOpExec::new())]).ok();
            }
            src.clone_into(&mut pooled);
            forks.push((format!("pooled@{t}"), pooled));
            let mut target = Game::from_state(State::default(), "x");
            src.fork_into(&mut target); // GC on the source first
            forks.push((format!("fork_into@{t}"), target));
            forks.push((format!("clone@{t}"), src.clone()));
        }
        step(&mut src, t);
        for (_, f) in forks.iter_mut() {
            step(f, t);
            // GC at random points on the fork only (unobservable by construction).
            if r.next().is_multiple_of(7) {
                f.state.collect_garbage(&f.execs);
            }
        }
        let want = digests(&src);
        if want.0.is_ok() {
            real_k += 1;
        }
        for (name, f) in &forks {
            assert_eq!(digests(f), want, "{name} diverged from the source after tick {t}");
            assert_eq!(f.fingerprint64(), src.fingerprint64(), "{name}: fingerprint64 after tick {t}");
        }
    }
    assert_eq!(src.state.ticks, 200);
    assert_eq!(forks.len(), 9);
    eprintln!("fork_identity_k: 200 ticks, 9 forks, real K on {real_k} ticks (section K on all)");
}
