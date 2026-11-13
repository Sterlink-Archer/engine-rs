//! Numeric helpers: `packages/engine-lib/src/Util.ts`, `packages/engine/src/execution/Util.ts`
//! (`closestTwoTiles`), `packages/engine-lib/src/game/DoomsdayClock.ts` (integer formulas)
//! and `packages/engine-lib/src/game/Veterancy.ts`. Checked against `util.json.zst`.

use crate::bigint::{self, BigError, BigResult};
use crate::detmath;
use crate::js;
use crate::timsort;

/// `simpleHash(str)` (Util.ts:105) over UTF-16 code units. Returns a JS number: it can be
/// 2147483648 (`Math.abs(-2^31)`), which does not fit an i32.
pub fn simple_hash_u16(units: &[u16]) -> f64 {
    let mut h: i32 = 0;
    for &c in units {
        // (hash << 5) - hash + char, then `hash & hash` = ToInt32. Exact in i64.
        let v = (h.wrapping_shl(5) as i64) - (h as i64) + c as i64;
        h = v as i32; // wrap modulo 2^32
    }
    (h as i64).abs() as f64
}

/// `simpleHash` of a Rust string (encoded as UTF-16 like JS `charCodeAt`).
pub fn simple_hash(s: &str) -> f64 {
    let units: Vec<u16> = s.encode_utf16().collect();
    simple_hash_u16(&units)
}

/// `toInt(num)` (Util.ts:232): ±Inf clamp to ±MAX_SAFE_INTEGER, NaN throws RangeError
/// (reported as `NotInteger`), else `BigInt(Math.floor(num))`.
pub fn to_int(num: f64) -> BigResult {
    if num == f64::INFINITY {
        return Ok(9_007_199_254_740_991);
    }
    if num == f64::NEG_INFINITY {
        return Ok(-9_007_199_254_740_991);
    }
    if num.is_nan() {
        return Err(BigError::NotInteger);
    }
    bigint::from_number_floor(num)
}

/// `toInt(num)` (`engine-lib/src/Util.ts:232-240`) on the engine's `i64` bigint storage:
/// ±Inf clamp to ±(2^53−1); NaN -> `TsThrow` (RangeError, as `BigInt(NaN)` throws); else
/// `BigInt(Math.floor(num))`. **Only** for calls to `toInt` itself (PORTING_GUIDE §4).
#[inline]
#[track_caller]
pub fn to_int_strict(num: f64) -> i64 {
    if num == f64::INFINITY {
        return 9_007_199_254_740_991;
    }
    if num == f64::NEG_INFINITY {
        return -9_007_199_254_740_991;
    }
    bigint::from_floor_strict(num)
}

/// `within(value, min, max)` (Util.ts:24) = `Math.min(Math.max(value, min), max)`.
#[inline]
pub fn within(value: f64, lo: f64, hi: f64) -> f64 {
    js::min(js::max(value, lo), hi)
}

/// `sigmoid(value, decayRate, midpoint)` (Util.ts:254).
#[inline]
pub fn sigmoid(value: f64, decay_rate: f64, midpoint: f64) -> f64 {
    1.0 / (1.0 + detmath::exp(-decay_rate * (value - midpoint)))
}

/// `manhattanDistWrapped(c1, c2, width)` (Util.ts:7).
#[inline]
pub fn manhattan_dist_wrapped(x1: f64, y1: f64, x2: f64, y2: f64, width: f64) -> f64 {
    let mut dx = (x1 - x2).abs();
    dx = js::min(dx, width - dx);
    let dy = (y1 - y2).abs();
    dx + dy
}

/// `boundingBoxCenter({min, max})` (Util.ts:209).
#[inline]
pub fn bounding_box_center(minx: f64, miny: f64, maxx: f64, maxy: f64) -> (f64, f64) {
    (minx + ((maxx - minx) / 2.0).floor(), miny + ((maxy - miny) / 2.0).floor())
}

