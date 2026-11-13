//! `engine-rs`: the of-engine command line (ARCHITECTURE §9, §12 G1/G2). Owner: **P13**.
//!
//! - `conform --scenario <dir> [--json] [--max-ticks N] [--gc-every-tick] [--probe]
//!   [--prng-trace OUT.jsonl [--prng-compare TS.jsonl]]`: the SPEC §3 loop with the native
//!   engine ([`of_cli::engine::OfEngine`]); exit 0 = conformant, 1 = divergence.
//! - `resume --scenario <dir> [--from T | --before T] [--max-ticks N] [--json]`: SPEC §3.2
//!   steps 1–3 / R3 from a resumable dump ([`of_cli::resume`]).
//! - `bench --scenario <dir> [--max-ticks N]`: ticks/s and allocations per tick (G2).
//! - `fork-bench (--scenario <dir> [--at 1000,4000,8000] | --empty) [--reps N]`: pooled and
//!   unpooled fork costs, allocations, page faults, per-component bytes (G1, t091).

use of_cli::alloc::Counting;
use std::path::PathBuf;
use std::process::ExitCode;

#[global_allocator]
static ALLOC: Counting = Counting;

// rules-v36 (t713): the v0.36.0 release commit (openfront.io, 2026-11-09). v0.36.0 descends from the earlier pin
// 5b75bd3 (+48 commits): engine-lib tree be16de51 unchanged, packages/engine f1aae493, engine-api 61772ac9.
const ENGINE_PIN: &str = "0211f17b71efa762242b620b91ffa5d8439522e6";

fn usage() -> String {
    format!(
        "engine-rs {} (of-engine; upstream @{ENGINE_PIN})\n© OpenFront and Contributors (AGPL-3.0). Unofficial Rust port of the OpenFront.io engine; not affiliated with or endorsed by OpenFront Inc.\n\nusage: engine-rs <command>\n\ncommands:\n\
  version     print the version and engine pin\n\
  conform     --scenario <dir> [--json] [--max-ticks N] [--ignore-commit] [--gc-every-tick] [--probe]\n\
              [--prng-trace OUT.jsonl [--prng-compare TS.jsonl]]: SPEC §3 conformance run\n\
  resume      --scenario <dir> [--from T | --before T] [--max-ticks N] [--json]: resume from a dump (R3)\n\
  bench       --scenario <dir> [--max-ticks N]: ticks/s, allocations per tick (G2)\n\
  fork-bench  (--scenario <dir> [--at T,T,..] | --empty) [--reps N]: fork costs (G1, t091)\n",
        env!("CARGO_PKG_VERSION")
    )
}

/// Parsed common arguments.
#[derive(Default)]
struct Args {
    scenario: Option<PathBuf>,
    json: bool,
    max_ticks: u32,
    ignore_commit: bool,
    gc_every_tick: bool,
    probe: bool,
    prng_trace: Option<PathBuf>,
    prng_compare: Option<PathBuf>,
    from: Option<u32>,
    before: Option<u32>,
    at: Vec<u32>,
    reps: usize,
    empty: bool,
}

fn parse(cmd: &str, args: &[String]) -> Result<Args, String> {
    let mut a = Args { reps: 50, at: vec![1000, 4000, 8000], ..Args::default() };
    let mut it = args.iter();
    let num = |v: Option<&String>, f: &str| -> Result<u32, String> {
        v.and_then(|x| x.parse().ok()).ok_or_else(|| format!("{f} needs a number"))
    };
    while let Some(x) = it.next() {
        match x.as_str() {
            "--scenario" => a.scenario = it.next().map(PathBuf::from),
            "--json" => a.json = true,
            "--max-ticks" => a.max_ticks = num(it.next(), "--max-ticks")?,
            "--ignore-commit" => a.ignore_commit = true,
            "--gc-every-tick" => a.gc_every_tick = true,
            "--probe" => a.probe = true,
            "--prng-trace" => a.prng_trace = it.next().map(PathBuf::from),
            "--prng-compare" => a.prng_compare = it.next().map(PathBuf::from),
            "--from" => a.from = Some(num(it.next(), "--from")?),
            "--before" => a.before = Some(num(it.next(), "--before")?),
            "--reps" => a.reps = num(it.next(), "--reps")? as usize,
            "--empty" => a.empty = true,
            "--at" => {
                a.at = it
                    .next()
                    .ok_or("--at needs a list")?
                    .split(',')
                    .map(|t| t.trim().parse().map_err(|_| format!("--at: bad tick {t:?}")))
                    .collect::<Result<_, _>>()?
            }
            other => return Err(format!("engine-rs {cmd}: unknown argument {other:?}")),
        }
    }
    Ok(a)
}

