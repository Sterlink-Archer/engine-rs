//! P4a water-layer tests (FILE_MAP §4 ladder T0/T2 + the mandatory design tests). Owner: **P4a**.
//!
//! - `cc_*`: port of `tests/core/pathfinding/WaterComponents.test.ts` (all 17 cases).
//! - `nukes_*`: port of `tests/core/pathfinding/PathFinding.WaterNukes.test.ts` at the
//!   `WaterManager` level (the `setup()` game is reduced to the two maps + the manager + the
//!   end-of-tick `waterManager.tick(ticks)`, `GameImpl.ts:657`). Boat-path assertions run on
//!   the HPA (P4b's chain is not ported in this branch); see each test.
//! - `g12_*`: the fork/segment-cache design tests (FILE_MAP §4 P4a row, ARCHITECTURE §3.8).
//! - `oracle_*`: golden data produced by the real TS engine (`testdata/oracle`).

use super::abstract_graph::{CLUSTER_SIZE, WaterGraph};
use super::components::{Components, LAND_MARKER};
use super::hpa::{Hpa, LocalKind};
use super::manager::{WaterManager, build_version0_with};
use super::map_view::{WaterMap, WaterMapMut};
use super::test_support::{ShimBounded, ShimMap};
use crate::ids::TileRef;
use crate::path::astar_water_bounded::SearchBounds;
use crate::scratch::path::AStarWaterBoundedScratch;
use crate::scratch::water::{HpaScratch, WaterMgrScratch};
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::PathBuf;

// ---------------------------------------------------------------------------------------
// fixtures

fn island_map() -> ShimMap {
    // `_fixtures.ts:427-437` createIslandMap
    ShimMap::from_grid(5, 5, "WWWWW WLLLW WLLLW WLLLW WWWWW")
}

fn two_components_map() -> ShimMap {
    // WaterComponents.test.ts:9-17
    ShimMap::from_grid(7, 5, "WWLLLWW WWLLLWW WWLLLWW WWLLLWW WWLLLWW")
}

fn r(m: &ShimMap, x: u32, y: u32) -> TileRef {
    m.ref_(f64::from(x), f64::from(y))
}

