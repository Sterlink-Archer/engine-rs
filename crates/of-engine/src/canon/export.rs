//! CS v1 extraction from a live `Game` (SPEC §1.2.1, §1.3, §1.5): the canonical extraction
//! order, the drain loop, table sorting and key uniqueness, the section assembly and the K
//! projection, plus `Game::{canonical_state, canon_k, canon_d}`. Owner: **P13**.
//!
//! A literal port of `conformance/src/canon.ts` `canonicalState` (the reference extractor):
//! 1. register `execs` then `unInitExecs` (key = position, [`CanonCx::new`]);
//! 2. G (`snapshotState` order, [`super::sections::g_section`]), then P in smallID order;
//! 3. drain until no table grows: execs (key order), units, attacks, alliances, alliance
//!    requests, stations, railroads, clusters (`canon.ts:113-123`);
//! 4. sort the tables by key (`:124-131`) and assert key uniqueness (`:132-136`);
//! 5. assemble `{tick, G, T, M, P, U, A, D, R, W, E, S}` (`:144-157`); in K mode apply
//!    the K projection (T → {map}, S → null, every TYPED → INT(len), `:158-163`).

use super::exec_rows::exec_row;
use super::sections::{g_section, m_section, t_section};
use super::{CanonCx, CanonError, Cs};
use crate::Game;
use of_canon::digest::{self, CanonState, Digests};

/// Which projection to extract (SPEC §1.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanonMode {
    /// The full CS (D digests, dumps).
    Full,
    /// The K projection (K digests): S is never computed, T.miniMap is dropped.
    Key,
}

/// A row with its sort key.
struct Row<K> {
    key: K,
    v: Cs,
}

/// `rows.sort(cmp)` on the keys (V8 TimSort, `of_det::js_sort_by`), moving each row once
/// instead of cloning CS trees inside the sort.
fn sort_rows<K: Clone>(rows: &mut Vec<Row<K>>, mut cmp: impl FnMut(&K, &K) -> f64) {
    let mut idx: Vec<(K, usize)> = rows.iter().enumerate().map(|(i, r)| (r.key.clone(), i)).collect();
    of_det::js_sort_by(&mut idx, |a, b| cmp(&a.0, &b.0));
    let mut slots: Vec<Option<Row<K>>> = rows.drain(..).map(Some).collect();
    for (_, i) in idx {
        if let Some(r) = slots[i].take() {
            rows.push(r); // in-range: i < slots.len() (an index of the original rows)
        }
    }
}

fn attack_cmp(a: &(u16, String), b: &(u16, String)) -> f64 {
    // TS: `a.attacker - b.attacker || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0)`
    let d = f64::from(a.0) - f64::from(b.0);
    if d != 0.0 { d } else { str_cmp(&a.1, &b.1) }
}