fn load_trace(a: &Args, cmd: &str) -> Result<of_trace::trace::Trace, ExitCode> {
    let Some(dir) = &a.scenario else {
        eprintln!("engine-rs {cmd}: --scenario <dir> is required");
        return Err(ExitCode::from(2));
    };
    of_trace::trace::Trace::load(dir).map_err(|e| {
        eprintln!("engine-rs {cmd}: {}: {e:#}", dir.display());
        ExitCode::from(2)
    })
}

fn conform(a: &Args) -> ExitCode {
    let tr = match load_trace(a, "conform") {
        Ok(t) => t,
        Err(c) => return c,
    };
    if a.probe {
        // SPEC §3.2.6 needs a hook after every `live[i].tick()` inside the tick loop (P1's
        // runner); `of_engine::canon::export::probe_digest` is the digest it will call.
        eprintln!(
            "engine-rs conform --probe: not available yet: needs `Game::step_with_probe` in the tick loop \
             (request P13 → P1, docs/requests/P13.md; ledger t073 for the TS side)"
        );
        return ExitCode::from(2);
    }
    if a.prng_trace.is_some() && !of_cli::prngtrace::ENABLED {
        eprintln!("engine-rs conform --prng-trace: rebuild with `--features prng-trace`");
        return ExitCode::from(2);
    }
    let opt = of_trace::conform::Options { max_ticks: a.max_ticks, ignore_commit: a.ignore_commit };
    let mut eng = of_cli::engine::OfEngine::new();
    eng.gc_every_tick = a.gc_every_tick;
    let rec = a.prng_trace.as_ref().map(|_| {
        let r = of_cli::prngtrace::Recorder::default();
        r.install();
        r
    });
    eng.prng = rec.clone();
    let rep = of_trace::conform::run(&tr, &mut eng, &opt);
    let mut code = if rep.ok() { ExitCode::SUCCESS } else { ExitCode::from(1) };
    let mut v = rep.to_json(ENGINE_PIN);
    if let (Some(rec), Some(out)) = (&rec, &a.prng_trace) {
        rec.uninstall();
        let draws = rec.take();
        if let Err(e) = of_cli::prngtrace::write_jsonl(out, &draws) {
            eprintln!("engine-rs conform: {}: {e}", out.display());
        }
        v["prngDraws"] = draws.len().into();
        if let Some(ts) = &a.prng_compare {
            match of_cli::prngtrace::read_jsonl(ts) {
                Ok(t) => {
                    let d = of_cli::prngtrace::first_divergence_per_tag(&t, &draws);
                    if d.as_object().is_some_and(|m| !m.is_empty()) {
                        code = ExitCode::from(1);
                    }
                    v["prngDivergence"] = d;
                }
                Err(e) => eprintln!("engine-rs conform: {}: {e}", ts.display()),
            }
        }
    }
    if a.json {
        println!("{v}");
    } else {
        println!("{}", rep.render());
        if let Some(d) = v.get("prngDivergence") {
            println!("  prng first divergence per tag: {d}");
        }
    }
    code
}

fn resume(a: &Args) -> ExitCode {
    let tr = match load_trace(a, "resume") {
        Ok(t) => t,
        Err(c) => return c,
    };
    let mut eng = of_cli::engine::OfEngine::new();
    let o = of_cli::resume::ResumeOpts { from: a.from, before: a.before, max_ticks: a.max_ticks };
    let rep = of_cli::resume::resume(&tr, &mut eng, &of_cli::engine::default_upstream_dir(), &o);
    if a.json {
        println!("{}", of_cli::resume::report_json(&rep, ENGINE_PIN));
    } else {
        println!("{}", rep.render());
    }
    if rep.ok() { ExitCode::SUCCESS } else { ExitCode::from(1) }
}

