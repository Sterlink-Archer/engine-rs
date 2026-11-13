//! `ParabolaUniversalPathFinder` (`pathfinding/PathFinder.Parabola.ts`). Owner: **P4b**.
//! Snapshot: `pathFinder { options?, lastTo, curve { points, currentIndex,
//! accumulatedDistanceScaled }? }` (Nuke, MIRV).

use crate::StateClone;
use crate::ctx::TsExpect;
use crate::ids::TileRef;
use crate::map::GameMap;
use crate::path::MapRead;
use crate::path::line::{DistanceBasedBezierCurve, Point};
use crate::path::types::PathResult;
use of_det::js;

/// `PARABOLA_MIN_HEIGHT = 50` (`:13`).
const PARABOLA_MIN_HEIGHT: f64 = 50.0;

/// `ParabolaOptions` (`:6-11`).
#[derive(StateClone, Debug, Default, PartialEq)]
pub struct ParabolaOptions {
    pub increment: Option<f64>,
    pub distance_based_height: Option<bool>,
    pub direction_up: Option<bool>,
    pub ignore_map_bounds: Option<bool>,
}

/// `getParabolaControlPoints(gameMap, from, to, options?)` (`:15-54`).
pub fn get_parabola_control_points(
    map: &GameMap,
    from: TileRef,
    to: TileRef,
    options: Option<&ParabolaOptions>,
) -> [Point; 4] {
    get_parabola_control_points_m(map, from, to, options)
}

/// [`get_parabola_control_points`] over any [`MapRead`].
pub fn get_parabola_control_points_m<M: MapRead>(
    map: &M,
    from: TileRef,
    to: TileRef,
    options: Option<&ParabolaOptions>,
) -> [Point; 4] {
    let distance_based_height = options.and_then(|o| o.distance_based_height).unwrap_or(true);
    let direction_up = options.and_then(|o| o.direction_up).unwrap_or(true);
    let ignore_map_bounds = options.and_then(|o| o.ignore_map_bounds).unwrap_or(false);

    let p0 = Point { x: f64::from(map.x(from)), y: f64::from(map.y(from)) };
    let p3 = Point { x: f64::from(map.x(to)), y: f64::from(map.y(to)) };
    let dx = p3.x - p0.x;
    let dy = p3.y - p0.y;
    let distance = js::sqrt(dx * dx + dy * dy);
    let max_height = if distance_based_height { js::max(distance / 3.0, PARABOLA_MIN_HEIGHT) } else { 0.0 };
    let height_mult = if direction_up { -1.0 } else { 1.0 };
    let map_height = f64::from(map.height());

    // TS: :41-42 (left-to-right: `(p0.y + dy / 4) + heightMult * maxHeight`)
    let p1y = p0.y + dy / 4.0 + height_mult * max_height;
    let p2y = p0.y + (dy * 3.0) / 4.0 + height_mult * max_height;

    let p1 = Point {
        x: p0.x + dx / 4.0,
        y: if ignore_map_bounds { p1y } else { of_det::util::within(p1y, 0.0, map_height - 1.0) },
    };
    let p2 = Point {
        x: p0.x + (dx * 3.0) / 4.0,
        y: if ignore_map_bounds { p2y } else { of_det::util::within(p2y, 0.0, map_height - 1.0) },
    };
    [p0, p1, p2, p3]
}

/// `ParabolaUniversalPathFinder` data.
#[derive(StateClone, Debug, Default, PartialEq)]
pub struct ParabolaPathFinder {
    pub options: Option<ParabolaOptions>,
    pub last_to: Option<TileRef>,
    pub curve: Option<DistanceBasedBezierCurve>,
}

