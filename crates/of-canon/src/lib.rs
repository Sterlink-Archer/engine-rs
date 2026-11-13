//! `of-canon`: CANONICAL STATE v1 per `conformance/docs/SPEC.md` §1 (draft v1.0, 2026-11-05).
//!
//! | SPEC | here |
//! |---|---|
//! | §1.1 numeric model | [`value::Cs`] (`Int` i64 / `Num` f64 / `Big` i128 / …), [`value::is_safe_int`] |
//! | §1.3 sections G T M P U A D R W E S | [`digest::Section`], [`digest::CanonState`] |
//! | §1.4 CSE1 byte encoding (+ DELTA_U32 storage form) | [`cse::encode`], [`cse::encode_storage`], [`cse::decode`], [`cse::CseWriter`] |
//! | §1.5 FNV64W, section digests, D_t, K_t | [`fnv::Fnv64w`], [`digest::digest_full`], [`digest::digest_key`], [`digest::key_projection`] |
//! | §2.5 dumps (T omitted, rebuilt on load) | [`tilestate::rebuild_t`] |
//!
//! NOT here yet (SPEC §1.2, §1.6, appendix `conformance/schema/snapshot_schemas_5b75bd3.txt`):
//! typed Rust structs for the 65 snapshot record types and the extraction of a CS tree
//! from native engine state. Those belong to the engine port (t065): an engine builds a
//! [`value::Cs`] per section (or streams it with [`cse::CseWriter`]) and hands it to
//! [`digest`]. TODO(SPEC §1.2/§1.6): generate the record structs from the schema dump.

pub mod cse;
pub mod digest;
pub mod fnv;
pub mod tilestate;
pub mod value;

pub use digest::{CanonState, Digests, SECTIONS, Section};
pub use value::{Cs, Typed};
/// Name used by `engine-rs/docs/ARCHITECTURE.md`.
pub type CsValue = value::Cs;
