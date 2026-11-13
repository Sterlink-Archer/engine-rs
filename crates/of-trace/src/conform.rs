//! Per-tick digest comparison loop (SPEC §3.1) and the first-divergence report (§3.3).

use crate::engine::{Engine, StepError};
use crate::localise::{self, FieldDiff};
use crate::trace::{self, Trace};
use of_canon::digest::{CanonState, SECTIONS};
use serde_json::{Value, json};
use std::time::Instant;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// K_t differs.
    K,
    /// D_t differs at a section checkpoint while K matched — order-only divergence.
    KBlind,
    /// The port failed (or did not fail) where the TS engine did not (did).
    Fatal,
    /// The port could not compute something it needed (unimplemented).
    Unimplemented,
}

#[derive(Clone, Debug)]
pub struct SectionDiff {
    pub sec: &'static str,
    pub class: &'static str,
    pub expected: u64,
    pub actual: u64,
}

#[derive(Clone, Debug)]
pub struct Divergence {
    pub tick: u32,
    pub trigger: Trigger,
    pub message: String,
    pub expected: Option<u64>,
    pub actual: Option<u64>,
    pub last_good: Option<(u32, u64)>,
    /// Largest dump tick < tick (resumable or not), for bracketing (SPEC §3.2.1).
    pub bracket_dump: Option<u32>,
    pub sections: Vec<SectionDiff>,
    /// Field diffs (SPEC §3.2.5) when both a TS dump and the engine's CS exist at `tick`.
    pub fields: Vec<FieldDiff>,
}

#[derive(Clone, Debug)]
pub struct RunReport {
    pub scenario: String,
    pub engine: String,
    pub ticks: u32,
    pub steps: u32,
    pub k_checked: u32,
    pub d_checked: u32,
    pub ms: u128,
    pub divergence: Option<Divergence>,
}

impl RunReport {
    pub fn ok(&self) -> bool {
        self.divergence.is_none()
    }
    /// SPEC §3.3 report JSON (plus run counters).
    pub fn to_json(&self, engine_commit: &str) -> Value {
        let d = self.divergence.as_ref();
        json!({
            "scenario": self.scenario,
            "engine": self.engine,
            "engineCommit": engine_commit,
            "ok": self.ok(),
            "ticks": self.ticks, "steps": self.steps, "kChecked": self.k_checked, "dChecked": self.d_checked, "ms": self.ms as u64,
            "firstBadTick": d.map(|d| d.tick),
            "trigger": d.map(|d| match d.trigger { Trigger::K => "K", Trigger::KBlind => "K-blind", Trigger::Fatal => "fatal", Trigger::Unimplemented => "unimplemented" }),
            "message": d.map(|d| d.message.clone()),
            "expected": d.and_then(|d| d.expected).map(|x| format!("{x:016x}")),
            "actual": d.and_then(|d| d.actual).map(|x| format!("{x:016x}")),
            "lastGood": d.and_then(|d| d.last_good).map(|(t, k)| json!({"tick": t, "K": format!("{k:016x}")})),
            "bracketDump": d.and_then(|d| d.bracket_dump),
            "sections": d.map(|d| d.sections.iter().map(|s| json!({"sec": s.sec, "class": s.class, "expected": format!("{:016x}", s.expected), "actual": format!("{:016x}", s.actual)})).collect::<Vec<_>>()),
            "fields": d.map(|d| d.fields.iter().map(|f| f.to_json()).collect::<Vec<_>>()),
            "probe": Value::Null,
            "repro": Value::Null,
        })
    }
    /// One human line + details.
    pub fn render(&self) -> String {
        let mut s = format!(
            "{} [{}]: {} — {} steps, K checked {}, D checked {}, {} ms",
            self.scenario,
            self.engine,
            if self.ok() { "PASS" } else { "DIVERGED" },
            self.steps,
            self.k_checked,
            self.d_checked,
            self.ms
        );
        if let Some(d) = &self.divergence {
            s.push_str(&format!("\n  first bad tick {} trigger {:?}: {}", d.tick, d.trigger, d.message));
            if let (Some(e), Some(a)) = (d.expected, d.actual) {
                s.push_str(&format!("\n  expected {e:016x} actual {a:016x}"));
            }
            if let Some((t, k)) = d.last_good {
                s.push_str(&format!("\n  last good tick {t} K {k:016x}"));
            }
            if let Some(b) = d.bracket_dump {
                s.push_str(&format!("\n  bracket dump t{b}"));
            }
            for x in &d.sections {
                s.push_str(&format!(
                    "\n  section {} ({}) expected {:016x} actual {:016x}",
                    x.sec, x.class, x.expected, x.actual
                ));
            }
            for f in d.fields.iter().take(20) {
                s.push_str(&format!("\n  field {}", f.render()));
            }
        }
        s
    }
}

#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Stop after this many ticks (0 = full trace).
    pub max_ticks: u32,
    /// Skip the engine-commit check.
    pub ignore_commit: bool,
}

