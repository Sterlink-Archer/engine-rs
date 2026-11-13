//! T1/T2 replay of the P4b golden vectors (`tests/p4b_vectors/p4b_vectors.json.zst`,
//! generated from the REAL upstream code by `tests/p4b_vectors/gen_p4b_vectors.mts`; maps alongside under `tests/p4b_vectors/maps/`). Owner: **P4b**.
//!
//! Every case compares the Rust output with the TS output **exactly** (tile lists, f64
//! values parsed with `float_roundtrip`). Suites: `line`, `parabola`, `air`, `astar_water`,
//! `bounded`, `rail`, `smoothing`, `shore`, `simple_chain`, `rail_chain`, `memo`,
//! `water_pf`, `steppers`, `spatial`.
#![allow(clippy::disallowed_methods)] // TS-throw cases are asserted with catch_unwind

use crate::ctx::Ctx;
use crate::map::GameMap;
use crate::path::astar::AStarBuffers;
use crate::path::astar_water_bounded::{self, BoundedAStarConfig, SearchBounds};
use crate::path::line::{DistanceBasedBezierCurve, Point};
use crate::path::parabola::{ParabolaOptions, ParabolaPathFinder, get_parabola_control_points_m};
use crate::path::pathfinder::{AirStepper, RailStepper, WaterPathFinder, WaterPathMemo};
use crate::path::testkit::map_from_terrain;
use crate::path::transform::{shore, smoothing};
use crate::path::types::{PathResult, Src};
use crate::scratch::Scratch;
use crate::scratch::path::{AStarRailScratch, AStarWaterBoundedScratch, AStarWaterScratch};
use crate::state::State;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/p4b_vectors")
}

fn doc() -> &'static Value {
    static DOC: OnceLock<Value> = OnceLock::new();
    DOC.get_or_init(|| {
        let raw = of_trace::trace::zstd_read(&dir().join("p4b_vectors.json.zst")).expect("p4b_vectors.json.zst");
        let v: Value = serde_json::from_slice(&raw).expect("json");
        assert_eq!(v["format"], "p4b-path-vectors/1");
        v
    })
}

/// `(main, mini)` of a vector map (raw bins, or `.zst` for the large ones).
fn maps(name: &str) -> &'static (GameMap, GameMap) {
    static MAPS: OnceLock<std::sync::Mutex<BTreeMap<String, &'static (GameMap, GameMap)>>> = OnceLock::new();
    let m = MAPS.get_or_init(Default::default);
    let mut g = m.lock().expect("lock");
    if let Some(p) = g.get(name) {
        return p;
    }
    let d = dir().join("maps").join(name);
    let man: Value = serde_json::from_slice(&std::fs::read(d.join("manifest.json")).expect("manifest")).expect("json");
    let read = |f: &str| -> Vec<u8> {
        let zst = d.join(format!("{f}.zst"));
        if zst.exists() { of_trace::trace::zstd_read(&zst).expect("zstd") } else { std::fs::read(d.join(f)).expect(f) }
    };
    let dims = |k: &str| (man[k]["width"].as_u64().unwrap() as u32, man[k]["height"].as_u64().unwrap() as u32);
    let (w, h) = dims("map");
    let (mw, mh) = dims("map4x");
    let mut main = map_from_terrain(w, h, read("map.bin"));
    main.num_land_tiles = man["map"]["num_land_tiles"].as_u64().unwrap() as u32;
    let mini = map_from_terrain(mw, mh, read("map4x.bin"));
    let leaked: &'static (GameMap, GameMap) = Box::leak(Box::new((main, mini)));
    g.insert(name.to_string(), leaked);
    leaked
}

