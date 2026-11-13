//! T0: ports of `tests/core/pathfinding/transformers/*.test.ts` (ComponentCheck,
//! MiniMap, ShoreCoercing) plus smoother checks. Owner: **P4b**.

use super::{component_check, minimap, shore, smoothing};
use crate::ids::TileRef;
use crate::map::GameMap;
use crate::path::testkit::{ISLAND, game_map, map_from_terrain, r};
use crate::path::types::Src;
use crate::scratch::path::AStarWaterBoundedScratch;

/// A recorded inner call: `(from, to)` with the scalar/array distinction kept.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Arg {
    One(u32),
    Many(Vec<u32>),
}
impl Arg {
    fn of(s: Src<'_>) -> Arg {
        match s {
            Src::One(t) => Arg::One(t),
            Src::Many(v) => Arg::Many(v.to_vec()),
        }
    }
}

/// `createMockPathFinder()`: records calls; returns `returnPath` if set, else `[start, to]`.
struct Mock {
    calls: Vec<(Arg, u32)>,
    return_path: Option<Option<Vec<u32>>>,
}
impl Mock {
    fn new() -> Self {
        Mock { calls: Vec::new(), return_path: None }
    }
    fn ret(p: Option<Vec<u32>>) -> Self {
        Mock { calls: Vec::new(), return_path: Some(p) }
    }
    fn find(&mut self, from: Src<'_>, to: u32) -> Option<Vec<u32>> {
        self.calls.push((Arg::of(from), to));
        if let Some(p) = &self.return_path {
            return p.clone();
        }
        let mut one = [0];
        Some(vec![from.as_slice(&mut one)[0], to])
    }
}

// ------------------------------------------------------------ ComponentCheckTransformer

fn even_odd(t: TileRef) -> u32 {
    t % 2
}

#[test]
fn component_check_delegates_same_component() {
    let mut m = Mock::new();
    let r = component_check::find_path(even_odd, Src::One(2), 4, |f, t| m.find(f, t));
    assert_eq!(r, Some(vec![2, 4]));
    assert_eq!(m.calls, [(Arg::One(2), 4)]);
}

#[test]
fn component_check_null_for_different_components() {
    let mut m = Mock::new();
    assert_eq!(component_check::find_path(even_odd, Src::One(2), 3, |f, t| m.find(f, t)), None);
    assert!(m.calls.is_empty());
}

#[test]
fn component_check_filters_and_unwraps() {
    let mut m = Mock::new();
    assert!(component_check::find_path(even_odd, Src::Many(&[1, 2, 3, 4]), 4, |f, t| m.find(f, t)).is_some());
    assert_eq!(m.calls, [(Arg::Many(vec![2, 4]), 4)]);
    let mut m = Mock::new();
    assert_eq!(component_check::find_path(even_odd, Src::Many(&[1, 3, 5]), 4, |f, t| m.find(f, t)), None);
    assert!(m.calls.is_empty());
    let mut m = Mock::new();
    assert!(component_check::find_path(even_odd, Src::Many(&[1, 2, 3]), 4, |f, t| m.find(f, t)).is_some());
    assert_eq!(m.calls, [(Arg::One(2), 4)], "single valid source unwrapped");
    let mut m = Mock::new();
    assert_eq!(component_check::find_path(even_odd, Src::One(4), 6, |f, t| m.find(f, t)), Some(vec![4, 6]));
    assert_eq!(m.calls[0].0, Arg::One(4));
}

#[test]
fn component_check_propagates_inner_and_edge_cases() {
    assert_eq!(component_check::find_path(even_odd, Src::One(2), 4, |_, _| None), None);
    assert_eq!(
        component_check::find_path(even_odd, Src::One(2), 4, |_, _| Some(vec![10, 20, 30, 40])),
        Some(vec![10, 20, 30, 40])
    );
    let mut m = Mock::new();
    assert_eq!(component_check::find_path(even_odd, Src::Many(&[]), 4, |f, t| m.find(f, t)), None);
    assert!(m.calls.is_empty());
    let tens = |t: TileRef| t / 10;
    assert!(component_check::find_path(tens, Src::One(15), 18, |f, t| Some(vec![f.to_vec()[0], t])).is_some());
    assert!(component_check::find_path(tens, Src::One(15), 25, |f, t| Some(vec![f.to_vec()[0], t])).is_none());
}

