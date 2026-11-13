//! P3 T0/T2 tests for the tile layer and the unit grid (FILE_MAP §4). Owner: **P3**.
//!
//! - `NeighborIteration.test.ts` (the map half; the conquer half is T3, it needs P6),
//! - the map-level assertions of `ImpassableTerrain.test.ts`,
//! - `UnitGrid.test.ts` (on an `Arena<Unit>` instead of a `Game`: the grid only reads
//!   `tile/lastTile/type/owner/isActive/isUnderConstruction`),
//! - mandatory design tests: nested `bfsSearch` on the same map → `TsThrow`, on the other
//!   map → OK; terrain COW + `terrain_gen`; `pristine_edits` first-edit order.

#![allow(clippy::disallowed_methods, clippy::unwrap_used, clippy::expect_used, clippy::type_complexity)] // test code

use super::game_map::*;
use super::unit_grid::*;
use crate::api::types::{TerrainType, UnitType};
use crate::arena::Arena;
use crate::ids::{SmallId, TileRef, UnitRef};
use crate::scratch::bfs::BfsScratch;
use crate::state::unit::Unit;
use std::sync::Arc;

const LAND_PLAINS: u8 = 0b1000_0000;
const IMPASSABLE: u8 = 0b1001_1111;

fn map_of(w: u32, h: u32, f: impl Fn(u32, u32) -> u8) -> GameMap {
    let mut d = Vec::with_capacity((w * h) as usize);
    let mut land = 0;
    for y in 0..h {
        for x in 0..w {
            let b = f(x, y);
            if b & 0x80 != 0 && (b & 0x1f) != 31 {
                land += 1;
            }
            d.push(b);
        }
    }
    GameMap::new(w, h, Arc::from(d), land, false)
}
fn plains(w: u32, h: u32) -> GameMap {
    map_of(w, h, |_, _| LAND_PLAINS)
}
fn throw_msg(f: impl FnOnce()) -> Option<String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
        .err()
        .map(|p| of_det::throw::as_ts_throw(&*p).expect("TsThrow").msg.to_string())
}

// ---------------------------------------------------------------- NeighborIteration.test.ts

#[test]
fn neighbors_n_s_w_e_interior_edges_corners() {
    let g = plains(16, 16); // `ocean_and_land` is 16x16; neighbour order ignores terrain
    let r = |x: f64, y: f64| g.ref_(x, y);
    let col = |t: TileRef| {
        let mut v = vec![];
        g.for_each_neighbor(t, |n| v.push(n));
        v
    };
    assert_eq!(col(r(5.0, 7.0)), [r(5.0, 6.0), r(5.0, 8.0), r(4.0, 7.0), r(6.0, 7.0)]);
    let (w, h) = (16.0, 16.0);
    assert_eq!(col(r(0.0, 0.0)), [r(0.0, 1.0), r(1.0, 0.0)]);
    assert_eq!(col(r(w - 1.0, h - 1.0)), [r(w - 1.0, h - 2.0), r(w - 2.0, h - 1.0)]);
    assert_eq!(col(r(0.0, 5.0)), [r(0.0, 4.0), r(0.0, 6.0), r(1.0, 5.0)]);
    assert_eq!(col(r(5.0, h - 1.0)), [r(5.0, h - 2.0), r(4.0, h - 1.0), r(6.0, h - 1.0)]);
    // forEachNeighbor == neighbors4 == neighbors() for every tile.
    let mut buf = [0; 4];
    g.for_each_tile(|t| {
        let n = g.neighbors4(t, &mut buf);
        assert_eq!(col(t), g.neighbors(t));
        assert_eq!(&buf[..n], g.neighbors(t).as_slice());
    });
}

