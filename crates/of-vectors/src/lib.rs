//! Golden-vector checker for `of-det` (SPEC level L0, tier (a)).
//!
//! Reads `conformance/golden/vectors/<suite>.json.zst` (format `openfront-conformance-vectors/1`,
//! `conformance/docs/VECTORS.md` §2.1) and compares every row bit-exactly:
//! f64 by `to_bits()` (NaN matches any NaN, as the format specifies), bigints exactly,
//! strings exactly. Each function gets a [`FnReport`] with pass / fail / out-of-domain
//! counts and the first failing row.
//!
//! "Out of domain" (`ood`) is used ONLY for bigint rows whose values exceed i128 (the
//! port's documented BigInt domain, `of_det::bigint`): such a row counts as `ood` when the
//! port reports `BigOverflow` (or the input itself does not fit i128), and as a FAIL if
//! the port returned a value instead. Config-suite functions not yet ported are listed
//! with `unported = cases` and are not failures (see README).

use anyhow::{Context, Result, anyhow, bail};
use of_det::bigint::{self, BigError};
use of_det::heap::FlatBinaryHeap;
use of_det::ordmap::{OrdMap, OrdSet, object_keys_order};
use of_det::prng::PseudoRandom;
use of_det::util::{self, DoomsdaySpeed};
use of_det::{detmath, js, timsort};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub const SUITES: [&str; 6] = ["detmath", "prng", "util", "config", "jssem", "order"];

#[derive(Debug, Clone, Default)]
pub struct FnReport {
    pub suite: String,
    pub function: String,
    pub source: String,
    pub cases: usize,
    pub pass: usize,
    pub fail: usize,
    pub ood: usize,
    pub unported: usize,
    pub first_fail: Option<String>,
}

impl FnReport {
    fn new(suite: &str, function: &str, source: &str) -> Self {
        FnReport { suite: suite.into(), function: function.into(), source: source.into(), ..Default::default() }
    }
    fn row(&mut self, ok: bool, ctx: impl FnOnce() -> String) {
        self.cases += 1;
        if ok {
            self.pass += 1;
        } else {
            self.fail += 1;
            if self.first_fail.is_none() {
                self.first_fail = Some(ctx());
            }
        }
    }
    fn ood(&mut self) {
        self.cases += 1;
        self.ood += 1;
    }
}

/// Default vectors directory: `$OF_VECTORS_DIR`, else `<repo>/conformance/golden/vectors`.
pub fn default_dir() -> PathBuf {
    if let Ok(d) = std::env::var("OF_VECTORS_DIR") {
        return PathBuf::from(d);
    }
    // rules-v35 (t492; carried to rules-v36, t713): the nearest ancestor holding conformance/golden/vectors (work/rs-v35 sits two levels below the
    // project root, engine-rs/ one). These engine-lib vectors (DetMath, PRNG, JS semantics, util) are pin-generated;
    // their TS sources are unchanged in v0.35.14.
    let start = Path::new(env!("CARGO_MANIFEST_DIR"));
    start
        .ancestors()
        .map(|a| a.join("conformance/golden/vectors"))
        .find(|c| c.is_dir())
        .unwrap_or_else(|| start.join("../../conformance/golden/vectors"))
}

pub fn load_suite(dir: &Path, suite: &str) -> Result<Value> {
    let p = dir.join(format!("{suite}.json.zst"));
    let raw = std::fs::read(&p).with_context(|| format!("reading {}", p.display()))?;
    let json = zstd::decode_all(&raw[..]).with_context(|| format!("zstd {}", p.display()))?;
    let v: Value = serde_json::from_slice(&json).with_context(|| format!("json {}", p.display()))?;
    if v["schema"] != "openfront-conformance-vectors/1" {
        bail!("{}: unexpected schema {}", p.display(), v["schema"]);
    }
    Ok(v)
}

// ------------------------------------------------------------------ decoding helpers

/// `num`: JSON int, or `"<16 hex IEEE-754 bits>:<decimal>"` (hex is normative).
pub fn dnum(v: &Value) -> f64 {
    match v {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i as f64
            } else if let Some(u) = n.as_u64() {
                u as f64
            } else {
                n.as_f64().unwrap()
            }
        }
        Value::String(s) => {
            let h = s.split(':').next().unwrap();
            f64::from_bits(u64::from_str_radix(h, 16).unwrap_or_else(|_| panic!("bad num {s}")))
        }
        _ => panic!("not a num: {v}"),
    }
}

/// Bit-exact f64 equality; NaN matches any NaN.
#[inline]
pub fn same(got: f64, want: f64) -> bool {
    if want.is_nan() { got.is_nan() } else { got.to_bits() == want.to_bits() }
}
fn same_v(got: f64, want: &Value) -> bool {
    same(got, dnum(want))
}

/// `big`: `"-42n"` -> Some(i128) or None when outside i128.
pub fn dbig(v: &Value) -> Option<i128> {
    let s = v.as_str().unwrap_or_else(|| panic!("not a big: {v}"));
    s.strip_suffix('n').unwrap_or_else(|| panic!("bad big {s}")).parse().ok()
}
fn is_throws(v: &Value) -> bool {
    v.get("throws").is_some()
}
/// `str`: JSON string or `{"u16":[...]}` -> UTF-16 code units.
pub fn dstr(v: &Value) -> Vec<u16> {
    match v {
        Value::String(s) => s.encode_utf16().collect(),
        Value::Object(o) => o["u16"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u16).collect(),
        _ => panic!("not a str: {v}"),
    }
}
fn arr(v: &Value) -> &Vec<Value> {
    v.as_array().unwrap_or_else(|| panic!("not an array: {v}"))
}
fn fmt_f(x: f64) -> String {
    format!("{:016x}:{:?}", x.to_bits(), x)
}

