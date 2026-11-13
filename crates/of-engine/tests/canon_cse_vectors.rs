//! P13 rung T1 (FILE_MAP §4): the SPEC §1.4 CSE1 vector table, produced from **engine values
//! through the engine's `Canon` leaves** (not hand-built `Cs` trees), encoded by of-canon,
//! and compared byte for byte (and by FNV64W) with `conformance/results/cse_vectors.txt`,
//! the TS reference encoder's output (`conformance/tools/cse_vectors.ts`).
//!
//! This pins the engine-side numeric model of SPEC §1.1: an `f64` holding a safe integer is
//! INT, `-0.0` stays F64, NaN is canonical, BigInt fields go through `Big`, `Tri::Absent`
//! is an omitted field / ABSENT element, and typed arrays keep their TS kind.
//!
//! Owner: **P13**.

use of_canon::Cs;
use of_engine::canon::{Big, Canon, CanonCx, f32s, tiles, u16s};
use of_engine::exec::ExecStore;
use of_engine::jsval::Tri;
use of_engine::state::State;
use std::path::PathBuf;

/// `<project>/conformance`, found by walking up from this crate (the worktree layout differs
/// from `engine-rs/`'s).
fn conformance_dir() -> PathBuf {
    let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        if d.join("conformance/results/cse_vectors.txt").exists() {
            return d.join("conformance");
        }
        assert!(d.pop(), "conformance/ not found above {}", env!("CARGO_MANIFEST_DIR"));
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[test]
fn spec_1_4_table_from_engine_values() {
    let st = State::default();
    let ex = ExecStore::default();
    let mut cx = CanonCx::new(&st, &ex);
    let cx = &mut cx;
    // (label in the table, the engine value's CS)
    let rows: Vec<(&str, Cs)> = vec![
        ("`null`", None::<u32>.canon(cx)),
        ("`true`", true.canon(cx)),
        ("`0`", 0i32.canon(cx)),
        ("`-1`", (-1i32).canon(cx)),
        ("`300`", 300u32.canon(cx)),
        ("`2^53-1`", 9_007_199_254_740_991.0f64.canon(cx)), // a `num` that is a safe integer → INT
        ("`-0`", (-0.0f64).canon(cx)),
        ("`0.5`", 0.5f64.canon(cx)),
        ("`NaN`", f64::NAN.canon(cx)),
        ("`Infinity`", f64::INFINITY.canon(cx)),
        ("`1234567890123n`", Big(1_234_567_890_123).canon(cx)),
        ("`-5n`", Big(-5).canon(cx)),
        ("`\"ab\"`", String::from("ab").canon(cx)),
        ("`\"é\"`", String::from("é").canon(cx)),
        ("`[1,undefined,null]`", vec![Tri::Some(1u32), Tri::Absent, Tri::Null].canon(cx)),
        ("`{b:1,a:2,c:undefined}`", of_engine::canon_obj!(cx; "b" => 1u32, "a" => 2u32, "c" => Tri::<u32>::Absent)),
        ("`Uint32Array[1,258]`", tiles([1u32, 258])),
        ("`Float32Array[0.1]`", f32s(&[0.1f32])),
        ("`Uint16Array[7]`", u16s(&[7])),
    ];
    assert!(cx.errors.is_empty());
    let table = std::fs::read_to_string(conformance_dir().join("results/cse_vectors.txt")).expect("cse_vectors.txt");
    let mut want = std::collections::BTreeMap::new();
    for line in table.lines().filter(|l| l.starts_with("| `")) {
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        // | value | `hex` | `fnv` |
        want.insert(
            cells[1].to_owned(),
            (cells[2].trim_matches('`').to_owned(), cells[3].trim_matches('`').to_owned()),
        );
    }
    assert_eq!(want.len(), 19, "the SPEC §1.4 table has 19 rows");
    let mut checked = 0;
    for (label, v) in &rows {
        let (wh, wf) = want.get(*label).unwrap_or_else(|| panic!("{label} not in cse_vectors.txt"));
        let b = of_canon::cse::encode(v).unwrap_or_else(|e| panic!("{label}: {e}"));
        assert_eq!(&hex(&b), wh, "{label}: CSE1 bytes");
        let f = of_canon::fnv::Fnv64w::new().bytes(&b).h;
        assert_eq!(&format!("{f:016x}"), wf, "{label}: FNV64W");
        checked += 1;
    }
    assert_eq!(checked, 19);
}