#[test]
fn neighbors_with_diag_dx_major() {
    let g = plains(16, 16);
    let r = |x: f64, y: f64| g.ref_(x, y);
    let col = |t: TileRef| {
        let mut v = vec![];
        g.for_each_neighbor_with_diag(t, |n| v.push(n));
        v
    };
    assert_eq!(
        col(r(5.0, 7.0)),
        [r(4.0, 6.0), r(4.0, 7.0), r(4.0, 8.0), r(5.0, 6.0), r(5.0, 8.0), r(6.0, 6.0), r(6.0, 7.0), r(6.0, 8.0)]
    );
    assert_eq!(col(r(0.0, 0.0)), [r(0.0, 1.0), r(1.0, 0.0), r(1.0, 1.0)]);
    assert_eq!(col(r(15.0, 15.0)), [r(14.0, 14.0), r(14.0, 15.0), r(15.0, 14.0)]);
    assert_eq!(col(r(5.0, 0.0)), [r(4.0, 0.0), r(4.0, 1.0), r(5.0, 1.0), r(6.0, 0.0), r(6.0, 1.0)]);
    let mut buf = [0; 8];
    let n = g.neighbors_with_diag(r(5.0, 7.0), &mut buf);
    assert_eq!(&buf[..n], col(r(5.0, 7.0)).as_slice());
}

// ---------------------------------------------------------------- ImpassableTerrain.test.ts

const WALL_X: u32 = 100;
const WALL_WIDTH: u32 = 2;
fn impassable_map() -> GameMap {
    map_of(200, 200, |x, _| if (WALL_X..WALL_X + WALL_WIDTH).contains(&x) { IMPASSABLE } else { LAND_PLAINS })
}

#[test]
fn impassable_terrain_map_level() {
    let mut g = impassable_map();
    let r = |g: &GameMap, x: u32, y: u32| g.ref_(f64::from(x), f64::from(y));
    assert!(!g.is_impassable(r(&g, 50, 50)));
    assert!(g.is_impassable(r(&g, WALL_X, 50)));
    assert!(g.is_impassable(r(&g, WALL_X + 1, 50)));
    assert_eq!(g.terrain_type(r(&g, WALL_X, 50)), TerrainType::Impassable);
    assert!(g.is_land(r(&g, WALL_X, 50)));
    assert_eq!(g.num_land_tiles(), 200 * 200 - WALL_WIDTH * 200);
    assert!(g.is_on_edge_of_map(r(&g, WALL_X - 1, 50)));
    assert!(g.is_on_edge_of_map(r(&g, WALL_X + WALL_WIDTH, 50)));
    assert!(!g.is_on_edge_of_map(r(&g, WALL_X - 2, 50)));
    assert!(!g.is_on_edge_of_map(r(&g, 50, 50)));
    assert!(g.is_on_edge_of_map(r(&g, 0, 50)));
    assert!(g.is_on_edge_of_map(r(&g, 50, 199)));
    // setWater does not convert impassable tiles (and records no edit)
    let t = r(&g, WALL_X, 50);
    g.set_water(t);
    assert!(g.is_land(t) && g.is_impassable(t));
    assert!(g.edited_terrain_tiles().is_empty());
    assert_eq!((g.terrain_gen, g.water_version()), (0, 0));
}

// ---------------------------------------------------------------- GameMapImpl semantics

#[test]
fn ref_validation_and_throw_messages() {
    let mut g = plains(10, 5);
    assert_eq!(g.ref_(3.0, 2.0), 23);
    assert_eq!((g.x(23), g.y(23)), (3, 2));
    assert_eq!(g.try_ref(1.5, 0.0), None);
    assert_eq!(g.try_ref(-0.0, 0.0), Some(0)); // Number.isInteger(-0) && -0 >= 0
    assert_eq!(g.try_ref(10.0, 0.0), None);
    assert_eq!(
        throw_msg(|| {
            let _ = g.ref_(1.5, -1.0);
        })
        .as_deref(),
        Some("Invalid coordinates: 1.5,-1")
    );
    assert!(g.is_valid_ref(49.0) && !g.is_valid_ref(50.0) && !g.is_valid_ref(0.5) && !g.is_valid_ref(f64::NAN));
    assert_eq!(throw_msg(|| g.set_owner_id(0, 4096)).as_deref(), Some("Player ID 4096 exceeds maximum value 4095"));
    g.set_owner_id(0, 4095);
    assert_eq!(g.owner_id(0), 4095);
    let bad = throw_msg(|| drop(GameMap::new(3, 3, Arc::from(vec![0u8; 8]), 0, false)));
    assert_eq!(bad.as_deref(), Some("Terrain data length 8 doesn't match dimensions 3x3"));
}

