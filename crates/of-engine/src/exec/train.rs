//! `TrainExecution` (`engine/src/execution/TrainExecution.ts`). Owner: **P5**.
//!
//! Fields: the `Train v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1); the bodies are generic
//! over the rail env (`rail::env`). `railNetwork` is `cx.st.rail` (the snapshot asserts the
//! train uses the game's network, `:406-408`).
//!
//! Index semantics: `currentTile` and `pathIndex` are JS numbers used as array indexes.
//! Every read that TS can make out of range (`getTiles()[currentTile]`, `pathTiles[cursor++]`,
//! `tiles[index]` in `stopIndexAtBoundary`) goes through `.get()` and keeps the `undefined`
//! consequence (PORTING_GUIDE §8).

use crate::api::types::TrainType;
use crate::ctx::ts_throw;
use crate::exec::prelude::*;
use crate::rail::env::RailEnv;
use crate::rail::railroad::get_oriented_railroad_env;
use crate::rail::station::{
    station_is_active_env, station_on_train_stop_env, station_tile_env, station_trade_available_env,
};
use crate::state::unit::UnitParams;

/// `TrainExecution` data.
#[derive(StateClone, Debug)]
pub struct TrainExec {
    pub active: bool,
    /// Snapshot `initialized` = `this.mg !== null` (set by `init`).
    pub initialized: bool,
    /// primary unit
    pub train: Option<UnitRef>,
    /// stored back to front
    pub cars: Vec<UnitRef>,
    pub has_cargo: bool,
    pub current_tile: i32,
    /// `spacing = 2`
    pub spacing: i32,
    /// `usedTiles` (`shift()`, `:230-232`)
    pub used_tiles: VecDeque<TileRef>,
    pub stations: Vec<StationId>,
    pub current_railroad: Option<OrientedRailroad>,
    /// `speed = 2`
    pub speed: i32,
    pub trade_stops_visited: i32,
    pub path_tiles: Vec<TileRef>,
    pub path_index: i32,
    pub player: SmallId,
    pub source: StationId,
    pub destination: StationId,
    pub num_cars: i32,
}

/// `arr[i]` for a JS number index: `undefined` (None) when negative or past the end.
#[inline]
fn at(arr: &[TileRef], i: i64) -> Option<TileRef> {
    usize::try_from(i).ok().and_then(|i| arr.get(i).copied())
}

impl TrainExec {
    /// `new TrainExecution(railNetwork, player, source, destination, numCars)` (the network is `cx.st.rail`).
    pub fn new(player: SmallId, source: StationId, destination: StationId, num_cars: i32) -> Self {
        TrainExec {
            active: true,
            initialized: false,
            train: None,
            cars: Vec::new(),
            has_cargo: false,
            current_tile: 0,
            spacing: 2,
            used_tiles: VecDeque::new(),
            stations: Vec::new(),
            current_railroad: None,
            speed: 2,
            trade_stops_visited: 0,
            path_tiles: Vec::new(),
            path_index: 0,
            player,
            source,
            destination,
            num_cars,
        }
    }

    /// `owner()` (`:46-48`).
    pub fn owner(&self) -> SmallId {
        self.player
    }

    /// `tradeStopsVisited()` (`:50-52`).
    pub fn trade_stops_visited(&self) -> i32 {
        self.trade_stops_visited
    }

    /// `init(mg, ticks)` (`:54-114`).
    pub(crate) fn init_env<E: RailEnv + ?Sized>(&mut self, e: &mut E) {
        self.initialized = true;
        let stations = e.find_stations_path(self.source, self.destination);
        if stations.len() <= 1 {
            self.active = false;
            return;
        }

        self.stations = stations;
        // in-range: stations.len() >= 2 (checked above) for every index below.
        match get_oriented_railroad_env(e, self.stations[0], self.stations[1]) {
            Some(railroad) => self.current_railroad = Some(railroad),
            None => {
                self.active = false;
                return;
            }
        }

        let spawn_at = station_tile_env(e, self.stations[0]);
        let Some(spawn) = e.player_can_build(self.player, UnitType::Train, spawn_at) else {
            // console.warn(`cannot build train`): dropped
            self.active = false;
            return;
        };
        let train = self.create_train_units(e, spawn);
        self.train = Some(train);

        // `carUnitIds` (`:82`) feeds only the motion plan below.
        let mut path_tiles: Vec<TileRef> = Vec::new();
        let mut i = 0;
        while i + 1 < self.stations.len() {
            let Some(segment) = get_oriented_railroad_env(e, self.stations[i], self.stations[i + 1]) else {
                self.active = false;
                return;
            };
            path_tiles.extend_from_slice(segment.get_tiles());
            i += 1;
        }
        let start_tile = e.unit_tile(train);
        if path_tiles.is_empty() || path_tiles[0] != start_tile {
            path_tiles.insert(0, start_tile);
            self.path_index = 1;
            self.current_tile = -1;
        }
        self.path_tiles = path_tiles;

        // mg.recordMotionPlan({kind: "train", engineUnitId, carUnitIds, ..}): only the
        // `planDrivenUnitIds` adds are state (GameImpl.ts:707-711), engine first then cars.
        e.record_motion_plan(e.unit_id(train));
        for &car in &self.cars {
            e.record_motion_plan(e.unit_id(car));
        }
    }

