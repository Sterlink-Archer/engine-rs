//! `TrainStation` / `Cluster` (`game/TrainStation.ts`). Owner: **P5**.
//!
//! No PRNG field: `createTrainStopHandlers(random)` ignores its argument (`:59-68`), so the
//! stop handler is a `match` on the unit type (ARCHITECTURE §3.9, §4.5).
//!
//! Identity: a station is its TS id ([`StationId`], unique, never reused); its data lives in
//! the station manager's object store (`RailNetwork::station_obj`), which keeps removed
//! stations readable, as TS executions keep reading a removed station object. A cluster is a
//! [`ClusterRef`] into `RailNetwork.clusters`. `Set<TrainStation>`/`Set<Railroad>`/
//! `Map<TrainStation, Railroad>` key by those ids, in insertion order.
//!
//! Every method has a generic `*_env` body over the rail env (`rail::env`); the frozen
//! `State`/`Ctx` methods call it with `self`.

use crate::StateClone;
use crate::api::types::UnitType;
use crate::config::engine_config::TrainRelation;
use crate::ctx::{Ctx, TsExpect};
use crate::exec::train::TrainExec;
use crate::ids::{ClusterRef, RailroadId, SmallId, StationId, TileRef, UnitRef};
use crate::jsorder::{SmallJsMap, SmallJsSet};
use crate::rail::env::{RailEnv, RailRead};
use crate::state::State;
use of_det::Prng;

/// `TrainStation` data, `TrainStation v1` snapshot order.
#[derive(StateClone, Debug, Default)]
pub struct TrainStation {
    pub id: StationId,
    pub unit: UnitRef,
    pub cluster: Option<ClusterRef>,
    /// `railroads: Set<Railroad>` (`:75`).
    pub railroads: SmallJsSet<RailroadId, 4>,
    /// `railroadByNeighbor: Map<TrainStation, Railroad>` (`:77`).
    pub railroad_by_neighbor: SmallJsMap<StationId, RailroadId, 4>,
}

impl TrainStation {
    /// `new TrainStation(mg, unit)` (`:79-84`). The id is assigned by
    /// `StationManager::add_station` (TS: `-1` until then). The constructor's
    /// `new PseudoRandom(mg.ticks())` is never drawn (`:59-68`) and has no port.
    pub fn new(id: StationId, unit: UnitRef) -> Self {
        TrainStation {
            id,
            unit,
            cluster: None,
            railroads: SmallJsSet::default(),
            railroad_by_neighbor: SmallJsMap::default(),
        }
    }
    /// `clearRailroads()` (`:91-94`).
    pub fn clear_railroads(&mut self) {
        self.railroads.clear();
        self.railroad_by_neighbor.clear();
    }
    /// `getRailroads()` (`:141-143`).
    pub fn get_railroads(&self) -> &SmallJsSet<RailroadId, 4> {
        &self.railroads
    }
    /// The field write of `setCluster(cluster)` only. The TS method also removes the station
    /// from its previous cluster (`:149-155`); that needs the cluster arena, so the full method
    /// is [`crate::rail::RailNetwork::set_station_cluster`] (no caller outside this package).
    pub fn set_cluster(&mut self, cluster: Option<ClusterRef>) {
        self.cluster = cluster;
    }
    /// `getCluster()` (`:157-159`).
    pub fn get_cluster(&self) -> Option<ClusterRef> {
        self.cluster
    }
}

// ---- TrainStation, generic bodies ----

/// `station.tile()` (`:133-135`).
#[inline]
pub(crate) fn station_tile_env<E: RailRead + ?Sized>(e: &E, s: StationId) -> TileRef {
    e.unit_tile(e.rail().station_obj(s).unit)
}

/// `station.isActive()` (`:137-139`).
#[inline]
pub(crate) fn station_is_active_env<E: RailRead + ?Sized>(e: &E, s: StationId) -> bool {
    e.unit_is_active(e.rail().station_obj(s).unit)
}

