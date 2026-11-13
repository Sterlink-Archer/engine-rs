//! `of-engine`: the bit-exact Rust port of the OpenFront engine at `upstream/` @ `5b75bd3`.
//!
//! Design: `docs/ARCHITECTURE.md`; translation rules: `docs/PORTING_GUIDE.md`; module
//! ownership and the S0/S1/S2 plan: `docs/FILE_MAP.md`. This crate never re-implements
//! what the conformance crates own: deterministic math, the PRNG, JS containers, TimSort
//! and `TsThrow` come from `of-det` (re-exported as [`of_det`]).
//!
//! Stage S0 steps 1-4 (this state of the crate): the foundations ([`ids`], [`arena`],
//! [`ctx`], [`jsarr`], [`tileset`], [`jsorder`], [`jsval`], [`scratch`], [`plain`] +
//! `#[derive(StateClone)]`, [`api`]); every state struct with all its fields; all 44
//! execution structs in [`exec::ExecArenas`]; and every cross-package signature with a
//! `todo!()` body in the file its owning package fills (FILE_MAP §1, §2.1, §3).
//!
//! Owner: **P1** (crate root; FILE_MAP §2).

// Lets `#[derive(StateClone)]`'s `::of_engine::…` paths resolve inside this crate too.
extern crate self as of_engine;

pub mod api;
pub mod arena;
pub mod bot;
pub mod canon;
pub mod config;
pub mod ctx;
pub mod exec;
pub mod ids;
pub mod jsarr;
pub mod jsorder;
pub mod jsval;
pub mod map;
pub mod path;
pub mod plain;
#[cfg(feature = "prof")]
pub mod prof;
pub mod rail;
pub mod runner;
pub mod scratch;
pub mod setup;
pub mod state;
mod testkit;
pub mod tileset;
pub mod util;
pub mod water;

pub use of_det;
pub use of_engine_derive::StateClone;
pub use runner::{Fatal, Game, StepEvents};

// ARCHITECTURE §10: many independent games per thread/core; a `Game` must be `Send`.
const _: fn() = || {
    fn assert_send<T: Send + 'static>() {}
    assert_send::<Game>();
};