// ------------------------------------------------------------ MiniMapTransformer

/// `createTestMaps()`: 10×10 and 5×5, all water.
fn test_maps() -> (GameMap, GameMap) {
    (map_from_terrain(10, 10, vec![0x20; 100]), map_from_terrain(5, 5, vec![0x20; 25]))
}

#[test]
fn minimap_converts_coordinates() {
    let (map, mini) = test_maps();
    let (from, to) = (r(&map, 4, 6), r(&map, 8, 2));
    let (mini_from, mini_to) = (r(&mini, 2, 3), r(&mini, 4, 1));
    let mut m = Mock::ret(Some(vec![mini_from, mini_to]));
    minimap::find_path(&map, &mini, Src::One(from), to, |f, t| m.find(f, t));
    assert_eq!(m.calls, [(Arg::One(mini_from), mini_to)]);
}

#[test]
fn minimap_upscales_and_fixes_extremes() {
    let (map, mini) = test_maps();
    let (from, to) = (r(&map, 0, 0), r(&map, 8, 0));
    let mut m = Mock::ret(Some((0..5).map(|x| r(&mini, x, 0)).collect()));
    let res = minimap::find_path(&map, &mini, Src::One(from), to, |f, t| m.find(f, t)).expect("path");
    assert_eq!((res[0], *res.last().unwrap()), (from, to));
    // Interpolated: every main tile from x = 0 to 8 on row 0.
    assert_eq!(res, (0..=8).map(|x| r(&map, x, 0)).collect::<Vec<_>>());

    // Odd coordinates do not map exactly: the extremes are fixed to from/to.
    let (from, to) = (r(&map, 1, 1), r(&map, 9, 9));
    let mut m = Mock::ret(Some(vec![r(&mini, 0, 0), r(&mini, 4, 4)]));
    let res = minimap::find_path(&map, &mini, Src::One(from), to, |f, t| m.find(f, t)).expect("path");
    assert_eq!((res[0], *res.last().unwrap()), (from, to));
    // Diagonal (0,0)→(8,8): steps = 8; `from` (1,1) is on it at index 1 → slice(1); `to`
    // (9,9) is not → pushed.
    let mut want: Vec<u32> = (1..=8).map(|i| r(&map, i, i)).collect();
    want.push(to);
    assert_eq!(res, want);
}

#[test]
fn minimap_null_and_empty_inner() {
    let (map, mini) = test_maps();
    let (a, b) = (r(&map, 0, 0), r(&map, 8, 8));
    let mut m = Mock::ret(None);
    assert_eq!(minimap::find_path(&map, &mini, Src::One(a), b, |f, t| m.find(f, t)), None);
    let mut m = Mock::ret(Some(vec![]));
    assert_eq!(minimap::find_path(&map, &mini, Src::One(a), b, |f, t| m.find(f, t)), None);
}

#[test]
fn minimap_multiple_sources() {
    let (map, mini) = test_maps();
    let (from1, from2, to) = (r(&map, 0, 0), r(&map, 2, 0), r(&map, 8, 0));
    let mut m = Mock::ret(Some(vec![r(&mini, 0, 0), r(&mini, 4, 0)]));
    let res = minimap::find_path(&map, &mini, Src::Many(&[from1, from2]), to, |f, t| m.find(f, t)).expect("path");
    assert_eq!(m.calls.len(), 1);
    assert!(matches!(m.calls[0].0, Arg::Many(_)));
    // Closest source to the path start (0,0) is from1, which is already on the path.
    assert_eq!(res[0], from1);
}

#[test]
fn minimap_coordinate_mapping_floor_division() {
    let (map, mini) = test_maps();
    for (x, mx) in [(0, 0), (1, 0), (2, 1)] {
        let mut m = Mock::ret(Some(vec![r(&mini, mx, mx)]));
        minimap::find_path(&map, &mini, Src::One(r(&map, x, x)), r(&map, x, x), |f, t| m.find(f, t));
        assert_eq!(m.calls, [(Arg::One(r(&mini, mx, mx)), r(&mini, mx, mx))]);
    }
}

/// `fixExtremes` slices when `src`/`dst` are inside the path (`:99-123`).
#[test]
fn minimap_fix_extremes_slices() {
    assert_eq!(minimap::fix_extremes(vec![1, 2, 3, 4, 5], 4, Some(2)), vec![2, 3, 4]);
    assert_eq!(minimap::fix_extremes(vec![1, 2, 3], 9, Some(8)), vec![8, 1, 2, 3, 9]);
    assert_eq!(minimap::fix_extremes(vec![1, 2, 3], 3, None), vec![1, 2, 3]);
}

