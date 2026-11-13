//! `engine/src/execution/Util.ts`. Owner: **P6**.
//!
//! `getSpawnTiles` runs `gm.bfs(..)`, which needs the main map's BFS scratch
//! (`sc.bfs_main`, ARCHITECTURE §3.6.1). The frozen S0 signature [`get_spawn_tiles`] has no
//! scratch argument, so the scratch-taking form is [`get_spawn_tiles_with`] (what
//! `SpawnExecution` calls); the frozen one delegates with a private scratch (correct, but it
//! allocates a map-sized stamp array per call). Request #2 in `docs/requests/P6.md`.
//!
//! `nearestTileDistCapped` dispatches on `isTileSetLike(tiles)` (`:81-97`): a set with `has`
//! takes the ring search, a plain iterable the linear scan. Two functions here (PORTING_GUIDE
//! §10 `instanceof` rule): [`nearest_tile_dist_capped`] (iterable) and
//! [`nearest_tile_dist_capped_set`] (set: `size` + `has`).

use crate::ids::{SmallId, TileRef};
use crate::map::GameMap;
use crate::scratch::bfs::BfsScratch;
use crate::state::State;

/// `getSpawnTiles(gm, tile, requireAllValid = false)` (`:6-35`): `None` only when
/// `require_all_valid` and a tile is invalid. Prefer [`get_spawn_tiles_with`] (module doc).
pub fn get_spawn_tiles(gm: &GameMap, tile: TileRef, require_all_valid: bool) -> Option<Vec<TileRef>> {
    let mut sc = BfsScratch::default();
    get_spawn_tiles_with(gm, &mut sc, tile, require_all_valid)
}

/// [`get_spawn_tiles`] on the caller's BFS scratch of `gm` (`sc.bfs_main` for the main map).
pub fn get_spawn_tiles_with(
    gm: &GameMap,
    sc: &mut BfsScratch,
    tile: TileRef,
    require_all_valid: bool,
) -> Option<Vec<TileRef>> {
    // TS: :21 `Array.from(gm.bfs(tile, euclDistFN(tile, 4, true)))`
    let spawn_tiles: Vec<TileRef> = gm.bfs(sc, tile, |g, n| eucl_dist_center(g, tile, 4.0, n)).to_vec();

    // TS: :23-24
    let is_invalid = |t: TileRef| gm.has_owner(t) || !gm.is_land(t) || gm.is_impassable(t);

    if !require_all_valid {
        return Some(spawn_tiles.into_iter().filter(|&t| !is_invalid(t)).collect());
    }
    if spawn_tiles.iter().any(|&t| is_invalid(t)) {
        return None;
    }
    Some(spawn_tiles)
}

/// `euclDistFN(root, dist, center = true)` (`engine-lib/src/game/GameMapImpl.ts:665-686`):
/// the root's coordinates are shifted by -0.5 so the disc is centred on a pixel corner.
fn eucl_dist_center(gm: &GameMap, root: TileRef, dist: f64, n: TileRef) -> bool {
    let dist2 = dist * dist;
    let root_x = f64::from(gm.x(root)) - 0.5;
    let root_y = f64::from(gm.y(root)) - 0.5;
    let dx = f64::from(gm.x(n)) - root_x;
    let dy = f64::from(gm.y(n)) - root_y;
    dx * dx + dy * dy <= dist2
}

/// `closestTile(gm, refs, tile): [TileRef | null, number]` (`:37-57`).
pub fn closest_tile(gm: &GameMap, refs: impl IntoIterator<Item = TileRef>, tile: TileRef) -> (Option<TileRef>, f64) {
    let mut min_distance = f64::INFINITY;
    let mut min_ref: Option<TileRef> = None;
    for r in refs {
        let distance = f64::from(gm.manhattan_dist(r, tile));
        if distance < min_distance {
            min_distance = distance;
            min_ref = Some(r);
        }
    }
    (min_ref, min_distance)
}

/// `nearestTileDist(gm, tiles, tile)` (`:59-70`): `Infinity` for an empty `tiles`.
pub fn nearest_tile_dist(gm: &GameMap, tiles: impl IntoIterator<Item = TileRef>, tile: TileRef) -> f64 {
    let mut best = f64::INFINITY;
    for t in tiles {
        let d = f64::from(gm.manhattan_dist(t, tile));
        if d < best {
            best = d;
        }
    }
    best
}

