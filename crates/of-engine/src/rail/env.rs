//! The `Game` surface the rail package touches, as two traits. Owner: **P5**.
//!
//! TS injects this surface: `RailNetworkImpl` takes `game` and a `RailPathFinderService`
//! (`RailNetworkImpl.ts:117-120,312-318`), and `TrainStation`/`TrainExecution`/
//! `TrainStationExecution`/`FactoryExecution` call a handful of `mg`, `Unit` and `Player`
//! methods. The port keeps that seam explicit:
//!
//! - [`RailRead`]: every read (rail data, `GameMap.x/y`, unit and player getters, config).
//! - [`RailEnv`]: `RailRead` plus every write and the two path queries.
//!
//! Production code is generic over these traits and runs on `State` (reads) and `Ctx`
//! (reads + writes): each method below delegates to the frozen API of the package that
//! owns it (P1 game, P2 player/unit/stats, P3 map/config, P4b path finding), so the rail
//! logic never reads another package's raw fields. The unit tests implement the traits on
//! a small fake world (`rail::testenv`) so the rail logic is checked before the other
//! packages land; the fake mirrors only the TS semantics each method documents.
//!
//! Every method maps to exactly one TS call; its doc names it.

use crate::api::types::{Gold, UnitType};
use crate::config::engine_config::TrainRelation;
use crate::ctx::Ctx;
use crate::exec::Exec;
use crate::ids::{SmallId, StationId, TileRef, UnitRef};
use crate::map::unit_grid::NearbyUnit;
use crate::path::pathfinder::{RailStepper, StationStepper};
use crate::rail::network::RailNetwork;
use crate::state::State;
use crate::state::stats::StatNum;
use crate::state::unit::UnitParams;

/// Read-only half of the rail package's `Game` surface.
pub(crate) trait RailRead {
    /// `game.railNetwork()` data.
    fn rail(&self) -> &RailNetwork;
    /// `game.x(tile)`.
    fn tile_x(&self, t: TileRef) -> u32;
    /// `game.y(tile)`.
    fn tile_y(&self, t: TileRef) -> u32;
    /// `unit.id()`.
    fn unit_id(&self, u: UnitRef) -> u32;
    /// `unit.type()`.
    fn unit_type(&self, u: UnitRef) -> UnitType;
    /// `unit.owner()` (always a player for the structures and trains seen here).
    fn unit_owner(&self, u: UnitRef) -> SmallId;
    /// `unit.tile()`.
    fn unit_tile(&self, u: UnitRef) -> TileRef;
    /// `unit.isActive()`.
    fn unit_is_active(&self, u: UnitRef) -> bool;
    /// `unit.hasTrainStation()`.
    fn unit_has_train_station(&self, u: UnitRef) -> bool;
    /// `unit.level()`.
    fn unit_level(&self, u: UnitRef) -> i32;
    /// `player.canTrade(other)`.
    fn player_can_trade(&self, p: SmallId, other: SmallId) -> bool;
    /// `player.isOnSameTeam(other)`.
    fn player_is_on_same_team(&self, p: SmallId, other: SmallId) -> bool;
    /// `player.isAlliedWith(other)`.
    fn player_is_allied_with(&self, p: SmallId, other: SmallId) -> bool;
    /// `player.unitCount(type)`.
    fn player_unit_count(&self, p: SmallId, t: UnitType) -> i32;
    /// `mg.ticks()`.
    fn ticks(&self) -> i32;
    /// `mg.unitCount(type)`.
    fn unit_count(&self, t: UnitType) -> i32;
    /// `game.nearbyUnits(tile, searchRange, types)` (no predicate, `includeUnderConstruction`
    /// false): UnitGrid order, `distSquared` per unit.
    fn nearby_units(&self, tile: TileRef, search_range: f64, types: &[UnitType]) -> Vec<NearbyUnit>;
    /// `config().trainStationMaxRange()`.
    fn train_station_max_range(&self) -> f64;
    /// `config().trainStationMinRange()`.
    fn train_station_min_range(&self) -> f64;
    /// `config().railroadMaxSize()`.
    fn railroad_max_size(&self) -> f64;
    /// `config().trainSpawnRate(numPlayerFactories, numTrainUnits)`.
    fn train_spawn_rate(&self, num_player_factories: f64, num_train_units: f64) -> f64;
    /// `config().trainGold(rel, citiesVisited, player)`.
    fn train_gold(&self, rel: TrainRelation, cities_visited: f64, player: SmallId) -> Gold;
}

