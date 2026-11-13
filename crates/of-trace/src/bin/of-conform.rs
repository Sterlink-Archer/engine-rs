//! `of-conform` — run a conformance corpus and print first-divergence reports.
//!
//! ```text
//! of-conform smoke  <corpus-or-scenario>... [--json OUT]   pipeline self-test (no engine needed)
//! of-conform run    <corpus-or-scenario>... --engine replay [--inject k:T|sec:T:X|fatal:T]
//!                                           [--max-ticks N] [--json OUT] [--ignore-commit]
//! of-conform check-dumps <corpus-or-scenario>...           native re-digest of every TS dump
//! of-conform info   <scenario>
//! of-conform cat-turns <scenario>                          turns as JSONL
//! of-conform diff-dumps <a.cs.zst> <b.cs.zst>              field diff of two dumps
//! ```
//! Exit status: 0 all pass, 1 a divergence / failed check, 2 usage or I/O error.

use anyhow::{Context, Result, anyhow, bail};
use of_canon::digest::{CanonState, Section};
use of_canon::{Cs, SECTIONS};
use of_trace::conform::{self, Divergence, Options, Trigger};
use of_trace::dumpcheck;
use of_trace::engine::{Engine, Inject, ReplayEngine};
use of_trace::localise;
use of_trace::trace::{self, Trace};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

struct Args {
    cmd: String,
    paths: Vec<PathBuf>,
    json: Option<PathBuf>,
    engine: String,
    inject: Option<String>,
    max_ticks: u32,
    ignore_commit: bool,
}

fn parse() -> Result<Args> {
    let mut it = std::env::args().skip(1);
    let cmd =
        it.next().ok_or_else(|| anyhow!("usage: of-conform <smoke|run|check-dumps|info|cat-turns|diff-dumps> ..."))?;
    let mut a = Args {
        cmd,
        paths: vec![],
        json: None,
        engine: "replay".into(),
        inject: None,
        max_ticks: 0,
        ignore_commit: false,
    };
    while let Some(x) = it.next() {
        match x.as_str() {
            "--json" => a.json = Some(PathBuf::from(it.next().context("--json OUT")?)),
            "--engine" => a.engine = it.next().context("--engine NAME")?,
            "--inject" => a.inject = Some(it.next().context("--inject SPEC")?),
            "--max-ticks" => a.max_ticks = it.next().context("--max-ticks N")?.parse()?,
            "--ignore-commit" => a.ignore_commit = true,
            s if s.starts_with("--") => bail!("unknown flag {s}"),
            p => a.paths.push(PathBuf::from(p)),
        }
    }
    Ok(a)
}

fn scenarios(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut v = vec![];
    for p in paths {
        let s = trace::find_scenarios(p)?;
        if s.is_empty() {
            bail!("no scenario.json under {}", p.display());
        }
        v.extend(s);
    }
    Ok(v)
}

fn parse_inject(s: &str) -> Result<Inject> {
    let parts: Vec<&str> = s.split(':').collect();
    Ok(match parts.as_slice() {
        ["k", t] => Inject::K(t.parse()?),
        ["fatal", t] => Inject::Fatal(t.parse()?),
        ["sec", t, x] => Inject::Section(t.parse()?, Section::from_name(x).ok_or_else(|| anyhow!("section {x}"))?),
        _ => bail!("--inject k:T | sec:T:X | fatal:T"),
    })
}

fn make_engine(name: &str, tr: &Trace, inject: Option<Inject>) -> Result<Box<dyn Engine>> {
    match name {
        "replay" => {
            let mut e = ReplayEngine::new(tr);
            e.inject = inject;
            Ok(Box::new(e))
        }
        // The native engine registers here once it exists (t065), e.g. "native" => Box::new(of_engine::Native::new()).
        other => bail!("unknown engine {other:?} (available: replay)"),
    }
}

fn write_json(p: &Option<PathBuf>, v: &Value) -> Result<()> {
    if let Some(p) = p {
        std::fs::write(p, serde_json::to_string_pretty(v)? + "\n").with_context(|| format!("write {}", p.display()))?;
    }
    Ok(())
}