struct Fns<'a> {
    suite: &'a str,
    v: &'a Value,
}
impl<'a> Fns<'a> {
    fn get(&self, name: &str) -> Result<(FnReport, &'a Vec<Value>)> {
        let f = self.v["functions"].get(name).ok_or_else(|| anyhow!("{}: missing function {name}", self.suite))?;
        Ok((FnReport::new(self.suite, name, f["source"].as_str().unwrap_or("")), arr(&f["cases"])))
    }
    fn names(&self) -> Vec<String> {
        self.v["functions"].as_object().unwrap().keys().cloned().collect()
    }
}

/// Run every suite found in `dir`.
pub fn run_all(dir: &Path) -> Result<Vec<FnReport>> {
    let mut out = Vec::new();
    for s in SUITES {
        out.extend(run_suite(dir, s)?);
    }
    Ok(out)
}

pub fn run_suite(dir: &Path, suite: &str) -> Result<Vec<FnReport>> {
    let v = load_suite(dir, suite)?;
    let f = Fns { suite, v: &v };
    let reps = match suite {
        "detmath" => detmath_suite(&f)?,
        "prng" => prng_suite(&f)?,
        "util" => util_suite(&f)?,
        "config" => config_suite(&f)?,
        "jssem" => jssem_suite(&f)?,
        "order" => order_suite(&f)?,
        _ => bail!("unknown suite {suite}"),
    };
    // Guard: every function in the file must be accounted for.
    let names = f.names();
    for n in &names {
        if !reps.iter().any(|r| &r.function == n) {
            bail!("{suite}: function {n} present in vectors but not checked");
        }
    }
    Ok(reps)
}

// ------------------------------------------------------------------ detmath

fn detmath_suite(f: &Fns) -> Result<Vec<FnReport>> {
    let mut out = vec![];
    for (name, fun) in [("exp", detmath::exp as fn(f64) -> f64), ("log", detmath::log), ("pow2", detmath::pow2)] {
        let (mut r, cases) = f.get(name)?;
        for c in cases {
            let c = arr(c);
            let x = dnum(&c[0]);
            let got = fun(x);
            r.row(same_v(got, &c[1]), || format!("{name}({}) = {} want {}", fmt_f(x), fmt_f(got), c[1]));
        }
        out.push(r);
    }
    for (name, fun) in [("pow", detmath::pow as fn(f64, f64) -> f64), ("atan2", detmath::atan2)] {
        let (mut r, cases) = f.get(name)?;
        for c in cases {
            let c = arr(c);
            let (a, b) = (dnum(&c[0]), dnum(&c[1]));
            let got = fun(a, b);
            r.row(same_v(got, &c[2]), || format!("{name}({}, {}) = {} want {}", fmt_f(a), fmt_f(b), fmt_f(got), c[2]));
        }
        out.push(r);
    }
    let (mut r, cases) = f.get("nuke_ring_lookup")?;
    for c in cases {
        let c = arr(c);
        let (dy, dx) = (dnum(&c[0]), dnum(&c[1]));
        let (t, i0, frac) = detmath::nuke_ring_lookup(dy, dx);
        let ok = same_v(t, &c[2]) && same_v(i0, &c[3]) && same_v(frac, &c[4]);
        r.row(ok, || format!("ring({dy},{dx}) = ({},{i0},{}) want {:?}", fmt_f(t), fmt_f(frac), c));
    }
    out.push(r);
    Ok(out)
}

// ------------------------------------------------------------------ prng

fn state_eq(got: [i32; 4], want: &Value) -> bool {
    let w = arr(want);
    w.len() == 4 && (0..4).all(|i| same(got[i] as f64, dnum(&w[i])))
}

