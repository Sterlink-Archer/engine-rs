//! Inline Fx hasher (the rustc / Firefox multiply-rotate hash), so `of-det` stays
//! dependency-free (ARCHITECTURE §2, §12 G7(b); ledger t088).
//!
//! Used ONLY for lookup indexes (`OrdMap` index, `of_engine::jsorder::LookupMap`). No
//! iteration order anywhere in the engine depends on it: `OrdMap` iterates its entry
//! vector, never its index. The hash is deterministic (no per-process random seed),
//! which `std`'s `RandomState` SipHash is not; that is not needed for determinism (hash
//! order is never observed) but it makes lookup cost and memory layout reproducible.

use std::hash::{BuildHasherDefault, Hasher};

const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

/// The Fx hasher: `h = (h.rotl(5) ^ word) * SEED` per word.
#[derive(Clone, Copy, Default, Debug)]
pub struct FxHasher {
    hash: u64,
}

impl FxHasher {
    #[inline]
    fn add_to_hash(&mut self, word: u64) {
        self.hash = (self.hash.rotate_left(5) ^ word).wrapping_mul(SEED);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        // Index loop rather than `chunks_exact(8)`: clippy ≥ 1.99 wants `as_chunks` (Rust 1.88),
        // above the workspace MSRV (1.85). Same words, same order.
        let n = bytes.len() / 8;
        for i in 0..n {
            let c = &bytes[i * 8..i * 8 + 8];
            self.add_to_hash(u64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]));
        }
        let rest = &bytes[n * 8..];
        if !rest.is_empty() {
            let mut b = [0u8; 8];
            b[..rest.len()].copy_from_slice(rest);
            self.add_to_hash(u64::from_le_bytes(b));
        }
    }
    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add_to_hash(i as u64);
    }
    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.add_to_hash(i as u64);
    }
    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.add_to_hash(i as u64);
    }
    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.add_to_hash(i);
    }
    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add_to_hash(i as u64);
    }
    #[inline]
    fn finish(&self) -> u64 {
        self.hash
    }
}

/// `BuildHasher` for [`FxHasher`]; the default hasher of `OrdMap`/`OrdSet`.
pub type FxBuildHasher = BuildHasherDefault<FxHasher>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::hash::BuildHasher;
    #[test]
    fn deterministic_and_spreads() {
        let b = FxBuildHasher::default();
        let h = |x: u32| b.hash_one(x);
        assert_eq!(h(1), h(1));
        assert_ne!(h(1), h(2));
        // Pinned value: the hash must not depend on the process (no random seed).
        assert_eq!(h(1), SEED);
    }
}