/// `station.tradeAvailable(otherPlayer)` (`:86-89`).
pub(crate) fn station_trade_available_env<E: RailRead + ?Sized>(e: &E, s: StationId, other: SmallId) -> bool {
    let player = e.unit_owner(e.rail().station_obj(s).unit);
    other == player || e.player_can_trade(player, other)
}

/// `station.neighbors()` (`:121-131`): one entry per railroad, in `railroads` order.
pub(crate) fn station_neighbors_env<E: RailRead + ?Sized>(e: &E, s: StationId) -> Vec<StationId> {
    let rail = e.rail();
    let mut neighbors = Vec::with_capacity(rail.station_obj(s).railroads.len());
    for &r in rail.station_obj(s).railroads.iter() {
        let rr = rail.railroad(r);
        if rr.from != s {
            neighbors.push(rr.from);
        } else {
            neighbors.push(rr.to);
        }
    }
    neighbors
}

/// `station.addRailroad(railRoad)` (`:96-100`).
pub(crate) fn station_add_railroad_env<E: RailEnv + ?Sized>(e: &mut E, s: StationId, r: RailroadId) {
    let rail = e.rail_mut();
    let neighbor = {
        let rr = rail.railroad(r);
        if rr.from == s { rr.to } else { rr.from }
    };
    let st = rail.station_obj_mut(s);
    st.railroads.insert(r);
    st.railroad_by_neighbor.insert(neighbor, r);
}

/// `station.removeRailroad(railRoad)` (`:102-106`).
pub(crate) fn station_remove_railroad_env<E: RailEnv + ?Sized>(e: &mut E, s: StationId, r: RailroadId) {
    let rail = e.rail_mut();
    let neighbor = {
        let rr = rail.railroad(r);
        if rr.from == s { rr.to } else { rr.from }
    };
    let st = rail.station_obj_mut(s);
    st.railroads.remove(&r);
    st.railroad_by_neighbor.remove(&neighbor);
}

/// `station.removeNeighboringRails(station)` (`:108-119`). No caller in the engine at the pin
/// (only the upstream unit test); ported for completeness. The destruction update is
/// view-only (dropped).
pub(crate) fn station_remove_neighboring_rails_env<E: RailEnv + ?Sized>(e: &mut E, s: StationId, other: StationId) {
    // TS: [...this.railroads].find(r => r.from === station || r.to === station)
    let to_remove = {
        let rail = e.rail();
        rail.station_obj(s).railroads.iter().copied().find(|&r| {
            let rr = rail.railroad(r);
            rr.from == other || rr.to == other
        })
    };
    if let Some(r) = to_remove {
        station_remove_railroad_env(e, s, r);
        e.rail_mut().retire_railroad_if_detached(r);
    }
}

/// `rel(player, other)` (`TrainStation.ts:322-336`).
fn rel<E: RailRead + ?Sized>(e: &E, player: SmallId, other: SmallId) -> TrainRelation {
    if player == other {
        return TrainRelation::SelfOwned;
    }
    if e.player_is_on_same_team(player, other) {
        return TrainRelation::Team;
    }
    if e.player_is_allied_with(player, other) {
        return TrainRelation::Ally;
    }
    TrainRelation::Other
}

/// `station.onTrainStop(trainExecution)` (`:161-167`) with the handler table of
/// `createTrainStopHandlers` (`:61-69`): City and Port → `TradeStationStopHandler`
/// (`:26-51`), Factory → `FactoryStopHandler` (no-op, `:53-59`), any other type → none.
/// `train_owner`/`trade_stops_visited` are `trainExecution.owner()`/`.tradeStopsVisited()`.
pub(crate) fn station_on_train_stop_env<E: RailEnv + ?Sized>(
    e: &mut E,
    s: StationId,
    train_owner: SmallId,
    trade_stops_visited: i32,
) {
    let unit = e.rail().station_obj(s).unit;
    match e.unit_type(unit) {
        UnitType::City | UnitType::Port => {
            // TradeStationStopHandler.onStop (:27-50)
            let station_owner = e.unit_owner(unit);
            let gold = e.train_gold(rel(e, train_owner, station_owner), f64::from(trade_stops_visited), train_owner);
            // Share revenue with the station owner if it's not the current player
            if train_owner != station_owner {
                let tile = station_tile_env(e, s);
                e.player_add_gold(station_owner, gold, tile);
                e.player_add_train_gold(station_owner, gold);
                e.stats_train_external_trade(station_owner, gold);
            }
            let tile = station_tile_env(e, s);
            e.player_add_gold(train_owner, gold, tile);
            e.player_add_train_gold(train_owner, gold);
            e.stats_train_self_trade(train_owner, gold);
        }
        // FactoryStopHandler.onStop is empty; other types have no handler (`if (handler)`).
        _ => {}
    }
}

