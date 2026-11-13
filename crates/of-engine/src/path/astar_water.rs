//! `AStarWater` (`pathfinding/algorithms/AStar.Water.ts`). Owner: **P4b**. Scratch: `sc.astar_water`.
//!
//! The inlined 4-neighbour A* of the simple water chain (`PathFinder.ts:52-58`). Arithmetic
//! is JS `number` (f64) exactly as written; `gScore` is a `Uint32Array` (compare as number,
//! store `ToUint32`); priorities go into P4a's `MinHeap` (`Float32Array`).

use crate::map::GameMap;
use crate::path::MapRead;
use crate::path::astar::PQueue;
use crate::scratch::path::AStarWaterScratch;
use of_det::js;

const LAND_BIT: u32 = 7; // TS: :5
const MAGNITUDE_MASK: u8 = 0x1f; // TS: :6
const COST_SCALE: f64 = 100.0; // TS: :7
const BASE_COST: f64 = 1.0 * COST_SCALE; // TS: :8

/// `getMagnitudePenalty` (`:11-15`).
fn get_magnitude_penalty(magnitude: u8) -> f64 {
    if magnitude < 3 {
        return 10.0 * COST_SCALE;
    }
    if magnitude <= 10 {
        return 0.0;
    }
    1.0 * COST_SCALE
}

/// `AStarWaterConfig` (`:17-20`).
#[derive(Clone, Copy, Debug, Default)]
pub struct AStarWaterConfig {
    pub heuristic_weight: Option<f64>,
    pub max_iterations: Option<u32>,
}

/// `findPath(start, goal)` (`:51-239`) on `map` (the minimap).
pub fn find_path(
    sc: &mut AStarWaterScratch,
    map: &GameMap,
    cfg: AStarWaterConfig,
    start: &[u32],
    goal: u32,
) -> Option<Vec<u32>> {
    find_path_m(sc, map, cfg, start, goal)
}

/// [`find_path`] over any [`MapRead`].
pub fn find_path_m<M: MapRead>(
    sc: &mut AStarWaterScratch,
    map: &M,
    cfg: AStarWaterConfig,
    start: &[u32],
    goal: u32,
) -> Option<Vec<u32>> {
    // TS: :36-46 (constructor)
    let width = map.width();
    let num_nodes = width * map.height();
    let heuristic_weight = cfg.heuristic_weight.unwrap_or(5.0);
    let max_iterations = cfg.max_iterations.unwrap_or(1_000_000);
    sc.ensure(num_nodes as usize);

    // TS: :52-57
    if sc.stamp == u32::MAX {
        sc.closed_stamp.fill(0);
        sc.g_score_stamp.fill(0);
        sc.stamp = 1;
    } else {
        sc.stamp += 1;
    }
    let stamp = sc.stamp;
    let terrain = map.terrain();
    let AStarWaterScratch { closed_stamp, g_score_stamp, g_score, came_from, queue, .. } = sc;
    let queue = queue.as_mut().expect("rust-invariant: ensure() built the queue");
    let weight = heuristic_weight;
    let land_mask: u8 = 1 << LAND_BIT;
    let wf = f64::from(width);

    let goal_x = f64::from(goal % width);
    let goal_y = f64::from(goal / width); // TS: `(goal / width) | 0`, non-negative

    PQueue::clear(queue);
    // TS: :74-78 (`starts[0]`). An empty array reads `undefined` there, but then nothing is
    // queued and the search returns null before the tie-breaker runs, so it is never used.
    let &s0 = start.first()?;

    // For cross-product tie-breaker (prefer diagonal paths) — TS: :77-84
    let start_x = f64::from(s0 % width);
    let start_y = f64::from(s0 / width);
    let dx_goal = goal_x - start_x;
    let dy_goal = goal_y - start_y;
    let cross_norm = js::max(1.0, js::abs(dx_goal) + js::abs(dy_goal));
    // TS: :87-92
    let cross_tie_breaker = |nx: f64, ny: f64| -> f64 {
        let dx_n = nx - goal_x;
        let dy_n = ny - goal_y;
        let cross = js::abs(dx_goal * dy_n - dy_goal * dx_n);
        js::floor((cross * (COST_SCALE - 1.0)) / cross_norm / cross_norm)
    };

    // TS: :94-103
    for &s in start {
        let si = s as usize; // in-range: sources are minimap tiles
        g_score[si] = 0;
        g_score_stamp[si] = stamp;
        came_from[si] = -1;
        let sx = f64::from(s % width);
        let sy = f64::from(s / width);
        let h = weight * BASE_COST * (js::abs(sx - goal_x) + js::abs(sy - goal_y));
        PQueue::push(queue, s as i32, h);
    }

    let mut iterations = i64::from(max_iterations);

    // One neighbour relaxation, TS: :125-151 (and its three copies, :153-235).
    macro_rules! relax {
        ($neighbor:expr, $current:expr, $current_g:expr, $nx:expr, $ny:expr) => {{
            let neighbor: u32 = $neighbor;
            let ni = neighbor as usize; // in-range: guarded by the edge tests above each use
            let neighbor_terrain = terrain[ni];
            if closed_stamp[ni] != stamp && (neighbor == goal || (neighbor_terrain & land_mask) == 0) {
                let magnitude = neighbor_terrain & MAGNITUDE_MASK;
                let cost = BASE_COST + get_magnitude_penalty(magnitude);
                let tentative_g = $current_g + cost;
                if g_score_stamp[ni] != stamp || tentative_g < f64::from(g_score[ni]) {
                    came_from[ni] = $current as i32;
                    g_score[ni] = js::to_uint32(tentative_g);
                    g_score_stamp[ni] = stamp;
                    let (nx, ny): (f64, f64) = ($nx, $ny);
                    let h = weight * BASE_COST * (js::abs(nx - goal_x) + js::abs(ny - goal_y));
                    let f = tentative_g + h + cross_tie_breaker(nx, ny);
                    PQueue::push(queue, neighbor as i32, f);
                }
            }
        }};
    }

    while !PQueue::is_empty(queue) {
        iterations -= 1;
        if iterations <= 0 {
            return None;
        }
        let current = PQueue::pop(queue) as u32;
        let ci = current as usize;
        if closed_stamp[ci] == stamp {
            continue;
        }
        closed_stamp[ci] = stamp;
        if current == goal {
            return Some(build_path(came_from, goal));
        }
        let current_g = f64::from(g_score[ci]);
        let current_x = f64::from(current % width);
        let current_y = f64::from(current / width);

        if current >= width {
            relax!(current - width, current, current_g, current_x, current_y - 1.0);
        }
        if current < num_nodes - width {
            relax!(current + width, current, current_g, current_x, current_y + 1.0);
        }
        if current_x != 0.0 {
            relax!(current - 1, current, current_g, current_x - 1.0, current_y);
        }
        if current_x != wf - 1.0 {
            relax!(current + 1, current, current_g, current_x + 1.0, current_y);
        }
    }
    None
}

/// `buildPath(goal)` (`:241-252`).
fn build_path(came_from: &[i32], goal: u32) -> Vec<u32> {
    let mut path = Vec::new();
    let mut current = goal as i32;
    while current != -1 {
        path.push(current as u32);
        current = came_from[current as usize];
    }
    path.reverse();
    path
}
