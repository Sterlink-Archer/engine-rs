//! Exact port of `packages/engine-lib/src/DetMath.ts` (engine pin 5b75bd3).
//!
//! DetMath uses only `+ - * /` and bit views, so plain Rust f64 arithmetic in the same
//! operation order reproduces it bit for bit (Rust never fuses into FMA). Quirks kept:
//! `log(+Inf)` and `log(NaN)` are FINITE (the exponent field 0x7ff is read as e = 1024),
//! `pow(+Inf, y)` is finite, `pow2` truncates its argument through an int32 shift
//! (`pow2(-0.5) = 0.5`, `pow2(NaN) = +0`). Checked by `of-vectors` against
//! `detmath.json.zst` (exp, log, pow, pow2, atan2, nuke_ring_lookup).

use crate::js::to_int32;

pub const LN2: f64 = 0.6931471805599453;
pub const LOG2E: f64 = 1.4426950408889634;
pub const SQRT2: f64 = 1.4142135623730951;
pub const PI: f64 = 3.141592653589793;
pub const PI_2: f64 = 1.5707963267948966;
pub const PI_4: f64 = 0.7853981633974483;
pub const TAN_PI_8: f64 = 0.41421356237309503;

/// Build an f64 from (hi, lo) 32-bit words.
#[inline(always)]
fn from_words(hi: u32, lo: u32) -> f64 {
    f64::from_bits(((hi as u64) << 32) | lo as u64)
}

/// `pow2(n)` (DetMath.ts:31): `u32[HI] = (n + 1023) << 20; u32[LO] = 0`.
#[inline]
pub fn pow2(n: f64) -> f64 {
    if n > 1023.0 {
        return f64::INFINITY;
    }
    if n < -1022.0 {
        return 0.0;
    }
    let hi = (to_int32(n + 1023.0) as u32).wrapping_shl(20);
    from_words(hi, 0)
}

/// `exp(x)` (DetMath.ts:40).
#[inline]
pub fn exp(x: f64) -> f64 {
    if x > 709.0 {
        return f64::INFINITY;
    }
    if x < -708.0 {
        return 0.0;
    }
    let n = (x * LOG2E + 0.5).floor();
    let r = x - n * LN2;
    let p = 1.0
        + r * (1.0
            + r * (1.0 / 2.0
                + r * (1.0 / 6.0
                    + r * (1.0 / 24.0
                        + r * (1.0 / 120.0 + r * (1.0 / 720.0 + r * (1.0 / 5040.0 + r * (1.0 / 40320.0))))))));
    p * pow2(n)
}

/// `log(x)` (DetMath.ts:64).
#[inline]
pub fn log(x: f64) -> f64 {
    let mut x = x;
    if x <= 0.0 {
        return if x == 0.0 { f64::NEG_INFINITY } else { f64::NAN };
    }
    let mut e: i32 = 0;
    if x < 2.2250738585072014e-308 {
        x *= 18014398509481984.0; // 2^54
        e = -54;
    }
    let bits = x.to_bits();
    let hi = (bits >> 32) as u32;
    let lo = bits as u32;
    e += ((hi >> 20) & 0x7ff) as i32 - 1023;
    let mut m = from_words((hi & 0x000f_ffff) | 0x3ff0_0000, lo);
    if m > SQRT2 {
        m *= 0.5;
        e += 1;
    }
    let s = (m - 1.0) / (m + 1.0);
    let z = s * s;
    let series = 1.0
        + z * (1.0 / 3.0 + z * (1.0 / 5.0 + z * (1.0 / 7.0 + z * (1.0 / 9.0 + z * (1.0 / 11.0 + z * (1.0 / 13.0))))));
    e as f64 * LN2 + 2.0 * s * series
}

/// `pow(x, y)` for x >= 0 (DetMath.ts:92).
#[inline]
pub fn pow(x: f64, y: f64) -> f64 {
    if y == 0.0 || x == 1.0 {
        return 1.0;
    }
    if x == 0.0 {
        return if y > 0.0 { 0.0 } else { f64::INFINITY };
    }
    if x < 0.0 {
        return f64::NAN;
    }
    exp(y * log(x))
}

/// `atanUnit(z)` for z in [0, 1] (DetMath.ts:100).
#[inline]
fn atan_unit(z: f64) -> f64 {
    let mut z = z;
    let mut base = 0.0;
    if z > TAN_PI_8 {
        base = PI_4;
        z = (z - 1.0) / (z + 1.0);
    }
    let w = z * z;
    let series = 1.0
        - w * (1.0 / 3.0
            - w * (1.0 / 5.0
                - w * (1.0 / 7.0
                    - w * (1.0 / 9.0 - w * (1.0 / 11.0 - w * (1.0 / 13.0 - w * (1.0 / 15.0 - w / 17.0)))))));
    base + z * series
}

/// `atan2(y, x)` (DetMath.ts:124): angle in (-pi, pi], signed zeros ignored.
#[inline]
pub fn atan2(y: f64, x: f64) -> f64 {
    if y == 0.0 {
        return if x >= 0.0 { 0.0 } else { PI };
    }
    if x == 0.0 {
        return if y > 0.0 { PI_2 } else { -PI_2 };
    }
    let ax = if x < 0.0 { -x } else { x };
    let ay = if y < 0.0 { -y } else { y };
    let mut a = if ay <= ax { atan_unit(ay / ax) } else { PI_2 - atan_unit(ax / ay) };
    if x < 0.0 {
        a = PI - a;
    }
    if y < 0.0 { -a } else { a }
}

/// NukeExecution.ts:120-124 ring lookup (NUM_SAMPLES = 16): `angle = atan2(dy,dx) + Math.PI`,
/// `t = (angle / (2*Math.PI)) * 16`, `i0 = Math.floor(t) % 16`, `frac = t - Math.floor(t)`.
#[inline]
pub fn nuke_ring_lookup(dy: f64, dx: f64) -> (f64, f64, f64) {
    let angle = atan2(dy, dx) + PI;
    let t = (angle / (2.0 * PI)) * 16.0;
    (t, t.floor() % 16.0, t - t.floor())
}
