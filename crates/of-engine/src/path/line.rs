//! `DistanceBasedBezierCurve` (`engine/src/utilities/Line.ts`). Owner: **P4b**.
//! `Math.round` ×20 → `of_det::js::round`; `(p + 128) >> 8` → `of_det::js::sar` (PG §14.1/14.4).
//!
//! All arithmetic is JS `number` (f64; every value here is an integer far below 2^53), and
//! every `>>` is `ToInt32` then an arithmetic shift ([`of_det::js::sar`]). The progress
//! fields are `i32` (frozen at S0); the f64 results are converted with [`exact_i32`], which
//! treats a non-integral or out-of-range value as a Rust bug (TS would store a non-integer
//! there only for a non-finite speed, which no caller passes).

use crate::StateClone;
use of_det::js;

/// `Point { x, y }`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}
crate::plain_copy!(Point);

/// `SUB_SCALE = 256` (`:7`).
const SUB_SCALE: f64 = 256.0;

/// An integral f64 that TS keeps in a number field mirrored by an `i32` here.
#[inline]
fn exact_i32(v: f64) -> i32 {
    let i = v as i32;
    assert!(f64::from(i) == v, "Bezier progress value {v} is not an exact i32 (non-finite speed?)");
    i
}

/// `(v + 128) >> 8` as a number.
#[inline]
fn round_px(v: f64) -> f64 {
    f64::from(js::sar(v + 128.0, 8.0))
}

/// `(a + b) >> 1` as a number.
#[inline]
fn mid(a: f64, b: f64) -> f64 {
    f64::from(js::sar(a + b, 1.0))
}

/// The `st` object of `sharedSubdivide` (typed `:180-186`, built `:34`, `:128-134`); `step` is `{stepThreshold,
/// cachedPoints}` (both set by `computeAllPoints`, both absent for `getLength`).
struct SubdivState<'p> {
    last_x: f64,
    last_y: f64,
    accum_dist: f64,
    step: Option<(f64, &'p mut Vec<Point>)>,
}

/// `DistanceBasedBezierCurve` data; snapshot `curve { points, currentIndex,
/// accumulatedDistanceScaled }` (the cached points are recomputed).
#[derive(StateClone, Debug, Default, PartialEq)]
pub struct DistanceBasedBezierCurve {
    pub points: [Point; 4],
    /// non-snapshot: the constructor's `distanceIncrement` (`Line.ts:19`, not a TS field), kept
    /// so `cached_points` can be recomputed on restore.
    pub increment: f64,
    /// non-snapshot: `cachedPoints`.
    pub cached_points: Vec<Point>,
    pub current_index: i32,
    /// non-snapshot: `pixelSpacingScaled`.
    pub pixel_spacing_scaled: i32,
    pub accumulated_distance_scaled: i32,
}

impl DistanceBasedBezierCurve {
    /// `new DistanceBasedBezierCurve(p0, p1, p2, p3, increment)` (`:14-25`).
    pub fn new(p0: Point, p1: Point, p2: Point, p3: Point, increment: f64) -> Self {
        let mut c = DistanceBasedBezierCurve {
            points: [p0, p1, p2, p3],
            increment,
            cached_points: Vec::new(),
            current_index: 0,
            pixel_spacing_scaled: 1,
            accumulated_distance_scaled: 0,
        };
        c.compute_all_points(increment);
        c
    }

    /// `static getLength(p0..p3)` (`:27-54`).
    pub fn get_length(p0: Point, p1: Point, p2: Point, p3: Point) -> f64 {
        let scale = 256.0;
        let p0x = js::round(p0.x) * scale;
        let p0y = js::round(p0.y) * scale;
        let p3x = js::round(p3.x) * scale;
        let p3y = js::round(p3.y) * scale;
        let mut st = SubdivState { last_x: p0x, last_y: p0y, accum_dist: 0.0, step: None };
        Self::shared_subdivide(
            p0x,
            p0y,
            js::round(p1.x) * scale,
            js::round(p1.y) * scale,
            js::round(p2.x) * scale,
            js::round(p2.y) * scale,
            p3x,
            p3y,
            0,
            &mut st,
        );
        let edx = p3x - st.last_x;
        let edy = p3y - st.last_y;
        st.accum_dist += js::floor(js::sqrt(edx * edx + edy * edy));
        st.accum_dist / scale
    }

    /// `getAllPoints()` (`:56-58`).
    pub fn get_all_points(&self) -> &[Point] {
        &self.cached_points
    }

