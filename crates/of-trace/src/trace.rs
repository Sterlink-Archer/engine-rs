//! Trace reader for corpus layout v1 (SPEC §2.1-2.5).

use anyhow::{Context, Result, anyhow, bail, ensure};
use of_canon::cse;
use of_canon::digest::{CS_VERSION, CanonState, Section};
use of_canon::fnv::fnv64w;
use of_canon::tilestate::rebuild_t;
use of_canon::{Cs, SECTIONS};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One section-checkpoint record of `digests.sec.bin` (100 B).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SecRecord {
    pub tick: u32,
    pub d: u64,
    pub sec: [u64; 11],
}

#[derive(Clone, Debug)]
pub struct DumpInfo {
    pub tick: u32,
    pub file: String,
    pub resumable: bool,
    pub d: Option<u64>,
}

/// A loaded scenario directory.
#[derive(Clone, Debug)]
pub struct Trace {
    pub dir: PathBuf,
    pub id: String,
    pub scenario: Value,
    pub ticks: u32,
    pub k_stride: u32,
    pub sec_every: u32,
    /// K_t by tick (t % kStride == 0).
    pub k: BTreeMap<u32, u64>,
    pub sec: BTreeMap<u32, SecRecord>,
    pub dumps: Vec<DumpInfo>,
    /// Raw CSE1 bytes of the turns array (FNV64W = scenario.turnsDigest).
    pub turns_cse: Vec<u8>,
    pub turns: Vec<Cs>,
    pub fatal: Option<(u32, String)>,
}

fn rd_u32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
fn rd_u64_hi_lo(b: &[u8], at: usize) -> u64 {
    ((rd_u32(b, at) as u64) << 32) | rd_u32(b, at + 4) as u64
}
pub fn parse_hex64(s: &str) -> Result<u64> {
    u64::from_str_radix(s, 16).with_context(|| format!("bad 16-hex digest {s:?}"))
}

/// `digests.k.bin`: "OFCK" u32 ver u32 kStride u32 count, then (u32 tick, u32 hi, u32 lo).
pub fn read_k(b: &[u8]) -> Result<(u32, BTreeMap<u32, u64>)> {
    ensure!(b.len() >= 16 && &b[0..4] == b"OFCK", "digests.k.bin: bad magic");
    ensure!(rd_u32(b, 4) == 1, "digests.k.bin: version {}", rd_u32(b, 4));
    let stride = rd_u32(b, 8);
    let n = rd_u32(b, 12) as usize;
    ensure!(b.len() == 16 + 12 * n, "digests.k.bin: size {} != 16 + 12*{n}", b.len());
    let mut m = BTreeMap::new();
    for i in 0..n {
        let at = 16 + 12 * i;
        let t = rd_u32(b, at);
        ensure!(m.insert(t, rd_u64_hi_lo(b, at + 4)).is_none(), "digests.k.bin: duplicate tick {t}");
    }
    Ok((stride, m))
}

/// `digests.sec.bin`: "OFCS" u32 ver u32 secEvery u32 count, then (u32 tick, u64 D, 11 × u64).
pub fn read_sec(b: &[u8]) -> Result<(u32, BTreeMap<u32, SecRecord>)> {
    ensure!(b.len() >= 16 && &b[0..4] == b"OFCS", "digests.sec.bin: bad magic");
    ensure!(rd_u32(b, 4) == 1, "digests.sec.bin: version {}", rd_u32(b, 4));
    let every = rd_u32(b, 8);
    let n = rd_u32(b, 12) as usize;
    ensure!(b.len() == 16 + 100 * n, "digests.sec.bin: size {} != 16 + 100*{n}", b.len());
    let mut m = BTreeMap::new();
    for i in 0..n {
        let at = 16 + 100 * i;
        let tick = rd_u32(b, at);
        let d = rd_u64_hi_lo(b, at + 4);
        let mut sec = [0u64; 11];
        for (j, s) in sec.iter_mut().enumerate() {
            *s = rd_u64_hi_lo(b, at + 12 + 8 * j);
        }
        ensure!(m.insert(tick, SecRecord { tick, d, sec }).is_none(), "digests.sec.bin: duplicate tick {tick}");
    }
    Ok((every, m))
}

pub fn zstd_read(p: &Path) -> Result<Vec<u8>> {
    let raw = std::fs::read(p).with_context(|| format!("read {}", p.display()))?;
    zstd::decode_all(&raw[..]).with_context(|| format!("zstd {}", p.display()))
}