impl ParabolaPathFinder {
    /// `UniversalPathFinding.Parabola(gameMap, options?)` (`PathFinder.ts:29-34`).
    pub fn new(options: Option<ParabolaOptions>) -> Self {
        ParabolaPathFinder { options, last_to: None, curve: None }
    }
    /// `createCurve(from, to)` (`:65-74`).
    fn create_curve<M: MapRead>(&self, map: &M, from: TileRef, to: TileRef) -> DistanceBasedBezierCurve {
        let increment = self.options.as_ref().and_then(|o| o.increment).unwrap_or(3.0);
        let [p0, p1, p2, p3] = get_parabola_control_points_m(map, from, to, self.options.as_ref());
        DistanceBasedBezierCurve::new(p0, p1, p2, p3, increment)
    }
    /// `findPath(from, to)` (`:76-86`) for a scalar `from` (TS throws for an array). Never
    /// `None`; `map.ref` throws `Invalid coordinates` for a point off the map (possible with
    /// `ignoreMapBounds`).
    pub fn find_path(&mut self, map: &GameMap, from: TileRef, to: TileRef) -> Option<Vec<TileRef>> {
        self.find_path_m(map, from, to)
    }
    /// [`ParabolaPathFinder::find_path`] over any [`MapRead`].
    pub fn find_path_m<M: MapRead>(&mut self, map: &M, from: TileRef, to: TileRef) -> Option<Vec<TileRef>> {
        let curve = self.create_curve(map, from, to);
        Some(curve.get_all_points().iter().map(|p| map.ref_(js::floor(p.x), js::floor(p.y))).collect())
    }
    /// `next(from, to, speed?)` (`:88-103`).
    pub fn next(&mut self, map: &GameMap, from: TileRef, to: TileRef, speed: Option<f64>) -> PathResult<TileRef> {
        self.next_m(map, from, to, speed)
    }
    /// [`ParabolaPathFinder::next`] over any [`MapRead`].
    pub fn next_m<M: MapRead>(
        &mut self,
        map: &M,
        from: TileRef,
        to: TileRef,
        speed: Option<f64>,
    ) -> PathResult<TileRef> {
        if self.last_to != Some(to) {
            self.curve = Some(self.create_curve(map, from, to));
            self.last_to = Some(to);
        }
        // `this.curve!.increment(..)`: a restored `{lastTo, curve: null}` is a TypeError in TS.
        let next_point =
            self.curve.as_mut().ts_expect("PathFinder.Parabola.ts:94 this.curve!").increment(speed.unwrap_or(1.0));
        let Some(p) = next_point else {
            return PathResult::Complete(to);
        };
        PathResult::Next(map.ref_(js::floor(p.x), js::floor(p.y)))
    }
    /// `invalidate()` (`:105-108`).
    pub fn invalidate(&mut self) {
        self.curve = None;
        self.last_to = None;
    }
    /// `currentIndex()` (`:110-113`).
    pub fn current_index(&self) -> i32 {
        self.curve.as_ref().map_or(0, |c| c.get_current_index())
    }
}

/// CS: `ParabolaSchema` (`snapshot/PathfinderSnapshots.ts:165-180`, `getState()` at
/// `PathFinder.Parabola.ts:115-125`). `options` is `{...this.options}` or `null`; an unset
/// option key is absent (`.optional()`), never `null`. `curve` is
/// `DistanceBasedBezierCurve.getState()` (`Line.ts:89-99`): the 4 control points, `currentIndex`
/// and `accumulatedDistanceScaled`. Written at merge1 (request P8 #2) on behalf of P4b.
impl crate::canon::Canon for ParabolaPathFinder {
    fn canon(&self, _cx: &mut crate::canon::CanonCx<'_>) -> crate::canon::Cs {
        use crate::canon::Cs;
        let opt_num = |v: Option<f64>| v.map_or(Cs::Absent, Cs::Num);
        let opt_bool = |v: Option<bool>| v.map_or(Cs::Absent, Cs::Bool);
        let options = self.options.as_ref().map_or(Cs::Null, |o| {
            Cs::Obj(vec![
                ("increment".into(), opt_num(o.increment)),
                ("distanceBasedHeight".into(), opt_bool(o.distance_based_height)),
                ("directionUp".into(), opt_bool(o.direction_up)),
                ("ignoreMapBounds".into(), opt_bool(o.ignore_map_bounds)),
            ])
        });
        let curve = self.curve.as_ref().map_or(Cs::Null, |c| {
            Cs::Obj(vec![
                (
                    "points".into(),
                    Cs::Arr(
                        c.points
                            .iter()
                            .map(|p| Cs::Obj(vec![("x".into(), Cs::Num(p.x)), ("y".into(), Cs::Num(p.y))]))
                            .collect(),
                    ),
                ),
                ("currentIndex".into(), Cs::Int(i64::from(c.current_index))),
                ("accumulatedDistanceScaled".into(), Cs::Int(i64::from(c.accumulated_distance_scaled))),
            ])
        });
        Cs::Obj(vec![
            ("options".into(), options),
            ("lastTo".into(), self.last_to.map_or(Cs::Null, |t| Cs::Int(i64::from(t)))),
            ("curve".into(), curve),
        ])
    }
}