    /// `increment(distance = 1)` (`:64-82`).
    pub fn increment(&mut self, distance: f64) -> Option<Point> {
        let acc = f64::from(self.accumulated_distance_scaled) + js::max(1.0, js::round(distance * SUB_SCALE));
        self.accumulated_distance_scaled = exact_i32(acc);
        let last = self.cached_points.len() as i64 - 1;
        while i64::from(self.current_index) < last && self.accumulated_distance_scaled >= self.pixel_spacing_scaled {
            self.current_index += 1;
            self.accumulated_distance_scaled -= self.pixel_spacing_scaled;
        }
        if i64::from(self.current_index) >= last {
            return None;
        }
        Some(self.cached_points[self.current_index as usize])
    }

    /// `getCurrentIndex()` (`:84-86`).
    pub fn get_current_index(&self) -> i32 {
        self.current_index
    }

    /// `setProgress(currentIndex, accumulatedDistanceScaled)` (`:102-105`).
    pub fn set_progress(&mut self, current_index: i32, accumulated_distance_scaled: i32) {
        self.current_index = current_index;
        self.accumulated_distance_scaled = accumulated_distance_scaled;
    }

    /// `computeAllPoints(pixelSpacing)` (`:111-168`): single-pass in-order recursive
    /// De Casteljau subdivision with integer distance accumulation.
    pub fn compute_all_points(&mut self, pixel_spacing: f64) {
        self.cached_points = Vec::new();
        self.current_index = 0;
        self.accumulated_distance_scaled = 0;
        self.pixel_spacing_scaled = exact_i32(js::max(SUB_SCALE, js::round(pixel_spacing * SUB_SCALE)));

        let scale = SUB_SCALE; // 8-bit fixed-point precision
        let step_threshold = f64::from(self.pixel_spacing_scaled);
        let [p0, p1, p2, p3] = self.points;
        let p0x = js::round(p0.x) * scale;
        let p0y = js::round(p0.y) * scale;
        let p3x = js::round(p3.x) * scale;
        let p3y = js::round(p3.y) * scale;

        let mut cached: Vec<Point> = Vec::new();
        cached.push(Point { x: round_px(p0x), y: round_px(p0y) });
        {
            let mut st =
                SubdivState { last_x: p0x, last_y: p0y, accum_dist: 0.0, step: Some((step_threshold, &mut cached)) };
            Self::shared_subdivide(
                p0x,
                p0y,
                js::round(p1.x) * scale,
                js::round(p1.y) * scale,
                js::round(p2.x) * scale,
                js::round(p2.y) * scale,
                p3x,
                p3y,
                0,
                &mut st,
            );
        }
        // Ensure endpoint is included if not already P3 (`:155-167`)
        let last_pt = Point { x: round_px(p3x), y: round_px(p3y) };
        match cached.last() {
            Some(end) => {
                if end.x != last_pt.x || end.y != last_pt.y {
                    cached.push(last_pt);
                }
            }
            None => cached.push(last_pt),
        }
        self.cached_points = cached;
    }

    /// `sharedSubdivide(ax, ay, bx, by, cx, cy, dx, dy, depth, st)` (`:170-256`).
    #[allow(clippy::too_many_arguments)] // literal port of the TS signature
    fn shared_subdivide(
        ax: f64,
        ay: f64,
        bx: f64,
        by: f64,
        cx: f64,
        cy: f64,
        dx: f64,
        dy: f64,
        depth: u32,
        st: &mut SubdivState<'_>,
    ) {
        let dist = js::abs(bx - ax)
            + js::abs(by - ay)
            + js::abs(cx - bx)
            + js::abs(cy - by)
            + js::abs(dx - cx)
            + js::abs(dy - cy);
        if dist <= 256.0 || depth >= 10 {
            let edx = ax - st.last_x;
            let edy = ay - st.last_y;
            st.accum_dist += js::floor(js::sqrt(edx * edx + edy * edy));
            st.last_x = ax;
            st.last_y = ay;
            if let Some((step_threshold, cached)) = st.step.as_mut() {
                while st.accum_dist >= *step_threshold {
                    cached.push(Point { x: round_px(ax), y: round_px(ay) });
                    st.accum_dist -= *step_threshold;
                }
            }
            return;
        }
        // De Casteljau midpoints via bitwise right-shift >> 1
        let m01_x = mid(ax, bx);
        let m01_y = mid(ay, by);
        let m12_x = mid(bx, cx);
        let m12_y = mid(by, cy);
        let m23_x = mid(cx, dx);
        let m23_y = mid(cy, dy);
        let m012_x = mid(m01_x, m12_x);
        let m012_y = mid(m01_y, m12_y);
        let m123_x = mid(m12_x, m23_x);
        let m123_y = mid(m12_y, m23_y);
        let mx = mid(m012_x, m123_x);
        let my = mid(m012_y, m123_y);
        // IN-ORDER RECURSION: Left segment first, then Right segment
        Self::shared_subdivide(ax, ay, m01_x, m01_y, m012_x, m012_y, mx, my, depth + 1, st);
        Self::shared_subdivide(mx, my, m123_x, m123_y, m23_x, m23_y, dx, dy, depth + 1, st);
    }
}
