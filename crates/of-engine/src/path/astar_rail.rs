//! `AStarRail` (`pathfinding/algorithms/AStar.Rail.ts`). Owner: **P4b**. Scratch: `sc.astar_rail`.
//!
//! The generic [`crate::path::astar`] over `RailAdapter` on the minimap. TS builds one engine
//! per game (`_railChainCache`, `PathFinder.ts:171-183`); its adapter "reads the minimap live
//! and caches only its dimensions", which is what reading `map` per call does.

use crate::map::GameMap;
use crate::path::MapRead;
use crate::path::astar::{self, AStarAdapter, AStarConfig};
use crate::scratch::path::AStarRailScratch;

/// `findPath(from, to)` (`:14-18`) on `map` (the minimap).
pub fn find_path(sc: &mut AStarRailScratch, map: &GameMap, from: &[u32], to: u32) -> Option<Vec<u32>> {
    find_path_m(sc, map, from, to)
}

/// [`find_path`] over any [`MapRead`].
pub fn find_path_m<M: MapRead>(sc: &mut AStarRailScratch, map: &M, from: &[u32], to: u32) -> Option<Vec<u32>> {
    let adapter = RailAdapter::new(map);
    astar::find_path(&mut sc.astar, &adapter, AStarConfig::default(), from, to)
}

/// `RailAdapter` (`:22-114`).
struct RailAdapter<'m, M: MapRead> {
    game_map: &'m M,
    width: u32,
    height: u32,
    num_nodes: u32,
}

const WATER_PENALTY: f64 = 5.0; // TS: :27
const HEURISTIC_WEIGHT: f64 = 2.0; // TS: :28
const DIRECTION_CHANGE_PENALTY: f64 = 3.0; // TS: :29

impl<'m, M: MapRead> RailAdapter<'m, M> {
    fn new(game_map: &'m M) -> Self {
        let width = game_map.width();
        let height = game_map.height();
        RailAdapter { game_map, width, height, num_nodes: width * height }
    }

    /// `isTraversable(to, fromShoreline, isImpassable)` (`:81-90`).
    fn is_traversable(&self, to: u32, from_shoreline: bool, is_impassable: bool) -> bool {
        if is_impassable {
            return false;
        }
        let to_water = self.game_map.is_water(to);
        if !to_water {
            return true;
        }
        from_shoreline || self.game_map.is_shoreline(to)
    }
}

impl<M: MapRead> AStarAdapter for RailAdapter<'_, M> {
    fn num_nodes(&self) -> u32 {
        self.num_nodes
    }
    fn max_neighbors(&self) -> u32 {
        4
    }
    /// `:46-49`: `heuristicWeight * (width + height) * maxCost` (= 18 (w + h), integral).
    fn max_priority(&self) -> u32 {
        let max_cost = 1.0 + WATER_PENALTY + DIRECTION_CHANGE_PENALTY;
        (HEURISTIC_WEIGHT * f64::from(self.width + self.height) * max_cost) as u32
    }
    /// `:51-79`.
    fn neighbors(&self, node: u32, buffer: &mut [i32]) -> usize {
        let mut count = 0;
        let x = node % self.width;
        let from_shoreline = self.game_map.is_shoreline(node);
        let is_impassable = self.game_map.is_impassable(node);
        if node >= self.width {
            let n = node - self.width;
            if self.is_traversable(n, from_shoreline, is_impassable) {
                buffer[count] = n as i32;
                count += 1;
            }
        }
        if node < self.num_nodes - self.width {
            let n = node + self.width;
            if self.is_traversable(n, from_shoreline, is_impassable) {
                buffer[count] = n as i32;
                count += 1;
            }
        }
        if x != 0 {
            let n = node - 1;
            if self.is_traversable(n, from_shoreline, is_impassable) {
                buffer[count] = n as i32;
                count += 1;
            }
        }
        if x != self.width - 1 {
            let n = node + 1;
            if self.is_traversable(n, from_shoreline, is_impassable) {
                buffer[count] = n as i32;
                count += 1;
            }
        }
        count
    }
    /// `:92-105`. `d1`/`d2` are JS number differences of refs (may be negative).
    fn cost(&self, from: u32, to: u32, prev: Option<u32>) -> f64 {
        let penalized = self.game_map.is_water(to) || self.game_map.is_shoreline(to);
        let mut c = if penalized { 1.0 + WATER_PENALTY } else { 1.0 };
        if let Some(prev) = prev {
            let d1 = i64::from(from) - i64::from(prev);
            let d2 = i64::from(to) - i64::from(from);
            if d1 != d2 {
                c += DIRECTION_CHANGE_PENALTY;
            }
        }
        c
    }
    /// `:107-113`.
    fn heuristic(&self, node: u32, goal: u32) -> f64 {
        let nx = node % self.width;
        let ny = node / self.width;
        let gx = goal % self.width;
        let gy = goal / self.width;
        HEURISTIC_WEIGHT * f64::from(nx.abs_diff(gx) + ny.abs_diff(gy))
    }
}
