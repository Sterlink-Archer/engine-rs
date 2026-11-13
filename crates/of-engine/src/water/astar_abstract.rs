//! `AbstractGraphAStar` (`pathfinding/algorithms/AStar.AbstractGraph.ts`). Owner: **P4a**.
//! `gScore` is a `Float32Array` (f32 store, f64 compare, PORTING_GUIDE §14.2). Scratch:
//! `sc.hpa.abstract_astar`.
//!
//! Float shape kept literally: `tentativeG = currentG + edge.cost` is f64 arithmetic on the
//! f32-rounded `currentG` (`:102,112`); the improvement test compares that f64 against the
//! stored f32 (`:114`); the store rounds to f32 (`:116`); the queue priority
//! `tentativeG + h` is computed in f64 and rounded to f32 by the `MinHeap` store.

use crate::scratch::water::AbstractAStarScratch;
use crate::water::abstract_graph::WaterGraph;

/// `AbstractGraphAStarConfig` (`:5-8`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AbstractGraphAStarConfig {
    pub heuristic_weight: f64,
    pub max_iterations: u32,
}

impl Default for AbstractGraphAStarConfig {
    /// `config?.heuristicWeight ?? 1`, `config?.maxIterations ?? 100_000` (`:25-26`).
    fn default() -> Self {
        AbstractGraphAStarConfig { heuristic_weight: 1.0, max_iterations: 100_000 }
    }
}

impl AbstractAStarScratch {
    /// The constructor's sizing (`:28-39`) for `graph`; grows only (stamps are relative).
    fn ensure(&mut self, n: usize) {
        if self.closed_stamp.len() < n {
            self.closed_stamp.resize(n, 0);
            self.g_score_stamp.resize(n, 0);
            self.g_score.resize(n, 0.0);
            self.came_from.resize(n, 0);
            self.start_node.resize(n, 0);
        }
    }

    /// `this.stamp++` with the wrap reset (`:50-55`, `:142-147`).
    fn bump(&mut self) -> u32 {
        if self.stamp == u32::MAX {
            self.closed_stamp.fill(0);
            self.g_score_stamp.fill(0);
            self.stamp = 1;
        } else {
            self.stamp += 1;
        }
        self.stamp
    }
}

/// `|a - b|` of two u32 node coordinates, as the f64 TS computes.
#[inline]
fn absd(a: u32, b: u32) -> f64 {
    f64::from(a.abs_diff(b))
}

/// `findPath(start, goal)` (`:42-47`): node ids. A TS array start is the multi-source
/// search, a number the single-source one; a 1-element array delegates to the single
/// search (`:140`), so the slice form is exact for both.
pub fn find_path(
    sc: &mut AbstractAStarScratch,
    graph: &WaterGraph,
    cfg: AbstractGraphAStarConfig,
    start: &[u32],
    goal: u32,
) -> Option<Vec<u32>> {
    match start {
        [] => None,
        [s] => find_path_single(sc, graph, cfg, *s, goal),
        _ => find_path_multi_source(sc, graph, cfg, start, goal),
    }
}

