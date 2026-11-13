//! JS `BigInt` semantics on `i128`.
//!
//! The engine keeps gold, troops, health and some counters as `bigint`
//! (`packages/engine-lib/src/Util.ts:232-251`). Engine magnitudes are far below 2^127,
//! so the port uses `i128` with CHECKED arithmetic: any value outside i128 is an
//! [`BigOverflow`] error (loud failure), never a silent wrap. JS BigInt is unbounded;
//! the golden vectors include a few rows beyond i128 (`jssem` bigint_div_mod_mul,
//! `Number(bigint)`, `BigInt(number)`, `util` toInt) and `of-vectors` checks that each
//! of those is reported as overflow. (Engine values staying < 2^127 is UNVERIFIED in
//! general — infinite-gold cheats are the case to watch.)
//!
//! Semantics (ECMAScript):
//! - `a / b` truncates toward zero; `a % b` has the dividend's sign — same as Rust i128.
//! - division by `0n` throws `RangeError` ([`BigError::DivZero`]).
//! - `Number(b)` rounds to nearest, ties to even — same as Rust `i128 as f64`.
//! - `BigInt(x)` throws for non-integer / NaN / ±Inf numbers.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BigError {
    /// `RangeError: Division by zero`.
    DivZero,
    /// `RangeError: The number … cannot be converted to a BigInt because it is not an integer`.
    NotInteger,
    /// Result outside the i128 domain this port supports.
    BigOverflow,
}

pub type BigResult = Result<i128, BigError>;

/// JS `a / b` on bigints.
#[inline]
pub fn div(a: i128, b: i128) -> BigResult {
    if b == 0 {
        return Err(BigError::DivZero);
    }
    a.checked_div(b).ok_or(BigError::BigOverflow)
}
/// JS `a % b` on bigints.
#[inline]
pub fn rem(a: i128, b: i128) -> BigResult {
    if b == 0 {
        return Err(BigError::DivZero);
    }
    a.checked_rem(b).ok_or(BigError::BigOverflow)
}
/// JS `a * b`.
#[inline]
pub fn mul(a: i128, b: i128) -> BigResult {
    a.checked_mul(b).ok_or(BigError::BigOverflow)
}
/// JS `a + b`.
#[inline]
pub fn add(a: i128, b: i128) -> BigResult {
    a.checked_add(b).ok_or(BigError::BigOverflow)
}
/// JS `a - b`.
#[inline]
pub fn sub(a: i128, b: i128) -> BigResult {
    a.checked_sub(b).ok_or(BigError::BigOverflow)
}

/// JS `Number(b)`: round to nearest, ties to even.
#[inline]
pub fn to_number(b: i128) -> f64 {
    b as f64
}

/// JS `BigInt(x)` for a number `x`.
pub fn from_number(x: f64) -> BigResult {
    if !x.is_finite() || x.trunc() != x {
        return Err(BigError::NotInteger);
    }
    // i128 range is [-2^127, 2^127); f64 integers there convert exactly.
    if !(-1.7014118346046923e38..1.7014118346046923e38).contains(&x) {
        return Err(BigError::BigOverflow);
    }
    Ok(x as i128)
}

/// `BigInt(Math.floor(x))` — the engine's float->bigint idiom.
#[inline]
pub fn from_number_floor(x: f64) -> BigResult {
    from_number(x.floor())
}

/// `maxInt` (`Util.ts:238`).
#[inline]
pub fn max_int(a: i128, b: i128) -> i128 {
    if a > b { a } else { b }
}
/// `minInt` (`Util.ts:242`).
#[inline]
pub fn min_int(a: i128, b: i128) -> i128 {
    if a < b { a } else { b }
}
/// `withinInt` (`Util.ts:245`): `minInt(maxInt(num, min), max)`.
#[inline]
pub fn within_int(num: i128, lo: i128, hi: i128) -> i128 {
    min_int(max_int(num, lo), hi)
}

// ---------------------------------------------------------------------------------------
// Strict i64 conversions for the engine (ARCHITECTURE §7.2, PORTING_GUIDE §4; ledger t088
// item (e)). The engine stores bigints as `i64`. Each wrapper raises a `TsThrow` in EVERY
// build exactly where TS throws a RangeError; a result outside i64 (TS BigInt is
// unbounded) is a plain panic, i.e. `Fatal::Bug` — a port limitation, never a TS behaviour.

const I64_LIMIT: f64 = 9_223_372_036_854_775_808.0; // 2^63

/// V8's `BigInt(x)` RangeError text for a non-integral number.
#[cold]
fn throw_not_integer(x: f64) -> ! {
    crate::throw::raise(
        crate::throw::TsErrorKind::RangeError,
        format!(
            "The number {} cannot be converted to a BigInt because it is not an integer",
            crate::throw::js_number_to_string(x)
        )
        .into(),
    )
}

