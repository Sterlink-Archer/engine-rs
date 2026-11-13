//! `TrainStationExecution` (`engine/src/execution/TrainStationExecution.ts`). Owner: **P5**.
//!
//! Fields: the `TrainStation v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1); the bodies are generic
//! over the rail env (`rail::env`).
//!
//! **Constructor side effect.** The TS constructor calls `unit.setTrainStation(true)`
//! (`:29-32`), so the flag is set when the caller writes `new TrainStationExecution(u)`, before
//! `addExecution` and long before `init`. Callers that test `hasTrainStation()` in the same
//! tick depend on it (`FactoryExecution.ts:50-54`, `PortExecution.ts:51`). The frozen
//! [`TrainStationExec::new`] takes no context, so construct with [`TrainStationExec::create`]
//! (or call `cx.unit_mut(u).set_train_station(true)` right after `new`).

use crate::ctx::ts_throw;
use crate::exec::prelude::*;
use crate::exec::train::TrainExec;
use crate::rail::env::RailEnv;
use crate::rail::network::connect_station_env;
use crate::rail::station::{
    TrainStation, cluster_has_any_trade_destination_env, cluster_random_trade_destination_env, station_is_active_env,
};

/// `TrainStationExecution` data.
#[derive(StateClone, Debug)]
pub struct TrainStationExec {
    pub active: bool,
    /// Snapshot `initialized` = `this.mg !== undefined` (set by `init`).
    pub initialized: bool,
    pub unit: UnitRef,
    pub spawn_trains: Option<bool>,
    /// `PseudoRandom(mg.ticks())` at init (`:41`)
    pub rng: Option<Prng>,
    pub station: Option<StationId>,
    /// `numCars = 5`
    pub num_cars: i32,
    pub last_spawn_tick: i32,
    pub ticks_cooldown: i32,
}

impl TrainStationExec {
    /// `new TrainStationExecution(unit, spawnTrains?)` **without** the constructor's
    /// `unit.setTrainStation(true)` (see the module docs; use [`Self::create`]).
    pub fn new(unit: UnitRef, spawn_trains: Option<bool>) -> Self {
        TrainStationExec {
            active: true,
            initialized: false,
            unit,
            spawn_trains,
            rng: None,
            station: None,
            num_cars: 5,
            last_spawn_tick: 0,
            // Minimum cooldown between two trains
            ticks_cooldown: 10,
        }
    }

    /// `new TrainStationExecution(unit, spawnTrains?)` (`:27-32`) including the constructor's
    /// `this.unit.setTrainStation(true)`.
    pub fn create_in(cx: &mut Ctx, unit: UnitRef, spawn_trains: Option<bool>) -> Self {
        Self::create(cx, unit, spawn_trains)
    }

    /// [`Self::create_in`] over the rail env.
    pub(crate) fn create<E: RailEnv + ?Sized>(e: &mut E, unit: UnitRef, spawn_trains: Option<bool>) -> Self {
        let exec = Self::new(unit, spawn_trains);
        e.unit_set_train_station(unit, true);
        exec
    }

    /// `if (this.spawnTrains)` on `boolean | undefined`.
    #[inline]
    fn spawns_trains(&self) -> bool {
        self.spawn_trains == Some(true)
    }

    /// `init(mg, ticks)` (`:38-43`).
    pub(crate) fn init_env<E: RailEnv + ?Sized>(&mut self, e: &mut E) {
        self.initialized = true;
        if self.spawns_trains() {
            self.rng = Some(Prng::new(f64::from(e.ticks())));
        }
    }

    /// `tick(ticks)` (`:45-64`).
    pub(crate) fn tick_env<E: RailEnv + ?Sized>(&mut self, e: &mut E, ticks: i32) {
        if !self.initialized {
            ts_throw!("Not initialized");
        }
        // TS: `|| this.unit === undefined` — the unit is a constructor argument, never undefined.
        if !self.is_active() {
            return;
        }
        let station = match self.station {
            Some(s) => s,
            None => {
                // Can't create new executions on init, so it has to be done in the tick
                // TS: this.station = new TrainStation(mg, unit); railNetwork().connectStation(station)
                // `connectStation` assigns the id (`addStation`) first; the station is stored
                // here before the connection runs, as in TS.
                let key = UnitKey(e.unit_id(self.unit));
                let s = e.rail_mut().station_manager.add_station(key, TrainStation::new(StationId(0), self.unit));
                self.station = Some(s);
                connect_station_env(e, s);
                s
            }
        };
        if !station_is_active_env(e, station) {
            self.active = false;
            return;
        }
        if self.spawns_trains() {
            self.spawn_train(e, station, ticks);
        }
    }