    /// `tick(ticks)` (`:116-140`).
    pub(crate) fn tick_env<E: RailEnv + ?Sized>(&mut self, e: &mut E) {
        let Some(train) = self.train else {
            ts_throw!("Not initialized");
        };

        if !e.unit_is_active(train) {
            self.delete_train(e);
            return;
        }

        self.reconcile_current_railroad_split(e);

        if !self.active_source_or_destination(e) {
            self.delete_train(e);
            return;
        }

        let tile = self.get_next_tile(e);
        // `if (tile)`: TileRef 0 is falsy, like `null`/`undefined`.
        match tile {
            Some(t) if t != 0 => self.update_cars_positions(e, t),
            _ => {
                self.target_reached(e);
                self.delete_train(e);
            }
        }
    }

    /// `loadCargo()` (`:142-151`). No caller in the engine at the pin.
    pub(crate) fn load_cargo_env<E: RailEnv + ?Sized>(&mut self, e: &mut E) {
        if self.has_cargo || self.train.is_none() {
            return;
        }
        self.has_cargo = true;
        // Starts at 1: don't load tail engine
        for &car in self.cars.iter().skip(1) {
            e.unit_set_loaded(car, true);
        }
    }

    /// `loadCargo()` (`:142-151`).
    pub fn load_cargo(&mut self, cx: &mut Ctx) {
        self.load_cargo_env(cx);
    }

    /// `targetReached()` (`:153-161`).
    fn target_reached<E: RailEnv + ?Sized>(&mut self, e: &mut E) {
        let Some(train) = self.train else { return };
        e.unit_set_reached_target(train);
        // forEach: length captured once; setReachedTarget does not touch `cars`.
        for &car in &self.cars {
            e.unit_set_reached_target(car);
        }
    }

    /// `createTrainUnits(tile)` (`:163-184`).
    fn create_train_units<E: RailEnv + ?Sized>(&mut self, e: &mut E, tile: TileRef) -> UnitRef {
        let destination_unit = e.rail().station_obj(self.destination).unit;
        let train = e.player_build_unit(
            self.player,
            UnitType::Train,
            tile,
            UnitParams {
                target_unit: Some(Some(destination_unit)),
                train_type: Some(TrainType::Engine),
                ..UnitParams::default()
            },
        );
        // Tail is also an engine, just for cosmetics
        let tail = e.player_build_unit(
            self.player,
            UnitType::Train,
            tile,
            UnitParams {
                target_unit: Some(Some(destination_unit)),
                train_type: Some(TrainType::TailEngine),
                ..UnitParams::default()
            },
        );
        self.cars.push(tail);
        for _ in 0..self.num_cars {
            let car = e.player_build_unit(
                self.player,
                UnitType::Train,
                tile,
                UnitParams {
                    train_type: Some(TrainType::Carriage),
                    loaded: Some(Some(self.has_cargo)),
                    ..UnitParams::default()
                },
            );
            self.cars.push(car);
        }
        train
    }

    /// `deleteTrain()` (`:186-196`).
    fn delete_train<E: RailEnv + ?Sized>(&mut self, e: &mut E) {
        self.active = false;
        if let Some(train) = self.train
            && e.unit_is_active(train)
        {
            e.unit_delete(train, false);
        }
        // iter-live: deleting a unit never touches this execution's `cars`.
        for &car in &self.cars {
            if e.unit_is_active(car) {
                e.unit_delete(car, false);
            }
        }
    }

    /// `activeSourceOrDestination()` (`:198-204`).
    fn active_source_or_destination<E: RailEnv + ?Sized>(&self, e: &E) -> bool {
        self.stations.len() > 1
            && station_is_active_env(e, self.stations[1])
            && station_is_active_env(e, self.stations[0])
    }

