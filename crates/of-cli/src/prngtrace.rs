//! `conform --prng-trace OUT.jsonl [--prng-compare TS.jsonl]` (ARCHITECTURE §4.5, §12 G13):
//! record every PRNG draw of the native run, tagged by its owner, and report the first draw
//! per tag that differs from the TS probe's trace. Owner: **P13**.
//!
//! One record per line, the format both sides write (the TS probe patch is ledger t073/t108):
//! `{"tick":T,"tag":N,"index":I,"value":V,"loc":"file:line"}` where `tick` is the step being
//! run (`game.ticks()` before it), `tag` the owner's trace tag (`Prng::set_trace_tag`; nations
//! tag their stream with their smallID, 0 = untagged), `index` that generator's 0-based draw
//! counter, `value` the raw sfc32 output (`t >>> 0`) and `loc` the drawing call site.
//!
//! The draw hook is `of_det::prng::trace` (feature `prng-trace`; compiled out otherwise).

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

/// One recorded draw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DrawRec {
    pub tick: u32,
    pub tag: u32,
    pub index: u64,
    pub value: u32,
    pub loc: String,
}

impl DrawRec {
    pub fn to_json(&self) -> Value {
        json!({"tick": self.tick, "tag": self.tag, "index": self.index, "value": self.value, "loc": self.loc})
    }
    pub fn from_json(v: &Value) -> Option<DrawRec> {
        Some(DrawRec {
            tick: u32::try_from(v.get("tick")?.as_u64()?).ok()?,
            tag: u32::try_from(v.get("tag")?.as_u64()?).ok()?,
            index: v.get("index")?.as_u64()?,
            value: u32::try_from(v.get("value")?.as_u64()?).ok()?,
            loc: v.get("loc").and_then(Value::as_str).unwrap_or("").to_owned(),
        })
    }
}

/// The draws of one run, in draw order, plus the tick the engine is in.
#[derive(Clone, Default)]
pub struct Recorder {
    pub draws: Arc<Mutex<Vec<DrawRec>>>,
    pub tick: Arc<AtomicU32>,
}

impl Recorder {
    /// Install this thread's draw hook (`prng-trace` builds only; a no-op otherwise).
    pub fn install(&self) {
        #[cfg(feature = "prng-trace")]
        {
            let draws = self.draws.clone();
            let tick = self.tick.clone();
            of_engine::of_det::prng::trace::set_hook(Some(Box::new(
                move |d: &of_engine::of_det::prng::trace::Draw| {
                    let rec = DrawRec {
                        tick: tick.load(Ordering::Relaxed),
                        tag: d.tag,
                        index: d.index,
                        value: d.value,
                        loc: format!("{}:{}", d.location.file(), d.location.line()),
                    };
                    if let Ok(mut v) = draws.lock() {
                        v.push(rec);
                    }
                },
            )));
        }
    }
    /// Remove the hook.
    pub fn uninstall(&self) {
        #[cfg(feature = "prng-trace")]
        of_engine::of_det::prng::trace::set_hook(None);
    }
    pub fn set_tick(&self, t: u32) {
        self.tick.store(t, Ordering::Relaxed);
    }
    pub fn take(&self) -> Vec<DrawRec> {
        self.draws.lock().map(|mut v| std::mem::take(&mut *v)).unwrap_or_default()
    }
}

/// Whether this binary was built with draw tracing.
pub const ENABLED: bool = cfg!(feature = "prng-trace");

pub fn write_jsonl(path: &std::path::Path, draws: &[DrawRec]) -> std::io::Result<()> {
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    for d in draws {
        writeln!(f, "{}", d.to_json())?;
    }
    f.flush()
}

pub fn read_jsonl(path: &std::path::Path) -> std::io::Result<Vec<DrawRec>> {
    let f = std::io::BufReader::new(std::fs::File::open(path)?);
    let mut out = Vec::new();
    for (i, line) in f.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(&line)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("line {}: {e}", i + 1)))?;
        out.push(DrawRec::from_json(&v).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, format!("line {}: not a draw record", i + 1))
        })?);
    }
    Ok(out)
}

/// Per tag, the first draw where the two traces differ (value, or one side ending early):
/// `{tag: {index, ts: rec|null, native: rec|null}}`, tags ascending. Equal traces → `{}`.
pub fn first_divergence_per_tag(ts: &[DrawRec], native: &[DrawRec]) -> Value {
    let group = |v: &[DrawRec]| {
        let mut m: BTreeMap<u32, Vec<DrawRec>> = BTreeMap::new();
        for d in v {
            m.entry(d.tag).or_default().push(d.clone());
        }
        m
    };
    let (a, b) = (group(ts), group(native));
    let mut out = serde_json::Map::new();
    let tags: std::collections::BTreeSet<u32> = a.keys().chain(b.keys()).copied().collect();
    let empty = Vec::new();
    for tag in tags {
        let (x, y) = (a.get(&tag).unwrap_or(&empty), b.get(&tag).unwrap_or(&empty));
        let n = x.len().max(y.len());
        for i in 0..n {
            let (p, q) = (x.get(i), y.get(i));
            let same =
                matches!((p, q), (Some(p), Some(q)) if p.value == q.value && p.index == q.index && p.tick == q.tick);
            if !same {
                out.insert(
                    tag.to_string(),
                    json!({"draw": i, "ts": p.map(DrawRec::to_json), "native": q.map(DrawRec::to_json)}),
                );
                break;
            }
        }
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(tick: u32, tag: u32, index: u64, value: u32) -> DrawRec {
        DrawRec { tick, tag, index, value, loc: format!("x.rs:{index}") }
    }

    #[test]
    fn divergence_per_tag() {
        let ts = vec![rec(0, 1, 0, 5), rec(0, 2, 0, 9), rec(1, 1, 1, 6), rec(1, 2, 1, 1)];
        let native = vec![rec(0, 1, 0, 5), rec(0, 2, 0, 9), rec(1, 1, 1, 7)];
        let d = first_divergence_per_tag(&ts, &native);
        assert_eq!(d["1"]["draw"], 1);
        assert_eq!(d["1"]["native"]["value"], 7);
        assert_eq!(d["2"]["draw"], 1);
        assert!(d["2"]["native"].is_null(), "native ended early");
        assert_eq!(first_divergence_per_tag(&ts, &ts), json!({}));
        let dir = std::env::temp_dir().join(format!("prngtrace-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("tmp");
        let p = dir.join("t.jsonl");
        write_jsonl(&p, &ts).expect("write");
        assert_eq!(read_jsonl(&p).expect("read"), ts);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