impl State {
    /// The station object with TS id `s` (TS code holds the object itself; a removed station
    /// stays readable). An id the manager never assigned is a TS `TypeError` on `undefined`.
    #[inline]
    pub fn station(&self, s: StationId) -> &TrainStation {
        self.rail.station_manager.obj(s).ts_expect("TrainStation lookup by id")
    }
    /// `station.tradeAvailable(otherPlayer)` (`:86-89`).
    pub fn station_trade_available(&self, s: StationId, other: SmallId) -> bool {
        station_trade_available_env(self, s, other)
    }
    /// `station.neighbors()` (`:121-131`).
    pub fn station_neighbors(&self, s: StationId) -> Vec<StationId> {
        station_neighbors_env(self, s)
    }
    /// `station.tile()` (`:133-135`).
    pub fn station_tile(&self, s: StationId) -> TileRef {
        station_tile_env(self, s)
    }
    /// `station.isActive()` (`:137-139`).
    pub fn station_is_active(&self, s: StationId) -> bool {
        station_is_active_env(self, s)
    }
    /// `getRailroadTo(station)` (`:145-147`).
    pub fn station_railroad_to(&self, s: StationId, to: StationId) -> Option<RailroadId> {
        self.station(s).railroad_by_neighbor.get(&to).copied()
    }
}

impl Ctx<'_> {
    /// `station.addRailroad(r)` (`:96-100`).
    pub fn station_add_railroad(&mut self, s: StationId, r: RailroadId) {
        station_add_railroad_env(self, s, r);
    }
    /// `station.removeRailroad(r)` (`:102-106`).
    pub fn station_remove_railroad(&mut self, s: StationId, r: RailroadId) {
        station_remove_railroad_env(self, s, r);
        self.st.rail.retire_railroad_if_detached(r);
    }
    /// `station.removeNeighboringRails(station)` (`:108-119`).
    pub fn station_remove_neighboring_rails(&mut self, s: StationId, other: StationId) {
        station_remove_neighboring_rails_env(self, s, other);
    }
    /// `station.onTrainStop(trainExecution)` (`:161-167`): the handler for the unit's type.
    pub fn station_on_train_stop(&mut self, s: StationId, train: &mut TrainExec) {
        station_on_train_stop_env(self, s, train.player, train.trade_stops_visited);
    }
}

/// `Cluster` data, `Cluster v1` snapshot order (`:212-215`).
#[derive(StateClone, Debug, Default)]
pub struct Cluster {
    pub stations: SmallJsSet<StationId, 8>,
    pub trade_stations: SmallJsSet<StationId, 8>,
}

impl Cluster {
    /// `has(station)` (`:221-223`).
    pub fn has(&self, s: StationId) -> bool {
        self.stations.contains(&s)
    }
    /// `size()` (`:289-291`).
    pub fn size(&self) -> usize {
        self.stations.len()
    }
    /// `clear()` (`:293-296`).
    pub fn clear(&mut self) {
        self.stations.clear();
        self.trade_stations.clear();
    }
    /// `removeStation(station)` (`:233-236`).
    pub fn remove_station(&mut self, s: StationId) {
        self.stations.remove(&s);
        self.trade_stations.remove(&s);
    }
}

