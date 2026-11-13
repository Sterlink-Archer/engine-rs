//! Upstream-bump guard for the view side-effect shim (ARCHITECTURE §5 "Guards", G5).
//! Owner: **P2**.
//!
//! The port drops `toUpdate()`/`touch()` except for their one state write (`warshipState()`
//! sets `isInCombat`, `UnitImpl.ts:437-442`), replaced by `State::unit_touched` at every call
//! site. Two things would silently break that on an upstream bump:
//! 1. a body change in `UnitImpl.toUpdate`/`touch`/`warshipState`/`transportShipState` or in
//!    `PlayerImpl.toFullUpdate` (a new side-effecting getter);
//! 2. a new or moved `.touch()`/`.toUpdate()` call site.
//!
//! This test hashes the five bodies (FNV-1a 64 over the UTF-8 lines from the signature to the
//! first two-space `}`) and lists the call sites with the PORTING_GUIDE §14 grep, against the
//! values at the pin `5b75bd3`. It reads an `upstream/` checkout next to this repository (or `$OF_UPSTREAM_DIR`)
//! and is skipped, with a message, when none is present.

use std::path::{Path, PathBuf};

fn upstream() -> PathBuf {
    match std::env::var_os("OF_UPSTREAM_DIR") {
        Some(d) => PathBuf::from(d).join("packages"),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../upstream/packages"),
    }
}

fn fnv1a64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn body(file: &str, sig: &str) -> String {
    let text = std::fs::read_to_string(upstream().join(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
    let lines: Vec<&str> = text.split('\n').collect();
    let i = lines.iter().position(|l| l.starts_with(sig)).unwrap_or_else(|| panic!("{file}: `{sig}` not found"));
    let mut out = Vec::new();
    for l in &lines[i..] {
        out.push(*l);
        if *l == "  }" {
            break;
        }
    }
    out.join("\n")
}

#[test]
fn guarded_bodies_are_unchanged_since_the_pin() {
    if !upstream().is_dir() {
        eprintln!("skipped: no upstream checkout at {} (set OF_UPSTREAM_DIR)", upstream().display());
        return;
    }
    let cases = [
        ("engine/src/game/UnitImpl.ts", "  toUpdate(): UnitUpdate {", 0x21f9_51e0_dbcd_330c_u64),
        ("engine/src/game/UnitImpl.ts", "  touch(): void {", 0xf39c_66cb_7f34_f096),
        ("engine/src/game/UnitImpl.ts", "  warshipState(): WarshipState {", 0x19fa_179f_c86e_a730),
        ("engine/src/game/UnitImpl.ts", "  transportShipState(): TransportShipState {", 0x7f9b_3d89_3767_169b),
        ("engine/src/game/PlayerImpl.ts", "  private toFullUpdate(): PlayerUpdate {", 0xaeae_bbb7_b058_c135),
    ];
    for (file, sig, want) in cases {
        let got = fnv1a64(&body(file, sig));
        assert_eq!(
            got,
            want,
            "{file} `{}` changed upstream: re-check its state writes (ARCHITECTURE §5) and update the hash",
            sig.trim()
        );
    }
}

fn ts_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| e.unwrap_or_else(|e| panic!("{e}")).path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            ts_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "ts") {
            out.push(p);
        }
    }
}

