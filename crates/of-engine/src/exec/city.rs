//! `CityExecution` (`engine/src/execution/CityExecution.ts`). Owner: **P7**.
//!
//! Fields: the `City v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1).

use crate::canon::{Canon, CanonCx, Cs};
use crate::exec::prelude::*;
use crate::exec::train_station::TrainStationExec;

/// `CityExecution` data.
#[derive(StateClone, Debug)]
pub struct CityExec {
    pub active: bool,
    /// `initialized: this.mg !== undefined` (set by `init`).
    pub initialized: bool,
    pub station_created: bool,
    pub city: UnitRef,
}

impl CityExec {
    /// `new CityExecution(city)`.
    pub fn new(city: UnitRef) -> Self {
        // TS: :14-15 `active = true`, `stationCreated = false`
        CityExec { active: true, initialized: false, station_created: false, city }
    }

    /// `createStation()` (`:42-51`).
    fn create_station(&mut self, cx: &mut Ctx) {
        // TS: :43-47 `mg.hasUnitNearby(city.tile()!, config.trainStationMaxRange(), Factory)`
        // (`playerId`, `includeUnderConstruction` undefined).
        let tile = cx.st.unit(self.city).tile();
        let range = cx.st.config().train_station_max_range();
        let nearby_factory = cx.st.has_unit_nearby(tile, range, UnitType::Factory, None, false);
        if nearby_factory {
            add_train_station_exec(cx, self.city);
        }
    }
}

/// `mg.addExecution(new TrainStationExecution(unit))` (no `spawnTrains`).
///
/// The TS constructor writes `this.unit.setTrainStation(true)` (`TrainStationExecution.ts:31`)
/// at construction time, but the frozen `TrainStationExec::new(unit, spawn_trains)` has no
/// `cx`. The caller therefore performs that write at the TS statement position, right before
/// the `addExecution` (`docs/requests/P7.md` R2). `setTrainStation` is idempotent, so a second
/// write by P5 is harmless.
pub(crate) fn add_train_station_exec(cx: &mut Ctx, unit: UnitRef) {
    // TS: TrainStationExecution.ts:27-32 (constructor side effect)
    cx.unit_mut(unit).set_train_station(true);
    cx.st.add_execution(Exec::TrainStation(TrainStationExec::new(unit, None)));
}

impl ExecBehavior for CityExec {
    fn is_active(&self) -> bool {
        self.active
    }
    /// `activeDuringSpawnPhase()` returns `false` (`CityExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    /// TS: :19-21 `this.mg = mg`.
    fn init(&mut self, _cx: &mut Ctx, _ticks: i32) {
        self.initialized = true;
    }
    fn tick(&mut self, cx: &mut Ctx, _ticks: i32) {
        // TS: :24-27
        if !self.station_created {
            self.create_station(cx);
            self.station_created = true;
        }
        // TS: :28-31
        if !cx.st.unit(self.city).is_active() {
            self.active = false;
        }
    }
}

/// `City v1`: `{ active, initialized, stationCreated, city }`.
impl Canon for CityExec {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        crate::canon_obj!(cx;
            "active" => self.active,
            "initialized" => self.initialized,
            "stationCreated" => self.station_created,
            "city" => self.city,
        )
    }
}