    /// `saveTraversedTiles(from, speed)` (`:210-227`).
    fn save_traversed_tiles(&mut self, from: i64, speed: i64) {
        let Some(rr) = &self.current_railroad else { return };
        let len = rr.get_tiles().len() as i64; // int: a Vec length
        let mut tile_to_save = from;
        let mut i = 0;
        while i < speed && tile_to_save < len {
            let tile = if tile_to_save < 0 {
                // in-range: a negative `currentTile` is only the -1 set by `init` together with
                // `pathIndex = 1` (`:96-99`); `reconcileCurrentRailroadSplit` never lowers it
                // below 0 and `nextStation` resets it to `leftOver >= 0`. So the index is 0.
                self.path_tiles[(tile_to_save + i64::from(self.path_index)) as usize]
            } else {
                // in-range: 0 <= tile_to_save < len (loop condition)
                rr.get_tiles()[tile_to_save as usize]
            };
            Self::save_tile(&mut self.used_tiles, self.cars.len(), self.spacing, tile);
            tile_to_save += 1;
            i += 1;
        }
    }

    /// `saveTile(tile)` (`:229-234`).
    fn save_tile(used_tiles: &mut VecDeque<TileRef>, cars: usize, spacing: i32, tile: TileRef) {
        used_tiles.push_back(tile);
        // int: small counts; TS compares numbers.
        if used_tiles.len() as i64 > cars as i64 * i64::from(spacing) + 3 {
            used_tiles.pop_front();
        }
    }

    /// `updateCarsPositions(newTile)` (`:236-248`).
    fn update_cars_positions<E: RailEnv + ?Sized>(&mut self, e: &mut E, new_tile: TileRef) {
        if !self.cars.is_empty() {
            for i in (0..self.cars.len()).rev() {
                let car_tile_index = (i as i64 + 1) * i64::from(self.spacing) + 2;
                if self.used_tiles.len() as i64 > car_tile_index {
                    // in-range: checked just above
                    let t = self.used_tiles[car_tile_index as usize];
                    e.unit_move(self.cars[i], t);
                }
            }
        }
        if let Some(train) = self.train {
            e.unit_move(train, new_tile);
        }
    }

    /// `nextStation()` (`:250-263`).
    fn next_station<E: RailEnv + ?Sized>(&mut self, e: &mut E) -> bool {
        if self.stations.len() > 2 {
            // TS: this.pathIndex += this.currentRailroad?.getTiles().length ?? 0
            let add = self.current_railroad.as_ref().map_or(0, |r| r.get_tiles().len() as i32);
            self.path_index += add;
            self.stations.remove(0); // stations.shift()
            let rail_road = match get_oriented_railroad_env(e, self.stations[0], self.stations[1]) {
                Some(r) => Some(r),
                None => self.resolve_split_railroad(e),
            };
            if let Some(r) = rail_road {
                self.current_railroad = Some(r);
                return true;
            }
        }
        false
    }

    /// `resolveSplitRailroad()` (`:265-281`).
    fn resolve_split_railroad<E: RailEnv + ?Sized>(&mut self, e: &mut E) -> Option<OrientedRailroad> {
        let (station0, station1) = (self.stations[0], self.stations[1]); // in-range: len >= 2 (nextStation)
        let path = e.find_stations_path(station0, station1);
        if path.len() <= 2 {
            return None;
        }
        let mut cursor = i64::from(self.path_index);
        for i in 0..path.len() - 1 {
            let segment = get_oriented_railroad_env(e, path[i], path[i + 1])?;
            for &tile in segment.get_tiles() {
                // `this.pathTiles[cursor++] !== tile` (undefined past the end)
                let got = at(&self.path_tiles, cursor);
                cursor += 1;
                if got != Some(tile) {
                    return None;
                }
            }
        }
        splice_front(&mut self.stations, 2, &path);
        get_oriented_railroad_env(e, self.stations[0], self.stations[1])
    }

    /// `reconcileCurrentRailroadSplit()` (`:283-333`): accept a split of the occupied railroad
    /// only if the replacement segments' tiles match the motion plan exactly.
    fn reconcile_current_railroad_split<E: RailEnv + ?Sized>(&mut self, e: &mut E) {
        if self.current_railroad.is_none() || self.stations.len() < 2 {
            return;
        }

        let (station0, station1) = (self.stations[0], self.stations[1]);
        if e.rail().station_obj(station0).railroad_by_neighbor.contains_key(&station1) {
            return;
        }

        let path = e.find_stations_path(station0, station1);
        if path.len() <= 2 {
            return;
        }

        let mut segments: Vec<OrientedRailroad> = Vec::new();
        let mut cursor = i64::from(self.path_index);
        for i in 0..path.len() - 1 {
            let Some(segment) = get_oriented_railroad_env(e, path[i], path[i + 1]) else { return };
            for &tile in segment.get_tiles() {
                let got = at(&self.path_tiles, cursor);
                cursor += 1;
                if got != Some(tile) {
                    return;
                }
            }
            segments.push(segment);
        }

        // rust-invariant: checked non-None at the top; nothing above clears it.
        let old_len = self.current_railroad.as_ref().map_or(0, |r| r.get_tiles().len()) as i64;
        if cursor != i64::from(self.path_index) + old_len {
            return;
        }

        // Drop replacement stations whose stop boundary is already behind the
        // engine. A station exactly at the engine remains so stationReached() can
        // process it normally and award gold once.
        let mut active_segment = 0usize;
        let mut segment_start: i64 = 0;
        let mut boundary: i64 = 0;
        for i in 0..segments.len() - 1 {
            boundary += segments[i].get_tiles().len() as i64;
            let stop_index = self.stop_index_at_boundary(e, path[i + 1], boundary);
            if f64::from(self.current_tile) <= stop_index {
                break;
            }
            active_segment = i + 1;
            segment_start = boundary;
        }

        // int: segment_start <= old tiles length (< 2^31)
        self.path_index += segment_start as i32;
        self.current_tile -= segment_start as i32;
        self.current_railroad = Some(segments.swap_remove(active_segment)); // in-range: active_segment < len
        let rest: Vec<StationId> = path[active_segment..].to_vec();
        splice_front(&mut self.stations, 2, &rest);
    }

