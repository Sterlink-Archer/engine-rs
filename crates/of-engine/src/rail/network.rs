//! `RailNetworkImpl` + `StationManagerImpl` (`game/RailNetworkImpl.ts`). Owner: **P5**.
//!
//! The ghost-rail preview (`computeGhostRailPaths`, `GhostRailNetworkState`, `:150-296`,
//! `:463-766`) is skipped (UI only, ARCHITECTURE §3.9). `networkVersion` is kept although it
//! only keys that preview memo (derived, not in the snapshot).
//!
//! Every algorithm is a generic `*_env` function over the rail env (`rail::env`), called by the
//! frozen `Ctx`/`State` methods at the bottom.

use crate::StateClone;
use crate::api::types::UnitType;
use crate::arena::Arena;
use crate::ctx::{Ctx, TsExpect};
use crate::ids::{ClusterRef, RailroadId, RailroadRef, StationId, TileRef, UnitKey, UnitRef};
use crate::jsorder::{JsMap, JsSet, LookupMap, LookupSet, SmallJsSet};
use crate::rail::env::{RailEnv, RailRead};
use crate::rail::grid::RailSpatialGrid;
use crate::rail::railroad::{Railroad, closest_tile_index_env, railroad_delete_env};
use crate::rail::station::{
    Cluster, TrainStation, cluster_add_station_env, cluster_add_stations_env, cluster_merge_env,
    station_add_railroad_env, station_neighbors_env, station_remove_railroad_env, station_set_cluster_env,
    station_tile_env,
};
use crate::state::State;
use std::collections::VecDeque;

/// `maxConnectionDistance` (`:299`).
const MAX_CONNECTION_DISTANCE: u32 = 4;
/// `stationRadius` (`:300`).
const STATION_RADIUS: u32 = 3;
/// `gridCellSize` (`:301`): 4x4 tiles spatial grid.
const GRID_CELL_SIZE: u32 = 4;

/// `StationManagerImpl` (`:35-115`); snapshot `stationManager { stations, nextId }`.
#[derive(StateClone, Debug, Default)]
pub struct StationManager {
    /// `stations: Set<TrainStation>` (`:36`).
    pub stations: JsSet<StationId>,
    /// `nextId` (from 1, `:42-48`).
    pub next_id: u32,
    /// non-snapshot: the station objects by id (slot = id; slot 0 never used). TS
    /// `stationsById[]` (`:37`) is cleared on removal, but TS executions keep the removed
    /// station object and keep reading it; so this store keeps every station ever added, and
    /// TS `stationsById[id]` is [`StationManager::get`] (present iff the id is in `stations`:
    /// both are written by the same `addStation`/`removeStation` statements). GC-only on the
    /// walk: the canon pass reaches stations through `stations` and the references to them.
    #[refs(gc_only)]
    pub by_id: Vec<Option<TrainStation>>,
    /// non-snapshot: `stationsByUnit: Map<Unit, TrainStation>` (rebuilt in `stations` order on
    /// removal, `:53-68`).
    #[refs(skip)]
    pub by_unit: JsMap<UnitKey, StationId>,
    /// non-snapshot: `station.unit` as a map key (TS `unit.id()`) by station id, so the
    /// `stationsByUnit` rebuild needs no `State` (P5 extension field).
    #[refs(skip)]
    pub unit_key_by_id: Vec<UnitKey>,
}

impl StationManager {
    /// `new StationManagerImpl()`.
    pub fn new() -> Self {
        StationManager {
            stations: JsSet::default(),
            // Start from 1; 0 is reserved as invalid/sentinel (TS: :42)
            next_id: 1,
            by_id: Vec::new(),
            by_unit: JsMap::default(),
            unit_key_by_id: Vec::new(),
        }
    }

    /// `addStation(station)` (`:44-51`): assigns the id.
    pub fn add_station(&mut self, unit_key: UnitKey, mut station: TrainStation) -> StationId {
        // TS: station.id = this.nextId++
        let id = StationId(self.next_id);
        self.next_id += 1;
        station.id = id;
        let slot = id.0 as usize; // int: ids are small (one per structure)
        if self.by_id.len() <= slot {
            self.by_id.resize_with(slot + 1, || None);
            self.unit_key_by_id.resize(slot + 1, UnitKey(0));
        }
        self.by_id[slot] = Some(station); // in-range: resized above
        self.unit_key_by_id[slot] = unit_key; // in-range: resized above
        self.stations.insert(id);
        if !self.by_unit.contains_key(&unit_key) {
            self.by_unit.insert(unit_key, id);
        }
        id
    }

    /// `removeStation(station)` (`:53-68`).
    pub fn remove_station(&mut self, s: StationId) {
        // `stationsById[id] = undefined` + trailing-hole pops: `get` reads `stations` (above).
        self.stations.remove(&s);
        let unit_key = self.unit_key(s);
        if self.by_unit.get(&unit_key) == Some(&s) {
            // The unit's next station, if any, takes over. Rebuild rather than
            // re-insert so the Map keeps the order restoreSnapshot produces.
            self.rebuild_stations_by_unit();
        }
    }

    /// `rebuildStationsByUnit()` (`:107-114`): the first station of each unit, in `stations` order.
    fn rebuild_stations_by_unit(&mut self) {
        let mut by_unit: JsMap<UnitKey, StationId> = JsMap::default();
        for &station in self.stations.iter() {
            let k = self.unit_key(station);
            if !by_unit.contains_key(&k) {
                by_unit.insert(k, station);
            }
        }
        self.by_unit = by_unit;
    }

    /// The `unit.id()` of station `s`'s unit.
    #[inline]
    fn unit_key(&self, s: StationId) -> UnitKey {
        // in-range: every StationId was assigned by add_station, which sized this table.
        self.unit_key_by_id[s.0 as usize]
    }