fn cc_init(m: &ShimMap) -> Components {
    let mut c = Components::new_with(m);
    let mut q = Vec::new();
    c.initialize_with(m, true, &mut q);
    c
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Raw `genTerrainFromBin` bytes (`TerrainMapLoader.ts:121-137`: the bin IS the terrain).
fn load_bin(path: PathBuf, w: u32, h: u32) -> ShimMap {
    let b = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    ShimMap::new(w, h, b)
}

/// The `setup(name)` game reduced to what the water layer touches.
#[derive(Clone)]
struct TGame {
    map: ShimMap,
    mini: ShimMap,
    wm: WaterManager,
    ticks: i32,
}

/// Thread-local-free scratch for a test run (one per test, like one per thread).
struct Sc {
    wm: WaterMgrScratch,
    hpa: HpaScratch,
    bounded: ShimBounded,
}

impl Sc {
    fn new() -> Self {
        Sc { wm: WaterMgrScratch::default(), hpa: HpaScratch::default(), bounded: ShimBounded::new(CLUSTER_SIZE) }
    }
}

impl TGame {
    fn new(map: ShimMap, mini: ShimMap) -> Self {
        let mut bfs = crate::scratch::water::BfsGridScratch::default();
        let mut q = Vec::new();
        let (g, c) = build_version0_with(&mini, &mut bfs, &mut q);
        TGame { map, mini, wm: WaterManager::new(Some(g), Some(c)), ticks: 0 }
    }
    fn plains() -> Self {
        let d = manifest_dir().join("tests/testdata/maps/plains");
        TGame::new(load_bin(d.join("map.bin"), 100, 100), load_bin(d.join("map4x.bin"), 50, 50))
    }
    fn half_land_half_ocean() -> Self {
        let d = manifest_dir().join("src/water/testdata/maps/half_land_half_ocean");
        TGame::new(load_bin(d.join("map.bin"), 16, 16), load_bin(d.join("map4x.bin"), 8, 8))
    }
    /// `executeNextTick()` reduced to the end-of-tick water flush (`GameImpl.ts:656-663`).
    fn execute_next_tick(&mut self, sc: &mut Sc) -> Vec<TileRef> {
        let t = self.ticks;
        let changed = self.wm.tick_with(&mut sc.wm, &mut self.map, &mut self.mini, || false, t);
        self.ticks += 1;
        changed
    }
    /// `nukeCircle(game, cx, cy, r)` (`PathFinding.WaterNukes.test.ts:25-46`).
    fn nuke_circle(&mut self, cx: i64, cy: i64, rad: i64) {
        let r2 = rad * rad;
        let (w, h) = (i64::from(self.map.w), i64::from(self.map.h));
        let (x0, y0, x1, y1) = ((cx - rad).max(0), (cy - rad).max(0), (w - 1).min(cx + rad), (h - 1).min(cy + rad));
        for y in y0..=y1 {
            for x in x0..=x1 {
                let (dx, dy) = (x - cx, y - cy);
                if dx * dx + dy * dy > r2 {
                    continue;
                }
                let t = self.map.ref_(x as f64, y as f64);
                if self.map.is_land(t) && !self.map.has_owner(t) && !self.map.is_impassable(t) {
                    self.wm.queue_tile(t);
                }
            }
        }
    }
    /// `tickUntilRebuild(game, maxTicks = 40)` (`:49-58`).
    fn tick_until_rebuild(&mut self, sc: &mut Sc) {
        let v = self.wm.water_graph_version();
        for _ in 0..40 {
            self.execute_next_tick(sc);
            if self.wm.water_graph_version() > v {
                return;
            }
        }
        panic!("water graph did not rebuild within 40 ticks (version {v})");
    }
    fn gref(&self, x: u32, y: u32) -> TileRef {
        r(&self.map, x, y)
    }
    fn get_water_component(&self, t: TileRef) -> Option<u32> {
        self.wm.water_component_with(&self.map, &self.mini, t)
    }
    fn has_water_component(&self, t: TileRef, c: u32) -> bool {
        self.wm.has_water_component_with(&self.map, &self.mini, t, c)
    }
    fn get_water_component_size(&self, t: TileRef) -> Option<u32> {
        self.wm.water_component_size_with(&self.map, &self.mini, t)
    }
    fn graph(&self) -> &WaterGraph {
        self.wm.graph.as_deref().expect("graph")
    }
    /// One HPA query on this game's current graph (the HPA's `map` is the minimap).
    fn hpa_find_path(&mut self, sc: &mut Sc, from: &[TileRef], to: TileRef) -> Option<Vec<TileRef>> {
        let mini = &self.mini;
        let graph = self.wm.graph.as_deref().expect("graph");
        let bounded = &mut sc.bounded;
        let mut local =
            |k: LocalKind, _s: &mut AStarWaterBoundedScratch, st: &[TileRef], goal: TileRef, b: SearchBounds| {
                bounded.search(k, mini, st, goal, b)
            };
        let mut h = Hpa {
            map: mini,
            graph,
            mini_terrain_gen: mini.terrain_gen,
            seg_vis: &mut self.wm.seg_vis,
            sc: &mut sc.hpa,
            local: &mut local,
        };
        h.find_path(from, to)
    }
}

/// `edgeCostMap(graph)` (`PathFinding.WaterNukes.test.ts:138-153`).
fn edge_cost_map(g: &WaterGraph) -> BTreeMap<(TileRef, TileRef), f64> {
    let mut m = BTreeMap::new();
    for e in &g.edges {
        let a = g.nodes[e.node_a as usize].tile;
        let b = g.nodes[e.node_b as usize].tile;
        let k = if a < b { (a, b) } else { (b, a) };
        let ex = m.get(&k).copied();
        if ex.is_none_or(|c| e.cost < c) {
            m.insert(k, e.cost);
        }
    }
    m
}

fn node_tiles(g: &WaterGraph) -> BTreeSet<TileRef> {
    g.nodes.iter().map(|n| n.tile).collect()
}

/// `scratchMagnitudes(map, passableCoastOnly)` (`:65-120`).
#[allow(clippy::needless_range_loop)] // literal port of the TS oracle's index loops
fn scratch_magnitudes(m: &ShimMap, passable_coast_only: bool) -> Vec<i32> {
    let (w, h) = (m.w as i64, m.h as i64);
    let n = (w * h) as usize;
    let mut dist = vec![-1i32; n];
    let mut queue = Vec::new();
    let is_coast = |t: usize| m.is_land(t as u32) && (!passable_coast_only || !m.is_impassable(t as u32));
    for t in 0..n {
        if m.is_land(t as u32) {
            continue;
        }
        let x = t as i64 % w;
        let y = (t as i64 - x) / w;
        let wu = w as usize;
        let coast = (y > 0 && is_coast(t - wu))
            || (y < h - 1 && is_coast(t + wu))
            || (x > 0 && is_coast(t - 1))
            || (x < w - 1 && is_coast(t + 1));
        if coast {
            dist[t] = 0;
            queue.push(t);
        }
    }
    let mut head = 0;
    while head < queue.len() {
        let t = queue[head];
        head += 1;
        let d = dist[t] + 1;
        let x = t as i64 % w;
        let y = (t as i64 - x) / w;
        let wu = w as usize;
        let mut nb = Vec::new();
        if y > 0 {
            nb.push(t - wu);
        }
        if y < h - 1 {
            nb.push(t + wu);
        }
        if x > 0 {
            nb.push(t - 1);
        }
        if x < w - 1 {
            nb.push(t + 1);
        }
        for n in nb {
            if !m.is_land(n as u32) && dist[n] < 0 {
                dist[n] = d;
                queue.push(n);
            }
        }
    }
    (0..n)
        .map(|t| {
            if m.is_land(t as u32) {
                -1
            } else if dist[t] < 0 {
                31
            } else {
                ((dist[t] + 1) / 2).min(31)
            }
        })
        .collect()
}

/// `expectMagnitudesMatchOracle(map, passableCoastOnly)` (`:122-136`).
fn expect_magnitudes_match_oracle(m: &ShimMap, passable_coast_only: bool) {
    let oracle = scratch_magnitudes(m, passable_coast_only);
    let mut mismatches = 0;
    let mut first = String::new();
    for t in 0..(m.w * m.h) {
        if m.is_land(t) {
            continue;
        }
        if i32::from(m.magnitude(t)) != oracle[t as usize] {
            if mismatches == 0 {
                first =
                    format!("tile ({},{}): actual={} expected={}", m.x(t), m.y(t), m.magnitude(t), oracle[t as usize]);
            }
            mismatches += 1;
        }
    }
    assert_eq!(mismatches, 0, "magnitude mismatches, first: {first}");
}

/// Same partition (ids may differ) and same sizes (`WaterComponents.test.ts:346-363`).
fn assert_same_partition(m: &ShimMap, inc: &Components, fresh: &Components) -> usize {
    let mut f2i: BTreeMap<u32, u32> = BTreeMap::new();
    let mut i2f: BTreeMap<u32, u32> = BTreeMap::new();
    for t in 0..(m.w * m.h) {
        if !m.is_water(t) {
            assert_eq!(inc.get_component_id(t), inc.land_marker);
            continue;
        }
        let f = fresh.get_component_id(t);
        let i = inc.get_component_id(t);
        if !f2i.contains_key(&f) && !i2f.contains_key(&i) {
            f2i.insert(f, i);
            i2f.insert(i, f);
        }
        assert_eq!(f2i.get(&f), Some(&i), "tile {t}");
        assert_eq!(i2f.get(&i), Some(&f), "tile {t}");
        assert_eq!(inc.get_component_size(i), fresh.get_component_size(f), "tile {t}");
    }
    f2i.len()
}

// ---------------------------------------------------------------------------------------
// WaterComponents.test.ts

#[test]
fn cc_get_component_id_returns_0_before_initialization() {
    let m = island_map();
    let wc = Components::new_with(&m);
    assert_eq!(wc.get_component_id(r(&m, 0, 0)), 0);
}

#[test]
fn cc_same_id_for_single_connected_area() {
    let m = island_map();
    let wc = cc_init(&m);
    let ts = [r(&m, 0, 0), r(&m, 4, 0), r(&m, 0, 4), r(&m, 4, 4)];
    for &t in &ts {
        assert!(m.is_water(t));
    }
    assert_eq!(wc.get_component_id(ts[0]), 1);
    for &t in &ts[1..] {
        assert_eq!(wc.get_component_id(t), 1);
    }
}

#[test]
fn cc_different_ids_for_disconnected_areas() {
    let m = two_components_map();
    let wc = cc_init(&m);
    let (l1, l2, r1, r2) = (r(&m, 0, 0), r(&m, 1, 2), r(&m, 5, 0), r(&m, 6, 4));
    for t in [l1, l2, r1, r2] {
        assert!(m.is_water(t));
    }
    let (a, b, c, d) =
        (wc.get_component_id(l1), wc.get_component_id(l2), wc.get_component_id(r1), wc.get_component_id(r2));
    assert_ne!(a, c);
    assert_eq!(a, b);
    assert!(a > 0 && a != LAND_MARKER);
    assert_eq!(c, d);
    assert!(c > 0 && c != LAND_MARKER);
}

#[test]
fn cc_land_marker_for_land() {
    let m = two_components_map();
    let wc = cc_init(&m);
    for t in [r(&m, 2, 0), r(&m, 3, 2), r(&m, 4, 4)] {
        assert!(m.is_land(t));
        assert_eq!(wc.get_component_id(t), LAND_MARKER);
    }
}

#[test]
fn cc_deterministic_on_repeated_initialization() {
    let m = two_components_map();
    let (a, b) = (cc_init(&m), cc_init(&m));
    for t in 0..35 {
        assert_eq!(a.get_component_id(t), b.get_component_id(t));
    }
}

#[test]
fn cc_direct_terrain_access_matches_indirect() {
    // Also on a size that is not a multiple of 4 (the Uint32 chunk tail, `:270-272`).
    for m in [two_components_map(), island_map()] {
        let mut d = Components::new_with(&m);
        let mut i = Components::new_with(&m);
        let mut q = Vec::new();
        d.initialize_with(&m, true, &mut q);
        i.initialize_with(&m, false, &mut q);
        for t in 0..(m.w * m.h) {
            assert_eq!(d.get_component_id(t), i.get_component_id(t));
        }
        assert_eq!(d.snapshot(), i.snapshot());
    }
}

#[test]
fn cc_size_0_before_initialization() {
    let m = island_map();
    assert_eq!(Components::new_with(&m).get_component_size(1), 0);
}

#[test]
fn cc_size_of_single_ring() {
    let m = island_map();
    let wc = cc_init(&m);
    let id = wc.get_component_id(r(&m, 0, 0));
    assert!(id > 0);
    assert_eq!(wc.get_component_size(id), 16);
}

#[test]
fn cc_sizes_of_two_areas() {
    let m = two_components_map();
    let wc = cc_init(&m);
    let (l, rr) = (wc.get_component_id(r(&m, 0, 0)), wc.get_component_id(r(&m, 5, 0)));
    assert_ne!(l, rr);
    assert_eq!(wc.get_component_size(l), 10);
    assert_eq!(wc.get_component_size(rr), 10);
}

#[test]
fn cc_size_0_for_land_id() {
    let m = two_components_map();
    assert_eq!(cc_init(&m).get_component_size(0xff), 0);
}

#[test]
fn cc_size_0_for_unknown_id() {
    let m = island_map();
    assert_eq!(cc_init(&m).get_component_size(999), 0);
}

#[test]
fn cc_never_assigns_0xffff() {
    let m = island_map();
    let wc = cc_init(&m);
    assert_eq!(wc.get_component_size(0xffff), 0);
    for t in 0..25 {
        if m.is_water(t) {
            assert_ne!(wc.get_component_id(t), 0xffff);
        }
    }
}

#[test]
fn cc_add_water_tiles_noop_before_initialization() {
    let mut m = two_components_map();
    let mut wc = Components::new_with(&m);
    let t = r(&m, 2, 0);
    m.set_water(t);
    wc.add_water_tiles_impl(&[t]);
    assert_eq!(wc.get_component_id(t), 0);
}

#[test]
fn cc_add_joins_adjacent_component() {
    let mut m = two_components_map();
    let mut wc = cc_init(&m);
    let left = wc.get_component_id(r(&m, 0, 0));
    assert_eq!(wc.get_component_size(left), 10);
    let t = r(&m, 2, 0);
    m.set_water(t);
    wc.add_water_tiles_impl(&[t]);
    assert_eq!(wc.get_component_id(t), left);
    assert_eq!(wc.get_component_size(left), 11);
}

#[test]
fn cc_add_isolated_creates_component() {
    let mut m = two_components_map();
    let mut wc = cc_init(&m);
    let (left, right) = (wc.get_component_id(r(&m, 0, 0)), wc.get_component_id(r(&m, 5, 0)));
    let t = r(&m, 3, 0);
    m.set_water(t);
    wc.add_water_tiles_impl(&[t]);
    let id = wc.get_component_id(t);
    assert!(id > 0 && id != LAND_MARKER && id != left && id != right);
    assert_eq!(wc.get_component_size(id), 1);
}

#[test]
fn cc_add_bridge_merges() {
    let mut m = two_components_map();
    let mut wc = cc_init(&m);
    let (lb, rb) = (wc.get_component_id(r(&m, 0, 0)), wc.get_component_id(r(&m, 5, 0)));
    assert_ne!(lb, rb);
    let bridge = [r(&m, 2, 2), r(&m, 3, 2), r(&m, 4, 2)];
    for &t in &bridge {
        m.set_water(t);
    }
    wc.add_water_tiles_impl(&bridge);
    let merged = wc.get_component_id(r(&m, 0, 0));
    assert_eq!(wc.get_component_id(r(&m, 6, 4)), merged);
    for &t in &bridge {
        assert_eq!(wc.get_component_id(t), merged);
    }
    assert_eq!(wc.get_component_size(merged), 23);
    assert_eq!(wc.get_component_size(lb), 23);
    assert_eq!(wc.get_component_size(rb), 23);
}

#[test]
fn cc_add_ignores_labeled_water() {
    let mut m = two_components_map();
    let mut wc = cc_init(&m);
    let left = wc.get_component_id(r(&m, 0, 0));
    let t = r(&m, 2, 0);
    m.set_water(t);
    wc.add_water_tiles_impl(&[t]);
    wc.add_water_tiles_impl(&[t]);
    assert_eq!(wc.get_component_size(left), 11);
}

#[test]
fn cc_snapshot_independent_of_lookups() {
    let mut m = ShimMap::from_grid(5, 3, "WLWLW WLWLW WLWLW");
    let mut wc = cc_init(&m);
    for t in [r(&m, 3, 1), r(&m, 1, 1)] {
        m.set_water(t);
        wc.add_water_tiles_impl(&[t]);
    }
    let snap = wc.snapshot().expect("snapshot");
    let mut restored = Components::new_with(&m);
    restored.restore_snapshot(&snap);
    assert_eq!(restored.get_component_id(r(&m, 4, 0)), 1);
    assert_eq!(restored.snapshot().as_ref(), Some(&snap));
}

#[test]
fn cc_incremental_matches_fresh_flood_fill() {
    let mut m = two_components_map();
    let mut wc = cc_init(&m);
    let added = [r(&m, 2, 1), r(&m, 3, 1), r(&m, 4, 1), r(&m, 3, 3), r(&m, 2, 4)];
    for &t in &added {
        m.set_water(t);
    }
    wc.add_water_tiles_impl(&added);
    let fresh = cc_init(&m);
    assert!(assert_same_partition(&m, &wc, &fresh) > 0);
}

#[test]
fn cc_promotes_to_u16_at_component_253() {
    // 3-wide checkerboard of isolated 1-tile lakes: > 253 components forces the u8 → u16
    // promotion inside `initialize` (`:98-100`) and land markers become 0xffff.
    let (w, h) = (40u32, 30u32);
    let mut t = vec![0x80u8; (w * h) as usize];
    let mut lakes = 0;
    for y in (0..h).step_by(2) {
        for x in (0..w).step_by(2) {
            t[(y * w + x) as usize] = 0x00;
            lakes += 1;
        }
    }
    assert!(lakes > 253);
    let m = ShimMap::new(w, h, t);
    let wc = cc_init(&m);
    assert!(wc.component_ids.as_ref().expect("ids").is_u16());
    assert_eq!(wc.land_marker, 0xffff);
    assert_eq!(wc.max_id, lakes);
    assert_eq!(wc.get_component_id(1), 0xffff);
    assert_eq!(wc.get_component_id(0), 1);
    assert_eq!(wc.get_component_id(2), 2);
    // Incremental promotion: 252 lakes, then two isolated additions cross id 253.
    let (w, h) = (36u32, 28u32);
    let mut t = vec![0x80u8; (w * h) as usize];
    let mut lakes = 0;
    'outer: for y in (0..h).step_by(2) {
        for x in (0..w).step_by(2) {
            if lakes == 252 {
                break 'outer;
            }
            t[(y * w + x) as usize] = 0x00;
            lakes += 1;
        }
    }
    let mut m = ShimMap::new(w, h, t);
    let mut wc = cc_init(&m);
    assert!(!wc.component_ids.as_ref().expect("ids").is_u16());
    let last = w * h - 1; // bottom-right corner (odd coords: land, isolated)
    m.set_water(last);
    wc.add_water_tiles_impl(&[last]);
    assert_eq!(wc.get_component_id(last), 253);
    assert!(wc.component_ids.as_ref().expect("ids").is_u16());
    assert_eq!(wc.land_marker, 0xffff);
    assert_eq!(wc.get_component_id(1), 0xffff);
}

