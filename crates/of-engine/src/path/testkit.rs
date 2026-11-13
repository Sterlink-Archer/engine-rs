//! Test fixtures for `path/` (port of `tests/core/pathfinding/_fixtures.ts`). Owner: **P4b**.
//! `#[cfg(test)]` only; maps are built on `GameMap`'s public fields and read through the
//! `MapRead` test shim (`path/mod.rs`).

use crate::ids::TileRef;
use crate::map::{GameMap, TileStateBuf};
use crate::path::MapRead;
use crate::state::State;
use std::sync::Arc;

// Terrain encoding (`_fixtures.ts:20-23`)
pub const WATER_BIT: u8 = 0x20;
pub const LAND_BIT: u8 = 0x80;
pub const SHORELINE_BIT: u8 = 6;

/// `computeShoreline(terrain, width, height)` (`_fixtures.ts:31-58`).
fn compute_shoreline(terrain: &mut [u8], width: u32, height: u32) {
    for y in 0..height as i64 {
        for x in 0..width as i64 {
            let idx = (y * width as i64 + x) as usize;
            let is_land = terrain[idx] & LAND_BIT != 0;
            for (nx, ny) in [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)] {
                if nx < 0 || nx >= width as i64 || ny < 0 || ny >= height as i64 {
                    continue;
                }
                let neighbor_is_land = terrain[(ny * width as i64 + nx) as usize] & LAND_BIT != 0;
                if is_land != neighbor_is_land {
                    terrain[idx] |= 1 << SHORELINE_BIT;
                    break;
                }
            }
        }
    }
}

/// A `GameMap` from raw terrain bytes (`new GameMapImpl(w, h, terrain, numLand)`), with a
/// full (owner-capable) state buffer.
pub fn map_from_terrain(width: u32, height: u32, terrain: Vec<u8>) -> GameMap {
    assert_eq!(terrain.len(), (width * height) as usize);
    let num_land = terrain.iter().filter(|&&b| b & LAND_BIT != 0).count() as u32;
    let mut m = GameMap::default();
    m.w = width;
    m.h = height;
    m.terrain = Arc::from(terrain);
    m.num_land_tiles = num_land;
    *m.state_buf_mut() = TileStateBuf::new_full((width * height) as usize);
    m
}

/// `createGameMap(data)` (`_fixtures.ts:170-185`): `grid` rows of `W`/`L`, whitespace ignored.
pub fn game_map(width: u32, height: u32, grid: &str) -> GameMap {
    let cells: Vec<char> = grid.chars().filter(|c| !c.is_whitespace()).collect();
    assert_eq!(cells.len(), (width * height) as usize, "grid size");
    let mut terrain: Vec<u8> = cells.iter().map(|&c| if c == 'L' { LAND_BIT } else { WATER_BIT }).collect();
    compute_shoreline(&mut terrain, width, height);
    map_from_terrain(width, height, terrain)
}

/// The minimap of `createGame` (`_fixtures.ts:98-127`): 2×2 → 1, water if ANY is water.
pub fn mini_of(map: &GameMap) -> GameMap {
    let (width, height) = (map.w, map.h);
    let mini_width = width.div_ceil(2);
    let mini_height = height.div_ceil(2);
    let mut mini_terrain = vec![0u8; (mini_width * mini_height) as usize];
    for my in 0..mini_height {
        for mx in 0..mini_width {
            let m_idx = (my * mini_width + mx) as usize;
            let mut has_water = false;
            for dy in 0..2 {
                for dx in 0..2 {
                    let x = mx * 2 + dx;
                    let y = my * 2 + dy;
                    if x < width && y < height && map.terrain[(y * width + x) as usize] & LAND_BIT == 0 {
                        has_water = true;
                    }
                }
            }
            mini_terrain[m_idx] = if has_water { WATER_BIT } else { LAND_BIT };
        }
    }
    compute_shoreline(&mut mini_terrain, mini_width, mini_height);
    let mut m = map_from_terrain(mini_width, mini_height, mini_terrain);
    *m.state_buf_mut() = TileStateBuf::new_zero((mini_width * mini_height) as usize);
    m
}

/// `createIslandMap()` (`_fixtures.ts:60-71`): 5×5, a 3×3 island in the middle.
pub const ISLAND: &str = "
    WWWWW
    WLLLW
    WLLLW
    WLLLW
    WWWWW";

/// A `State` whose main map is `grid` and whose minimap is derived as `createGame` does;
/// no nav mesh (`water.graph = None`, the simple chain).
pub fn state_with(width: u32, height: u32, grid: &str) -> State {
    let mut st = State::default();
    st.map = game_map(width, height, grid);
    st.mini = mini_of(&st.map);
    st
}

/// `map.ref(x, y)` for tests.
pub fn r(map: &GameMap, x: u32, y: u32) -> TileRef {
    MapRead::ref_(map, f64::from(x), f64::from(y))
}

/// Set a magnitude on every water tile (`terrain & ~0x1f | mag`), e.g. to let the smoother's
/// line-of-sight pass.
pub fn set_water_magnitude(map: &mut GameMap, mag: u8) {
    let mut t = map.terrain.to_vec();
    for b in t.iter_mut() {
        if *b & LAND_BIT == 0 {
            *b = (*b & !0x1f) | (mag & 0x1f);
        }
    }
    map.terrain = Arc::from(t);
}