/// The CS v1 of `game` (SPEC §1). In [`CanonMode::Key`] the result is already the K
/// projection (feed it to `digest_key`'s section loop through [`digest_k_projected`]).
pub fn canonical_state(game: &Game, mode: CanonMode) -> Result<CanonState, CanonError> {
    let st = &game.state;
    let ex = &game.execs;
    let mut cx = CanonCx::with_mode(st, ex, mode);

    // (2) G, then P.
    let g = g_section(&mut cx);
    let mut players = Vec::with_capacity(st.players.len());
    for p in &st.players {
        players.push(crate::canon_or_missing!(*p, &mut cx).map_err(|e| e.at(format!("P[smallID={}]", p.small_id.0)))?);
    }

    // (3) drain loop (`canon.ts:108-123`): each drain extends its output while its table grows.
    let mut execs: Vec<Cs> = Vec::new();
    let mut units: Vec<Row<u32>> = Vec::new();
    let mut attacks: Vec<Row<(u16, String)>> = Vec::new();
    let mut alliances: Vec<Row<u32>> = Vec::new();
    let mut reqs: Vec<Row<String>> = Vec::new();
    let mut stations: Vec<Row<u32>> = Vec::new();
    let mut railroads: Vec<Row<u32>> = Vec::new();
    let mut clusters: Vec<Row<String>> = Vec::new();
    loop {
        let before = execs.len()
            + units.len()
            + attacks.len()
            + alliances.len()
            + reqs.len()
            + stations.len()
            + railroads.len()
            + clusters.len();
        while execs.len() < cx.exec_order.len() {
            let id = cx.exec_order[execs.len()]; // in-range: checked by the loop condition
            let pos = execs.len();
            let row = exec_row(&ex.ar, id, &mut cx).map_err(|e| e.at(format!("E[{pos}:{}]", id.kind.as_str())))?;
            execs.push(row);
        }
        while units.len() < cx.unit_queue.len() {
            let u = cx.unit_queue[units.len()]; // in-range: loop condition
            let unit = st.units.get(u);
            let v = crate::canon_or_missing!(*unit, &mut cx).map_err(|e| e.at(format!("U[id={}]", unit.id)))?;
            units.push(Row { key: unit.id, v });
        }
        while attacks.len() < cx.attack_queue.len() {
            let a = cx.attack_queue[attacks.len()]; // in-range: loop condition
            let att = st.attacks.get(a);
            let id = att.id.to_ts_string();
            let v = crate::canon_or_missing!(*att, &mut cx).map_err(|e| e.at(format!("A[{id}]")))?;
            attacks.push(Row { key: (att.attacker.0, id), v });
        }
        while alliances.len() < cx.alliance_queue.len() {
            let a = cx.alliance_queue[alliances.len()]; // in-range: loop condition
            let al = &st.alliances[a.0 as usize]; // in-range: alliance_key checked it
            let v = crate::canon_or_missing!(*al, &mut cx).map_err(|e| e.at(format!("D.alliances[id={}]", a.0)))?;
            alliances.push(Row { key: a.0, v });
        }
        while reqs.len() < cx.request_queue.len() {
            let r = cx.request_queue[reqs.len()]; // in-range: loop condition
            let rq = &st.alliance_reqs[r.0 as usize]; // in-range: request_key checked it
            #[allow(clippy::expect_used)] // rust-invariant: request_key checked the index
            let key = CanonCx::request_key_str(st, r).expect("queued request exists");
            let v = crate::canon_or_missing!(*rq, &mut cx).map_err(|e| e.at(format!("D.allianceRequests[{key}]")))?;
            // TS: `({ key: reqKey(r), ...r.snapshot(w) })` (N6)
            reqs.push(Row { key: key.clone(), v: with_key_first(Cs::Str(key), v) });
        }
        while stations.len() < cx.station_queue.len() {
            let s = cx.station_queue[stations.len()]; // in-range: loop condition
            let Some(ts) = st.rail.station_manager.by_id.get(s.0 as usize).and_then(Option::as_ref) else {
                return Err(CanonError::new(format!("R.stations: station {} is referenced but not stored", s.0)));
            };
            let v = crate::canon_or_missing!(*ts, &mut cx).map_err(|e| e.at(format!("R.stations[id={}]", s.0)))?;
            stations.push(Row { key: s.0, v });
        }
        while railroads.len() < cx.railroad_queue.len() {
            let r = cx.railroad_queue[railroads.len()]; // in-range: loop condition
            let rr = st.rail.railroads.get(r);
            let v = crate::canon_or_missing!(*rr, &mut cx).map_err(|e| e.at(format!("R.railroads[id={}]", rr.id.0)))?;
            railroads.push(Row { key: rr.id.0, v });
        }
        while clusters.len() < cx.cluster_queue.len() {
            let c = cx.cluster_queue[clusters.len()]; // in-range: loop condition
            let key = match cx.cluster_key(c) {
                Cs::Str(k) => k,
                _ => return Err(cx.errors.first().cloned().unwrap_or_else(|| CanonError::new("cluster key"))),
            };
            let cl = st.rail.clusters.get(c);
            let v = crate::canon_or_missing!(*cl, &mut cx).map_err(|e| e.at(format!("R.clusters[{key}]")))?;
            // TS: `({ key: w.cluster(c), ...c.snapshot(w) })` (N6)
            clusters.push(Row { key: key.clone(), v: with_key_first(Cs::Str(key), v) });
        }
        let after = execs.len()
            + units.len()
            + attacks.len()
            + alliances.len()
            + reqs.len()
            + stations.len()
            + railroads.len()
            + clusters.len();
        if after == before {
            break;
        }
    }
    if let Some(e) = cx.errors.first() {
        return Err(e.clone());
    }

    // (4) sort (`canon.ts:124-131`): every comparator is consistent, and V8's sort is stable,
    // so `js_sort_by` reproduces the TS order exactly (equal keys keep first-reference order).
    let by_num = |a: &u32, b: &u32| f64::from(*a) - f64::from(*b);
    let by_str = |a: &String, b: &String| str_cmp(a, b);
    sort_rows(&mut units, by_num);
    sort_rows(&mut attacks, attack_cmp);
    sort_rows(&mut alliances, by_num);
    sort_rows(&mut reqs, by_str);
    sort_rows(&mut stations, by_num);
    sort_rows(&mut railroads, by_num);
    sort_rows(&mut clusters, by_str);
    // `assertUnique(rows, k)` compares adjacent rows' `k` (`canon.ts:171-173`); attacks are
    // checked on `id` alone, so equal ids of different attackers that end up adjacent throw too.
    assert_unique(units.iter().map(|r| r.key.to_string()), "unit")?;
    assert_unique(attacks.iter().map(|r| r.key.1.clone()), "attack")?;
    assert_unique(reqs.iter().map(|r| r.key.clone()), "allianceRequest")?;
    assert_unique(stations.iter().map(|r| r.key.to_string()), "station")?;
    assert_unique(railroads.iter().map(|r| r.key.to_string()), "railroad")?;

    // (5) assembly (`canon.ts:144-157`).
    let mut gf = g.fields;
    gf.push((
        "execTypes".into(),
        Cs::Arr(cx.exec_order.iter().map(|id| Cs::Str(id.kind.as_str().to_owned())).collect()),
    ));
    let rail = g.rail.map_err(|e| e.at("R.network"))?;
    let stats = match g.stats {
        None => Cs::Null,
        Some(s) => s.map_err(|e| e.at("S"))?,
    };
    let rows = |v: Vec<Row<u32>>| Cs::Arr(v.into_iter().map(|r| r.v).collect());
    let srows = |v: Vec<Row<String>>| Cs::Arr(v.into_iter().map(|r| r.v).collect());
    let sec = [
        Cs::Obj(gf),
        t_section(st, mode),
        m_section(st),
        Cs::Arr(players),
        rows(units),
        Cs::Arr(attacks.into_iter().map(|r| r.v).collect()),
        Cs::Obj(vec![("alliances".into(), rows(alliances)), ("allianceRequests".into(), srows(reqs))]),
        Cs::Obj(vec![
            ("network".into(), rail),
            ("stations".into(), rows(stations)),
            ("railroads".into(), rows(railroads)),
            ("clusters".into(), srows(clusters)),
        ]),
        Cs::Obj(vec![("water".into(), g.water), ("sharedWaterCache".into(), g.shared_water)]),
        Cs::Arr(execs),
        stats,
    ];
    let tick = u32::try_from(st.ticks).map_err(|_| CanonError::new(format!("negative tick {}", st.ticks)))?;
    let cs = CanonState { tick, sec };
    Ok(match mode {
        CanonMode::Full => cs,
        CanonMode::Key => digest::key_projection(&cs),
    })
}

