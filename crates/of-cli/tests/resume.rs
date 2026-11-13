//! `engine-rs resume` (SPEC §3.2 steps 1–3, R3) on a golden CI trace: dump selection, and a
//! clean failure (never a crash) while the S1 bodies the loader needs are missing.
//! Owner: **P13**.

use of_cli::engine::OfEngine;
use of_cli::resume::{ResumeOpts, pick_dump, report_json, resume};
use of_trace::conform::Trigger;
use of_trace::trace::Trace;
use std::path::PathBuf;

fn golden(id: &str) -> PathBuf {
    if let Some(d) = std::env::var_os("OF_CORPUS_DIR") {
        return PathBuf::from(d).join("traces/ci").join(id);
    }
    let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        let p = d.join("conformance/golden/traces/ci").join(id);
        if p.is_dir() {
            return p;
        }
        assert!(d.pop(), "conformance/golden/traces/ci/{id} not found");
    }
}

#[test]
#[ignore = "needs the full conformance corpus (not shipped): set OF_CORPUS_DIR"]
fn picks_the_bracketing_resumable_dump() {
    let tr = Trace::load(&golden("F-CI-c0")).expect("trace");
    let ticks: Vec<u32> = tr.dumps.iter().filter(|d| d.resumable).map(|d| d.tick).collect();
    assert!(ticks.contains(&0) && ticks.contains(&700), "{ticks:?}");
    assert_eq!(pick_dump(&tr, &ResumeOpts::default()), Ok(700), "first resumable dump after tick 0");
    assert_eq!(pick_dump(&tr, &ResumeOpts { before: Some(700), ..ResumeOpts::default() }), Ok(0));
    assert_eq!(pick_dump(&tr, &ResumeOpts { from: Some(0), ..ResumeOpts::default() }), Ok(0));
    assert!(pick_dump(&tr, &ResumeOpts { from: Some(5), ..ResumeOpts::default() }).is_err());
    assert!(pick_dump(&tr, &ResumeOpts { before: Some(0), ..ResumeOpts::default() }).is_err());
}

#[test]
#[ignore = "needs the full conformance corpus (not shipped): set OF_CORPUS_DIR"]
fn resume_fails_cleanly_until_s1_lands() {
    let tr = Trace::load(&golden("F-CI-c0")).expect("trace");
    let mut eng = OfEngine::new();
    let o = ResumeOpts { from: Some(0), max_ticks: 3, ..ResumeOpts::default() };
    let rep = resume(&tr, &mut eng, &of_cli::engine::default_upstream_dir(), &o);
    match &rep.divergence {
        // S2: once every owner has landed this resumes and checks K/D for 3 ticks.
        None => assert_eq!(rep.k_checked, 4),
        Some(d) => {
            assert_eq!(d.tick, 0);
            assert!(
                d.trigger == Trigger::Unimplemented || d.message.starts_with("LOADER:"),
                "{:?}: {}",
                d.trigger,
                d.message
            );
            let v = report_json(&rep, "5b75bd3ad8f66325a727f7bcec47680a0cc07e63");
            assert_eq!(v["firstBadTick"], 0);
            if d.message.starts_with("LOADER:") {
                assert_eq!(v["trigger"], "LOADER");
            }
        }
    }
}
