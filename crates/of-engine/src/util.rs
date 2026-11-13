//! `engine-lib/src/Util.ts` (FILE_MAP row 4). Owner: **P1**. The pure numeric helpers are in
//! `of-det::util` (VECTORS `util`) and re-exported; the map-dependent ones are here.

use crate::api::types::Cell;
use crate::ids::TileRef;
use crate::map::GameMap;
use crate::state::player::BoundingBox;
use of_det::js;

pub use of_det::util::{
    find_minimum_by, get_mode, manhattan_dist_wrapped, sigmoid, simple_hash, to_int_strict, within,
};

/// `distSort(gm, target)` (`:28-35`): the comparator `(a, b) => d(a) - d(b)` for `js_sort_by`.
/// `manhattanDist` is a non-negative integer, so the f64 difference is exact.
pub fn dist_sort(gm: &GameMap, target: TileRef, a: TileRef, b: TileRef) -> f64 {
    f64::from(gm.manhattan_dist(a, target)) - f64::from(gm.manhattan_dist(b, target))
}

/// `distSortUnit(gm, target)` (`:37-53`) comparator over unit tiles; `target` is a tile here
/// (a `UnitLike` target is passed as its tile by the caller: `target.tile()` is read once,
/// when the comparator is built, `:41`).
pub fn dist_sort_unit(gm: &GameMap, target: TileRef, a_tile: TileRef, b_tile: TileRef) -> f64 {
    f64::from(gm.manhattan_dist(a_tile, target)) - f64::from(gm.manhattan_dist(b_tile, target))
}

/// `findClosestBy(values, distance, isCandidate?)` (`:97-103`) = `findMinimumBy`: the index of
/// the first value with the strictly smallest distance (`null` → `None`).
pub fn find_closest_by<T>(
    values: &[T],
    distance: impl FnMut(&T) -> f64,
    is_candidate: Option<impl FnMut(&T) -> bool>,
) -> Option<usize> {
    find_minimum_by(values, distance, is_candidate)
}