    /// `findStation(unit)` (`:70-72`).
    pub fn find_station(&self, unit_key: UnitKey) -> Option<StationId> {
        self.by_unit.get(&unit_key).copied()
    }
    /// `getAll()` (`:74-76`).
    pub fn get_all(&self) -> &JsSet<StationId> {
        &self.stations
    }
    /// `getById(id)` (`:78-80`): `undefined` for an id never assigned or already removed.
    pub fn get(&self, id: StationId) -> Option<&TrainStation> {
        if self.stations.contains(&id) { self.obj(id) } else { None }
    }
    /// `getById(id)` for writes (same presence rule as [`Self::get`]).
    pub fn get_mut(&mut self, id: StationId) -> Option<&mut TrainStation> {
        if self.stations.contains(&id) { self.by_id.get_mut(id.0 as usize).and_then(Option::as_mut) } else { None }
    }
    /// `count()` (`:82-84`): TS returns `nextId` (the A* node count of the station graph).
    pub fn count(&self) -> usize {
        self.next_id as usize
    }
    /// The station object with id `id`, removed or not (TS code holds the object).
    #[inline]
    pub fn obj(&self, id: StationId) -> Option<&TrainStation> {
        self.by_id.get(id.0 as usize).and_then(Option::as_ref)
    }
    #[inline]
    pub fn obj_mut(&mut self, id: StationId) -> Option<&mut TrainStation> {
        self.by_id.get_mut(id.0 as usize).and_then(Option::as_mut)
    }
}

/// `RailNetworkImpl` data, `RailNetwork v1` snapshot order, then non-snapshot.
#[derive(StateClone, Debug, Default)]
pub struct RailNetwork {
    pub station_manager: StationManager,
    /// `nextId` (railroads, `:303`).
    pub next_id: u32,
    /// `dirtyClusters: Set<Cluster>` (`:304`).
    pub dirty_clusters: JsSet<ClusterRef>,
    /// `railGrid`.
    pub rail_grid: RailSpatialGrid,
    /// non-snapshot: railroad storage (GC-swept with their tiles) and id → slot.
    pub railroads: Arena<Railroad, RailroadRef>,
    /// non-snapshot: railroad id → slot (lookup-only: `railroad(id)` resolution) for every
    /// railroad still attached to a station set or the grid; a detached railroad leaves it
    /// (`retire_railroad_if_detached`) and is then kept alive only by `RailroadRef`s
    /// (`TrainExecution.currentRailroad`).
    pub railroad_by_id: LookupMap<RailroadId, RailroadRef>,
    /// non-snapshot: clusters (no TS id).
    pub clusters: Arena<Cluster, ClusterRef>,
    /// non-snapshot, derived: `networkVersion` (`:307`).
    pub network_version: u32,
}

impl RailNetwork {
    /// `createRailNetwork(game)` (`:292-296`): a `StationManagerImpl`, the rail path service
    /// (the `RailEnv` path methods) and `new RailSpatialGrid(game, 4)` (`:317`).
    pub fn new() -> Self {
        RailNetwork {
            station_manager: StationManager::new(),
            next_id: 0,
            dirty_clusters: JsSet::default(),
            rail_grid: RailSpatialGrid::new(GRID_CELL_SIZE),
            railroads: Arena::new(),
            railroad_by_id: LookupMap::new(),
            clusters: Arena::new(),
            network_version: 0,
        }
    }

    /// The attached railroad with TS id `id`.
    pub fn railroad(&self, id: RailroadId) -> &Railroad {
        self.railroads.get(self.railroad_ref(id))
    }
    /// The arena handle of attached railroad `id`.
    #[inline]
    pub fn railroad_ref(&self, id: RailroadId) -> RailroadRef {
        *self.railroad_by_id.get(&id).ts_expect("Railroad lookup by id")
    }
    /// The station object with id `s` (see [`StationManager::obj`]).
    #[inline]
    pub fn station_obj(&self, s: StationId) -> &TrainStation {
        self.station_manager.obj(s).ts_expect("TrainStation lookup by id")
    }
    #[inline]
    pub fn station_obj_mut(&mut self, s: StationId) -> &mut TrainStation {
        self.station_manager.obj_mut(s).ts_expect("TrainStation lookup by id")
    }

    /// `station.setCluster(cluster)` (`TrainStation.ts:149-155`), including the removal from
    /// the previous cluster.
    pub fn set_station_cluster(&mut self, s: StationId, cluster: Option<ClusterRef>) {
        if let Some(old) = self.station_obj(s).cluster
            && Some(old) != cluster
        {
            self.clusters.get_mut(old).remove_station(s);
        }
        self.station_obj_mut(s).cluster = cluster;
    }

    /// `new Railroad(from, to, tiles, this.nextId++)`.
    pub(crate) fn new_railroad(&mut self, from: StationId, to: StationId, tiles: Vec<TileRef>) -> RailroadId {
        let id = RailroadId(self.next_id);
        self.next_id += 1;
        let r = self.railroads.alloc(Railroad::new(from, to, tiles, id));
        self.railroad_by_id.insert(id, r);
        id
    }

    /// `this.networkVersion++` (derived: keys only the skipped preview memo).
    #[inline]
    pub(crate) fn bump_network_version(&mut self) {
        self.network_version = self.network_version.wrapping_add(1);
    }

    /// `new Cluster()`.
    pub(crate) fn new_cluster(&mut self) -> ClusterRef {
        self.clusters.alloc(Cluster::default())
    }

    /// Bookkeeping only (no TS statement): once no station set and no grid entry names
    /// railroad `r`, drop it from `railroad_by_id` and mark it deleted, so the GC can free it
    /// when no execution holds it either. TS reaches a railroad only through those sets and
    /// through `OrientedRailroad`s, so this changes nothing observable.
    pub(crate) fn retire_railroad_if_detached(&mut self, r: RailroadId) {
        let Some(&h) = self.railroad_by_id.get(&r) else { return };
        let (from, to) = {
            let rr = self.railroads.get(h);
            (rr.from, rr.to)
        };
        let named = |s: StationId| self.station_manager.obj(s).is_some_and(|st| st.railroads.contains(&r));
        if named(from) || named(to) || self.rail_grid.rail_to_cells.contains_key(&r) {
            return;
        }
        self.railroad_by_id.remove(&r);
        self.railroads.get_mut(h).deleted = true;
    }
}