/// JS `a < b ? -1 : a > b ? 1 : 0` on strings: UTF-16 code-unit order (PORTING_GUIDE §9).
/// Every CS key string is ASCII, where that equals byte order.
fn str_cmp(a: &str, b: &str) -> f64 {
    match a.encode_utf16().cmp(b.encode_utf16()) {
        std::cmp::Ordering::Less => -1.0,
        std::cmp::Ordering::Greater => 1.0,
        std::cmp::Ordering::Equal => 0.0,
    }
}

/// `{ key, ...row }`: the key field first (key order is irrelevant to CSE1, which sorts).
fn with_key_first(key: Cs, row: Cs) -> Cs {
    match row {
        Cs::Obj(mut f) => {
            f.retain(|(k, _)| k != "key");
            f.insert(0, ("key".into(), key));
            Cs::Obj(f)
        }
        other => other,
    }
}

/// `assertUnique` (`canon.ts:171-173`).
fn assert_unique(keys: impl Iterator<Item = String>, what: &str) -> Result<(), CanonError> {
    let mut prev: Option<String> = None;
    for k in keys {
        if prev.as_deref() == Some(k.as_str()) {
            return Err(CanonError::new(format!("CS v1: duplicate {what} key {k} — spec §1.2 fallback needed")));
        }
        prev = Some(k);
    }
    Ok(())
}

