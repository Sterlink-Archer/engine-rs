//! `engine-rs bench` and `engine-rs fork-bench` (ARCHITECTURE §8.3 gate t091, §12 G1/G2).
//! Owner: **P13**.
//!
//! - `bench`: steps a scenario's turns and reports ticks/s, per-step p50/p95 and allocations
//!   per tick (the G2 numbers; TS reference 194–302 t/s on World, `docs/recon/06_benchmark.md`).
//! - `fork-bench`: at each requested tick, the GC time, then `reps` **pooled** forks
//!   (`src.clone_into(&mut pooled)`, ARCHITECTURE §8.1 item 5) and `reps` **unpooled** forks
//!   (`src.clone()`), each with wall time, allocations, bytes and page faults; plus the bytes
//!   and allocations of every state component (the §8.3 table, rail and water included).
//!   Gate: pooled p95 ≤ 5 ms and pooled allocations p95 ≤ 2,000 (§8.3).
//!
//! Allocation figures need the counting allocator the `engine-rs` binary installs
//! ([`crate::alloc`]); without it (library tests) they read 0 and `allocCounter` is `false`.

use crate::alloc::{AllocCount, cpu_seconds, installed, nonvoluntary_switches, page_faults};
use crate::engine::{OfEngine, adapter_guard};
use of_engine::Game;
use of_engine::exec::ExecArenas;
use of_trace::engine::Engine;
use of_trace::trace::Trace;
use serde_json::{Value, json};
use std::time::Instant;

/// `q`-quantile (nearest rank) of `v` (sorted in place).
pub fn quantile(v: &mut [f64], q: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    of_engine::of_det::js_sort_by(v, |a, b| a - b);
    let i = ((q * v.len() as f64).ceil() as usize).clamp(1, v.len()) - 1;
    v[i]
}

fn stats(mut v: Vec<f64>) -> Value {
    let mean = if v.is_empty() { f64::NAN } else { v.iter().sum::<f64>() / v.len() as f64 };
    json!({ "p50": quantile(&mut v, 0.5), "p95": quantile(&mut v, 0.95), "mean": mean, "max": v.last().copied().unwrap_or(f64::NAN) })
}

/// What one measured fork cost.
#[derive(Clone, Copy, Debug, Default)]
struct Sample {
    ms: f64,
    allocs: u64,
    bytes: u64,
    minflt: u64,
    majflt: u64,
}

fn measure(f: impl FnOnce()) -> Sample {
    let pf0 = page_faults().unwrap_or((0, 0));
    let a0 = AllocCount::now();
    let t0 = Instant::now();
    f();
    let ms = t0.elapsed().as_secs_f64() * 1e3;
    let a = AllocCount::now().since(a0);
    let pf = page_faults().unwrap_or((0, 0));
    Sample { ms, allocs: a.allocs, bytes: a.bytes, minflt: pf.0 - pf0.0, majflt: pf.1 - pf0.1 }
}

fn summarise(s: &[Sample]) -> Value {
    json!({
        "ms": stats(s.iter().map(|x| x.ms).collect()),
        "allocs": stats(s.iter().map(|x| x.allocs as f64).collect()),
        "bytes": stats(s.iter().map(|x| x.bytes as f64).collect()),
        "minorFaults": stats(s.iter().map(|x| x.minflt as f64).collect()),
        "majorFaults": stats(s.iter().map(|x| x.majflt as f64).collect()),
    })
}

/// Bytes and allocations of cloning `x` (≈ its heap footprint; the measured clone is dropped
/// outside the count).
fn clone_cost<T: Clone>(x: &T) -> Value {
    let a0 = AllocCount::now();
    let c = x.clone();
    let a = AllocCount::now().since(a0);
    drop(c);
    json!({ "bytes": a.bytes, "allocs": a.allocs })
}

macro_rules! exec_components {
    ($ar:expr, $out:expr; $($f:ident),* $(,)?) => {{
        $(
            if !$ar.$f.is_empty() {
                $out.insert(format!("execs.{}", stringify!($f)), clone_cost(&$ar.$f));
            }
        )*
    }};
}

