//! Test-only stand-ins for the water layer's two foreign dependencies. Owner: **P4a**.
//!
//! - [`ShimMap`]: a literal `GameMapImpl` (`engine-lib/src/game/GameMapImpl.ts`) subset
//!   behind [`WaterMap`]/[`WaterMapMut`], so the water code is testable while P3's
//!   `GameMap` bodies are still `todo!()`. Remove once P3 lands (tests then use `GameMap`).
//! - [`ShimBounded`]: a literal port of `AStarWaterBounded.searchBounded`
//!   (`pathfinding/algorithms/AStar.WaterBounded.ts`, owned by **P4b**), the HPA's local
//!   search. Test-only reference; production calls `path::astar_water_bounded`.
//!
//! Both are verified against the TS engine by the golden oracle (`tests.rs`,
//! `testdata/oracle/*`).

use crate::ids::TileRef;
use crate::path::astar_water_bounded::SearchBounds;
use crate::water::hpa::LocalKind;
use crate::water::map_view::{WaterMap, WaterMapMut};
use crate::water::priority_queue::MinHeap;
use of_det::ts_throw;

/// `GameMapImpl` terrain + state (`:14-15`), with the Rust-only `terrain_gen`.
#[derive(Clone, Debug, Default)]
pub(crate) struct ShimMap {
    pub w: u32,
    pub h: u32,
    pub terrain: Vec<u8>,
    pub state: Vec<u16>,
    pub num_land_tiles: u32,
    pub water_version: u32,
    pub num_tiles_with_fallout: u32,
    pub terrain_gen: u32,
}

impl ShimMap {
    pub fn new(w: u32, h: u32, terrain: Vec<u8>) -> Self {
        assert_eq!(terrain.len(), (w * h) as usize);
        let num_land_tiles = terrain.iter().filter(|&&b| b & 0x80 != 0).count() as u32;
        ShimMap { w, h, state: vec![0; terrain.len()], terrain, num_land_tiles, ..Default::default() }
    }
    /// `tests/core/pathfinding/_fixtures.ts:533-551` `createGameMap` (W = 0x20, L = 0x80,
    /// then `computeShoreline`).
    pub fn from_grid(w: u32, h: u32, grid: &str) -> Self {
        let cells: Vec<char> = grid.chars().filter(|c| *c == 'W' || *c == 'L').collect();
        assert_eq!(cells.len(), (w * h) as usize);
        let mut t: Vec<u8> = cells.iter().map(|&c| if c == 'L' { 0x80 } else { 0x20 }).collect();
        compute_shoreline(&mut t, w, h);
        ShimMap::new(w, h, t)
    }
    pub fn set_owner(&mut self, t: TileRef, id: u16) {
        let s = &mut self.state[t as usize];
        *s = (*s & !0xfff) | id;
    }
    pub fn magnitude(&self, t: TileRef) -> u8 {
        self.terrain[t as usize] & 0x1f
    }
    /// `setFallout(ref, true)`.
    pub fn set_fallout_on(&mut self, t: TileRef) {
        WaterMapMut::set_fallout(self, t, true)
    }
}

/// `_fixtures.ts:398-424` `computeShoreline`.
pub(crate) fn compute_shoreline(terrain: &mut [u8], w: u32, h: u32) {
    let (w, h) = (w as i64, h as i64);
    for y in 0..h {
        for x in 0..w {
            let idx = (y * w + x) as usize;
            let is_land = terrain[idx] & 0x80 != 0;
            for (nx, ny) in [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)] {
                if nx < 0 || nx >= w || ny < 0 || ny >= h {
                    continue;
                }
                if (terrain[(ny * w + nx) as usize] & 0x80 != 0) != is_land {
                    terrain[idx] |= 1 << 6;
                    break;
                }
            }
        }
    }
}