    /// `shouldSpawnTrain()` (`:66-79`).
    fn should_spawn_train<E: RailEnv + ?Sized>(&mut self, e: &mut E) -> bool {
        let owner = e.unit_owner(self.unit);
        // Argument order as TS (`:70-71`); both are pure reads (the memo fills are not state).
        let factories = f64::from(e.player_unit_count_memo(owner, UnitType::Factory));
        let trains = f64::from(e.unit_count_memo(UnitType::Train));
        let spawn_rate = e.train_spawn_rate(factories, trains);
        let rng = self.rng.as_mut().ts_expect("TrainStationExecution.ts:74 (this.random)");
        let mut i = 0;
        // TS re-reads `this.unit!.level()` every iteration (`:73`).
        while i < e.unit_level(self.unit) {
            if rng.chance(spawn_rate) {
                return true;
            }
            i += 1;
        }
        false
    }

    /// `spawnTrain(station, currentTick)` (`:81-114`).
    fn spawn_train<E: RailEnv + ?Sized>(&mut self, e: &mut E, station: StationId, current_tick: i32) {
        if !self.initialized {
            ts_throw!("Not initialized");
        }
        if !self.spawns_trains() {
            return;
        }
        if self.rng.is_none() {
            ts_throw!("Not initialized");
        }
        if current_tick < self.last_spawn_tick + self.ticks_cooldown {
            return;
        }
        let Some(cluster) = e.rail().station_obj(station).cluster else { return };
        let owner = e.unit_owner(self.unit);
        if !cluster_has_any_trade_destination_env(e, cluster, owner) {
            return;
        }
        if !self.should_spawn_train(e) {
            return;
        }

        // Pick a destination randomly.
        // Could be improved to pick a lucrative trip
        let rng = self.rng.as_mut().ts_expect("TrainStationExecution.ts:100 (this.random)");
        let Some(destination) = cluster_random_trade_destination_env(e, cluster, owner, rng) else { return };
        if destination == station {
            return;
        }

        e.add_execution(Exec::Train(TrainExec::new(owner, station, destination, self.num_cars)));
        self.last_spawn_tick = current_tick;
    }
}

impl ExecBehavior for TrainStationExec {
    fn is_active(&self) -> bool {
        self.active
    }
    /// `activeDuringSpawnPhase()` returns `false` (`TrainStationExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    fn init(&mut self, cx: &mut Ctx, _ticks: i32) {
        self.init_env(cx);
    }
    fn tick(&mut self, cx: &mut Ctx, ticks: i32) {
        self.tick_env(cx, ticks);
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)] // tests: a failed expect is a failed test
mod tests {
    //! T2 module tests (no upstream vitest targets this file directly; its behaviour is in the
    //! upstream scenario tests `TradeTrainScenarios`/`TradeTrainGolden`, which need the full
    //! engine) on the fake world.
    use super::*;
    use crate::rail::testenv::{Call, World};

    #[test]
    fn create_sets_the_flag_at_construction() {
        let mut w = World::new();
        let u = w.unit(UnitType::City, 1, 3);
        let e = TrainStationExec::create(&mut w, u, None);
        assert!(w.unit_data(u).has_train_station);
        assert_eq!(w.calls, [Call::SetTrainStation(1, true)]);
        assert!(e.active && !e.initialized && e.station.is_none() && e.rng.is_none());
        assert_eq!((e.num_cars, e.last_spawn_tick, e.ticks_cooldown), (5, 0, 10));
    }

    #[test]
    fn init_seeds_the_prng_only_for_spawning_stations() {
        let mut w = World::new();
        w.ticks = 77;
        let u = w.unit(UnitType::Factory, 1, 3);
        for (spawn, has_rng) in [(Some(true), true), (Some(false), false), (None, false)] {
            let mut e = TrainStationExec::new(u, spawn);
            e.init_env(&mut w);
            assert!(e.initialized);
            assert_eq!(e.rng.is_some(), has_rng, "{spawn:?}");
            if let Some(r) = &e.rng {
                assert_eq!(r.get_state(), Prng::new(77.0).get_state(), "PseudoRandom(mg.ticks())");
            }
        }
    }

    #[test]
    #[should_panic]
    fn tick_before_init_throws() {
        let mut w = World::new();
        let u = w.unit(UnitType::City, 1, 3);
        TrainStationExec::new(u, None).tick_env(&mut w, 1);
    }

