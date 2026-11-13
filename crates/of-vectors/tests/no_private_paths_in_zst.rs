//! Publication guard: no compressed artifact in the tree may embed a host path, user name or
//! machine name from the environment that generated it. Decompresses every `*.zst` file under
//! the workspace root and fails on any forbidden marker.

use std::io::Read;
use std::path::{Path, PathBuf};

// Built from pieces so this file does not contain the literal markers itself.
fn markers() -> Vec<Vec<u8>> {
    [
        &["/ho", "me/"][..],
        &["v_re", "mote"],
        &["ubu", "ntu"],
        &["box", "-2"],
        &["g", "h2"],
        &["192.", "222."],
    ]
    .iter()
    .map(|p| p.concat().into_bytes())
    .collect()
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        if p.is_dir() {
            if name != "target" && name != ".git" {
                walk(&p, out);
            }
        } else if name.ends_with(".zst") && !name.ends_with(".bin.zst") {
            // .bin.zst are raw map tiles (no text); short markers would false-positive on them.
            out.push(p);
        }
    }
}

fn find(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn no_private_paths_in_zst() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    walk(&root, &mut files);
    assert!(!files.is_empty(), "found no .zst files under {root:?}");
    let markers = markers();
    let mut bad = Vec::new();
    for f in &files {
        let mut dec = zstd::stream::read::Decoder::new(std::fs::File::open(f).unwrap()).unwrap();
        let mut buf = Vec::new();
        dec.read_to_end(&mut buf).unwrap();
        for m in &markers {
            if find(&buf, m) {
                bad.push(format!("{}: {}", f.display(), String::from_utf8_lossy(m)));
            }
        }
    }
    assert!(bad.is_empty(), "private markers in compressed files:\n{}", bad.join("\n"));
}