#[test]
fn fallout_counter_and_update_tile() {
    let mut g = plains(4, 4);
    g.set_fallout(5, true);
    g.set_fallout(5, true);
    assert_eq!(g.num_tiles_with_fallout(), 1);
    g.set_fallout(6, false);
    assert_eq!(g.num_tiles_with_fallout(), 1);
    g.set_fallout(5, false);
    assert_eq!(g.num_tiles_with_fallout(), 0);
    // updateTile: state bits 0-15, terrain bits 16-23; land → water flips counters.
    let changed = g.update_tile(7, (u32::from(LAND_PLAINS) << 16) | (1 << 13) | 3);
    assert!(!changed);
    assert_eq!((g.owner_id(7), g.has_fallout(7), g.num_tiles_with_fallout()), (3, true, 1));
    let changed = g.update_tile(7, 0x20 << 16);
    assert!(changed);
    assert_eq!((g.is_ocean(7), g.is_land(7), g.num_land_tiles(), g.water_version()), (true, false, 15, 1));
    assert_eq!(g.num_tiles_with_fallout(), 0);
    assert_eq!(g.edited_terrain_tiles(), [7]);
    assert_eq!(g.pristine_edits.get(&7), Some(&LAND_PLAINS));
}

/// Every terrain write bumps `terrain_gen` and copies the shared terrain on first write (COW);
/// `pristine_edits` keeps the first original byte, in first-edit order (`:160-205`).
#[test]
fn terrain_writes_cow_gen_and_pristine_order() {
    let g0 = plains(4, 4);
    let mut g = GameMap::new(4, 4, g0.terrain.clone(), 16, false);
    assert!(Arc::ptr_eq(&g.terrain, &g0.terrain));
    g.set_magnitude(9, 0x3f); // value & 0x1f
    assert!(!Arc::ptr_eq(&g.terrain, &g0.terrain), "first write copies");
    assert_eq!((g.magnitude(9), g0.magnitude(9)), (31, 0));
    g.set_water(3);
    g.set_shoreline_bit(9);
    g.set_ocean(3);
    g.clear_shoreline_bit(9);
    assert_eq!(g.terrain_gen, 5);
    assert_eq!(g.edited_terrain_tiles(), [9, 3]);
    assert_eq!((g.pristine_edits.get(&9), g.pristine_edits.get(&3)), (Some(&LAND_PLAINS), Some(&LAND_PLAINS)));
    assert_eq!((g.terrain_byte(3), g.num_land_tiles(), g.water_version()), (0x20, 15, 1));
    assert_eq!(g.terrain_type(3), TerrainType::Ocean);
    assert_eq!((g.cost(9), g.cost(0)), (1, 2));
    // setWater on water is a no-op (no gen bump, no version bump)
    g.set_water(3);
    assert_eq!((g.terrain_gen, g.water_version()), (5, 1));
}

#[test]
fn circle_search_order_and_filter() {
    let g = plains(10, 10);
    let c = g.ref_(1.0, 1.0);
    let got = g.circle_search(c, 1.0, |_, _| true).to_vec();
    // i (x) outer, j (y) inner; corners at d2 = 2 > 1 excluded
    assert_eq!(got, [g.ref_(0.0, 1.0), g.ref_(1.0, 0.0), g.ref_(1.0, 1.0), g.ref_(1.0, 2.0), g.ref_(2.0, 1.0)]);
    let mut seen = vec![];
    let f = g.circle_search(c, 2.0, |t, d2| {
        seen.push(d2);
        t != c
    });
    assert!(!f.has(c) && f.size() == 9 + 3 - 1 - 1); // 3x4 box minus far corner minus center
    assert_eq!(seen.len(), 11);
    assert_eq!(g.circle_search(c, f64::NAN, |_, _| true).size(), 0);
}

