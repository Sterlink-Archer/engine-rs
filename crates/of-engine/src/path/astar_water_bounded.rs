//! `AStarWaterBounded` (`pathfinding/algorithms/AStar.WaterBounded.ts`). Owner: **P4b**.
//!
//! Instances (each a `Scratch` field, `scratch/path.rs`): the smoother's `localAStar` on the
//! minimap (`SmoothingWaterTransformer.ts:33`, area 100×100), `SpatialQuery`'s refiner on
//! the **main** map (`SpatialQuery.ts:32-39`, area 100×100), and P4a's three HPA locals.
//! The capacity `maxSearchArea` is semantic (`:117-119`), so it is part of the instance:
//! [`search_bounded_cap`] takes it; the frozen [`search_bounded`] uses the capacity the
//! scratch was last sized with ([`AStarWaterBoundedScratch::ensure`]).
//!
//! Coordinates are JS numbers: all box arithmetic is `i64` (exact; can go negative for a
//! degenerate box exactly as in TS). Terrain reads past the array read `undefined`
//! (`undefined & mask === 0`), as TS does.

use crate::map::GameMap;
use crate::path::MapRead;
use crate::path::astar::PQueue;
use crate::scratch::path::AStarWaterBoundedScratch;
use of_det::js;

const LAND_BIT: u32 = 7; // TS: :5
const MAGNITUDE_MASK: u8 = 0x1f; // TS: :6
const COST_SCALE: f64 = 100.0; // TS: :7
const BASE_COST: f64 = 1.0 * COST_SCALE; // TS: :8

/// `getMagnitudePenalty` (`:11-15`): 3× near shore here (AStarWater uses 10×).
fn get_magnitude_penalty(magnitude: u8) -> f64 {
    if magnitude < 3 {
        return 3.0 * COST_SCALE;
    }
    if magnitude <= 10 {
        return 0.0;
    }
    1.0 * COST_SCALE
}

/// `BoundedAStarConfig` (`:17-20`).
#[derive(Clone, Copy, Debug, Default)]
pub struct BoundedAStarConfig {
    pub heuristic_weight: Option<f64>,
    pub max_iterations: Option<u32>,
}

/// `SearchBounds` (`:22-27`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchBounds {
    pub min_x: u32,
    pub max_x: u32,
    pub min_y: u32,
    pub max_y: u32,
}

/// `findPath(start, goal)` (`:60-85`): the box spanned by the starts and the goal.
pub fn find_path(
    sc: &mut AStarWaterBoundedScratch,
    map: &GameMap,
    max_search_area: u32,
    cfg: BoundedAStarConfig,
    start: &[u32],
    goal: u32,
) -> Option<Vec<u32>> {
    find_path_m(sc, map, max_search_area, cfg, start, goal)
}

/// [`find_path`] over any [`MapRead`].
pub fn find_path_m<M: MapRead>(
    sc: &mut AStarWaterBoundedScratch,
    map: &M,
    max_search_area: u32,
    cfg: BoundedAStarConfig,
    start: &[u32],
    goal: u32,
) -> Option<Vec<u32>> {
    let map_width = map.width();
    let goal_x = goal % map_width;
    let goal_y = goal / map_width;
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (goal_x, goal_x, goal_y, goal_y);
    for &s in start {
        let sx = s % map_width;
        let sy = s / map_width;
        // TS: :73-76 Math.min/max on non-negative integers
        min_x = min_x.min(sx);
        max_x = max_x.max(sx);
        min_y = min_y.min(sy);
        max_y = max_y.max(sy);
    }
    search_bounded_m(sc, map, max_search_area, cfg, start, goal, SearchBounds { min_x, max_x, min_y, max_y })
}

/// `searchBounded(start, goal, bounds)` (`:87-310`) on an instance already sized by
/// [`AStarWaterBoundedScratch::ensure`] (or by an earlier [`search_bounded_cap`]). A
/// scratch never sized is a Rust bug (it would silently refuse every box).
pub fn search_bounded(
    sc: &mut AStarWaterBoundedScratch,
    map: &GameMap,
    cfg: BoundedAStarConfig,
    start: &[u32],
    goal: u32,
    bounds: SearchBounds,
) -> Option<Vec<u32>> {
    assert!(sc.queue.is_some(), "AStarWaterBoundedScratch used before ensure(maxSearchArea); use search_bounded_cap");
    let cap = sc.max_search_area;
    search_bounded_m(sc, map, cap, cfg, start, goal, bounds)
}

