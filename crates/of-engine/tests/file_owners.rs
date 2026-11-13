//! S0 guard for FILE_MAP §5 rule 1 ("one owner per file") at the Rust-file level.
//!
//! `tools/filemap.py` asserts one owner per **TS** file; this test makes the Rust side
//! mechanical too, so 14 packages can work in parallel without editing each other's files:
//! 1. every `.rs` file under `crates/of-engine/src` names exactly one owning package in its
//!    leading `//!` block (`Owner: **P6**`, `Owner of this file: **P10**`, `(owner: **P3**; ..)`;
//!    the first marker counts);
//! 2. every `port`/`generated` row of the FILE_MAP §1 table names Rust modules that exist as
//!    files, and each such file's header names the row's package.
//!
//! A new file without a marker, a marker that disagrees with FILE_MAP, or a FILE_MAP module
//! with no file fails here.
//!
//! Owner: **P1** (shared test fixtures, FILE_MAP §5 rule 8).

use std::path::{Path, PathBuf};

const PACKAGES: &[&str] = &["P1", "P2", "P3", "P4a", "P4b", "P5", "P6", "P7", "P8", "P9", "P10", "P11", "P12", "P13"];

fn src_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| e.unwrap_or_else(|e| panic!("{e}")).path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// The first owner marker in the file's leading `//!` block.
fn header_owner(text: &str) -> Option<String> {
    let head: String = text
        .lines()
        .take_while(|l| l.starts_with("//!"))
        .map(|l| l.trim_start_matches("//!"))
        .collect::<Vec<_>>()
        .join(" ");
    let mut rest = head.as_str();
    while let Some(i) = rest.find("wner") {
        let after = &rest[i + 4..];
        let tail: String = after.chars().take(30).collect();
        let t = tail.trim_start_matches(" of this file").trim_start_matches(':').trim_start().trim_start_matches("**");
        if let Some(p) = t.strip_prefix('P') {
            let digits: String = p.chars().take_while(|c| c.is_ascii_digit()).collect();
            if !digits.is_empty() {
                let mut name = format!("P{digits}");
                if let Some(c) = p[digits.len()..].chars().next()
                    && (c == 'a' || c == 'b')
                {
                    name.push(c);
                }
                return Some(name);
            }
        }
        rest = after;
    }
    None
}

#[test]
fn every_source_file_names_one_owner() {
    let root = src_root();
    let mut files = Vec::new();
    rs_files(&root, &mut files);
    assert!(files.len() > 100, "found {} files under {}", files.len(), root.display());
    let mut bad = Vec::new();
    for f in &files {
        let text = std::fs::read_to_string(f).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        match header_owner(&text) {
            Some(p) if PACKAGES.contains(&p.as_str()) => {}
            Some(p) => bad.push(format!("{}: unknown package {p}", f.display())),
            None => bad.push(format!("{}: no `Owner: **Pn**` marker in the leading //! block", f.display())),
        }
    }
    assert!(bad.is_empty(), "{} file(s) without a valid owner:\n{}", bad.len(), bad.join("\n"));
}

/// `a::b::c` → `src/a/b/c.rs` or `src/a/b/c/mod.rs` (`::mod` names the directory's `mod.rs`).
fn module_file(root: &Path, module: &str) -> Option<PathBuf> {
    let mut segs: Vec<&str> = module.split("::").collect();
    if segs.last() == Some(&"mod") {
        segs.pop();
    }
    let base = segs.iter().fold(root.to_path_buf(), |p, s| p.join(s));
    let file = base.with_extension("rs");
    if file.is_file() {
        return Some(file);
    }
    let m = base.join("mod.rs");
    m.is_file().then_some(m)
}

/// Strip `( .. )` groups (nesting-aware).
fn strip_parens(s: &str) -> String {
    let mut out = String::new();
    let mut depth = 0i32;
    for c in s.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

#[test]
fn file_map_modules_exist_and_match_headers() {
    let root = src_root();
    let fm = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/FILE_MAP.md");
    let text = std::fs::read_to_string(&fm).unwrap_or_else(|e| panic!("{}: {e}", fm.display()));
    let mut rows = 0;
    let mut bad = Vec::new();
    for line in text.lines() {
        // | # | `ts` | lines | `module` | pkg | status | note |
        let cols: Vec<&str> = line.split(" | ").collect();
        if cols.len() < 6
            || !cols[0].starts_with("| ")
            || cols[0][2..].parse::<u32>().is_err()
            || !cols[1].starts_with('`')
        {
            continue;
        }
        rows += 1;
        let (module_col, pkg, status) = (cols[3], cols[4].trim(), cols[5].trim());
        if status != "port" && status != "generated" {
            continue;
        }
        for (i, part) in module_col.split('`').enumerate() {
            if i % 2 == 0 {
                continue; // outside backticks
            }
            for piece in strip_parens(part).split(['+', ',']) {
                let m = piece.split_whitespace().next().unwrap_or("");
                if m.is_empty() || m.starts_with("of-det") {
                    continue;
                }
                match module_file(&root, m) {
                    None => bad.push(format!("row {}: module `{m}` ({pkg}) has no file", &cols[0][2..])),
                    Some(f) => {
                        let src = std::fs::read_to_string(&f).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
                        let owner = header_owner(&src);
                        if owner.as_deref() != Some(pkg) {
                            bad.push(format!(
                                "row {}: `{m}` → {} is owned by {owner:?} in its header, FILE_MAP says {pkg}",
                                &cols[0][2..],
                                f.display()
                            ));
                        }
                    }
                }
            }
        }
    }
    assert_eq!(rows, 152, "FILE_MAP §1 has 152 rows (ops/engine_files.txt)");
    assert!(bad.is_empty(), "{} FILE_MAP/header mismatch(es):\n{}", bad.len(), bad.join("\n"));
}

#[test]
fn header_owner_parses_the_marker_forms() {
    assert_eq!(header_owner("//! x. Owner: **P6**.\n").as_deref(), Some("P6"));
    assert_eq!(header_owner("//! Owner of this file: **P10**.\n").as_deref(), Some("P10"));
    assert_eq!(header_owner("//! scratch (owner: **P4a**; declared by P1)\n").as_deref(), Some("P4a"));
    assert_eq!(header_owner("//! Owner:\n//! **P13** (framework)\n").as_deref(), Some("P13"));
    assert_eq!(header_owner("//! Owner: P1 (the game-level memos)\n").as_deref(), Some("P1"));
    assert_eq!(header_owner("//! no marker\nuse x;\n//! Owner: **P2**\n"), None);
}