    /// `stopIndexAtBoundary(station, tiles, boundary)` (`:340-358`) with `tiles` = the current
    /// railroad's tiles. Returns `before` or `after` (as f64: an out-of-range tile gives NaN
    /// distances, and `NaN <= x` is false → `after`, as in TS).
    fn stop_index_at_boundary<E: RailEnv + ?Sized>(&self, e: &E, station: StationId, boundary: i64) -> f64 {
        if !self.initialized {
            ts_throw!("Not initialized");
        }
        let tiles: &[TileRef] = self.current_railroad.as_ref().map_or(&[], |r| r.get_tiles());

        let before = boundary - 1;
        let after = boundary;
        let station_tile = station_tile_env(e, station);
        let station_x = f64::from(e.tile_x(station_tile));
        let station_y = f64::from(e.tile_y(station_tile));
        let distance_squared = |index: i64| -> f64 {
            // `this.mg.x(tiles[index])` on `undefined` is `undefined % w` = NaN.
            match at(tiles, index) {
                Some(t) => {
                    let dx = f64::from(e.tile_x(t)) - station_x;
                    let dy = f64::from(e.tile_y(t)) - station_y;
                    dx * dx + dy * dy
                }
                None => f64::NAN,
            }
        };

        // int: before/after are small integers
        if distance_squared(before) <= distance_squared(after) { before as f64 } else { after as f64 }
    }

    /// `canTradeWithDestination()` (`:360-364`).
    fn can_trade_with_destination<E: RailEnv + ?Sized>(&self, e: &E) -> bool {
        self.stations.len() > 1 && station_trade_available_env(e, self.stations[1], self.player)
    }

    /// `getNextTile()` (`:366-383`); `None` = `null` or `undefined`.
    fn get_next_tile<E: RailEnv + ?Sized>(&mut self, e: &mut E) -> Option<TileRef> {
        if self.current_railroad.is_none() || !self.can_trade_with_destination(e) {
            return None;
        }
        self.save_traversed_tiles(i64::from(self.current_tile), i64::from(self.speed));
        self.current_tile += self.speed;
        let cur_len = self.current_railroad.as_ref().map_or(0, |r| r.get_tiles().len()) as i64;
        let left_over = i64::from(self.current_tile) - cur_len;
        if left_over >= 0 {
            // Station reached, pick the next station
            self.station_reached(e);
            if !self.next_station(e) {
                return None; // Destination reached (or no valid connection)
            }
            self.current_tile = left_over as i32; // int: 0 <= leftOver < speed
            self.save_traversed_tiles(0, left_over);
        }
        let rr = self.current_railroad.as_ref()?;
        at(rr.get_tiles(), i64::from(self.current_tile))
    }

    /// `stationReached()` (`:385-395`).
    fn station_reached<E: RailEnv + ?Sized>(&mut self, e: &mut E) {
        if !self.initialized {
            ts_throw!("Not initialized");
        }
        let s1 = self.stations[1]; // in-range: canTradeWithDestination checked len > 1
        station_on_train_stop_env(e, s1, self.player, self.trade_stops_visited);
        let station_type = e.unit_type(e.rail().station_obj(s1).unit);
        if station_type == UnitType::City || station_type == UnitType::Port {
            self.trade_stops_visited += 1;
        }
    }
}

/// `arr.splice(0, n, ...items)` (JS clamps `n` to the length).
fn splice_front(arr: &mut Vec<StationId>, n: usize, items: &[StationId]) {
    let n = n.min(arr.len());
    arr.splice(0..n, items.iter().copied());
}

