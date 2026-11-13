//! JS (ECMAScript / V8) numeric semantics the engine relies on, ported bit-exactly.
//!
//! Every function here is checked against `conformance/golden/vectors/jssem.json.zst`
//! (V8 13.6.233.17, Node v24.21.0) by `of-vectors`. Rust's IEEE-754 `+ - * / %`, `sqrt`,
//! `floor`, `ceil`, `trunc` and `f64 as f32` are already exactly the JS operations
//! (Rust never contracts `a*b+c` into an FMA), so only the operators whose JS meaning
//! differs from Rust's live here:
//!
//! | JS | here | why it differs from the obvious Rust |
//! |---|---|---|
//! | `Math.round` | [`round`] | half toward +inf, keeps -0; `f64::round` is half away from zero |
//! | `x \| 0`, `x >>> 0` | [`to_int32`], [`to_uint32`] | modular wrap of the truncated value; `as i32` saturates |
//! | `Math.imul` | [`imul`] | wrapping i32 multiply of ToInt32 operands |
//! | `<< >> >>>` | [`shl`], [`sar`], [`shr`] | count is `ToUint32(s) & 31` |
//! | `Math.min/max` | [`min`], [`max`] | NaN-propagating, `min(0,-0) = -0`; `f64::min` ignores NaN |
//! | `Math.sign` | [`sign`] | keeps ±0 and NaN; `f64::signum(0.0) = 1.0` |
//! | `Math.fround` | [`fround`] | = `x as f32 as f64` (round-to-nearest-even, overflow to inf) |
//! | `Number.prototype.toString(36)` | [`to_string_radix`] | integers only |

/// `2^32` as f64.
pub const TWO32: f64 = 4_294_967_296.0;
/// `Number.MAX_SAFE_INTEGER`.
pub const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// ECMAScript ToInt32 (`x | 0`): NaN/±Inf -> 0, else truncate and wrap modulo 2^32.
#[inline]
pub fn to_int32(x: f64) -> i32 {
    if !x.is_finite() {
        return 0;
    }
    // Fast path: already in i32 range.
    if x > -2_147_483_649.0 && x < 2_147_483_648.0 {
        return x as i32; // `as` truncates toward zero; in range, no saturation
    }
    // Fast path: [2^31, 2^32) (e.g. `>>> 0` of a uint32): trunc mod 2^32 is the value itself,
    // reinterpreted (P3 perf: skips the fmod call).
    if (2_147_483_648.0..4_294_967_296.0).contains(&x) {
        return x as u32 as i32;
    }
    let t = x.trunc();
    // fmod is exact; result in (-2^32, 2^32).
    let mut m = t % TWO32;
    if m < 0.0 {
        m += TWO32; // exact: m is an integer in (-2^32, 0)
    }
    (m as u64 as u32) as i32
}

/// ECMAScript ToUint32 (`x >>> 0`).
#[inline]
pub fn to_uint32(x: f64) -> u32 {
    to_int32(x) as u32
}

/// `Math.imul(a, b)` on int32 operands.
#[inline]
pub fn imul(a: i32, b: i32) -> i32 {
    a.wrapping_mul(b)
}

/// `Math.imul(a, b)` on arbitrary JS numbers (ToInt32 on both).
#[inline]
pub fn imul_f64(a: f64, b: f64) -> i32 {
    imul(to_int32(a), to_int32(b))
}

/// `a << s` (JS numbers).
#[inline]
pub fn shl(a: f64, s: f64) -> i32 {
    to_int32(a).wrapping_shl(to_uint32(s) & 31)
}
/// `a >> s` (JS numbers).
#[inline]
pub fn sar(a: f64, s: f64) -> i32 {
    to_int32(a) >> (to_uint32(s) & 31)
}
/// `a >>> s` (JS numbers).
#[inline]
pub fn shr(a: f64, s: f64) -> u32 {
    to_uint32(a) >> (to_uint32(s) & 31)
}

/// `Math.round`: round half toward +Infinity; -0 for -0.5 <= x < 0 and for -0.
///
/// Not `(x + 0.5).floor()` (wrong at 0.49999999999999994 and above 2^52).
/// Form verified on 8,664 V8 cases (`check_vectors.py` `js_round`, VECTORS.md §5).
#[inline]
pub fn round(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let f = x.floor();
    let r = if x - f >= 0.5 { f + 1.0 } else { f };
    if r == 0.0 && (x < 0.0 || x.is_sign_negative()) { -0.0 } else { r }
}

/// `Math.floor` (Rust's floor already keeps -0 and NaN/inf).
#[inline]
pub fn floor(x: f64) -> f64 {
    x.floor()
}
/// `Math.ceil` (IEEE ceil: `ceil(-0.5) = -0`).
#[inline]
pub fn ceil(x: f64) -> f64 {
    x.ceil()
}
/// `Math.trunc`.
#[inline]
pub fn trunc(x: f64) -> f64 {
    x.trunc()
}
/// `Math.abs`.
#[inline]
pub fn abs(x: f64) -> f64 {
    x.abs()
}
/// `Math.sqrt` (correctly rounded on every IEEE platform).
#[inline]
pub fn sqrt(x: f64) -> f64 {
    x.sqrt()
}