// ---- RailNetworkImpl, generic bodies ----

/// `rail.tiles` copy plus `from`/`to` of attached railroad `r`.
fn railroad_parts<E: RailRead + ?Sized>(e: &E, r: RailroadId) -> (StationId, StationId, Vec<TileRef>) {
    let rr = e.rail().railroad(r);
    (rr.from, rr.to, rr.tiles.clone())
}

/// `railGrid.register(rail)` with the env's `x/y`.
fn grid_register<E: RailEnv + ?Sized>(e: &mut E, r: RailroadId) {
    let coords: Vec<(u32, u32)> = e.rail().railroad(r).tiles.iter().map(|&t| (e.tile_x(t), e.tile_y(t))).collect();
    e.rail_mut().rail_grid.register_coords(r, &coords);
}

/// `railGrid.query(tile, radius)`.
fn grid_query<E: RailRead + ?Sized>(e: &E, tile: TileRef, radius: u32) -> Vec<RailroadId> {
    e.rail().rail_grid.query_xy(e.tile_x(tile), e.tile_y(tile), radius)
}

/// `connectStation(station)` (`:324-330`) after its `addStation` (see [`Ctx::rail_connect_station`]).
pub(crate) fn connect_station_env<E: RailEnv + ?Sized>(e: &mut E, s: StationId) {
    e.rail_mut().bump_network_version();
    if !connect_to_existing_rails_env(e, s) {
        connect_to_nearby_stations_env(e, s);
    }
}

/// `recomputeClusters()` (`:332-355`).
pub(crate) fn recompute_clusters_env<E: RailEnv + ?Sized>(e: &mut E) {
    if e.rail().dirty_clusters.is_empty() {
        return;
    }
    e.rail_mut().bump_network_version();

    // iter-mut: the body never touches `dirtyClusters` (live cursor anyway).
    let mut cur = e.rail().dirty_clusters.cursor();
    while let Some(cluster) = e.rail().dirty_clusters.next_live(&mut cur) {
        // TS: new Set(cluster.stations)
        let mut all_original_stations: JsSet<StationId> =
            e.rail().clusters.get(cluster).stations.iter().copied().collect();
        while !all_original_stations.is_empty() {
            // rust-invariant: the set is non-empty (loop condition).
            #[allow(clippy::expect_used)]
            let next_station = *all_original_stations.first_live().expect("non-empty set");
            let all_connected_stations = compute_cluster_env(e, next_station);
            // Filter stations that are connected to the current cluster
            for connected_station in all_connected_stations.iter() {
                all_original_stations.remove(connected_station);
            }
            // Those stations were disconnected: new cluster
            if !all_original_stations.is_empty() {
                let new_cluster = e.rail_mut().new_cluster();
                // Switching their cluster will automatically remove them from their current cluster
                let list: Vec<StationId> = all_connected_stations.iter().copied().collect();
                cluster_add_stations_env(e, new_cluster, &list);
            }
        }
    }
    e.rail_mut().dirty_clusters.clear();
}

/// `removeStation(unit)` (`:357-377`).
pub(crate) fn remove_station_env<E: RailEnv + ?Sized>(e: &mut E, unit: UnitRef) {
    let key = UnitKey(e.unit_id(unit));
    let Some(station) = e.rail().station_manager.find_station(key) else { return };
    e.rail_mut().bump_network_version();

    disconnect_from_network_env(e, station);
    e.rail_mut().station_manager.remove_station(station);
    let station_unit = e.rail().station_obj(station).unit;
    e.unit_set_train_station(station_unit, false);

    let Some(cluster) = e.rail().station_obj(station).cluster else { return };

    e.rail_mut().clusters.get_mut(cluster).remove_station(station);
    if e.rail().clusters.get(cluster).size() == 0 {
        delete_cluster_env(e, cluster);
        e.rail_mut().dirty_clusters.remove(&cluster);
        return;
    }

    e.rail_mut().dirty_clusters.insert(cluster);
}

/// `connectToExistingRails(station)` (`:386-448`).
fn connect_to_existing_rails_env<E: RailEnv + ?Sized>(e: &mut E, station: StationId) -> bool {
    let station_tile = station_tile_env(e, station);
    let rails = grid_query(e, station_tile, STATION_RADIUS);

    // `Set<Cluster>`: insertion order decides the merge order.
    let mut edited_clusters: SmallJsSet<ClusterRef, 4> = SmallJsSet::default();
    for rail in rails {
        let (from, to, tiles) = railroad_parts(e, rail);
        let closest_rail_index = closest_tile_index_env(e, &tiles, station_tile);
        // int: closest_rail_index ∈ [-1, len)
        if closest_rail_index == 0 || closest_rail_index >= tiles.len() as i64 {
            continue;
        }
        // in-range: 0 < closest_rail_index < tiles.len() here (the grid never holds an empty
        // railroad, RailroadSpatialGrid.ts:37-39, so -1 cannot reach this point either).
        let split = closest_rail_index as usize;

        // Disconnect current rail as it will become invalid
        station_remove_railroad_env(e, from, rail);
        station_remove_railroad_env(e, to, rail);
        e.rail_mut().rail_grid.unregister(rail);

        let new_rail_from = e.rail_mut().new_railroad(from, station, tiles[..split].to_vec());
        let new_rail_to = e.rail_mut().new_railroad(station, to, tiles[split..].to_vec());

        // New station is connected to both new rails
        station_add_railroad_env(e, station, new_rail_from);
        station_add_railroad_env(e, station, new_rail_to);
        // From and to are connected to the new segments
        station_add_railroad_env(e, from, new_rail_from);
        station_add_railroad_env(e, to, new_rail_to);

        grid_register(e, new_rail_to);
        grid_register(e, new_rail_from);
        if let Some(cluster) = e.rail().station_obj(from).cluster {
            cluster_add_station_env(e, cluster, station);
            edited_clusters.insert(cluster);
        }
        // RailroadSnapEvent update: view-only (dropped).
        e.rail_mut().retire_railroad_if_detached(rail);
    }
    // If multiple clusters own the new station, merge them into a single cluster
    if edited_clusters.len() > 1 {
        let list: Vec<ClusterRef> = edited_clusters.iter().copied().collect();
        merge_clusters_env(e, &list);
    }
    !edited_clusters.is_empty()
}

