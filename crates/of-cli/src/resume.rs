//! `engine-rs resume`: SPEC §3.2 steps 1–3 and §2.9 R3 on the native side. Owner: **P13**.
//!
//! Load the trace's resumable dump at `t_lo` with `Game::from_canon` (TS-restore semantics),
//! check the loaded state against the trace (`D(t_lo)` and `K(t_lo)` equal: otherwise the
//! failure class is **LOADER**, SPEC §3.2.2), then step `turn_{t_lo} …` and compare K every
//! tick and D at every section checkpoint, exactly like the fresh-start loop
//! (`of_trace::conform::run`), stopping at the first divergence.
//!
//! `--from` defaults to the largest resumable dump below `--before` (the bracket of a known
//! first bad tick), or to the first resumable dump after tick 0.

use crate::engine::{OfEngine, from_canon_guarded, read_dump_state};
use of_trace::conform::{Divergence, RunReport, Trigger, fill_localisation, section_diffs};
use of_trace::engine::{Engine, StepError};
use of_trace::trace::Trace;
use serde_json::Value;
use std::time::Instant;

/// Options of one resume run.
#[derive(Clone, Debug, Default)]
pub struct ResumeOpts {
    /// The dump tick to start from (`None`: see the module docs).
    pub from: Option<u32>,
    /// Pick the largest resumable dump `< before` when `from` is `None`.
    pub before: Option<u32>,
    /// Stop after this tick (0 = the trace's end).
    pub max_ticks: u32,
}

/// The dump tick to resume from.
pub fn pick_dump(trace: &Trace, o: &ResumeOpts) -> Result<u32, String> {
    let resumable = |t: u32| trace.dumps.iter().any(|d| d.tick == t && d.resumable);
    match (o.from, o.before) {
        (Some(t), _) if resumable(t) => Ok(t),
        (Some(t), _) => Err(format!("no resumable dump at tick {t}")),
        (None, Some(b)) => trace
            .dumps
            .iter()
            .filter(|d| d.resumable && d.tick < b)
            .map(|d| d.tick)
            .max()
            .ok_or_else(|| format!("no resumable dump before tick {b}")),
        (None, None) => trace
            .dumps
            .iter()
            .filter(|d| d.resumable && d.tick > 0)
            .map(|d| d.tick)
            .min()
            .or_else(|| trace.dumps.iter().find(|d| d.resumable).map(|d| d.tick))
            .ok_or_else(|| "the trace has no resumable dump".to_owned()),
    }
}

fn div(tick: u32, trigger: Trigger, message: String, last_good: Option<(u32, u64)>, bracket: u32) -> Divergence {
    Divergence {
        tick,
        trigger,
        message,
        expected: None,
        actual: None,
        last_good,
        bracket_dump: Some(bracket),
        sections: vec![],
        fields: vec![],
    }
}