fn prng_suite(f: &Fns) -> Result<Vec<FnReport>> {
    let mut out = vec![];
    {
        let (mut r, cases) = f.get("stream_next_1000")?;
        for c in cases {
            let c = arr(c);
            let seed = dnum(&c[0]);
            let mut p = PseudoRandom::new(seed);
            let mut ok = state_eq(p.get_state(), &c[1]);
            for (i, w) in arr(&c[2]).iter().enumerate() {
                let x = p.next();
                // recorded as next()*2^32 (exact)
                if !same(x * 4294967296.0, dnum(w)) {
                    ok = false;
                    r.row(false, || format!("seed {} draw {i}", fmt_f(seed)));
                    break;
                }
            }
            if ok {
                r.row(true, String::new);
            } else if r.first_fail.is_none() {
                r.row(false, || format!("seed {} state {:?}", fmt_f(seed), p.get_state()));
            }
        }
        out.push(r);
    }
    {
        let (mut r, cases) = f.get("fromState")?;
        for c in cases {
            let c = arr(c);
            let st: Vec<f64> = arr(&c[0]).iter().map(dnum).collect();
            let mut p = PseudoRandom::from_state_f64([st[0], st[1], st[2], st[3]]);
            let draws_ok = arr(&c[1]).iter().all(|w| same(p.next_u32() as f64, dnum(w)));
            let ok = draws_ok && state_eq(p.get_state(), &c[2]);
            r.row(ok, || format!("fromState({st:?})"));
        }
        out.push(r);
    }
    {
        let (mut r, cases) = f.get("mixed_scripts")?;
        for c in cases {
            let c = arr(c);
            let seed = dnum(&c[0]);
            let mut p = PseudoRandom::new(seed);
            let mut bad: Option<String> = None;
            for (k, op) in arr(&c[1]).iter().enumerate() {
                let op = arr(op);
                let name = op[0].as_str().unwrap();
                let ok = match name {
                    "next" => same(p.next_u32() as f64, dnum(&op[1])),
                    "nextInt" => same_v(p.next_int(dnum(&op[1]), dnum(&op[2])), &op[3]),
                    "nextFloat" => same_v(p.next_float(dnum(&op[1]), dnum(&op[2])), &op[3]),
                    "nextID" => op[1].as_str() == Some(p.next_id().as_str()),
                    "randElement" => {
                        let n = dnum(&op[1]) as usize;
                        match p.rand_index(n) {
                            Ok(i) => !is_throws(&op[2]) && same(i as f64, dnum(&op[2])),
                            Err(_) => is_throws(&op[2]),
                        }
                    }
                    "randFromSet" => {
                        let n = dnum(&op[1]) as u32;
                        let s: OrdSet<u32> = (0..n).collect();
                        match p.rand_from_iter(s.iter().copied().collect::<Vec<_>>().into_iter()) {
                            Ok(x) => same(x as f64, dnum(&op[2])),
                            Err(_) => is_throws(&op[2]),
                        }
                    }
                    "chance" => op[2].as_bool() == Some(p.chance(dnum(&op[1]))),
                    "shuffleArray" => {
                        let n = dnum(&op[1]) as u32;
                        let a: Vec<u32> = (0..n).collect();
                        let got = p.shuffle_array(&a);
                        let want: Vec<u32> = arr(&op[2]).iter().map(|x| dnum(x) as u32).collect();
                        got == want
                    }
                    "getState" => state_eq(p.get_state(), &op[1]),
                    other => panic!("unknown script op {other}"),
                };
                if !ok {
                    bad = Some(format!("seed {} op #{k} {:?}", fmt_f(seed), op));
                    break;
                }
            }
            let ok = bad.is_none();
            r.row(ok, || bad.unwrap());
        }
        out.push(r);
    }
    for (name, lo, hi, is_float) in [
        ("nextInt_0_100", 0.0, 100.0, false),
        ("nextInt_m7p5_3p25", -7.5, 3.25, false),
        ("nextFloat_m1_1", -1.0, 1.0, true),
    ] {
        let (mut r, cases) = f.get(name)?;
        for c in cases {
            let c = arr(c);
            let seed = dnum(&c[0]);
            let mut p = PseudoRandom::new(seed);
            let ok = arr(&c[1]).iter().all(|w| {
                let x = if is_float { p.next_float(lo, hi) } else { p.next_int(lo, hi) };
                same_v(x, w)
            });
            r.row(ok, || format!("{name} seed {}", fmt_f(seed)));
        }
        out.push(r);
    }
    {
        let (mut r, cases) = f.get("chance")?;
        for c in cases {
            let c = arr(c);
            let (odds, seed) = (dnum(&c[0]), dnum(&c[1]));
            let mut p = PseudoRandom::new(seed);
            let ok = arr(&c[2]).iter().all(|w| w.as_bool() == Some(p.chance(odds)));
            r.row(ok, || format!("chance({odds}) seed {seed}"));
        }
        out.push(r);
    }
    {
        let (mut r, cases) = f.get("nextID")?;
        for c in cases {
            let c = arr(c);
            let seed = dnum(&c[0]);
            let mut p = PseudoRandom::new(seed);
            let ok = arr(&c[1]).iter().all(|w| w.as_str() == Some(p.next_id().as_str()));
            r.row(ok, || format!("nextID seed {seed}"));
        }
        out.push(r);
    }
    {
        let (mut r, cases) = f.get("shuffleArray")?;
        for c in cases {
            let c = arr(c);
            let seed = dnum(&c[0]);
            let n = dnum(&c[1]) as u32;
            let mut p = PseudoRandom::new(seed);
            let got = p.shuffle_array(&(0..n).collect::<Vec<_>>());
            let want: Vec<u32> = arr(&c[2]).iter().map(|x| dnum(x) as u32).collect();
            let ok = got == want && state_eq(p.get_state(), &c[3]);
            r.row(ok, || format!("shuffle seed {seed} n {n}: got {got:?}"));
        }
        out.push(r);
    }
    Ok(out)
}

// ------------------------------------------------------------------ util

fn big_or_throw_matches(got: Result<i128, BigError>, want: &Value, r: &mut FnReport, ctx: impl FnOnce() -> String) {
    if is_throws(want) {
        r.row(matches!(got, Err(BigError::NotInteger) | Err(BigError::DivZero)), ctx);
        return;
    }
    match dbig(want) {
        Some(w) => r.row(got == Ok(w), ctx),
        None => {
            if got == Err(BigError::BigOverflow) {
                r.ood()
            } else {
                r.row(false, ctx)
            }
        }
    }
}

