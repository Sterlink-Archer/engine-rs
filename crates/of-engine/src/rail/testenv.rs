//! Test-only fake world implementing the rail env (`rail::env`). Owner: **P5**.
//!
//! It stands in for the packages the rail logic calls (P1 game, P2 player/unit/stats, P3
//! map/config, P4b path finding) so the rail port can be unit-tested before they land. Each
//! fake method implements only the TS semantics that `rail::env` documents for it, simplified
//! where the upstream unit tests themselves use mocks:
//! - map: `x = t % w`, `y = t / w` (`GameMapImpl.ts`), `w = 100` (the `plains` test map);
//! - `nearbyUnits`: every active unit of a listed type within range, `distSquared` from tile
//!   coordinates, in unit-creation order (a stand-in for UnitGrid order);
//! - `findStationsPath`: BFS over `station.neighbors()` restricted to `to`'s cluster
//!   (`PathFinder.Station.ts:17-33`); on the linear test graphs it equals the A* path;
//! - `findTilePath`: an override table, else an L-shaped path (x first) including both ends;
//! - `trainGold`: the upstream schedule (`TrainStation.test.ts` "trainGold trade stop
//!   penalty"): base self 10k / ally 35k / team and other 25k, 5k less per stop past the
//!   ninth, floored at 5k.

use crate::api::types::{Gold, UnitType};
use crate::config::engine_config::TrainRelation;
use crate::exec::Exec;
use crate::ids::{RailroadId, SmallId, StationId, TileRef, UnitKey, UnitRef};
use crate::map::unit_grid::NearbyUnit;
use crate::rail::env::{RailEnv, RailRead};
use crate::rail::network::RailNetwork;
use crate::rail::railroad::Railroad;
use crate::rail::station::{TrainStation, station_add_railroad_env, station_neighbors_env};
use crate::state::unit::UnitParams;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Clone, Debug)]
pub(crate) struct FakeUnit {
    pub id: u32,
    pub ty: UnitType,
    pub owner: SmallId,
    pub tile: TileRef,
    pub active: bool,
    pub has_train_station: bool,
    pub level: i32,
    pub loaded: Option<bool>,
    pub reached_target: bool,
    pub params: UnitParams,
    /// Every tile passed to `move`, in order.
    pub moves: Vec<TileRef>,
}

/// One recorded write (for assertions on call order).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Call {
    SetTrainStation(u32, bool),
    AddGold(SmallId, Gold, TileRef),
    AddTrainGold(SmallId, Gold),
    StatsExternal(SmallId, Gold),
    StatsSelf(SmallId, Gold),
    Delete(u32),
}

pub(crate) struct World {
    pub rail: RailNetwork,
    pub w: u32,
    pub units: Vec<FakeUnit>,
    pub ticks: i32,
    /// `player.canTrade(other)` = false for these (p, other) pairs.
    pub no_trade: BTreeSet<(u16, u16)>,
    pub same_team: BTreeSet<(u16, u16)>,
    pub allied: BTreeSet<(u16, u16)>,
    pub min_range: f64,
    pub max_range: f64,
    pub railroad_max_size: f64,
    pub spawn_rate: f64,
    pub can_build: bool,
    pub tile_paths: BTreeMap<(TileRef, TileRef), Vec<TileRef>>,
    pub calls: Vec<Call>,
    pub train_gold: BTreeMap<u16, Gold>,
    pub motion_plan: Vec<u32>,
    pub execs: Vec<Exec>,
    pub tile_path_queries: Vec<(TileRef, TileRef)>,
}

impl World {
    pub fn new() -> Self {
        World {
            rail: RailNetwork::new(),
            w: 100,
            units: Vec::new(),
            ticks: 0,
            no_trade: BTreeSet::new(),
            same_team: BTreeSet::new(),
            allied: BTreeSet::new(),
            min_range: 10.0,
            max_range: 80.0,
            railroad_max_size: 100.0,
            spawn_rate: 1.0,
            can_build: true,
            tile_paths: BTreeMap::new(),
            calls: Vec::new(),
            train_gold: BTreeMap::new(),
            motion_plan: Vec::new(),
            execs: Vec::new(),
            tile_path_queries: Vec::new(),
        }
    }

    fn u(&self, u: UnitRef) -> &FakeUnit {
        assert_eq!(u.generation, 0, "fake units have generation 0");
        &self.units[u.slot as usize]
    }
    fn u_mut(&mut self, u: UnitRef) -> &mut FakeUnit {
        &mut self.units[u.slot as usize]
    }