impl WaterMap for ShimMap {
    fn width(&self) -> u32 {
        self.w
    }
    fn height(&self) -> u32 {
        self.h
    }
    fn terrain(&self) -> &[u8] {
        &self.terrain
    }
    fn x(&self, t: TileRef) -> u32 {
        t % self.w
    }
    fn y(&self, t: TileRef) -> u32 {
        t / self.w
    }
    fn ref_(&self, x: f64, y: f64) -> TileRef {
        if !self.is_valid_coord(x, y) {
            ts_throw!("Invalid coordinates: {},{}", x, y);
        }
        (y as u32) * self.w + x as u32
    }
    fn is_valid_coord(&self, x: f64, y: f64) -> bool {
        x.is_finite()
            && x.trunc() == x
            && y.is_finite()
            && y.trunc() == y
            && x >= 0.0
            && x < f64::from(self.w)
            && y >= 0.0
            && y < f64::from(self.h)
    }
    fn is_land(&self, t: TileRef) -> bool {
        self.terrain[t as usize] & 0x80 != 0
    }
    fn is_water(&self, t: TileRef) -> bool {
        !self.is_land(t)
    }
    fn is_ocean(&self, t: TileRef) -> bool {
        self.terrain[t as usize] & 0x20 != 0
    }
    fn is_shoreline(&self, t: TileRef) -> bool {
        self.terrain[t as usize] & 0x40 != 0
    }
    fn is_impassable(&self, t: TileRef) -> bool {
        self.is_land(t) && self.terrain[t as usize] & 0x1f == 31
    }
    fn has_owner(&self, t: TileRef) -> bool {
        self.state[t as usize] & 0xfff != 0
    }
    fn has_fallout(&self, t: TileRef) -> bool {
        self.state[t as usize] & (1 << 13) != 0
    }
    fn manhattan_dist(&self, a: TileRef, b: TileRef) -> u32 {
        self.x(a).abs_diff(self.x(b)) + self.y(a).abs_diff(self.y(b))
    }
    fn neighbors4(&self, r: TileRef, out: &mut [TileRef; 4]) -> usize {
        let w = self.w;
        let x = r % w;
        let mut n = 0;
        if r >= w {
            out[n] = r - w;
            n += 1;
        }
        if r < (self.h - 1) * w {
            out[n] = r + w;
            n += 1;
        }
        if x != 0 {
            out[n] = r - 1;
            n += 1;
        }
        if x != w - 1 {
            out[n] = r + 1;
            n += 1;
        }
        n
    }
    fn terrain_gen(&self) -> u32 {
        self.terrain_gen
    }
}

impl WaterMapMut for ShimMap {
    fn set_water(&mut self, t: TileRef) {
        if !self.is_land(t) || self.is_impassable(t) {
            return;
        }
        self.water_version += 1;
        self.terrain[t as usize] = 0;
        self.num_land_tiles -= 1;
        self.terrain_gen += 1;
    }
    fn set_ocean(&mut self, t: TileRef) {
        self.terrain[t as usize] |= 1 << 5;
        self.terrain_gen += 1;
    }
    fn set_shoreline_bit(&mut self, t: TileRef) {
        self.terrain[t as usize] |= 1 << 6;
        self.terrain_gen += 1;
    }
    fn clear_shoreline_bit(&mut self, t: TileRef) {
        self.terrain[t as usize] &= !(1 << 6);
        self.terrain_gen += 1;
    }
    fn set_magnitude(&mut self, t: TileRef, value: u8) {
        let b = &mut self.terrain[t as usize];
        *b = (*b & !0x1f) | (value & 0x1f);
        self.terrain_gen += 1;
    }
    fn set_fallout(&mut self, t: TileRef, value: bool) {
        let has = self.has_fallout(t);
        if value {
            if !has {
                self.num_tiles_with_fallout += 1;
                self.state[t as usize] |= 1 << 13;
            }
        } else if has {
            self.num_tiles_with_fallout -= 1;
            self.state[t as usize] &= !(1 << 13);
        }
    }
}

