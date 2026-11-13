//! TS-equivalent throws (ARCHITECTURE §4.6, PORTING_GUIDE §11; ledger t088 item (e)).
//!
//! Exactly two panic classes exist in the port:
//! 1. [`TsThrow`]: a throw that TS performs at the same point. Raised only by
//!    [`ts_throw!`](crate::ts_throw), [`TsExpect::ts_expect`] and the strict BigInt
//!    conversions (`bigint::{from_number_strict, from_floor_strict, div_strict, rem_strict}`,
//!    `util::to_int_strict`). The payload is a `TsThrow` passed to `std::panic::panic_any`.
//! 2. Everything else (overflow checks, `debug_assert!`, out-of-bounds indexing,
//!    `unreachable!`, arena generation checks): a Rust bug with no TS counterpart. Those are
//!    always fatal and are never mapped to a TS behaviour.
//!
//! `msg` is what TS's `error.message` holds (no `"RangeError: "` prefix), because that is
//! what `GameRunner` reports (`engine/src/GameRunner.ts:236-242`, `errMsg: error.message`)
//! and what the trace writer records as `fatal.message` (SPEC §2.2).

use std::any::Any;
use std::borrow::Cow;
use std::fmt;

/// The JS error class of a throw. Only informative: the conformance diff compares `msg`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TsErrorKind {
    /// `throw new Error(..)`.
    Error,
    /// BigInt conversions and `/ 0n` (`RangeError`).
    RangeError,
    /// Implicit `TypeError`s: `undefined.foo`, a failing `x!`.
    TypeError,
}

/// Panic payload for a TS-equivalent throw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TsThrow {
    pub kind: TsErrorKind,
    pub msg: Cow<'static, str>,
}

impl fmt::Display for TsThrow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.msg)
    }
}

/// Raise a [`TsThrow`]. Never returns.
#[cold]
#[inline(never)]
pub fn raise(kind: TsErrorKind, msg: Cow<'static, str>) -> ! {
    std::panic::panic_any(TsThrow { kind, msg })
}

/// Backend of [`ts_throw!`](crate::ts_throw): keeps a static message borrowed.
#[cold]
#[inline(never)]
pub fn raise_fmt(kind: TsErrorKind, args: fmt::Arguments<'_>) -> ! {
    let msg = match args.as_str() {
        Some(s) => Cow::Borrowed(s),
        None => Cow::Owned(args.to_string()),
    };
    raise(kind, msg)
}

/// `throw new Error(msg)`: `ts_throw!("Re-entrant bfsSearch is not supported")`,
/// `ts_throw!("Invalid coordinates: {x},{y}")`. Use the TS message text verbatim.
/// `ts_throw!(RangeError, "Division by zero")` selects another error class.
#[macro_export]
macro_rules! ts_throw {
    (RangeError, $($arg:tt)+) => {
        $crate::throw::raise_fmt($crate::throw::TsErrorKind::RangeError, ::std::format_args!($($arg)+))
    };
    (TypeError, $($arg:tt)+) => {
        $crate::throw::raise_fmt($crate::throw::TsErrorKind::TypeError, ::std::format_args!($($arg)+))
    };
    ($($arg:tt)+) => {
        $crate::throw::raise_fmt($crate::throw::TsErrorKind::Error, ::std::format_args!($($arg)+))
    };
}