/// `searchBounded` on the instance of capacity `max_search_area`.
pub fn search_bounded_cap(
    sc: &mut AStarWaterBoundedScratch,
    map: &GameMap,
    max_search_area: u32,
    cfg: BoundedAStarConfig,
    start: &[u32],
    goal: u32,
    bounds: SearchBounds,
) -> Option<Vec<u32>> {
    search_bounded_m(sc, map, max_search_area, cfg, start, goal, bounds)
}

/// [`search_bounded_cap`] over any [`MapRead`].
pub fn search_bounded_m<M: MapRead>(
    sc: &mut AStarWaterBoundedScratch,
    map: &M,
    max_search_area: u32,
    cfg: BoundedAStarConfig,
    start: &[u32],
    goal: u32,
    bounds: SearchBounds,
) -> Option<Vec<u32>> {
    // TS: :42-58 (constructor)
    sc.ensure(max_search_area);
    let heuristic_weight = cfg.heuristic_weight.unwrap_or(3.0);
    let max_iterations = cfg.max_iterations.unwrap_or(100_000);

    // TS: :92-97
    if sc.stamp == u32::MAX {
        sc.closed_stamp.fill(0);
        sc.g_score_stamp.fill(0);
        sc.stamp = 1;
    } else {
        sc.stamp += 1;
    }
    let stamp = sc.stamp;
    let map_width = i64::from(map.width());
    let terrain = map.terrain();
    let AStarWaterBoundedScratch { closed_stamp, g_score_stamp, g_score, came_from, queue, .. } = sc;
    let queue = queue.as_mut().expect("rust-invariant: ensure() built the queue");
    let weight = heuristic_weight;
    let land_mask: u8 = 1 << LAND_BIT;
    // `terrain[t]` with `undefined` past the end (`undefined & m === 0`).
    let terrain_at = |t: i64| -> u8 { usize::try_from(t).ok().and_then(|i| terrain.get(i).copied()).unwrap_or(0) };

    // TS: :110-115
    let (min_x, max_x, min_y, max_y) =
        (i64::from(bounds.min_x), i64::from(bounds.max_x), i64::from(bounds.min_y), i64::from(bounds.max_y));
    let bounds_width = max_x - min_x + 1;
    let goal = i64::from(goal);
    let goal_x = goal % map_width;
    let goal_y = goal / map_width;
    let bounds_height = max_y - min_y + 1;
    let num_local_nodes = bounds_width * bounds_height;
    // in-range: a box wider than u32 cannot fit `closedStamp` (checked just below)
    let bw32 = u32::try_from(bounds_width).unwrap_or(u32::MAX);
    let box_in_map = min_x >= 0 && min_y >= 0 && max_x < map_width && bw32 > 0;

    // TS: :117-119
    if num_local_nodes > closed_stamp.len() as i64 {
        return None;
    }

    // TS: :121-129
    let to_local = |tile: i64, clamp: bool| -> i64 {
        let mut x = tile % map_width;
        let mut y = tile / map_width;
        if clamp {
            x = min_x.max(max_x.min(x)); // Math.max(minX, Math.min(maxX, x))
            y = min_y.max(max_y.min(y));
        }
        (y - min_y) * bounds_width + (x - min_x)
    };
    // TS: :131-135 (`local` is in [0, numLocalNodes), so `%` and `| 0` are exact)
    let to_global = |local: i64| -> i64 {
        let local_x = local % bounds_width;
        let local_y = local / bounds_width;
        (local_y + min_y) * map_width + (local_x + min_x)
    };

    // TS: :137-140
    let goal_local = to_local(goal, true);
    if goal_local < 0 || goal_local >= num_local_nodes {
        return None;
    }

    PQueue::clear(queue);
    // TS: :146 `starts[0]`: with no start nothing is queued, so it is never used.
    let &s0 = start.first()?;
    // TS: :146-152
    let s0 = i64::from(s0);
    let start_x = (s0 % map_width) as f64;
    let start_y = (s0 / map_width) as f64;
    let (goal_xf, goal_yf) = (goal_x as f64, goal_y as f64);
    let dx_goal = goal_xf - start_x;
    let dy_goal = goal_yf - start_y;
    let cross_norm = js::max(1.0, js::abs(dx_goal) + js::abs(dy_goal));
    // TS: :155-160
    let cross_tie_breaker = |nx: f64, ny: f64| -> f64 {
        let dx_n = nx - goal_xf;
        let dy_n = ny - goal_yf;
        let cross = js::abs(dx_goal * dy_n - dy_goal * dx_n);
        js::floor((cross * (COST_SCALE - 1.0)) / cross_norm / cross_norm)
    };

    // TS: :162-175
    for &s in start {
        let s = i64::from(s);
        let start_local = to_local(s, true);
        if start_local < 0 || start_local >= num_local_nodes {
            continue;
        }
        let sl = start_local as usize; // in-range: < numLocalNodes <= closedStamp.length
        g_score[sl] = 0;
        g_score_stamp[sl] = stamp;
        came_from[sl] = -1;
        let sx = (s % map_width) as f64;
        let sy = (s / map_width) as f64;
        let h = weight * BASE_COST * (js::abs(sx - goal_xf) + js::abs(sy - goal_yf));
        PQueue::push(queue, start_local as i32, h);
    }

    let mut iterations = i64::from(max_iterations);

    // One neighbour relaxation, TS: :200-225 (and its three copies, :227-306).
    macro_rules! relax {
        ($neighbor:expr, $neighbor_local:expr, $current_local:expr, $current_g:expr, $nx:expr, $ny:expr) => {{
            let neighbor: i64 = $neighbor;
            let nl = $neighbor_local as usize; // in-range: the edge test keeps it inside the box
            let neighbor_terrain = terrain_at(neighbor);
            if closed_stamp[nl] != stamp && (neighbor == goal || (neighbor_terrain & land_mask) == 0) {
                let (nx, ny): (f64, f64) = ($nx, $ny);
                let dist_to_goal = js::abs(nx - goal_xf) + js::abs(ny - goal_yf);
                let magnitude = neighbor_terrain & MAGNITUDE_MASK;
                let cost = BASE_COST + get_magnitude_penalty(magnitude);
                let tentative_g = $current_g + cost;
                if g_score_stamp[nl] != stamp || tentative_g < f64::from(g_score[nl]) {
                    came_from[nl] = $current_local as i32;
                    g_score[nl] = js::to_uint32(tentative_g);
                    g_score_stamp[nl] = stamp;
                    let h = weight * BASE_COST * dist_to_goal;
                    let f = tentative_g + h + cross_tie_breaker(nx, ny);
                    PQueue::push(queue, nl as i32, f);
                }
            }
        }};
    }

    while !PQueue::is_empty(queue) {
        iterations -= 1;
        if iterations <= 0 {
            return None;
        }
        let current_local = i64::from(PQueue::pop(queue));
        let cl = current_local as usize; // in-range: only box-local nodes are queued
        if closed_stamp[cl] == stamp {
            continue;
        }
        closed_stamp[cl] = stamp;
        if current_local == goal_local {
            return Some(build_path(came_from, goal_local, &to_global, num_local_nodes));
        }
        let current_g = f64::from(g_score[cl]);
        // `toGlobal(currentLocal)` then `% mapWidth` / `/ mapWidth`: with `local_x <
        // boundsWidth` and `minX + local_x < mapWidth`, the coordinates are the local ones
        // offset by the box corner (one u32 divmod instead of two i64 pairs; P3 perf).
        let (current, current_x, current_y) = if box_in_map {
            let (local_x, local_y) = ((cl as u32 % bw32) as i64, (cl as u32 / bw32) as i64);
            let (x, y) = (local_x + min_x, local_y + min_y);
            (y * map_width + x, x, y)
        } else {
            let current = to_global(current_local);
            (current, current % map_width, current / map_width)
        };
        let (cxf, cyf) = (current_x as f64, current_y as f64);

        if current_y > min_y {
            relax!(current - map_width, current_local - bounds_width, current_local, current_g, cxf, cyf - 1.0);
        }
        if current_y < max_y {
            relax!(current + map_width, current_local + bounds_width, current_local, current_g, cxf, cyf + 1.0);
        }
        if current_x > min_x {
            relax!(current - 1, current_local - 1, current_local, current_g, cxf - 1.0, cyf);
        }
        if current_x < max_x {
            relax!(current + 1, current_local + 1, current_local, current_g, cxf + 1.0, cyf);
        }
    }
    None
}

/// `buildPath(goalLocal, toGlobal, maxPathLength)` (`:312-330`).
fn build_path(came_from: &[i32], goal_local: i64, to_global: &impl Fn(i64) -> i64, max_path_length: i64) -> Vec<u32> {
    let mut path = Vec::new();
    let mut current = goal_local;
    let mut iterations = 0;
    while current != -1 && iterations < max_path_length {
        path.push(to_global(current) as u32);
        current = i64::from(came_from[current as usize]);
        iterations += 1;
    }
    path.reverse();
    path
}