fn util_suite(f: &Fns) -> Result<Vec<FnReport>> {
    let mut out = vec![];
    {
        let (mut r, cases) = f.get("simpleHash")?;
        for c in cases {
            let c = arr(c);
            let s = dstr(&c[0]);
            let got = util::simple_hash_u16(&s);
            r.row(same_v(got, &c[1]), || format!("simpleHash({:?}) = {got} want {}", c[0], c[1]));
        }
        out.push(r);
    }
    {
        let (mut r, cases) = f.get("toInt")?;
        for c in cases {
            let c = arr(c);
            let x = dnum(&c[0]);
            let got = util::to_int(x);
            big_or_throw_matches(got, &c[1], &mut r, || format!("toInt({}) = {got:?} want {}", fmt_f(x), c[1]));
        }
        out.push(r);
    }
    {
        let (mut r, cases) = f.get("minInt_maxInt_withinInt")?;
        for c in cases {
            let c = arr(c);
            match (dbig(&c[0]), dbig(&c[1]), dbig(&c[2]), dbig(&c[3]), dbig(&c[4]), dbig(&c[5])) {
                (Some(a), Some(b), Some(cc), Some(mn), Some(mx), Some(w)) => {
                    let ok =
                        bigint::min_int(a, b) == mn && bigint::max_int(a, b) == mx && bigint::within_int(a, b, cc) == w;
                    r.row(ok, || format!("{c:?}"));
                }
                _ => r.ood(),
            }
        }
        out.push(r);
    }
    {
        let (mut r, cases) = f.get("within")?;
        for c in cases {
            let c = arr(c);
            let got = util::within(dnum(&c[0]), dnum(&c[1]), dnum(&c[2]));
            r.row(same_v(got, &c[3]), || format!("within{c:?} = {}", fmt_f(got)));
        }
        out.push(r);
    }
    {
        let (mut r, cases) = f.get("sigmoid")?;
        for c in cases {
            let c = arr(c);
            let got = util::sigmoid(dnum(&c[0]), dnum(&c[1]), dnum(&c[2]));
            r.row(same_v(got, &c[3]), || format!("sigmoid{c:?} = {}", fmt_f(got)));
        }
        out.push(r);
    }
    {
        let (mut r, cases) = f.get("manhattanDistWrapped")?;
        for c in cases {
            let c = arr(c);
            let got = util::manhattan_dist_wrapped(dnum(&c[0]), dnum(&c[1]), dnum(&c[2]), dnum(&c[3]), dnum(&c[4]));
            r.row(same_v(got, &c[5]), || format!("mdw{c:?} = {}", fmt_f(got)));
        }
        out.push(r);
    }
    {
        let (mut r, cases) = f.get("boundingBoxCenter_inscribed")?;
        for c in cases {
            let c = arr(c);
            let n: Vec<f64> = c[..10].iter().map(dnum).collect();
            let (cx, cy) = util::bounding_box_center(n[0], n[1], n[2], n[3]);
            let ins = util::inscribed((n[0], n[1], n[2], n[3]), (n[6], n[7], n[8], n[9]));
            let ok = same(cx, n[4]) && same(cy, n[5]) && c[10].as_bool() == Some(ins);
            r.row(ok, || format!("bbc{c:?} = ({cx},{cy},{ins})"));
        }
        out.push(r);
    }
    {
        let (mut r, cases) = f.get("findMinimumBy")?;
        for c in cases {
            let c = arr(c);
            let scores: Vec<f64> = arr(&c[0]).iter().map(dnum).collect();
            let cand: Vec<bool> = arr(&c[1]).iter().map(|b| b.as_bool().unwrap()).collect();
            let idx: Vec<usize> = (0..scores.len()).collect();
            let a = util::find_minimum_by(&idx, |&i| scores[i], None::<fn(&usize) -> bool>);
            let b = util::find_minimum_by(&idx, |&i| scores[i], Some(|&i: &usize| cand[i]));
            let want = |v: &Value| if v.is_null() { None } else { Some(dnum(v) as usize) };
            let ok = a == want(&c[2]) && b == want(&c[3]);
            r.row(ok, || format!("findMinimumBy{c:?} = {a:?},{b:?}"));
        }
        out.push(r);
    }
    {
        let (mut r, cases) = f.get("getMode")?;
        for c in cases {
            let c = arr(c);
            // Map insertion order: build an OrdMap from the pairs as listed.
            let mut m: OrdMap<i64, f64> = OrdMap::new();
            for kv in arr(&c[0]) {
                let kv = arr(kv);
                m.insert(dnum(&kv[0]) as i64, dnum(&kv[1]));
            }
            let got = util::get_mode(m.iter().map(|(k, v)| (*k, *v)));
            let want = if c[1].is_null() { None } else { Some(dnum(&c[1]) as i64) };
            r.row(got == want, || format!("getMode{c:?} = {got:?}"));
        }
        out.push(r);
    }
    {
        let (mut r, cases) = f.get("closestTwoTiles")?;
        for c in cases {
            let c = arr(c);
            let w = dnum(&c[0]) as u32;
            let xs: Vec<u32> = arr(&c[1]).iter().map(|x| dnum(x) as u32).collect();
            let ys: Vec<u32> = arr(&c[2]).iter().map(|x| dnum(x) as u32).collect();
            let got = util::closest_two_tiles(w, &xs, &ys);
            let want = if c[3].is_null() {
                None
            } else {
                let p = arr(&c[3]);
                Some((dnum(&p[0]) as u32, dnum(&p[1]) as u32))
            };
            r.row(got == want, || format!("closestTwoTiles{c:?} = {got:?}"));
        }
        out.push(r);
    }
    {
        let (mut r, cases) = f.get("doomsdayClockRequiredTiles")?;
        for c in cases {
            let c = arr(c);
            let sp = DoomsdaySpeed::parse(c[0].as_str().unwrap());
            let got = util::doomsday_clock_required_tiles(sp, c[1].as_bool().unwrap(), dnum(&c[2]), dnum(&c[3]));
            r.row(same_v(got, &c[4]), || format!("reqTiles{c:?} = {got}"));
        }
        out.push(r);
    }
    {
        let (mut r, cases) = f.get("doomsdayClockTroopFloor")?;
        for c in cases {
            let c = arr(c);
            let got = util::doomsday_clock_troop_floor(dnum(&c[0]), dnum(&c[1]), 5.0, 40.0, 90.0);
            r.row(same_v(got, &c[2]), || format!("troopFloor{c:?} = {got}"));
        }
        out.push(r);
    }
    {
        let (mut r, cases) = f.get("doomsdayClockDrain")?;
        for c in cases {
            let c = arr(c);
            let e = dnum(&c[2]);
            let (s, m) = if e <= 1.0 { (2.0, 5.0) } else { (1.0, 50.0) };
            let got = util::doomsday_clock_drain(dnum(&c[0]), dnum(&c[1]), s, m, 90.0, e);
            r.row(same_v(got, &c[3]), || format!("drain{c:?} = {got}"));
        }
        out.push(r);
    }
    {
        let (mut r, cases) = f.get("doomsdayClockRotQuota")?;
        for c in cases {
            let c = arr(c);
            let got = util::doomsday_clock_rot_quota(dnum(&c[0]), dnum(&c[1]), 150.0);
            r.row(same_v(got, &c[2]), || format!("rotQuota{c:?} = {got}"));
        }
        out.push(r);
    }
    {
        let (mut r, cases) = f.get("rotNoise")?;
        for c in cases {
            let c = arr(c);
            let n: Vec<f64> = c[..4].iter().map(dnum).collect();
            let a = util::rot_speckle_noise(n[0], n[1], n[2]);
            let b = util::rot_front_noise(n[3], n[2]);
            r.row(same_v(a, &c[4]) && same_v(b, &c[5]), || format!("rotNoise{c:?} = {a},{b}"));
        }
        out.push(r);
    }
    {
        let (mut r, cases) = f.get("maxHealthWithVeterancy")?;
        for c in cases {
            let c = arr(c);
            let got = util::max_health_with_veterancy(dnum(&c[0]), dnum(&c[1]), dnum(&c[2]));
            r.row(same_v(got, &c[3]), || format!("veterancy{c:?} = {got}"));
        }
        out.push(r);
    }
    Ok(out)
}