/// `BigInt(n)` for a number: NaN, ±Inf or a non-integer -> `TsThrow` (RangeError).
/// Never `n as i64`, not even after a `debug_assert!` (PORTING_GUIDE §4).
#[inline]
#[track_caller]
pub fn from_number_strict(n: f64) -> i64 {
    if !n.is_finite() || n.trunc() != n {
        throw_not_integer(n);
    }
    assert!((-I64_LIMIT..I64_LIMIT).contains(&n), "BigInt({n}) is outside the i64 domain of the port (ledger t096)");
    n as i64 // exact: integral and in range
}

/// `BigInt(Math.floor(x))` written out (no `toInt`): **does not clamp**; NaN/±Inf throw.
/// Sites: `EngineConfig.ts:281, 464, 684`, `NationStructureBehavior.ts:881`,
/// `DonateGoldExecution.ts:123`, every `StatsImpl._bigint` (`StatsImpl.ts:58-65`).
#[inline]
#[track_caller]
pub fn from_floor_strict(x: f64) -> i64 {
    from_number_strict(x.floor())
}

/// bigint `a / b`: `0n` divisor -> `TsThrow` ("Division by zero"); truncates toward 0.
#[inline]
#[track_caller]
pub fn div_strict(a: i64, b: i64) -> i64 {
    if b == 0 {
        crate::ts_throw!(RangeError, "Division by zero");
    }
    a.checked_div(b).expect("bigint i64::MIN / -1 overflows the port's i64 domain (ledger t096)")
}

/// bigint `a % b`: `0n` divisor -> `TsThrow` ("Division by zero"); sign of the dividend.
#[inline]
#[track_caller]
pub fn rem_strict(a: i64, b: i64) -> i64 {
    if b == 0 {
        crate::ts_throw!(RangeError, "Division by zero");
    }
    a.checked_rem(b).expect("bigint i64::MIN % -1 overflows the port's i64 domain (ledger t096)")
}

#[cfg(test)]
mod strict_tests {
    use super::*;
    use crate::throw::{TsErrorKind, as_ts_throw};
    use std::panic::{AssertUnwindSafe, catch_unwind};

    fn throw_msg<F: FnOnce() -> i64>(f: F) -> String {
        let p = catch_unwind(AssertUnwindSafe(f)).expect_err("must throw");
        let t = as_ts_throw(&*p).expect("TsThrow");
        assert_eq!(t.kind, TsErrorKind::RangeError);
        t.msg.to_string()
    }

    #[test]
    fn strict_conversions() {
        assert_eq!(from_number_strict(-0.0), 0);
        assert_eq!(from_number_strict(9_007_199_254_740_993.0), 9_007_199_254_740_992);
        assert_eq!(from_floor_strict(-1.5), -2);
        assert_eq!(from_floor_strict(2.999), 2);
        // floor(x) + floor(-x) = -1 for a fraction (StatsImpl attack/attackCancel asymmetry)
        assert_eq!(from_floor_strict(10.5) + from_floor_strict(-10.5), -1);
        assert_eq!(
            throw_msg(|| from_number_strict(1.5)),
            "The number 1.5 cannot be converted to a BigInt because it is not an integer"
        );
        assert_eq!(
            throw_msg(|| from_floor_strict(f64::NAN)),
            "The number NaN cannot be converted to a BigInt because it is not an integer"
        );
        // no clamp (unlike toInt)
        assert_eq!(
            throw_msg(|| from_floor_strict(f64::INFINITY)),
            "The number Infinity cannot be converted to a BigInt because it is not an integer"
        );
        assert_eq!(div_strict(-7, 2), -3);
        assert_eq!(rem_strict(-7, 2), -1);
        assert_eq!(throw_msg(|| div_strict(1, 0)), "Division by zero");
        assert_eq!(throw_msg(|| rem_strict(1, 0)), "Division by zero");
    }

    #[test]
    fn out_of_domain_is_not_a_ts_throw() {
        let p = catch_unwind(|| from_number_strict(1e19)).expect_err("outside i64");
        assert!(as_ts_throw(&*p).is_none());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn semantics() {
        assert_eq!(div(-7, 2), Ok(-3));
        assert_eq!(rem(-7, 2), Ok(-1));
        assert_eq!(rem(7, -2), Ok(1));
        assert_eq!(div(1, 0), Err(BigError::DivZero));
        assert_eq!(from_number(1.5), Err(BigError::NotInteger));
        assert_eq!(from_number(-0.0), Ok(0));
        // 2^53 + 1 is not representable; nearest-even of 9007199254740993 is ...992
        assert_eq!(to_number(9_007_199_254_740_993), 9_007_199_254_740_992.0);
    }
}