/// Read-write half: every state write the rail package makes outside `RailNetwork`, plus the
/// `RailPathFinderService` queries (they take scratch).
pub(crate) trait RailEnv: RailRead {
    fn rail_mut(&mut self) -> &mut RailNetwork;
    /// `player.unitCount(type)` with TS's memo filled (`PlayerImpl.ts:530-548`); the same value
    /// as [`RailRead::player_unit_count`] (P3 perf, t066).
    fn player_unit_count_memo(&mut self, p: SmallId, t: UnitType) -> i32 {
        self.player_unit_count(p, t)
    }
    /// `mg.unitCount(type)` with TS's memo filled (`GameImpl.ts:417-428`); the same value as
    /// [`RailRead::unit_count`] (P3 perf, t066).
    fn unit_count_memo(&mut self, t: UnitType) -> i32 {
        self.unit_count(t)
    }
    /// `unit.setTrainStation(v)` (writes the flag and calls `toUpdate()`, `UnitImpl.ts:735-738`).
    fn unit_set_train_station(&mut self, u: UnitRef, v: bool);
    /// `unit.move(tile)`.
    fn unit_move(&mut self, u: UnitRef, t: TileRef);
    /// `unit.delete(displayMessage)` (no destroyer).
    fn unit_delete(&mut self, u: UnitRef, display_message: bool);
    /// `unit.setLoaded(loaded)`.
    fn unit_set_loaded(&mut self, u: UnitRef, loaded: bool);
    /// `unit.setReachedTarget()`.
    fn unit_set_reached_target(&mut self, u: UnitRef);
    /// `player.canBuild(type, tile)` (`TileRef | false` → `Option`).
    fn player_can_build(&mut self, p: SmallId, t: UnitType, tile: TileRef) -> Option<TileRef>;
    /// `player.buildUnit(type, tile, params)`.
    fn player_build_unit(&mut self, p: SmallId, t: UnitType, tile: TileRef, params: UnitParams) -> UnitRef;
    /// `player.addGold(gold, tile)`.
    fn player_add_gold(&mut self, p: SmallId, gold: Gold, tile: TileRef);
    /// `player.addTrainGold(gold)`.
    fn player_add_train_gold(&mut self, p: SmallId, gold: Gold);
    /// `mg.stats().trainExternalTrade(player, gold)`.
    fn stats_train_external_trade(&mut self, p: SmallId, gold: Gold);
    /// `mg.stats().trainSelfTrade(player, gold)`.
    fn stats_train_self_trade(&mut self, p: SmallId, gold: Gold);
    /// The `planDrivenUnitIds.add(id)` part of `mg.recordMotionPlan(record)` (`GameImpl.ts:702-715`).
    fn record_motion_plan(&mut self, unit_id: u32);
    /// `mg.addExecution(exec)`.
    fn add_execution(&mut self, e: Exec);
    /// `pathService.findTilePath(from, to)` = `PathFinding.Rail(game).findPath(from, to) ?? []`
    /// (`RailNetworkImpl.ts:281-284`).
    fn find_tile_path(&mut self, from: TileRef, to: TileRef) -> Vec<TileRef>;
    /// `pathService.findStationsPath(from, to)` = `PathFinding.Stations(game).findPath(from, to) ?? []`
    /// (`RailNetworkImpl.ts:286-288`).
    fn find_stations_path(&mut self, from: StationId, to: StationId) -> Vec<StationId>;
}

// ---- production: `State` (reads) ----