// ------------------------------------------------------------------ config (game rules)

/// The config-independent formulas are ported in `of_det::config` and checked here; the
/// remaining EngineConfig functions need GameConfig/player/unit models and belong to the
/// engine port (t065). They are listed with `unported = cases`, not counted as failures.
fn config_suite(f: &Fns) -> Result<Vec<FnReport>> {
    use of_det::config as c;
    let mut out = vec![];
    for name in f.names() {
        let (mut r, cases) = f.get(&name)?;
        let n = |row: &Vec<Value>, i: usize| dnum(&row[i]);
        match name.as_str() {
            "saturation" => cases.iter().map(arr).for_each(|row| {
                let x = n(row, 0);
                let (a, b) = (c::train_saturation(x), c::trade_ship_saturation(x));
                r.row(same_v(a, &row[1]) && same_v(b, &row[2]), || {
                    format!("saturation({}) = {},{}", fmt_f(x), fmt_f(a), fmt_f(b))
                });
            }),
            "smallEconomySpawnBoost" => cases.iter().map(arr).for_each(|row| {
                let g = c::small_economy_spawn_boost(n(row, 0));
                r.row(same_v(g, &row[1]), || format!("boost{row:?} = {}", fmt_f(g)));
            }),
            "tradeShipSpawnRate" => cases.iter().map(arr).for_each(|row| {
                let g = c::trade_ship_spawn_rate(n(row, 0), n(row, 1), n(row, 2));
                r.row(same_v(g, &row[3]), || format!("tradeShipSpawnRate{row:?} = {}", fmt_f(g)));
            }),
            "trainSpawnRate" => cases.iter().map(arr).for_each(|row| {
                let g = c::train_spawn_rate(n(row, 0), n(row, 1));
                r.row(same_v(g, &row[2]), || format!("trainSpawnRate{row:?} = {}", fmt_f(g)));
            }),
            "samRange" => cases.iter().map(arr).for_each(|row| {
                let g = c::sam_range(n(row, 0));
                r.row(same_v(g, &row[1]), || format!("samRange{row:?} = {}", fmt_f(g)));
            }),
            _ => {
                r.cases = cases.len();
                r.unported = cases.len();
            }
        }
        out.push(r);
    }
    Ok(out)
}

// ------------------------------------------------------------------ jssem