fn u32s(v: &Value) -> Vec<u32> {
    v.as_array().expect("array").iter().map(|x| x.as_u64().expect("u32") as u32).collect()
}
fn opt_path(v: &Value) -> Option<Vec<u32>> {
    if v.is_null() { None } else { Some(u32s(v)) }
}
fn f(v: &Value) -> f64 {
    v.as_f64().expect("number")
}
fn src_of(v: &Value) -> (Vec<u32>, bool) {
    if v.is_array() { (u32s(v), true) } else { (vec![v.as_u64().unwrap() as u32], false) }
}
fn mk_src(v: &(Vec<u32>, bool)) -> Src<'_> {
    if v.1 { Src::Many(&v.0) } else { Src::One(v.0[0]) }
}
fn status_of<T: Copy + Into<u64>>(r: &PathResult<T>) -> (u64, Option<u64>) {
    match *r {
        PathResult::Next(n) => (0, Some(n.into())),
        PathResult::Complete(n) => (2, Some(n.into())),
        PathResult::NotFound => (3, None),
    }
}
fn cases(suite: &str) -> &'static [Value] {
    let v = &doc()["cases"][suite];
    let a = if v.is_array() { v } else { &v["cases"] };
    a.as_array().unwrap_or_else(|| panic!("suite {suite}"))
}
/// Run `f`, returning `Err(msg)` for a `TsThrow` (any other panic propagates).
fn ts<R>(f: impl FnOnce() -> R) -> Result<R, String> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => Ok(r),
        Err(p) => match of_det::throw::as_ts_throw(&*p) {
            Some(t) => Err(t.msg.to_string()),
            None => std::panic::resume_unwind(p),
        },
    }
}

#[test]
fn line_curves() {
    let cs = cases("line");
    assert!(cs.len() >= 300);
    for (i, c) in cs.iter().enumerate() {
        let p: Vec<Point> = c["pts"].as_array().unwrap().iter().map(|q| Point { x: f(&q[0]), y: f(&q[1]) }).collect();
        let mut curve = DistanceBasedBezierCurve::new(p[0], p[1], p[2], p[3], f(&c["increment"]));
        let cached: Vec<f64> = curve.get_all_points().iter().flat_map(|q| [q.x, q.y]).collect();
        let want: Vec<f64> = c["cached"].as_array().unwrap().iter().map(f).collect();
        assert_eq!(cached, want, "line[{i}] cachedPoints");
        assert_eq!(
            DistanceBasedBezierCurve::get_length(p[0], p[1], p[2], p[3]) * 256.0,
            f(&c["length256"]),
            "line[{i}] getLength"
        );
        for (s, st) in c["steps"].as_array().unwrap().iter().enumerate() {
            let got = curve.increment(f(&st[0]));
            let want = if st[1].is_null() { None } else { Some(Point { x: f(&st[1][0]), y: f(&st[1][1]) }) };
            assert_eq!(got, want, "line[{i}] step {s}");
            assert_eq!(f64::from(curve.current_index), f(&st[2]), "line[{i}] step {s} currentIndex");
            assert_eq!(f64::from(curve.accumulated_distance_scaled), f(&st[3]), "line[{i}] step {s} acc");
        }
    }
}

fn parabola_options(v: &Value) -> Option<ParabolaOptions> {
    if v.is_null() {
        return None;
    }
    Some(ParabolaOptions {
        increment: v.get("increment").map(f),
        distance_based_height: v.get("distanceBasedHeight").map(|b| b.as_bool().unwrap()),
        direction_up: v.get("directionUp").map(|b| b.as_bool().unwrap()),
        ignore_map_bounds: v.get("ignoreMapBounds").map(|b| b.as_bool().unwrap()),
    })
}