/// `overlappingRailroads(unitType, tile)` (`:450-461`).
pub(crate) fn overlapping_railroads_env<E: RailRead + ?Sized>(
    e: &E,
    unit_type: UnitType,
    tile: TileRef,
) -> Vec<TileRef> {
    if ![UnitType::City, UnitType::Port, UnitType::Factory].contains(&unit_type) {
        return Vec::new();
    }
    // `new Set<TileRef>()` then `Array.from(tiles)`: dedup in first-seen order.
    // lookup-only: `seen` only answers `has` for the dedup; `out` keeps the order.
    let mut seen: LookupSet<TileRef> = LookupSet::new();
    let mut out: Vec<TileRef> = Vec::new();
    for railroad in grid_query(e, tile, STATION_RADIUS) {
        for &t in e.rail().railroad(railroad).tiles.iter() {
            if seen.insert(t) {
                out.push(t);
            }
        }
    }
    // TS: .sort((a, b) => a - b)
    of_det::js_sort_by(&mut out, |a, b| f64::from(*a) - f64::from(*b));
    out
}

/// `connectToNearbyStations(station)` (`:773-810`).
fn connect_to_nearby_stations_env<E: RailEnv + ?Sized>(e: &mut E, station: StationId) {
    let station_unit = e.rail().station_obj(station).unit;
    let mut neighbors = e.nearby_units(
        station_tile_env(e, station),
        e.train_station_max_range(),
        &[UnitType::City, UnitType::Factory, UnitType::Port],
    );

    let mut edited_clusters: SmallJsSet<ClusterRef, 4> = SmallJsSet::default();
    // TS: neighbors.sort((a, b) => a.distSquared - b.distSquared)
    of_det::js_sort_by(&mut neighbors, |a, b| a.dist_squared - b.dist_squared);
    let mut candidates: Vec<(StationId, f64)> = Vec::new();
    for neighbor in &neighbors {
        if neighbor.unit == station_unit {
            continue;
        }
        let key = UnitKey(e.unit_id(neighbor.unit));
        let Some(neighbor_station) = e.rail().station_manager.find_station(key) else { continue };
        if e.rail().station_obj(neighbor_station).cluster.is_none() {
            continue;
        }
        candidates.push((neighbor_station, neighbor.dist_squared));
    }

    plan_nearby_connections_env(e, station, &candidates, |e, neighbor, path| {
        connect_with_path_env(e, station, neighbor, path);
        let Some(neighbor_cluster) = e.rail().station_obj(neighbor).cluster else { return };
        cluster_add_station_env(e, neighbor_cluster, station);
        edited_clusters.insert(neighbor_cluster);
    });

    // If multiple clusters own the new station, merge them into a single cluster
    if edited_clusters.len() > 1 {
        let list: Vec<ClusterRef> = edited_clusters.iter().copied().collect();
        merge_clusters_env(e, &list);
    } else if edited_clusters.is_empty() {
        // If no cluster owns the station, creates a new one for it
        let new_cluster = e.rail_mut().new_cluster();
        cluster_add_station_env(e, new_cluster, station);
    }
}

/// `disconnectFromNetwork(station)` (`:812-818`).
fn disconnect_from_network_env<E: RailEnv + ?Sized>(e: &mut E, station: StationId) {
    // iter-mut: `rail.delete` removes the current railroad from this very set (JS Set
    // semantics: the iteration continues with the next live entry).
    let mut cur = e.rail().station_obj(station).railroads.cursor();
    while let Some(rail) = e.rail().station_obj(station).railroads.next_live(&mut cur) {
        railroad_delete_env(e, rail);
        e.rail_mut().rail_grid.unregister(rail);
        e.rail_mut().retire_railroad_if_detached(rail);
    }
    e.rail_mut().station_obj_mut(station).clear_railroads();
}

/// `deleteCluster(cluster)` (`:820-825`).
fn delete_cluster_env<E: RailEnv + ?Sized>(e: &mut E, cluster: ClusterRef) {
    // iter-mut: `setCluster(null)` removes the current station from this set.
    let mut cur = e.rail().clusters.get(cluster).stations.cursor();
    while let Some(station) = e.rail().clusters.get(cluster).stations.next_live(&mut cur) {
        station_set_cluster_env(e, station, None);
    }
    e.rail_mut().clusters.get_mut(cluster).clear();
}

/// `connectWithPath(from, to, path)` (`:827-841`); the construction update is view-only.
fn connect_with_path_env<E: RailEnv + ?Sized>(e: &mut E, from: StationId, to: StationId, path: Vec<TileRef>) {
    let railroad = e.rail_mut().new_railroad(from, to, path);
    station_add_railroad_env(e, from, railroad);
    station_add_railroad_env(e, to, railroad);
    grid_register(e, railroad);
}

/// `validRailPath(from, to)` (`:843-849`).
fn valid_rail_path_env<E: RailEnv + ?Sized>(e: &mut E, from: TileRef, to: TileRef) -> Option<Vec<TileRef>> {
    let path = e.find_tile_path(from, to);
    // int: a path length (< 2^53) compared with the number TS returns.
    if !path.is_empty() && (path.len() as f64) < e.railroad_max_size() {
        return Some(path);
    }
    None
}

/// `planNearbyConnections(station, neighbors, connect)` (`:856-883`) without a budget (the live
/// network passes none; the budget is preview-only).
fn plan_nearby_connections_env<E: RailEnv + ?Sized>(
    e: &mut E,
    station: StationId,
    neighbors: &[(StationId, f64)],
    mut connect: impl FnMut(&mut E, StationId, Vec<TileRef>),
) {
    // TS: this.game.config().trainStationMinRange() ** 2
    let min_range = e.train_station_min_range();
    let min_range_squared = min_range * min_range;
    // Rails are undirected, so one search out from the station tells for every
    // neighbor at once whether it is already in reach. Each new rail extends
    // the reach, so the search is redone after one.
    let mut reachable = stations_within_env(e, station, MAX_CONNECTION_DISTANCE);
    for &(neighbor, dist_squared) in neighbors {
        if reachable.contains(&neighbor) || dist_squared <= min_range_squared {
            continue;
        }

        let from = station_tile_env(e, station);
        let to = station_tile_env(e, neighbor);
        if let Some(path) = valid_rail_path_env(e, from, to) {
            connect(e, neighbor, path);
            reachable = stations_within_env(e, station, MAX_CONNECTION_DISTANCE);
        }
    }
}

