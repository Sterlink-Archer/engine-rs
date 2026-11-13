//! `pathfinding/types.ts`. Owner: **P4b**.
//!
//! Besides the TS types this file holds [`Src`], the Rust spelling of the `from: T | T[]`
//! parameter every `PathFinder.findPath` takes. Scalar vs array is **observable** in TS at
//! two places: `WaterPathMemo.findPath` only memoises a scalar `from`
//! (`PathFinder.ts:116`, `typeof from !== "number"`), and `MiniMapTransformer` picks the
//! closest source only for an array (`MiniMapTransformer.ts:39-55`). So the chain carries
//! the distinction instead of a bare slice.

/// `PathStatus` (`:6-10`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PathStatus {
    Next = 0,
    Complete = 2,
    NotFound = 3,
}

/// `PathResult<T>` (`:12-15`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathResult<T> {
    Next(T),
    Complete(T),
    NotFound,
}

impl<T> PathResult<T> {
    pub fn status(&self) -> PathStatus {
        match self {
            PathResult::Next(_) => PathStatus::Next,
            PathResult::Complete(_) => PathStatus::Complete,
            PathResult::NotFound => PathStatus::NotFound,
        }
    }
    /// `result.node` (`None` for `NOT_FOUND`).
    pub fn node(&self) -> Option<&T> {
        match self {
            PathResult::Next(n) | PathResult::Complete(n) => Some(n),
            PathResult::NotFound => None,
        }
    }
    /// Map the node type (e.g. `u32` → `StationId`).
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> PathResult<U> {
        match self {
            PathResult::Next(n) => PathResult::Next(f(n)),
            PathResult::Complete(n) => PathResult::Complete(f(n)),
            PathResult::NotFound => PathResult::NotFound,
        }
    }
}

/// `from: T | T[]` of `PathFinder.findPath` (`:22`). `One` is a TS number, `Many` a TS
/// array (of any length, 1 included: `Array.isArray([t])` is true).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Src<'a> {
    One(u32),
    Many(&'a [u32]),
}

impl<'a> Src<'a> {
    /// The frozen S0 signatures take `from: &[TileRef]`. Convention (documented for every
    /// caller in `docs/requests/P4b.md`): a **1-element slice is the TS scalar**, any other
    /// length is a TS array. Every TS call site that reaches a scalar/array-sensitive
    /// finder passes a scalar (`TransportShipExecution.ts:149,314`,
    /// `TradeShipExecution.ts:151`, `NationAttackBehavior.ts:1288`); the one array call site
    /// (`SpatialQuery.ts:217`, `PathFinding.Water(..).findPath(shores, ..)`) reaches neither
    /// the memo (not memoised) nor a scalar-sensitive step (`MiniMapTransformer` with a
    /// 1-element array picks that element, the same as the scalar path). So the convention
    /// is exact for every TS caller. Rust callers that need a 1-element *array* use
    /// [`Src::Many`] through the `*_src` entry points.
    #[inline]
    pub fn from_slice(s: &'a [u32]) -> Src<'a> {
        if s.len() == 1 { Src::One(s[0]) } else { Src::Many(s) }
    }
    /// `Array.isArray(from) ? from : [from]` as a slice (`buf` holds the scalar).
    #[inline]
    pub fn as_slice<'b>(&'b self, buf: &'b mut [u32; 1]) -> &'b [u32] {
        match *self {
            Src::One(t) => {
                buf[0] = t;
                &buf[..]
            }
            Src::Many(s) => s,
        }
    }
    /// The elements as an owned list (`fromArray`).
    pub fn to_vec(&self) -> Vec<u32> {
        match *self {
            Src::One(t) => vec![t],
            Src::Many(s) => s.to_vec(),
        }
    }
    pub fn is_array(&self) -> bool {
        matches!(self, Src::Many(_))
    }
}

/// `x.length === 1 ? x[0] : x` (the delegate rule of ComponentCheck/ShoreCoercing/MiniMap,
/// `ComponentCheckTransformer.ts:31-32`, `ShoreCoercingTransformer.ts:57`,
/// `MiniMapTransformer.ts:33-34`).
#[inline]
pub fn delegate_src(v: &[u32]) -> Src<'_> {
    if v.len() == 1 { Src::One(v[0]) } else { Src::Many(v) }
}