#[test]
fn parabola_paths_and_steps() {
    let dims = &doc()["cases"]["parabola"]["map"];
    let (w, h) = (dims[0].as_u64().unwrap() as u32, dims[1].as_u64().unwrap() as u32);
    let map = map_from_terrain(w, h, vec![0x20; (w * h) as usize]);
    for (i, c) in cases("parabola").iter().enumerate() {
        let (from, to) = (c["from"].as_u64().unwrap() as u32, c["to"].as_u64().unwrap() as u32);
        let opts = parabola_options(&c["options"]);
        let cp: Vec<f64> =
            get_parabola_control_points_m(&map, from, to, opts.as_ref()).iter().flat_map(|p| [p.x, p.y]).collect();
        let want_cp: Vec<f64> = c["cp"].as_array().unwrap().iter().flat_map(|p| [f(&p[0]), f(&p[1])]).collect();
        assert_eq!(cp, want_cp, "parabola[{i}] control points");
        let mut pf = ParabolaPathFinder::new(parabola_options(&c["options"]));
        let got = ts(|| pf.find_path_m(&map, from, to));
        match c["find"].get("throw") {
            Some(m) => assert_eq!(got.err().as_deref(), m.as_str(), "parabola[{i}] find throw"),
            None => assert_eq!(got.expect("no throw"), Some(u32s(&c["find"])), "parabola[{i}] findPath"),
        }
        let speed = c["speed"].as_f64();
        let mut stepper = ParabolaPathFinder::new(parabola_options(&c["options"]));
        let mut cur = from;
        for (s, st) in c["steps"].as_array().unwrap().iter().enumerate() {
            let r = ts(|| stepper.next_m(&map, cur, to, speed));
            if let Some(m) = st.get("throw") {
                assert_eq!(r.err().as_deref(), m.as_str(), "parabola[{i}] step {s} throw");
                break;
            }
            let r = r.expect("no throw");
            let (code, node) = status_of(&r.map(u64::from));
            assert_eq!(code, st[0].as_u64().unwrap(), "parabola[{i}] step {s} status");
            assert_eq!(node, st[1].as_u64(), "parabola[{i}] step {s} node");
            assert_eq!(i64::from(stepper.current_index()), st[2].as_i64().unwrap(), "parabola[{i}] step {s} index");
            if let PathResult::Next(n) = r {
                cur = n;
            }
        }
    }
}

#[test]
fn air_paths() {
    let map = map_from_terrain(64, 64, vec![0x20; 64 * 64]);
    for (i, c) in cases("air").iter().enumerate() {
        let seed = c["seed"].as_i64().unwrap() as i32;
        let got = crate::path::air::find_path_m(
            &map,
            seed,
            c["from"].as_u64().unwrap() as u32,
            c["to"].as_u64().unwrap() as u32,
        );
        assert_eq!(got, u32s(&c["path"]), "air[{i}] seed {seed}");
    }
}

#[test]
fn astar_water_paths() {
    let mut sc: BTreeMap<String, AStarWaterScratch> = BTreeMap::new();
    for (i, c) in cases("astar_water").iter().enumerate() {
        let name = c["map"].as_str().unwrap();
        let (_, mini) = maps(name);
        let start = src_of(&c["start"]).0;
        let got = crate::path::astar_water::find_path_m(
            sc.entry(name.into()).or_default(),
            mini,
            Default::default(),
            &start,
            c["goal"].as_u64().unwrap() as u32,
        );
        assert_eq!(got, opt_path(&c["path"]), "astar_water[{i}] on {name}");
    }
}

#[test]
fn bounded_paths() {
    let mut sc: BTreeMap<(String, u64), AStarWaterBoundedScratch> = BTreeMap::new();
    for (i, c) in cases("bounded").iter().enumerate() {
        let name = c["map"].as_str().unwrap();
        let cap = c["cap"].as_u64().unwrap();
        let (main, _) = maps(name);
        let s = sc.entry((name.into(), cap)).or_default();
        let start = u32s(&c["start"]);
        let goal = c["goal"].as_u64().unwrap() as u32;
        let got = if c["op"] == "find" {
            astar_water_bounded::find_path_m(s, main, cap as u32, BoundedAStarConfig::default(), &start, goal)
        } else {
            let b = u32s(&c["bounds"]);
            let bounds = SearchBounds { min_x: b[0], max_x: b[1], min_y: b[2], max_y: b[3] };
            astar_water_bounded::search_bounded_m(
                s,
                main,
                cap as u32,
                BoundedAStarConfig::default(),
                &start,
                goal,
                bounds,
            )
        };
        assert_eq!(got, opt_path(&c["path"]), "bounded[{i}] {} on {name} cap {cap}", c["op"]);
    }
}

