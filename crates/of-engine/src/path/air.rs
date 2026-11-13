//! `AirPathFinder` (`pathfinding/PathFinder.Air.ts`). Owner: **P4b**. A fresh
//! `PseudoRandom(seed)` per query (`:27`); the seed is `game.ticks()` at construction.

use crate::ids::TileRef;
use crate::map::GameMap;
use crate::path::MapRead;
use of_det::Prng;
use of_det::js;

/// `findPath(from, to)` (`:22-39`) for a scalar `from` (an array throws
/// `"AirPathFinder does not support multiple start points"`, raised by the caller that holds
/// the `T | T[]` value: `path::pathfinder::AirStepper`). `game.x/y/ref` are the main map's.
pub fn find_path(map: &GameMap, seed: i32, from: TileRef, to: TileRef) -> Vec<TileRef> {
    find_path_m(map, seed, from, to)
}

/// [`find_path`] over any [`MapRead`].
pub fn find_path_m<M: MapRead>(map: &M, seed: i32, from: TileRef, to: TileRef) -> Vec<TileRef> {
    let mut random = Prng::new(f64::from(seed));
    let mut path: Vec<TileRef> = vec![from];
    let mut current = from;
    while current != to {
        let next = compute_next(map, current, to, &mut random);
        if next == current {
            break; // Prevent infinite loop if something breaks
        }
        current = next;
        path.push(current);
    }
    path
}

/// `computeNext(from, to, random)` (`:41-74`).
#[allow(clippy::if_same_then_else)] // TS branch order: `random.chance` is drawn only off both axes
fn compute_next<M: MapRead>(map: &M, from: TileRef, to: TileRef, random: &mut Prng) -> TileRef {
    let x = f64::from(map.x(from));
    let y = f64::from(map.y(from));
    let dst_x = f64::from(map.x(to));
    let dst_y = f64::from(map.y(to));
    if x == dst_x && y == dst_y {
        return to;
    }
    let mut next_x = x;
    let mut next_y = y;
    let ratio = js::floor(1.0 + js::abs(dst_y - y) / (js::abs(dst_x - x) + 1.0));
    if x == dst_x {
        next_y += if y < dst_y { 1.0 } else { -1.0 };
    } else if y == dst_y {
        next_x += if x < dst_x { 1.0 } else { -1.0 };
    } else if random.chance(ratio) {
        next_x += if x < dst_x { 1.0 } else { -1.0 };
    } else {
        next_y += if y < dst_y { 1.0 } else { -1.0 };
    }
    map.ref_(next_x, next_y)
}

#[cfg(test)]
mod tests {
    //! T0: port of `tests/core/pathfinding/PathFinding.Air.test.ts` (terrain is ignored, so
    //! a 16×16 map stands in for `ocean_and_land`; the stepper/array cases are in
    //! `path::pathfinder`).
    use super::*;
    use crate::path::testkit::{map_from_terrain, r};

    fn map16() -> GameMap {
        map_from_terrain(16, 16, vec![0x20; 256])
    }
    fn moves(map: &GameMap, path: &[TileRef]) -> (u32, u32) {
        let (mut xm, mut ym) = (0, 0);
        for w in path.windows(2) {
            if MapRead::x(map, w[1]) != MapRead::x(map, w[0]) {
                xm += 1;
            }
            if MapRead::y(map, w[1]) != MapRead::y(map, w[0]) {
                ym += 1;
            }
        }
        (xm, ym)
    }

    #[test]
    fn path_between_any_two_points_is_adjacent_steps() {
        let map = map16();
        let (from, to) = (r(&map, 2, 2), r(&map, 14, 14));
        let path = find_path_m(&map, 0, from, to);
        assert_eq!(path.len(), 25);
        assert_eq!((path[0], *path.last().unwrap()), (from, to));
        for w in path.windows(2) {
            assert_eq!(MapRead::manhattan_dist(&map, w[0], w[1]), 1);
        }
        assert_eq!(find_path_m(&map, 0, r(&map, 8, 8), r(&map, 8, 8)), vec![r(&map, 8, 8)]);
        let p = find_path_m(&map, 0, r(&map, 5, 5), r(&map, 10, 12));
        assert_eq!(*p.last().unwrap(), r(&map, 10, 12));
    }

    #[test]
    fn path_shapes() {
        let map = map16();
        assert_eq!(moves(&map, &find_path_m(&map, 0, r(&map, 0, 0), r(&map, 10, 10))), (10, 10));
        assert_eq!(moves(&map, &find_path_m(&map, 0, r(&map, 0, 5), r(&map, 15, 5))), (15, 0));
        assert_eq!(moves(&map, &find_path_m(&map, 0, r(&map, 5, 0), r(&map, 5, 15))), (0, 15));
        assert_eq!(find_path_m(&map, 0, r(&map, 3, 3), r(&map, 4, 3)), vec![r(&map, 3, 3), r(&map, 4, 3)]);
    }

    /// The walk is a pure function of the seed (re-seeded per query, `:27`), and the PRNG
    /// draws exactly once per off-axis step.
    #[test]
    #[allow(clippy::if_same_then_else)] // mirrors computeNext's branch order
    fn seeded_walk_is_deterministic_and_draws_per_diagonal_step() {
        let map = map16();
        let (from, to) = (r(&map, 1, 1), r(&map, 13, 6));
        let a = find_path_m(&map, 42, from, to);
        assert_eq!(a, find_path_m(&map, 42, from, to));
        // Replay the walk with an explicit PRNG: one `chance(ratio)` per step off both axes.
        let mut rng = Prng::new(42.0);
        let (mut x, mut y) = (1.0f64, 1.0f64);
        let mut want = vec![from];
        while (x, y) != (13.0, 6.0) {
            let ratio = js::floor(1.0 + js::abs(6.0 - y) / (js::abs(13.0 - x) + 1.0));
            if x == 13.0 {
                y += 1.0;
            } else if y == 6.0 {
                x += 1.0;
            } else if rng.chance(ratio) {
                x += 1.0;
            } else {
                y += 1.0;
            }
            want.push(r(&map, x as u32, y as u32));
        }
        assert_eq!(a, want);
    }
}