/// Pipeline self-test on TS-recorded traces. Every check must hold for the suite to be usable
/// before an engine exists.
fn smoke(a: &Args) -> Result<bool> {
    let mut all_ok = true;
    let mut rows = vec![];
    for dir in scenarios(&a.paths)? {
        let t0 = std::time::Instant::now();
        let tr = Trace::load(&dir).with_context(|| format!("load {}", dir.display()))?;
        let mut checks: Vec<(String, bool, String)> = vec![];
        let n_sums = if dir.join("SHA256SUMS").exists() { trace::verify_sums(&dir)? } else { 0 };
        checks.push((
            "load: SHA256SUMS + magics + turnsDigest + turn count".into(),
            true,
            format!("{n_sums} files, {} turns, {} K, {} D records", tr.turns.len(), tr.k.len(), tr.sec.len()),
        ));

        // 1. Replay the recorded digests through the real comparison loop.
        let mut e = ReplayEngine::new(&tr);
        let r = conform::run(&tr, &mut e, &Options::default());
        checks.push((
            "replay: recorded digests vs themselves -> PASS".into(),
            r.ok() && r.k_checked as usize == tr.k.len() && r.d_checked as usize == tr.sec.len(),
            r.render(),
        ));

        // 2. Injected divergences are found at exactly the injected tick with the right trigger.
        let mid = tr.ticks / 2;
        let sec_tick = *tr.sec.keys().find(|t| **t > 0).unwrap_or(&0);
        for (inj, want_tick, want_trig) in [
            (Inject::K(mid), mid, Trigger::K),
            (Inject::Section(sec_tick, Section::E), sec_tick, Trigger::KBlind),
            (Inject::Fatal(mid), mid + 1, Trigger::Fatal),
        ] {
            let mut e = ReplayEngine::new(&tr).with_inject(inj);
            let r = conform::run(&tr, &mut e, &Options::default());
            let ok = r.divergence.as_ref().is_some_and(|d| d.tick == want_tick && d.trigger == want_trig)
                && (!matches!(inj, Inject::Section(..))
                    || r.divergence.as_ref().is_some_and(|d| d.sections.len() == 1 && d.sections[0].sec == "E"));
            checks.push((
                format!("inject {inj:?} -> first bad tick {want_tick} trigger {want_trig:?}"),
                ok,
                r.render(),
            ));
        }

        // 3. Native re-digest of every dump == TS digests (of-canon end-to-end vs TS).
        for dc in dumpcheck::check_dumps(&tr)? {
            checks.push((
                format!("dump {}: native D/K/sections == TS, storage round-trip", dc.file),
                dc.ok(),
                format!(
                    "D {:016x} d_ok={} sec_ok={:?} bad={:?} k_ok={:?} roundtrip={} body={}B {}ms",
                    dc.d, dc.d_ok, dc.sec_ok, dc.bad_sections, dc.k_ok, dc.roundtrip_ok, dc.body_bytes, dc.ms
                ),
            ));
        }

        // 4. Localisation: corrupt one field of the last dump and the diff must name it.
        if let Some(info) = tr.dumps.last() {
            let dump = trace::read_dump(&dir.join(&info.file))?;
            let exp = trace::dump_state(&dump)?;
            let (act, want_entity, want_path) = corrupt_gold(&exp)?;
            let mut d = Divergence {
                tick: info.tick,
                trigger: Trigger::K,
                message: "localisation self-test".into(),
                expected: None,
                actual: None,
                last_good: None,
                bracket_dump: None,
                sections: vec![],
                fields: vec![],
            };
            conform::fill_localisation(&exp, &act, &mut d);
            let ok = d.sections.len() == 1
                && d.sections[0].sec == "P"
                && d.fields.len() == 1
                && d.fields[0].entity == want_entity
                && d.fields[0].path == want_path;
            checks.push((
                format!("localise: gold+1n at {want_entity}{want_path} -> exactly that field"),
                ok,
                d.fields.iter().map(|f| f.render()).collect::<Vec<_>>().join("; "),
            ));
            let (act2, ntiles) = corrupt_tile_owner(&exp)?;
            let f2 = localise::diff_states(&exp, &act2, 50);
            let ok2 =
                f2.first().is_some_and(|f| f.sec == "T" && f.path.contains(&format!("total differing tiles {ntiles}")));
            checks.push((
                "localise: one raw owner flip in T -> (x,y) tile report".into(),
                ok2,
                f2.first().map(|f| f.render()).unwrap_or_default(),
            ));
        }

        println!("== {} ({})  commit {}", tr.id, dir.display(), tr.engine_commit());
        for (name, ok, detail) in &checks {
            println!("  [{}] {name}\n        {}", if *ok { "ok" } else { "FAIL" }, detail.replace('\n', "\n        "));
            all_ok &= ok;
        }
        println!("  {} ms", t0.elapsed().as_millis());
        rows.push(json!({"scenario": tr.id, "dir": dir, "checks": checks.iter().map(|(n, o, d)| json!({"check": n, "ok": o, "detail": d})).collect::<Vec<_>>()}));
    }
    write_json(
        &a.json,
        &json!({"tool": "of-conform smoke", "arch": std::env::consts::ARCH, "ok": all_ok, "scenarios": rows}),
    )?;
    println!("SMOKE {}", if all_ok { "PASS" } else { "FAIL" });
    Ok(all_ok)
}

fn corrupt_gold(cs: &CanonState) -> Result<(CanonState, String, String)> {
    let mut act = cs.clone();
    let Cs::Arr(rows) = act.get_mut(Section::P) else { bail!("P not an array") };
    let row = rows.first_mut().ok_or_else(|| anyhow!("no players"))?;
    let id = row.get("smallID").and_then(Cs::as_i64).unwrap_or(-1);
    match row.get_mut("gold") {
        Some(Cs::Big(g)) => *g += 1,
        _ => bail!("P[0].gold not a bigint"),
    }
    Ok((act, format!("P[smallID={id}]"), ".gold".into()))
}