#[test]
fn rail_paths() {
    let mut sc: BTreeMap<String, AStarRailScratch> = BTreeMap::new();
    for (i, c) in cases("rail").iter().enumerate() {
        let name = c["map"].as_str().unwrap();
        let (_, mini) = maps(name);
        let start = src_of(&c["start"]).0;
        let got = crate::path::astar_rail::find_path_m(
            sc.entry(name.into()).or_default(),
            mini,
            &start,
            c["goal"].as_u64().unwrap() as u32,
        );
        assert_eq!(got, opt_path(&c["path"]), "rail[{i}] on {name}");
    }
}

#[test]
fn smoothing_paths() {
    let mut a = AStarWaterScratch::default();
    let mut s = AStarWaterBoundedScratch::default();
    for (i, c) in cases("smoothing").iter().enumerate() {
        let (_, mini) = maps(c["map"].as_str().unwrap());
        let (from, to) = (c["from"].as_u64().unwrap() as u32, c["to"].as_u64().unwrap() as u32);
        let raw = crate::path::astar_water::find_path_m(&mut a, mini, Default::default(), &[from], to);
        assert_eq!(raw, opt_path(&c["raw"]), "smoothing[{i}] raw");
        let sm = raw.map(|p| smoothing::smooth_m(&mut s, mini, p));
        assert_eq!(sm, opt_path(&c["smoothed"]), "smoothing[{i}] smoothed");
    }
}

#[test]
fn shore_paths() {
    let mut a = AStarWaterScratch::default();
    for (i, c) in cases("shore").iter().enumerate() {
        let (_, mini) = maps(c["map"].as_str().unwrap());
        let from = src_of(&c["from"]);
        let got = shore::find_path(mini, mk_src(&from), c["to"].as_u64().unwrap() as u32, |f, t| {
            let mut one = [0];
            crate::path::astar_water::find_path_m(&mut a, mini, Default::default(), f.as_slice(&mut one), t)
        });
        assert_eq!(got, opt_path(&c["path"]), "shore[{i}]");
    }
}

#[test]
fn simple_chain_paths() {
    let mut a = AStarWaterScratch::default();
    for (i, c) in cases("simple_chain").iter().enumerate() {
        let (main, mini) = maps(c["map"].as_str().unwrap());
        let from = src_of(&c["from"]);
        let to = c["to"].as_u64().unwrap() as u32;
        let got = ts(|| crate::path::builder::simple_water_chain(main, mini, &mut a, mk_src(&from), to));
        match c["path"].get("throw") {
            Some(m) => assert_eq!(got.err().as_deref(), m.as_str(), "simple_chain[{i}] throw"),
            None => assert_eq!(got.expect("no throw"), opt_path(&c["path"]), "simple_chain[{i}]"),
        }
    }
}

#[test]
fn rail_chain_paths() {
    let mut sc = AStarRailScratch { astar: AStarBuffers::default() };
    for (i, c) in cases("rail_chain").iter().enumerate() {
        let (main, mini) = maps(c["map"].as_str().unwrap());
        let got = crate::path::builder::rail_chain(
            main,
            mini,
            &mut sc,
            Src::One(c["from"].as_u64().unwrap() as u32),
            c["to"].as_u64().unwrap() as u32,
        );
        assert_eq!(got, opt_path(&c["path"]), "rail_chain[{i}]");
    }
}