/// `Math.sign`: NaN -> NaN, ±0 -> ±0, else ±1.
#[inline]
pub fn sign(x: f64) -> f64 {
    if x.is_nan() || x == 0.0 {
        x
    } else if x > 0.0 {
        1.0
    } else {
        -1.0
    }
}

/// `Math.fround`: the value a `Float32Array` store reads back as.
#[inline]
pub fn fround(x: f64) -> f64 {
    x as f32 as f64
}

/// `Math.min(a, b)`: NaN if either is NaN; `min(0, -0) = -0`.
#[inline]
pub fn min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        return f64::NAN;
    }
    if a == 0.0 && b == 0.0 {
        return if a.is_sign_negative() || b.is_sign_negative() { -0.0 } else { 0.0 };
    }
    if a < b { a } else { b }
}

/// `Math.max(a, b)`: NaN if either is NaN; `max(-0, 0) = 0`.
#[inline]
pub fn max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        return f64::NAN;
    }
    if a == 0.0 && b == 0.0 {
        return if a.is_sign_positive() || b.is_sign_positive() { 0.0 } else { -0.0 };
    }
    if a > b { a } else { b }
}

/// `Math.min()` over a slice (empty -> +Infinity), left fold like V8.
pub fn min_n(xs: &[f64]) -> f64 {
    xs.iter().fold(f64::INFINITY, |acc, &x| min(acc, x))
}
/// `Math.max()` over a slice (empty -> -Infinity).
pub fn max_n(xs: &[f64]) -> f64 {
    xs.iter().fold(f64::NEG_INFINITY, |acc, &x| max(acc, x))
}

/// JS `a % b` (= C fmod; sign of the dividend; `-0 % 5 = -0`). Rust `%` on f64 is fmod.
#[inline]
pub fn rem(a: f64, b: f64) -> f64 {
    a % b
}

/// JS `x || y` where both are numbers: `y` when `x` is ±0 or NaN.
#[inline]
pub fn or_num(x: f64, y: f64) -> f64 {
    if x.is_nan() || x == 0.0 { y } else { x }
}

/// Is `x` a "safe integer" (`Number.isSafeInteger`)? Note -0 IS a safe integer in JS.
#[inline]
pub fn is_safe_integer(x: f64) -> bool {
    x.is_finite() && x.trunc() == x && x.abs() <= MAX_SAFE_INTEGER
}

/// `Number.prototype.toString(radix)` for integral `x` with |x| < 2^53 (the engine only
/// formats integers: `nextID` uses base 36). Panics on a non-integer or non-finite input.
pub fn to_string_radix(x: f64, radix: u32) -> String {
    assert!((2..=36).contains(&radix));
    assert!(x.is_finite() && x.trunc() == x, "to_string_radix: non-integer {x}");
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let neg = x < 0.0;
    let mut n = x.abs();
    if n == 0.0 {
        return "0".into();
    }
    let mut out = Vec::new();
    let r = radix as f64;
    while n > 0.0 {
        let d = n % r; // exact for integers
        out.push(DIGITS[d as usize]);
        n = ((n - d) / r).trunc(); // exact: n - d is a multiple of r
    }
    if neg {
        out.push(b'-');
    }
    out.reverse();
    String::from_utf8(out).unwrap()
}

/// JS `String.prototype.padStart(len, "0")` for ASCII strings.
pub fn pad_start_zero(s: &str, len: usize) -> String {
    if s.len() >= len {
        s.to_string()
    } else {
        let mut o = "0".repeat(len - s.len());
        o.push_str(s);
        o
    }
}

/// V8 quietens a signalling NaN when it is passed through a JS value (`7ff0…01` -> `7ff8…01`).
/// Native code that reads raw bits (DetMath.log) only ever sees quiet NaNs from JS.
#[inline]
pub fn quiet_nan_bits(x: f64) -> f64 {
    if x.is_nan() { f64::from_bits(x.to_bits() | 0x0008_0000_0000_0000) } else { x }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_edges() {
        assert_eq!(round(0.49999999999999994), 0.0);
        assert_eq!(round(2.5), 3.0);
        assert_eq!(round(-2.5), -2.0);
        assert!(round(-0.5).is_sign_negative() && round(-0.5) == 0.0);
        assert!(round(-0.0).is_sign_negative());
        assert_eq!(round(4503599627370497.0), 4503599627370497.0);
    }
    #[test]
    fn int32() {
        assert_eq!(to_int32(4294967295.9), -1);
        assert_eq!(to_uint32(4294967295.9), 4294967295);
        assert_eq!(to_int32(1e20), 1661992960);
        assert_eq!(to_int32(-1e20), -1661992960);
        assert_eq!(to_int32(f64::NAN), 0);
        assert_eq!(to_int32(-0.0), 0);
    }
    #[test]
    fn radix() {
        assert_eq!(to_string_radix(1369859069664.0, 36), "hhaypyw0");
        assert_eq!(to_string_radix(0.0, 36), "0");
    }
}