/// `inscribed(outer, inner)` (Util.ts:216).
#[inline]
pub fn inscribed(outer: (f64, f64, f64, f64), inner: (f64, f64, f64, f64)) -> bool {
    outer.0 <= inner.0 && outer.1 <= inner.1 && outer.2 >= inner.2 && outer.3 >= inner.3
}

/// `findMinimumBy(values, score, isCandidate?)` (Util.ts:55): first strict minimum,
/// starting from `bestScore = Infinity` (so all-NaN / all-Inf -> None).
pub fn find_minimum_by<T, S, P>(values: &[T], mut score: S, mut is_candidate: Option<P>) -> Option<usize>
where
    S: FnMut(&T) -> f64,
    P: FnMut(&T) -> bool,
{
    let mut best = None;
    let mut best_score = f64::INFINITY;
    for (i, v) in values.iter().enumerate() {
        if let Some(p) = is_candidate.as_mut() {
            if !p(v) {
                continue;
            }
        }
        let s = score(v);
        if s < best_score {
            best_score = s;
            best = Some(i);
        }
    }
    best
}

/// `getMode(counts)` (Util.ts:187) over `(key, count)` pairs in Map insertion order.
pub fn get_mode<K: Clone>(counts: impl IntoIterator<Item = (K, f64)>) -> Option<K> {
    let mut mode = None;
    let mut max_count = 0.0;
    for (k, c) in counts {
        if c > max_count {
            max_count = c;
            mode = Some(k);
        }
    }
    mode
}

/// `closestTwoTiles(gm, x, y)` (execution/Util.ts:120) on tile refs, map width `w`.
/// Both lists are sorted by `ref % w` with `Array.prototype.sort` (V8 TimSort). The
/// comparator `(a % w) - (b % w)` is a consistent total preorder on integer refs (no NaN, no
/// -0 effect on the sign test), and TimSort is stable, so its output is THE stable order by
/// `ref % w`, which any stable sort produces: `sort_by_key` (driftsort, stable) instead of the
/// call-for-call TimSort port (P3 perf, t066: 5.8% of a World FFA game; the equality is
/// pinned by `tests::stable_key_sort_equals_timsort`).
#[allow(clippy::if_same_then_else)] // branch structure mirrors execution/Util.ts:148-160
pub fn closest_two_tiles(w: u32, x: &[u32], y: &[u32]) -> Option<(u32, u32)> {
    if x.is_empty() || y.is_empty() {
        return None;
    }
    let mut xs = x.to_vec();
    let mut ys = y.to_vec();
    sort_by_column(&mut xs, w);
    sort_by_column(&mut ys, w);
    let (mut i, mut j) = (0usize, 0usize);
    let mut min_distance = f64::INFINITY;
    let mut result = (xs[0], ys[0]);
    while i < xs.len() && j < ys.len() {
        let cx = xs[i];
        let cy = ys[j];
        // `ref % w` and `(ref / w) | 0` of non-negative integer refs: the f64 `%` and the
        // ToInt32 of the f64 quotient equal the u32 remainder and quotient (no fmod call).
        let cx_x = f64::from(cx % w);
        let cy_x = f64::from(cy % w);
        let distance = (cx_x - cy_x).abs() + ((i64::from(cx / w) - i64::from(cy / w)) as f64).abs();
        if distance < min_distance {
            min_distance = distance;
            result = (cx, cy);
        }
        if i == xs.len() - 1 {
            j += 1;
        } else if j == ys.len() - 1 {
            i += 1;
        } else if cx_x < cy_x {
            i += 1;
        } else {
            j += 1;
        }
    }
    Some(result)
}

/// Stable sort of tile refs by column `ref % w` (`closestTwoTiles`' `arr.sort((a, b) =>
/// gm.x(a) - gm.x(b))`, see [`closest_two_tiles`]).
#[inline]
pub fn sort_by_column(v: &mut [u32], w: u32) {
    v.sort_by_key(|&r| r % w);
}

/// The TimSort form of [`sort_by_column`] (the reference the stable sort must equal).
pub fn sort_by_column_timsort(v: &mut [u32], w: u32) {
    let wf = w as f64;
    timsort::sort_by(v, |a: &u32, b: &u32| (*a as f64 % wf) - (*b as f64 % wf));
}

