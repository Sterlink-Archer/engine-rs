//! CS v1 export of the rail records and the four rail executions (ARCHITECTURE §9, SPEC §1.2,
//! §1.3 R/E). Owner: **P5**.
//!
//! Fields follow the snapshot dump order (`conformance/schema/snapshot_schemas_5b75bd3.txt`:
//! `TrainStation v1`, `Railroad v1`, `Cluster v1`, `RailNetwork v1`, and the `Factory`,
//! `RecomputeRailCluster`, `Train`, `TrainStation` execution records); references become
//! SPEC §1.2 keys (station id, railroad id, cluster key, unit id, smallID). The R-section
//! builder (P13) adds the cluster row `key` (SPEC N6) and decides which rows exist; these
//! impls only render one value each.

use crate::canon::{Canon, CanonCx, Cs};
use crate::exec::factory::FactoryExec;
use crate::exec::rail_cluster::RecomputeRailClusterExec;
use crate::exec::train::TrainExec;
use crate::exec::train_station::TrainStationExec;
use crate::ids::TileRef;
use crate::rail::grid::RailSpatialGrid;
use crate::rail::network::{RailNetwork, StationManager};
use crate::rail::railroad::{OrientedRailroad, Railroad};
use crate::rail::station::{Cluster, TrainStation};
use crate::{canon_obj, impl_canon_record};
use of_canon::Typed;

// ---- keys ----
// The StationId / RailroadId / RailroadRef key impls live in `canon/leaves.rs` (P13 contract,
// docs/requests/P13.md item 1): they emit the SPEC §1.2 key AND queue the row (merge1).

/// `zTiles()`: a `Uint32Array` (SPEC §1.1 `tiles`, TYPED kind 5).
fn tiles(t: &[TileRef]) -> Cs {
    Cs::Typed(Typed::U32(t.to_vec()))
}

/// A JS `Set` of keys, in insertion order.
fn set_of<'a, K: Canon + 'a>(it: impl Iterator<Item = &'a K>, cx: &mut CanonCx<'_>) -> Cs {
    Cs::Arr(it.map(|k| k.canon(cx)).collect())
}

// ---- rows ----

/// `TrainStation.snapshot` (`TrainStation.ts:169-180`).
impl Canon for TrainStation {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        let railroads = set_of(self.railroads.iter(), cx);
        let by_neighbor =
            Cs::Arr(self.railroad_by_neighbor.iter().map(|(s, r)| Cs::Arr(vec![s.canon(cx), r.canon(cx)])).collect());
        Cs::Obj(vec![
            ("id".to_string(), self.id.canon(cx)),
            ("unit".to_string(), self.unit.canon(cx)),
            ("cluster".to_string(), self.cluster.canon(cx)),
            ("railroads".to_string(), railroads),
            ("railroadByNeighbor".to_string(), by_neighbor),
        ])
    }
}

/// `Railroad.snapshot` (`Railroad.ts:54-61`).
impl Canon for Railroad {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        Cs::Obj(vec![
            // The row's own id, not a reference: a detached railroad (reached through a
            // `RailroadRef`, e.g. `TrainExecution.currentRailroad`) is no longer in
            // `railroad_by_id`, so the `RailroadId` key leaf would fail on it (merge1: F-CI-c4/c9/c11
            // "reference to railroad id N, which has no slot").
            ("id".to_string(), Cs::Int(i64::from(self.id.0))),
            ("from".to_string(), self.from.canon(cx)),
            ("to".to_string(), self.to.canon(cx)),
            ("tiles".to_string(), tiles(&self.tiles)),
        ])
    }
}

/// `Cluster.snapshot` (`TrainStation.ts:298-303`); the row `key` is added by the R builder.
impl Canon for Cluster {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        let stations = set_of(self.stations.iter(), cx);
        let trade_stations = set_of(self.trade_stations.iter(), cx);
        Cs::Obj(vec![("stations".to_string(), stations), ("tradeStations".to_string(), trade_stations)])
    }
}

/// `StationManagerImpl.snapshot` (`RailNetworkImpl.ts:86-91`).
impl Canon for StationManager {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        let stations = set_of(self.stations.iter(), cx);
        Cs::Obj(vec![("stations".to_string(), stations), ("nextId".to_string(), self.next_id.canon(cx))])
    }
}