#[test]
fn bfs_order_early_stop_and_filter_on_start() {
    let g = plains(5, 5);
    let mut sc = BfsScratch::default();
    let c = g.ref_(2.0, 2.0);
    let set = g.bfs(&mut sc, c, |gm, t| gm.manhattan_dist(c, t) <= 1);
    assert_eq!(set.to_vec(), [c, c - 5, c + 5, c - 1, c + 1]);
    let mut n = 0;
    assert!(!g.bfs_search(
        &mut sc,
        c,
        |_, _| true,
        |_| {
            n += 1;
            n < 3
        }
    ));
    assert_eq!(n, 3);
    assert!(!sc.busy && sc.queue.is_empty());
    // filter(start) false → returns true without visiting
    assert!(g.bfs_search(&mut sc, c, |_, _| false, |_| panic!("visited")));
    // distance predicates (GameMapImpl.ts:665-788)
    let e = eucl_dist_fn(c, 1.0, true);
    let set = g.bfs(&mut sc, c, e);
    // center=true shifts the root to (1.5,1.5): (1,1),(1,2),(2,1),(2,2) are within 1
    assert_eq!(set.size(), 4);
    let m = and_fn(manhattan_dist_fn(c, 2.0, false), |gm: &GameMap, t| gm.x(t) >= 2);
    assert_eq!(g.bfs(&mut sc, c, m).size(), 9);
    assert!(rect_dist_fn(c, 1.0, false)(&g, g.ref_(3.0, 3.0)));
    assert!(!hex_dist_fn(c, 1.0, false)(&g, g.ref_(3.0, 3.0)));
    assert!(isometric_dist_fn(c, 1.0, true)(&g, g.ref_(2.0, 2.0)));
}

/// Mandatory (FILE_MAP §4): a nested `bfsSearch` on the same map throws the TS message, and
/// the guard is released by the TS `finally`; a nested one on the other map works.
#[test]
fn nested_bfs_same_map_throws_other_map_ok() {
    let main = plains(6, 6);
    let mini = GameMap::new(3, 3, Arc::from(vec![LAND_PLAINS; 9]), 9, true);
    let mut sc_main = BfsScratch::default();
    let mut sc_mini = BfsScratch::default();
    // other map: fine
    let mut inner_total = 0;
    main.bfs_search(
        &mut sc_main,
        0,
        |_, _| true,
        |_| {
            inner_total += mini.bfs(&mut sc_mini, 4, |_, _| true).size();
            true
        },
    );
    assert_eq!(inner_total, 36 * 9);
    // same map: TsThrow. In Rust the borrow checker already rejects re-entering through the
    // same `&mut BfsScratch`, so an outer search in progress is simulated by its `enter()`.
    sc_main.enter();
    let msg = throw_msg(|| {
        let _ = main.bfs_search(&mut sc_main, 0, |_, _| true, |_| true);
    });
    assert_eq!(msg.as_deref(), Some("Re-entrant bfsSearch is not supported"));
    assert!(sc_main.busy, "the inner throw happens before its try: the outer search still owns the guard");
    sc_main.exit(); // the outer search's `finally`
    // a throw from inside a search unwinds through the guard (TS `finally`)
    let msg = throw_msg(|| {
        let _ = main.bfs_search(&mut sc_main, 0, |_, _| true, |_| of_det::ts_throw!("boom"));
    });
    assert_eq!(msg.as_deref(), Some("boom"));
    assert!(!sc_main.busy && sc_main.queue.is_empty(), "finally cleared the guard and the queue");
    // the scratch is usable again
    assert_eq!(main.bfs(&mut sc_main, 0, |_, _| true).size(), 36);
}

#[test]
fn mini_map_state_reads_zero() {
    let mini = GameMap::new(3, 3, Arc::from(vec![LAND_PLAINS; 9]), 9, true);
    assert_eq!((mini.owner_id(4), mini.has_fallout(4), mini.tile_state(4)), (0, false, 0));
    assert!(mini.tile_state_buffer().is_none());
}

// ---------------------------------------------------------------- UnitGrid.test.ts

struct World {
    units: Arena<Unit, UnitRef>,
    grid: UnitGrid,
    w: u32,
}
impl World {
    fn new(w: u32, h: u32) -> Self {
        World { units: Arena::new(), grid: UnitGrid::new(w, h), w }
    }
    fn build(&mut self, t: UnitType, x: u32, y: u32, owner: u16) -> UnitRef {
        let tile = y * self.w + x;
        let u = self.units.alloc(Unit {
            id: self.units.len() as u32 + 1,
            unit_type: t,
            owner: SmallId(owner),
            tile,
            last_tile: tile,
            active: true,
            ..Unit::default()
        });
        self.grid.add_unit(&self.units, u);
        u
    }
    fn mv(&mut self, u: UnitRef, x: u32, y: u32) {
        let tile = y * self.w + x;
        let d = self.units.get_mut(u);
        d.last_tile = d.tile;
        d.tile = tile;
        self.grid.update_unit_cell(&self.units, u);
    }
    fn has(&self, x: u32, range: f64, t: UnitType, p: Option<u16>) -> bool {
        self.grid.has_unit_nearby(&self.units, self.w, x, range, t, p.map(SmallId), false)
    }
    fn nearby(&self, x: u32, range: f64, ts: &[UnitType]) -> Vec<NearbyUnit> {
        self.grid.nearby_units(&self.units, self.w, x, range, ts, |_, _| true, false)
    }
}