/// The section digests of an already K-projected CS (K_t, SPEC §1.5).
pub fn digest_k_projected(k: &CanonState) -> Result<Digests, CanonError> {
    let mut w = of_canon::cse::CseWriter::with_capacity(1 << 20);
    let sec = digest::section_digests(k, &mut w).map_err(|e| CanonError::new(format!("CSE1: {e}")))?;
    Ok(Digests { tick: k.tick, all: digest::combine(digest::MAGIC_KEY, k.tick, &sec), sec })
}

/// Magic of the exec-boundary probe digest: "OFCP".
pub const MAGIC_PROBE: u32 = 0x4f46_4350;

/// The SPEC §3.2.6 exec-boundary probe digest of `st`: K restricted to {T.map, P scalars, U,
/// A}, i.e. the K projection of those four sections (P/U/A rows through their owners'
/// `Canon`; typed arrays → lengths), combined like K with magic `"OFCP"` over the four section
/// digests in that order. It needs no execution (the probe runs between two `live[i]`
/// ticks, when the exec store is borrowed by the loop): the rows reached from P, U and A never
/// name an execution. The TS side (the probe patch, ledger t073) must hash the same four
/// projected sections.
pub fn probe_digest(st: &crate::state::State) -> Result<u64, CanonError> {
    let ex = crate::exec::ExecStore::default();
    let mut cx = CanonCx::with_mode(st, &ex, CanonMode::Key);
    let mut players = Vec::with_capacity(st.players.len());
    for p in &st.players {
        players.push(crate::canon_or_missing!(*p, &mut cx).map_err(|e| e.at(format!("P[smallID={}]", p.small_id.0)))?);
    }
    let mut units: Vec<Row<u32>> = Vec::new();
    let mut attacks: Vec<Row<(u16, String)>> = Vec::new();
    loop {
        let before = units.len() + attacks.len();
        while units.len() < cx.unit_queue.len() {
            let unit = st.units.get(cx.unit_queue[units.len()]); // in-range: loop condition
            let v = crate::canon_or_missing!(*unit, &mut cx).map_err(|e| e.at(format!("U[id={}]", unit.id)))?;
            units.push(Row { key: unit.id, v });
        }
        while attacks.len() < cx.attack_queue.len() {
            let att = st.attacks.get(cx.attack_queue[attacks.len()]); // in-range: loop condition
            let v = crate::canon_or_missing!(*att, &mut cx).map_err(|e| e.at("A"))?;
            attacks.push(Row { key: (att.attacker.0, att.id.to_ts_string()), v });
        }
        if units.len() + attacks.len() == before {
            break;
        }
    }
    if let Some(e) = cx.errors.first() {
        return Err(e.clone());
    }
    sort_rows(&mut units, |a, b| f64::from(*a) - f64::from(*b));
    sort_rows(&mut attacks, attack_cmp);
    let t = super::sections::t_section(st, CanonMode::Key);
    let secs = [
        t,
        Cs::Arr(players),
        Cs::Arr(units.into_iter().map(|r| r.v).collect()),
        Cs::Arr(attacks.into_iter().map(|r| r.v).collect()),
    ];
    let mut w = of_canon::cse::CseWriter::with_capacity(1 << 16);
    let mut h = of_canon::fnv::Fnv64w::new();
    h.word(MAGIC_PROBE).word(digest::CS_VERSION).word(st.ticks as u32);
    for (i, s) in secs.into_iter().enumerate() {
        // P/U/A in their K projection (TYPED → INT(len)); T.map stays raw.
        let mut cs = CanonState { tick: 0, sec: std::array::from_fn(|_| Cs::Null) };
        cs.sec[0] = s;
        let v = if i == 0 { cs.sec[0].clone() } else { digest::key_projection(&cs).sec[0].clone() };
        w.clear();
        w.value(&v).map_err(|e| CanonError::new(format!("CSE1: {e}")))?;
        let d = of_canon::fnv::Fnv64w::new().bytes(&w.buf).h;
        h.word((d >> 32) as u32).word(d as u32);
    }
    Ok(h.h)
}