/// `RailSpatialGrid.snapshot` (`RailroadSpatialGrid.ts:86-98`): cells keyed by the TS string.
impl Canon for RailSpatialGrid {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        let cells = Cs::Arr(
            self.cells
                .iter()
                .map(|(&k, set)| Cs::Arr(vec![Cs::Str(RailSpatialGrid::cell_key_string(k)), set_of(set.iter(), cx)]))
                .collect(),
        );
        let rail_to_cells = Cs::Arr(
            self.rail_to_cells
                .iter()
                .map(|(r, keys)| {
                    let ks = keys.iter().map(|&k| Cs::Str(RailSpatialGrid::cell_key_string(k))).collect();
                    Cs::Arr(vec![r.canon(cx), Cs::Arr(ks)])
                })
                .collect(),
        );
        Cs::Obj(vec![("cells".to_string(), cells), ("railToCells".to_string(), rail_to_cells)])
    }
}

/// `RailNetworkImpl.snapshot` (`RailNetworkImpl.ts:950-958`).
impl Canon for RailNetwork {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        let station_manager = self.station_manager.canon(cx);
        let dirty_clusters = set_of(self.dirty_clusters.iter(), cx);
        let rail_grid = self.rail_grid.canon(cx);
        Cs::Obj(vec![
            ("stationManager".to_string(), station_manager),
            ("nextId".to_string(), self.next_id.canon(cx)),
            ("dirtyClusters".to_string(), dirty_clusters),
            ("railGrid".to_string(), rail_grid),
        ])
    }
}

/// `currentRailroad: { railroad, forward }` (`TrainExecution.ts:409,420-423`).
impl Canon for OrientedRailroad {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        canon_obj!(cx; "railroad" => self.railroad, "forward" => self.forward)
    }
}

// ---- execution records ----

impl_canon_record!(FactoryExec {
    "active" => active,
    "initialized" => initialized,
    "stationCreated" => station_created,
    "factory" => factory,
});

/// `RecomputeRailClusterExecutionSnapshot.write({})`.
impl Canon for RecomputeRailClusterExec {
    fn canon(&self, _cx: &mut CanonCx<'_>) -> Cs {
        Cs::Obj(Vec::new())
    }
}

/// `TrainExecution.snapshot` (`TrainExecution.ts:405-433`).
impl Canon for TrainExec {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        let used: Vec<TileRef> = self.used_tiles.iter().copied().collect();
        Cs::Obj(vec![
            ("active".to_string(), self.active.canon(cx)),
            ("initialized".to_string(), self.initialized.canon(cx)),
            ("train".to_string(), self.train.canon(cx)),
            ("cars".to_string(), self.cars.canon(cx)),
            ("hasCargo".to_string(), self.has_cargo.canon(cx)),
            ("currentTile".to_string(), self.current_tile.canon(cx)),
            ("spacing".to_string(), self.spacing.canon(cx)),
            ("usedTiles".to_string(), tiles(&used)),
            ("stations".to_string(), self.stations.canon(cx)),
            ("currentRailroad".to_string(), self.current_railroad.canon(cx)),
            ("speed".to_string(), self.speed.canon(cx)),
            ("tradeStopsVisited".to_string(), self.trade_stops_visited.canon(cx)),
            ("pathTiles".to_string(), tiles(&self.path_tiles)),
            ("pathIndex".to_string(), self.path_index.canon(cx)),
            ("player".to_string(), self.player.canon(cx)),
            ("source".to_string(), self.source.canon(cx)),
            ("destination".to_string(), self.destination.canon(cx)),
            ("numCars".to_string(), self.num_cars.canon(cx)),
        ])
    }
}

/// `TrainStationExecution.snapshot` (`TrainStationExecution.ts:246-259`). `spawnTrains` is
/// `boolean | undefined` (`?undef`: absent when undefined); `random` is the sfc32 words or
/// `null` (created only by `init` of a train-spawning station).
impl Canon for TrainStationExec {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        let spawn_trains = match self.spawn_trains {
            Some(b) => Cs::Bool(b),
            None => Cs::Absent,
        };
        let random = match &self.rng {
            Some(r) => Cs::Arr(r.get_state().iter().map(|&w| Cs::Int(i64::from(w))).collect()),
            None => Cs::Null,
        };
        Cs::Obj(vec![
            ("active".to_string(), self.active.canon(cx)),
            ("initialized".to_string(), self.initialized.canon(cx)),
            ("unit".to_string(), self.unit.canon(cx)),
            ("spawnTrains".to_string(), spawn_trains),
            ("random".to_string(), random),
            ("station".to_string(), self.station.canon(cx)),
            ("numCars".to_string(), self.num_cars.canon(cx)),
            ("lastSpawnTick".to_string(), self.last_spawn_tick.canon(cx)),
            ("ticksCooldown".to_string(), self.ticks_cooldown.canon(cx)),
        ])
    }
}