// ---- Cluster, generic bodies ----

/// `isTradeStation(station)` (`:216-219`).
fn is_trade_station<E: RailRead + ?Sized>(e: &E, s: StationId) -> bool {
    let t = e.unit_type(e.rail().station_obj(s).unit);
    t == UnitType::City || t == UnitType::Port
}

/// `station.setCluster(cluster)` (`:149-155`).
pub(crate) fn station_set_cluster_env<E: RailEnv + ?Sized>(e: &mut E, s: StationId, cluster: Option<ClusterRef>) {
    let rail = e.rail_mut();
    // Properly disconnect cluster if it's already set
    if let Some(old) = rail.station_obj(s).cluster
        && Some(old) != cluster
    {
        rail.clusters.get_mut(old).remove_station(s);
    }
    rail.station_obj_mut(s).cluster = cluster;
}

/// `cluster.addStation(station)` (`:225-231`).
pub(crate) fn cluster_add_station_env<E: RailEnv + ?Sized>(e: &mut E, c: ClusterRef, s: StationId) {
    let trade = is_trade_station(e, s);
    let cl = e.rail_mut().clusters.get_mut(c);
    cl.stations.insert(s);
    if trade {
        cl.trade_stations.insert(s);
    }
    station_set_cluster_env(e, s, Some(c));
}

/// `cluster.addStations(stations)` (`:238-242`); `stations` in the caller's `Set` order.
pub(crate) fn cluster_add_stations_env<E: RailEnv + ?Sized>(e: &mut E, c: ClusterRef, stations: &[StationId]) {
    for &s in stations {
        cluster_add_station_env(e, c, s);
    }
}

/// `cluster.merge(other)` (`:244-248`). The body removes each visited station from `other`
/// (through `setCluster`), the element the loop is on.
pub(crate) fn cluster_merge_env<E: RailEnv + ?Sized>(e: &mut E, c: ClusterRef, other: ClusterRef) {
    // iter-mut: JS Set iteration; the current element is deleted by the body (live cursor).
    let mut cur = e.rail().clusters.get(other).stations.cursor();
    while let Some(s) = e.rail().clusters.get(other).stations.next_live(&mut cur) {
        cluster_add_station_env(e, c, s);
    }
}

/// `cluster.hasAnyTradeDestination(player)` (`:250-257`).
pub(crate) fn cluster_has_any_trade_destination_env<E: RailRead + ?Sized>(
    e: &E,
    c: ClusterRef,
    player: SmallId,
) -> bool {
    for &station in e.rail().clusters.get(c).trade_stations.iter() {
        if station_trade_available_env(e, station, player) {
            return true;
        }
    }
    false
}

/// `cluster.randomTradeDestination(player, random)` (`:259-277`): reservoir sampling, one
/// `nextInt(0, eligibleSeen)` draw per eligible station.
pub(crate) fn cluster_random_trade_destination_env<E: RailRead + ?Sized>(
    e: &E,
    c: ClusterRef,
    player: SmallId,
    random: &mut Prng,
) -> Option<StationId> {
    let mut selected: Option<StationId> = None;
    let mut eligible_seen: i32 = 0; // int: a count

    for &station in e.rail().clusters.get(c).trade_stations.iter() {
        if !station_trade_available_env(e, station, player) {
            continue;
        }
        eligible_seen += 1;

        // Reservoir sampling: keep each eligible station with probability 1/eligibleSeen.
        if random.next_int(0.0, f64::from(eligible_seen)) == 0.0 {
            selected = Some(station);
        }
    }

    selected
}

/// `cluster.availableForTrade(player)` (`:279-287`): a new `Set` in `tradeStations` order.
pub(crate) fn cluster_available_for_trade_env<E: RailRead + ?Sized>(
    e: &E,
    c: ClusterRef,
    player: SmallId,
) -> Vec<StationId> {
    let mut trading_stations = Vec::new();
    for &station in e.rail().clusters.get(c).trade_stations.iter() {
        if station_trade_available_env(e, station, player) {
            trading_stations.push(station);
        }
    }
    trading_stations
}

