//! `FactoryExecution` (`engine/src/execution/FactoryExecution.ts`). Owner: **P5**.
//!
//! Fields: the `Factory v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1); the body is generic
//! over the rail env (`rail::env`) so it is unit-testable before the other packages land.

use crate::exec::prelude::*;
use crate::exec::train_station::TrainStationExec;
use crate::rail::env::RailEnv;

/// `FactoryExecution` data.
#[derive(StateClone, Debug)]
pub struct FactoryExec {
    pub active: bool,
    /// Snapshot `initialized` = `this.game !== undefined` (set by `init`).
    pub initialized: bool,
    pub station_created: bool,
    pub factory: UnitRef,
}

impl FactoryExec {
    /// `new FactoryExecution(factory)` (`:13-18`).
    pub fn new(factory: UnitRef) -> Self {
        FactoryExec { active: true, initialized: false, station_created: false, factory }
    }

    /// `tick(ticks)` (`:24-33`).
    pub(crate) fn tick_env<E: RailEnv + ?Sized>(&mut self, e: &mut E) {
        if !self.station_created {
            self.create_station(e);
            self.station_created = true;
        }
        if !e.unit_is_active(self.factory) {
            self.active = false;
        }
    }

    /// `createStation()` (`:43-56`).
    fn create_station<E: RailEnv + ?Sized>(&mut self, e: &mut E) {
        let structures = e.nearby_units(
            e.unit_tile(self.factory),
            e.train_station_max_range(),
            &[UnitType::City, UnitType::Port, UnitType::Factory],
        );

        // `new TrainStationExecution(..)` sets the unit's train-station flag in its
        // constructor, i.e. before the `hasTrainStation()` checks below (the factory itself
        // is one of `structures`).
        let exec = TrainStationExec::create(e, self.factory, Some(true));
        e.add_execution(Exec::TrainStation(exec));
        for s in &structures {
            if !e.unit_has_train_station(s.unit) {
                let exec = TrainStationExec::create(e, s.unit, None);
                e.add_execution(Exec::TrainStation(exec));
            }
        }
    }
}

impl ExecBehavior for FactoryExec {
    fn is_active(&self) -> bool {
        self.active
    }
    /// `activeDuringSpawnPhase()` returns `false` (`FactoryExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    /// `init(mg, ticks)`: `this.game = mg` (`:20-22`).
    fn init(&mut self, _cx: &mut Ctx, _ticks: i32) {
        self.initialized = true;
    }
    fn tick(&mut self, cx: &mut Ctx, _ticks: i32) {
        self.tick_env(cx);
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)] // tests: a failed expect is a failed test
mod tests {
    use super::*;
    use crate::rail::testenv::{Call, World};

    #[test]
    fn creates_stations_once_factory_first_then_unstationed_structures() {
        let mut w = World::new();
        let c1 = w.unit(UnitType::City, 1, 10);
        let p = w.unit(UnitType::Port, 2, 20);
        let f = w.unit(UnitType::Factory, 1, 0);
        let c2 = w.unit(UnitType::City, 1, 30);
        let _far = w.unit(UnitType::City, 1, 9000); // out of range 80
        let _silo = w.unit(UnitType::MissileSilo, 1, 5); // not a rail structure
        w.unit_data_mut(c2).has_train_station = true;
        let mut e = FactoryExec::new(f);
        e.initialized = true; // init(mg): `this.game = mg`
        e.tick_env(&mut w);
        assert!(e.station_created && e.active);
        // The factory (spawnTrains) first; then nearby City/Port/Factory without a station, in
        // nearbyUnits order (the factory itself already has its flag from its constructor).
        let got: Vec<(UnitRef, Option<bool>)> = w
            .execs
            .iter()
            .map(|x| match x {
                Exec::TrainStation(t) => (t.unit, t.spawn_trains),
                _ => panic!("expected TrainStationExecution"),
            })
            .collect();
        assert_eq!(got, [(f, Some(true)), (c1, None), (p, None)]);
        assert_eq!(w.calls[0], Call::SetTrainStation(w.unit_data(f).id, true));
        // Second tick: no new stations; factory deleted → inactive.
        e.tick_env(&mut w);
        assert_eq!(w.execs.len(), 3);
        w.unit_data_mut(f).active = false;
        e.tick_env(&mut w);
        assert!(!e.active);
    }
}