#[cfg(test)]
mod tests {
    //! T0: port of `tests/core/pathfinding/UniversalPathFinding.Parabola.test.ts` (exact
    //! path lengths included: they pin `Line.ts`'s fixed-point subdivision).
    use super::*;
    use crate::path::testkit::{map_from_terrain, r};
    use crate::path::types::PathStatus;

    fn large_map() -> GameMap {
        map_from_terrain(100, 100, vec![0x20; 10_000])
    }
    fn opts(f: impl FnOnce(&mut ParabolaOptions)) -> Option<ParabolaOptions> {
        let mut o = ParabolaOptions::default();
        f(&mut o);
        Some(o)
    }

    #[test]
    fn find_path_lengths() {
        let map = large_map();
        let mut f = ParabolaPathFinder::new(None);
        let (from, to) = (r(&map, 10, 50), r(&map, 90, 50));
        let p = f.find_path_m(&map, from, to).expect("path");
        assert_eq!(p.len(), 40);
        assert_eq!((p[0], *p.last().unwrap()), (from, to));
        assert_eq!(f.find_path_m(&map, r(&map, 50, 50), r(&map, 50, 50)).expect("path").len(), 26);
        let (from, to) = (r(&map, 0, 50), r(&map, 99, 50));
        let p = f.find_path_m(&map, from, to).expect("path");
        assert_eq!(p.len(), 45);
        assert_eq!((p[0], *p.last().unwrap()), (from, to));
    }

    #[test]
    fn next_steps_and_speed() {
        let map = large_map();
        let (from, to) = (r(&map, 10, 50), r(&map, 90, 50));
        let mut f1 = ParabolaPathFinder::new(None);
        let mut f2 = ParabolaPathFinder::new(None);
        let r1 = f1.next_m(&map, from, to, Some(1.0));
        let r2 = f2.next_m(&map, from, to, Some(5.0));
        assert_eq!((r1.status(), r2.status()), (PathStatus::Next, PathStatus::Next));
        let (n1, n2) = (*r1.node().unwrap(), *r2.node().unwrap());
        assert!(MapRead::manhattan_dist(&map, from, n2) > MapRead::manhattan_dist(&map, from, n1));
        assert!(f2.current_index() > f1.current_index());
    }

    #[test]
    fn options_change_the_arc() {
        let map = large_map();
        let (from, to) = (r(&map, 10, 50), r(&map, 90, 50));
        let p1 = ParabolaPathFinder::new(opts(|o| o.increment = Some(1.0))).find_path_m(&map, from, to).unwrap();
        let p2 = ParabolaPathFinder::new(opts(|o| o.increment = Some(10.0))).find_path_m(&map, from, to).unwrap();
        assert!(p1.len() > p2.len());

        let dev = |p: &[TileRef]| p.iter().map(|&t| MapRead::y(&map, t).abs_diff(50)).max().unwrap();
        let h1 = ParabolaPathFinder::new(opts(|o| o.distance_based_height = Some(true)))
            .find_path_m(&map, from, to)
            .unwrap();
        let h2 = ParabolaPathFinder::new(opts(|o| o.distance_based_height = Some(false)))
            .find_path_m(&map, from, to)
            .unwrap();
        assert!(dev(&h1) > dev(&h2));

        let up = ParabolaPathFinder::new(opts(|o| o.direction_up = Some(true))).find_path_m(&map, from, to).unwrap();
        let down = ParabolaPathFinder::new(opts(|o| o.direction_up = Some(false))).find_path_m(&map, from, to).unwrap();
        let mid = up.len() / 2;
        assert!(MapRead::y(&map, up[mid]) < 50);
        assert!(MapRead::y(&map, down[mid]) > 50);
    }

