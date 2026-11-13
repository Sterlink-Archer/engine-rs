//! Localisation (SPEC §3.2 steps 4-5): which sections, rows and fields of two canonical
//! states differ. Rows are matched by their CS key (§1.2): P by smallID, U by id, A by
//! (attacker, id), E by position (+type), D/R tables by id or key; G T M W S are one row.
//! Path syntax follows SPEC §3.2.5: `P[smallID=3].tiles`, `E[17:Attack].toConquer.pri[412]`.

use of_canon::digest::{CanonState, SECTIONS, Section};
use of_canon::{Cs, Typed};
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq)]
pub struct FieldDiff {
    pub sec: &'static str,
    pub class: &'static str,
    pub entity: String,
    pub path: String,
    pub expected: String,
    pub actual: String,
}

impl FieldDiff {
    pub fn to_json(&self) -> Value {
        json!({"sec": self.sec, "class": self.class, "entity": self.entity, "path": self.path, "expected": self.expected, "actual": self.actual})
    }
    pub fn render(&self) -> String {
        format!("{} {}{}: expected {} actual {}", self.class, self.entity, self.path, self.expected, self.actual)
    }
}

fn short(v: &Cs) -> String {
    match v {
        Cs::Absent => "<absent>".into(),
        Cs::Null => "null".into(),
        Cs::Bool(b) => b.to_string(),
        Cs::Int(i) => i.to_string(),
        Cs::Num(x) => format!("{x:?} (0x{:016x})", x.to_bits()),
        Cs::Big(b) => format!("{b}n"),
        Cs::Str(s) => format!("{s:?}"),
        Cs::Arr(a) => format!("[{} items]", a.len()),
        Cs::Obj(f) => format!("{{{} fields}}", f.len()),
        Cs::Typed(t) => format!("typed(kind {}, {} elems)", t.kind(), t.len()),
    }
}

struct Ctx<'a> {
    out: &'a mut Vec<FieldDiff>,
    cap: usize,
    sec: Section,
    entity: String,
}

impl Ctx<'_> {
    fn push(&mut self, path: String, e: String, a: String) {
        if self.out.len() < self.cap {
            self.out.push(FieldDiff {
                sec: self.sec.name(),
                class: self.sec.class(),
                entity: self.entity.clone(),
                path,
                expected: e,
                actual: a,
            });
        }
    }
    fn full(&self) -> bool {
        self.out.len() >= self.cap
    }
}

fn typed_elems(t: &Typed) -> Vec<u64> {
    match t {
        Typed::I8(v) => v.iter().map(|x| *x as u8 as u64).collect(),
        Typed::U8(v) => v.iter().map(|x| *x as u64).collect(),
        Typed::I16(v) => v.iter().map(|x| *x as u16 as u64).collect(),
        Typed::U16(v) => v.iter().map(|x| *x as u64).collect(),
        Typed::I32(v) => v.iter().map(|x| *x as u32 as u64).collect(),
        Typed::U32(v) => v.iter().map(|x| *x as u64).collect(),
        Typed::F32(v) => v.iter().map(|x| x.to_bits() as u64).collect(),
        Typed::F64(v) => v.iter().map(|x| x.to_bits()).collect(),
    }
}

fn walk(c: &mut Ctx, path: &str, e: &Cs, a: &Cs) {
    // (PartialEq on Cs::Num treats NaN != NaN; the first match arm below handles that.)
    if e == a || c.full() {
        return;
    }
    match (e, a) {
        (Cs::Num(x), Cs::Num(y)) if x.to_bits() == y.to_bits() || (x.is_nan() && y.is_nan()) => {}
        // rules-v35 (t492; carried to rules-v36, t713): CSE encodes a safe-integer num as INT, so Int(n) and Num(n) digest equally.
        (Cs::Int(i), Cs::Num(x)) | (Cs::Num(x), Cs::Int(i)) if (*i as f64).to_bits() == x.to_bits() => {}
        (Cs::Obj(fe), Cs::Obj(fa)) => {
            let mut keys: Vec<&str> = fe.iter().chain(fa.iter()).map(|(k, _)| k.as_str()).collect();
            keys.sort();
            keys.dedup();
            for k in keys {
                let ve = fe.iter().find(|(x, _)| x == k).map(|(_, v)| v).unwrap_or(&Cs::Absent);
                let va = fa.iter().find(|(x, _)| x == k).map(|(_, v)| v).unwrap_or(&Cs::Absent);
                walk(c, &format!("{path}.{k}"), ve, va);
            }
        }
        (Cs::Arr(xe), Cs::Arr(xa)) => {
            if xe.len() != xa.len() {
                c.push(format!("{path}.length"), xe.len().to_string(), xa.len().to_string());
            }
            for i in 0..xe.len().min(xa.len()) {
                walk(c, &format!("{path}[{i}]"), &xe[i], &xa[i]);
                if c.full() {
                    return;
                }
            }
        }
        (Cs::Typed(te), Cs::Typed(ta)) => {
            if te.kind() != ta.kind() {
                c.push(format!("{path}.kind"), te.kind().to_string(), ta.kind().to_string());
                return;
            }
            let (ve, va) = (typed_elems(te), typed_elems(ta));
            if ve.len() != va.len() {
                c.push(format!("{path}.length"), ve.len().to_string(), va.len().to_string());
            }
            let n = ve.len().min(va.len());
            let diff: Vec<usize> = (0..n).filter(|&i| ve[i] != va[i]).collect();
            if let Some(&i) = diff.first() {
                c.push(
                    format!("{path}[{i}]"),
                    format!("0x{:x} ({} differing elements)", ve[i], diff.len()),
                    format!("0x{:x}", va[i]),
                );
            }
        }
        _ => c.push(path.to_string(), short(e), short(a)),
    }
}

