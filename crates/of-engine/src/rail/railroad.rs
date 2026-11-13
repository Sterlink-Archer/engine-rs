//! `Railroad` / `OrientedRailroad` (`game/Railroad.ts`). Owner: **P5**.
//!
//! A railroad lives in `RailNetwork.railroads` (arena) and is named by its TS id
//! ([`RailroadId`]) wherever TS keys a `Set`/`Map` by the object (station sets, the rail grid);
//! `railroad_by_id` resolves the id while the railroad is attached to the network. Executions
//! that keep a railroad object (`TrainExecution.currentRailroad`) hold its [`RailroadRef`],
//! so a detached railroad stays readable until the GC sees no reference (ARCHITECTURE §3.11).

use crate::StateClone;
use crate::ctx::Ctx;
use crate::ids::{RailroadId, RailroadRef, StationId, TileRef};
use crate::rail::env::{RailEnv, RailRead};
use crate::rail::station::station_remove_railroad_env;
use crate::state::State;

/// `Railroad` data, `Railroad v1` snapshot order (`:18-23`).
#[derive(StateClone, Debug, Default, PartialEq, Eq)]
pub struct Railroad {
    pub id: RailroadId,
    pub from: StationId,
    pub to: StationId,
    pub tiles: Vec<TileRef>,
    /// non-snapshot: deleted (unregistered) railroads stay readable while referenced (GC).
    pub deleted: bool,
}

impl Railroad {
    /// `new Railroad(from, to, tiles, id)` (`:17-23`).
    pub fn new(from: StationId, to: StationId, tiles: Vec<TileRef>, id: RailroadId) -> Self {
        Railroad { id, from, to, tiles, deleted: false }
    }
}

/// `railroad.getClosestTileIndex(game, to)` (`:34-52`): `-1` for an empty railroad.
pub(crate) fn closest_tile_index_env<E: RailRead + ?Sized>(e: &E, tiles: &[TileRef], to: TileRef) -> i64 {
    if tiles.is_empty() {
        return -1;
    }
    // int: tile coordinates are < 2^16, so the squared distances below are exact in f64 as in
    // TS; f64 keeps the `Infinity` initial value literal.
    let to_x = f64::from(e.tile_x(to));
    let to_y = f64::from(e.tile_y(to));
    let mut closest_index: i64 = 0;
    let mut min_dist_squared = f64::INFINITY;
    for (i, &tile) in tiles.iter().enumerate() {
        let dx = f64::from(e.tile_x(tile)) - to_x;
        let dy = f64::from(e.tile_y(tile)) - to_y;
        let dist_squared = dx * dx + dy * dy;

        if dist_squared < min_dist_squared {
            min_dist_squared = dist_squared;
            closest_index = i as i64; // int: a Vec index
        }
    }
    closest_index
}

impl State {
    /// `railroad.getClosestTileIndex(game, to)` (`:34-52`). TS returns `-1` for an empty
    /// railroad; that is `usize::MAX` here (no caller outside this package).
    pub fn railroad_closest_tile_index(&self, r: RailroadRef, to: TileRef) -> usize {
        let tiles = &self.rail.railroads.get(r).tiles;
        usize::try_from(closest_tile_index_env(self, tiles, to)).unwrap_or(usize::MAX)
    }
}

/// `railroad.delete(game)` (`:25-32`): the destruction update is view-only (dropped).
pub(crate) fn railroad_delete_env<E: RailEnv + ?Sized>(e: &mut E, r: RailroadId) {
    let (from, to) = {
        let rr = e.rail().railroad(r);
        (rr.from, rr.to)
    };
    station_remove_railroad_env(e, from, r);
    station_remove_railroad_env(e, to, r);
}

impl Ctx<'_> {
    /// `railroad.delete(game)` (`:25-32`).
    pub fn railroad_delete(&mut self, r: RailroadRef) {
        let id = self.st.rail.railroads.get(r).id;
        railroad_delete_env(self, id);
    }
}

/// `OrientedRailroad` (`:98-125`); snapshot `currentRailroad { railroad, forward }`.
#[derive(StateClone, Debug, Default, PartialEq, Eq)]
pub struct OrientedRailroad {
    pub railroad: RailroadRef,
    pub forward: bool,
    /// non-snapshot: the oriented tile copy (`:99`). A forward one shares the railroad's
    /// array in TS; railroad tiles are never mutated in place (every edit is a `slice`), so a
    /// copy reads the same values.
    pub tiles: Vec<TileRef>,
}

impl OrientedRailroad {
    /// `new OrientedRailroad(railroad, forward)`.
    pub fn new(st: &State, railroad: RailroadRef, forward: bool) -> Self {
        Self::of_tiles(railroad, forward, &st.rail.railroads.get(railroad).tiles)
    }
    /// The constructor body (`:100-107`) on the railroad's tiles.
    pub(crate) fn of_tiles(railroad: RailroadRef, forward: bool, tiles: &[TileRef]) -> Self {
        let tiles = if forward {
            tiles.to_vec()
        } else {
            // TS: [...this.railroad.tiles].reverse()
            tiles.iter().rev().copied().collect()
        };
        OrientedRailroad { railroad, forward, tiles }
    }
    /// `getTiles()`.
    #[inline]
    pub fn get_tiles(&self) -> &[TileRef] {
        &self.tiles
    }
    /// `getStart()`.
    pub fn get_start(&self, st: &State) -> StationId {
        self.start_env(st)
    }
    /// `getEnd()`.
    pub fn get_end(&self, st: &State) -> StationId {
        self.end_env(st)
    }
    pub(crate) fn start_env<E: RailRead + ?Sized>(&self, e: &E) -> StationId {
        let r = e.rail().railroads.get(self.railroad);
        if self.forward { r.from } else { r.to }
    }
    pub(crate) fn end_env<E: RailRead + ?Sized>(&self, e: &E) -> StationId {
        let r = e.rail().railroads.get(self.railroad);
        if self.forward { r.to } else { r.from }
    }
}

/// `getOrientedRailroad(from, to)` (`:84-96`).
pub fn get_oriented_railroad(st: &State, from: StationId, to: StationId) -> Option<OrientedRailroad> {
    get_oriented_railroad_env(st, from, to)
}

/// `getOrientedRailroad(from, to)` (`:84-96`) over the rail env.
pub(crate) fn get_oriented_railroad_env<E: RailRead + ?Sized>(
    e: &E,
    from: StationId,
    to: StationId,
) -> Option<OrientedRailroad> {
    let rail = e.rail();
    let id = rail.station_obj(from).railroad_by_neighbor.get(&to).copied()?;
    let r = rail.railroad_ref(id);
    let rr = rail.railroads.get(r);
    // If tiles are stored from -> to, we go forward when railroad.to === to
    let forward = rr.to == to;
    Some(OrientedRailroad::of_tiles(r, forward, &rr.tiles))
}