impl ExecBehavior for TrainExec {
    fn is_active(&self) -> bool {
        self.active
    }
    /// `activeDuringSpawnPhase()` returns `false` (`TrainExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    fn init(&mut self, cx: &mut Ctx, _ticks: i32) {
        self.init_env(cx);
    }
    fn tick(&mut self, cx: &mut Ctx, _ticks: i32) {
        self.tick_env(cx);
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)] // tests: a failed expect is a failed test
mod tests {
    //! T0: `tests/core/executions/TrainExecution.test.ts` (all 8 cases) on the fake world
    //! (`rail::testenv`; `plains` is 100 wide, so tiles 0..10 lie on row 0).
    use super::*;
    use crate::ids::RailroadId;
    use crate::rail::testenv::{Call, World};

    struct Fixture {
        w: World,
        exec: TrainExec,
        inserted: StationId,
        a: StationId,
        b: StationId,
        orig: RailroadId,
    }

    fn city(w: &mut World, tile: TileRef) -> StationId {
        let u = w.unit(UnitType::City, 1, tile);
        // `new TrainStation(game, unit)`; added to the manager by the caller.
        let key = UnitKey(w.unit_data(u).id);
        w.rail.station_manager.add_station(key, crate::rail::TrainStation::new(StationId(0), u))
    }

    /// `currentSegmentFixture(splitIndex, reverseDirection)`.
    fn current_segment_fixture(split_index: usize, reverse: bool) -> Fixture {
        let mut w = World::new();
        let rail_tiles: Vec<TileRef> = (1..=9).collect();
        let a = city(&mut w, rail_tiles[0]);
        let b = city(&mut w, rail_tiles[8]);
        // The inserted station's unit exists now; its station is added at split time.
        let inserted_unit = w.unit(UnitType::City, 1, rail_tiles[split_index]);
        let orig = w.link(a, b, &rail_tiles, 1);
        let (source, destination) = if reverse { (b, a) } else { (a, b) };
        let mut exec = TrainExec::new(SmallId(1), source, destination, 1);
        exec.init_env(&mut w);
        assert!(exec.is_active());
        // Placeholder until splitRail adds it.
        let _ = inserted_unit;
        Fixture { w, exec, inserted: StationId(0), a, b, orig }
    }

    impl Fixture {
        fn split_rail(&mut self, split_index: usize) {
            let rail_tiles: Vec<TileRef> = (1..=9).collect();
            let w = &mut self.w;
            crate::rail::station::station_remove_railroad_env(w, self.a, self.orig);
            crate::rail::station::station_remove_railroad_env(w, self.b, self.orig);
            let inserted_unit = UnitRef { slot: 2, generation: 0 };
            let key = UnitKey(w.unit_data(inserted_unit).id);
            self.inserted =
                w.rail.station_manager.add_station(key, crate::rail::TrainStation::new(StationId(0), inserted_unit));
            w.link(self.a, self.inserted, &rail_tiles[..split_index], 2);
            w.link(self.inserted, self.b, &rail_tiles[split_index..], 3);
        }
        fn tick(&mut self) {
            self.exec.tick_env(&mut self.w);
        }
        /// `onTrainStop` calls that paid at the inserted station (its tile identifies the stop).
        fn stops_at_inserted(&self) -> usize {
            let t = self.w.rail.station_obj(self.inserted).unit;
            let tile = self.w.unit_data(t).tile;
            self.w.calls.iter().filter(|c| matches!(c, Call::AddGold(_, _, tt) if *tt == tile)).count()
        }
    }

    #[test]
    fn visits_and_pays_a_station_inserted_ahead_of_the_engine() {
        let mut f = current_segment_fixture(4, false);
        f.tick();
        f.split_rail(4);
        f.tick();
        assert!(f.exec.is_active());
        assert_eq!(f.stops_at_inserted(), 1);
        assert_eq!(f.w.train_gold_of(1), 10_000);
    }

    #[test]
    fn does_not_visit_or_pay_a_station_inserted_behind_the_engine() {
        let mut f = current_segment_fixture(3, false);
        f.tick();
        f.tick();
        f.split_rail(3);
        f.tick();
        assert!(f.exec.is_active());
        assert_eq!(f.stops_at_inserted(), 0);
        assert_eq!(f.w.train_gold_of(1), 0);
    }

    #[test]
    fn visits_and_pays_a_station_inserted_exactly_at_the_engine_once() {
        let mut f = current_segment_fixture(2, false);
        f.tick();
        f.split_rail(2);
        f.tick();
        f.tick();
        assert_eq!(f.stops_at_inserted(), 1);
        assert_eq!(f.w.train_gold_of(1), 10_000);
    }

    #[test]
    fn does_not_revisit_a_passed_station_while_travelling_in_reverse() {
        let mut f = current_segment_fixture(3, true);
        f.tick();
        f.tick();
        f.tick();
        f.split_rail(3);
        f.tick();
        assert!(f.exec.is_active());
        assert_eq!(f.stops_at_inserted(), 0);
        assert_eq!(f.w.train_gold_of(1), 0);
    }