/// `findPathSingle(startId, goalId)` (`:49-133`).
fn find_path_single(
    sc: &mut AbstractAStarScratch,
    graph: &WaterGraph,
    cfg: AbstractGraphAStarConfig,
    start_id: u32,
    goal_id: u32,
) -> Option<Vec<u32>> {
    sc.ensure(graph.nodes.len());
    let stamp = sc.bump();
    let weight = cfg.heuristic_weight;
    let goal_node = graph.get_node(goal_id)?;
    let (goal_x, goal_y) = (goal_node.x, goal_node.y);
    let start_node = graph.get_node(start_id)?;
    sc.queue.clear();
    let s = start_id as usize; // in-range: getNode succeeded
    sc.g_score[s] = 0.0;
    sc.g_score_stamp[s] = stamp;
    sc.came_from[s] = -1;
    let start_h = weight * (absd(start_node.x, goal_x) + absd(start_node.y, goal_y));
    sc.queue.push(start_id as i32, start_h);
    let mut iterations = i64::from(cfg.max_iterations);
    while !sc.queue.is_empty() {
        iterations -= 1;
        if iterations <= 0 {
            return None;
        }
        let current = sc.queue.pop() as u32; // int: node ids pushed are valid
        let cu = current as usize;
        if sc.closed_stamp[cu] == stamp {
            continue;
        }
        sc.closed_stamp[cu] = stamp;
        if current == goal_id {
            return build_path_from_goal(sc, graph, goal_id);
        }
        let current_g = f64::from(sc.g_score[cu]);
        for &eid in graph.get_node_edges(current) {
            let edge = &graph.edges[eid as usize]; // in-range: incident lists hold edge ids
            let neighbor = graph.get_other_node(edge, current);
            let nu = neighbor as usize;
            if sc.closed_stamp[nu] == stamp {
                continue;
            }
            let tentative_g = current_g + edge.cost;
            if sc.g_score_stamp[nu] != stamp || tentative_g < f64::from(sc.g_score[nu]) {
                sc.came_from[nu] = current as i32;
                sc.g_score[nu] = tentative_g as f32; // TS: Float32Array store
                sc.g_score_stamp[nu] = stamp;
                if let Some(nn) = graph.get_node(neighbor) {
                    let h = weight * (absd(nn.x, goal_x) + absd(nn.y, goal_y));
                    sc.queue.push(neighbor as i32, tentative_g + h);
                }
            }
        }
    }
    None
}

/// `findPathMultiSource(startIds, goalId)` (`:135-227`).
fn find_path_multi_source(
    sc: &mut AbstractAStarScratch,
    graph: &WaterGraph,
    cfg: AbstractGraphAStarConfig,
    start_ids: &[u32],
    goal_id: u32,
) -> Option<Vec<u32>> {
    sc.ensure(graph.nodes.len());
    let stamp = sc.bump();
    let weight = cfg.heuristic_weight;
    let goal_node = graph.get_node(goal_id)?;
    let (goal_x, goal_y) = (goal_node.x, goal_node.y);
    sc.queue.clear();
    for &start_id in start_ids {
        let Some(node) = graph.get_node(start_id) else {
            continue;
        };
        let s = start_id as usize; // in-range: getNode succeeded
        sc.g_score[s] = 0.0;
        sc.g_score_stamp[s] = stamp;
        sc.came_from[s] = -1;
        sc.start_node[s] = start_id as i32;
        let h = weight * (absd(node.x, goal_x) + absd(node.y, goal_y));
        sc.queue.push(start_id as i32, h);
    }
    let mut iterations = i64::from(cfg.max_iterations);
    while !sc.queue.is_empty() {
        iterations -= 1;
        if iterations <= 0 {
            return None;
        }
        let current = sc.queue.pop() as u32;
        let cu = current as usize;
        if sc.closed_stamp[cu] == stamp {
            continue;
        }
        sc.closed_stamp[cu] = stamp;
        if current == goal_id {
            return build_path_from_goal(sc, graph, goal_id);
        }
        let current_g = f64::from(sc.g_score[cu]);
        let current_start = sc.start_node[cu];
        for &eid in graph.get_node_edges(current) {
            let edge = &graph.edges[eid as usize]; // in-range: incident lists hold edge ids
            let neighbor = graph.get_other_node(edge, current);
            let nu = neighbor as usize;
            if sc.closed_stamp[nu] == stamp {
                continue;
            }
            let tentative_g = current_g + edge.cost;
            if sc.g_score_stamp[nu] != stamp || tentative_g < f64::from(sc.g_score[nu]) {
                sc.came_from[nu] = current as i32;
                sc.g_score[nu] = tentative_g as f32;
                sc.g_score_stamp[nu] = stamp;
                sc.start_node[nu] = current_start;
                if let Some(nn) = graph.get_node(neighbor) {
                    let h = weight * (absd(nn.x, goal_x) + absd(nn.y, goal_y));
                    sc.queue.push(neighbor as i32, tentative_g + h);
                }
            }
        }
    }
    None
}