/// Verify `SHA256SUMS` (sha256sum format). Returns the number of files checked.
pub fn verify_sums(dir: &Path) -> Result<usize> {
    let p = dir.join("SHA256SUMS");
    let txt = std::fs::read_to_string(&p).with_context(|| format!("read {}", p.display()))?;
    let mut n = 0;
    for line in txt.lines().filter(|l| !l.trim().is_empty()) {
        let (want, file) = line.split_once("  ").ok_or_else(|| anyhow!("bad SHA256SUMS line {line:?}"))?;
        let got = hex::encode(Sha256::digest(std::fs::read(dir.join(file)).with_context(|| format!("read {file}"))?));
        ensure!(got == want, "SHA256 mismatch for {file}: {got} != {want}");
        n += 1;
    }
    Ok(n)
}

/// Decoded dump: `"OFCD" u32 csVersion u32 tick` + storage-form CSE1 of the root without T.
pub struct Dump {
    pub tick: u32,
    pub body: Vec<u8>,
    pub root: Cs,
}

pub fn read_dump(p: &Path) -> Result<Dump> {
    let raw = zstd_read(p)?;
    ensure!(raw.len() >= 12 && &raw[0..4] == b"OFCD", "{}: bad dump magic", p.display());
    ensure!(rd_u32(&raw, 4) == CS_VERSION, "{}: csVersion {}", p.display(), rd_u32(&raw, 4));
    let tick = rd_u32(&raw, 8);
    let body = raw[12..].to_vec();
    let root = cse::decode(&body).map_err(|e| anyhow!("{}: {e}", p.display()))?;
    ensure!(root.get("T").is_none(), "{}: dump must omit section T (SPEC §2.5)", p.display());
    Ok(Dump { tick, body, root })
}

/// Full canonical state from a dump: sections from the root, T rebuilt (SPEC §2.5).
pub fn dump_state(d: &Dump) -> Result<CanonState> {
    let mut cs = CanonState::from_root(&d.root).ok_or_else(|| anyhow!("dump root lacks tick"))?;
    ensure!(cs.tick == d.tick, "dump header tick {} != root tick {}", d.tick, cs.tick);
    for s in SECTIONS {
        if s != Section::T && matches!(cs.get(s), Cs::Absent) {
            bail!("dump lacks section {}", s.name());
        }
    }
    let t = rebuild_t(cs.get(Section::M), cs.get(Section::P)).map_err(|e| anyhow!("rebuild T: {e:?}"))?;
    *cs.get_mut(Section::T) = t;
    Ok(cs)
}

impl Trace {
    /// Load a scenario directory. Verifies SHA256SUMS (if present), header magics, the
    /// turns digest, and the turn count.
    pub fn load(dir: &Path) -> Result<Trace> {
        let sc: Value = serde_json::from_slice(
            &std::fs::read(dir.join("scenario.json")).with_context(|| format!("{}/scenario.json", dir.display()))?,
        )?;
        ensure!(sc["spec"] == "ofconf/1", "unsupported spec {}", sc["spec"]);
        ensure!(sc["csVersion"] == 1, "unsupported csVersion {}", sc["csVersion"]);
        if dir.join("SHA256SUMS").exists() {
            verify_sums(dir)?;
        }
        let id = sc["id"].as_str().unwrap_or("?").to_string();
        let ticks = sc["ticks"].as_u64().ok_or_else(|| anyhow!("ticks"))? as u32;
        let (k_stride, k) = read_k(&std::fs::read(dir.join("digests.k.bin"))?)?;
        let (sec_every, sec) = read_sec(&std::fs::read(dir.join("digests.sec.bin"))?)?;
        ensure!(
            sc["kStride"].as_u64() == Some(k_stride as u64),
            "kStride header {} vs scenario {}",
            k_stride,
            sc["kStride"]
        );
        ensure!(
            sc["secEvery"].as_u64() == Some(sec_every as u64),
            "secEvery header {} vs scenario {}",
            sec_every,
            sc["secEvery"]
        );
        let turns_cse = zstd_read(&dir.join("turns.cse.zst"))?;
        if let Some(want) = sc["turnsDigest"].as_str() {
            let got = fnv64w(&turns_cse);
            ensure!(got == parse_hex64(want)?, "turnsDigest {got:016x} != {want}");
        }
        let turns = match cse::decode(&turns_cse).map_err(|e| anyhow!("turns: {e}"))? {
            Cs::Arr(a) => a,
            _ => bail!("turns.cse.zst is not an array"),
        };
        ensure!(
            turns.len() == ticks as usize,
            "turn count {} != ticks {ticks} (SPEC §2.3: empty turns are explicit)",
            turns.len()
        );
        for (i, t) in turns.iter().enumerate() {
            ensure!(
                t.get("turnNumber").and_then(Cs::as_i64) == Some(i as i64),
                "turn {i} has turnNumber {:?}",
                t.get("turnNumber")
            );
        }
        let mut dumps = vec![];
        if let Some(a) = sc["dumps"].as_array() {
            for d in a {
                dumps.push(DumpInfo {
                    tick: d["tick"].as_u64().ok_or_else(|| anyhow!("dump tick"))? as u32,
                    file: d["file"].as_str().ok_or_else(|| anyhow!("dump file"))?.to_string(),
                    resumable: d["resumable"].as_bool().unwrap_or(false),
                    d: d["D"].as_str().map(parse_hex64).transpose()?,
                });
            }
        }
        let fatal = match &sc["fatal"] {
            Value::Null => None,
            f => Some((f["tick"].as_u64().unwrap_or(0) as u32, f["message"].as_str().unwrap_or("").to_string())),
        };
        // Coverage of the digest files (SPEC §2.4): K for every t % kStride == 0 in 0..=ticks.
        for t in (0..=ticks).step_by(k_stride.max(1) as usize) {
            ensure!(k.contains_key(&t) || fatal.as_ref().is_some_and(|f| t > f.0), "digests.k.bin lacks tick {t}");
        }
        Ok(Trace {
            dir: dir.to_path_buf(),
            id,
            scenario: sc,
            ticks,
            k_stride,
            sec_every,
            k,
            sec,
            dumps,
            turns_cse,
            turns,
            fatal,
        })
    }