    /// The upstream `link` helper plus a world of cities at `tiles` (stations not yet added).
    fn cities(w: &mut World, tiles: &[TileRef]) -> Vec<UnitRef> {
        tiles.iter().map(|&t| w.unit(UnitType::City, 1, t)).collect()
    }
    fn add(w: &mut World, u: UnitRef) -> StationId {
        let key = UnitKey(w.unit_data(u).id);
        w.rail.station_manager.add_station(key, crate::rail::TrainStation::new(StationId(0), u))
    }

    #[test]
    fn re_resolves_intermediate_stations_when_railroad_is_split_in_transit() {
        let mut w = World::new();
        let us = cities(&mut w, &[0, 1, 4, 2, 3]);
        let (sa, sb, sc) = (add(&mut w, us[0]), add(&mut w, us[1]), add(&mut w, us[2]));
        w.link(sa, sb, &[0, 1], 1);
        let rail_bc = w.link(sb, sc, &[1, 2, 2, 3, 3, 4], 2);
        let mut exec = TrainExec::new(SmallId(1), sa, sc, 1);
        exec.init_env(&mut w);

        // Split edge B->C into B->D1, D1->D2, and D2->C while train is in transit
        crate::rail::station::station_remove_railroad_env(&mut w, sb, rail_bc);
        crate::rail::station::station_remove_railroad_env(&mut w, sc, rail_bc);
        let d1 = add(&mut w, us[3]);
        let d2 = add(&mut w, us[4]);
        w.link(sb, d1, &[1, 2], 3);
        w.link(d1, d2, &[2, 3], 4);
        w.link(d2, sc, &[3, 4], 5);

        for (t, active) in [(1, true), (2, true), (3, true), (4, false)] {
            exec.tick_env(&mut w);
            assert_eq!(exec.is_active(), active, "after tick {t}");
        }
    }

    #[test]
    fn rejects_detour_when_railroad_is_rerouted_off_original_motion_plan() {
        let mut w = World::new();
        let us = cities(&mut w, &[0, 1, 2, 4]);
        let (sa, sb, sc) = (add(&mut w, us[0]), add(&mut w, us[1]), add(&mut w, us[2]));
        w.link(sa, sb, &[0, 1], 1);
        let rail_bc = w.link(sb, sc, &[1, 3, 2], 2);
        let mut exec = TrainExec::new(SmallId(1), sa, sc, 1);
        exec.init_env(&mut w);

        // Replace B->C with a detour through station D (tile 4, not on original path [0, 1, 1, 3, 2])
        crate::rail::station::station_remove_railroad_env(&mut w, sb, rail_bc);
        crate::rail::station::station_remove_railroad_env(&mut w, sc, rail_bc);
        let sd = add(&mut w, us[3]);
        w.link(sb, sd, &[1, 4], 3);
        w.link(sd, sc, &[4, 2], 4);

        exec.tick_env(&mut w);
        // At station B, the train attempts nextStation() but rejects the detour through tile 4
        assert!(!exec.is_active());
    }

    #[test]
    fn re_resolves_when_intermediate_station_is_placed_off_track() {
        let mut w = World::new();
        // Station D is at tile 10 (adjacent off-track building tile, not in the railroad tile array)
        let us = cities(&mut w, &[0, 1, 3, 10]);
        let (sa, sb, sc) = (add(&mut w, us[0]), add(&mut w, us[1]), add(&mut w, us[2]));
        w.link(sa, sb, &[0, 1], 1);
        let rail_bc = w.link(sb, sc, &[1, 2, 2, 3], 2);
        let mut exec = TrainExec::new(SmallId(1), sa, sc, 1);
        exec.init_env(&mut w);

        // Split edge B->C into B->D [1, 2] and D->C [2, 3], where stationD.tile() = 10
        crate::rail::station::station_remove_railroad_env(&mut w, sb, rail_bc);
        crate::rail::station::station_remove_railroad_env(&mut w, sc, rail_bc);
        let sd = add(&mut w, us[3]);
        w.link(sb, sd, &[1, 2], 3);
        w.link(sd, sc, &[2, 3], 4);

        for (t, active) in [(1, true), (2, true), (3, false)] {
            exec.tick_env(&mut w);
            assert_eq!(exec.is_active(), active, "after tick {t}");
        }
    }