    #[test]
    fn first_tick_creates_and_connects_the_station() {
        let mut w = World::new();
        let u = w.unit(UnitType::City, 1, 3);
        let mut e = TrainStationExec::create(&mut w, u, None);
        e.init_env(&mut w);
        e.tick_env(&mut w, 1);
        let s = e.station.expect("station");
        assert_eq!(s, StationId(1));
        assert_eq!(w.rail.station_manager.find_station(UnitKey(1)), Some(s));
        assert!(w.rail.station_obj(s).cluster.is_some(), "connected: a new cluster");
        assert!(e.active);
        e.tick_env(&mut w, 2);
        assert_eq!(w.rail.station_manager.count(), 2, "created once");
        w.unit_data_mut(u).active = false;
        e.tick_env(&mut w, 3);
        assert!(!e.active);
        assert!(w.execs.is_empty());
    }

    #[test]
    fn spawns_trains_to_a_random_trade_destination_with_cooldown() {
        let mut w = World::new();
        let f = w.unit(UnitType::Factory, 1, 0);
        let c = w.unit(UnitType::City, 1, 30);
        // City station first (own cluster), then the factory, which connects to it.
        let mut ec = TrainStationExec::create(&mut w, c, None);
        ec.init_env(&mut w);
        ec.tick_env(&mut w, 0);
        let mut ef = TrainStationExec::create(&mut w, f, Some(true));
        ef.init_env(&mut w);
        // lastSpawnTick starts at 0: ticks < 10 are inside the cooldown.
        ef.tick_env(&mut w, 9);
        assert!(w.execs.is_empty());
        ef.tick_env(&mut w, 10);
        // spawnRate 1 (fake): chance(1) = nextInt(0,1) == 0 is always true; one destination.
        assert_eq!(w.execs.len(), 1);
        match &w.execs[0] {
            Exec::Train(t) => {
                assert_eq!(
                    (t.player, t.source, t.destination, t.num_cars),
                    (SmallId(1), ef.station.expect("f"), ec.station.expect("c"), 5)
                );
            }
            _ => panic!("expected a TrainExecution"),
        }
        assert_eq!(ef.last_spawn_tick, 10);
        // Cooldown: ticks 11..20 spawn nothing; tick 20 spawns again.
        for t in 11..20 {
            ef.tick_env(&mut w, t);
        }
        assert_eq!(w.execs.len(), 1);
        ef.tick_env(&mut w, 20);
        assert_eq!(w.execs.len(), 2);
        // The city station never spawns.
        ec.tick_env(&mut w, 50);
        assert_eq!(w.execs.len(), 2);
    }

    #[test]
    fn no_spawn_without_trade_partner_or_cluster_or_when_destination_is_self() {
        // A lone trade station picks itself → no train (destination === station).
        let mut w = World::new();
        let c = w.unit(UnitType::City, 1, 0);
        let mut e = TrainStationExec::create(&mut w, c, Some(true));
        e.init_env(&mut w);
        e.tick_env(&mut w, 100);
        assert!(w.execs.is_empty());
        assert_eq!(e.last_spawn_tick, 0);
        // A lone factory has no trade destination at all: no PRNG draw.
        let mut w = World::new();
        let f = w.unit(UnitType::Factory, 1, 0);
        let mut e = TrainStationExec::create(&mut w, f, Some(true));
        e.init_env(&mut w);
        let before = e.rng.as_ref().expect("rng").get_state();
        e.tick_env(&mut w, 100);
        assert!(w.execs.is_empty());
        assert_eq!(e.rng.as_ref().expect("rng").get_state(), before);
        // spawnRate high: chance fails for level-1 → no train, one draw per level.
        let mut w = World::new();
        w.spawn_rate = 1e9;
        let f = w.unit(UnitType::Factory, 1, 0);
        let c = w.unit(UnitType::City, 1, 30);
        let mut ec = TrainStationExec::create(&mut w, c, None);
        ec.init_env(&mut w);
        ec.tick_env(&mut w, 0);
        let mut e = TrainStationExec::create(&mut w, f, Some(true));
        e.init_env(&mut w);
        w.unit_data_mut(f).level = 3;
        let mut r = e.rng.clone().expect("rng");
        e.tick_env(&mut w, 100);
        assert!(w.execs.is_empty());
        for _ in 0..3 {
            assert!(!r.chance(1e9));
        }
        assert_eq!(e.rng.as_ref().expect("rng").get_state(), r.get_state(), "3 chance() draws for level 3");
    }
}