// ------------------------------------------------------------------ DoomsdayClock.ts

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DoomsdaySpeed {
    Slow,
    Normal,
    Fast,
    VeryFast,
}

impl DoomsdaySpeed {
    /// Unknown names fall back to `normal` (`SCHEDULES[profile.speed] ?? SCHEDULES.normal`).
    pub fn parse(s: &str) -> Self {
        match s {
            "slow" => Self::Slow,
            "fast" => Self::Fast,
            "veryfast" => Self::VeryFast,
            _ => Self::Normal,
        }
    }
    fn schedule(self) -> (f64, f64, [f64; 7]) {
        // (graceSeconds, rampSeconds (same for every wave), pauseSeconds[i])
        match self {
            Self::Normal => (600.0, 168.0, [54.0, 54.0, 54.0, 54.0, 54.0, 54.0, 0.0]),
            Self::Slow => (600.0, 240.0, [70.0, 70.0, 70.0, 70.0, 70.0, 70.0, 0.0]),
            Self::Fast => (600.0, 102.0, [31.0, 31.0, 31.0, 31.0, 31.0, 31.0, 0.0]),
            Self::VeryFast => (600.0, 36.0, [8.0, 8.0, 8.0, 8.0, 8.0, 8.0, 0.0]),
        }
    }
}

const LEVELS: [f64; 7] = [200.0, 400.0, 700.0, 1100.0, 1700.0, 2500.0, 3500.0];
const LEVELS_TEAM: [f64; 7] = [300.0, 600.0, 1000.0, 1500.0, 2100.0, 2800.0, 3500.0];

/// `requiredBasisPoints(profile, elapsed)` (DoomsdayClock.ts:117).
pub fn doomsday_required_basis_points(speed: DoomsdaySpeed, team_game: bool, elapsed: f64) -> f64 {
    let (grace, ramp, pauses) = speed.schedule();
    let levels = if team_game { &LEVELS_TEAM } else { &LEVELS };
    if elapsed <= grace {
        return 0.0;
    }
    let mut t = elapsed - grace;
    let mut prev = 0.0;
    for i in 0..levels.len() {
        let target = levels[i];
        if t < ramp {
            return prev + (((target - prev) * t) / ramp).floor();
        }
        t -= ramp;
        if t < pauses[i] {
            return target;
        }
        t -= pauses[i];
        prev = target;
    }
    levels[levels.len() - 1]
}

/// `doomsdayClockRequiredTiles(profile, land, elapsed)` (DoomsdayClock.ts:139).
pub fn doomsday_clock_required_tiles(speed: DoomsdaySpeed, team_game: bool, land: f64, elapsed: f64) -> f64 {
    if land <= 0.0 {
        return 0.0;
    }
    ((doomsday_required_basis_points(speed, team_game, elapsed) * land) / 10000.0).floor()
}

/// `rotSpeckleNoise(x, y, salt)` (DoomsdayClock.ts:255).
pub fn rot_speckle_noise(x: f64, y: f64, salt: f64) -> f64 {
    const R2_X: f64 = 3242174889.0;
    const R2_Y: f64 = 2447445413.0;
    // Math.imul results are int32 numbers; their sum is an f64 (exact), then >>> 0.
    let s = js::imul_f64(x, R2_X) as f64 + js::imul_f64(y, R2_Y) as f64 + js::imul_f64(salt, 2654435769.0) as f64;
    let h = js::to_uint32(s);
    ((h >> 16) % 65536) as f64
}

/// `rotFrontNoise(tile, salt)` (DoomsdayClock.ts:267).
pub fn rot_front_noise(tile: f64, salt: f64) -> f64 {
    let mut h = js::imul_f64(tile, 0x27d4eb2d as f64) ^ js::imul_f64(salt, 2654435769.0);
    h ^= ((h as u32) >> 15) as i32;
    h = h.wrapping_mul(0x2545f491);
    h ^= ((h as u32) >> 13) as i32;
    (((h as u32) >> 16) % 65536) as f64
}