/// The LRU memo over the world simple chain at a 6 kB budget: outputs, entry/byte counts
/// and the full key order after every query.
#[test]
fn memo_lru_sequence() {
    let max = doc()["cases"]["memo"]["max_bytes"].as_u64().unwrap();
    let (main, mini) = maps("world");
    let mut memo = WaterPathMemo::with_max_bytes(u64::from(main.w) * u64::from(main.h), 0, max);
    let mut a = AStarWaterScratch::default();
    for (i, c) in cases("memo").iter().enumerate() {
        let from = src_of(&c["from"]);
        let wv = c["wv"].as_u64().unwrap() as u32;
        let got = memo.find_path(wv, mk_src(&from), c["to"].as_u64().unwrap() as u32, |f, t| {
            crate::path::builder::simple_water_chain(main, mini, &mut a, f, t)
        });
        assert_eq!(got, opt_path(&c["path"]), "memo[{i}] path");
        assert_eq!(memo.entry_count() as u64, c["entries"].as_u64().unwrap(), "memo[{i}] entries");
        assert_eq!(memo.byte_count(), c["bytes"].as_u64().unwrap(), "memo[{i}] bytes");
        let keys: Vec<u64> = memo.paths.keys().copied().collect();
        let want: Vec<u64> = c["keys"].as_array().unwrap().iter().map(|k| k.as_u64().unwrap()).collect();
        assert_eq!(keys, want, "memo[{i}] LRU order");
    }
}

fn world_state() -> State {
    let (main, mini) = maps("world");
    let mut st = State::default();
    st.map = main.clone();
    st.mini = mini.clone();
    st
}

/// Three `WaterPathFinder`s (stagger 3 memoised, 0, 2) over random next/findPath/
/// pathForTraversal/rebuilt calls with graph-version and water-version bumps: every result,
/// the full finder state after each op, and the memoised finder's memo counts.
#[test]
fn water_path_finder_sequence() {
    let case = &cases("water_pf")[0];
    let mut st = world_state();
    let mut sc = Scratch::new();
    let mut finders: Vec<WaterPathFinder> = {
        let mut cx = Ctx::new(&mut st, &mut sc, None);
        case["finders"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| WaterPathFinder::new(&mut cx, p[0].as_i64().unwrap() as i32, p[1].as_bool().unwrap()))
            .collect()
    };
    for (i, o) in case["ops"].as_array().unwrap().iter().enumerate() {
        let rec = &o["rec"];
        let k = rec["k"].as_u64().unwrap_or(0) as usize;
        let op = rec["op"].as_str().unwrap();
        match op {
            "bump_graph" => st.water.water_graph_version = rec["v"].as_u64().unwrap() as u32,
            "bump_water" => st.map.water_version = rec["wv"].as_u64().unwrap() as u32,
            _ => {
                let mut cx = Ctx::new(&mut st, &mut sc, None);
                let f = &mut finders[k];
                let from = rec["from"].as_u64().map(|x| x as u32);
                let to = rec["to"].as_u64().map(|x| x as u32);
                match op {
                    "next" => {
                        let r = f.next(&mut cx, from.unwrap(), to.unwrap(), rec["dist"].as_f64());
                        let (code, node) = status_of(&r.map(u64::from));
                        assert_eq!(
                            (code, node),
                            (rec["status"].as_u64().unwrap(), rec["node"].as_u64()),
                            "water_pf[{i}] next"
                        );
                    }
                    "find" => {
                        let got = f.find_path(&mut cx, &[from.unwrap()], to.unwrap());
                        assert_eq!(got, opt_path(&rec["path"]), "water_pf[{i}] findPath");
                    }
                    "traversal" => {
                        let got = f.path_for_traversal(&mut cx, from.unwrap(), to.unwrap());
                        assert_eq!(got, u32s(&rec["path"]), "water_pf[{i}] pathForTraversal");
                    }
                    "rebuilt" => {
                        assert_eq!(f.take_rebuilt(&mut cx), rec["v"].as_bool().unwrap(), "water_pf[{i}] rebuilt")
                    }
                    other => panic!("op {other}"),
                }
            }
        }
        let f = &finders[if op.starts_with("bump") { 0 } else { k }];
        if !op.starts_with("bump") {
            let s = &o["state"];
            assert_eq!(u64::from(f.water_graph_version), s[0].as_u64().unwrap(), "water_pf[{i}] waterGraphVersion");
            assert_eq!(f.rebuilt, s[1].as_bool().unwrap(), "water_pf[{i}] rebuilt flag");
            assert_eq!(i64::from(f.stagger_countdown), s[2].as_i64().unwrap(), "water_pf[{i}] countdown");
            assert_eq!(f.pending_version, s[3].as_i64().unwrap(), "water_pf[{i}] pendingVersion");
            assert_eq!(u64::from(f.stepper.path_index), s[4].as_u64().unwrap(), "water_pf[{i}] pathIndex");
            assert_eq!(f.stepper.last_to.map(u64::from), s[5].as_u64(), "water_pf[{i}] lastTo");
            assert_eq!(f.stepper.path.as_ref().map(|p| p.len() as u64), s[6].as_u64(), "water_pf[{i}] path len");
            if let Some(m) = o["memo"].as_array() {
                let slot = st.water.chains.slot(f.chain_version).expect("slot");
                assert_eq!(slot.memo.entry_count() as u64, m[0].as_u64().unwrap(), "water_pf[{i}] memo entries");
                assert_eq!(slot.memo.byte_count(), m[1].as_u64().unwrap(), "water_pf[{i}] memo bytes");
            }
        }
    }
}