/// Resume `trace` from a dump with `eng`. The report's `ticks` is the horizon; a loader
/// mismatch is reported with the message prefix `LOADER:` (and `"trigger": "LOADER"` in
/// [`report_json`]).
pub fn resume(trace: &Trace, eng: &mut OfEngine, upstream: &std::path::Path, o: &ResumeOpts) -> RunReport {
    let t0 = Instant::now();
    let mut rep = RunReport {
        scenario: trace.id.clone(),
        engine: format!("{}+resume", eng.name()),
        ticks: trace.ticks,
        steps: 0,
        k_checked: 0,
        d_checked: 0,
        ms: 0,
        divergence: None,
    };
    let t_lo = match pick_dump(trace, o) {
        Ok(t) => t,
        Err(e) => {
            rep.divergence = Some(div(0, Trigger::Unimplemented, e, None, 0));
            return rep;
        }
    };
    let finish = |mut rep: RunReport, d: Divergence| {
        rep.divergence = Some(d);
        rep.ms = t0.elapsed().as_millis();
        rep
    };
    // Build the game: the scenario's start info and maps, the dump's state.
    let info = trace.dumps.iter().find(|d| d.tick == t_lo).cloned();
    let Some(info) = info else {
        return finish(rep, div(t_lo, Trigger::Unimplemented, "dump vanished".into(), None, t_lo));
    };
    let cs = match read_dump_state(&trace.dir.join(&info.file)) {
        Ok(cs) => cs,
        Err(e) => return finish(rep, div(t_lo, Trigger::Unimplemented, format!("{e:?}"), None, t_lo)),
    };
    let built = (|| -> Result<of_engine::Game, StepError> {
        let sc = &trace.scenario;
        let gs: of_engine::api::GameStartInfo = serde_json::from_value(sc["gameStart"].clone())
            .map_err(|e| StepError::Fatal(format!("gameStart does not parse: {e}")))?;
        let files = crate::engine::load_map_files(upstream, &sc["map"], &gs.config)?;
        let statics = crate::engine::adapter_guard("MapStatics::load", || of_engine::map::MapStatics::load(&files))?
            .map_err(|e| StepError::Fatal(format!("loadTerrainMap: {e}")))?;
        from_canon_guarded(&gs, statics, &cs)
    })();
    let game = match built {
        Ok(g) => g,
        Err(e) => {
            // A CS loader error that is not a missing owner impl / S1 `todo!()` is the LOADER
            // failure class (SPEC §3.2.2); everything else is still unimplemented.
            let loader_bug = matches!(&e, StepError::Unimplemented(m) if m.contains("(port bug)"));
            let (trig, msg) = match &e {
                StepError::Unimplemented(_) if !loader_bug => (Trigger::Unimplemented, format!("{e:?}")),
                _ => (Trigger::Fatal, format!("LOADER: {e:?}")),
            };
            return finish(rep, div(t_lo, trig, msg, None, t_lo));
        }
    };
    eng.set_game(game, t_lo);

    // LOADER check (SPEC §3.2.2): the loaded state must re-digest to the trace's D(t_lo) / K.
    if let Some(want) = trace.sec.get(&t_lo) {
        match eng.full_digests() {
            Ok(got) if got.all == want.d && got.sec == want.sec => rep.d_checked += 1,
            Ok(got) => {
                let mut d = div(t_lo, Trigger::Fatal, "LOADER: D(t_lo) of the loaded dump differs".into(), None, t_lo);
                d.expected = Some(want.d);
                d.actual = Some(got.all);
                d.sections = section_diffs(&want.sec, &got.sec);
                if let Some(actual) = eng.canonical_state() {
                    fill_localisation(&cs, &actual, &mut d);
                }
                return finish(rep, d);
            }
            Err(e) => {
                return finish(rep, div(t_lo, trigger_of(&e), format!("LOADER: full_digests: {e:?}"), None, t_lo));
            }
        }
    }
    let mut last_good: Option<(u32, u64)> = None;
    let horizon = if o.max_ticks > 0 { o.max_ticks.min(trace.ticks) } else { trace.ticks };
    for t in t_lo..=horizon {
        if t > t_lo {
            let turn = &trace.turns[(t - 1) as usize]; // in-range: t - 1 < ticks = turns.len()
            let r = eng.step(turn);
            let ts_fatal_here = trace.fatal.as_ref().is_some_and(|f| f.0 == t - 1);
            match (r, ts_fatal_here) {
                (Ok(()), false) => {}
                (Err(StepError::Fatal(_)), true) => break,
                (Ok(()), true) => {
                    return finish(
                        rep,
                        div(t, Trigger::Fatal, "TS engine failed here; port did not".into(), last_good, t_lo),
                    );
                }
                (Err(e), _) => {
                    return finish(rep, div(t, trigger_of(&e), format!("step {}: {e:?}", t - 1), last_good, t_lo));
                }
            }
            rep.steps += 1;
        }
        if let Some(&want) = trace.k.get(&t) {
            match eng.key_digest() {
                Ok(got) if got == want => {
                    rep.k_checked += 1;
                    last_good = Some((t, got));
                }
                Ok(got) => {
                    let msg = if t == t_lo { "LOADER: K(t_lo) of the loaded dump differs" } else { "K digest differs" };
                    let mut d =
                        div(t, if t == t_lo { Trigger::Fatal } else { Trigger::K }, msg.into(), last_good, t_lo);
                    d.expected = Some(want);
                    d.actual = Some(got);
                    return finish(rep, d);
                }
                Err(e) => return finish(rep, div(t, trigger_of(&e), format!("key_digest: {e:?}"), last_good, t_lo)),
            }
        }
        if t > t_lo
            && let Some(want) = trace.sec.get(&t)
        {
            match eng.full_digests() {
                Ok(got) if got.all == want.d && got.sec == want.sec => rep.d_checked += 1,
                Ok(got) => {
                    let mut d = div(t, Trigger::KBlind, "D digest differs while K matched".into(), last_good, t_lo);
                    d.expected = Some(want.d);
                    d.actual = Some(got.all);
                    d.sections = section_diffs(&want.sec, &got.sec);
                    return finish(rep, d);
                }
                Err(e) => return finish(rep, div(t, trigger_of(&e), format!("full_digests: {e:?}"), last_good, t_lo)),
            }
        }
    }
    rep.ms = t0.elapsed().as_millis();
    rep
}

fn trigger_of(e: &StepError) -> Trigger {
    match e {
        StepError::Fatal(_) => Trigger::Fatal,
        StepError::Unimplemented(_) => Trigger::Unimplemented,
    }
}

/// SPEC §3.3 JSON, with `"trigger": "LOADER"` for loader failures (§3.2.2).
pub fn report_json(rep: &RunReport, engine_commit: &str) -> Value {
    let mut v = rep.to_json(engine_commit);
    if let Some(d) = &rep.divergence
        && d.message.starts_with("LOADER:")
    {
        v["trigger"] = Value::from("LOADER");
    }
    v
}