fn jssem_suite(f: &Fns) -> Result<Vec<FnReport>> {
    let mut out = vec![];
    for (name, fun) in [("Math.round", js::round as fn(f64) -> f64), ("Math.fround", js::fround)] {
        let (mut r, cases) = f.get(name)?;
        for c in cases {
            let c = arr(c);
            let x = dnum(&c[0]);
            let got = fun(x);
            r.row(same_v(got, &c[1]), || format!("{name}({}) = {} want {}", fmt_f(x), fmt_f(got), c[1]));
        }
        out.push(r);
    }
    {
        let (mut r, cases) = f.get("Math.floor_ceil_trunc")?;
        for c in cases {
            let c = arr(c);
            let x = dnum(&c[0]);
            let ok = same_v(js::floor(x), &c[1]) && same_v(js::ceil(x), &c[2]) && same_v(js::trunc(x), &c[3]);
            r.row(ok, || format!("fct({}) want {c:?}", fmt_f(x)));
        }
        out.push(r);
    }
    {
        let (mut r, cases) = f.get("Math.sign_abs_sqrt")?;
        for c in cases {
            let c = arr(c);
            let x = dnum(&c[0]);
            let ok = same_v(js::sign(x), &c[1]) && same_v(js::abs(x), &c[2]) && same_v(js::sqrt(x), &c[3]);
            r.row(ok, || format!("sas({}) want {c:?}", fmt_f(x)));
        }
        out.push(r);
    }
    {
        let (mut r, cases) = f.get("ToInt32_ToUint32")?;
        for c in cases {
            let c = arr(c);
            let x = dnum(&c[0]);
            let ok = same_v(js::to_int32(x) as f64, &c[1])
                && same_v(js::to_uint32(x) as f64, &c[2])
                && same_v((!js::to_int32(x)) as f64, &c[3])
                && same_v(js::to_int32(-x) as f64, &c[4]);
            r.row(ok, || format!("toint32({}) want {c:?}", fmt_f(x)));
        }
        out.push(r);
    }
    {
        let (mut r, cases) = f.get("Math.imul")?;
        for c in cases {
            let c = arr(c);
            let got = js::imul_f64(dnum(&c[0]), dnum(&c[1]));
            r.row(same_v(got as f64, &c[2]), || format!("imul{c:?} = {got}"));
        }
        out.push(r);
    }
    {
        let (mut r, cases) = f.get("shifts")?;
        for c in cases {
            let c = arr(c);
            let (a, s) = (dnum(&c[0]), dnum(&c[1]));
            let ok = same_v(js::shl(a, s) as f64, &c[2])
                && same_v(js::sar(a, s) as f64, &c[3])
                && same_v(js::shr(a, s) as f64, &c[4]);
            r.row(ok, || format!("shifts{c:?}"));
        }
        out.push(r);
    }
    {
        let (mut r, cases) = f.get("bitwise_and_wrapping_add")?;
        for c in cases {
            let c = arr(c);
            let (a, b) = (dnum(&c[0]), dnum(&c[1]));
            let (ia, ib) = (js::to_int32(a), js::to_int32(b));
            let ok = same_v((ia & ib) as f64, &c[2])
                && same_v((ia | ib) as f64, &c[3])
                && same_v((ia ^ ib) as f64, &c[4])
                && same_v(js::to_int32(a + b) as f64, &c[5])
                && same_v(js::to_int32(a - b) as f64, &c[6]);
            r.row(ok, || format!("bitwise{c:?}"));
        }
        out.push(r);
    }
    {
        let (mut r, cases) = f.get("mod_div")?;
        for c in cases {
            let c = arr(c);
            let (a, b) = (dnum(&c[0]), dnum(&c[1]));
            let ok = same_v(js::rem(a, b), &c[2]) && same_v(a / b, &c[3]);
            r.row(ok, || format!("mod_div{c:?} = {},{}", fmt_f(js::rem(a, b)), fmt_f(a / b)));
        }
        out.push(r);
    }
    {
        let (mut r, cases) = f.get("Math.min_max")?;
        for c in cases {
            let c = arr(c);
            if c[0].is_array() {
                // no-arg row: [[], min(), max()]
                let ok = same_v(js::min_n(&[]), &c[1]) && same_v(js::max_n(&[]), &c[2]);
                r.row(ok, || format!("min/max no-arg {c:?}"));
                continue;
            }
            let (a, b) = (dnum(&c[0]), dnum(&c[1]));
            let ok = same_v(js::min(a, b), &c[2]) && same_v(js::max(a, b), &c[3]);
            r.row(ok, || format!("minmax{c:?}"));
        }
        out.push(r);
    }
    {
        let (mut r, cases) = f.get("bigint_div_mod_mul")?;
        for c in cases {
            let c = arr(c);
            let (a, b) = match (dbig(&c[0]), dbig(&c[1])) {
                (Some(a), Some(b)) => (a, b),
                _ => {
                    r.ood();
                    continue;
                }
            };
            // Each output cell: exact match, or (beyond i128) BigOverflow.
            // Each output cell: Some(true) exact match, Some(false) mismatch, None = beyond i128
            // and the port reported BigOverflow (out of domain).
            let cell = |got: Result<i128, BigError>, want: &Value| -> Option<bool> {
                if is_throws(want) {
                    return Some(got == Err(BigError::DivZero));
                }
                match dbig(want) {
                    Some(w) => Some(got == Ok(w)),
                    None => {
                        if got == Err(BigError::BigOverflow) {
                            None
                        } else {
                            Some(false)
                        }
                    }
                }
            };
            let cells =
                [cell(bigint::div(a, b), &c[2]), cell(bigint::rem(a, b), &c[3]), cell(bigint::mul(a, b), &c[4])];
            let num_cell = match bigint::sub(a, b) {
                Ok(d) => Some(same_v(bigint::to_number(d), &c[5])),
                Err(_) => None,
            };
            let num_ok = num_cell != Some(false);
            if cells.iter().any(|x| *x == Some(false)) || !num_ok {
                r.row(false, || format!("bigint{c:?} cells {cells:?} num_ok {num_ok}"));
            } else if cells.iter().any(|x| x.is_none()) || num_cell.is_none() {
                r.ood();
            } else {
                r.row(true, String::new);
            }
        }
        out.push(r);
    }
    {
        let (mut r, cases) = f.get("Number(bigint)")?;
        for c in cases {
            let c = arr(c);
            match dbig(&c[0]) {
                Some(b) => {
                    let got = bigint::to_number(b);
                    r.row(same_v(got, &c[1]), || format!("Number({}) = {} want {}", c[0], fmt_f(got), c[1]));
                }
                None => r.ood(),
            }
        }
        out.push(r);
    }
    {
        let (mut r, cases) = f.get("BigInt(number)")?;
        for c in cases {
            let c = arr(c);
            let x = dnum(&c[0]);
            let mut sub = FnReport::default();
            big_or_throw_matches(bigint::from_number(x), &c[1], &mut sub, String::new);
            big_or_throw_matches(bigint::from_number_floor(x), &c[2], &mut sub, String::new);
            if sub.fail > 0 {
                r.row(false, || {
                    format!(
                        "BigInt({}) = {:?},{:?} want {c:?}",
                        fmt_f(x),
                        bigint::from_number(x),
                        bigint::from_number_floor(x)
                    )
                });
            } else if sub.ood > 0 {
                r.ood();
            } else {
                r.row(true, String::new);
            }
        }
        out.push(r);
    }
    {
        let (mut r, cases) = f.get("toString36")?;
        for c in cases {
            let c = arr(c);
            // The generator records `x.toString(36).padStart(8, "0")` (suites/jssem.ts:131),
            // i.e. the nextID formatting, although the sig says "int x -> str".
            let got = js::pad_start_zero(&js::to_string_radix(dnum(&c[0]), 36), 8);
            r.row(c[1].as_str() == Some(got.as_str()), || format!("toString36({}) = {got} want {}", c[0], c[1]));
        }
        out.push(r);
    }
    Ok(out)
}