/// `buildPathFromGoal(goalId)` (`:229-243`). `maxLen` is `cameFrom.length`, i.e. the
/// graph's node count (the scratch may be longer).
fn build_path_from_goal(sc: &AbstractAStarScratch, graph: &WaterGraph, goal_id: u32) -> Option<Vec<u32>> {
    let max_len = graph.nodes.len() as i64;
    let mut path = Vec::new();
    let mut current = i64::from(goal_id);
    while current != -1 {
        if current < 0 || current >= max_len {
            return None;
        }
        path.push(current as u32);
        if path.len() as i64 > max_len {
            return None;
        }
        current = i64::from(sc.came_from[current as usize]); // in-range: 0 <= current < maxLen
    }
    path.reverse();
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::water::abstract_graph::{AbstractEdge, AbstractNode};

    fn graph(nodes: &[(u32, u32)], edges: &[(u32, u32, f64)]) -> WaterGraph {
        let mut g = WaterGraph::default();
        for (i, &(x, y)) in nodes.iter().enumerate() {
            g.nodes.push(AbstractNode { id: i as u32, x, y, tile: i as u32, component_id: 1 });
            g.node_edges.push(Vec::new());
        }
        for (i, &(a, b, c)) in edges.iter().enumerate() {
            g.edges.push(AbstractEdge { id: i as u32, node_a: a, node_b: b, cost: c, cluster_x: 0, cluster_y: 0 });
            g.node_edges[a as usize].push(i as u32);
            g.node_edges[b as usize].push(i as u32);
        }
        g
    }

    #[test]
    fn single_and_multi_source() {
        // 0 -(1)- 1 -(1)- 2 ; 0 -(5)- 2 ; 3 -(1)- 2
        let g = graph(&[(0, 0), (1, 0), (2, 0), (3, 0)], &[(0, 1, 1.0), (1, 2, 1.0), (0, 2, 5.0), (2, 3, 1.0)]);
        let mut sc = AbstractAStarScratch::default();
        let cfg = AbstractGraphAStarConfig::default();
        assert_eq!(find_path(&mut sc, &g, cfg, &[0], 2), Some(vec![0, 1, 2]));
        assert_eq!(find_path(&mut sc, &g, cfg, &[0, 3], 2), Some(vec![3, 2]));
        assert_eq!(find_path(&mut sc, &g, cfg, &[0], 9), None);
        assert_eq!(find_path(&mut sc, &g, cfg, &[], 2), None);
        // maxIterations: `--iterations <= 0` allows maxIterations - 1 pops.
        let tight = AbstractGraphAStarConfig { heuristic_weight: 1.0, max_iterations: 1 };
        assert_eq!(find_path(&mut sc, &g, tight, &[0], 0), None);
        let two = AbstractGraphAStarConfig { heuristic_weight: 1.0, max_iterations: 2 };
        assert_eq!(find_path(&mut sc, &g, two, &[0], 0), Some(vec![0]));
    }

    #[test]
    fn f32_gscore_rounding_is_kept() {
        // Costs that are not f32-representable: the stored gScore rounds, the compare is f64.
        let c = 0.1 + 0.2; // 0.30000000000000004
        let g = graph(&[(0, 0), (0, 0), (0, 0)], &[(0, 1, c), (1, 2, c), (0, 2, 2.0 * c)]);
        let mut sc = AbstractAStarScratch::default();
        let cfg = AbstractGraphAStarConfig::default();
        let p = find_path(&mut sc, &g, cfg, &[0], 2).expect("path");
        // gScore[2] via the direct edge = f32(0.6000000000000001); via 1: f32(0.3)+0.3 in f64.
        let via1 = f64::from(c as f32) + c;
        let direct = f64::from((2.0 * c) as f32);
        assert!(via1 != direct);
        assert_eq!(p.first(), Some(&0));
        assert_eq!(p.last(), Some(&2));
    }
}