/// `stationsWithin(start, maxDistance)` (`:886-904`): every station within `maxDistance` rail
/// hops of `start`, itself included.
// lookup-only: the result is only asked `has` (`:870`); the BFS order is irrelevant to it.
fn stations_within_env<E: RailRead + ?Sized>(e: &E, start: StationId, max_distance: u32) -> LookupSet<StationId> {
    let mut visited: LookupSet<StationId> = LookupSet::new();
    visited.insert(start);
    let mut frontier: Vec<StationId> = vec![start];
    let mut hop = 0;
    while hop < max_distance && !frontier.is_empty() {
        let mut next: Vec<StationId> = Vec::new();
        for &station in &frontier {
            for neighbor in station_neighbors_env(e, station) {
                if visited.contains(&neighbor) {
                    continue;
                }
                visited.insert(neighbor);
                next.push(neighbor);
            }
        }
        frontier = next;
        hop += 1;
    }
    visited
}

/// `computeCluster(start)` (`:906-921`): BFS with `queue.shift()`; the returned `Set` is in
/// visit order.
fn compute_cluster_env<E: RailRead + ?Sized>(e: &E, start: StationId) -> JsSet<StationId> {
    let mut visited: JsSet<StationId> = JsSet::default();
    // `queue = [start]` with push/shift only: a VecDeque (PORTING_GUIDE §13).
    let mut queue: VecDeque<StationId> = VecDeque::from([start]);

    while let Some(current) = queue.pop_front() {
        if visited.contains(&current) {
            continue;
        }
        visited.insert(current);

        for neighbor in station_neighbors_env(e, current) {
            if !visited.contains(&neighbor) {
                queue.push_back(neighbor);
            }
        }
    }

    visited
}

/// `mergeClusters(clustersToMerge)` (`:923-928`); `clusters` in `Set` order.
fn merge_clusters_env<E: RailEnv + ?Sized>(e: &mut E, clusters: &[ClusterRef]) {
    let merged = e.rail_mut().new_cluster();
    for &cluster in clusters {
        cluster_merge_env(e, merged, cluster);
    }
}

/// `hash()` (`:935-948`): order-independent digest (legacy hash, on demand).
pub(crate) fn rail_hash_env<E: RailRead + ?Sized>(e: &E) -> f64 {
    let rail = e.rail();
    let mut hash: f64 = 0.0;
    for &station in rail.station_manager.get_all().iter() {
        let st = rail.station_obj(station);
        // TS: (hash + station.unit.id() * 7919) >>> 0
        hash = f64::from(of_det::js::to_uint32(hash + f64::from(e.unit_id(st.unit)) * 7919.0));
        for &r in st.railroads.iter() {
            let rr = rail.railroad(r);
            if rr.from != station {
                continue;
            }
            let tiles = &rr.tiles;
            let ends = match (tiles.first(), tiles.last()) {
                (Some(&a), Some(&b)) => f64::from(a) * 3.0 + f64::from(b),
                _ => 0.0,
            };
            // int: lengths and ids are small exact integers in f64.
            let len = tiles.len() as f64;
            hash = f64::from(of_det::js::to_uint32(hash + (f64::from(rr.id.0) + 1.0) * (len * 31.0 + ends)));
        }
    }
    hash
}

impl State {
    /// `railNetwork.overlappingRailroads(unitType, tile)` (`:450-461`).
    pub fn rail_overlapping_railroads(&self, unit_type: UnitType, tile: TileRef) -> Vec<TileRef> {
        overlapping_railroads_env(self, unit_type, tile)
    }
    /// `railNetwork.computeGhostRailPaths(unitType, tile)` (`:463-497`): **skipped** (UI preview;
    /// ARCHITECTURE §3.9). Always empty; it cannot change state.
    pub fn rail_compute_ghost_rail_paths(&self, _unit_type: UnitType, _tile: TileRef) -> Vec<Vec<TileRef>> {
        Vec::new()
    }
    /// `railNetwork.hash()` (legacy hash, on demand, `:935-948`).
    pub fn rail_hash(&self) -> f64 {
        rail_hash_env(self)
    }
}

impl Ctx<'_> {
    /// `railNetwork.connectStation(station)` (`:324-330`) for a station already added with
    /// `station_manager.add_station` (TS `addStation` is the first statement after the derived
    /// `networkVersion++`). Splitting the two lets the caller store the station before the
    /// connection runs, as `TrainStationExecution.tick` does (`:53-55`).
    pub fn rail_connect_station(&mut self, s: StationId) {
        connect_station_env(self, s);
    }
    /// `railNetwork.removeStation(unit)` (`:357-380`).
    pub fn rail_remove_station(&mut self, unit: UnitRef) {
        remove_station_env(self, unit);
    }
    /// `railNetwork.findStationsPath(from, to)` (`:382-384`).
    pub fn rail_find_stations_path(&mut self, from: StationId, to: StationId) -> Vec<StationId> {
        crate::rail::env::RailEnv::find_stations_path(self, from, to)
    }
    /// `railNetwork.recomputeClusters()` (`:332-355`).
    pub fn rail_recompute_clusters(&mut self) {
        recompute_clusters_env(self);
    }
}

#[cfg(test)]
mod tests {
    //! T0: `tests/core/game/RailNetwork.test.ts` (StationManagerImpl + RailNetworkImpl blocks,
    //! the ghost-preview blocks are skipped with the preview) on the fake world, plus T2 module
    //! tests for the snap/split/cluster paths the upstream mocks do not reach.
    use super::*;
    use crate::rail::testenv::World;