impl State {
    /// `cluster.hasAnyTradeDestination(player)` (`:250-257`).
    pub fn cluster_has_any_trade_destination(&self, c: ClusterRef, player: SmallId) -> bool {
        cluster_has_any_trade_destination_env(self, c, player)
    }
    /// `cluster.availableForTrade(player)` (`:279-287`).
    pub fn cluster_available_for_trade(&self, c: ClusterRef, player: SmallId) -> Vec<StationId> {
        cluster_available_for_trade_env(self, c, player)
    }
}

impl Ctx<'_> {
    /// `cluster.addStation(station)` (`:225-231`).
    pub fn cluster_add_station(&mut self, c: ClusterRef, s: StationId) {
        cluster_add_station_env(self, c, s);
    }
    /// `cluster.removeStation(station)` (`:233-236`).
    pub fn cluster_remove_station(&mut self, c: ClusterRef, s: StationId) {
        self.st.rail.clusters.get_mut(c).remove_station(s);
    }
    /// `cluster.addStations(stations)` (`:238-242`).
    pub fn cluster_add_stations(&mut self, c: ClusterRef, stations: &[StationId]) {
        cluster_add_stations_env(self, c, stations);
    }
    /// `cluster.merge(other)` (`:244-248`).
    pub fn cluster_merge(&mut self, c: ClusterRef, other: ClusterRef) {
        cluster_merge_env(self, c, other);
    }
}

/// `cluster.randomTradeDestination(player, random)` (`:259-277`): draws from the caller's PRNG.
pub fn cluster_random_trade_destination(
    st: &State,
    c: ClusterRef,
    player: SmallId,
    random: &mut Prng,
) -> Option<StationId> {
    cluster_random_trade_destination_env(st, c, player, random)
}

#[cfg(test)]
mod tests {
    //! T0: `tests/core/game/TrainStation.test.ts` (the `TrainStation` block; the
    //! `EngineConfig.trainGold` block belongs to P3's config) on the fake world.
    use super::*;
    use crate::rail::testenv::{Call, World};

    fn station_of(w: &mut World, ty: UnitType, owner: u16, tile: TileRef) -> StationId {
        let u = w.unit(ty, owner, tile);
        w.station(u)
    }

    #[test]
    fn handles_city_stop() {
        let mut w = World::new();
        let s = station_of(&mut w, UnitType::City, 1, 7);
        station_on_train_stop_env(&mut w, s, SmallId(1), 0);
        // Same owner: rel "self" → 10k, paid once to the owner (no external share).
        assert_eq!(
            w.calls,
            [
                Call::AddGold(SmallId(1), 10_000, 7),
                Call::AddTrainGold(SmallId(1), 10_000),
                Call::StatsSelf(SmallId(1), 10_000),
            ]
        );
    }

    #[test]
    fn handles_allied_trade() {
        let mut w = World::new();
        w.allied.insert((2, 1));
        let s = station_of(&mut w, UnitType::Port, 1, 7);
        station_on_train_stop_env(&mut w, s, SmallId(2), 0);
        // rel(trainOwner=2, stationOwner=1) = "ally" (35k), shared with the station owner.
        assert_eq!(
            w.calls,
            [
                Call::AddGold(SmallId(1), 35_000, 7),
                Call::AddTrainGold(SmallId(1), 35_000),
                Call::StatsExternal(SmallId(1), 35_000),
                Call::AddGold(SmallId(2), 35_000, 7),
                Call::AddTrainGold(SmallId(2), 35_000),
                Call::StatsSelf(SmallId(2), 35_000),
            ]
        );
    }