/// Non-adjacent minimap nodes (golden backlog t155): `Math.round` of `x.5` rounds up.
#[test]
fn minimap_upscale_rounds_half_up() {
    let map = map_from_terrain(20, 20, vec![0x20; 400]);
    let mini = map_from_terrain(10, 10, vec![0x20; 100]);
    // mini (0,0) → (3,1): main (0,0) → (6,2), steps = 6, y offsets 2*k/6: 1/3, 2/3, 1, 4/3, 5/3.
    let up = minimap::upscale_path_m(&map, &mini, &[r(&mini, 0, 0), r(&mini, 3, 1)], 2.0);
    let ys: Vec<u32> = up.iter().map(|&t| t / 20).collect();
    assert_eq!(ys, [0, 0, 1, 1, 1, 2, 2]);
    // mini (0,0) → (1,1) then (1,1) → (3,0): (2,2)→(6,0), steps 4, y 2 - 2k/4 = 1.5 at k=1 → round(1.5) = 2.
    let up = minimap::upscale_path_m(&map, &mini, &[r(&mini, 1, 1), r(&mini, 3, 0)], 2.0);
    let pts: Vec<(u32, u32)> = up.iter().map(|&t| (t % 20, t / 20)).collect();
    assert_eq!(pts, [(2, 2), (3, 2), (4, 1), (5, 1), (6, 0)]);
}

// ------------------------------------------------------------ ShoreCoercingTransformer

#[test]
fn shore_passes_water_unchanged() {
    let map = game_map(5, 5, ISLAND);
    let (w1, w2) = (r(&map, 0, 0), r(&map, 4, 0));
    let mut m = Mock::ret(Some(vec![w1, w2]));
    assert_eq!(shore::find_path(&map, Src::One(w1), w2, |f, t| m.find(f, t)), Some(vec![w1, w2]));
    assert_eq!(m.calls, [(Arg::One(w1), w2)]);
}

#[test]
fn shore_coerces_start_and_destination() {
    let map = game_map(5, 5, ISLAND);
    let (shore1, w_n1) = (r(&map, 1, 1), r(&map, 1, 0));
    let (shore2, w_n2) = (r(&map, 3, 3), r(&map, 3, 4));
    let water = r(&map, 4, 4);
    let mut m = Mock::new();
    let res = shore::find_path(&map, Src::One(shore1), water, |f, t| m.find(f, t)).expect("path");
    assert_eq!((res[0], res[1]), (shore1, w_n1));
    let mut m = Mock::new();
    let res = shore::find_path(&map, Src::One(r(&map, 0, 0)), shore1, |f, t| m.find(f, t)).expect("path");
    assert_eq!(res[0], r(&map, 0, 0));
    assert_eq!((res[res.len() - 2], res[res.len() - 1]), (w_n1, shore1));
    let mut m = Mock::new();
    let res = shore::find_path(&map, Src::One(shore1), shore2, |f, t| m.find(f, t)).expect("path");
    assert_eq!((res[0], res[1]), (shore1, w_n1));
    assert_eq!((res[res.len() - 2], res[res.len() - 1]), (w_n2, shore2));
}

#[test]
fn shore_null_cases() {
    let map = game_map(5, 5, ISLAND);
    let (land, water) = (r(&map, 2, 2), r(&map, 0, 0));
    let mut m = Mock::new();
    assert_eq!(shore::find_path(&map, Src::One(land), water, |f, t| m.find(f, t)), None);
    assert!(m.calls.is_empty());
    let mut m = Mock::new();
    assert_eq!(shore::find_path(&map, Src::One(water), land, |f, t| m.find(f, t)), None);
    assert!(m.calls.is_empty());
    let mut m = Mock::ret(None);
    assert_eq!(shore::find_path(&map, Src::One(water), r(&map, 4, 4), |f, t| m.find(f, t)), None);
    let mut m = Mock::ret(Some(vec![]));
    assert_eq!(shore::find_path(&map, Src::One(water), r(&map, 4, 4), |f, t| m.find(f, t)), None);
    let mut m = Mock::new();
    assert_eq!(shore::find_path(&map, Src::Many(&[land]), water, |f, t| m.find(f, t)), None);
    assert!(m.calls.is_empty());
}