/// `calculateBoundingBox(gm, borderTiles)` (`:115-147`). Every input path (array, `Set`/
/// `TileSet` `forEach`, iterator) visits in iteration order, and `visit` is order-insensitive
/// except for NaN/-0 (impossible: coordinates are non-negative integers). An empty input
/// gives `min = (Infinity, Infinity)`, `max = (-Infinity, -Infinity)`, as in TS.
///
/// Since every coordinate is a non-negative integer (`+0`, never `-0` or NaN), the JS
/// `Math.min`/`Math.max` folds equal integer min/max in `u32`, converted once at the end
/// (P3 perf, t066: the f64 fold over a nation's border was 3% of a World game).
pub fn calculate_bounding_box(gm: &GameMap, tiles: impl IntoIterator<Item = TileRef>) -> BoundingBox {
    let w = gm.width();
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (u32::MAX, u32::MAX, 0u32, 0u32);
    let mut any = false;
    for tile in tiles {
        let (x, y) = (tile % w, tile / w); // == gm.x(tile), gm.y(tile)
        any = true;
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    if !any {
        return BoundingBox {
            min: Cell::new(f64::INFINITY, f64::INFINITY),
            max: Cell::new(f64::NEG_INFINITY, f64::NEG_INFINITY),
        };
    }
    BoundingBox {
        min: Cell::new(f64::from(min_x), f64::from(min_y)),
        max: Cell::new(f64::from(max_x), f64::from(max_y)),
    }
}

/// `boundingBoxTiles(gm, center, radius)` (`:149-185`): the ring of tiles on the box edge, in
/// TS push order (top/bottom pairs per column, then left/right pairs per row). `radius` is a
/// JS number: the loops run in f64 exactly as written, and `isValidCoord` rejects any
/// non-integer coordinate.
pub fn bounding_box_tiles(gm: &GameMap, center: TileRef, radius: f64) -> Vec<TileRef> {
    let mut tiles = Vec::new();
    let center_x = f64::from(gm.x(center));
    let center_y = f64::from(gm.y(center));
    let min_x = center_x - radius;
    let max_x = center_x + radius;
    let min_y = center_y - radius;
    let max_y = center_y + radius;
    // Top and bottom edges (full width)
    let mut x = min_x;
    while x <= max_x {
        if gm.is_valid_coord(x, min_y) {
            tiles.push(gm.ref_(x, min_y));
        }
        if gm.is_valid_coord(x, max_y) && min_y != max_y {
            tiles.push(gm.ref_(x, max_y));
        }
        x += 1.0;
    }
    // Left and right edges (exclude corners already added)
    let mut y = min_y + 1.0;
    while y < max_y {
        if gm.is_valid_coord(min_x, y) {
            tiles.push(gm.ref_(min_x, y));
        }
        if gm.is_valid_coord(max_x, y) && min_x != max_x {
            tiles.push(gm.ref_(max_x, y));
        }
        y += 1.0;
    }
    tiles
}

/// `calculateBoundingBoxCenter(gm, borderTiles)` (`:201-207`).
pub fn calculate_bounding_box_center(gm: &GameMap, tiles: impl IntoIterator<Item = TileRef>) -> Cell {
    let b = calculate_bounding_box(gm, tiles);
    bounding_box_center(&b)
}

/// `boundingBoxCenter(box)` (`:209-214`): `min + Math.floor((max - min) / 2)` per axis.
pub fn bounding_box_center(b: &BoundingBox) -> Cell {
    Cell::new(b.min.x + js::floor((b.max.x - b.min.x) / 2.0), b.min.y + js::floor((b.max.y - b.min.y) / 2.0))
}

/// `inscribed(outer, inner)` (`:216-226`).
pub fn inscribed(outer: &BoundingBox, inner: &BoundingBox) -> bool {
    outer.min.x <= inner.min.x && outer.min.y <= inner.min.y && outer.max.x >= inner.max.x && outer.max.y >= inner.max.y
}

/// `maxInt(a, b)` = `a > b ? a : b` (`:242-244`), `minInt(a, b)` = `a < b ? a : b` (`:246-248`)
/// and `withinInt(num, min, max)` (`:249-252`), on bigint (`i64`).
pub fn min_int(a: i64, b: i64) -> i64 {
    if a < b { a } else { b }
}
pub fn max_int(a: i64, b: i64) -> i64 {
    if a > b { a } else { b }
}
pub fn within_int(num: i64, min: i64, max: i64) -> i64 {
    // TS: `const atLeastMin = maxInt(num, min); return minInt(atLeastMin, max);` (`:249-252`)
    let at_least_min = max_int(num, min);
    min_int(at_least_min, max)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn int_helpers_follow_ts_statement_order() {
        assert_eq!(within_int(5, 0, 10), 5);
        assert_eq!(within_int(-1, 0, 10), 0);
        assert_eq!(within_int(11, 0, 10), 10);
        // min > max: TS gives minInt(maxInt(num, min), max) = max.
        assert_eq!(within_int(5, 10, 0), 0);
        assert_eq!((max_int(2, 3), min_int(2, 3)), (3, 2));
    }

    fn bb(x0: f64, y0: f64, x1: f64, y1: f64) -> BoundingBox {
        BoundingBox { min: Cell::new(x0, y0), max: Cell::new(x1, y1) }
    }

    #[test]
    fn bounding_box_center_floors_half_extent() {
        // TS: min + Math.floor((max - min) / 2)
        assert_eq!(bounding_box_center(&bb(2.0, 3.0, 7.0, 3.0)), Cell::new(4.0, 3.0));
        assert_eq!(bounding_box_center(&bb(0.0, 0.0, 1.0, 9.0)), Cell::new(0.0, 4.0));
        // The empty box (Infinity / -Infinity) gives NaN, as in JS: -Inf - Inf = -Inf,
        // floor(-Inf / 2) = -Inf, Inf + -Inf = NaN.
        let e = bounding_box_center(&bb(f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY));
        assert!(e.x.is_nan() && e.y.is_nan());
    }

    #[test]
    fn inscribed_is_inclusive_on_every_edge() {
        let outer = bb(0.0, 0.0, 10.0, 10.0);
        assert!(inscribed(&outer, &bb(0.0, 0.0, 10.0, 10.0)));
        assert!(inscribed(&outer, &bb(1.0, 2.0, 3.0, 4.0)));
        assert!(!inscribed(&outer, &bb(-1.0, 0.0, 10.0, 10.0)));
        assert!(!inscribed(&outer, &bb(0.0, 0.0, 10.0, 11.0)));
        // NaN compares false (an empty inner box is never inscribed).
        assert!(!inscribed(&outer, &bb(f64::NAN, 0.0, 1.0, 1.0)));
    }

    #[test]
    fn find_closest_by_keeps_first_strict_minimum() {
        let v = [5.0, 2.0, 7.0, 2.0, f64::NAN];
        assert_eq!(find_closest_by(&v, |x| *x, None::<fn(&f64) -> bool>), Some(1));
        assert_eq!(find_closest_by(&v, |x| *x, Some(|x: &f64| *x > 2.0)), Some(0));
        // Only NaN / +Infinity scores: `currentScore < Infinity` never holds → null.
        assert_eq!(find_closest_by(&[f64::NAN, f64::INFINITY], |x| *x, None::<fn(&f64) -> bool>), None);
        assert_eq!(find_closest_by(&[] as &[f64], |x| *x, None::<fn(&f64) -> bool>), None);
    }
}