// ---------------------------------------------------------------------------------------
// PathFinding.WaterNukes.test.ts (WaterManager level)

#[test]
fn nukes_isolated_crater_forms_one_component() {
    let (mut g, mut sc) = (TGame::plains(), Sc::new());
    g.nuke_circle(30, 30, 8);
    g.tick_until_rebuild(&mut sc);
    let center = g.gref(30, 30);
    assert!(g.map.is_water(center));
    let comp = g.get_water_component(center).expect("component");
    for (x, y) in [(30, 24), (30, 36), (24, 30), (36, 30)] {
        let t = g.gref(x, y);
        assert!(g.map.is_water(t));
        assert_eq!(g.get_water_component(t), Some(comp));
        assert!(g.has_water_component(t, comp));
    }
    let size = g.get_water_component_size(center).expect("size");
    assert!(size > 100 && size < 400, "{size}");
}

#[test]
fn nukes_separate_craters_distinct_components() {
    let (mut g, mut sc) = (TGame::plains(), Sc::new());
    g.nuke_circle(25, 30, 7);
    g.nuke_circle(70, 30, 7);
    g.tick_until_rebuild(&mut sc);
    let (a, b) = (g.gref(25, 30), g.gref(70, 30));
    let (ca, cb) = (g.get_water_component(a).expect("a"), g.get_water_component(b).expect("b"));
    assert_ne!(ca, cb);
    assert!(!g.has_water_component(a, cb));
    assert!(!g.has_water_component(b, ca));
    // HPA level (TS: `PathFinding.Water(game).findPath` is null): different components.
    let (ma, mb) = (r(&g.mini, 12, 15), r(&g.mini, 35, 15));
    assert!(g.hpa_find_path(&mut sc, &[ma], mb).is_none());
}

#[test]
fn nukes_channel_merges_lakes() {
    let (mut g, mut sc) = (TGame::plains(), Sc::new());
    g.nuke_circle(25, 30, 7);
    g.nuke_circle(70, 30, 7);
    g.tick_until_rebuild(&mut sc);
    let (a, b) = (g.gref(25, 30), g.gref(70, 30));
    assert_ne!(g.get_water_component(a), g.get_water_component(b));
    for x in (30..=65).step_by(5) {
        g.nuke_circle(x, 30, 5);
    }
    g.tick_until_rebuild(&mut sc);
    let merged = g.get_water_component(a).expect("merged");
    assert_eq!(g.get_water_component(b), Some(merged));
    assert!(g.has_water_component(b, merged));
    let mid = g.gref(47, 30);
    assert!(g.map.is_water(mid));
    assert_eq!(g.get_water_component(mid), Some(merged));
    assert!(g.get_water_component_size(a).expect("size") > 300);
    // HPA level: a path across now exists, endpoints exact (`findLocalPath` fix-up).
    let (ma, mb) = (r(&g.mini, 12, 15), r(&g.mini, 35, 15));
    let p = g.hpa_find_path(&mut sc, &[ma], mb).expect("path across the channel");
    assert_eq!((p[0], p[p.len() - 1]), (ma, mb));
    for w in p.windows(2) {
        assert_eq!(g.mini.manhattan_dist(w[0], w[1]), 1, "4-connected");
    }
}