fn bench(a: &Args) -> ExitCode {
    let tr = match load_trace(a, "bench") {
        Ok(t) => t,
        Err(c) => return c,
    };
    let mut eng = of_cli::engine::OfEngine::new();
    match of_cli::bench::bench_trace(&mut eng, &tr, a.max_ticks) {
        Ok(v) => {
            println!("{v}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("engine-rs bench: {e}");
            ExitCode::from(1)
        }
    }
}

fn fork_bench(a: &Args) -> ExitCode {
    if a.empty {
        let mut g = of_engine::Game::from_state(of_engine::state::State::default(), "fork-bench");
        println!("{}", of_cli::bench::fork_bench_game(&mut g, a.reps.max(1)));
        return ExitCode::SUCCESS;
    }
    let tr = match load_trace(a, "fork-bench") {
        Ok(t) => t,
        Err(c) => return c,
    };
    let mut eng = of_cli::engine::OfEngine::new();
    match of_cli::bench::fork_bench_trace(&mut eng, &tr, &a.at, a.reps.max(1)) {
        Ok(v) => {
            println!("{v}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("engine-rs fork-bench: {e}");
            ExitCode::from(1)
        }
    }
}

/// ARCHITECTURE §4.6: a quiet panic hook. A `TsThrow` payload is a TS-equivalent throw that
/// the stepper (`NOT_FOUND`) or `Game::step` (`Fatal::Tick`) catches and reports, so it is not
/// printed; every other panic (a Rust bug, an S1 `todo!()`) keeps the default report.
fn install_quiet_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if info.payload().is::<of_engine::ctx::TsThrow>() {
            return;
        }
        default(info);
    }));
}

/// rules-v35 (t492; carried to rules-v36, t713): `legacy-hash --scenario <dir>`: replay the trace's turns and print, one JSON line per hash
/// tick, the `GameImpl.hash()` value the browser sends every 10 ticks (`{"tick": T, "hash": H}`; T is the tick that
/// `executeNextTick` was running, i.e. the state after T + 1 steps). For checking our shadow against the hashes in
/// real openfront.io game records.
fn legacy_hash(a: &Args) -> ExitCode {
    use of_trace::engine::Engine;
    let tr = match load_trace(a, "legacy-hash") {
        Ok(t) => t,
        Err(c) => return c,
    };
    let mut eng = of_cli::engine::OfEngine::new();
    if let Err(e) = eng.start(&tr) {
        eprintln!("engine-rs legacy-hash: start: {e:?}");
        return ExitCode::from(1);
    }
    let n = if a.max_ticks > 0 { (a.max_ticks as usize).min(tr.turns.len()) } else { tr.turns.len() };
    for (i, turn) in tr.turns.iter().take(n).enumerate() {
        if let Err(e) = eng.step(turn) {
            eprintln!("engine-rs legacy-hash: step {}: {e:?}", i + 1);
            return ExitCode::from(1);
        }
        if i % 10 == 0 {
            let Some(g) = eng.game() else { return ExitCode::from(1) };
            println!("{{\"tick\":{},\"hash\":{}}}", i, g.state.legacy_hash());
        }
    }
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    of_cli::alloc::mark_installed();
    install_quiet_panic_hook();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().map(String::as_str) else {
        eprint!("{}", usage());
        return ExitCode::from(2);
    };
    if cmd == "version" {
        println!(
            "© OpenFront and Contributors (AGPL-3.0). Unofficial port of the OpenFront.io engine; not endorsed by OpenFront Inc."
        );
        println!(
            "engine-rs {} upstream@{ENGINE_PIN} exec-kinds={} prng-trace={} stats-full={}",
            env!("CARGO_PKG_VERSION"),
            of_engine::ids::ExecKind::COUNT,
            of_cli::prngtrace::ENABLED,
            cfg!(feature = "stats-full"),
        );
        return ExitCode::SUCCESS;
    }
    let a = match parse(cmd, &args[1..]) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    match cmd {
        "conform" => conform(&a),
        "resume" => resume(&a),
        "bench" => bench(&a),
        "fork-bench" => fork_bench(&a),
        "legacy-hash" => legacy_hash(&a),
        _ => {
            eprint!("{}", usage());
            ExitCode::from(2)
        }
    }
}