impl RailRead for State {
    #[inline]
    fn rail(&self) -> &RailNetwork {
        &self.rail
    }
    #[inline]
    fn tile_x(&self, t: TileRef) -> u32 {
        self.map.x(t)
    }
    #[inline]
    fn tile_y(&self, t: TileRef) -> u32 {
        self.map.y(t)
    }
    #[inline]
    fn unit_id(&self, u: UnitRef) -> u32 {
        self.unit(u).id()
    }
    #[inline]
    fn unit_type(&self, u: UnitRef) -> UnitType {
        self.unit(u).unit_type()
    }
    #[inline]
    fn unit_owner(&self, u: UnitRef) -> SmallId {
        self.unit(u).owner()
    }
    #[inline]
    fn unit_tile(&self, u: UnitRef) -> TileRef {
        self.unit(u).tile()
    }
    #[inline]
    fn unit_is_active(&self, u: UnitRef) -> bool {
        self.unit(u).is_active()
    }
    #[inline]
    fn unit_has_train_station(&self, u: UnitRef) -> bool {
        self.unit(u).has_train_station()
    }
    #[inline]
    fn unit_level(&self, u: UnitRef) -> i32 {
        self.unit(u).level()
    }
    #[inline]
    fn player_can_trade(&self, p: SmallId, other: SmallId) -> bool {
        self.player(p).can_trade(other)
    }
    #[inline]
    fn player_is_on_same_team(&self, p: SmallId, other: SmallId) -> bool {
        self.player(p).is_on_same_team(other)
    }
    #[inline]
    fn player_is_allied_with(&self, p: SmallId, other: SmallId) -> bool {
        self.player(p).is_allied_with(other)
    }
    #[inline]
    fn player_unit_count(&self, p: SmallId, t: UnitType) -> i32 {
        self.player(p).unit_count(t)
    }
    #[inline]
    fn ticks(&self) -> i32 {
        State::ticks(self)
    }
    #[inline]
    fn unit_count(&self, t: UnitType) -> i32 {
        State::unit_count(self, t)
    }
    fn nearby_units(&self, tile: TileRef, search_range: f64, types: &[UnitType]) -> Vec<NearbyUnit> {
        // TS passes no predicate (`undefined`): every candidate passes.
        State::nearby_units(self, tile, search_range, types, |_, _, _| true, false)
    }
    #[inline]
    fn train_station_max_range(&self) -> f64 {
        self.config().train_station_max_range()
    }
    #[inline]
    fn train_station_min_range(&self) -> f64 {
        self.config().train_station_min_range()
    }
    #[inline]
    fn railroad_max_size(&self) -> f64 {
        self.config().railroad_max_size()
    }
    #[inline]
    fn train_spawn_rate(&self, num_player_factories: f64, num_train_units: f64) -> f64 {
        self.config().train_spawn_rate(num_player_factories, num_train_units)
    }
    #[inline]
    fn train_gold(&self, rel: TrainRelation, cities_visited: f64, player: SmallId) -> Gold {
        self.config().train_gold(rel, cities_visited, &self.player(player))
    }
}

// ---- production: `Ctx` (reads through its state, writes through the owners' APIs) ----

impl RailRead for Ctx<'_> {
    #[inline]
    fn rail(&self) -> &RailNetwork {
        &self.st.rail
    }
    #[inline]
    fn tile_x(&self, t: TileRef) -> u32 {
        RailRead::tile_x(&*self.st, t)
    }
    #[inline]
    fn tile_y(&self, t: TileRef) -> u32 {
        RailRead::tile_y(&*self.st, t)
    }
    #[inline]
    fn unit_id(&self, u: UnitRef) -> u32 {
        RailRead::unit_id(&*self.st, u)
    }
    #[inline]
    fn unit_type(&self, u: UnitRef) -> UnitType {
        RailRead::unit_type(&*self.st, u)
    }
    #[inline]
    fn unit_owner(&self, u: UnitRef) -> SmallId {
        RailRead::unit_owner(&*self.st, u)
    }
    #[inline]
    fn unit_tile(&self, u: UnitRef) -> TileRef {
        RailRead::unit_tile(&*self.st, u)
    }
    #[inline]
    fn unit_is_active(&self, u: UnitRef) -> bool {
        RailRead::unit_is_active(&*self.st, u)
    }
    #[inline]
    fn unit_has_train_station(&self, u: UnitRef) -> bool {
        RailRead::unit_has_train_station(&*self.st, u)
    }
    #[inline]
    fn unit_level(&self, u: UnitRef) -> i32 {
        RailRead::unit_level(&*self.st, u)
    }
    #[inline]
    fn player_can_trade(&self, p: SmallId, other: SmallId) -> bool {
        RailRead::player_can_trade(&*self.st, p, other)
    }
    #[inline]
    fn player_is_on_same_team(&self, p: SmallId, other: SmallId) -> bool {
        RailRead::player_is_on_same_team(&*self.st, p, other)
    }
    #[inline]
    fn player_is_allied_with(&self, p: SmallId, other: SmallId) -> bool {
        RailRead::player_is_allied_with(&*self.st, p, other)
    }
    #[inline]
    fn player_unit_count(&self, p: SmallId, t: UnitType) -> i32 {
        RailRead::player_unit_count(&*self.st, p, t)
    }
    #[inline]
    fn ticks(&self) -> i32 {
        RailRead::ticks(&*self.st)
    }
    #[inline]
    fn unit_count(&self, t: UnitType) -> i32 {
        RailRead::unit_count(&*self.st, t)
    }
    #[inline]
    fn nearby_units(&self, tile: TileRef, search_range: f64, types: &[UnitType]) -> Vec<NearbyUnit> {
        RailRead::nearby_units(&*self.st, tile, search_range, types)
    }
    #[inline]
    fn train_station_max_range(&self) -> f64 {
        RailRead::train_station_max_range(&*self.st)
    }
    #[inline]
    fn train_station_min_range(&self) -> f64 {
        RailRead::train_station_min_range(&*self.st)
    }
    #[inline]
    fn railroad_max_size(&self) -> f64 {
        RailRead::railroad_max_size(&*self.st)
    }
    #[inline]
    fn train_spawn_rate(&self, num_player_factories: f64, num_train_units: f64) -> f64 {
        RailRead::train_spawn_rate(&*self.st, num_player_factories, num_train_units)
    }
    #[inline]
    fn train_gold(&self, rel: TrainRelation, cities_visited: f64, player: SmallId) -> Gold {
        RailRead::train_gold(&*self.st, rel, cities_visited, player)
    }
}