/// The per-component table of ARCHITECTURE §8.3 (state fields, then non-empty exec arenas).
pub fn component_bytes(g: &Game) -> Value {
    let st = &g.state;
    let mut m = serde_json::Map::new();
    let mut put = |k: &str, v: Value| {
        m.insert(k.to_owned(), v);
    };
    put("map", clone_cost(&st.map));
    put("mini", clone_cost(&st.mini));
    put("pos_tiles", clone_cost(st.pos_tiles()));
    put("pos_border", clone_cost(st.pos_border()));
    put("players", clone_cost(&st.players));
    put("units", clone_cost(&st.units));
    put("unit_by_id", clone_cost(&st.unit_by_id));
    put("unit_grid", clone_cost(&st.unit_grid));
    put("attacks", clone_cost(&st.attacks));
    put("alliances", clone_cost(&st.alliances));
    put("alliance_reqs", clone_cost(&st.alliance_reqs));
    put("rail", clone_cost(&st.rail));
    put("water", clone_cost(&st.water));
    put("shared_water", clone_cost(&st.shared_water));
    put("stats", clone_cost(&st.stats));
    put("units_by_type_memo", clone_cost(&st.units_by_type_memo));
    put("plan_driven_unit_ids", clone_cost(&st.plan_driven_unit_ids));
    put("uninit", clone_cost(&st.uninit));
    put("execs.live", clone_cost(&g.execs.live));
    let ar: &ExecArenas = &g.execs.ar;
    exec_components!(ar, m;
        alliance_extension, alliance_reject, alliance_request, attack, boat_retreat, break_alliance, city,
        construction, defense_post, delete_unit, donate_gold, donate_troops, doomsday_clock, embargo_all, embargo,
        emoji, factory, mark_disconnected, mirv, missile_silo, move_warship, nation, noop, nuke, pause, player, port,
        quick_chat, recompute_rail_cluster, retreat, sam_launcher, sam_missile, shell, spawn, spawn_timer,
        target_player, trade_ship, train, train_station, transport_ship, tribe, upgrade_structure, warship, win_check,
    );
    m.insert("total".into(), clone_cost(g));
    Value::Object(m)
}

/// Fork costs of `src` at its current tick: GC, `reps` pooled forks into a warm target,
/// `reps` unpooled clones, and the component table.
pub fn fork_bench_game(src: &mut Game, reps: usize) -> Value {
    let gc = measure(|| {
        src.state.collect_garbage(&src.execs);
    });
    let mut pooled = src.clone(); // warm target (every buffer already sized)
    let mut p = Vec::with_capacity(reps);
    for _ in 0..reps {
        p.push(measure(|| src.clone_into(&mut pooled)));
    }
    let mut u = Vec::with_capacity(reps);
    for _ in 0..reps {
        let mut hold: Option<Game> = None;
        u.push(measure(|| hold = Some(src.clone())));
        drop(hold);
    }
    let pooled_s = summarise(&p);
    let mut ms: Vec<f64> = p.iter().map(|x| x.ms).collect();
    let mut al: Vec<f64> = p.iter().map(|x| x.allocs as f64).collect();
    let (p95_ms, p95_allocs) = (quantile(&mut ms, 0.95), quantile(&mut al, 0.95));
    json!({
        "tick": src.state.ticks,
        "reps": reps,
        "allocCounter": installed(),
        "gc": { "ms": gc.ms, "allocs": gc.allocs },
        "pooled": pooled_s,
        "unpooled": summarise(&u),
        "components": component_bytes(src),
        "gate": {
            "pooledP95Ms": p95_ms, "pooledP95MsMax": 5.0,
            "pooledP95Allocs": p95_allocs, "pooledP95AllocsMax": 2000,
            "pass": p95_ms <= 5.0 && (!installed() || p95_allocs <= 2000.0),
        },
    })
}

/// Step `eng` through `trace` up to tick `to` (from its current step count).
fn step_to(eng: &mut OfEngine, trace: &Trace, to: u32) -> Result<(), String> {
    while eng.tick() < to {
        let t = eng.tick() as usize;
        let turn = trace.turns.get(t).ok_or_else(|| format!("the trace has only {} turns", trace.turns.len()))?;
        eng.step(turn).map_err(|e| format!("step {t}: {e:?}"))?;
    }
    Ok(())
}

/// `fork-bench --scenario`: fork costs at each tick of `at` (clamped to the trace).
pub fn fork_bench_trace(eng: &mut OfEngine, trace: &Trace, at: &[u32], reps: usize) -> Result<Value, String> {
    eng.start(trace).map_err(|e| format!("start: {e:?}"))?;
    let mut out = Vec::new();
    for &t in at {
        let t = t.min(trace.ticks);
        step_to(eng, trace, t)?;
        let g = eng.game_mut().ok_or("no game")?;
        let r = adapter_guard("fork-bench", || fork_bench_game(g, reps)).map_err(|e| format!("{e:?}"))?;
        out.push(r);
    }
    Ok(json!({ "scenario": trace.id, "points": out }))
}

