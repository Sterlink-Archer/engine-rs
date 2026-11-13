//! Native re-digest of TS dumps (SPEC §2.9 R4 done by the port, and MOD-SNAPSHOT
//! §5.2): decode the storage form, rebuild T, compute D, the 11 section digests and K
//! with `of-canon`, and compare with what the TS writer recorded. Also checks that
//! re-encoding the decoded root in storage form reproduces the dump bytes exactly.

use crate::trace::{self, Trace};
use anyhow::Result;
use of_canon::cse;
use of_canon::digest::{self, SECTIONS};
use std::time::Instant;

#[derive(Clone, Debug)]
pub struct DumpCheck {
    pub tick: u32,
    pub file: String,
    /// D_t vs `scenario.dumps[].D`.
    pub d_ok: bool,
    /// D_t + sections vs `digests.sec.bin` (None if the dump tick is not a checkpoint).
    pub sec_ok: Option<bool>,
    pub bad_sections: Vec<&'static str>,
    /// K_t vs `digests.k.bin` (None if K not recorded at that tick).
    pub k_ok: Option<bool>,
    /// storage-form re-encode == dump body bytes.
    pub roundtrip_ok: bool,
    pub d: u64,
    pub k: u64,
    pub body_bytes: usize,
    pub ms: u128,
    /// Row counts and execution-type histogram of the decoded state (coverage hint).
    pub summary: String,
}

impl DumpCheck {
    pub fn ok(&self) -> bool {
        self.d_ok && self.sec_ok != Some(false) && self.k_ok != Some(false) && self.roundtrip_ok
    }
}

pub fn check_dumps(tr: &Trace) -> Result<Vec<DumpCheck>> {
    let mut out = vec![];
    for info in &tr.dumps {
        let t0 = Instant::now();
        let dump = trace::read_dump(&tr.dir.join(&info.file))?;
        let cs = trace::dump_state(&dump)?;
        let d = digest::digest_full(&cs)?;
        let k = digest::digest_key(&cs)?;
        let rec = tr.sec.get(&info.tick);
        let bad_sections: Vec<&'static str> = match rec {
            Some(r) => {
                SECTIONS.iter().filter(|s| r.sec[**s as usize] != d.sec[**s as usize]).map(|s| s.name()).collect()
            }
            None => vec![],
        };
        let re = cse::encode_storage(&dump.root)?;
        let summary = summarise(&cs);
        out.push(DumpCheck {
            tick: info.tick,
            file: info.file.clone(),
            d_ok: info.d.is_none_or(|want| want == d.all),
            sec_ok: rec.map(|r| r.d == d.all && bad_sections.is_empty()),
            bad_sections,
            k_ok: tr.k.get(&info.tick).map(|want| *want == k.all),
            roundtrip_ok: re == dump.body,
            d: d.all,
            k: k.all,
            body_bytes: dump.body.len(),
            ms: t0.elapsed().as_millis(),
            summary,
        });
    }
    Ok(out)
}

/// `P=42 U=3 A=12 E=126 {Attack: 12, Player: 42, ...}`.
pub fn summarise(cs: &of_canon::CanonState) -> String {
    use of_canon::{Cs, Section};
    let n = |s: Section| cs.get(s).as_arr().map(|a| a.len()).unwrap_or(0);
    let mut h: std::collections::BTreeMap<String, usize> = Default::default();
    for e in cs.get(Section::E).as_arr().into_iter().flatten() {
        *h.entry(e.get("t").and_then(Cs::as_str).unwrap_or("?").to_string()).or_default() += 1;
    }
    let mut ut: std::collections::BTreeMap<String, usize> = Default::default();
    for u in cs.get(Section::U).as_arr().into_iter().flatten() {
        *ut.entry(u.get("type").and_then(Cs::as_str).unwrap_or("?").to_string()).or_default() += 1;
    }
    format!("P={} U={} A={} E={} execs={h:?} units={ut:?}", n(Section::P), n(Section::U), n(Section::A), n(Section::E))
}
