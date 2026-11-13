//! `SmoothingWaterTransformer` (`:1-347`). Owner: **P4b**. Its `localAStar` is
//! `sc.smoothing_local` (an `AStarWaterBounded` of area `LOCAL_ASTAR_MAX_AREA`).
//!
//! Integer coordinate math in `i64` (JS numbers, exact); `(t / w) | 0` on non-negative refs
//! is integer division. Terrain reads use `undefined & mask === 0` past the end (never hit:
//! every Bresenham tile lies in the endpoints' bounding box).
#![allow(clippy::collapsible_if)] // nested ifs kept as the TS writes them

use crate::ids::TileRef;
use crate::map::GameMap;
use crate::path::MapRead;
use crate::path::astar_water_bounded::{self, BoundedAStarConfig, SearchBounds};
use crate::path::types::Src;
use crate::scratch::path::AStarWaterBoundedScratch;

const ENDPOINT_REFINEMENT_TILES: i64 = 50; // TS: :9
/// `LOCAL_ASTAR_MAX_AREA = 100 * 100` (`:10`).
pub const LOCAL_ASTAR_MAX_AREA: u32 = 100 * 100;
const LOS_MIN_MAGNITUDE_PASS1: u8 = 2; // TS: :11
const LOS_MIN_MAGNITUDE_PASS2: u8 = 3; // TS: :12
const MAGNITUDE_MASK: u8 = 0x1f; // TS: :13

/// Smooth a minimap water path (`smooth`, `:46-67`).
pub fn smooth(sc: &mut AStarWaterBoundedScratch, mini: &GameMap, path: Vec<TileRef>) -> Vec<TileRef> {
    smooth_m(sc, mini, path)
}

/// `findPath(from, to)` (`:38-44`): the inner path, smoothed (`path ? smooth(path) : null`;
/// an empty array is truthy).
pub fn find_path<M: MapRead>(
    sc: &mut AStarWaterBoundedScratch,
    map: &M,
    from: Src<'_>,
    to: TileRef,
    inner: impl FnOnce(Src<'_>, TileRef) -> Option<Vec<TileRef>>,
) -> Option<Vec<TileRef>> {
    let path = inner(from, to);
    path.map(|p| smooth_m(sc, map, p))
}

/// [`smooth`] over any [`MapRead`].
pub fn smooth_m<M: MapRead>(sc: &mut AStarWaterBoundedScratch, map: &M, path: Vec<TileRef>) -> Vec<TileRef> {
    if path.len() <= 2 {
        return path;
    }
    let s = Smoother { map, map_width: i64::from(map.width()) };
    // Pass 1: LOS smoothing with binary search
    let smoothed = s.los_smooth(&path, LOS_MIN_MAGNITUDE_PASS1);
    // Pass 2: Local A* refinement on endpoints
    let smoothed = s.refine_endpoints(sc, smoothed);
    // Pass 3: LOS smoothing again, farther from the shore
    s.los_smooth(&smoothed, LOS_MIN_MAGNITUDE_PASS2)
}

struct Smoother<'m, M: MapRead> {
    map: &'m M,
    map_width: i64,
}

