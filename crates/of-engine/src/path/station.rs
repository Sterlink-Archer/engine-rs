//! `StationPathFinder` (`pathfinding/PathFinder.Station.ts`). Owner: **P4b**. Scratch:
//! `sc.astar_station`.
//!
//! TS builds a fresh `StationPathFinder` (and generic `AStar`) per query
//! (`RailNetworkImpl.ts:287-289`), sized to `manager.count()` = `nextId` at that moment, so
//! every station id is in range. `StationGraphAdapter.neighbors` writes into an
//! `Int32Array(32)`: a station with more than 32 railroads overflows it, which the generic
//! A* ports literally (`path::astar` module doc).

use crate::ctx::Rcx;
use crate::ids::{StationId, TileRef};
use crate::path::MapRead;
use crate::path::astar::{self, AStarAdapter, AStarConfig};
use crate::state::State;

/// `findPath(from, to)` (`:17-33`) over the station graph.
pub fn find_path(rcx: &mut Rcx, from: &[StationId], to: StationId) -> Option<Vec<StationId>> {
    let st: &State = rcx.st;
    let to_cluster = st.station(to).get_cluster();
    // `fromArray.filter((s) => s.getCluster() === toCluster)` (null === null included)
    let same_cluster: Vec<u32> =
        from.iter().copied().filter(|&s| st.station(s).get_cluster() == to_cluster).map(|s| s.0).collect();
    if same_cluster.is_empty() {
        return None;
    }
    let adapter = StationGraphAdapter { st };
    let path =
        astar::find_path(&mut rcx.sc.astar_station.astar, &adapter, AStarConfig::default(), &same_cluster, to.0)?;
    // `path.map((id) => this.manager.getById(id)!)`: the ids, unchanged (`!` has no runtime
    // effect in TS; a missing station would surface where it is used).
    Some(path.into_iter().map(StationId).collect())
}

/// `StationGraphAdapter` (`:36-78`).
struct StationGraphAdapter<'s> {
    st: &'s State,
}

impl StationGraphAdapter<'_> {
    fn tile(&self, id: u32) -> Option<TileRef> {
        self.st.rail.station_manager.get(StationId(id)).map(|_| self.st.station_tile(StationId(id)))
    }
}

impl AStarAdapter for StationGraphAdapter<'_> {
    /// `manager.count()` (= `nextId`, `RailNetworkImpl.ts:82-84`).
    fn num_nodes(&self) -> u32 {
        self.st.rail.station_manager.count() as u32
    }
    fn max_neighbors(&self) -> u32 {
        32
    }
    /// `game.map().width() + game.map().height()` (`:50-52`).
    fn max_priority(&self) -> u32 {
        MapRead::width(&self.st.map) + MapRead::height(&self.st.map)
    }
    /// `:54-63`: `buffer[count++] = n.id` for every neighbour (writes past 32 are dropped).
    fn neighbors(&self, node: u32, buffer: &mut [i32]) -> usize {
        if self.st.rail.station_manager.get(StationId(node)).is_none() {
            return 0;
        }
        let mut count = 0;
        for n in self.st.station_neighbors(StationId(node)) {
            if let Some(slot) = buffer.get_mut(count) {
                *slot = n.0 as i32;
            }
            count += 1;
        }
        count
    }
    /// `cost()` (`:65-67`).
    fn cost(&self, _: u32, _: u32, _: Option<u32>) -> f64 {
        1.0
    }
    /// `:69-77`.
    fn heuristic(&self, node: u32, goal: u32) -> f64 {
        let (Some(a), Some(b)) = (self.tile(node), self.tile(goal)) else {
            return 0.0;
        };
        let map = &self.st.map;
        let ax = f64::from(MapRead::x(map, a));
        let ay = f64::from(MapRead::y(map, a));
        let bx = f64::from(MapRead::x(map, b));
        let by = f64::from(MapRead::y(map, b));
        of_det::js::abs(ax - bx) + of_det::js::abs(ay - by)
    }
    /// `cost(from, undefined)` is still 1.
    fn cost_to_undefined(&self, _: u32, _: Option<u32>) -> f64 {
        1.0
    }
    /// `heuristic(undefined, goal)`: `getById(undefined)` is undefined → 0.
    fn heuristic_undefined(&self, _: u32) -> f64 {
        0.0
    }
}