    #[test]
    fn current_index_is_monotone() {
        let map = large_map();
        let mut f = ParabolaPathFinder::new(None);
        assert_eq!(f.current_index(), 0);
        let (mut cur, to) = (r(&map, 10, 50), r(&map, 90, 50));
        let mut prev = 0;
        for _ in 0..50 {
            let res = f.next_m(&map, cur, to, None);
            assert_eq!(res.status(), PathStatus::Next);
            assert!(f.current_index() >= prev);
            prev = f.current_index();
            cur = *res.node().unwrap();
        }
    }

    #[test]
    fn short_distances() {
        let map = large_map();
        let mut f = ParabolaPathFinder::new(opts(|o| o.distance_based_height = Some(true)));
        let (from, to) = (r(&map, 50, 50), r(&map, 80, 50));
        let p = f.find_path_m(&map, from, to).unwrap();
        assert_eq!(p.len(), 29);
        assert_eq!((p[0], *p.last().unwrap()), (from, to));
        let mut g = ParabolaPathFinder::new(None);
        let (from, to) = (r(&map, 50, 50), r(&map, 51, 50));
        let p = g.find_path_m(&map, from, to).unwrap();
        assert_eq!(p.len(), 26);
        assert_eq!((p[0], *p.last().unwrap()), (from, to));
        let (from, to) = (r(&map, 50, 50), r(&map, 55, 50));
        let p = f.find_path_m(&map, from, to).unwrap();
        assert_eq!((p[0], *p.last().unwrap()), (from, to));
    }

    #[test]
    fn map_boundary_clipping() {
        let map = large_map();
        let mut up = ParabolaPathFinder::new(opts(|o| {
            o.direction_up = Some(true);
            o.distance_based_height = Some(true);
        }));
        // `map.ref` would throw for y < 0, so a full path is the assertion.
        let p = up.find_path_m(&map, r(&map, 10, 5), r(&map, 90, 5)).unwrap();
        assert!(p.iter().all(|&t| MapRead::y(&map, t) < 100));
        let mut down = ParabolaPathFinder::new(opts(|o| {
            o.direction_up = Some(false);
            o.distance_based_height = Some(true);
        }));
        let p = down.find_path_m(&map, r(&map, 10, 95), r(&map, 90, 95)).unwrap();
        assert!(p.iter().all(|&t| MapRead::y(&map, t) < 100));
    }

    /// `ignoreMapBounds`: control points leave the map and `map.ref` throws, as in TS.
    #[test]
    #[allow(clippy::disallowed_methods)] // asserting a TS throw
    fn ignore_map_bounds_can_throw_invalid_coordinates() {
        let map = large_map();
        let mut f = ParabolaPathFinder::new(opts(|o| o.ignore_map_bounds = Some(true)));
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            f.find_path_m(&map, r(&map, 10, 5), r(&map, 90, 5));
        }));
        let p = res.expect_err("off-map point");
        let t = of_det::throw::as_ts_throw(&*p).expect("TsThrow");
        assert!(t.msg.starts_with("Invalid coordinates: "), "{}", t.msg);
    }

    /// A restored `{ lastTo, curve: null }` meets `this.curve!` → TypeError.
    #[test]
    #[allow(clippy::disallowed_methods)] // asserting a TS throw
    fn restored_null_curve_with_last_to_is_a_type_error() {
        let map = large_map();
        let to = r(&map, 90, 50);
        let mut f = ParabolaPathFinder { options: None, last_to: Some(to), curve: None };
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f.next_m(&map, r(&map, 10, 50), to, None)));
        let p = res.expect_err("TypeError");
        assert_eq!(of_det::throw::as_ts_throw(&*p).expect("TsThrow").kind, of_det::throw::TsErrorKind::TypeError);
    }
}
