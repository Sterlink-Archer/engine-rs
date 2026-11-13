//! `PlainState`: the marker every state type carries (ARCHITECTURE §8.1 item 2).
//!
//! `#[derive(StateClone)]` implements it for the deriving type and asserts at compile time
//! that every field type implements it too. It is implemented here for plain data only:
//! primitives, `String`, `Vec`/`VecDeque`/arrays/tuples/`Option` of plain data, owned
//! slices (`Box<[T]>`, `Box<str>`), `Arc` of plain data (immutable or COW-shared bulk data,
//! §8.1 item 4), `OnceLock` (the deterministic write-once segment cache values, §3.8) and
//! the `of-det` containers.
//!
//! Deliberately **not** implemented — so a state struct containing one does not compile:
//! `Rc`, `RefCell`, `Cell`, `Box<T>` for a sized `T`, `Box<dyn Trait>`, `std` `HashMap` /
//! `HashSet` (use `jsorder::LookupMap`/`LookupSet`, or `JsMap`/`JsSet` when iterated).
//!
//! ```compile_fail
//! use of_engine::StateClone;
//! #[derive(StateClone)]
//! struct Bad { x: std::cell::RefCell<u32> }
//! ```
//! ```compile_fail
//! use of_engine::StateClone;
//! #[derive(StateClone)]
//! struct Bad { x: std::rc::Rc<u32> }
//! ```
//! ```compile_fail
//! use of_engine::StateClone;
//! #[derive(StateClone)]
//! struct Bad { x: Box<u32> }
//! ```
//! ```compile_fail
//! use of_engine::StateClone;
//! #[derive(StateClone)]
//! struct Bad { x: std::collections::HashMap<u32, u32> }
//! ```
//! ```
//! use of_engine::StateClone;
//! #[derive(StateClone)]
//! struct Good { a: Vec<u32>, b: Option<std::sync::Arc<[u8]>>, c: of_engine::jsorder::JsMap<u32, f64> }
//! ```
//!
//! Owner: **P1** (FILE_MAP §2 foundations).

use std::collections::VecDeque;
use std::sync::{Arc, OnceLock};

/// Marker for plain, deterministic, deep-clonable state data. Implement it by hand only
/// for a type whose `Clone` is a deep copy with no shared mutable state; the P13 reviewer
/// greps manual impls.
pub trait PlainState {}

/// `impl PlainState` for `Copy` value types that hold **no handle** (fieldless enums, plain
/// ids, coordinates); it also implements the no-op [`crate::canon::Refs`]. Handle types
/// (`UnitRef`, `ExecId`, …) implement both traits by hand (`ids.rs`, `canon/refs.rs`).
#[macro_export]
macro_rules! plain_copy {
    ($($t:ty),* $(,)?) => { $(impl $crate::plain::PlainState for $t {})* $crate::refs_leaf!($($t),*); };
}

plain_copy!(bool, char, u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize, f32, f64, ());

impl PlainState for String {}
impl PlainState for str {}
impl<T: PlainState> PlainState for [T] {}
impl<T: PlainState, const N: usize> PlainState for [T; N] {}
impl<T: PlainState> PlainState for Option<T> {}
impl<T: PlainState> PlainState for Vec<T> {}
impl<T: PlainState> PlainState for VecDeque<T> {}
impl<T: PlainState> PlainState for Box<[T]> {}
impl PlainState for Box<str> {}
impl<T: ?Sized + PlainState> PlainState for Arc<T> {}
impl<T: PlainState> PlainState for OnceLock<T> {}
impl<A: PlainState, B: PlainState> PlainState for (A, B) {}
impl<A: PlainState, B: PlainState, C: PlainState> PlainState for (A, B, C) {}
impl<A: PlainState, B: PlainState, C: PlainState, D: PlainState> PlainState for (A, B, C, D) {}

// of-det containers and values (their Clone is a deep copy; OrdMap/OrdSet/FlatBinaryHeap
// have hand-written clone_from, ledger t088).
impl<K: PlainState, V: PlainState, S, const N: usize> PlainState for of_det::ordmap::OrdMap<K, V, S, N> {}
impl<K: PlainState, S, const N: usize> PlainState for of_det::ordmap::OrdSet<K, S, N> {}
impl PlainState for of_det::ordmap::Cursor {}
impl PlainState for of_det::Prng {}
impl PlainState for of_det::heap::FlatBinaryHeap {}