/// `PathFinding.Rail` / `PathFinding.Air` steppers walking to a target on the world map.
#[test]
fn rail_and_air_steppers() {
    let mut st = world_state();
    st.ticks = 777;
    let mut sc = Scratch::new();
    for (i, c) in cases("steppers").iter().enumerate() {
        let to = c["to"].as_u64().unwrap() as u32;
        let steps = c["steps"].as_array().unwrap();
        let mut cx = Ctx::new(&mut st, &mut sc, None);
        let mut cur = c["from"].as_u64().expect("from") as u32;
        let air = c["kind"] == "air";
        let mut rail = RailStepper::new();
        let mut airs = AirStepper::new(cx.st.ticks);
        for (s, stp) in steps.iter().enumerate() {
            let r = if air { airs.next(&mut cx, cur, to, None) } else { rail.next(&mut cx, cur, to, None) };
            let (code, node) = status_of(&r.map(u64::from));
            assert_eq!((code, node), (stp[0].as_u64().unwrap(), stp[1].as_u64()), "steppers[{i}] step {s}");
            if let PathResult::Next(n) = r {
                cur = n;
            }
        }
    }
}

/// `closestShore` on a synthetic owned map, and `refineStartTile` on the world map.
#[test]
fn spatial_queries() {
    let sp = &doc()["cases"]["spatial"];
    let (w, h) = (sp["map"][0].as_u64().unwrap() as u32, sp["map"][1].as_u64().unwrap() as u32);
    let terrain: Vec<u8> = sp["terrain"].as_array().unwrap().iter().map(|b| b.as_u64().unwrap() as u8).collect();
    let mut m = map_from_terrain(w, h, terrain);
    for (t, o) in sp["owners"].as_array().unwrap().iter().enumerate() {
        let o = o.as_u64().unwrap() as u16;
        if o != 0 {
            m.state_buf_mut().set_owner_id(t as u32, o);
        }
    }
    let mut trav = crate::scratch::traversal::TraversalScratch::default();
    for (i, c) in sp["closest"].as_array().unwrap().iter().enumerate() {
        let got = crate::path::spatial_query::closest_shore_m(
            &mut trav,
            &m,
            crate::ids::SmallId(c["owner"].as_u64().unwrap() as u16),
            c["tile"].as_u64().unwrap() as u32,
            c["maxDist"].as_u64().unwrap() as u32,
        );
        assert_eq!(got, c["res"].as_u64().map(|x| x as u32), "closestShore[{i}]");
    }
    let (main, _) = maps("world");
    let mut b = AStarWaterBoundedScratch::default();
    for (i, c) in sp["refine"].as_array().unwrap().iter().enumerate() {
        let got = crate::path::spatial_query::refine_start_tile(&mut b, main, &u32s(&c["path"]), &u32s(&c["shores"]));
        assert_eq!(u64::from(got), c["res"].as_u64().unwrap(), "refineStartTile[{i}]");
    }
}

/// Provenance: the vectors were made from the pinned engine.
#[test]
fn vectors_header() {
    let d = doc();
    let c = d["engine_commit"].as_str().unwrap();
    assert!(c.starts_with("5b75bd3"), "engine commit {c}");
}
