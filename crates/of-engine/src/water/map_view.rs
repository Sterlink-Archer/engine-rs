//! The map surface the water code reads and writes. Owner: **P4a**.
//!
//! Every P4a algorithm is written once against [`WaterMap`] / [`WaterMapMut`] and called with
//! the real [`GameMap`] (P3) in production; the impl below only forwards to P3's methods, so
//! semantics are P3's (`GameMapImpl.ts`). The traits exist so the water layer can be tested
//! on its own (a `GameMapImpl` shim in `water::test_support`) before P3's bodies land, and so
//! MOD-CC / MOD-PATH-WATER oracles can run on raw terrain bytes.
//!
//! [`WaterMap::terrain`] is TS's direct terrain access (`(map as any).terrain`, used by
//! `ConnectedComponents.ts:250`, `WaterManager.ts:788`, `AStar.WaterBounded.ts:47`): always
//! the **live** bytes (TS mutates its `Uint8Array` in place; Rust re-reads the COW `Arc`).

use crate::ids::TileRef;
use crate::map::GameMap;

/// Read access (the subset of `GameMap` the water layer uses).
pub(crate) trait WaterMap {
    fn width(&self) -> u32;
    fn height(&self) -> u32;
    /// Live terrain bytes (`terrain: Uint8Array`).
    fn terrain(&self) -> &[u8];
    fn x(&self, t: TileRef) -> u32;
    fn y(&self, t: TileRef) -> u32;
    /// `ref(x, y)`: throws `Invalid coordinates: …` (`GameMapImpl.ts:67-72`).
    fn ref_(&self, x: f64, y: f64) -> TileRef;
    fn is_valid_coord(&self, x: f64, y: f64) -> bool;
    fn is_land(&self, t: TileRef) -> bool;
    fn is_water(&self, t: TileRef) -> bool;
    fn is_ocean(&self, t: TileRef) -> bool;
    fn is_shoreline(&self, t: TileRef) -> bool;
    fn is_impassable(&self, t: TileRef) -> bool;
    fn has_owner(&self, t: TileRef) -> bool;
    fn has_fallout(&self, t: TileRef) -> bool;
    fn manhattan_dist(&self, a: TileRef, b: TileRef) -> u32;
    /// `neighbors(ref)` order N, S, W, E (`GameMapImpl.ts:313-325`).
    fn neighbors4(&self, t: TileRef, out: &mut [TileRef; 4]) -> usize;
    /// non-TS: terrain write generation (gates `seg_shared`, ARCHITECTURE §3.8).
    fn terrain_gen(&self) -> u32;
}

/// Terrain writes (`GameMapImpl.ts:177-205`) and the fallout clear of `WaterManager.ts:160-162`.
pub(crate) trait WaterMapMut: WaterMap {
    fn set_water(&mut self, t: TileRef);
    fn set_ocean(&mut self, t: TileRef);
    fn set_shoreline_bit(&mut self, t: TileRef);
    fn clear_shoreline_bit(&mut self, t: TileRef);
    fn set_magnitude(&mut self, t: TileRef, value: u8);
    fn set_fallout(&mut self, t: TileRef, value: bool);
}

impl WaterMap for GameMap {
    #[inline]
    fn width(&self) -> u32 {
        GameMap::width(self)
    }
    #[inline]
    fn height(&self) -> u32 {
        GameMap::height(self)
    }
    #[inline]
    fn terrain(&self) -> &[u8] {
        &self.terrain
    }
    #[inline]
    fn x(&self, t: TileRef) -> u32 {
        GameMap::x(self, t)
    }
    #[inline]
    fn y(&self, t: TileRef) -> u32 {
        GameMap::y(self, t)
    }
    #[inline]
    fn ref_(&self, x: f64, y: f64) -> TileRef {
        GameMap::ref_(self, x, y)
    }
    #[inline]
    fn is_valid_coord(&self, x: f64, y: f64) -> bool {
        GameMap::is_valid_coord(self, x, y)
    }
    #[inline]
    fn is_land(&self, t: TileRef) -> bool {
        GameMap::is_land(self, t)
    }
    #[inline]
    fn is_water(&self, t: TileRef) -> bool {
        GameMap::is_water(self, t)
    }
    #[inline]
    fn is_ocean(&self, t: TileRef) -> bool {
        GameMap::is_ocean(self, t)
    }
    #[inline]
    fn is_shoreline(&self, t: TileRef) -> bool {
        GameMap::is_shoreline(self, t)
    }
    #[inline]
    fn is_impassable(&self, t: TileRef) -> bool {
        GameMap::is_impassable(self, t)
    }
    #[inline]
    fn has_owner(&self, t: TileRef) -> bool {
        GameMap::has_owner(self, t)
    }
    #[inline]
    fn has_fallout(&self, t: TileRef) -> bool {
        GameMap::has_fallout(self, t)
    }
    #[inline]
    fn manhattan_dist(&self, a: TileRef, b: TileRef) -> u32 {
        GameMap::manhattan_dist(self, a, b)
    }
    #[inline]
    fn neighbors4(&self, t: TileRef, out: &mut [TileRef; 4]) -> usize {
        GameMap::neighbors4(self, t, out)
    }
    #[inline]
    fn terrain_gen(&self) -> u32 {
        self.terrain_gen
    }
}

impl WaterMapMut for GameMap {
    #[inline]
    fn set_water(&mut self, t: TileRef) {
        GameMap::set_water(self, t)
    }
    #[inline]
    fn set_ocean(&mut self, t: TileRef) {
        GameMap::set_ocean(self, t)
    }
    #[inline]
    fn set_shoreline_bit(&mut self, t: TileRef) {
        GameMap::set_shoreline_bit(self, t)
    }
    #[inline]
    fn clear_shoreline_bit(&mut self, t: TileRef) {
        GameMap::clear_shoreline_bit(self, t)
    }
    #[inline]
    fn set_magnitude(&mut self, t: TileRef, value: u8) {
        GameMap::set_magnitude(self, t, value)
    }
    #[inline]
    fn set_fallout(&mut self, t: TileRef, value: bool) {
        GameMap::set_fallout(self, t, value)
    }
}