/// `doomsdayClockTroopFloor(maxTroops, secondsPastWarn, cfg)` (DoomsdayClock.ts:289).
pub fn doomsday_clock_troop_floor(
    max_troops: f64,
    seconds_past_warn: f64,
    drain_floor_percent: f64,
    floor_start_percent: f64,
    floor_decay_seconds: f64,
) -> f64 {
    let end = drain_floor_percent;
    let span = floor_start_percent - end;
    let t = js::max(0.0, seconds_past_warn);
    let pct = if span > 0.0 && floor_decay_seconds > 0.0 && t < floor_decay_seconds {
        floor_start_percent - ((span * t) / floor_decay_seconds).floor()
    } else {
        end
    };
    ((max_troops * pct) / 100.0).floor()
}

const DRAIN_CURVE_SCALE: f64 = 1_000_000.0;

fn drain_curve_fraction(t: f64, r: f64, exponent: f64) -> f64 {
    let ratio = ((t * DRAIN_CURVE_SCALE) / r).floor();
    let mut acc = DRAIN_CURVE_SCALE;
    let mut i = 0.0;
    while i < exponent {
        acc = ((acc * ratio) / DRAIN_CURVE_SCALE).floor();
        i += 1.0;
    }
    acc
}

/// `doomsdayClockDrain(maxTroops, secondsPastWarn, cfg, curveExponent)` (DoomsdayClock.ts:334).
pub fn doomsday_clock_drain(
    max_troops: f64,
    seconds_past_warn: f64,
    drain_start_percent: f64,
    drain_max_percent: f64,
    drain_ramp_seconds: f64,
    curve_exponent: f64,
) -> f64 {
    let t = js::max(0.0, seconds_past_warn);
    let r = drain_ramp_seconds;
    let span = drain_max_percent - drain_start_percent;
    let mut pct = drain_max_percent;
    if r > 0.0 && t < r {
        let grown = if curve_exponent <= 1.0 {
            ((span * t) / r).floor()
        } else {
            ((span * drain_curve_fraction(t, r, curve_exponent)) / DRAIN_CURVE_SCALE).floor()
        };
        pct = drain_start_percent + grown;
    }
    js::max(1.0, ((max_troops * pct) / 100.0).floor())
}

/// `doomsdayClockRotQuota(tilesLeft, secondsUnder, cfg)` (DoomsdayClock.ts:368).
pub fn doomsday_clock_rot_quota(tiles_left: f64, seconds_under: f64, rot_death_seconds: f64) -> f64 {
    if tiles_left <= 0.0 || rot_death_seconds <= 0.0 {
        return 0.0;
    }
    let seconds_left = js::max(1.0, rot_death_seconds - seconds_under);
    (tiles_left / seconds_left).ceil()
}

/// `maxHealthWithVeterancy(base, veterancy, pct)` (Veterancy.ts:11).
pub fn max_health_with_veterancy(base: f64, veterancy: f64, pct: f64) -> f64 {
    if veterancy <= 0.0 {
        return base;
    }
    base + ((base * veterancy * pct) / 100.0).floor()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P3 (t066) guard: the stable `sort_by_key` used by `closestTwoTiles` gives exactly V8
    /// TimSort's order (duplicates, runs, long inputs that gallop, both map widths).
    #[test]
    fn stable_key_sort_equals_timsort() {
        let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for case in 0..3000 {
            let w = if case % 2 == 0 { 2000 } else { 37 };
            let n = (next() % if case % 10 == 0 { 5000 } else { 300 }) as usize;
            let span = 1 + (next() % 4_000_000) as u32;
            let mut v: Vec<u32> = (0..n).map(|_| (next() % u64::from(span)) as u32).collect();
            if case % 7 == 0 {
                v.sort_unstable(); // pre-sorted runs
            }
            let (mut a, mut b) = (v.clone(), v);
            sort_by_column(&mut a, w);
            sort_by_column_timsort(&mut b, w);
            assert_eq!(a, b, "case {case} w {w}");
        }
    }
}