/// `bench --scenario`: wall time per step over the trace (no digests), allocations per tick.
pub fn bench_trace(eng: &mut OfEngine, trace: &Trace, max_ticks: u32) -> Result<Value, String> {
    eng.start(trace).map_err(|e| format!("start: {e:?}"))?;
    let horizon = if max_ticks > 0 { max_ticks.min(trace.ticks) } else { trace.ticks };
    let mut per = Vec::with_capacity(horizon as usize);
    // OF_SAMPLE_OUT=<file>: statistical profile of the stepping loop (feature `sampler`, t066).
    let sample_out = std::env::var_os("OF_SAMPLE_OUT").map(std::path::PathBuf::from);
    let sampler = match &sample_out {
        Some(_) => Some(crate::sampler::Sampler::start(1000, 64 << 20)?),
        None => None,
    };
    let a0 = AllocCount::now();
    let (c0, nv0) = (cpu_seconds().unwrap_or(f64::NAN), nonvoluntary_switches().unwrap_or(0));
    let t0 = Instant::now();
    for t in 0..horizon as usize {
        let s = Instant::now();
        // A TS tick throw is part of the workload (the game stays steppable); stop on anything else.
        match eng.step(&trace.turns[t]) {
            Ok(()) | Err(of_trace::engine::StepError::Fatal(_)) => {}
            Err(e) => return Err(format!("step {t}: {e:?}")),
        }
        per.push(s.elapsed().as_secs_f64() * 1e3);
    }
    let secs = t0.elapsed().as_secs_f64();
    let cpu = cpu_seconds().unwrap_or(f64::NAN) - c0;
    let nv = nonvoluntary_switches().unwrap_or(0).saturating_sub(nv0);
    let a = AllocCount::now().since(a0);
    let samples = match (sampler, &sample_out) {
        (Some(s), Some(p)) => s.stop(p).map_err(|e| format!("{}: {e}", p.display()))?,
        _ => 0,
    };
    let n = per.len().max(1) as f64;
    Ok(json!({
        "scenario": trace.id,
        "ticks": per.len(),
        "seconds": secs,
        "ticksPerSecond": per.len() as f64 / secs,
        "cpuSeconds": cpu,
        "ticksPerCpuSecond": per.len() as f64 / cpu,
        "nonvoluntaryCtxSwitches": nv,
        "stepMs": stats(per),
        "allocCounter": installed(),
        "allocsPerTick": a.allocs as f64 / n,
        "bytesPerTick": a.bytes as f64 / n,
        "tsReferenceTicksPerSecondWorld": [194, 302],
        "profileSamples": samples,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use of_engine::state::State;

    /// The fork-bench report fields of the t091 gate (FILE_MAP §4 P13 mandatory design test).
    #[test]
    fn fork_bench_report_fields() {
        let mut g = Game::from_state(State::default(), "fb");
        let r = fork_bench_game(&mut g, 5);
        for k in ["tick", "reps", "allocCounter", "gc", "pooled", "unpooled", "components", "gate"] {
            assert!(r.get(k).is_some(), "missing {k}");
        }
        for side in ["pooled", "unpooled"] {
            for k in ["ms", "allocs", "bytes", "minorFaults", "majorFaults"] {
                for q in ["p50", "p95", "mean", "max"] {
                    assert!(r[side][k][q].is_number() || r[side][k][q].is_null(), "{side}.{k}.{q}: {}", r[side][k][q]);
                }
            }
        }
        for k in ["map", "mini", "pos_tiles", "pos_border", "players", "units", "attacks", "rail", "water", "total"] {
            assert!(r["components"][k]["bytes"].is_u64(), "components.{k}");
        }
        assert!(r["gate"]["pass"].is_boolean());
    }

    #[test]
    fn quantiles() {
        let mut v = vec![5.0, 1.0, 3.0, 2.0, 4.0];
        assert_eq!(quantile(&mut v, 0.5), 3.0);
        assert_eq!(quantile(&mut v, 0.95), 5.0);
        assert!(quantile(&mut [], 0.5).is_nan());
    }
}