#[test]
fn unit_grid_range_cases() {
    // [map, unitPosX, range, rangeCheck, expected] (UnitGrid.test.ts:56-62)
    for (dim, ux, range, check, want) in [
        (100, 0, 10.0, 0, true),
        (100, 0, 10.0, 10, true),
        (100, 0, 10.0, 11, false),
        (200, 0, 198.0, 42, true),
        (200, 0, 198.0, 199, false),
    ] {
        let mut w = World::new(dim, dim);
        w.build(UnitType::DefensePost, ux, 0, 1);
        assert_eq!(w.has(check, range, UnitType::DefensePost, Some(1)), want, "{dim} {ux} {range} {check}");
        assert!(!w.has(check, range, UnitType::DefensePost, Some(2)), "owner filter");
    }
    // [map, unitPosX, range, rangeCheck, units, expected] (:79-87)
    use UnitType::*;
    let cases: &[(u32, u32, f64, u32, &[UnitType], usize)] = &[
        (100, 0, 10.0, 0, &[Warship], 1),
        (100, 0, 10.0, 0, &[City, Port], 2),
        (100, 0, 10.0, 0, &[], 0),
        (100, 0, 10.0, 10, &[City], 1),
        (100, 0, 10.0, 11, &[DefensePost], 0),
        (200, 0, 198.0, 42, &[TradeShip], 1),
        (200, 0, 198.0, 199, &[TransportShip], 0),
    ];
    for &(dim, ux, range, check, ts, want) in cases {
        let mut w = World::new(dim, dim);
        for &t in ts {
            w.build(t, ux, 0, 1);
        }
        assert_eq!(w.nearby(check, range, ts).len(), want);
    }
    // Wrong unit type in range (:104-118)
    let mut w = World::new(100, 100);
    w.build(City, 0, 0, 1);
    assert!(w.nearby(0, 10.0, &[Port]).is_empty());
    // One inside, one outside (:120-135)
    w.build(City, 99, 0, 1);
    assert_eq!(w.nearby(0, 10.0, &[City]).len(), 1);
}

