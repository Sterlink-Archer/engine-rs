//! Pipeline tests on the TS-recorded fixtures in `engine-rs/tests/fixtures/` (written by
//! `conformance/tools/rs_fixture.ts` from the real TS engine at 5b75bd3).
use of_canon::digest::Section;
use of_trace::conform::{self, Options, Trigger};
use of_trace::dumpcheck::check_dumps;
use of_trace::engine::{Inject, ReplayEngine};
use of_trace::trace::{Trace, find_scenarios};
use std::path::PathBuf;

fn fixtures() -> Vec<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    let v = find_scenarios(&root).unwrap();
    assert!(!v.is_empty(), "no fixtures under {}", root.display());
    v
}

#[test]
fn replay_passes_and_covers_every_digest() {
    for d in fixtures() {
        let tr = Trace::load(&d).unwrap();
        let r = conform::run(&tr, &mut ReplayEngine::new(&tr), &Options::default());
        assert!(r.ok(), "{}", r.render());
        assert_eq!(r.steps, tr.ticks);
        assert_eq!(r.k_checked as usize, tr.k.len());
        assert_eq!(r.d_checked as usize, tr.sec.len());
    }
}

#[test]
fn injected_divergences_are_pinned_to_their_tick() {
    for d in fixtures() {
        let tr = Trace::load(&d).unwrap();
        for t in [0, 1, tr.ticks / 3, tr.ticks] {
            let r = conform::run(&tr, &mut ReplayEngine::new(&tr).with_inject(Inject::K(t)), &Options::default());
            let dv = r.divergence.expect("must diverge");
            assert_eq!((dv.tick, dv.trigger), (t, Trigger::K));
            assert_eq!(dv.last_good.map(|x| x.0), t.checked_sub(1));
        }
        for &t in tr.sec.keys() {
            for s in [Section::G, Section::T, Section::S] {
                let r = conform::run(
                    &tr,
                    &mut ReplayEngine::new(&tr).with_inject(Inject::Section(t, s)),
                    &Options::default(),
                );
                let dv = r.divergence.expect("must diverge");
                assert_eq!((dv.tick, dv.trigger.clone()), (t, Trigger::KBlind));
                assert_eq!(dv.sections.iter().map(|x| x.sec).collect::<Vec<_>>(), [s.name()]);
            }
        }
        let r = conform::run(&tr, &mut ReplayEngine::new(&tr).with_inject(Inject::Fatal(5)), &Options::default());
        assert_eq!(r.divergence.map(|d| (d.tick, d.trigger)), Some((6, Trigger::Fatal)));
    }
}

#[test]
fn native_redigest_of_ts_dumps_matches() {
    for d in fixtures() {
        let tr = Trace::load(&d).unwrap();
        let checks = check_dumps(&tr).unwrap();
        assert!(!checks.is_empty());
        for c in checks {
            assert!(c.ok(), "{}: {c:?}", tr.id);
            assert_eq!(c.k_ok, Some(true), "K must be recorded at dump ticks");
        }
    }
}

#[test]
fn cli_smoke_exit_codes() {
    let bin = env!("CARGO_BIN_EXE_of-conform");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    let ok = std::process::Command::new(bin).arg("smoke").arg(&root).output().unwrap();
    assert!(ok.status.success(), "{}", String::from_utf8_lossy(&ok.stdout));
    assert!(String::from_utf8_lossy(&ok.stdout).contains("SMOKE PASS"));
    let bad = std::process::Command::new(bin).args(["run", "--inject", "k:7"]).arg(&root).output().unwrap();
    assert_eq!(bad.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&bad.stdout).contains("first bad tick 7 trigger K"));
}

#[test]
fn tampered_trace_is_rejected() {
    let src = &fixtures()[0];
    let tmp = std::env::temp_dir().join(format!("of-trace-tamper-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("dumps")).unwrap();
    for e in walk(src) {
        let rel = e.strip_prefix(src).unwrap();
        std::fs::copy(&e, tmp.join(rel)).unwrap();
    }
    assert!(Trace::load(&tmp).is_ok());
    let mut k = std::fs::read(tmp.join("digests.k.bin")).unwrap();
    let n = k.len();
    k[n - 1] ^= 1;
    std::fs::write(tmp.join("digests.k.bin"), &k).unwrap();
    let err = Trace::load(&tmp).unwrap_err().to_string();
    assert!(err.contains("SHA256 mismatch"), "{err}");
    std::fs::remove_dir_all(&tmp).unwrap();
}

fn walk(p: &std::path::Path) -> Vec<PathBuf> {
    let mut v = vec![];
    for e in std::fs::read_dir(p).unwrap() {
        let e = e.unwrap().path();
        if e.is_dir() {
            v.extend(walk(&e));
        } else {
            v.push(e);
        }
    }
    v
}