impl<M: MapRead> Smoother<'_, M> {
    /// `this.isTraversable(t)` = the default `(t) => map.isWater(t)` (`:30`).
    #[inline]
    fn is_traversable(&self, t: i64) -> bool {
        self.map.is_water(t as TileRef)
    }
    /// `this.terrain[t] & MAGNITUDE_MASK`.
    #[inline]
    fn magnitude(&self, t: i64) -> u8 {
        usize::try_from(t).ok().and_then(|i| self.map.terrain().get(i).copied()).unwrap_or(0) & MAGNITUDE_MASK
    }

    /// `losSmooth(path, minMagnitude)` (`:69-108`).
    fn los_smooth(&self, path: &[TileRef], min_magnitude: u8) -> Vec<TileRef> {
        let mut result: Vec<TileRef> = vec![path[0]];
        let mut current: usize = 0;
        while current < path.len() - 1 {
            let mut lo = current + 1;
            let mut hi = path.len() as i64 - 1;
            let mut farthest = lo;
            while lo as i64 <= hi {
                let mid = (lo + hi as usize) >> 1; // `(lo + hi) >>> 1`, non-negative
                if self.can_see(path[current], path[mid], min_magnitude) {
                    farthest = mid;
                    lo = mid + 1;
                } else {
                    hi = mid as i64 - 1;
                }
            }
            if farthest > current + 1 {
                if let Some(trace) = self.trace_path(path[current], path[farthest]) {
                    for &t in trace.iter().take(trace.len().saturating_sub(1)).skip(1) {
                        result.push(t);
                    }
                }
            }
            current = farthest;
            if current < path.len() - 1 {
                result.push(path[current]);
            }
        }
        result.push(path[path.len() - 1]);
        result
    }

    /// `refineEndpoints(path)` (`:110-153`).
    fn refine_endpoints(&self, sc: &mut AStarWaterBoundedScratch, path: Vec<TileRef>) -> Vec<TileRef> {
        if path.len() <= 2 {
            return path;
        }
        let refine_dist = ENDPOINT_REFINEMENT_TILES;
        let mut result = path;

        let start_end_idx = self.find_tile_at_distance(&result, 0, refine_dist, true);
        if start_end_idx > 1 {
            let start_segment = self.refine_segment(sc, result[0], result[start_end_idx]);
            if let Some(seg) = start_segment {
                if !seg.is_empty() {
                    // `[...startSegment.slice(0, -1), ...result.slice(startEndIdx)]`
                    let mut next: Vec<TileRef> = seg[..seg.len() - 1].to_vec();
                    next.extend_from_slice(&result[start_end_idx..]);
                    result = next;
                }
            }
        }

        let end_start_idx = self.find_tile_at_distance(&result, result.len() - 1, refine_dist, false);
        if (end_start_idx as i64) < result.len() as i64 - 2 {
            let end_segment = self.refine_segment(sc, result[result.len() - 1], result[end_start_idx]);
            if let Some(mut seg) = end_segment {
                if !seg.is_empty() {
                    seg.reverse();
                    // `[...result.slice(0, endStartIdx), ...endSegment]`
                    let mut next: Vec<TileRef> = result[..end_start_idx].to_vec();
                    next.extend_from_slice(&seg);
                    result = next;
                }
            }
        }
        result
    }

    /// `findTileAtDistance(path, startIdx, distance, forward)` (`:155-177`).
    fn find_tile_at_distance(&self, path: &[TileRef], start_idx: usize, distance: i64, forward: bool) -> usize {
        let mut cum_dist: i64 = 0;
        let mut idx = start_idx;
        if forward {
            while idx < path.len() - 1 && cum_dist < distance {
                cum_dist += self.manhattan_dist(path[idx], path[idx + 1]);
                idx += 1;
            }
        } else {
            while idx > 0 && cum_dist < distance {
                cum_dist += self.manhattan_dist(path[idx], path[idx - 1]);
                idx -= 1;
            }
        }
        idx
    }

    /// `refineSegment(from, to)` (`:179-195`).
    fn refine_segment(&self, sc: &mut AStarWaterBoundedScratch, from: TileRef, to: TileRef) -> Option<Vec<TileRef>> {
        let x0 = i64::from(self.map.x(from));
        let y0 = i64::from(self.map.y(from));
        let x1 = i64::from(self.map.x(to));
        let y1 = i64::from(self.map.y(to));
        let padding = 10;
        let w = i64::from(self.map.width());
        let h = i64::from(self.map.height());
        let bounds = SearchBounds {
            min_x: 0.max(x0.min(x1) - padding) as u32,
            max_x: (w - 1).min(x0.max(x1) + padding) as u32,
            min_y: 0.max(y0.min(y1) - padding) as u32,
            max_y: (h - 1).min(y0.max(y1) + padding) as u32,
        };
        astar_water_bounded::search_bounded_m(
            sc,
            self.map,
            LOCAL_ASTAR_MAX_AREA,
            BoundedAStarConfig::default(),
            &[from],
            to,
            bounds,
        )
    }

    /// `canSee(from, to, minMagnitude)` (`:197-267`).
    fn can_see(&self, from: TileRef, to: TileRef, min_magnitude: u8) -> bool {
        let w = self.map_width;
        let (from, to) = (i64::from(from), i64::from(to));
        let x0 = from % w;
        let y0 = from / w;
        let x1 = to % w;
        let y1 = to / w;
        let dx = (x1 - x0).abs();
        let dy = (y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx - dy;
        let mut x = x0;
        let mut y = y0;
        let max_tiles = 100_000;
        let mut iterations = 0;
        loop {
            // `if (iterations++ > maxTiles) return false`
            let it = iterations;
            iterations += 1;
            if it > max_tiles {
                return false;
            }
            let tile = y * w + x;
            if !self.is_traversable(tile) {
                return false;
            }
            if self.magnitude(tile) < min_magnitude {
                return false;
            }
            if x == x1 && y == y1 {
                return true;
            }
            let e2 = 2 * err;
            let should_move_x = e2 > -dy;
            let should_move_y = e2 < dx;
            if should_move_x && should_move_y {
                x += sx;
                err -= dy;
                let intermediate_tile = y * w + x;
                let int_mag = self.magnitude(intermediate_tile);
                if !self.is_traversable(intermediate_tile) || int_mag < min_magnitude {
                    x -= sx;
                    err += dy;
                    y += sy;
                    err += dx;
                    let alt_tile = y * w + x;
                    let alt_mag = self.magnitude(alt_tile);
                    if !self.is_traversable(alt_tile) || alt_mag < min_magnitude {
                        return false;
                    }
                    x += sx;
                    err -= dy;
                } else {
                    y += sy;
                    err += dx;
                }
            } else {
                if should_move_x {
                    x += sx;
                    err -= dy;
                }
                if should_move_y {
                    y += sy;
                    err += dx;
                }
            }
        }
    }

    /// `tracePath(from, to)` (`:269-338`).
    fn trace_path(&self, from: TileRef, to: TileRef) -> Option<Vec<TileRef>> {
        let w = self.map_width;
        let (from, to) = (i64::from(from), i64::from(to));
        let x0 = from % w;
        let y0 = from / w;
        let x1 = to % w;
        let y1 = to / w;
        let mut tiles: Vec<TileRef> = Vec::new();
        let dx = (x1 - x0).abs();
        let dy = (y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx - dy;
        let mut x = x0;
        let mut y = y0;
        let max_tiles = 100_000;
        let mut iterations = 0;
        loop {
            let it = iterations;
            iterations += 1;
            if it > max_tiles {
                return None;
            }
            let tile = y * w + x;
            if !self.is_traversable(tile) {
                return None;
            }
            tiles.push(tile as TileRef);
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * err;
            let should_move_x = e2 > -dy;
            let should_move_y = e2 < dx;
            if should_move_x && should_move_y {
                x += sx;
                err -= dy;
                let intermediate_tile = y * w + x;
                if !self.is_traversable(intermediate_tile) {
                    x -= sx;
                    err += dy;
                    y += sy;
                    err += dx;
                    let alt_tile = y * w + x;
                    if !self.is_traversable(alt_tile) {
                        return None;
                    }
                    tiles.push(alt_tile as TileRef);
                    x += sx;
                    err -= dy;
                } else {
                    tiles.push(intermediate_tile as TileRef);
                    y += sy;
                    err += dx;
                }
            } else {
                if should_move_x {
                    x += sx;
                    err -= dy;
                }
                if should_move_y {
                    y += sy;
                    err += dx;
                }
            }
        }
        Some(tiles)
    }

    /// `manhattanDist(a, b)` (`:340-346`).
    fn manhattan_dist(&self, a: TileRef, b: TileRef) -> i64 {
        let w = self.map_width;
        let (a, b) = (i64::from(a), i64::from(b));
        let ax = a % w;
        let ay = a / w;
        let bx = b % w;
        let by = b / w;
        (ax - bx).abs() + (ay - by).abs()
    }
}