#[test]
fn nukes_components_consistent_before_rebuild() {
    let (mut g, mut sc) = (TGame::plains(), Sc::new());
    g.nuke_circle(30, 30, 8);
    g.execute_next_tick(&mut sc); // finalize runs, rebuild does not (same tick)
    assert_eq!(g.wm.water_graph_version(), 0);
    assert!(g.wm.water_graph_dirty);
    let center = g.gref(30, 30);
    assert!(g.map.is_water(center));
    let comp = g.get_water_component(center).expect("component");
    assert!(g.has_water_component(g.gref(34, 30), comp));
}

#[test]
fn nukes_crater_next_to_ocean_joins_ocean() {
    let (mut g, mut sc) = (TGame::half_land_half_ocean(), Sc::new());
    let mut found = None;
    'outer: for y in 1..g.map.h - 1 {
        for x in 1..g.map.w - 1 {
            let t = g.gref(x, y);
            if !g.map.is_land(t) {
                continue;
            }
            for n in [g.gref(x - 1, y), g.gref(x + 1, y), g.gref(x, y - 1), g.gref(x, y + 1)] {
                if g.map.is_ocean(n) {
                    found = Some((t, n));
                    break 'outer;
                }
            }
        }
    }
    let (target, ocean) = found.expect("coastal land tile");
    g.nuke_circle(i64::from(g.map.x(target)), i64::from(g.map.y(target)), 3);
    g.tick_until_rebuild(&mut sc);
    assert!(g.map.is_water(target));
    assert!(g.map.is_ocean(target));
    assert_eq!(g.get_water_component(target), g.get_water_component(ocean));
}

#[test]
fn nukes_magnitudes_single_crater() {
    let (mut g, mut sc) = (TGame::plains(), Sc::new());
    g.nuke_circle(40, 40, 10);
    g.tick_until_rebuild(&mut sc);
    expect_magnitudes_match_oracle(&g.map, true);
    expect_magnitudes_match_oracle(&g.mini, false);
}

#[test]
fn nukes_magnitudes_barrage() {
    let (mut g, mut sc) = (TGame::plains(), Sc::new());
    g.nuke_circle(15, 15, 6);
    g.nuke_circle(80, 80, 6);
    g.nuke_circle(80, 15, 6);
    g.tick_until_rebuild(&mut sc);
    expect_magnitudes_match_oracle(&g.map, true);
    expect_magnitudes_match_oracle(&g.mini, false);
}

#[test]
fn nukes_magnitudes_overlapping_repeats() {
    let (mut g, mut sc) = (TGame::plains(), Sc::new());
    for (x, y) in [(40, 40), (46, 40), (52, 44)] {
        g.nuke_circle(x, y, 8);
        g.tick_until_rebuild(&mut sc);
    }
    expect_magnitudes_match_oracle(&g.map, true);
    expect_magnitudes_match_oracle(&g.mini, false);
}

#[test]
fn nukes_magnitudes_far_coastline() {
    let (mut g, mut sc) = (TGame::plains(), Sc::new());
    for cy in (25..=85).step_by(10) {
        for cx in (15..=75).step_by(10) {
            g.nuke_circle(cx, cy, 8);
        }
    }
    g.tick_until_rebuild(&mut sc);
    g.nuke_circle(95, 55, 4);
    g.tick_until_rebuild(&mut sc);
    expect_magnitudes_match_oracle(&g.map, true);
    expect_magnitudes_match_oracle(&g.mini, false);
    let probe = g.gref(35, 55);
    assert!(g.map.is_water(probe));
    assert!(g.map.magnitude(probe) < 16);
}

#[test]
fn nukes_interior_crater_not_shoreline() {
    let (mut g, mut sc) = (TGame::plains(), Sc::new());
    g.nuke_circle(40, 40, 10);
    g.tick_until_rebuild(&mut sc);
    assert!(g.map.magnitude(g.gref(40, 40)) > 0);
    let mc = r(&g.mini, 20, 20);
    assert!(g.mini.is_water(mc));
    assert!(g.mini.magnitude(mc) > 0);
}

fn rebuild_sequence(g: &mut TGame, sc: &mut Sc) {
    // `graph rebuild consistency (plains)` beforeEach (`:394-406`)
    g.nuke_circle(25, 30, 7);
    g.nuke_circle(70, 30, 7);
    g.tick_until_rebuild(sc);
    for x in (30..=65).step_by(5) {
        g.nuke_circle(x, 30, 5);
    }
    g.tick_until_rebuild(sc);
    g.nuke_circle(50, 70, 9);
    g.tick_until_rebuild(sc);
}

#[test]
fn nukes_incremental_rebuild_equals_full_rebuild() {
    let (mut g, mut sc) = (TGame::plains(), Sc::new());
    rebuild_sequence(&mut g, &mut sc);
    let mut bfs = crate::scratch::water::BfsGridScratch::default();
    let full = WaterGraph::build_with(&g.mini, CLUSTER_SIZE, None, None, None, &mut bfs);
    assert_eq!(node_tiles(g.graph()), node_tiles(&full));
    assert_eq!(edge_cost_map(g.graph()), edge_cost_map(&full));
    assert!(!full.nodes.is_empty(), "non-vacuity: the craters created gateway nodes");
}

#[test]
fn nukes_incremental_components_match_fresh() {
    let (mut g, mut sc) = (TGame::plains(), Sc::new());
    rebuild_sequence(&mut g, &mut sc);
    let fresh = cc_init(&g.mini);
    let inc = g.wm.components.as_deref().expect("components");
    assert!(assert_same_partition(&g.mini, inc, &fresh) > 0);
}

#[test]
fn nukes_deterministic_across_two_games() {
    let (mut g1, mut sc) = (TGame::plains(), Sc::new());
    let mut g2 = TGame::plains();
    rebuild_sequence(&mut g1, &mut sc);
    let mut sc2 = Sc::new();
    rebuild_sequence(&mut g2, &mut sc2);
    assert_eq!(g1.map.terrain, g2.map.terrain);
    assert_eq!(g1.mini.terrain, g2.mini.terrain);
    assert_eq!(edge_cost_map(g1.graph()), edge_cost_map(g2.graph()));
    // And the same with one scratch shared by both games (per-thread scratch, G14).
    let mut g3 = TGame::plains();
    rebuild_sequence(&mut g3, &mut sc);
    assert_eq!(g1.map.terrain, g3.map.terrain);
    assert_eq!(g1.mini.terrain, g3.mini.terrain);
}

#[test]
fn nukes_cluster_boundary_rebuild() {
    let (mut g, mut sc) = (TGame::plains(), Sc::new());
    for y in [40, 50, 60, 66] {
        g.nuke_circle(71, y, 7);
    }
    g.tick_until_rebuild(&mut sc);
    g.nuke_circle(56, 40, 7);
    g.tick_until_rebuild(&mut sc);
    assert!(g.mini.is_water(r(&g.mini, 31, 20)));
    assert!(g.mini.is_water(r(&g.mini, 32, 20)));
    let mut bfs = crate::scratch::water::BfsGridScratch::default();
    let full = WaterGraph::build_with(&g.mini, CLUSTER_SIZE, None, None, None, &mut bfs);
    assert_eq!(node_tiles(g.graph()), node_tiles(&full));
    assert_eq!(edge_cost_map(g.graph()), edge_cost_map(&full));
    let (a, b) = (r(&g.mini, 28, 20), r(&g.mini, 35, 20));
    assert!(g.hpa_find_path(&mut sc, &[a], b).is_some());
}