/// `nearestTileDistCapped(gm, tiles, tile, cap)` with a plain iterable (`:93-97`: the
/// linear-scan fallback).
pub fn nearest_tile_dist_capped(
    gm: &GameMap,
    tiles: impl IntoIterator<Item = TileRef>,
    tile: TileRef,
    cap: f64,
) -> f64 {
    let d = nearest_tile_dist(gm, tiles, tile);
    if d <= cap { d } else { f64::INFINITY }
}

/// `nearestTileDistCapped(gm, tiles, tile, cap)` with a `ReadonlyTileSet` (`:98-117`): the
/// Manhattan-ring search over `has`. `size` is `tiles.size`.
pub fn nearest_tile_dist_capped_set(
    gm: &GameMap,
    size: usize,
    mut has: impl FnMut(TileRef) -> bool,
    tile: TileRef,
    cap: f64,
) -> f64 {
    if size == 0 {
        return f64::INFINITY;
    }
    if has(tile) {
        return 0.0;
    }
    // Coordinates as JS numbers: `x`/`y1` can go negative (checked before the lookup).
    let w = i64::from(gm.width());
    let h = i64::from(gm.height());
    let cx = i64::from(gm.x(tile));
    let cy = i64::from(gm.y(tile));
    let mut d: i64 = 1;
    while (d as f64) <= cap {
        for dx in -d..=d {
            let x = cx + dx;
            if x < 0 || x >= w {
                continue;
            }
            let dy = d - dx.abs();
            let y1 = cy - dy;
            // int: 0 <= y1 < h and 0 <= x < w, so the ref is a valid u32 tile ref.
            if y1 >= 0 && has((y1 * w + x) as TileRef) {
                return d as f64;
            }
            if dy != 0 {
                let y2 = cy + dy;
                if y2 < h && has((y2 * w + x) as TileRef) {
                    return d as f64;
                }
            }
        }
        d += 1;
    }
    f64::INFINITY
}

/// `closestTwoTiles(gm, x, y)` (`:120-172`; `js_sort_by` at `:128-129`; the literal
/// `(currentX / w) | 0` row term at `:148`). The core is `of_det::util::closest_two_tiles`.
pub fn closest_two_tiles(gm: &GameMap, x: &[TileRef], y: &[TileRef]) -> Option<(TileRef, TileRef)> {
    of_det::util::closest_two_tiles(gm.width(), x, y)
}

/// `calculateTerritoryCenter(game, target)` (`:182-230`).
pub fn calculate_territory_center(st: &State, target: SmallId) -> Option<TileRef> {
    let border_tiles = &st.player_data(target).border_tiles;
    if border_tiles.size() == 0 {
        return None;
    }
    let map = &st.map;
    // TS: :190-202 (one pass over the border tiles)
    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for tile in border_tiles.iter() {
        let x = f64::from(map.x(tile));
        let y = f64::from(map.y(tile));
        if x < min_x {
            min_x = x;
        }
        if x > max_x {
            max_x = x;
        }
        if y < min_y {
            min_y = y;
        }
        if y > max_y {
            max_y = y;
        }
    }
    let center_x = of_det::js::floor((min_x + max_x) / 2.0);
    let center_y = of_det::js::floor((min_y + max_y) / 2.0);
    let center_tile = map.ref_(center_x, center_y);

    // TS: :210-212 (`game.owner(centerTile) === target`)
    if st.map.owner_id(center_tile) == target.0 {
        return Some(center_tile);
    }

    // TS: :215-229 (nearest border tile; strict `<` keeps the first)
    let mut closest: Option<TileRef> = None;
    let mut closest_distance_squared = f64::INFINITY;
    for tile in border_tiles.iter() {
        let dx = f64::from(map.x(tile)) - center_x;
        let dy = f64::from(map.y(tile)) - center_y;
        let dist_squared = dx * dx + dy * dy;
        if dist_squared < closest_distance_squared {
            closest_distance_squared = dist_squared;
            closest = Some(tile);
        }
    }
    closest
}