    pub fn engine_commit(&self) -> &str {
        self.scenario["engine"]["commit"].as_str().unwrap_or("")
    }
}

/// Find every scenario directory (containing `scenario.json`) under `root`, sorted.
pub fn find_scenarios(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = vec![];
    fn walk(p: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
        if p.join("scenario.json").is_file() {
            out.push(p.to_path_buf());
            return Ok(());
        }
        if p.is_dir() {
            let mut ents: Vec<_> =
                std::fs::read_dir(p)?.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect();
            ents.sort();
            for e in ents {
                walk(&e, out)?;
            }
        }
        Ok(())
    }
    walk(root, &mut out)?;
    Ok(out)
}

/// Render a CS value as JSON for humans (`of-conform cat-turns`): non-safe numbers as
/// `"<hex>:<dec>"`, bigints as `"Nn"`, typed arrays as `{"typed":kind,"v":[...]}`.
pub fn cs_to_json(v: &Cs) -> Value {
    use of_canon::Typed;
    match v {
        Cs::Absent => Value::String("<undefined>".into()),
        Cs::Null => Value::Null,
        Cs::Bool(b) => Value::Bool(*b),
        Cs::Int(i) => Value::from(*i),
        Cs::Num(x) => Value::String(format!("{:016x}:{x:?}", x.to_bits())),
        Cs::Big(b) => Value::String(format!("{b}n")),
        Cs::Str(s) => Value::String(s.clone()),
        Cs::Arr(a) => Value::Array(a.iter().map(cs_to_json).collect()),
        Cs::Obj(f) => Value::Object(f.iter().map(|(k, v)| (k.clone(), cs_to_json(v))).collect()),
        Cs::Typed(t) => {
            let v: Vec<Value> = match t {
                Typed::I8(x) => x.iter().map(|e| Value::from(*e)).collect(),
                Typed::U8(x) => x.iter().map(|e| Value::from(*e)).collect(),
                Typed::I16(x) => x.iter().map(|e| Value::from(*e)).collect(),
                Typed::U16(x) => x.iter().map(|e| Value::from(*e)).collect(),
                Typed::I32(x) => x.iter().map(|e| Value::from(*e)).collect(),
                Typed::U32(x) => x.iter().map(|e| Value::from(*e)).collect(),
                Typed::F32(x) => x.iter().map(|e| Value::String(format!("{:08x}", e.to_bits()))).collect(),
                Typed::F64(x) => x.iter().map(|e| Value::String(format!("{:016x}", e.to_bits()))).collect(),
            };
            serde_json::json!({"typed": t.kind(), "v": v})
        }
    }
}