#[test]
fn nukes_rebuild_is_throttled_and_skips_conversion_ticks() {
    // `WaterManager.ts:175-184`: no rebuild in a tick that converted, and at most one per 20.
    let (mut g, mut sc) = (TGame::plains(), Sc::new());
    g.ticks = 5;
    g.nuke_circle(30, 30, 4);
    let changed = g.execute_next_tick(&mut sc); // tick 5: converts
    assert!(!changed.is_empty());
    assert_eq!(g.wm.water_graph_version(), 0);
    for _ in 6..20 {
        g.execute_next_tick(&mut sc);
        assert_eq!(g.wm.water_graph_version(), 0, "tick {}", g.ticks);
    }
    g.execute_next_tick(&mut sc); // tick 20: 20 - 0 >= 20
    assert_eq!((g.wm.water_graph_version(), g.wm.water_graph_last_rebuild_tick), (1, 20));
    assert!(!g.wm.water_graph_dirty);
    assert!(g.wm.dirty_mini_tiles.is_empty());
    // A conversion queued on tile already water/owned is skipped (`:155-158`).
    let owned = g.gref(80, 80);
    g.map.set_owner(owned, 3);
    g.wm.queue_tile(owned);
    g.wm.queue_tile(g.gref(30, 30)); // already water
    let changed = g.execute_next_tick(&mut sc);
    assert!(changed.is_empty());
    assert!(g.map.is_land(owned));
    assert!(g.wm.pending_water_tiles.is_empty());
}

#[test]
fn nukes_fallout_cleared_on_conversion() {
    let (mut g, mut sc) = (TGame::plains(), Sc::new());
    let t = g.gref(10, 10);
    g.map.set_fallout_on(t);
    assert_eq!(g.map.num_tiles_with_fallout, 1);
    g.wm.queue_tile(t);
    g.execute_next_tick(&mut sc);
    assert!(g.map.is_water(t));
    assert_eq!(g.map.num_tiles_with_fallout, 0);
}

// ---------------------------------------------------------------------------------------
// Design tests: forks and the segment cache (G12, ARCHITECTURE §3.8)

/// A 256 × 256 main map / 128 × 128 minimap (4 × 4 clusters) of open water with 30 land
/// blocks, so the graph has many nodes and edges, and walls a nuke can cut through.
fn lake_world() -> TGame {
    let (mw, mh) = (128u32, 128u32);
    let mut mini = vec![0x20u8; (mw * mh) as usize];
    let mut s: u64 = 12345;
    let mut next = |m: u32| {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((s >> 33) as u32) % m
    };
    for _ in 0..30 {
        let (x0, y0) = (next(mw - 4), next(mh - 4));
        let (bw, bh) = (3 + next(18), 3 + next(18));
        for y in y0..(y0 + bh).min(mh) {
            for x in x0..(x0 + bw).min(mw) {
                mini[(y * mw + x) as usize] = 0x80 | 5;
            }
        }
    }
    super::test_support::compute_shoreline(&mut mini, mw, mh);
    // main = mini scaled 2× (bits only), shoreline recomputed
    let (w, h) = (mw * 2, mh * 2);
    let mut main = vec![0u8; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            main[(y * w + x) as usize] = mini[((y / 2) * mw + x / 2) as usize] & !0x40;
        }
    }
    super::test_support::compute_shoreline(&mut main, w, h);
    // water magnitude: 1 next to the coast, 6 elsewhere (so bounded A* costs vary)
    for b in mini.iter_mut().chain(main.iter_mut()) {
        if *b & 0x80 == 0 {
            *b = (*b & !0x1f) | if *b & 0x40 != 0 { 1 } else { 6 };
        }
    }
    TGame::new(ShimMap::new(w, h, main), ShimMap::new(mw, mh, mini))
}

/// Coastal land tiles of the main map (deterministic pick), as nuke centres.
fn coastal_land(g: &TGame, n: usize) -> Vec<(i64, i64)> {
    let mut out = Vec::new();
    let mut nb = [0u32; 4];
    let total = g.map.w * g.map.h;
    let mut t = 7919u32;
    while out.len() < n {
        t = (t + 104_729) % total;
        if !g.map.is_land(t) {
            continue;
        }
        let k = g.map.neighbors4(t, &mut nb);
        if nb[..k].iter().any(|&x| g.map.is_water(x)) {
            out.push((i64::from(g.map.x(t)), i64::from(g.map.y(t))));
        }
    }
    out
}

/// Deterministic query list over water tiles of the minimap (LCG).
fn queries(g: &TGame, n: usize, seed: u64) -> Vec<(Vec<TileRef>, TileRef)> {
    let water: Vec<TileRef> = (0..g.mini.w * g.mini.h).filter(|&t| g.mini.is_water(t)).collect();
    let mut s = seed;
    let mut next = || {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (s >> 33) as usize
    };
    (0..n)
        .map(|i| {
            let k = if i % 4 == 3 { 2 + next() % 3 } else { 1 };
            let from: Vec<TileRef> = (0..k).map(|_| water[next() % water.len()]).collect();
            (from, water[next() % water.len()])
        })
        .collect()
}

/// An independent copy of `g` whose graph is rebuilt from `pristine_mini` (same content,
/// **unshared** cells): the
/// TS semantics, where every game owns its `_pathCache`.
fn unshared_copy(g: &TGame, pristine_mini: &ShimMap) -> TGame {
    let mut c = g.clone();
    let old = c.wm.graph.as_deref().expect("graph");
    let mut bfs = crate::scratch::water::BfsGridScratch::default();
    // The (stale) graph was built on the pristine minimap with its own fresh components.
    let rebuilt = WaterGraph::build_with(pristine_mini, CLUSTER_SIZE, None, None, None, &mut bfs);
    assert_eq!(edge_cost_map(old), edge_cost_map(&rebuilt));
    let mut rebuilt = rebuilt;
    rebuilt.built_at_mini_gen = old.built_at_mini_gen;
    // Keep this game's own fills: copy its visible segments into the overlay.
    let vis = &c.wm.seg_vis;
    let mut overlay_fill = Vec::new();
    for idx in 0..old.seg_slots() {
        if vis.filled.get(idx / 64).is_some_and(|w| w & (1 << (idx % 64)) != 0) {
            let e = &old.edges[idx / 2];
            let from = if idx % 2 == 0 { e.node_a } else { e.node_b };
            let p = vis.get_cached_path(old, e.id, from).expect("visible").to_vec();
            overlay_fill.push((e.id, from, p));
        }
    }
    c.wm.seg_vis.reset(&rebuilt);
    for (eid, from, p) in overlay_fill {
        c.wm.seg_vis.set_cached_path(&rebuilt, u32::MAX, eid, from, &p); // gen mismatch: overlay
    }
    c.wm.graph = Some(std::sync::Arc::new(rebuilt));
    c
}