    fn city(w: &mut World, tile: TileRef) -> (UnitRef, StationId) {
        let u = w.unit(UnitType::City, 1, tile);
        let s = w.station(u);
        (u, s)
    }

    fn cluster_stations(w: &World, c: ClusterRef) -> Vec<u32> {
        w.rail.clusters.get(c).stations.iter().map(|s| s.0).collect()
    }

    // ---- StationManagerImpl ----

    #[test]
    fn station_manager_adds_and_retrieves_station() {
        let mut m = StationManager::new();
        let s = m.add_station(UnitKey(1), TrainStation::new(StationId(0), UnitRef::default()));
        assert_eq!(s, StationId(1), "ids start at 1");
        assert_eq!(m.find_station(UnitKey(1)), Some(s));
        assert_eq!(m.get(s).map(|t| t.id), Some(s));
        assert_eq!(m.count(), 2, "count() is nextId");
    }

    #[test]
    fn station_manager_removes_station() {
        let mut m = StationManager::new();
        let s = m.add_station(UnitKey(1), TrainStation::new(StationId(0), UnitRef::default()));
        m.remove_station(s);
        assert_eq!(m.find_station(UnitKey(1)), None);
        assert!(m.get(s).is_none(), "getById: undefined after removal");
        assert!(m.obj(s).is_some(), "the object stays readable (TS executions keep it)");
        assert_eq!(m.count(), 2, "nextId never decreases");
    }

    #[test]
    fn station_manager_falls_back_to_next_station_in_restore_order() {
        let mut m = StationManager::new();
        let first = m.add_station(UnitKey(1), TrainStation::new(StationId(0), UnitRef::default()));
        let second = m.add_station(UnitKey(1), TrainStation::new(StationId(0), UnitRef::default()));
        let other = m.add_station(UnitKey(2), TrainStation::new(StationId(0), UnitRef::default()));
        assert_eq!(m.find_station(UnitKey(1)), Some(first));

        m.remove_station(first);
        assert_eq!(m.find_station(UnitKey(1)), Some(second));
        // The live map equals the one restoreSnapshot rebuilds from `stations` order.
        let live: Vec<(u32, u32)> = m.by_unit.iter().map(|(k, s)| (k.0, s.0)).collect();
        assert_eq!(live, [(1, second.0), (2, other.0)]);
        // Removing a non-first station keeps the map as is (no rebuild).
        let third = m.add_station(UnitKey(2), TrainStation::new(StationId(0), UnitRef::default()));
        m.remove_station(third);
        let live: Vec<(u32, u32)> = m.by_unit.iter().map(|(k, s)| (k.0, s.0)).collect();
        assert_eq!(live, [(1, second.0), (2, other.0)]);
    }

    // ---- RailNetworkImpl ----

    #[test]
    fn does_not_connect_if_path_is_empty_or_too_long() {
        let mut w = World::new();
        let (_, b) = city(&mut w, 50);
        // b gets its own cluster (no neighbours yet).
        connect_station_env(&mut w, b);
        let cb = w.rail.station_obj(b).cluster.expect("b clustered");
        let (_, a) = city(&mut w, 0);
        w.tile_paths.insert((0, 50), vec![]);
        connect_station_env(&mut w, a);
        assert_eq!(cluster_stations(&w, cb), [b.0], "empty path: not connected");
        assert!(w.rail.station_obj(a).railroads.is_empty());

        let (_, a2) = city(&mut w, 1);
        w.tile_paths.insert((1, 50), vec![7; 200]);
        connect_station_env(&mut w, a2);
        assert_eq!(cluster_stations(&w, cb), [b.0], "path of 200 >= railroadMaxSize 100: not connected");
        assert!(w.rail.station_obj(a2).railroads.is_empty());
        assert_ne!(w.rail.station_obj(a2).cluster, Some(cb));
    }

    #[test]
    fn remove_station_clears_rails_and_updates_cluster() {
        let mut w = World::new();
        let (_, a) = city(&mut w, 0);
        connect_station_env(&mut w, a);
        let (ub, b) = city(&mut w, 50);
        connect_station_env(&mut w, b);
        let (_, c) = city(&mut w, 99);
        connect_station_env(&mut w, c);
        let cl = w.rail.station_obj(a).cluster.expect("a clustered");
        assert_eq!(cluster_stations(&w, cl), [a.0, b.0, c.0], "b joined a's cluster, c joined via b");
        assert!(!w.rail.station_obj(b).railroads.is_empty());

        remove_station_env(&mut w, ub);
        assert!(w.rail.station_obj(b).railroads.is_empty(), "clearRailroads");
        assert_eq!(w.rail.station_manager.find_station(UnitKey(w.unit_data(ub).id)), None);
        let ub_id = w.unit_data(ub).id;
        assert!(w.calls.contains(&crate::rail::testenv::Call::SetTrainStation(ub_id, false)), "setTrainStation(false)");
        assert_eq!(cluster_stations(&w, cl), [a.0, c.0]);
        assert_eq!(w.rail.dirty_clusters.iter().copied().collect::<Vec<_>>(), [cl]);
        assert_eq!(w.rail.station_obj(b).cluster, Some(cl), "a removed station keeps its cluster field");
        // Every railroad that touched b is detached and retired from the id map.
        assert!(w.rail.rail_grid.rail_to_cells.iter().all(|(r, _)| {
            let rr = w.rail.railroad(*r);
            rr.from != b && rr.to != b
        }));

        // recomputeClusters splits {a, c} (no rail between them now): the first component found
        // moves to a new cluster, the last one stays.
        recompute_clusters_env(&mut w);
        assert!(w.rail.dirty_clusters.is_empty());
        let ca = w.rail.station_obj(a).cluster.expect("a");
        let cc = w.rail.station_obj(c).cluster.expect("c");
        assert_ne!(ca, cc);
        assert_eq!(cc, cl, "the last component stays in the original cluster");
        assert_eq!(cluster_stations(&w, ca), [a.0]);
        assert_eq!(cluster_stations(&w, cc), [c.0]);
    }