    /// Build a unit (ids from 1, in creation order).
    pub fn unit(&mut self, ty: UnitType, owner: u16, tile: TileRef) -> UnitRef {
        let slot = self.units.len() as u32;
        self.units.push(FakeUnit {
            id: slot + 1,
            ty,
            owner: SmallId(owner),
            tile,
            active: true,
            has_train_station: false,
            level: 1,
            loaded: None,
            reached_target: false,
            params: UnitParams::default(),
            moves: Vec::new(),
        });
        UnitRef { slot, generation: 0 }
    }
    pub fn unit_data(&self, u: UnitRef) -> &FakeUnit {
        self.u(u)
    }
    pub fn unit_data_mut(&mut self, u: UnitRef) -> &mut FakeUnit {
        self.u_mut(u)
    }

    /// `new TrainStation(game, unit)` + `stationManager.addStation(station)`.
    pub fn station(&mut self, unit: UnitRef) -> StationId {
        let key = UnitKey(self.u(unit).id);
        self.rail.station_manager.add_station(key, TrainStation::new(StationId(0), unit))
    }

    /// `new Railroad(a, b, tiles, id)` + `a.addRailroad(r)` + `b.addRailroad(r)` (the upstream
    /// tests' `link` helper; not registered in the grid).
    pub fn link(&mut self, a: StationId, b: StationId, tiles: &[TileRef], id: u32) -> RailroadId {
        let id = RailroadId(id);
        let r = self.rail.railroads.alloc(Railroad::new(a, b, tiles.to_vec(), id));
        self.rail.railroad_by_id.insert(id, r);
        station_add_railroad_env(self, a, id);
        station_add_railroad_env(self, b, id);
        id
    }

    pub fn train_gold_of(&self, p: u16) -> Gold {
        self.train_gold.get(&p).copied().unwrap_or(0)
    }
}

impl RailRead for World {
    fn rail(&self) -> &RailNetwork {
        &self.rail
    }
    fn tile_x(&self, t: TileRef) -> u32 {
        t % self.w
    }
    fn tile_y(&self, t: TileRef) -> u32 {
        t / self.w
    }
    fn unit_id(&self, u: UnitRef) -> u32 {
        self.u(u).id
    }
    fn unit_type(&self, u: UnitRef) -> UnitType {
        self.u(u).ty
    }
    fn unit_owner(&self, u: UnitRef) -> SmallId {
        self.u(u).owner
    }
    fn unit_tile(&self, u: UnitRef) -> TileRef {
        self.u(u).tile
    }
    fn unit_is_active(&self, u: UnitRef) -> bool {
        self.u(u).active
    }
    fn unit_has_train_station(&self, u: UnitRef) -> bool {
        self.u(u).has_train_station
    }
    fn unit_level(&self, u: UnitRef) -> i32 {
        self.u(u).level
    }
    fn player_can_trade(&self, p: SmallId, other: SmallId) -> bool {
        !self.no_trade.contains(&(p.0, other.0))
    }
    fn player_is_on_same_team(&self, p: SmallId, other: SmallId) -> bool {
        self.same_team.contains(&(p.0, other.0))
    }
    fn player_is_allied_with(&self, p: SmallId, other: SmallId) -> bool {
        self.allied.contains(&(p.0, other.0))
    }
    fn player_unit_count(&self, p: SmallId, t: UnitType) -> i32 {
        self.units.iter().filter(|u| u.active && u.owner == p && u.ty == t).count() as i32
    }
    fn ticks(&self) -> i32 {
        self.ticks
    }
    fn unit_count(&self, t: UnitType) -> i32 {
        self.units.iter().filter(|u| u.active && u.ty == t).count() as i32
    }
    fn nearby_units(&self, tile: TileRef, search_range: f64, types: &[UnitType]) -> Vec<NearbyUnit> {
        let (x, y) = (f64::from(self.tile_x(tile)), f64::from(self.tile_y(tile)));
        let mut out = Vec::new();
        for (slot, u) in self.units.iter().enumerate() {
            if !u.active || !types.contains(&u.ty) {
                continue;
            }
            let dx = f64::from(self.tile_x(u.tile)) - x;
            let dy = f64::from(self.tile_y(u.tile)) - y;
            let d = dx * dx + dy * dy;
            if d <= search_range * search_range {
                out.push(NearbyUnit { unit: UnitRef { slot: slot as u32, generation: 0 }, dist_squared: d });
            }
        }
        out
    }
    fn train_station_max_range(&self) -> f64 {
        self.max_range
    }
    fn train_station_min_range(&self) -> f64 {
        self.min_range
    }
    fn railroad_max_size(&self) -> f64 {
        self.railroad_max_size
    }
    fn train_spawn_rate(&self, _f: f64, _u: f64) -> f64 {
        self.spawn_rate
    }
    fn train_gold(&self, rel: TrainRelation, cities_visited: f64, _player: SmallId) -> Gold {
        let base: Gold = match rel {
            TrainRelation::SelfOwned => 10_000,
            TrainRelation::Ally => 35_000,
            TrainRelation::Team | TrainRelation::Other => 25_000,
        };
        let effective = if cities_visited >= 10.0 { cities_visited as i64 - 9 } else { 0 };
        (base - 5_000 * effective).max(5_000)
    }
}