/// `.ts_expect("<TS file:line>")` at every TS `TypeError` site (`x!`, `undefined.foo`).
/// Not `expect`/`unwrap`, which are Rust-bug panics (PORTING_GUIDE §1, §11).
pub trait TsExpect<T> {
    fn ts_expect(self, site: &'static str) -> T;
}

impl<T> TsExpect<T> for Option<T> {
    #[inline]
    #[track_caller]
    fn ts_expect(self, site: &'static str) -> T {
        match self {
            Some(v) => v,
            None => raise(TsErrorKind::TypeError, Cow::Borrowed(site)),
        }
    }
}

impl<T, E> TsExpect<T> for Result<T, E> {
    #[inline]
    #[track_caller]
    fn ts_expect(self, site: &'static str) -> T {
        match self {
            Ok(v) => v,
            Err(_) => raise(TsErrorKind::TypeError, Cow::Borrowed(site)),
        }
    }
}

/// Is this panic payload a TS-equivalent throw? (The stepper and `Game::step` boundary.)
#[inline]
pub fn is_ts_throw(payload: &(dyn Any + Send)) -> bool {
    payload.is::<TsThrow>()
}

/// Borrow the `TsThrow` inside a panic payload, if it is one.
#[inline]
pub fn as_ts_throw(payload: &(dyn Any + Send)) -> Option<&TsThrow> {
    payload.downcast_ref::<TsThrow>()
}

/// JS `Number.prototype.toString()` for the values that appear in V8's BigInt
/// conversion message. Exact for NaN, ±Infinity, integers and short decimals (the
/// shortest round-trip form is the same as Rust's `{}` below 1e21); exponent forms are
/// approximated. Only feeds error-message text, never state.
pub fn js_number_to_string(x: f64) -> String {
    if x.is_nan() {
        "NaN".into()
    } else if x == f64::INFINITY {
        "Infinity".into()
    } else if x == f64::NEG_INFINITY {
        "-Infinity".into()
    } else if x == 0.0 {
        "0".into()
    } else if x.abs() < 1e21 && x.abs() >= 1e-6 {
        format!("{x}")
    } else {
        // V8 writes e.g. "1e+21"; Rust writes "1e21".
        let s = format!("{x:e}");
        match s.split_once('e') {
            Some((m, e)) if !e.starts_with('-') => format!("{m}e+{e}"),
            _ => s,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{AssertUnwindSafe, catch_unwind};

    fn caught<F: FnOnce()>(f: F) -> TsThrow {
        let p = catch_unwind(AssertUnwindSafe(f)).expect_err("must throw");
        as_ts_throw(&*p).expect("TsThrow payload").clone()
    }

    #[test]
    fn macro_forms() {
        let t = caught(|| ts_throw!("Re-entrant bfsSearch is not supported"));
        assert_eq!(
            t,
            TsThrow { kind: TsErrorKind::Error, msg: Cow::Borrowed("Re-entrant bfsSearch is not supported") }
        );
        assert!(matches!(t.msg, Cow::Borrowed(_)));
        let (x, y) = (3, -1);
        let t = caught(|| ts_throw!("Invalid coordinates: {x},{y}"));
        assert_eq!(t.msg, "Invalid coordinates: 3,-1");
        let t = caught(|| ts_throw!(RangeError, "Division by zero"));
        assert_eq!(t.kind, TsErrorKind::RangeError);
    }

    #[test]
    fn ts_expect_and_classes() {
        let none: Option<u8> = None;
        let t = caught(|| {
            none.ts_expect("GameImpl.ts:931");
        });
        assert_eq!((t.kind, &*t.msg), (TsErrorKind::TypeError, "GameImpl.ts:931"));
        assert_eq!(Some(4).ts_expect("x"), 4);
        // A Rust bug panic is NOT a TsThrow.
        let p = catch_unwind(|| {
            let v: Vec<u8> = vec![];
            v[std::hint::black_box(1)]
        })
        .expect_err("oob");
        assert!(!is_ts_throw(&*p));
    }

    #[test]
    fn number_text() {
        assert_eq!(js_number_to_string(f64::NAN), "NaN");
        assert_eq!(js_number_to_string(f64::NEG_INFINITY), "-Infinity");
        assert_eq!(js_number_to_string(1.5), "1.5");
        assert_eq!(js_number_to_string(-0.0), "0");
        assert_eq!(js_number_to_string(1e21), "1e+21");
    }
}
