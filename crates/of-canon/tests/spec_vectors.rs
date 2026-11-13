//! SPEC §1.4 CSE1 test-vector table + §1.5 FNV64W vectors, transcribed from
//! `conformance/results/cse_vectors.txt` (output of the reference encoder
//! `conformance/tools/cse_vectors.ts`). The test also re-reads that file and fails if
//! any row there is missing here, so the two cannot drift silently.
use of_canon::cse::{decode, encode, encode_storage, hex};
use of_canon::fnv::fnv64w;
use of_canon::{Cs, Typed};

fn rows() -> Vec<(&'static str, Cs, &'static str, &'static str)> {
    vec![
        ("`null`", Cs::Null, "01", "082f2307b4e88e77"),
        ("`true`", Cs::Bool(true), "03", "082f2507b4e891dd"),
        ("`0`", Cs::Num(0.0), "0400", "08395007b4f12f73"),
        ("`-1`", Cs::Int(-1), "0401", "08385007b4ef7c73"),
        ("`300`", Cs::Num(300.0), "04d804", "0bddf207bb24d1e2"),
        ("`2^53-1`", Cs::Int(9007199254740991), "04feffffffffffff1f", "7e2c1e7e89a56616"),
        ("`-0`", Cs::Num(-0.0), "050000000000000080", "930339b9ea8d666b"),
        ("`0.5`", Cs::Num(0.5), "05000000000000e03f", "9302918a8a8e1fa2"),
        ("`NaN`", Cs::Num(f64::from_bits(0xfff8_0000_0000_0001)), "05000000000000f87f", "93023a71428db2e2"),
        ("`Infinity`", Cs::Num(f64::INFINITY), "05000000000000f07f", "93022357fa8db2e2"),
        ("`1234567890123n`", Cs::Big(1234567890123), "069693d89fee47", "61d26a3a97538eed"),
        ("`-5n`", Cs::Big(-5), "0609", "08305207b4e1e7d9"),
        ("`\"ab\"`", Cs::Str("ab".into()), "07026162", "6726f1a810504afc"),
        ("`\"é\"`", Cs::Str("é".into()), "0702c3a9", "c926f07527d64afc"),
        ("`[1,undefined,null]`", Cs::Arr(vec![Cs::Int(1), Cs::Absent, Cs::Null]), "080304020001", "6a0fe65da573e119"),
        (
            "`{b:1,a:2,c:undefined}`",
            Cs::obj([("b", Cs::Int(1)), ("a", Cs::Int(2)), ("c", Cs::Absent)]),
            "09020161040401620402",
            "712a39df51a60fc4",
        ),
        ("`Uint32Array[1,258]`", Cs::Typed(Typed::U32(vec![1, 258])), "0a05020100000002010000", "3cf4a1ba42fe6d13"),
        ("`Float32Array[0.1]`", Cs::Typed(Typed::F32(vec![0.1])), "0a0601cdcccc3d", "3bf41ebacf737eb8"),
        ("`Uint16Array[7]`", Cs::Typed(Typed::U16(vec![7])), "0a03010700", "3ded98a59ad19afa"),
    ]
}

#[test]
fn cse1_table() {
    for (name, v, want_hex, want_fnv) in rows() {
        let b = encode(&v).unwrap();
        assert_eq!(hex(&b), want_hex, "{name}");
        assert_eq!(format!("{:016x}", fnv64w(&b)), want_fnv, "{name}");
    }
}

#[test]
fn table_matches_reference_file() {
    // rules-v35 (t492; carried to rules-v36, t713): nearest ancestor holding conformance/results (worktrees sit deeper than engine-rs/).
    let start = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let p = start
        .ancestors()
        .map(|a| a.join("conformance/results/cse_vectors.txt"))
        .find(|c| c.is_file())
        .unwrap_or_else(|| start.join("../../conformance/results/cse_vectors.txt"));
    let txt = std::fs::read_to_string(p).expect("cse_vectors.txt");
    let mine = rows();
    let mut n = 0;
    for line in txt.lines().filter(|l| l.starts_with("| `")) {
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        let (name, hx, fnv) = (cells[1], cells[2].trim_matches('`'), cells[3].trim_matches('`'));
        let r = mine
            .iter()
            .find(|r| r.0 == name)
            .unwrap_or_else(|| panic!("row {name} of cse_vectors.txt not in this test"));
        assert_eq!((r.2, r.3), (hx, fnv), "{name}");
        n += 1;
    }
    assert_eq!(n, mine.len(), "row count");
    assert!(txt.contains("FNV64W(empty)= af63bd4c8601b7df"));
}

#[test]
fn roundtrip_and_storage() {
    let v = Cs::obj([
        ("tiles", Cs::Typed(Typed::U32(vec![5, 3, 4_000_000_000, 0, 7]))),
        ("pri", Cs::Typed(Typed::F32(vec![f32::from_bits(0x7fc0_0001), -0.0, 1.5]))),
        ("x", Cs::Num(-0.0)),
        ("big", Cs::Big(i128::MIN)),
        ("bigmax", Cs::Big(i128::MAX)),
        ("arr", Cs::Arr(vec![Cs::Absent, Cs::Str("ü€😀".into()), Cs::Int(-9007199254740991)])),
        ("aa", Cs::Typed(Typed::I8(vec![-1, 2]))),
        ("a", Cs::Typed(Typed::F64(vec![f64::NAN, 1.0]))),
    ]);
    let b = encode(&v).unwrap();
    let d = decode(&b).unwrap();
    assert_eq!(encode(&d).unwrap(), b, "decode∘encode is identity on bytes");
    // keys come back sorted
    if let Cs::Obj(f) = &d {
        let ks: Vec<&str> = f.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(ks, ["a", "aa", "arr", "big", "bigmax", "pri", "tiles", "x"]);
    }
    // f32 NaN payload preserved (kind 6 not canonicalised)
    assert_eq!(
        d.get("pri").unwrap().as_typed().map(|t| matches!(t, Typed::F32(v) if v[0].to_bits() == 0x7fc0_0001)),
        Some(true)
    );
    // storage form: DELTA_U32 decodes back to TYPED u32; re-encoding in digest form gives b
    let s = encode_storage(&v).unwrap();
    assert!(s.len() < b.len() + 8);
    let ds = decode(&s).unwrap();
    assert_eq!(encode(&ds).unwrap(), b);
}

#[test]
fn rejects_non_canonical() {
    assert!(decode(&[0x05, 0, 0, 0, 0, 0, 0, 0xf0, 0x3f]).is_err(), "F64 holding 1.0");
    assert!(decode(&[0x05, 1, 0, 0, 0, 0, 0, 0xf8, 0x7f]).is_err(), "NaN payload");
    assert!(decode(&[0x09, 0x02, 0x01, b'b', 0x01, 0x01, b'a', 0x01]).is_err(), "unsorted keys");
    assert!(decode(&[0x00]).is_err(), "top-level ABSENT");
    assert!(decode(&[0x01, 0x01]).is_err(), "trailing");
    assert!(encode(&Cs::Int(1 << 53)).is_err(), "unsafe INT");
    assert!(encode(&Cs::obj([("a", Cs::Null), ("a", Cs::Null)])).is_err(), "duplicate key");
}