#[test]
fn g12_fork_water_nuke_segments_stay_per_game() {
    let mut sc = Sc::new();
    let mut parent = lake_world();
    assert!(parent.graph().node_count() >= 20, "nodes {}", parent.graph().node_count());
    // Warm the parent's cache a little (both children inherit these fills).
    for (f, t) in queries(&parent, 20, 7) {
        let _ = parent.hpa_find_path(&mut sc, &f, t);
    }
    let mut a = parent.clone();
    let mut b = parent.clone();
    let mut c = parent.clone();
    assert!(std::sync::Arc::ptr_eq(a.wm.graph.as_ref().expect("a"), b.wm.graph.as_ref().expect("b")));
    // Child A: water-nuke through the walls (terrain edits, graph stays stale).
    for (x, y) in coastal_land(&a, 12) {
        a.nuke_circle(x, y, 8);
    }
    a.execute_next_tick(&mut sc);
    assert!(a.wm.water_graph_dirty && a.wm.water_graph_version() == 0, "graph still stale");
    assert!(a.mini.terrain_gen > parent.mini.terrain_gen);
    let mut ra = unshared_copy(&a, &parent.mini); // TS reference for A
    let mut rb = unshared_copy(&b, &parent.mini); // TS reference for B
    let qs = queries(&parent, 300, 99);
    // B first: fills shared cells from the pristine terrain.
    let pb: Vec<_> = qs.iter().map(|(f, t)| b.hpa_find_path(&mut sc, f, *t)).collect();
    let prb: Vec<_> = qs.iter().map(|(f, t)| rb.hpa_find_path(&mut sc, f, *t)).collect();
    assert_eq!(pb, prb, "B equals its unshared reference");
    assert!(b.wm.seg_vis.overlay.is_empty(), "B fills are shared");
    // A next: must NOT see B's shared fills (its bits are unset) and must segment on its own
    // edited terrain, into the overlay.
    let pa: Vec<_> = qs.iter().map(|(f, t)| a.hpa_find_path(&mut sc, f, *t)).collect();
    let pra: Vec<_> = qs.iter().map(|(f, t)| ra.hpa_find_path(&mut sc, f, *t)).collect();
    assert_eq!(pa, pra, "A equals its unshared reference");
    assert!(!a.wm.seg_vis.overlay.is_empty(), "post-edit fills go to the overlay");
    assert_ne!(pa, pb, "the nuke changed some routes (non-vacuity)");
    // Same edge in both children: B's value is the shared one; A's lives in A's overlay.
    let g = a.graph();
    let mut differing = 0;
    for idx in 0..g.seg_slots() {
        let bit = |v: &super::abstract_graph::SegVisibility| {
            v.filled.get(idx / 64).is_some_and(|w| w & (1 << (idx % 64)) != 0)
        };
        if bit(&a.wm.seg_vis) && bit(&b.wm.seg_vis) {
            let e = &g.edges[idx / 2];
            let from = if idx % 2 == 0 { e.node_a } else { e.node_b };
            let sa = a.wm.seg_vis.get_cached_path(g, e.id, from).expect("a");
            let sb = b.wm.seg_vis.get_cached_path(g, e.id, from).expect("b");
            let parent_had = bit(&parent.wm.seg_vis);
            if !parent_had {
                assert!(a.wm.seg_vis.overlay.contains_key(&(idx as u32)), "A fill after the edit is in A's overlay");
                assert!(!b.wm.seg_vis.overlay.contains_key(&(idx as u32)));
                assert_eq!(g.seg_shared[idx].get().map(|p| &p[..]), Some(sb), "B's fill is the shared value");
            }
            if sa != sb {
                differing += 1;
            }
        }
    }
    assert!(differing > 0, "some edge segment differs between the forks (non-vacuity)");
    // Child C (never edited, cloned before B's queries): shared cells are filled, but C's
    // bits are not, so C misses and recomputes (TS: C's own empty cache) — same values.
    let pc: Vec<_> = qs.iter().map(|(f, t)| c.hpa_find_path(&mut sc, f, *t)).collect();
    assert_eq!(pc, pb);
    assert!(c.wm.seg_vis.overlay.is_empty());
}

#[test]
fn g12_fill_after_terrain_edit_goes_to_overlay_not_shared() {
    let mut sc = Sc::new();
    let mut g = lake_world();
    let gen0 = g.mini.terrain_gen;
    assert_eq!(g.graph().built_at_mini_gen, gen0);
    let qs = queries(&g, 60, 3);
    for (f, t) in &qs[..30] {
        let _ = g.hpa_find_path(&mut sc, f, *t);
    }
    let shared_before = g.graph().seg_shared.iter().filter(|c| c.get().is_some()).count();
    assert!(shared_before > 0);
    assert!(g.wm.seg_vis.overlay.is_empty());
    // A terrain edit far from everything still changes the gen: later fills are overlay-only.
    let (x, y) = coastal_land(&g, 1)[0];
    g.nuke_circle(x, y, 1);
    g.execute_next_tick(&mut sc);
    assert_ne!(g.mini.terrain_gen, gen0);
    for (f, t) in &qs[30..] {
        let _ = g.hpa_find_path(&mut sc, f, *t);
    }
    let shared_after = g.graph().seg_shared.iter().filter(|c| c.get().is_some()).count();
    assert_eq!(shared_after, shared_before, "no shared write after the edit");
    assert!(!g.wm.seg_vis.overlay.is_empty());
    // A rebuild resets visibility (TS `_initPathCache` on the new graph).
    g.tick_until_rebuild(&mut sc);
    assert!(g.wm.seg_vis.overlay.is_empty());
    assert!(g.wm.seg_vis.filled.iter().all(|&w| w == 0));
    assert_eq!(g.graph().built_at_mini_gen, g.mini.terrain_gen);
}

#[test]
fn g12_components_are_live_and_cow_across_forks() {
    // An old-version chain's `componentCheckFn` is `graph.getComponentId` of a graph whose
    // `_waterComponents` is the one live object: after a nuke and BEFORE the rebuild, the
    // component answer is already the merged one (TS `PathFinder.ts:60`, `AbstractGraph.ts:142`).
    let mut sc = Sc::new();
    let parent = lake_world();
    let mut a = parent.clone();
    let b = parent.clone();
    // A mini land tile whose 5 × 5 neighbourhood is all land: nuking it makes an isolated crater.
    let (mw, mh) = (a.mini.w, a.mini.h);
    let inside = (0..mw * mh)
        .find(|&t| {
            let (x, y) = (t % mw, t / mw);
            x >= 2
                && y >= 2
                && x + 2 < mw
                && y + 2 < mh
                && (y - 2..=y + 2).all(|yy| (x - 2..=x + 2).all(|xx| a.mini.is_land(yy * mw + xx)))
        })
        .expect("an interior land tile");
    let (mx, my) = (i64::from(inside % mw), i64::from(inside / mw));
    a.nuke_circle(mx * 2, my * 2, 1); // main (2mx, 2my) → mini (mx, my)
    a.execute_next_tick(&mut sc);
    assert!(a.mini.is_water(inside));
    let old_graph = a.wm.graph.clone().expect("graph");
    assert_eq!(a.wm.water_graph_version(), 0, "stale graph");
    let comp = a.wm.graph_component_id(inside);
    assert!(
        comp != 0 && comp != a.wm.components.as_deref().expect("cc").land_marker,
        "live components know the crater"
    );
    assert_eq!(b.wm.graph_component_id(inside), LAND_MARKER, "sibling fork keeps its own components (COW)");
    assert!(!std::sync::Arc::ptr_eq(a.wm.components.as_ref().expect("a"), b.wm.components.as_ref().expect("b")));
    // The stale graph has no node for the crater; the component answer comes from the live
    // labelling regardless of which graph is current.
    a.tick_until_rebuild(&mut sc);
    assert!(!std::sync::Arc::ptr_eq(&old_graph, a.wm.graph.as_ref().expect("new")));
    assert_eq!(a.wm.graph_component_id(inside), comp);
}

#[test]
fn hpa_multi_source_short_path_and_source_resolution() {
    let mut sc = Sc::new();
    let mut g = lake_world();
    let water: Vec<TileRef> = (0..g.mini.w * g.mini.h).filter(|&t| g.mini.is_water(t)).collect();
    let a = water[0];
    let b = *water.last().expect("water");
    let near = *water
        .iter()
        .rev()
        .find(|&&t| t != b && g.mini.manhattan_dist(t, b) > 3 && g.mini.manhattan_dist(t, b) <= 40)
        .expect("near");
    assert!(g.mini.manhattan_dist(a, b) > 120);
    // Short path: one candidate within 120 → bounded search over the candidate box.
    let p = g.hpa_find_path(&mut sc, &[a, near], b).expect("path");
    assert_eq!(*p.last().expect("last"), b);
    assert_eq!(p[0], near);
    // Single source equals the 1-element form.
    let p1 = g.hpa_find_path(&mut sc, &[a], b).expect("path");
    assert_eq!(p1[0], a);
    assert_eq!(*p1.last().expect("last"), b);
    for w in p1.windows(2) {
        assert_eq!(g.mini.manhattan_dist(w[0], w[1]), 1);
    }
}

#[test]
fn segment_cache_hit_requires_non_empty_and_bit() {
    let g = lake_world();
    let graph = g.graph();
    let mut vis = super::abstract_graph::SegVisibility::for_graph(graph);
    let e = graph.edges[0];
    assert!(vis.get_cached_path(graph, e.id, e.node_a).is_none());
    vis.set_cached_path(graph, graph.built_at_mini_gen, e.id, e.node_a, &[1, 2, 3]);
    assert_eq!(vis.get_cached_path(graph, e.id, e.node_a), Some(&[1, 2, 3][..]));
    assert!(vis.get_cached_path(graph, e.id, e.node_b).is_none(), "direction-aware");
    let fresh = super::abstract_graph::SegVisibility::for_graph(graph);
    assert!(fresh.get_cached_path(graph, e.id, e.node_a).is_none(), "shared value invisible without the bit");
    assert!(vis.get_cached_path(graph, 999_999, 0).is_none(), "unknown edge");
}

