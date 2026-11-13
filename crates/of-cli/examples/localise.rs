//! `localise <scenario-dir>...`: coarse localisation of a K divergence (SPEC §3.2 steps 4-5)
//! when the TS trace has no dump at the first bad tick. Owner: **P13** (integrator debug tool,
//! merge1).
//!
//! For each scenario: run the native engine with K checked every tick (as `engine-rs conform`).
//! At the first K mismatch at tick `t`, keep stepping (ignoring K) to the next section
//! checkpoint and report which of the 11 section digests differ there, then on to the next TS
//! dump (if any, within 1000 ticks of `t`) and report the first field diffs against it. The
//! later the tick, the more the states have drifted, so the first differing *section* at the
//! nearest checkpoint is the strongest hint; field diffs at the dump are context.
//! Prints one JSON object per scenario.

use anyhow::{Context, Result, anyhow};
use of_cli::engine::OfEngine;
use of_trace::conform::section_diffs;
use of_trace::engine::Engine;
use of_trace::localise::diff_states;
use of_trace::trace::{self, Trace};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn one(dir: &Path) -> Result<Value> {
    let tr = Trace::load(dir).with_context(|| format!("{}", dir.display()))?;
    let mut e = OfEngine::new();
    e.start(&tr).map_err(|x| anyhow!("start: {x:?}"))?;
    let mut bad: Option<u32> = None;
    let mut out = json!({ "scenario": tr.id });
    for t in 0..=tr.ticks {
        if t > 0 {
            let turn = tr.turns.get((t - 1) as usize).ok_or_else(|| anyhow!("missing turn {}", t - 1))?;
            if let Err(x) = e.step(turn) {
                out["stepError"] = json!({ "tick": t, "error": format!("{x:?}") });
                break;
            }
        }
        if bad.is_none() {
            if let Some(&want) = tr.k.get(&t) {
                let got = match e.key_digest() {
                    Ok(k) => k,
                    Err(x) => {
                        out["kError"] = json!({ "tick": t, "error": format!("{x:?}"), "railAudit": rail_audit(&e) });
                        break;
                    }
                };
                if got != want {
                    bad = Some(t);
                    out["firstBadTick"] = json!(t);
                }
            }
            continue;
        }
        let Some(t0) = bad else { continue };
        if out.get("checkpoint").is_none()
            && let Some(want) = tr.sec.get(&t)
        {
            let got = e.full_digests().map_err(|x| anyhow!("D at {t}: {x:?}"))?;
            let diffs = section_diffs(&want.sec, &got.sec);
            out["checkpoint"] = json!({
                "tick": t,
                "sectionsDiffer": diffs.iter().map(|s| s.sec).collect::<Vec<_>>(),
            });
        }
        if let Some(info) = tr.dumps.iter().find(|d| d.tick == t) {
            let expected = trace::dump_state(&trace::read_dump(&tr.dir.join(&info.file))?)?;
            let actual = e.canonical_state().ok_or_else(|| anyhow!("no CS at {t}"))?;
            // `diff_states` compares CS trees, where a Rust `Num(194.0)` and a TS `Int(194)` differ
            // but encode identically (CSE1 writes a safe-integer NUM as INT, SPEC §1.1): skip those.
            let same_number = |e: &str, a: &str| a.split(' ').next().is_some_and(|x| x == format!("{e}.0"));
            let fields: Vec<Value> = diff_states(&expected, &actual, 400)
                .iter()
                .filter(|f| !same_number(&f.expected, &f.actual))
                .take(15)
                .map(|f| f.to_json())
                .collect();
            out["dump"] = json!({ "tick": t, "fields": fields });
            break;
        }
        if t > t0 + 1000 {
            break;
        }
    }
    Ok(out)
}

/// Every place a `RailroadId` that `railroad_by_id` no longer resolves is still named: station
/// railroad sets / neighbour maps (live and removed stations) and grid cells.
fn rail_audit(e: &OfEngine) -> Value {
    let Some(g) = e.game() else { return Value::Null };
    let rail = &g.state.rail;
    let live = |r: &of_engine::ids::RailroadId| rail.railroad_by_id.contains_key(r);
    let mut hits = Vec::new();
    for (i, st) in rail.station_manager.by_id.iter().enumerate() {
        let Some(st) = st else { continue };
        let listed = rail.station_manager.stations.contains(&st.id);
        for r in st.railroads.iter() {
            if !live(r) {
                hits.push(json!({"where": "station.railroads", "station": i, "listed": listed, "rail": r.0}));
            }
        }
        for (n, r) in st.railroad_by_neighbor.iter() {
            if !live(r) {
                hits.push(json!({"where": "station.railroadByNeighbor", "station": i, "neighbor": n.0, "listed": listed, "rail": r.0}));
            }
        }
    }
    for (c, set) in rail.rail_grid.cells.iter() {
        for r in set.iter() {
            if !live(r) {
                hits.push(json!({"where": "grid.cells", "cell": c, "rail": r.0}));
            }
        }
    }
    Value::Array(hits)
}

fn main() -> Result<()> {
    for a in std::env::args().skip(1) {
        let dir = PathBuf::from(&a);
        match one(&dir) {
            Ok(v) => println!("{v}"),
            Err(x) => println!("{}", json!({ "scenario": a, "error": format!("{x:#}") })),
        }
    }
    Ok(())
}