    #[test]
    fn rejects_detour_when_only_the_final_segment_detours_off_motion_plan() {
        let mut w = World::new();
        let us = cities(&mut w, &[0, 1, 3, 2]);
        let (sa, sb, sc) = (add(&mut w, us[0]), add(&mut w, us[1]), add(&mut w, us[2]));
        w.link(sa, sb, &[0, 1], 1);
        let rail_bc = w.link(sb, sc, &[1, 2, 2, 3], 2);
        let mut exec = TrainExec::new(SmallId(1), sa, sc, 1);
        exec.init_env(&mut w);

        // B->D uses planned tiles [1, 2], but D->C detours through tile 9 [2, 9, 3]
        crate::rail::station::station_remove_railroad_env(&mut w, sb, rail_bc);
        crate::rail::station::station_remove_railroad_env(&mut w, sc, rail_bc);
        let sd = add(&mut w, us[3]);
        w.link(sb, sd, &[1, 2], 3);
        w.link(sd, sc, &[2, 9, 3], 4);

        exec.tick_env(&mut w);
        // Rejects because final segment D->C uses unrecorded tile 9
        assert!(!exec.is_active());
    }

    // ---- T2 module tests beyond the upstream file ----

    #[test]
    fn init_builds_units_path_and_motion_plan() {
        let mut w = World::new();
        let us = cities(&mut w, &[5, 1, 3]);
        let (sa, sb, sc) = (add(&mut w, us[0]), add(&mut w, us[1]), add(&mut w, us[2]));
        // A at 5 → B at 1 (stored B→A, so traversed reversed), B → C at 3.
        w.link(sb, sa, &[1, 2, 3, 4], 1);
        w.link(sb, sc, &[2, 3], 2);
        let mut exec = TrainExec::new(SmallId(1), sa, sc, 2);
        exec.init_env(&mut w);
        assert!(exec.is_active());
        assert_eq!(exec.stations, [sa, sb, sc]);
        // Engine + tail + 2 carriages, built at A's tile; targetUnit = destination's unit.
        let train = exec.train.expect("train");
        assert_eq!(exec.cars.len(), 3);
        let t = w.unit_data(train);
        assert_eq!((t.ty, t.tile), (UnitType::Train, 5));
        assert_eq!(t.params.train_type, Some(TrainType::Engine));
        assert_eq!(t.params.target_unit, Some(Some(us[2])));
        assert_eq!(w.unit_data(exec.cars[0]).params.train_type, Some(TrainType::TailEngine));
        assert_eq!(w.unit_data(exec.cars[1]).params.train_type, Some(TrainType::Carriage));
        assert_eq!(w.unit_data(exec.cars[1]).params.loaded, Some(Some(false)));
        assert_eq!(w.unit_data(exec.cars[1]).params.target_unit, None, "carriages have no target");
        // pathTiles: reversed A→B segment then B→C; start tile 5 is not pathTiles[0] (4) → unshift.
        assert_eq!(exec.path_tiles, [5, 4, 3, 2, 1, 2, 3]);
        assert_eq!((exec.path_index, exec.current_tile), (1, -1));
        // Motion plan: engine id first, then cars (planDrivenUnitIds insertion order).
        let ids: Vec<u32> =
            std::iter::once(train).chain(exec.cars.iter().copied()).map(|u| w.unit_data(u).id).collect();
        assert_eq!(w.motion_plan, ids);
        assert_eq!(exec.current_railroad.as_ref().map(|r| r.forward), Some(false));
    }

    #[test]
    fn init_failures_deactivate() {
        // No path (single station).
        let mut w = World::new();
        let us = cities(&mut w, &[0]);
        let sa = add(&mut w, us[0]);
        let mut exec = TrainExec::new(SmallId(1), sa, sa, 1);
        exec.init_env(&mut w);
        assert!(!exec.is_active() && exec.train.is_none() && exec.initialized);
        // canBuild false: no units.
        let mut w = World::new();
        let us = cities(&mut w, &[0, 1]);
        let (sa, sb) = (add(&mut w, us[0]), add(&mut w, us[1]));
        w.link(sa, sb, &[0, 1], 1);
        w.can_build = false;
        let mut exec = TrainExec::new(SmallId(1), sa, sb, 1);
        exec.init_env(&mut w);
        assert!(!exec.is_active() && exec.train.is_none());
        assert_eq!(w.units.len(), 2);
    }

    #[test]
    #[should_panic]
    fn tick_before_init_throws() {
        let mut w = World::new();
        let mut exec = TrainExec::new(SmallId(1), StationId(1), StationId(2), 1);
        exec.tick_env(&mut w);
    }

