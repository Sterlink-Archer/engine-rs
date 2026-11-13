//! The serde edge against the real corpus (ARCHITECTURE §12 G8): every `scenario.json`
//! `gameStart` parses into `GameStartInfo` and serializes back to the same JSON value
//! (absent ≠ null preserved, no zod default re-applied), and every turn of every
//! `turns.cse.zst` parses into the typed `Turn`/`StampedIntent`/`Intent` and serializes
//! back to the same value.
//!
//! Corpus root: `$OF_CORPUS_DIR`, else `conformance/golden` in the nearest ancestor that has one. The corpus
//! is still being extended by another workflow, so a scenario directory without its
//! `SHA256SUMS` (still being written) is skipped and counted; everything else must pass.

use of_canon::Cs;
use of_engine::api::{GameStartInfo, Turn};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn corpus_root() -> PathBuf {
    match std::env::var("OF_CORPUS_DIR") {
        Ok(d) => PathBuf::from(d),
        Err(_) => conformance_dir().join("golden"),
    }
}

/// The project's `conformance/` directory: the nearest ancestor of the crate that has one
/// (`engine-rs/` sits one level below the project root, `work/rs-*` worktrees two; requests
/// P9 R2, P11 R5, P12 R4).
fn conformance_dir() -> PathBuf {
    let start = Path::new(env!("CARGO_MANIFEST_DIR"));
    start
        .ancestors()
        .map(|a| a.join("conformance"))
        .find(|c| c.join("golden").is_dir() || c.join("schema").is_dir())
        .unwrap_or_else(|| start.join("../../../conformance"))
}

/// CS value → the JSON value the TS side serialized (numbers keep int vs float).
fn cs_json(v: &Cs) -> Option<Value> {
    Some(match v {
        Cs::Null => Value::Null,
        Cs::Bool(b) => Value::Bool(*b),
        Cs::Int(i) => Value::from(*i),
        Cs::Num(x) => Value::from(serde_json::Number::from_f64(*x)?),
        Cs::Str(s) => Value::String(s.clone()),
        Cs::Arr(a) => Value::Array(a.iter().map(cs_json).collect::<Option<_>>()?),
        Cs::Obj(f) => {
            let mut m = serde_json::Map::new();
            for (k, x) in f {
                if !matches!(x, Cs::Absent) {
                    m.insert(k.clone(), cs_json(x)?);
                }
            }
            Value::Object(m)
        }
        Cs::Absent | Cs::Big(_) | Cs::Typed(_) => return None,
    })
}

/// Keys that are not part of the zod schemas: zod strips them before the engine sees the
/// object, so the typed structs drop them too. Record replays (`R-*`) carry them on
/// `gameStart.players[*]` (account cosmetics and end-of-game stats).
const STRIPPED_BY_ZOD: &[&str] = &["cosmetics", "persistentID", "publicID", "stats"];

/// Keys that older records omit although the schema has them as `.nullable()`; the engine
/// reads them with a `null` default (`GameRunner.ts:59-67`: `p.clanTag` feeds the
/// `clanTag: string | null = null` parameter of `PlayerInfo`), so absent ≡ null there and
/// the typed struct writes `null`.
const ABSENT_MEANS_NULL: &[&str] = &["clanTag"];

/// JSON equality with JS number semantics (1 == 1.0; -0 must stay -0). `out` may lack an
/// input key only if it is in [`STRIPPED_BY_ZOD`]; such keys are collected in `dropped`.
fn json_eq(a: &Value, b: &Value, dropped: &mut Vec<String>) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (x.as_f64().expect("number"), y.as_f64().expect("number"));
            x.to_bits() == y.to_bits()
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| json_eq(p, q, dropped))
        }
        (Value::Object(x), Value::Object(y)) => {
            y.iter().all(|(k, w)| x.contains_key(k) || (w.is_null() && ABSENT_MEANS_NULL.contains(&k.as_str())))
                && x.iter().all(|(k, v)| match y.get(k) {
                    Some(w) => json_eq(v, w, dropped),
                    None if STRIPPED_BY_ZOD.contains(&k.as_str()) => {
                        dropped.push(k.clone());
                        true
                    }
                    None => false,
                })
        }
        _ => a == b,
    }
}

#[test]
#[ignore = "needs the full conformance corpus (not shipped): set OF_CORPUS_DIR"]
fn corpus_game_starts_and_turns_round_trip() {
    let root = corpus_root();
    assert!(root.is_dir(), "corpus not found at {} (set OF_CORPUS_DIR)", root.display());
    let dirs = of_trace::trace::find_scenarios(&root).expect("walk corpus");
    let (mut starts, mut turns, mut skipped, mut no_cse) = (0usize, 0usize, Vec::new(), 0usize);
    let mut by_type: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut dropped: Vec<String> = Vec::new();
    for dir in &dirs {
        let id = dir.strip_prefix(&root).unwrap_or(dir).display().to_string();
        if !dir.join("SHA256SUMS").is_file() {
            skipped.push(id);
            continue;
        }
        let sc: Value = serde_json::from_slice(&std::fs::read(dir.join("scenario.json")).expect("read")).expect("json");
        let gs_json = &sc["gameStart"];
        let gs: GameStartInfo =
            serde_json::from_value(gs_json.clone()).unwrap_or_else(|e| panic!("{id}: gameStart does not parse: {e}"));
        let back = serde_json::to_value(&gs).expect("serialize");
        assert!(
            json_eq(gs_json, &back, &mut dropped),
            "{id}: gameStart round trip differs\n in:  {gs_json}\n out: {back}"
        );
        starts += 1;

        let tp = dir.join("turns.cse.zst");
        if !tp.is_file() {
            no_cse += 1; // long_world keeps turns as jsonl.gz
            continue;
        }
        let bytes = of_trace::trace::zstd_read(&tp).expect("zstd");
        let all = match of_canon::cse::decode(&bytes).unwrap_or_else(|e| panic!("{id}: cse: {e}")) {
            Cs::Arr(a) => a,
            _ => panic!("{id}: turns is not an array"),
        };
        for (i, t) in all.iter().enumerate() {
            let tj = cs_json(t).unwrap_or_else(|| panic!("{id}: turn {i} has non-JSON CS values"));
            let turn: Turn = serde_json::from_value(tj.clone())
                .unwrap_or_else(|e| panic!("{id}: turn {i} does not parse: {e}\n{tj}"));
            assert_eq!(turn.turn_number, i as u64);
            for si in &turn.intents {
                *by_type.entry(si.intent.type_name()).or_default() += 1;
            }
            let back = serde_json::to_value(&turn).expect("serialize");
            let mut d = Vec::new();
            assert!(
                json_eq(&tj, &back, &mut d) && d.is_empty(),
                "{id}: turn {i} round trip differs\n in:  {tj}\n out: {back}"
            );
            turns += 1;
        }
    }
    let intents: usize = by_type.values().sum();
    eprintln!(
        "api_corpus: {} scenario dirs, {starts} gameStarts, {turns} turns, {intents} intents {by_type:?}; \
         {no_cse} without turns.cse.zst; skipped (incomplete, no SHA256SUMS): {skipped:?}; \
         non-schema keys dropped: {}",
        dirs.len(),
        dropped.len()
    );
    assert!(starts > 0 && turns > 0 && intents > 0, "nothing checked");
}
