//! `of-trace`: read conformance traces (SPEC §2), drive an [`engine::Engine`] through
//! them, and report the first divergence (SPEC §3).
//!
//! - [`trace`]: `scenario.json`, `turns.cse.zst`, `digests.k.bin`, `digests.sec.bin`,
//!   `dumps/t<tick>.cs.zst`, `SHA256SUMS`.
//! - [`engine`]: the `Engine` trait a native port implements, plus [`engine::ReplayEngine`]
//!   (replays the TS-recorded digests against themselves — the pipeline smoke test).
//! - [`conform`]: the per-tick K / per-checkpoint D comparison loop and report.
//! - [`localise`]: section → row → field diff of two canonical states (SPEC §3.2 steps 4-5).
//! - [`dumpcheck`]: native re-digest of every dump (decode → rebuild T → D, K) against the
//!   TS digests (SPEC §2.9 R4, done natively) and the storage-form round trip (MOD-SNAPSHOT).

pub mod conform;
pub mod dumpcheck;
pub mod engine;
pub mod localise;
pub mod trace;