    #[test]
    fn records_external_trade_on_the_station_owner() {
        let mut w = World::new();
        let s = station_of(&mut w, UnitType::City, 1, 7);
        station_on_train_stop_env(&mut w, s, SmallId(2), 0);
        // rel "other" (25k): both owners paid, external trade on the station owner.
        assert!(w.calls.contains(&Call::AddGold(SmallId(1), 25_000, 7)));
        assert!(w.calls.contains(&Call::AddGold(SmallId(2), 25_000, 7)));
        assert!(w.calls.contains(&Call::AddTrainGold(SmallId(1), 25_000)));
        assert!(w.calls.contains(&Call::AddTrainGold(SmallId(2), 25_000)));
        assert!(w.calls.contains(&Call::StatsExternal(SmallId(1), 25_000)));
        assert!(w.calls.contains(&Call::StatsSelf(SmallId(2), 25_000)));
        // Team takes precedence over alliance.
        let mut w = World::new();
        w.same_team.insert((2, 1));
        w.allied.insert((2, 1));
        let s = station_of(&mut w, UnitType::City, 1, 7);
        station_on_train_stop_env(&mut w, s, SmallId(2), 0);
        assert!(w.calls.contains(&Call::StatsSelf(SmallId(2), 25_000)), "team: 25k, not ally 35k");
    }

    #[test]
    fn passes_trade_stops_visited_to_train_gold() {
        let mut w = World::new();
        let s = station_of(&mut w, UnitType::City, 1, 7);
        station_on_train_stop_env(&mut w, s, SmallId(1), 11);
        // self base 10k, 11 stops → effective 2 → 0 → floored at 5k.
        assert!(w.calls.contains(&Call::AddTrainGold(SmallId(1), 5_000)));
    }

    #[test]
    fn factory_and_other_stops_pay_nothing() {
        let mut w = World::new();
        let f = station_of(&mut w, UnitType::Factory, 1, 7);
        let silo = station_of(&mut w, UnitType::MissileSilo, 1, 8);
        station_on_train_stop_env(&mut w, f, SmallId(2), 0);
        station_on_train_stop_env(&mut w, silo, SmallId(2), 0);
        assert!(w.calls.is_empty());
    }

    #[test]
    fn checks_trade_availability() {
        let mut w = World::new();
        let s = station_of(&mut w, UnitType::City, 1, 7);
        assert!(station_trade_available_env(&w, s, SmallId(1)), "same owner");
        assert!(station_trade_available_env(&w, s, SmallId(2)), "canTrade");
        w.no_trade.insert((1, 2));
        assert!(!station_trade_available_env(&w, s, SmallId(2)), "embargo");
        assert!(station_trade_available_env(&w, s, SmallId(1)), "same owner ignores canTrade");
    }

    #[test]
    fn adds_and_retrieves_neighbors() {
        let mut w = World::new();
        let a = station_of(&mut w, UnitType::City, 1, 1);
        let b = station_of(&mut w, UnitType::City, 1, 2);
        let r = w.link(a, b, &[], 0);
        assert_eq!(station_neighbors_env(&w, a), [b]);
        assert_eq!(station_neighbors_env(&w, b), [a]);
        assert_eq!(w.rail.station_obj(a).railroad_by_neighbor.get(&b), Some(&r));
    }

    #[test]
    fn removes_neighboring_rail() {
        let mut w = World::new();
        let a = station_of(&mut w, UnitType::City, 1, 1);
        let b = station_of(&mut w, UnitType::City, 1, 2);
        w.link(a, b, &[11], 0);
        assert_eq!(w.rail.station_obj(a).get_railroads().len(), 1);
        station_remove_neighboring_rails_env(&mut w, a, b);
        assert_eq!(w.rail.station_obj(a).get_railroads().len(), 0);
        assert!(w.rail.station_obj(a).railroad_by_neighbor.is_empty());
        // Not a neighbour: no-op.
        station_remove_neighboring_rails_env(&mut w, a, b);
    }