    #[test]
    fn runs_to_destination_pays_and_deletes() {
        let mut w = World::new();
        let us = cities(&mut w, &[0, 4, 8]);
        let (sa, sb, sc) = (add(&mut w, us[0]), add(&mut w, us[1]), add(&mut w, us[2]));
        w.link(sa, sb, &[0, 1, 2, 3], 1);
        w.link(sb, sc, &[4, 5, 6, 7], 2);
        let mut exec = TrainExec::new(SmallId(1), sa, sc, 1);
        exec.init_env(&mut w);
        let train = exec.train.expect("train");
        let mut ticks = 0;
        while exec.is_active() {
            exec.tick_env(&mut w);
            ticks += 1;
            assert!(ticks < 20);
        }
        // Stops at B (paid, 1st trade stop) and C (paid, 2nd).
        assert_eq!(exec.trade_stops_visited, 2);
        assert_eq!(w.train_gold_of(1), 20_000);
        // Engine moves: 2, then B reached (tile 4 at index 0 of B→C), 6, then destination.
        assert_eq!(w.unit_data(train).moves, [2, 4, 6]);
        assert!(w.unit_data(train).reached_target);
        assert!(!w.unit_data(train).active);
        for &c in &exec.cars {
            assert!(!w.unit_data(c).active && w.unit_data(c).reached_target);
        }
        // usedTiles never exceeds cars*spacing+3 = 7 and keeps the newest tiles.
        assert!(exec.used_tiles.len() <= 7);
        assert_eq!(exec.used_tiles.back(), Some(&7));
    }

    #[test]
    fn tile_zero_is_falsy_and_ends_the_trip() {
        // `if (tile)`: reaching TileRef 0 counts as "no next tile" (targetReached + delete).
        let mut w = World::new();
        let us = cities(&mut w, &[4, 0]);
        let (sa, sb) = (add(&mut w, us[0]), add(&mut w, us[1]));
        w.link(sa, sb, &[4, 3, 2, 1, 0, 0, 0], 1);
        let mut exec = TrainExec::new(SmallId(1), sa, sb, 0);
        exec.init_env(&mut w);
        exec.tick_env(&mut w); // → tile 2
        assert!(exec.is_active());
        exec.tick_env(&mut w); // → tile index 4 = TileRef 0 → falsy
        assert!(!exec.is_active());
        assert!(w.unit_data(exec.train.expect("train")).reached_target);
        assert_eq!(w.train_gold_of(1), 0, "no station reached");
    }

    #[test]
    fn inactive_station_deletes_train_without_reaching_target() {
        let mut w = World::new();
        let us = cities(&mut w, &[0, 9]);
        let (sa, sb) = (add(&mut w, us[0]), add(&mut w, us[1]));
        w.link(sa, sb, &(0..=9).collect::<Vec<_>>(), 1);
        let mut exec = TrainExec::new(SmallId(1), sa, sb, 1);
        exec.init_env(&mut w);
        exec.tick_env(&mut w);
        w.unit_data_mut(us[1]).active = false;
        exec.tick_env(&mut w);
        assert!(!exec.is_active());
        let train = exec.train.expect("train");
        assert!(!w.unit_data(train).reached_target);
        // deleteTrain: engine first, then cars back to front order as stored.
        let deleted: Vec<u32> =
            w.calls.iter().filter_map(|c| if let Call::Delete(id) = c { Some(*id) } else { None }).collect();
        let expect: Vec<u32> =
            std::iter::once(train).chain(exec.cars.iter().copied()).map(|u| w.unit_data(u).id).collect();
        assert_eq!(deleted, expect);
    }

    #[test]
    fn embargo_stops_the_train() {
        let mut w = World::new();
        let u0 = w.unit(UnitType::City, 1, 0);
        let u1 = w.unit(UnitType::City, 2, 9);
        let (sa, sb) = (add(&mut w, u0), add(&mut w, u1));
        w.link(sa, sb, &(0..=9).collect::<Vec<_>>(), 1);
        let mut exec = TrainExec::new(SmallId(1), sa, sb, 1);
        exec.init_env(&mut w);
        exec.tick_env(&mut w);
        assert!(exec.is_active());
        w.no_trade.insert((2, 1));
        exec.tick_env(&mut w);
        assert!(!exec.is_active(), "canTradeWithDestination false → getNextTile null → arrive+delete");
        assert!(w.unit_data(exec.train.expect("train")).reached_target);
    }

    #[test]
    fn load_cargo_skips_the_tail_engine() {
        let mut w = World::new();
        let us = cities(&mut w, &[0, 9]);
        let (sa, sb) = (add(&mut w, us[0]), add(&mut w, us[1]));
        w.link(sa, sb, &(0..=9).collect::<Vec<_>>(), 1);
        let mut exec = TrainExec::new(SmallId(1), sa, sb, 2);
        exec.init_env(&mut w);
        exec.load_cargo_env(&mut w);
        assert!(exec.has_cargo);
        assert_eq!(w.unit_data(exec.cars[0]).loaded, None, "tail engine");
        assert_eq!(w.unit_data(exec.cars[1]).loaded, Some(true));
        assert_eq!(w.unit_data(exec.cars[2]).loaded, Some(true));
    }
}