    #[test]
    fn remove_station_does_nothing_if_station_not_found() {
        let mut w = World::new();
        let u = w.unit(UnitType::City, 1, 3);
        let before = w.rail.network_version;
        remove_station_env(&mut w, u);
        assert_eq!(w.rail.network_version, before);
        assert!(w.calls.is_empty());
    }

    #[test]
    fn remove_last_station_deletes_cluster() {
        let mut w = World::new();
        let (u, s) = city(&mut w, 0);
        connect_station_env(&mut w, s);
        let cl = w.rail.station_obj(s).cluster.expect("clustered");
        w.rail.dirty_clusters.insert(cl);
        remove_station_env(&mut w, u);
        assert_eq!(w.rail.clusters.get(cl).size(), 0);
        assert!(w.rail.dirty_clusters.is_empty(), "an emptied cluster leaves dirtyClusters");
    }

    #[test]
    fn find_stations_path_uses_the_path_service() {
        let mut w = World::new();
        let (_, a) = city(&mut w, 0);
        let (_, b) = city(&mut w, 1);
        w.link(a, b, &[0, 1], 0);
        assert_eq!(w.find_stations_path(a, b), [a, b]);
    }

    #[test]
    fn connect_to_nearby_stations_creates_new_cluster_when_no_neighbors() {
        let mut w = World::new();
        let (_, s) = city(&mut w, 0);
        connect_station_env(&mut w, s);
        let cl = w.rail.station_obj(s).cluster.expect("a new cluster");
        assert_eq!(cluster_stations(&w, cl), [s.0]);
        assert_eq!(w.rail.clusters.get(cl).trade_stations.iter().map(|s| s.0).collect::<Vec<_>>(), [s.0]);
    }

    #[test]
    fn connect_to_nearby_stations_connects_and_merges_clusters() {
        let mut w = World::new();
        w.max_range = 30.0;
        let (_, a) = city(&mut w, 0);
        connect_station_env(&mut w, a);
        let (_, b) = city(&mut w, 40);
        connect_station_env(&mut w, b);
        let (ca, cb) = (w.rail.station_obj(a).cluster.expect("a"), w.rail.station_obj(b).cluster.expect("b"));
        assert_ne!(ca, cb, "40 tiles apart, range 30: separate clusters");
        // m reaches both (20 each): rail m–a, then m–b; each joins m to that neighbour's
        // cluster (setCluster moves m out of ca into cb); then the two edited clusters merge
        // into a new one in insertion order: ca's stations {a}, then cb's {b, m}.
        let (_, m) = city(&mut w, 20);
        connect_station_env(&mut w, m);
        let cm = w.rail.station_obj(m).cluster.expect("m");
        assert_ne!(cm, ca);
        assert_ne!(cm, cb);
        assert_eq!(cluster_stations(&w, cm), [a.0, b.0, m.0]);
        for s in [a, b, m] {
            assert_eq!(w.rail.station_obj(s).cluster, Some(cm));
        }
        assert_eq!(w.rail.clusters.get(ca).size(), 0);
        assert_eq!(w.rail.clusters.get(cb).size(), 0);
        // Rails m–a and m–b exist, in connection order (sorted by distance, stable).
        assert_eq!(station_neighbors_env(&w, m), [a, b]);
    }

    #[test]
    fn min_range_and_reachability_skip_connections() {
        let mut w = World::new();
        let (_, a) = city(&mut w, 0);
        connect_station_env(&mut w, a);
        // 5 tiles away: distSquared 25 <= minRange² 100 → no rail, own cluster.
        let (_, b) = city(&mut w, 5);
        connect_station_env(&mut w, b);
        assert!(w.rail.station_obj(b).railroads.is_empty());
        assert_ne!(w.rail.station_obj(a).cluster, w.rail.station_obj(b).cluster);
        // c at 30 connects to a (dist 900) and to b (dist 625): b first (nearer), then a.
        let (_, c) = city(&mut w, 30);
        connect_station_env(&mut w, c);
        assert_eq!(station_neighbors_env(&w, c), [b, a]);
        // d at 31: c is 1 away (<= min range), b at 26, a at 31: b is nearest valid; after the
        // d–b rail, a and c are within 4 hops → no more rails.
        let q0 = w.tile_path_queries.len();
        let (_, d) = city(&mut w, 31);
        connect_station_env(&mut w, d);
        assert_eq!(station_neighbors_env(&w, d), [b]);
        assert_eq!(w.tile_path_queries.len() - q0, 1, "one path search: the rest were reachable");
    }

    #[test]
    fn snaps_to_an_existing_rail_and_splits_it() {
        let mut w = World::new();
        let (_, a) = city(&mut w, 0);
        connect_station_env(&mut w, a);
        let (_, b) = city(&mut w, 40);
        connect_station_env(&mut w, b);
        // connectWithPath(b, a, path): the rail runs from b to a.
        let r0 = *w.rail.station_obj(a).railroads.iter().next().expect("a–b rail");
        assert_eq!((w.rail.railroad(r0).from, w.rail.railroad(r0).to), (b, a));
        assert_eq!(w.rail.railroad(r0).tiles, (0..=40).rev().collect::<Vec<u32>>());
        let next_id = w.rail.next_id;
        // A factory one row below tile 20 (tile 120): the rail runs within the 3-tile radius.
        let f = w.unit(UnitType::Factory, 1, 120);
        let s = w.station(f);
        connect_station_env(&mut w, s);
        // Closest rail tile is 20 at index 20: from-half [40..21] (b → s), to-half [20..0] (s → a).
        let (rf, rt) = (RailroadId(next_id), RailroadId(next_id + 1));
        assert_eq!(w.rail.railroad(rf).tiles, (21..=40).rev().collect::<Vec<u32>>());
        assert_eq!((w.rail.railroad(rf).from, w.rail.railroad(rf).to), (b, s));
        assert_eq!(w.rail.railroad(rt).tiles, (0..=20).rev().collect::<Vec<u32>>());
        assert_eq!((w.rail.railroad(rt).from, w.rail.railroad(rt).to), (s, a));
        assert_eq!(station_neighbors_env(&w, s), [b, a]);
        assert_eq!(station_neighbors_env(&w, a), [s]);
        assert_eq!(station_neighbors_env(&w, b), [s]);
        // The old rail left the grid and the id map; grid registration order is to-half first.
        assert!(!w.rail.railroad_by_id.contains_key(&r0));
        assert_eq!(w.rail.rail_grid.rail_to_cells.keys().copied().collect::<Vec<_>>(), [rt, rf]);
        let cl = w.rail.station_obj(a).cluster.expect("cluster");
        assert_eq!(w.rail.station_obj(s).cluster, Some(cl));
        // A Factory is not a trade station.
        assert!(!w.rail.clusters.get(cl).trade_stations.contains(&s));
        // No nearby-station connection ran (snapping returned true): no path query from s.
        assert!(w.tile_path_queries.iter().all(|&(from, _)| from != 120));
    }

