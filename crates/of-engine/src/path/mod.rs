//! Path chains, steppers, spatial queries and the Bezier line (FILE_MAP rows 43-59). Owner: **P4b**.
//! ARCHITECTURE §3.8 (chain slots + memo are per-game state).
//!
//! ## `MapRead`: the `GameMap` surface the path code reads
//!
//! Every algorithm in `path/` reads the map through [`MapRead`], a 1:1 subset of the TS
//! `GameMap` interface (`engine-api/src/game/GameMap.ts`) with the same method names, so the
//! generic bodies read like the TS. Production instantiates it with [`GameMap`] only, and
//! that impl is pure delegation to P3's inherent methods (`map/game_map.rs`).
//!
//! The S1 test-only shim (literal `GameMapImpl.ts` reads over the public fields) was deleted
//! at merge1 (`docs/requests/P4b.md` R1): tests now run against P3's real `GameMap`.

pub mod air;
pub mod astar;
pub mod astar_rail;
pub mod astar_water;
pub mod astar_water_bounded;
pub mod builder;
pub mod line;
pub mod parabola;
pub mod pathfinder;
pub mod spatial_query;
pub mod station;
pub mod stepper;
pub mod transform;
pub mod types;

#[cfg(test)]
pub(crate) mod testkit;
#[cfg(test)]
mod vectors;

pub use types::{PathResult, PathStatus, Src};

use crate::ids::TileRef;
use crate::map::GameMap;

/// The read-only `GameMap` members the path code uses (TS names, `GameMap.ts`).
pub trait MapRead {
    fn width(&self) -> u32;
    fn height(&self) -> u32;
    /// `x(ref) = ref % width`.
    fn x(&self, r: TileRef) -> u32;
    /// `y(ref) = (ref / width) | 0`.
    fn y(&self, r: TileRef) -> u32;
    /// `ref(x, y)`: throws `Invalid coordinates: x,y` when `!isValidCoord(x, y)`.
    fn ref_(&self, x: f64, y: f64) -> TileRef;
    /// `isValidRef(ref)`: `Number.isInteger(ref) && ref >= 0 && ref < w*h`.
    fn is_valid_ref(&self, r: f64) -> bool;
    fn is_land(&self, r: TileRef) -> bool;
    fn is_water(&self, r: TileRef) -> bool;
    fn is_shoreline(&self, r: TileRef) -> bool;
    fn is_shore(&self, r: TileRef) -> bool;
    fn is_impassable(&self, r: TileRef) -> bool;
    /// The raw terrain bytes (`(map as any).terrain`, read by the inlined A*s and the
    /// smoother: `AStar.Water.ts:37`, `AStar.WaterBounded.ts:47`,
    /// `SmoothingWaterTransformer.ts:34`). TS edits this array in place (`setWater`,
    /// `setMagnitude`), so a read at call time sees what the TS reference sees.
    fn terrain(&self) -> &[u8];
    /// `neighbors4(ref, out)`: N, S, W, E.
    fn neighbors4(&self, r: TileRef, out: &mut [TileRef; 4]) -> usize;
    fn manhattan_dist(&self, a: TileRef, b: TileRef) -> u32;
    /// `ownerID(ref)`: `state[ref] & PLAYER_ID_MASK`.
    fn owner_id(&self, r: TileRef) -> u16;
}

impl MapRead for GameMap {
    #[inline]
    fn width(&self) -> u32 {
        GameMap::width(self)
    }
    #[inline]
    fn height(&self) -> u32 {
        GameMap::height(self)
    }
    #[inline]
    fn x(&self, r: TileRef) -> u32 {
        GameMap::x(self, r)
    }
    #[inline]
    fn y(&self, r: TileRef) -> u32 {
        GameMap::y(self, r)
    }
    #[inline]
    fn ref_(&self, x: f64, y: f64) -> TileRef {
        GameMap::ref_(self, x, y)
    }
    #[inline]
    fn is_valid_ref(&self, r: f64) -> bool {
        GameMap::is_valid_ref(self, r)
    }
    #[inline]
    fn is_land(&self, r: TileRef) -> bool {
        GameMap::is_land(self, r)
    }
    #[inline]
    fn is_water(&self, r: TileRef) -> bool {
        GameMap::is_water(self, r)
    }
    #[inline]
    fn is_shoreline(&self, r: TileRef) -> bool {
        GameMap::is_shoreline(self, r)
    }
    #[inline]
    fn is_shore(&self, r: TileRef) -> bool {
        GameMap::is_shore(self, r)
    }
    #[inline]
    fn is_impassable(&self, r: TileRef) -> bool {
        GameMap::is_impassable(self, r)
    }
    #[inline]
    fn terrain(&self) -> &[u8] {
        &self.terrain
    }
    #[inline]
    fn neighbors4(&self, r: TileRef, out: &mut [TileRef; 4]) -> usize {
        GameMap::neighbors4(self, r, out)
    }
    #[inline]
    fn manhattan_dist(&self, a: TileRef, b: TileRef) -> u32 {
        GameMap::manhattan_dist(self, a, b)
    }
    #[inline]
    fn owner_id(&self, r: TileRef) -> u16 {
        GameMap::owner_id(self, r)
    }
}
