//! `of-cli` library: the of-trace [`Engine`](of_trace::engine::Engine) adapter for of-engine
//! ([`engine::OfEngine`]), used by `engine-rs conform` (SPEC §3.4), plus the resume loop
//! ([`resume`], SPEC §3.2 / R3), `bench` / `fork-bench` ([`bench`], ARCHITECTURE §12 G1/G2),
//! the counting allocator ([`alloc`]) and the PRNG draw trace ([`prngtrace`], §4.5).
//! Owner: **P13**.

pub mod alloc;
pub mod bench;
pub mod engine;
pub mod prngtrace;
pub mod resume;
pub mod sampler;
