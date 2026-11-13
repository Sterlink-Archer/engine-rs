//! P9 golden checks against the TS corpus dumps (FILE_MAP §4, P9 T1/T2). Owner: **P9**.
//!
//! The corpus dumps are full canonical states written by the TS engine @5b75bd3, so every
//! P9 execution record in their E section is a TS-computed oracle:
//! 1. `tribe_constructor_matches_corpus`: `TribeExec::new(tribe, tribe.id())` reproduces
//!    the five constructor draws (`TribeExecution.ts:36-41`) of every corpus tribe, and the
//!    untouched PRNG words of every tribe that has not attacked yet (attackBehavior null
//!    implies no draw since the constructor, `:56-77`).
//! 2. `p9_records_round_trip`: for every P9 record, `Uncanon` then `Canon` gives back the
//!    exact CS value (field order, int-vs-num, bigint, `AllPlayers`, request keys).
//!
//! Corpus root: `$OF_CORPUS_DIR`, else `conformance/golden` next to `engine-rs` (or next to
//! the `work/` worktree). Scenario directories still being written (no `SHA256SUMS`) are
//! skipped.

use of_canon::Cs;
use of_canon::digest::Section;
use of_engine::canon::{Canon, CanonCx, Uncanon, UncanonCx};
use of_engine::exec::ExecStore;
use of_engine::exec::alliance::{
    break_::BreakAllianceExec, extension::AllianceExtensionExec, reject::AllianceRejectExec,
    request::AllianceRequestExec,
};
use of_engine::exec::donate_gold::DonateGoldExec;
use of_engine::exec::donate_troops::DonateTroopsExec;
use of_engine::exec::embargo::EmbargoExec;
use of_engine::exec::embargo_all::EmbargoAllExec;
use of_engine::exec::emoji::EmojiExec;
use of_engine::exec::mark_disconnected::MarkDisconnectedExec;
use of_engine::exec::pause::PauseExec;
use of_engine::exec::quick_chat::QuickChatExec;
use of_engine::exec::target_player::TargetPlayerExec;
use of_engine::exec::tribe::TribeExec;
use of_engine::ids::SmallId;
use of_engine::state::State;
use of_engine::state::alliance_request::AllianceRequest;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn corpus_root() -> PathBuf {
    match std::env::var("OF_CORPUS_DIR") {
        Ok(d) => PathBuf::from(d),
        // `engine-rs/crates/of-engine` or a worktree `work/rs-P9/crates/of-engine`.
        Err(_) => ["../../../conformance/golden", "../../../../conformance/golden"]
            .iter()
            .map(|r| Path::new(env!("CARGO_MANIFEST_DIR")).join(r))
            .find(|p| p.is_dir())
            .unwrap_or_else(|| panic!("conformance/golden not found; set OF_CORPUS_DIR")),
    }
}

/// Every dump of every complete corpus scenario: (path, canonical state).
fn dumps() -> Vec<(PathBuf, of_canon::CanonState)> {
    let root = corpus_root();
    let mut out = Vec::new();
    for dir in of_trace::trace::find_scenarios(&root).unwrap_or_else(|e| panic!("{e}")) {
        if !dir.join("SHA256SUMS").is_file() {
            continue; // still being written by the corpus workflow
        }
        let Ok(rd) = std::fs::read_dir(dir.join("dumps")) else { continue };
        let mut files: Vec<PathBuf> = rd.map(|f| f.unwrap_or_else(|e| panic!("{e}")).path()).collect();
        files.sort();
        for p in files {
            let d = of_trace::trace::read_dump(&p).unwrap_or_else(|e| panic!("{e}"));
            let cs = of_trace::trace::dump_state(&d).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            out.push((p, cs));
        }
    }
    assert!(!out.is_empty(), "no corpus dumps under {}", root.display());
    out
}

/// The `d` records of the E rows of type `t`.
fn rows<'a>(cs: &'a of_canon::CanonState, t: &str) -> Vec<&'a Cs> {
    let Cs::Arr(rows) = cs.get(Section::E) else { panic!("E is not an array") };
    rows.iter().filter(|r| r.get("t").and_then(Cs::as_str) == Some(t)).filter_map(|r| r.get("d")).collect()
}

/// smallID → `info.id` from the P section.
fn player_ids(cs: &of_canon::CanonState) -> BTreeMap<i64, String> {
    let Cs::Arr(ps) = cs.get(Section::P) else { panic!("P is not an array") };
    ps.iter()
        .map(|p| {
            let sid = p.get("smallID").and_then(Cs::as_i64).unwrap_or_else(|| panic!("smallID"));
            let id = p.get("info").and_then(|i| i.get("id")).and_then(Cs::as_str).unwrap_or_else(|| panic!("info.id"));
            (sid, id.to_string())
        })
        .collect()
}

/// CSE1 bytes: the canonical form (object keys sorted, safe-integer nums as INT, absent
/// fields dropped; SPEC §1.1, §1.4), so two values are CS-equal iff their encodings are.
fn cse(v: Option<&Cs>) -> Vec<u8> {
    let v = v.unwrap_or(&Cs::Absent);
    of_canon::cse::encode(&Cs::Arr(vec![v.clone()])).unwrap_or_else(|e| panic!("{e:?}"))
}