impl Game {
    /// The full CS v1 of the current state (SPEC §1).
    pub fn canon_full(&self) -> Result<CanonState, CanonError> {
        canonical_state(self, CanonMode::Full)
    }
    /// K_t (SPEC §1.5): the K projection's digest. Never computes S (so a build without
    /// `stats-full` can claim L2, ARCHITECTURE §3.10).
    pub fn canon_k(&self) -> Result<u64, CanonError> {
        digest_k_projected(&canonical_state(self, CanonMode::Key)?).map(|d| d.all)
    }
    /// D_t and the 11 section digests (SPEC §1.5).
    pub fn canon_d(&self) -> Result<Digests, CanonError> {
        digest::digest_full(&self.canon_full()?).map_err(|e| CanonError::new(format!("CSE1: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::State;
    use of_canon::digest::Section;

    #[test]
    fn empty_game_k_and_sections() {
        let g = Game::from_state(State::default(), "g");
        // An empty game still has a rail network (R.network, P5's `Canon`) and, for D, an S
        // section (stats-full): until those owners land the exporter reports them as missing.
        match canonical_state(&g, CanonMode::Key) {
            Ok(k) => {
                assert_eq!(k.get(Section::S), &Cs::Null);
                assert_eq!(k.get(Section::T).get("miniMap"), None);
                assert_eq!(k.get(Section::E), &Cs::Arr(vec![]));
                let kd = g.canon_k().expect("K");
                // The K digest of the K projection equals of-canon's digest_key of the same tree.
                assert_eq!(digest::digest_key(&k).expect("digest").all, kd);
            }
            Err(e) => assert!(e.is_missing() && e.0.starts_with("R.network"), "{e}"),
        }
        match g.canon_d() {
            Ok(_) => {}
            Err(e) => assert!(e.is_missing(), "{e}"),
        }
    }

    #[test]
    fn probe_digest_of_an_empty_state() {
        let st = State::default();
        let a = probe_digest(&st).expect("no rows, no owner impl needed");
        let mut st2 = State::default();
        st2.ticks = 1;
        assert_ne!(a, probe_digest(&st2).expect("probe"), "the tick is hashed");
    }

    #[test]
    fn sort_helpers_match_js() {
        // ':' (0x3a) sorts after '0' (0x30): "1:29:673" > "10:9:671", as in the dumps.
        assert_eq!(str_cmp("1:29:673", "10:9:671"), 1.0);
        assert_eq!(str_cmp("c10", "c9"), -1.0);
        assert_eq!(str_cmp("e0", "c1"), 1.0);
        assert!(assert_unique(["1", "2", "2"].into_iter().map(String::from), "unit").is_err());
        assert!(assert_unique(["1", "2", "1"].into_iter().map(String::from), "unit").is_ok());
        let r = with_key_first(Cs::Str("k".into()), Cs::Obj(vec![("a".into(), Cs::Int(1))]));
        assert_eq!(r, Cs::Obj(vec![("key".into(), Cs::Str("k".into())), ("a".into(), Cs::Int(1))]));
    }
}