impl RailEnv for Ctx<'_> {
    #[inline]
    fn player_unit_count_memo(&mut self, p: SmallId, t: UnitType) -> i32 {
        self.st.player_unit_count_memo(p, t)
    }
    #[inline]
    fn unit_count_memo(&mut self, t: UnitType) -> i32 {
        self.st.unit_count_memo(t)
    }
    #[inline]
    fn rail_mut(&mut self) -> &mut RailNetwork {
        &mut self.st.rail
    }
    #[inline]
    fn unit_set_train_station(&mut self, u: UnitRef, v: bool) {
        self.unit_mut(u).set_train_station(v);
    }
    #[inline]
    fn unit_move(&mut self, u: UnitRef, t: TileRef) {
        self.unit_mut(u).move_to(t);
    }
    #[inline]
    fn unit_delete(&mut self, u: UnitRef, display_message: bool) {
        self.unit_mut(u).delete(Some(display_message), None);
    }
    #[inline]
    fn unit_set_loaded(&mut self, u: UnitRef, loaded: bool) {
        self.unit_mut(u).set_loaded(loaded);
    }
    #[inline]
    fn unit_set_reached_target(&mut self, u: UnitRef) {
        self.unit_mut(u).set_reached_target();
    }
    #[inline]
    fn player_can_build(&mut self, p: SmallId, t: UnitType, tile: TileRef) -> Option<TileRef> {
        self.st.player(p).can_build(self.sc, t, tile, None)
    }
    #[inline]
    fn player_build_unit(&mut self, p: SmallId, t: UnitType, tile: TileRef, params: UnitParams) -> UnitRef {
        self.player_mut(p).build_unit(t, tile, params)
    }
    #[inline]
    fn player_add_gold(&mut self, p: SmallId, gold: Gold, tile: TileRef) {
        self.player_mut(p).add_gold(gold, Some(tile));
    }
    #[inline]
    fn player_add_train_gold(&mut self, p: SmallId, gold: Gold) {
        self.player_mut(p).add_train_gold(gold);
    }
    #[inline]
    fn stats_train_external_trade(&mut self, p: SmallId, gold: Gold) {
        self.st.stats_cx().train_external_trade(p, StatNum::Big(gold));
    }
    #[inline]
    fn stats_train_self_trade(&mut self, p: SmallId, gold: Gold) {
        self.st.stats_cx().train_self_trade(p, StatNum::Big(gold));
    }
    #[inline]
    fn record_motion_plan(&mut self, unit_id: u32) {
        Ctx::record_motion_plan(self, unit_id);
    }
    #[inline]
    fn add_execution(&mut self, e: Exec) {
        self.st.add_execution(e);
    }
    fn find_tile_path(&mut self, from: TileRef, to: TileRef) -> Vec<TileRef> {
        // TS: RailNetworkImpl.ts:281-284 — a fresh stepper per call over the shared rail chain.
        RailStepper::new().find_path(self, &[from], to).unwrap_or_default()
    }
    fn find_stations_path(&mut self, from: StationId, to: StationId) -> Vec<StationId> {
        // TS: RailNetworkImpl.ts:286-288 — a fresh `PathFinding.Stations(game)` per call.
        StationStepper::new().find_path(self, from, to).unwrap_or_default()
    }
}
