//! Compile-time dispatch to an owner's `Canon`/`Uncanon` impl, with a reported fallback while
//! the owner has not written it yet. Owner: **P13** (framework).
//!
//! FILE_MAP §5 rule 7 puts every row type's `Canon`/`Uncanon` impl next to the type, written
//! by its owner during S1, in parallel with this framework. The exporter and loader name each
//! row type concretely (`Player`, `Unit`, the 44 execution structs, …), so they would not
//! compile until every owner had landed. Instead they go through [`canon_or_missing!`] /
//! [`uncanon_or_missing!`], which use autoref specialisation on the concrete type:
//!
//! - when `T: Canon`, the `HasCanon` impl on `Probe<T>` matches the receiver `&Probe<T>`
//!   directly and the owner's impl runs;
//! - otherwise method resolution autorefs to `&&Probe<T>` and the `NoCanon` impl on
//!   `&Probe<T>` returns `CanonError::missing(..)`, which the conformance adapter reports as
//!   *unimplemented* (never as a divergence).
//!
//! So the moment an owner's impl merges, the call sites pick it up with no edit here and no
//! coherence conflict. The selection happens per concrete type at macro expansion, never
//! through a generic parameter (inside a generic fn every `T` would take the fallback).

use super::{Canon, CanonCx, CanonError, Cs, Uncanon, UncanonCx};
use std::marker::PhantomData;

#[doc(hidden)]
pub struct Probe<'a, T: ?Sized>(pub &'a T);

#[doc(hidden)]
pub trait HasCanon {
    fn probe_canon(&self, cx: &mut CanonCx<'_>) -> Result<Cs, CanonError>;
}
impl<T: Canon + ?Sized> HasCanon for Probe<'_, T> {
    #[inline]
    fn probe_canon(&self, cx: &mut CanonCx<'_>) -> Result<Cs, CanonError> {
        Ok(self.0.canon(cx))
    }
}

#[doc(hidden)]
pub trait NoCanon {
    fn probe_canon(&self, cx: &mut CanonCx<'_>) -> Result<Cs, CanonError>;
}
impl<T: ?Sized> NoCanon for &Probe<'_, T> {
    fn probe_canon(&self, _cx: &mut CanonCx<'_>) -> Result<Cs, CanonError> {
        Err(CanonError::missing("Canon", std::any::type_name::<T>()))
    }
}

#[doc(hidden)]
pub struct ProbeU<T>(pub PhantomData<T>);

#[doc(hidden)]
pub trait HasUncanon<T> {
    fn probe_uncanon(&self, v: &Cs, cx: &mut UncanonCx<'_>) -> Result<T, CanonError>;
}
impl<T: Uncanon> HasUncanon<T> for ProbeU<T> {
    #[inline]
    fn probe_uncanon(&self, v: &Cs, cx: &mut UncanonCx<'_>) -> Result<T, CanonError> {
        T::uncanon(v, cx)
    }
}

#[doc(hidden)]
pub trait NoUncanon<T> {
    fn probe_uncanon(&self, v: &Cs, cx: &mut UncanonCx<'_>) -> Result<T, CanonError>;
}
impl<T> NoUncanon<T> for &ProbeU<T> {
    fn probe_uncanon(&self, _v: &Cs, _cx: &mut UncanonCx<'_>) -> Result<T, CanonError> {
        Err(CanonError::missing("Uncanon", std::any::type_name::<T>()))
    }
}

/// `canon_or_missing!(x, cx)`: `Ok(x.canon(cx))` if `x`'s (concrete) type implements `Canon`,
/// else `Err(CanonError::missing(..))`.
#[macro_export]
macro_rules! canon_or_missing {
    ($x:expr, $cx:expr) => {{
        #[allow(unused_imports)]
        use $crate::canon::probe::{HasCanon as _, NoCanon as _};
        #[allow(clippy::needless_borrow)]
        (&$crate::canon::probe::Probe(&$x)).probe_canon($cx)
    }};
}

/// `uncanon_or_missing!(T, v, cx)`: `T::uncanon(v, cx)` if the concrete `T` implements
/// `Uncanon`, else `Err(CanonError::missing(..))`.
#[macro_export]
macro_rules! uncanon_or_missing {
    ($t:ty, $v:expr, $cx:expr) => {{
        #[allow(unused_imports)]
        use $crate::canon::probe::{HasUncanon as _, NoUncanon as _};
        #[allow(clippy::needless_borrow)]
        (&$crate::canon::probe::ProbeU::<$t>(::std::marker::PhantomData)).probe_uncanon($v, $cx)
    }};
}

#[cfg(test)]
mod tests {
    use crate::canon::{CanonCx, Cs, LoadKeys, UncanonCx};
    use crate::exec::ExecStore;
    use crate::state::State;

    struct NotYet;

    #[test]
    fn dispatch_picks_impl_or_reports_missing() {
        let st = State::default();
        let ex = ExecStore::default();
        let mut cx = CanonCx::new(&st, &ex);
        assert_eq!(crate::canon_or_missing!(7u32, &mut cx), Ok(Cs::Int(7)));
        let e = crate::canon_or_missing!(NotYet, &mut cx).expect_err("no impl");
        assert!(e.is_missing() && e.0.contains("NotYet"), "{e}");

        let keys = LoadKeys::default();
        let mut st2 = State::default();
        let mut ucx = UncanonCx { st: &mut st2, keys: &keys };
        assert_eq!(crate::uncanon_or_missing!(u32, &Cs::Int(9), &mut ucx), Ok(9));
        let e = crate::uncanon_or_missing!(NotYet, &Cs::Null, &mut ucx).map(|_| ()).expect_err("no impl");
        assert!(e.is_missing(), "{e}");
    }
}