/// One `AStarWaterBounded` instance (`AStar.WaterBounded.ts:29-58`).
#[derive(Debug)]
pub(crate) struct BoundedInst {
    stamp: u32,
    closed_stamp: Vec<u32>,
    g_score_stamp: Vec<u32>,
    g_score: Vec<u32>,
    came_from: Vec<i32>,
    queue: MinHeap,
}

impl BoundedInst {
    fn new(max_search_area: usize) -> Self {
        BoundedInst {
            stamp: 1,
            closed_stamp: vec![0; max_search_area],
            g_score_stamp: vec![0; max_search_area],
            g_score: vec![0; max_search_area],
            came_from: vec![0; max_search_area],
            queue: MinHeap::new(max_search_area * 4),
        }
    }
}

/// The HPA's three bounded A*s (`AStar.WaterHierarchical.ts:33-47`), default config
/// (`heuristicWeight 3`, `maxIterations 100_000`).
pub(crate) struct ShimBounded {
    local: BoundedInst,
    multi: BoundedInst,
    short: BoundedInst,
}

impl ShimBounded {
    pub fn new(cluster_size: u32) -> Self {
        let cs = cluster_size as usize;
        ShimBounded {
            local: BoundedInst::new(cs * cs),
            multi: BoundedInst::new((cs * 3) * (cs * 3)),
            short: BoundedInst::new(260 * 260),
        }
    }
    pub fn search<M: WaterMap + ?Sized>(
        &mut self,
        kind: LocalKind,
        map: &M,
        start: &[TileRef],
        goal: TileRef,
        b: SearchBounds,
    ) -> Option<Vec<TileRef>> {
        let inst = match kind {
            LocalKind::Local => &mut self.local,
            LocalKind::Multi => &mut self.multi,
            LocalKind::Short => &mut self.short,
        };
        search_bounded(inst, map, start, goal, b)
    }
}

const COST_SCALE: i64 = 100;
const BASE_COST: i64 = COST_SCALE;

/// `getMagnitudePenalty(magnitude)` (`:11-15`).
fn magnitude_penalty(m: u8) -> i64 {
    if m < 3 {
        3 * COST_SCALE
    } else if m <= 10 {
        0
    } else {
        COST_SCALE
    }
}