fn corrupt_tile_owner(cs: &CanonState) -> Result<(CanonState, usize)> {
    let mut act = cs.clone();
    match act.get_mut(Section::T).get_mut("map") {
        Some(Cs::Typed(of_canon::Typed::U16(m))) if !m.is_empty() => {
            let i = m.len() / 2;
            m[i] ^= 1;
        }
        _ => bail!("T.map missing"),
    }
    Ok((act, 1))
}

fn run_cmd(a: &Args) -> Result<bool> {
    let inject = a.inject.as_deref().map(parse_inject).transpose()?;
    let mut all_ok = true;
    let mut reps = vec![];
    for dir in scenarios(&a.paths)? {
        let tr = Trace::load(&dir).with_context(|| format!("load {}", dir.display()))?;
        let mut e = make_engine(&a.engine, &tr, inject)?;
        let r = conform::run(&tr, e.as_mut(), &Options { max_ticks: a.max_ticks, ignore_commit: a.ignore_commit });
        println!("{}", r.render());
        all_ok &= r.ok();
        reps.push(r.to_json(tr.engine_commit()));
    }
    write_json(&a.json, &json!({"tool": "of-conform run", "engine": a.engine, "ok": all_ok, "reports": reps}))?;
    println!("RUN {}", if all_ok { "PASS" } else { "DIVERGED" });
    Ok(all_ok)
}

fn check_dumps_cmd(a: &Args) -> Result<bool> {
    let mut ok = true;
    for dir in scenarios(&a.paths)? {
        let tr = Trace::load(&dir)?;
        for dc in dumpcheck::check_dumps(&tr)? {
            println!(
                "{} {} t{}: {} D={:016x} K={:016x} sec_ok={:?} bad={:?} k_ok={:?} roundtrip={} {}ms\n    {}",
                tr.id,
                dc.file,
                dc.tick,
                if dc.ok() { "ok" } else { "FAIL" },
                dc.d,
                dc.k,
                dc.sec_ok,
                dc.bad_sections,
                dc.k_ok,
                dc.roundtrip_ok,
                dc.ms,
                dc.summary
            );
            ok &= dc.ok();
        }
    }
    Ok(ok)
}

fn info(a: &Args) -> Result<bool> {
    for dir in scenarios(&a.paths)? {
        let tr = Trace::load(&dir)?;
        let n_int: usize =
            tr.turns.iter().map(|t| t.get("intents").and_then(Cs::as_arr).map(|v| v.len()).unwrap_or(0)).sum();
        println!(
            "{}: commit {} ticks {} kStride {} secEvery {} K {} D {} dumps {:?} intents {} fatal {:?} final {}",
            tr.id,
            tr.engine_commit(),
            tr.ticks,
            tr.k_stride,
            tr.sec_every,
            tr.k.len(),
            tr.sec.len(),
            tr.dumps.iter().map(|d| d.tick).collect::<Vec<_>>(),
            n_int,
            tr.fatal,
            tr.scenario["final"]
        );
        if let Some(r) = tr.sec.values().next_back() {
            for s in SECTIONS {
                print!("{}={:016x} ", s.name(), r.sec[s as usize]);
            }
            println!("(t{})", r.tick);
        }
    }
    Ok(true)
}

fn cat_turns(a: &Args) -> Result<bool> {
    for dir in scenarios(&a.paths)? {
        let tr = Trace::load(&dir)?;
        for t in &tr.turns {
            println!("{}", trace::cs_to_json(t));
        }
    }
    Ok(true)
}

fn diff_dumps(a: &Args) -> Result<bool> {
    if a.paths.len() != 2 {
        bail!("diff-dumps A B");
    }
    let load = |p: &Path| -> Result<CanonState> { trace::dump_state(&trace::read_dump(p)?) };
    let (e, x) = (load(&a.paths[0])?, load(&a.paths[1])?);
    let (de, dx) = (of_canon::digest::digest_full(&e)?, of_canon::digest::digest_full(&x)?);
    for s in conform::section_diffs(&de.sec, &dx.sec) {
        println!("section {} ({}) {:016x} vs {:016x}", s.sec, s.class, s.expected, s.actual);
    }
    let f = localise::diff_states(&e, &x, 50);
    for d in &f {
        println!("{}", d.render());
    }
    Ok(f.is_empty())
}

fn main() {
    let r = parse().and_then(|a| match a.cmd.as_str() {
        "smoke" => smoke(&a),
        "run" => run_cmd(&a),
        "check-dumps" => check_dumps_cmd(&a),
        "info" => info(&a),
        "cat-turns" => cat_turns(&a),
        "diff-dumps" => diff_dumps(&a),
        c => Err(anyhow!("unknown command {c}")),
    });
    match r {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(e) => {
            eprintln!("of-conform: {e:#}");
            std::process::exit(2);
        }
    }
}