/// Run `engine` through `trace`, stopping at the first divergence.
pub fn run(trace: &Trace, engine: &mut dyn Engine, opt: &Options) -> RunReport {
    let t0 = Instant::now();
    let mut rep = RunReport {
        scenario: trace.id.clone(),
        engine: engine.name(),
        ticks: trace.ticks,
        steps: 0,
        k_checked: 0,
        d_checked: 0,
        ms: 0,
        divergence: None,
    };
    let horizon = if opt.max_ticks > 0 { opt.max_ticks.min(trace.ticks) } else { trace.ticks };
    let mut last_good: Option<(u32, u64)> = None;
    let bracket = |t: u32| trace.dumps.iter().filter(|d| d.tick < t).map(|d| d.tick).max();
    let mk = |tick: u32, trigger: Trigger, message: String, last_good: Option<(u32, u64)>| Divergence {
        tick,
        trigger,
        message,
        expected: None,
        actual: None,
        last_good,
        bracket_dump: bracket(tick),
        sections: vec![],
        fields: vec![],
    };

    if !opt.ignore_commit && trace.engine_commit() != engine.target_commit() {
        rep.divergence = Some(mk(
            0,
            Trigger::Fatal,
            format!("engine commit {} != port target {}", trace.engine_commit(), engine.target_commit()),
            None,
        ));
        return rep;
    }
    if let Err(e) = engine.start(trace) {
        rep.divergence = Some(mk(0, trigger_of(&e), format!("start: {e:?}"), None));
        rep.ms = t0.elapsed().as_millis();
        return rep;
    }
    for t in 0..=horizon {
        if t > 0 {
            let turn = &trace.turns[(t - 1) as usize];
            let r = engine.step(turn);
            let ts_fatal_here = trace.fatal.as_ref().is_some_and(|f| f.0 == t - 1);
            match (r, ts_fatal_here) {
                (Ok(()), false) => {}
                (Err(StepError::Fatal(_)), true) => {
                    break; // both failed at the same step: conformant end of trace
                }
                (Ok(()), true) => {
                    rep.divergence = Some(mk(
                        t,
                        Trigger::Fatal,
                        format!(
                            "TS engine failed at step {} ({}); port did not",
                            t - 1,
                            trace.fatal.as_ref().unwrap().1
                        ),
                        last_good,
                    ));
                    break;
                }
                (Err(e), _) => {
                    rep.divergence = Some(mk(t, trigger_of(&e), format!("step {}: {e:?}", t - 1), last_good));
                    break;
                }
            }
            rep.steps += 1;
        }
        if let Some(&want) = trace.k.get(&t) {
            match engine.key_digest() {
                Ok(got) => {
                    rep.k_checked += 1;
                    if got != want {
                        let mut d = mk(t, Trigger::K, "K digest differs".into(), last_good);
                        d.expected = Some(want);
                        d.actual = Some(got);
                        localise_at(trace, engine, t, &mut d);
                        rep.divergence = Some(d);
                        break;
                    }
                    last_good = Some((t, got));
                }
                Err(e) => {
                    rep.divergence = Some(mk(t, trigger_of(&e), format!("key_digest: {e:?}"), last_good));
                    break;
                }
            }
        }
        if let Some(want) = trace.sec.get(&t) {
            match engine.full_digests() {
                Ok(got) => {
                    rep.d_checked += 1;
                    if got.all != want.d || got.sec != want.sec {
                        let mut d = mk(
                            t,
                            Trigger::KBlind,
                            "D digest differs while K matched (order-only divergence)".into(),
                            last_good,
                        );
                        d.expected = Some(want.d);
                        d.actual = Some(got.all);
                        d.sections = section_diffs(&want.sec, &got.sec);
                        localise_at(trace, engine, t, &mut d);
                        rep.divergence = Some(d);
                        break;
                    }
                }
                Err(e) => {
                    rep.divergence = Some(mk(t, trigger_of(&e), format!("full_digests: {e:?}"), last_good));
                    break;
                }
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

pub fn section_diffs(want: &[u64; 11], got: &[u64; 11]) -> Vec<SectionDiff> {
    SECTIONS
        .iter()
        .filter(|s| want[**s as usize] != got[**s as usize])
        .map(|s| SectionDiff { sec: s.name(), class: s.class(), expected: want[*s as usize], actual: got[*s as usize] })
        .collect()
}

/// If the trace has a dump at `t` and the engine can produce its CS, add section and
/// field diffs (SPEC §3.2 steps 4-5).
fn localise_at(trace: &Trace, engine: &mut dyn Engine, t: u32, d: &mut Divergence) {
    let Some(info) = trace.dumps.iter().find(|x| x.tick == t) else { return };
    let Some(actual) = engine.canonical_state() else { return };
    let Ok(dump) = trace::read_dump(&trace.dir.join(&info.file)) else { return };
    let Ok(expected) = trace::dump_state(&dump) else { return };
    fill_localisation(&expected, &actual, d);
}

pub fn fill_localisation(expected: &CanonState, actual: &CanonState, d: &mut Divergence) {
    let (Ok(e), Ok(a)) = (of_canon::digest::digest_full(expected), of_canon::digest::digest_full(actual)) else {
        return;
    };
    if d.sections.is_empty() {
        d.sections = section_diffs(&e.sec, &a.sec);
    }
    // rules-v35 (t492; carried to rules-v36, t713): only sections whose digests differ; cap from $OF_LOC_CAP (default 50).
    let cap = std::env::var("OF_LOC_CAP").ok().and_then(|v| v.parse().ok()).unwrap_or(50);
    let (es, as_) = (e.sec, a.sec);
    d.fields = localise::diff_states_in(expected, actual, cap, |s| es[s as usize] != as_[s as usize]);
}