fn small(x: i64) -> SmallId {
    SmallId(u16::try_from(x).unwrap_or_else(|e| panic!("{e}")))
}

#[test]
#[ignore = "needs the full conformance corpus (not shipped): set OF_CORPUS_DIR"]
fn tribe_constructor_matches_corpus() {
    let st = State::default();
    let ex = ExecStore::default();
    let mut ccx = CanonCx::new(&st, &ex);
    let (mut n, mut fresh) = (0usize, 0usize);
    for (path, cs) in dumps() {
        let ids = player_ids(&cs);
        for d in rows(&cs, "Tribe") {
            let sid = d.get("tribe").and_then(Cs::as_i64).unwrap_or_else(|| panic!("tribe"));
            let id = &ids[&sid];
            let mine = TribeExec::new(small(sid), id).canon(&mut ccx);
            for k in ["attackRate", "attackTick", "triggerRatio", "reserveRatio", "expandRatio"] {
                assert_eq!(cse(mine.get(k)), cse(d.get(k)), "{}: tribe {sid} ({id}) {k}", path.display());
            }
            match d.get("attackBehavior") {
                Some(Cs::Null) => {
                    // No draw since the constructor (`TribeExecution.ts:56-77`).
                    assert_eq!(
                        cse(mine.get("random")),
                        cse(d.get("random")),
                        "{}: tribe {sid} ({id}) random",
                        path.display()
                    );
                    fresh += 1;
                }
                Some(ab) => {
                    let inner = ab.get("d").unwrap_or_else(|| panic!("attackBehavior.d"));
                    for k in ["triggerRatio", "reserveRatio", "expandRatio"] {
                        assert_eq!(
                            cse(inner.get(k)),
                            cse(d.get(k)),
                            "{}: tribe {sid} attackBehavior.{k}",
                            path.display()
                        );
                    }
                }
                None => panic!("attackBehavior missing"),
            }
            n += 1;
        }
    }
    println!("tribe_constructor_matches_corpus: {n} tribe records ({fresh} with untouched PRNG)");
    assert!(n > 1000, "corpus too small to be a check: {n} tribe records");
}

/// `Uncanon` then `Canon` must reproduce the TS record exactly.
fn round_trip<T: Canon + Uncanon>(st: &mut State, d: &Cs, what: &str) {
    let back = {
        let mut ucx = UncanonCx { st, keys: &of_engine::canon::LoadKeys::default() };
        T::uncanon(d, &mut ucx).unwrap_or_else(|e| panic!("{what}: uncanon: {e:?}\n{d:?}"))
    };
    let ex = ExecStore::default();
    let mut ccx = CanonCx::new(st, &ex);
    let got = back.canon(&mut ccx);
    assert_eq!(cse(Some(&got)), cse(Some(d)), "{what}\n got: {got:?}\nwant: {d:?}");
}

#[test]
#[ignore = "needs the full conformance corpus (not shipped): set OF_CORPUS_DIR"]
fn p9_records_round_trip() {
    let mut counts = BTreeMap::<&str, usize>::new();
    for (path, cs) in dumps() {
        let ctx = path.display().to_string();
        let mut st = State::default();
        // The SPEC §1.2 request keys referenced by AllianceRequest executions resolve
        // against `State.alliance_reqs`.
        for d in rows(&cs, "AllianceRequest") {
            if let Some(k) = d.get("req").and_then(Cs::as_str) {
                let p: Vec<i64> = k.split(':').map(|x| x.parse().unwrap_or_else(|e| panic!("{k}: {e}"))).collect();
                st.alliance_reqs.push(AllianceRequest {
                    requestor: small(p[0]),
                    recipient: small(p[1]),
                    created_at: i32::try_from(p[2]).unwrap_or_else(|e| panic!("{e}")),
                    status: Default::default(),
                });
            }
        }
        macro_rules! check {
            ($($t:literal => $ty:ty),* $(,)?) => {$(
                for d in rows(&cs, $t) {
                    round_trip::<$ty>(&mut st, d, &format!("{ctx}: {}", $t));
                    *counts.entry($t).or_default() += 1;
                }
            )*};
        }
        check!(
            "AllianceExtension" => AllianceExtensionExec,
            "AllianceReject" => AllianceRejectExec,
            "AllianceRequest" => AllianceRequestExec,
            "BreakAlliance" => BreakAllianceExec,
            "DonateGold" => DonateGoldExec,
            "DonateTroops" => DonateTroopsExec,
            "Embargo" => EmbargoExec,
            "EmbargoAll" => EmbargoAllExec,
            "Emoji" => EmojiExec,
            "MarkDisconnected" => MarkDisconnectedExec,
            "Pause" => PauseExec,
            "QuickChat" => QuickChatExec,
            "TargetPlayer" => TargetPlayerExec,
            "Tribe" => TribeExec,
        );
    }
    println!("p9_records_round_trip: {counts:?}");
    assert!(counts.get("Tribe").copied().unwrap_or(0) > 1000, "{counts:?}");
}