#[test]
fn graph_get_edge_between_and_nearby_clusters_follow_js() {
    let g = lake_world();
    let graph = g.graph();
    let e = graph.edges[3];
    assert_eq!(graph.get_edge_between(e.node_a, e.node_b).map(|x| x.id), Some(e.id));
    assert_eq!(graph.get_edge_between(e.node_b, e.node_a).map(|x| x.id), Some(e.id));
    // a == b: the TS scan returns the node's first incident edge.
    let first = graph.get_node_edges(e.node_a)[0];
    assert_eq!(graph.get_edge_between(e.node_a, e.node_a).map(|x| x.id), Some(first));
    // Edges sorted by (nodeA, nodeB) and renumbered.
    for (i, w) in graph.edges.windows(2).enumerate() {
        assert!((w[0].node_a, w[0].node_b) < (w[1].node_a, w[1].node_b));
        assert_eq!(w[0].id as usize, i);
    }
    // getNearbyClusterNodes at x = 0 wraps into the previous row's last cluster (JS key).
    assert_eq!(graph.clusters_x, 4);
    let wrapped = graph.get_nearby_cluster_nodes(0, 1);
    let mut expect = Vec::new();
    for dy in -1i64..=1 {
        for dx in -1i64..=1 {
            let k = (1 + dy) * i64::from(graph.clusters_x) + dx;
            if k >= 0
                && let Some(Some(c)) = graph.clusters.get(k as usize)
            {
                expect.extend_from_slice(&c.node_ids);
            }
        }
    }
    assert_eq!(wrapped, expect);
}

// ---------------------------------------------------------------------------------------
// Golden oracle from the TS engine (`testdata/oracle/water_oracle.mts`, run with the TS harness)

use serde_json::Value;

/// FNV-1a-32 over u32 words (`fnv` in water_oracle.mts).
fn fnv32(a: impl IntoIterator<Item = u32>) -> String {
    let mut h: u32 = 0x811c_9dc5;
    for v in a {
        h ^= v;
        h = h.wrapping_mul(0x0100_0193);
    }
    format!("{h:08x}")
}

fn testdata() -> PathBuf {
    manifest_dir().join("src/water/testdata")
}

fn oracle() -> Value {
    let p = testdata().join("oracle/water_oracle.json.zst");
    let raw = of_trace::trace::zstd_read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    serde_json::from_slice(&raw).expect("oracle json")
}

/// The map bin named by an oracle key (`<map>/<bin>`), with its manifest dims.
fn oracle_map(key: &str) -> ShimMap {
    let (w, h) = match key {
        "world/map4x.bin" => (1000, 500),
        "world/map16x.bin" => (500, 250),
        "giantworldmap/map16x.bin" => (1027, 487),
        "half_land_half_ocean/map4x.bin" | "ocean_and_land/map4x.bin" => (8, 8),
        _ => panic!("unknown map {key}"),
    };
    let d = testdata().join("maps");
    let zst = d.join(format!("{key}.zst"));
    let bytes = if zst.exists() {
        of_trace::trace::zstd_read(&zst).expect("zstd")
    } else {
        std::fs::read(d.join(key)).expect("bin")
    };
    ShimMap::new(w, h, bytes)
}

fn ju32(v: &Value) -> u32 {
    v.as_u64().unwrap_or_else(|| panic!("u32 {v}")) as u32
}
fn jvec(v: &Value) -> Vec<i64> {
    v.as_array().expect("array").iter().map(|x| x.as_i64().unwrap_or_else(|| panic!("int {x}"))).collect()
}

fn check_cc(c: &Components, want: &Value, ctx: &str) {
    let s = c.snapshot().expect("initialized");
    let ids: Vec<u32> = match &s.component_ids {
        super::components::ComponentIdsSnap::U8(v) => v.iter().map(|&x| u32::from(x)).collect(),
        super::components::ComponentIdsSnap::U16(v) => v.iter().map(|&x| u32::from(x)).collect(),
    };
    assert_eq!(
        matches!(s.component_ids, super::components::ComponentIdsSnap::U16(_)),
        want["u16"].as_bool().expect("u16"),
        "{ctx}: u16"
    );
    assert_eq!(fnv32(ids), want["idsHash"].as_str().expect("h"), "{ctx}: componentIds");
    let sizes: Vec<i64> = s.component_sizes.iter().map(|x| x.map_or(-1, i64::from)).collect();
    assert_eq!(sizes, jvec(&want["sizes"]), "{ctx}: componentSizes");
    let parents: Vec<i64> = s.parents.iter().map(|&x| i64::from(x)).collect();
    assert_eq!(parents, jvec(&want["parents"]), "{ctx}: parents");
    assert_eq!(s.max_id, ju32(&want["maxId"]), "{ctx}: maxId");
    assert_eq!(s.land_marker, ju32(&want["landMarker"]), "{ctx}: landMarker");
}

fn graph_flat(g: &WaterGraph) -> (Vec<u32>, Vec<f64>, Vec<u32>) {
    let nodes = g.nodes.iter().flat_map(|n| [n.x, n.y, n.tile, n.component_id]).collect();
    let edges = g
        .edges
        .iter()
        .flat_map(|e| {
            [
                f64::from(e.id),
                f64::from(e.node_a),
                f64::from(e.node_b),
                e.cost,
                f64::from(e.cluster_x),
                f64::from(e.cluster_y),
            ]
        })
        .collect();
    let mut clusters = Vec::new();
    for cy in 0..g.clusters_y {
        for cx in 0..g.clusters_x {
            let ids = g.get_cluster_nodes(cx, cy);
            clusters.push(ids.len() as u32);
            clusters.extend_from_slice(ids);
        }
    }
    (nodes, edges, clusters)
}

fn check_graph_hash(g: &WaterGraph, want: &Value, ctx: &str) {
    let (n, e, c) = graph_flat(g);
    assert_eq!(g.nodes.len() as u64, want["nodes"].as_u64().expect("n"), "{ctx}: node count");
    assert_eq!(g.edges.len() as u64, want["edges"].as_u64().expect("e"), "{ctx}: edge count");
    assert_eq!(fnv32(n), want["nodesHash"].as_str().expect("h"), "{ctx}: nodes");
    assert_eq!(fnv32(e.iter().map(|&x| x as u32)), want["edgesHash"].as_str().expect("h"), "{ctx}: edges");
    assert_eq!(fnv32(c), want["clustersHash"].as_str().expect("h"), "{ctx}: clusters");
}

fn parse_queries(v: &Value) -> Vec<(Vec<TileRef>, TileRef)> {
    v.as_array()
        .expect("queries")
        .iter()
        .map(|q| (jvec(&q["from"]).into_iter().map(|x| x as TileRef).collect(), ju32(&q["to"])))
        .collect()
}

fn check_results(got: &[Option<Vec<TileRef>>], want: &Value, qs: &[(Vec<TileRef>, TileRef)], ctx: &str) {
    let want = want.as_array().expect("results");
    assert_eq!(got.len(), want.len());
    let mut found = 0;
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        let wl = w["len"].as_i64().expect("len");
        let q = &qs[i];
        match g {
            None => assert_eq!(wl, -1, "{ctx} q{i} {q:?}: Rust found no path"),
            Some(p) => {
                if let Some(wp) = w.get("path") {
                    let wp: Vec<TileRef> = jvec(wp).into_iter().map(|x| x as TileRef).collect();
                    assert_eq!(p, &wp, "{ctx} q{i} {q:?}: path");
                }
                assert_eq!(p.len() as i64, wl, "{ctx} q{i} {q:?}: length");
                assert_eq!(fnv32(p.iter().copied()), w["hash"].as_str().expect("hash"), "{ctx} q{i} {q:?}: path hash");
                found += 1;
            }
        }
    }
    assert!(found > 0, "{ctx}: non-vacuity");
}

fn filled_indices(v: &super::abstract_graph::SegVisibility, g: &WaterGraph) -> Vec<i64> {
    (0..g.seg_slots())
        .filter(|&i| v.filled.get(i / 64).is_some_and(|w| w & (1 << (i % 64)) != 0))
        .map(|i| i as i64)
        .collect()
}