/// `searchBounded(start, goal, bounds)` (`:87-310`), literal.
fn search_bounded<M: WaterMap + ?Sized>(
    s: &mut BoundedInst,
    map: &M,
    start: &[TileRef],
    goal: TileRef,
    bounds: SearchBounds,
) -> Option<Vec<TileRef>> {
    if s.stamp == u32::MAX {
        s.closed_stamp.fill(0);
        s.g_score_stamp.fill(0);
        s.stamp = 1;
    } else {
        s.stamp += 1;
    }
    let stamp = s.stamp;
    let map_width = i64::from(map.width());
    let terrain = map.terrain();
    let weight: i64 = 3;
    let (min_x, max_x, min_y, max_y) =
        (i64::from(bounds.min_x), i64::from(bounds.max_x), i64::from(bounds.min_y), i64::from(bounds.max_y));
    let bounds_width = max_x - min_x + 1;
    let goal_i = i64::from(goal);
    let goal_x = goal_i % map_width;
    let goal_y = goal_i / map_width;
    let bounds_height = max_y - min_y + 1;
    let num_local_nodes = bounds_width * bounds_height;
    if num_local_nodes > s.closed_stamp.len() as i64 {
        return None;
    }
    let to_local = |tile: i64, clamp: bool| -> i64 {
        let mut x = tile % map_width;
        let mut y = tile / map_width;
        if clamp {
            x = min_x.max(max_x.min(x));
            y = min_y.max(max_y.min(y));
        }
        (y - min_y) * bounds_width + (x - min_x)
    };
    let to_global = |local: i64| -> i64 {
        let lx = local % bounds_width;
        let ly = local / bounds_width;
        (ly + min_y) * map_width + (lx + min_x)
    };
    let goal_local = to_local(goal_i, true);
    if goal_local < 0 || goal_local >= num_local_nodes {
        return None;
    }
    s.queue.clear();
    let s0 = i64::from(start[0]);
    let start_x = s0 % map_width;
    let start_y = s0 / map_width;
    let dx_goal = goal_x - start_x;
    let dy_goal = goal_y - start_y;
    let cross_norm = (dx_goal.abs() + dy_goal.abs()).max(1) as f64;
    let cross_tie = |nx: i64, ny: i64| -> f64 {
        let dxn = nx - goal_x;
        let dyn_ = ny - goal_y;
        let cross = (dx_goal * dyn_ - dy_goal * dxn).abs() as f64;
        of_det::js::floor((cross * (COST_SCALE - 1) as f64) / cross_norm / cross_norm)
    };
    for &st in start {
        let st = i64::from(st);
        let sl = to_local(st, true);
        if sl < 0 || sl >= num_local_nodes {
            continue;
        }
        let su = sl as usize;
        s.g_score[su] = 0;
        s.g_score_stamp[su] = stamp;
        s.came_from[su] = -1;
        let sx = st % map_width;
        let sy = st / map_width;
        let h = weight * BASE_COST * ((sx - goal_x).abs() + (sy - goal_y).abs());
        s.queue.push(sl as i32, h as f64);
    }
    let mut iterations: i64 = 100_000;
    while !s.queue.is_empty() {
        iterations -= 1;
        if iterations <= 0 {
            return None;
        }
        let current_local = i64::from(s.queue.pop());
        let cu = current_local as usize;
        if s.closed_stamp[cu] == stamp {
            continue;
        }
        s.closed_stamp[cu] = stamp;
        if current_local == goal_local {
            let mut path = Vec::new();
            let mut cur = goal_local;
            let mut it = 0;
            while cur != -1 && it < num_local_nodes {
                path.push(to_global(cur) as TileRef);
                cur = i64::from(s.came_from[cur as usize]);
                it += 1;
            }
            path.reverse();
            return Some(path);
        }
        let current_g = i64::from(s.g_score[cu]);
        let current = to_global(current_local);
        let current_x = current % map_width;
        let current_y = current / map_width;
        let relax = |neighbor: i64, neighbor_local: i64, nx: i64, ny: i64, s: &mut BoundedInst| {
            let nt = terrain[neighbor as usize];
            let nl = neighbor_local as usize;
            if s.closed_stamp[nl] != stamp && (neighbor == goal_i || (nt & 0x80) == 0) {
                let dist_to_goal = (nx - goal_x).abs() + (ny - goal_y).abs();
                let cost = BASE_COST + magnitude_penalty(nt & 0x1f);
                let tentative_g = current_g + cost;
                if s.g_score_stamp[nl] != stamp || tentative_g < i64::from(s.g_score[nl]) {
                    s.came_from[nl] = current_local as i32;
                    s.g_score[nl] = tentative_g as u32;
                    s.g_score_stamp[nl] = stamp;
                    let h = weight * BASE_COST * dist_to_goal;
                    let f = (tentative_g + h) as f64 + cross_tie(nx, ny);
                    s.queue.push(neighbor_local as i32, f);
                }
            }
        };
        if current_y > min_y {
            relax(current - map_width, current_local - bounds_width, current_x, current_y - 1, s);
        }
        if current_y < max_y {
            relax(current + map_width, current_local + bounds_width, current_x, current_y + 1, s);
        }
        if current_x > min_x {
            relax(current - 1, current_local - 1, current_x - 1, current_y, s);
        }
        if current_x < max_x {
            relax(current + 1, current_local + 1, current_x + 1, current_y, s);
        }
    }
    None
}