impl RailEnv for World {
    fn rail_mut(&mut self) -> &mut RailNetwork {
        &mut self.rail
    }
    fn unit_set_train_station(&mut self, u: UnitRef, v: bool) {
        let id = self.u(u).id;
        self.u_mut(u).has_train_station = v;
        self.calls.push(Call::SetTrainStation(id, v));
    }
    fn unit_move(&mut self, u: UnitRef, t: TileRef) {
        let fu = self.u_mut(u);
        fu.tile = t;
        fu.moves.push(t);
    }
    fn unit_delete(&mut self, u: UnitRef, _display_message: bool) {
        assert!(self.u(u).active, "TS deletes only active units here");
        self.u_mut(u).active = false;
        let id = self.u(u).id;
        self.calls.push(Call::Delete(id));
    }
    fn unit_set_loaded(&mut self, u: UnitRef, loaded: bool) {
        self.u_mut(u).loaded = Some(loaded);
    }
    fn unit_set_reached_target(&mut self, u: UnitRef) {
        self.u_mut(u).reached_target = true;
    }
    fn player_can_build(&mut self, _p: SmallId, _t: UnitType, tile: TileRef) -> Option<TileRef> {
        if self.can_build { Some(tile) } else { None }
    }
    fn player_build_unit(&mut self, p: SmallId, t: UnitType, tile: TileRef, params: UnitParams) -> UnitRef {
        let u = self.unit(t, p.0, tile);
        let loaded = params.loaded.flatten();
        let fu = self.u_mut(u);
        fu.loaded = loaded;
        fu.params = params;
        u
    }
    fn player_add_gold(&mut self, p: SmallId, gold: Gold, tile: TileRef) {
        self.calls.push(Call::AddGold(p, gold, tile));
    }
    fn player_add_train_gold(&mut self, p: SmallId, gold: Gold) {
        *self.train_gold.entry(p.0).or_insert(0) += gold;
        self.calls.push(Call::AddTrainGold(p, gold));
    }
    fn stats_train_external_trade(&mut self, p: SmallId, gold: Gold) {
        self.calls.push(Call::StatsExternal(p, gold));
    }
    fn stats_train_self_trade(&mut self, p: SmallId, gold: Gold) {
        self.calls.push(Call::StatsSelf(p, gold));
    }
    fn record_motion_plan(&mut self, unit_id: u32) {
        self.motion_plan.push(unit_id);
    }
    fn add_execution(&mut self, e: Exec) {
        self.execs.push(e);
    }
    fn find_tile_path(&mut self, from: TileRef, to: TileRef) -> Vec<TileRef> {
        self.tile_path_queries.push((from, to));
        if let Some(p) = self.tile_paths.get(&(from, to)) {
            return p.clone();
        }
        let (mut x, y0) = (self.tile_x(from), self.tile_y(from));
        let (tx, ty) = (self.tile_x(to), self.tile_y(to));
        let mut path = vec![from];
        while x != tx {
            x = if x < tx { x + 1 } else { x - 1 };
            path.push(y0 * self.w + x);
        }
        let mut y = y0;
        while y != ty {
            y = if y < ty { y + 1 } else { y - 1 };
            path.push(y * self.w + x);
        }
        path
    }
    fn find_stations_path(&mut self, from: StationId, to: StationId) -> Vec<StationId> {
        // PathFinder.Station.ts:20-23: `from` must share `to`'s cluster (null === null too).
        let to_cluster = self.rail.station_obj(to).cluster;
        if self.rail.station_obj(from).cluster != to_cluster {
            return Vec::new();
        }
        let mut prev: BTreeMap<u32, u32> = BTreeMap::new();
        let mut seen: BTreeSet<u32> = BTreeSet::from([from.0]);
        let mut q = VecDeque::from([from]);
        while let Some(s) = q.pop_front() {
            if s == to {
                let mut path = vec![to];
                let mut c = to.0;
                while let Some(&p) = prev.get(&c) {
                    path.push(StationId(p));
                    c = p;
                }
                path.reverse();
                return path;
            }
            for n in station_neighbors_env(self, s) {
                if seen.insert(n.0) {
                    prev.insert(n.0, s.0);
                    q.push_back(n);
                }
            }
        }
        Vec::new()
    }
}