#[test]
fn oracle_mod_cc_matches_ts() {
    let o = oracle();
    let cc = o["cc"].as_object().expect("cc");
    assert_eq!(cc.len(), 5);
    for (key, want) in cc {
        let m = oracle_map(key);
        check_cc(&cc_init(&m), want, key);
    }
}

#[test]
fn oracle_graph_build_matches_ts() {
    let o = oracle();
    let want = &o["graph"]["world/map4x.bin"];
    let m = oracle_map("world/map4x.bin");
    let mut bfs = crate::scratch::water::BfsGridScratch::default();
    let mut q = Vec::new();
    let (g, _) = build_version0_with(&m, &mut bfs, &mut q);
    assert_eq!((g.clusters_x, g.clusters_y), (ju32(&want["clustersX"]), ju32(&want["clustersY"])));
    let (n, e, c) = graph_flat(&g);
    let wn: Vec<u32> = jvec(&want["nodes"]).into_iter().map(|x| x as u32).collect();
    assert_eq!(n, wn, "nodes [x, y, tile, componentId]");
    let we: Vec<f64> = want["edges"].as_array().expect("edges").iter().map(|x| x.as_f64().expect("f")).collect();
    assert_eq!(e, we, "edges [id, a, b, cost, cx, cy]");
    assert_eq!(fnv32(c), want["clustersHash"].as_str().expect("h"));
    assert!(g.nodes.len() > 100, "an HPA-sized graph");
}

#[test]
fn oracle_hpa_queries_and_segment_cache_match_ts() {
    let o = oracle();
    let want = &o["hpa"]["world/map4x.bin"];
    let mini = oracle_map("world/map4x.bin");
    let main = ShimMap::new(1, 1, vec![0x80]); // unused by HPA queries
    let mut g = TGame::new(main, mini);
    let mut sc = Sc::new();
    let qs = parse_queries(&want["queries"]);
    let got: Vec<_> = qs.iter().map(|(f, t)| g.hpa_find_path(&mut sc, f, *t)).collect();
    check_results(&got, &want["results"], &qs, "world hpa");
    assert_eq!(filled_indices(&g.wm.seg_vis, g.graph()), jvec(&want["cacheFilled"]), "segment cache contents");
}

#[test]
fn oracle_water_manager_nuke_sequence_matches_ts() {
    let o = oracle();
    let want = &o["nuke"]["world-compact"];
    let mut g = TGame::new(oracle_map("world/map4x.bin"), oracle_map("world/map16x.bin"));
    let mut sc = Sc::new();
    check_graph_hash(g.graph(), &want["initialGraph"], "initial graph");
    let qs = parse_queries(&want["queries"]);
    let run = |g: &mut TGame, sc: &mut Sc| -> Vec<Option<Vec<TileRef>>> {
        qs.iter().map(|(f, t)| g.hpa_find_path(sc, f, *t)).collect()
    };
    let got = run(&mut g, &mut sc);
    check_results(&got, &want["initial"], &qs, "initial");
    for (pi, ph) in want["phases"].as_array().expect("phases").iter().enumerate() {
        for n in ph["nukes"].as_array().expect("nukes") {
            let v = jvec(n);
            g.nuke_circle(v[0], v[1], v[2]);
        }
        let v0 = g.wm.water_graph_version();
        let changed = g.execute_next_tick(&mut sc);
        let af = &ph["afterFlush"];
        let ctx = format!("phase {pi}");
        assert_eq!(changed.len() as u64, af["changedLen"].as_u64().expect("l"), "{ctx}: changed count");
        let head: Vec<i64> = changed.iter().take(40).map(|&t| i64::from(t)).collect();
        assert_eq!(head, jvec(&af["changedHead"]), "{ctx}: changed order");
        assert_eq!(fnv32(changed.iter().copied()), af["changedHash"].as_str().expect("h"), "{ctx}: changed tiles");
        assert_eq!(
            fnv32(g.map.terrain.iter().map(|&b| u32::from(b))),
            af["mainHash"].as_str().expect("h"),
            "{ctx}: main terrain"
        );
        assert_eq!(
            fnv32(g.mini.terrain.iter().map(|&b| u32::from(b))),
            af["miniHash"].as_str().expect("h"),
            "{ctx}: mini terrain"
        );
        assert_eq!(g.map.water_version, ju32(&af["mainWaterVersion"]), "{ctx}");
        assert_eq!(g.mini.water_version, ju32(&af["miniWaterVersion"]), "{ctx}");
        let dirty: Vec<i64> = g.wm.dirty_mini_tiles.iter().map(|&t| i64::from(t)).collect();
        assert_eq!(dirty, jvec(&af["dirtyMini"]), "{ctx}: dirtyMiniTiles");
        check_cc(g.wm.components.as_deref().expect("cc"), &af["cc"], &format!("{ctx} cc"));
        // Q-WATERLAG: stale graph over new terrain
        let got = run(&mut g, &mut sc);
        check_results(&got, &ph["stale"], &qs, &format!("{ctx} stale"));
        assert_eq!(filled_indices(&g.wm.seg_vis, g.graph()), jvec(&ph["staleCache"]), "{ctx}: stale cache");
        let mut ticks = 0;
        while g.wm.water_graph_version() == v0 && ticks < 40 {
            g.execute_next_tick(&mut sc);
            ticks += 1;
        }
        assert_eq!(ticks, ju32(&ph["rebuildTicks"]), "{ctx}: rebuild ticks");
        assert_eq!(g.wm.water_graph_version(), ju32(&ph["version"]), "{ctx}");
        assert_eq!(i64::from(g.wm.water_graph_last_rebuild_tick), ph["lastRebuildTick"].as_i64().expect("t"), "{ctx}");
        check_graph_hash(g.graph(), &ph["graph"], &format!("{ctx} graph"));
        let got = run(&mut g, &mut sc);
        check_results(&got, &ph["after"], &qs, &format!("{ctx} after"));
        assert_eq!(filled_indices(&g.wm.seg_vis, g.graph()), jvec(&ph["afterCache"]), "{ctx}: cache after rebuild");
    }
}

// ---- rules-v36 (t713): upstream #6295 (394e150db) `tests/core/pathfinding/AbstractGraph.test.ts` ----

/// Two parallel channels (rows 10 and 20) that only meet through a U-bend in the last cluster, so the bend's
/// gateways are 10 tiles apart but 68 tiles by water (`riverWithBend()`).
fn river_with_bend() -> ShimMap {
    let (w, h) = (160u32, 32u32);
    let mut g = vec!['L'; (w * h) as usize];
    for x in 0..=156u32 {
        g[(10 * w + x) as usize] = 'W';
        g[(20 * w + x) as usize] = 'W';
    }
    for y in 10..=20u32 {
        g[(y * w + 156) as usize] = 'W';
    }
    ShimMap::from_grid(w, h, &g.into_iter().collect::<String>())
}

/// "links gateways whose river winds far beyond their straight-line distance": with the pin's cap (4 x the
/// Manhattan distance 10 = 40 < 68) this edge was missing.
#[test]
fn v36_links_gateways_around_a_river_bend() {
    let m = river_with_bend();
    let mut bfs = crate::scratch::water::BfsGridScratch::default();
    let mut q = Vec::new();
    let (g, _) = build_version0_with(&m, &mut bfs, &mut q);
    let node = |x: u32, y: u32| g.nodes.iter().find(|n| n.x == x && n.y == y).map(|n| n.id).expect("gateway node");
    let (upper, lower) = (node(127, 10), node(127, 20));
    assert_eq!(g.get_edge_between(upper, lower).map(|e| e.cost), Some(68.0));
}

/// "routes boats around the bend": the HPA path goes down one channel, around the bend and back.
#[test]
fn v36_routes_boats_around_a_river_bend() {
    let m = river_with_bend();
    let mut g = TGame::new(m.clone(), m);
    let mut sc = Sc::new();
    let from = g.gref(2, 10);
    let to = g.gref(2, 20);
    let path = g.hpa_find_path(&mut sc, &[from], to).expect("a path");
    assert_eq!(path.first(), Some(&from));
    assert_eq!(path.last(), Some(&to));
    assert_eq!(path.len(), 2 * 154 + 10 + 1);
}