// ------------------------------------------------------------------ order

#[derive(Clone)]
struct El {
    i: u32,
    k: f64,
    k2: f64,
    f: bool,
    g: i128,
}

fn order_suite(f: &Fns) -> Result<Vec<FnReport>> {
    let mut out = vec![];
    {
        let (mut r, cases) = f.get("array_sort")?;
        for c in cases {
            let c = arr(c);
            let kind = c[0].as_str().unwrap();
            let w = dnum(&c[1]);
            let keys = arr(&c[2]);
            let mut oob = false;
            let mut els: Vec<El> = keys
                .iter()
                .enumerate()
                .map(|(i, key)| {
                    let mut e = El { i: i as u32, k: 0.0, k2: 0.0, f: false, g: 0 };
                    match kind {
                        "asc" | "desc" | "mod_w" => e.k = dnum(key),
                        "asc2" | "desc2" => {
                            let p = arr(key);
                            e.k = dnum(&p[0]);
                            e.k2 = dnum(&p[1]);
                        }
                        "boolnum_desc" => e.f = key.as_bool().unwrap(),
                        "bigint_desc" => match dbig(key) {
                            Some(g) => e.g = g,
                            None => oob = true,
                        },
                        _ => panic!("unknown comparator {kind}"),
                    }
                    e
                })
                .collect();
            if oob {
                r.ood();
                continue;
            }
            let mut trace: Vec<u32> = Vec::new();
            {
                let tr = &mut trace;
                let mut cmp = |a: &El, b: &El| -> f64 {
                    tr.push(a.i);
                    tr.push(b.i);
                    match kind {
                        "asc" => a.k - b.k,
                        "desc" => b.k - a.k,
                        "asc2" => js::or_num(a.k - b.k, a.k2 - b.k2),
                        "desc2" => js::or_num(b.k - a.k, b.k2 - a.k2),
                        "mod_w" => js::rem(a.k, w) - js::rem(b.k, w),
                        "boolnum_desc" => (b.f as u8 as f64) - (a.f as u8 as f64),
                        "bigint_desc" => bigint::to_number(bigint::sub(b.g, a.g).expect("i128 diff")),
                        _ => unreachable!(),
                    }
                };
                timsort::sort_by(&mut els, &mut cmp);
            }
            let perm: Vec<u32> = els.iter().map(|e| e.i).collect();
            let want_perm: Vec<u32> = arr(&c[3]).iter().map(|x| dnum(x) as u32).collect();
            let trace_ok = if c[4].is_null() {
                true
            } else {
                let want: Vec<u32> = arr(&c[4]).iter().map(|x| dnum(x) as u32).collect();
                want == trace
            };
            r.row(perm == want_perm && trace_ok, || {
                format!("sort {kind} n={} perm_ok={} trace_ok={trace_ok}", keys.len(), perm == want_perm)
            });
        }
        out.push(r);
    }
    {
        let (mut r, cases) = f.get("FlatBinaryHeap")?;
        for c in cases {
            let c = arr(c);
            let cap = dnum(&c[1]) as usize;
            let mut h = FlatBinaryHeap::new(cap);
            let mut ok = true;
            for op in arr(&c[2]) {
                let op = arr(op);
                match op[0].as_str().unwrap() {
                    "e" => h.enqueue(dnum(&op[1]) as u32, dnum(&op[2])),
                    "d" => ok &= h.dequeue() == Ok(dnum(&op[1]) as u32),
                    "c" => h.clear(),
                    "d_empty_throws" => ok &= h.dequeue().is_err(),
                    o => panic!("unknown heap op {o}"),
                }
            }
            let st = h.get_state();
            let want_pri = arr(&c[3]);
            ok &= st.pri.len() == want_pri.len() && st.pri.iter().zip(want_pri).all(|(g, w)| same(*g as f64, dnum(w)));
            let want_tiles: Vec<u32> = arr(&c[4]).iter().map(|x| dnum(x) as u32).collect();
            ok &= st.tiles == want_tiles;
            ok &= st.capacity == dnum(&c[5]) as usize;
            let mut drain = vec![];
            while let Ok(t) = h.dequeue() {
                drain.push(t);
            }
            let want_drain: Vec<u32> = arr(&c[6]).iter().map(|x| dnum(x) as u32).collect();
            ok &= drain == want_drain;
            r.row(ok, || format!("heap {} cap {cap}: state {:?} drain {:?}", c[0], st, drain));
        }
        out.push(r);
    }
    {
        let (mut r, cases) = f.get("map_insertion_order")?;
        for c in cases {
            let mut m: OrdMap<i64, f64> = OrdMap::new();
            let mut ok = true;
            for op in arr(c) {
                let op = arr(op);
                match op[0].as_str().unwrap() {
                    "set" => {
                        m.insert(dnum(&op[1]) as i64, dnum(&op[2]));
                    }
                    "delete" => ok &= op[2].as_bool() == Some(m.remove(&(dnum(&op[1]) as i64)).is_some()),
                    "snapshot" => {
                        let want: Vec<i64> = arr(&op[1]).iter().map(|x| dnum(x) as i64).collect();
                        ok &= m.keys().copied().collect::<Vec<_>>() == want;
                    }
                    o => panic!("unknown map op {o}"),
                }
            }
            r.row(ok, || format!("map ops {c}"));
        }
        out.push(r);
    }
    {
        let (mut r, cases) = f.get("set_iteration_with_mutation")?;
        for c in cases {
            let c = arr(c);
            let mut s: OrdSet<i64> = arr(&c[0]).iter().map(|x| dnum(x) as i64).collect();
            let actions: Vec<(i64, String, i64)> = arr(&c[1])
                .iter()
                .map(|a| {
                    let a = arr(a);
                    (dnum(&a[0]) as i64, a[1].as_str().unwrap().to_string(), dnum(&a[2]) as i64)
                })
                .collect();
            let mut ai = 0;
            let mut visits = vec![];
            let mut cur = s.cursor();
            let mut steps = 0;
            while let Some(v) = s.next_live(&mut cur) {
                visits.push(v);
                steps += 1;
                if steps > 60 {
                    break;
                }
                if ai < actions.len() && actions[ai].0 == v {
                    let (_, ref act, x) = actions[ai];
                    match act.as_str() {
                        "add" => {
                            s.insert(x);
                        }
                        "delete" => {
                            s.remove(&x);
                        }
                        "readd_self" => {
                            s.remove(&v);
                            s.insert(v);
                        }
                        o => panic!("unknown set action {o}"),
                    }
                    ai += 1;
                }
            }
            let want_visits: Vec<i64> = arr(&c[2]).iter().map(|x| dnum(x) as i64).collect();
            let want_final: Vec<i64> = arr(&c[3]).iter().map(|x| dnum(x) as i64).collect();
            let fin: Vec<i64> = s.iter().copied().collect();
            let ok = ai == actions.len() && visits == want_visits && fin == want_final;
            r.row(ok, || format!("set iter {c:?}: visits {visits:?} final {fin:?}"));
        }
        out.push(r);
    }
    {
        let (mut r, cases) = f.get("object_key_order")?;
        for c in cases {
            let c = arr(c);
            let ins: Vec<String> = arr(&c[0]).iter().map(|x| x.as_str().unwrap().to_string()).collect();
            let want: Vec<String> = arr(&c[1]).iter().map(|x| x.as_str().unwrap().to_string()).collect();
            let got = object_keys_order(&ins);
            r.row(got == want, || format!("objkeys {ins:?} = {got:?} want {want:?}"));
        }
        out.push(r);
    }
    Ok(out)
}

/// Markdown table of reports.
pub fn table(reps: &[FnReport]) -> String {
    let mut s = String::from(
        "| suite | function | cases | pass | fail | ood (>i128) | unported | first failure |\n|---|---|---:|---:|---:|---:|---:|---|\n",
    );
    for r in reps {
        s.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
            r.suite,
            r.function,
            r.cases,
            r.pass,
            r.fail,
            r.ood,
            r.unported,
            r.first_fail.as_deref().map(|x| x.chars().take(160).collect::<String>()).unwrap_or_default()
        ));
    }
    s
}