/// CS G.unitGrid orders (ARCHITECTURE §3.7): per-cell type order = first insertion (emptied
/// sets keep their key), in-set order = insertion; a cross-cell move appends at the end.
#[test]
fn unit_grid_orders_moves_and_refs() {
    use UnitType::*;
    let mut w = World::new(250, 150); // 3 x 2 cells
    assert_eq!((w.grid.cols, w.grid.rows, w.grid.cells.len()), (3, 2, 6));
    let a = w.build(City, 5, 5, 1);
    let b = w.build(Port, 6, 5, 1);
    let c = w.build(City, 7, 5, 2);
    let d = w.build(Warship, 150, 120, 1); // cell 4
    let snap = w.grid.snapshot();
    assert_eq!(snap[0], vec![(City, vec![a, c]), (Port, vec![b])]);
    assert_eq!(snap[4], vec![(Warship, vec![d])]);
    // move inside a cell: nothing changes; across cells: remove + append
    w.mv(a, 50, 50);
    assert_eq!(w.grid.snapshot()[0], vec![(City, vec![a, c]), (Port, vec![b])]);
    w.mv(a, 120, 10); // cell 1
    w.mv(a, 60, 10); // back to cell 0: appended after c
    let snap = w.grid.snapshot();
    assert_eq!(snap[0], vec![(City, vec![c, a]), (Port, vec![b])]);
    assert_eq!(snap[1], vec![(City, vec![])], "emptied set keeps its key");
    // remove by tile of another cell: no-op; remove: gone, key stays
    w.grid.remove_unit_by_tile(&w.units, b, 200);
    w.grid.remove_unit(&w.units, b);
    assert_eq!(w.grid.snapshot()[0], vec![(City, vec![c, a]), (Port, vec![])]);
    // query order: cells row-major, then the given type order, then set order
    let got: Vec<UnitRef> = w
        .grid
        .nearby_units(&w.units, 250, 0, 1000.0, &[Warship, City], |_, _| true, false)
        .iter()
        .map(|n| n.unit)
        .collect();
    assert_eq!(got, [c, a, d]);
    // inactive / under construction are skipped unless included
    w.units.get_mut(c).under_construction = true;
    w.units.get_mut(d).active = false;
    let got: Vec<UnitRef> = w
        .grid
        .nearby_units(&w.units, 250, 0, 1000.0, &[Warship, City], |_, _| true, false)
        .iter()
        .map(|n| n.unit)
        .collect();
    assert_eq!(got, [a]);
    let got: Vec<UnitRef> = w
        .grid
        .nearby_units(&w.units, 250, 0, 1000.0, &[Warship, City], |_, _| true, true)
        .iter()
        .map(|n| n.unit)
        .collect();
    assert_eq!(got, [c, a]);
    // anyUnitNearby: predicate sees in-range units in order, stops at the first true
    let mut seen = vec![];
    assert!(w.grid.any_unit_nearby(
        &w.units,
        250,
        0,
        1000.0,
        &[City],
        |u| {
            seen.push(u);
            true
        },
        Some(SmallId(1)),
        true
    ));
    assert_eq!(seen, [a]);
    // Refs: every filed unit (dead ones included), in snapshot order
    let mut log = crate::canon::RefLog::default();
    crate::canon::Refs::visit_refs(&w.grid, &mut log);
    assert_eq!(log.units, [c, a, d]);
    // a deleted unit that moves is re-filed (TS updateUnitCell → addUnit)
    w.grid.remove_unit(&w.units, d);
    w.units.get_mut(d).active = false;
    w.mv(d, 10, 10);
    assert_eq!(w.grid.snapshot()[0][0], (City, vec![c, a]));
    assert_eq!(w.grid.snapshot()[0][2], (Warship, vec![d]));
}

#[test]
fn unit_grid_cells_in_range_matches_ts() {
    // getCellsInRange (`UnitGrid.ts:117-140`) with range 0 on a cell boundary: x = 100 gives
    // startGridX = 1 but endGridX = 1 + ceil(-100 / 100) = 0, so TS scans no cell and misses
    // the unit on the query tile itself (same for y % 100 == 0). Ported literally.
    let mut w = World::new(300, 100);
    let a = w.build(UnitType::City, 99, 5, 1);
    let b = w.build(UnitType::City, 100, 5, 1);
    let at = |x: u32, y: u32| y * 300 + x;
    let ids = |v: Vec<NearbyUnit>| v.iter().map(|n| n.unit).collect::<Vec<_>>();
    assert!(w.nearby(at(100, 5), 0.0, &[UnitType::City]).is_empty());
    assert!(!w.has(at(100, 5), 0.0, UnitType::City, None));
    assert_eq!(ids(w.nearby(at(99, 5), 0.0, &[UnitType::City])), [a]);
    assert!(w.nearby(at(99, 0), 5.0, &[UnitType::City]).len() == 1); // y = 0 boundary, range 5 reaches (99,5)
    assert!(w.nearby(at(99, 0), 0.0, &[UnitType::City]).is_empty());
    assert_eq!(ids(w.nearby(at(100, 5), 1.0, &[UnitType::City])), [a, b]);
    // negative or NaN ranges find nothing
    assert!(w.nearby(at(100, 5), -5.0, &[UnitType::City]).is_empty());
    assert!(w.nearby(at(100, 5), f64::NAN, &[UnitType::City]).is_empty());
}

// ---------------------------------------------------------------- TerrainMapLoader.ts

fn testdata_map(name: &str) -> (crate::map::manifest::MapManifest, Vec<u8>, Vec<u8>, Vec<u8>) {
    let d = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/testdata/maps").join(name);
    let rd = |f: &str| std::fs::read(d.join(f)).unwrap();
    (serde_json::from_slice(&rd("manifest.json")).unwrap(), rd("map.bin"), rd("map4x.bin"), rd("map16x.bin"))
}

