//! `of-det`: bit-exact ports of the OpenFront engine's deterministic leaf code
//! (engine pin `5b75bd3ad8f66325a727f7bcec47680a0cc07e63`) and of the JavaScript
//! semantics the engine depends on. No dependencies; every public function names its
//! upstream `file:line`. Conformance: `cargo run -p of-vectors --release` (golden vectors
//! in `conformance/golden/vectors/`, see `conformance/docs/VECTORS.md`).
//!
//! | module | ports |
//! |---|---|
//! | [`js`] | JS numeric operators: Math.round, ToInt32/ToUint32, imul, shifts, min/max, fround, toString(36) |
//! | [`bigint`] | JS BigInt semantics on checked i128 |
//! | [`detmath`] | `engine-lib/src/DetMath.ts` |
//! | [`prng`] | `engine-lib/src/PseudoRandom.ts` (sfc32) |
//! | [`util`] | `engine-lib/src/Util.ts`, `execution/Util.ts` closestTwoTiles, DoomsdayClock, Veterancy |
//! | [`timsort`] | V8 `Array.prototype.sort` (TimSort), call-for-call |
//! | [`heap`] | `execution/utils/FlatBinaryHeap.ts` (f32 priorities) |
//! | [`config`] | config-independent EngineConfig formulas (saturation, spawn rates, samRange) |
//! | [`ordmap`] | JS `Map`/`Set` insertion order with live iteration (incl. `clear()`); `Object.keys` order; inline small variants |
//! | [`fx`] | inline Fx hasher for lookup indexes |
//! | [`throw`] | `TsThrow` panic payload, `ts_throw!`, `TsExpect` (TS-equivalent throws) |
//!
//! Additions contributed by of-engine S0 (ARCHITECTURE §12 G7, ledger t088): `clear()` keeps
//! live cursors running; Fx hasher parameter; hand-written `clone_from` (OrdMap/OrdSet,
//! FlatBinaryHeap); `SmallJsMap`/`SmallJsSet`; `TsThrow`/`ts_throw!` + strict i64 BigInt
//! conversions (`bigint::{from_number_strict, from_floor_strict, div_strict, rem_strict}`,
//! `util::to_int_strict`); the `prng-trace` feature; `first_live()`.
// The engine's numeric literals are kept verbatim (they ARE the std constants bit for bit,
// but the port mirrors the TS source); `next` mirrors PseudoRandom.next().
#![allow(clippy::approx_constant, clippy::should_implement_trait, clippy::collapsible_if)]
// of-det IS the implementation of the JS primitives the workspace clippy.toml bans
// elsewhere (f64::max/min/round, std hash containers, slice sorts, RefCell for the
// feature-gated prng-trace hook).
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod bigint;
pub mod config;
pub mod detmath;
pub mod fx;
pub mod heap;
pub mod js;
pub mod ordmap;
pub mod prng;
pub mod throw;
pub mod timsort;
pub mod util;

/// Alias module named in the port plan: everything JS-semantics related.
pub mod js_compat {
    pub use crate::bigint;
    pub use crate::js::*;
    pub use crate::ordmap::{Cursor, OrdMap, OrdSet, object_keys_order};
    pub use crate::timsort::{sort_by as js_sort_by, sort_by_key_asc as js_sort_by_key_asc};
}

/// Names used by `engine-rs/docs/ARCHITECTURE.md` (G7 / ledger t088).
pub type Prng = prng::PseudoRandom;
pub type JsMap<K, V> = ordmap::OrdMap<K, V>;
pub type JsSet<K> = ordmap::OrdSet<K>;
/// `JsMap` with up to `N` entries inline (no heap block), spilling above (ARCHITECTURE §8.2).
pub type SmallJsMap<K, V, const N: usize> = ordmap::OrdMap<K, V, fx::FxBuildHasher, N>;
/// `JsSet` with up to `N` entries inline.
pub type SmallJsSet<K, const N: usize> = ordmap::OrdSet<K, fx::FxBuildHasher, N>;
pub use ordmap::Cursor;
pub use throw::{TsErrorKind, TsExpect, TsThrow};
pub use timsort::sort_by as js_sort_by;