#[test]
fn shore_multiple_sources_filters_invalid() {
    let map = game_map(5, 5, ISLAND);
    let (water_src, shore_src, land_src, dest) = (r(&map, 0, 0), r(&map, 1, 1), r(&map, 2, 2), r(&map, 4, 4));
    let mut m = Mock::ret(Some(vec![water_src, dest]));
    assert!(shore::find_path(&map, Src::Many(&[water_src, shore_src, land_src]), dest, |f, t| m.find(f, t)).is_some());
    assert_eq!(m.calls.len(), 1);
    assert_eq!(m.calls[0].0, Arg::Many(vec![water_src, r(&map, 1, 0)]));
}

#[test]
fn shore_determinism_and_corner() {
    let map = game_map(
        5,
        5,
        "LLWWW
         LLWWW
         LLWLL
         WWWLL
         WWWLL",
    );
    let (s, expected) = (r(&map, 1, 2), r(&map, 1, 3));
    for _ in 0..2 {
        let mut m = Mock::new();
        shore::find_path(&map, Src::One(s), r(&map, 2, 4), |f, t| m.find(f, t));
        assert_eq!(m.calls[0].0, Arg::One(expected));
    }
    let map = game_map(5, 5, ISLAND);
    let (corner, wn, dest) = (r(&map, 1, 1), r(&map, 1, 0), r(&map, 4, 4));
    let mut m = Mock::ret(Some(vec![wn, dest]));
    assert_eq!(shore::find_path(&map, Src::One(corner), dest, |f, t| m.find(f, t)), Some(vec![corner, wn, dest]));
}

/// "Last write wins": a raw water source drops an earlier shore restoration of the same
/// water tile (`:28-37`).
#[test]
fn shore_raw_water_source_overrides_earlier_shore() {
    let map = game_map(5, 5, ISLAND);
    let (shore_src, its_water, dest) = (r(&map, 1, 1), r(&map, 1, 0), r(&map, 4, 4));
    let mut m = Mock::ret(Some(vec![its_water, dest]));
    let res = shore::find_path(&map, Src::Many(&[shore_src, its_water]), dest, |f, t| m.find(f, t));
    assert_eq!(res, Some(vec![its_water, dest]), "no shore tile prepended");
    let mut m = Mock::ret(Some(vec![its_water, dest]));
    let res = shore::find_path(&map, Src::Many(&[its_water, shore_src]), dest, |f, t| m.find(f, t));
    assert_eq!(res, Some(vec![shore_src, its_water, dest]), "the later shore source wins");
}

// ------------------------------------------------------------ SmoothingWaterTransformer

/// Paths of length <= 2 are returned as they are; magnitude-0 water fails every LOS check,
/// so pass 1/3 keep the path and the endpoint refinement re-plans with the bounded A*.
#[test]
fn smoothing_short_and_shallow() {
    let map = map_from_terrain(12, 1, vec![0x20; 12]);
    let mut sc = AStarWaterBoundedScratch::default();
    assert_eq!(smoothing::smooth_m(&mut sc, &map, vec![0, 1]), vec![0, 1]);
    let path: Vec<u32> = (0..12).collect();
    assert_eq!(smoothing::smooth_m(&mut sc, &map, path.clone()), path);
}

/// Deep water: line of sight collapses a zig-zag into the straight Bresenham line.
#[test]
fn smoothing_deep_water_straightens() {
    let mut map = map_from_terrain(8, 8, vec![0x20; 64]);
    crate::path::testkit::set_water_magnitude(&mut map, 5);
    let mut sc = AStarWaterBoundedScratch::default();
    // An L-shaped path (0,0) → (7,0) → (7,7).
    let mut path: Vec<u32> = (0..8).collect();
    path.extend((1..8).map(|y| y * 8 + 7));
    let out = smoothing::smooth_m(&mut sc, &map, path);
    assert_eq!((out[0], *out.last().unwrap()), (0, 63));
    // Every consecutive pair is 4-adjacent (tracePath keeps the steps orthogonal).
    for w in out.windows(2) {
        let (a, b) = (w[0] as i64, w[1] as i64);
        assert_eq!((a % 8 - b % 8).abs() + (a / 8 - b / 8).abs(), 1, "{out:?}");
    }
    assert!(out.len() <= 15);
}