#[test]
fn load_terrain_plains_normal_and_compact() {
    use crate::api::map_files::MapFiles;
    use crate::api::types::GameMapSize;
    use crate::map::loader::MapStatics;
    let (manifest, map, map4x, map16x) = testdata_map("plains");
    let files = MapFiles {
        map: "Plains".into(),
        map_size: GameMapSize::Normal,
        manifest: manifest.clone(),
        map_bin: Some(map.clone()),
        map4x_bin: Some(map4x.clone()),
        map16x_bin: Some(map16x.clone()),
    };
    let st = MapStatics::load_terrain(&files).unwrap();
    assert_eq!((st.width, st.height, st.mini_width, st.mini_height), (100, 100, 50, 50));
    assert_eq!(st.num_land_tiles, 10_000);
    assert_eq!((st.land_index_of(0), st.land_index_of(9_999)), (0, 9_999));
    assert!(st.nations.is_empty(), "plains has no `nations` key");
    assert!(st.graph0.is_none() && st.components0.is_none());
    // Compact: map4x + map16x
    let compact = MapFiles { map_size: GameMapSize::Compact, map_bin: None, ..files.clone() };
    let st = MapStatics::load_terrain(&compact).unwrap();
    assert_eq!((st.width, st.height, st.mini_width, st.mini_height), (50, 50, 25, 25));
    // missing bin → the TS message
    let bad = MapFiles { map4x_bin: None, ..files.clone() };
    assert_eq!(MapStatics::load_terrain(&bad).err().as_deref(), Some("map4x.bin of Plains was not passed"));
    // wrong length → genTerrainFromBin's message
    let bad = MapFiles { map_bin: Some(map[..9_999].to_vec()), ..files };
    assert_eq!(
        MapStatics::load_terrain(&bad).err().as_deref(),
        Some("Invalid data: buffer size 9999 incorrect for 100x100 terrain plus 4 bytes for dimensions.")
    );
}

#[test]
fn compact_scaling_of_nations_and_spawn_areas() {
    use crate::api::map_files::MapFiles;
    use crate::api::types::{Cell, GameMapSize, SpawnArea};
    use crate::map::loader::MapStatics;
    let (mut manifest, map, map4x, map16x) = testdata_map("plains");
    let j = serde_json::json!({
        "nations": [{"name": "A", "coordinates": [7, 9], "flag": "fa"}, {"name": "B"}],
        "additionalNations": [{"name": "C", "coordinates": [3, 1]}],
        "teamGameSpawnAreas": {"10": [{"x": 1, "y": 3, "width": 1, "height": 5}], "2": [{"x": 9, "y": 9, "width": 4, "height": 4}]},
        "layers": [{"id": "trees", "placement": "land", "alpha": 0.5}]
    });
    let extra: crate::map::manifest::MapManifest = {
        let mut v = serde_json::to_value(&manifest).unwrap();
        for (k, x) in j.as_object().unwrap() {
            v[k] = x.clone();
        }
        serde_json::from_value(v).unwrap()
    };
    manifest = extra;
    let files = MapFiles {
        map: "Plains".into(),
        map_size: GameMapSize::Compact,
        manifest: manifest.clone(),
        map_bin: Some(map),
        map4x_bin: Some(map4x),
        map16x_bin: Some(map16x),
    };
    let st = MapStatics::load_terrain(&files).unwrap();
    assert_eq!(st.nations[0].spawn_cell, Some(Cell::new(3.0, 4.0)));
    assert_eq!(st.nations[0].player_info.nation_flag.as_deref(), Some("fa"));
    assert_eq!(st.nations[1].spawn_cell, None);
    assert_eq!(st.additional_nations[0].spawn_cell, Some(Cell::new(1.0, 0.0)));
    let areas = st.team_game_spawn_areas.as_ref().unwrap();
    // Object.entries: integer keys ascending
    assert_eq!(areas[0], (2, vec![SpawnArea { x: 4.0, y: 4.0, width: 2.0, height: 2.0 }]));
    assert_eq!(areas[1], (10, vec![SpawnArea { x: 0.0, y: 1.0, width: 1.0, height: 2.0 }]));
    // an invalid layer placement is rejected with the TS message
    let mut m2 = manifest.clone();
    m2.extra.insert("layers".into(), serde_json::json!([{"id": "x", "placement": "air"}]));
    let bad = MapFiles { manifest: m2, ..files };
    assert_eq!(
        MapStatics::load_terrain(&bad).err().as_deref(),
        Some("Map Plains: layer \"x\" has invalid placement \"air\" (must be \"land\" or \"water\")")
    );
}