/// Key of a row in a keyed table.
fn row_key(sec: Section, table: &str, row: &Cs, idx: usize) -> String {
    let g = |k: &str| {
        row.get(k)
            .map(|v| match v {
                Cs::Str(s) => s.clone(),
                x => short(x),
            })
            .unwrap_or_else(|| "?".into())
    };
    match (sec, table) {
        (Section::P, _) => format!("smallID={}", g("smallID")),
        (Section::U, _) => format!("id={}", g("id")),
        (Section::A, _) => format!("{}:{}", g("attacker"), g("id")),
        (Section::E, _) => format!("{idx}:{}", g("t")),
        (_, "allianceRequests") | (_, "clusters") => g("key"),
        (_, _) => match row.get("id") {
            Some(_) => format!("id={}", g("id")),
            None => idx.to_string(),
        },
    }
}

fn table_diff(c: &mut Ctx, sec: Section, table: &str, prefix: &str, e: &Cs, a: &Cs) {
    let (Some(re), Some(ra)) = (e.as_arr(), a.as_arr()) else {
        c.entity = prefix.to_string();
        walk(c, "", e, a);
        return;
    };
    let ke: Vec<String> = re.iter().enumerate().map(|(i, r)| row_key(sec, table, r, i)).collect();
    let ka: Vec<String> = ra.iter().enumerate().map(|(i, r)| row_key(sec, table, r, i)).collect();
    for (i, k) in ke.iter().enumerate() {
        c.entity = format!("{prefix}[{k}]");
        match ka.iter().position(|x| x == k) {
            Some(j) => walk(c, "", &re[i], &ra[j]),
            None => c.push(String::new(), "row present".into(), "row MISSING".into()),
        }
        if c.full() {
            return;
        }
    }
    for k in ka.iter().filter(|k| !ke.contains(k)) {
        c.entity = format!("{prefix}[{k}]");
        c.push(String::new(), "row absent".into(), "row EXTRA".into());
    }
}

/// T: report differing tiles as (x, y, expected bits, actual bits), capped, plus a total.
fn tile_diff(c: &mut Ctx, e: &CanonState, a: &CanonState) {
    for which in ["map", "miniMap"] {
        let (Some(Cs::Typed(Typed::U16(te))), Some(Cs::Typed(Typed::U16(ta)))) =
            (e.get(Section::T).get(which), a.get(Section::T).get(which))
        else {
            c.entity = format!("T.{which}");
            walk(
                c,
                "",
                e.get(Section::T).get(which).unwrap_or(&Cs::Absent),
                a.get(Section::T).get(which).unwrap_or(&Cs::Absent),
            );
            continue;
        };
        let w =
            e.get(Section::M).get(which).and_then(|m| m.get("width")).and_then(Cs::as_i64).unwrap_or(1).max(1) as usize;
        let diff: Vec<usize> = (0..te.len().min(ta.len())).filter(|&i| te[i] != ta[i]).collect();
        c.entity = format!("T.{which}");
        if te.len() != ta.len() {
            c.push(".length".into(), te.len().to_string(), ta.len().to_string());
        }
        for &i in diff.iter().take(10) {
            c.push(
                format!("(x={}, y={}) [total differing tiles {}]", i % w, i / w, diff.len()),
                format!("0x{:04x}", te[i]),
                format!("0x{:04x}", ta[i]),
            );
        }
    }
}

/// Field-level diff of two canonical states, findings in section order, capped at `cap`.
pub fn diff_states(expected: &CanonState, actual: &CanonState, cap: usize) -> Vec<FieldDiff> {
    diff_states_in(expected, actual, cap, |_| true)
}

/// [`diff_states`] restricted to the sections `want` accepts (rules-v35 t492: the caller passes
/// the sections whose digests differ, so encoding-only differences such as int-vs-double
/// `spawnCell` in G do not use up the cap).
pub fn diff_states_in(
    expected: &CanonState,
    actual: &CanonState,
    cap: usize,
    want: impl Fn(Section) -> bool,
) -> Vec<FieldDiff> {
    let mut out = vec![];
    for s in SECTIONS {
        let (e, a) = (expected.get(s), actual.get(s));
        if e == a || !want(s) {
            continue;
        }
        let mut c = Ctx { out: &mut out, cap, sec: s, entity: s.name().to_string() };
        match s {
            Section::T => tile_diff(&mut c, expected, actual),
            Section::P | Section::U | Section::A | Section::E => table_diff(&mut c, s, "", s.name(), e, a),
            Section::D => {
                for t in ["alliances", "allianceRequests"] {
                    table_diff(
                        &mut c,
                        s,
                        t,
                        &format!("D.{t}"),
                        e.get(t).unwrap_or(&Cs::Null),
                        a.get(t).unwrap_or(&Cs::Null),
                    );
                }
            }
            Section::R => {
                for t in ["stations", "railroads", "clusters"] {
                    table_diff(
                        &mut c,
                        s,
                        t,
                        &format!("R.{t}"),
                        e.get(t).unwrap_or(&Cs::Null),
                        a.get(t).unwrap_or(&Cs::Null),
                    );
                }
                c.entity = "R.network".into();
                walk(&mut c, "", e.get("network").unwrap_or(&Cs::Null), a.get("network").unwrap_or(&Cs::Null));
            }
            _ => walk(&mut c, "", e, a),
        }
        if out.len() >= cap {
            break;
        }
    }
    out
}