#[test]
fn touch_and_to_update_call_sites_are_unchanged_since_the_pin() {
    if !upstream().is_dir() {
        eprintln!("skipped: no upstream checkout at {} (set OF_UPSTREAM_DIR)", upstream().display());
        return;
    }
    // PORTING_GUIDE §14 `EX` filter.
    const EX: &[&str] = &[
        "snapshot/",
        "worker/",
        "WinnerReplay",
        "NameBoxCalculator",
        "DebugSpan",
        "Maps.gen",
        "PackedUnitUpdates",
        "MotionPlans",
        "GameUpdateUtils",
        "HumanStats",
        "Format.ts",
    ];
    let root = upstream();
    let mut files = Vec::new();
    ts_files(&root.join("engine/src"), &mut files);
    let mut sites = Vec::new();
    for f in files {
        let rel = f.strip_prefix(&root).expect("under root").to_string_lossy().replace('\\', "/");
        let text = std::fs::read_to_string(&f).unwrap_or_else(|e| panic!("{rel}: {e}"));
        for (i, line) in text.split('\n').enumerate() {
            let hit = line.contains(".touch()") || line.contains(".toUpdate()");
            let full = format!("{rel}:{}:{line}", i + 1);
            if hit && !EX.iter().any(|x| full.contains(x)) {
                sites.push(format!("{rel}:{}", i + 1));
            }
        }
    }
    sites.sort();
    let mut want: Vec<String> = [
        // P7 (WarshipExecution, TradeShipExecution) and P8 (NukeExecution) `touch()` callers.
        "engine/src/execution/NukeExecution.ts:504",
        "engine/src/execution/TradeShipExecution.ts:134",
        "engine/src/execution/WarshipExecution.ts:86",
        "engine/src/execution/WarshipExecution.ts:687",
        "engine/src/execution/WarshipExecution.ts:707",
        // P1: alliance-request updates (464, 501, 515), addViewResync (574, not ported),
        // maybeAddUnitUpdate (723).
        "engine/src/game/GameImpl.ts:464",
        "engine/src/game/GameImpl.ts:501",
        "engine/src/game/GameImpl.ts:515",
        "engine/src/game/GameImpl.ts:574",
        "engine/src/game/GameImpl.ts:723",
        // P2: buildUnit.
        "engine/src/game/PlayerImpl.ts:1427",
        // P2: every UnitImpl site, each ported as `unit_touched` (state/unit.rs `shim_update`).
        "engine/src/game/UnitImpl.ts:133",
        "engine/src/game/UnitImpl.ts:146",
        "engine/src/game/UnitImpl.ts:228",
        "engine/src/game/UnitImpl.ts:281",
        "engine/src/game/UnitImpl.ts:314",
        "engine/src/game/UnitImpl.ts:334",
        "engine/src/game/UnitImpl.ts:356",
        "engine/src/game/UnitImpl.ts:467",
        "engine/src/game/UnitImpl.ts:478",
        "engine/src/game/UnitImpl.ts:510",
        "engine/src/game/UnitImpl.ts:538",
        "engine/src/game/UnitImpl.ts:552",
        "engine/src/game/UnitImpl.ts:566",
        "engine/src/game/UnitImpl.ts:587",
        "engine/src/game/UnitImpl.ts:674",
        "engine/src/game/UnitImpl.ts:737",
        "engine/src/game/UnitImpl.ts:762",
        "engine/src/game/UnitImpl.ts:780",
        "engine/src/game/UnitImpl.ts:794",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    want.sort();
    assert_eq!(sites, want, "touch()/toUpdate() call sites changed upstream: port the shim at each new site");
}

/// Every UnitImpl/PlayerImpl `toUpdate` site above has its `unit_touched` in the port: the
/// P2 source cites each TS line next to the call.
#[test]
fn every_p2_site_is_cited_in_the_port() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/state");
    let unit = std::fs::read_to_string(src.join("unit.rs")).expect("unit.rs");
    let player = std::fs::read_to_string(src.join("player.rs")).expect("player.rs");
    // `touch()` (146) is the shim itself; the rest are `self.shim_update(); // UnitImpl.ts:<n>`.
    for n in [133, 228, 281, 314, 334, 356, 467, 478, 510, 538, 552, 566, 587, 674, 737, 762, 780, 794] {
        assert!(
            unit.contains(&format!("self.shim_update(); // UnitImpl.ts:{n}")),
            "UnitImpl.ts:{n} has no shim in unit.rs"
        );
    }
    assert!(unit.contains("pub fn touch(&mut self) {\n        self.shim_update();"), "touch() must be the shim");
    assert!(player.contains("self.cx.st.unit_touched(b); // `this.mg.addUpdate(b.toUpdate())` (`:1427`)"));
}
