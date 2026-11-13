//! JS value shapes that Rust's `Option` cannot express (PORTING_GUIDE §8). Owner: P1.
//!
//! The snapshot schema dump distinguishes a field that is **absent** (`?undef`) from one
//! that is **`null`** (`?null`); CS v1 keeps the difference (SPEC §1.1, N7). Where a TS
//! field can be both (`?null?undef` in the dump), the port uses [`Tri`]:
//! `Player.largestClusterBoundingBox`, `NukeExecution.src`, `TransportShipExecution.dst/src`.

use crate::plain::PlainState;

/// `T | null | undefined`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Tri<T> {
    /// `undefined` (the field is absent from the snapshot record).
    #[default]
    Absent,
    /// `null`.
    Null,
    /// A value.
    Some(T),
}

impl<T: PlainState> PlainState for Tri<T> {}

impl<T: crate::canon::Refs> crate::canon::Refs for Tri<T> {
    #[inline]
    fn visit_refs(&self, v: &mut dyn crate::canon::RefVisitor) {
        if let Tri::Some(x) = self {
            x.visit_refs(v);
        }
    }
}

impl<T> Tri<T> {
    /// The value, if any (`null` and `undefined` both map to `None`, as `x ?? y` does).
    #[inline]
    pub fn value(&self) -> Option<&T> {
        match self {
            Tri::Some(v) => Some(v),
            _ => None,
        }
    }
    /// `x == null` (loose equality: null or undefined).
    #[inline]
    pub fn is_nullish(&self) -> bool {
        !matches!(self, Tri::Some(_))
    }
    /// `x === undefined`.
    #[inline]
    pub fn is_absent(&self) -> bool {
        matches!(self, Tri::Absent)
    }
}

impl<T: Copy> Tri<T> {
    /// `x ?? undefined` as an `Option` (null and undefined → `None`).
    #[inline]
    pub fn get(self) -> Option<T> {
        match self {
            Tri::Some(v) => Some(v),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tri_basics() {
        let a: Tri<u32> = Tri::default();
        assert!(a.is_absent() && a.is_nullish() && a.get().is_none());
        assert!(!Tri::<u32>::Null.is_absent() && Tri::<u32>::Null.is_nullish());
        assert_eq!(Tri::Some(3).get(), Some(3));
    }
}