    #[test]
    fn snap_skips_rail_whose_closest_tile_is_its_start() {
        let mut w = World::new();
        let (_, a) = city(&mut w, 0);
        connect_station_env(&mut w, a);
        let (_, b) = city(&mut w, 40);
        connect_station_env(&mut w, b);
        // Tile 140 = (40,1): the closest rail tile is the rail's first tile (index 0) → no split;
        // the station connects to nearby stations instead.
        let f = w.unit(UnitType::Factory, 1, 140);
        let s = w.station(f);
        connect_station_env(&mut w, s);
        let r0 = *w.rail.station_obj(a).railroads.iter().next().expect("a–b rail");
        assert_eq!((w.rail.railroad(r0).from, w.rail.railroad(r0).to), (b, a), "the b–a rail is untouched");
        assert!(w.tile_path_queries.iter().any(|&(from, _)| from == 140), "connectToNearbyStations ran");
    }

    #[test]
    fn overlapping_railroads_dedups_and_sorts() {
        let mut w = World::new();
        let (_, a) = city(&mut w, 0);
        let (_, b) = city(&mut w, 99);
        let r1 = w.link(a, b, &[50, 42, 60], 0);
        let r2 = w.link(a, b, &[60, 45, 42], 1);
        w.rail.rail_grid.register_with(r1, &[50, 42, 60], |t| (t % 100, t / 100));
        w.rail.rail_grid.register_with(r2, &[60, 45, 42], |t| (t % 100, t / 100));
        assert_eq!(overlapping_railroads_env(&w, UnitType::City, 42), [42, 45, 50, 60]);
        assert!(overlapping_railroads_env(&w, UnitType::City, 9042).is_empty(), "no rail nearby");
        for t in [UnitType::MissileSilo, UnitType::DefensePost, UnitType::SamLauncher] {
            assert!(overlapping_railroads_env(&w, t, 42).is_empty(), "{t:?} cannot snap");
        }
        assert_eq!(overlapping_railroads_env(&w, UnitType::Port, 42).len(), 4);
        assert_eq!(overlapping_railroads_env(&w, UnitType::Factory, 42).len(), 4);
    }

    #[test]
    fn disconnect_visits_every_rail_while_deleting() {
        // iter-mut test: the loop body deletes the current railroad from the set it walks.
        let mut w = World::new();
        let (u0, hub) = city(&mut w, 500);
        let mut spokes = Vec::new();
        for (i, t) in [0u32, 10, 20, 30, 40, 50, 60].into_iter().enumerate() {
            let (_, s) = city(&mut w, t);
            w.link(hub, s, &[500, t], i as u32);
            spokes.push(s);
        }
        assert_eq!(w.rail.station_obj(hub).railroads.len(), 7, "spills past the inline capacity 4");
        remove_station_env(&mut w, u0);
        assert!(w.rail.station_obj(hub).railroads.is_empty());
        for s in spokes {
            assert!(w.rail.station_obj(s).railroads.is_empty(), "every spoke lost its rail");
            assert!(w.rail.station_obj(s).railroad_by_neighbor.is_empty());
        }
        assert!(w.rail.railroad_by_id.is_empty(), "all retired");
    }

    #[test]
    fn stations_within_counts_hops() {
        let mut w = World::new();
        let st: Vec<StationId> = (0..7).map(|i| city(&mut w, i * 10).1).collect();
        for i in 0..6 {
            w.link(st[i], st[i + 1], &[i as u32 * 10, i as u32 * 10 + 10], i as u32);
        }
        let r = stations_within_env(&w, st[0], 4);
        for (i, s) in st.iter().enumerate() {
            assert_eq!(r.contains(s), i <= 4, "station {i}");
        }
        let c = compute_cluster_env(&w, st[3]);
        assert_eq!(c.iter().map(|s| s.0).collect::<Vec<_>>(), [4, 3, 5, 2, 6, 1, 7], "BFS visit order");
    }

    #[test]
    fn hash_counts_each_rail_once_from_its_from_station() {
        let mut w = World::new();
        let (_, a) = city(&mut w, 1);
        let (_, b) = city(&mut w, 2);
        w.link(a, b, &[1, 2, 3], 5);
        // station a (unit id 1): 7919; rail from a: (5+1)*(3*31 + 1*3+3) = 6*99 = 594;
        // station b (unit id 2): + 2*7919.
        let expect = 7919.0 + 594.0 + 2.0 * 7919.0;
        assert_eq!(rail_hash_env(&w), expect);
    }

    #[test]
    fn retire_keeps_partially_attached_rails() {
        let mut w = World::new();
        let (_, a) = city(&mut w, 1);
        let (_, b) = city(&mut w, 2);
        let r = w.link(a, b, &[1, 2], 0);
        crate::rail::station::station_remove_neighboring_rails_env(&mut w, a, b);
        assert!(w.rail.station_obj(a).railroads.is_empty());
        assert!(w.rail.railroad_by_id.contains_key(&r), "b still names it");
        station_remove_railroad_env(&mut w, b, r);
        w.rail.retire_railroad_if_detached(r);
        assert!(!w.rail.railroad_by_id.contains_key(&r));
        assert!(w.rail.railroads.iter_slots().any(|(_, rr)| rr.id == r && rr.deleted));
    }
}