    #[test]
    fn assigns_and_retrieves_cluster() {
        let mut w = World::new();
        let s = station_of(&mut w, UnitType::City, 1, 1);
        let c1 = w.rail.new_cluster();
        let c2 = w.rail.new_cluster();
        cluster_add_station_env(&mut w, c1, s);
        assert_eq!(w.rail.station_obj(s).get_cluster(), Some(c1));
        assert!(w.rail.clusters.get(c1).has(s));
        assert!(w.rail.clusters.get(c1).trade_stations.contains(&s), "City is a trade station");
        // setCluster to another cluster removes it from the old one.
        cluster_add_station_env(&mut w, c2, s);
        assert!(!w.rail.clusters.get(c1).has(s) && !w.rail.clusters.get(c1).trade_stations.contains(&s));
        // setCluster(same) keeps it.
        station_set_cluster_env(&mut w, s, Some(c2));
        assert!(w.rail.clusters.get(c2).has(s));
        station_set_cluster_env(&mut w, s, None);
        assert!(!w.rail.clusters.get(c2).has(s));
        assert_eq!(w.rail.station_obj(s).get_cluster(), None);
    }

    #[test]
    fn returns_tile_and_active_status() {
        let mut w = World::new();
        let u = w.unit(UnitType::City, 1, 42);
        let s = w.station(u);
        assert_eq!(station_tile_env(&w, s), 42);
        assert!(station_is_active_env(&w, s));
        w.unit_data_mut(u).active = false;
        assert!(!station_is_active_env(&w, s));
    }

    #[test]
    fn cluster_merge_moves_every_station() {
        // iter-mut test: merge() removes the visited station from `other` while walking it.
        let mut w = World::new();
        let c1 = w.rail.new_cluster();
        let c2 = w.rail.new_cluster();
        let mut ss = Vec::new();
        for i in 0..11 {
            let ty = if i % 3 == 0 { UnitType::Factory } else { UnitType::City };
            let s = station_of(&mut w, ty, 1, i);
            cluster_add_station_env(&mut w, c2, s);
            ss.push(s);
        }
        cluster_merge_env(&mut w, c1, c2);
        assert_eq!(w.rail.clusters.get(c1).stations.iter().copied().collect::<Vec<_>>(), ss);
        assert_eq!(w.rail.clusters.get(c2).size(), 0);
        assert!(w.rail.clusters.get(c2).trade_stations.is_empty());
        assert_eq!(w.rail.clusters.get(c1).trade_stations.len(), 7);
    }

    #[test]
    fn trade_destinations() {
        let mut w = World::new();
        let c = w.rail.new_cluster();
        let a = station_of(&mut w, UnitType::City, 1, 1);
        let f = station_of(&mut w, UnitType::Factory, 1, 2);
        let b = station_of(&mut w, UnitType::Port, 2, 3);
        let d = station_of(&mut w, UnitType::City, 3, 4);
        for s in [a, f, b, d] {
            cluster_add_station_env(&mut w, c, s);
        }
        w.no_trade.insert((3, 1));
        // Player 1 trades with a (own), b (canTrade 2→1); d's owner 3 refuses.
        assert_eq!(cluster_available_for_trade_env(&w, c, SmallId(1)), [a, b]);
        assert!(cluster_has_any_trade_destination_env(&w, c, SmallId(1)));
        // Reservoir sampling: one nextInt(0, k) draw per eligible station, last zero wins.
        let mut r1 = Prng::new(5.0);
        let mut r2 = Prng::new(5.0);
        let got = cluster_random_trade_destination_env(&w, c, SmallId(1), &mut r1);
        let mut expect = None;
        for (k, s) in [a, b].into_iter().enumerate() {
            if r2.next_int(0.0, (k + 1) as f64) == 0.0 {
                expect = Some(s);
            }
        }
        assert_eq!(got, expect);
        assert_eq!(r1.get_state(), r2.get_state(), "same number of draws");
        // No eligible station: no draw, null.
        let c2 = w.rail.new_cluster();
        cluster_add_station_env(&mut w, c2, d);
        let before = r1.get_state();
        assert_eq!(cluster_random_trade_destination_env(&w, c2, SmallId(1), &mut r1), None);
        assert_eq!(r1.get_state(), before);
        assert!(!cluster_has_any_trade_destination_env(&w, c2, SmallId(1)));
    }
}
