//! FNV64W (SPEC §1.5): FNV-1a-64 arithmetic fed 32-bit words; the harness `H64`
//! (`harness/src/strongHash.ts:30-60`), reference `conformance/src/cse.ts` `Fnv64w`.

pub const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
pub const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fnv64w {
    pub h: u64,
}

impl Default for Fnv64w {
    fn default() -> Self {
        Self::new()
    }
}

impl Fnv64w {
    #[inline]
    pub const fn new() -> Self {
        Fnv64w { h: FNV_OFFSET }
    }
    /// `step(h, w) = (h XOR w) * P mod 2^64`.
    #[inline(always)]
    pub fn word(&mut self, w: u32) -> &mut Self {
        self.h = (self.h ^ w as u64).wrapping_mul(FNV_PRIME);
        self
    }
    /// `FNV64W(bytes)` continuation: word(len), LE words, zero-padded tail word.
    pub fn bytes(&mut self, b: &[u8]) -> &mut Self {
        assert!(b.len() <= u32::MAX as usize, "FNV64W: digested buffer must be < 2^32 bytes");
        self.word(b.len() as u32);
        let mut chunks = b.chunks_exact(4);
        let mut h = self.h;
        for c in &mut chunks {
            h = (h ^ u32::from_le_bytes([c[0], c[1], c[2], c[3]]) as u64).wrapping_mul(FNV_PRIME);
        }
        self.h = h;
        let rest = chunks.remainder();
        if !rest.is_empty() {
            let mut t = [0u8; 4];
            t[..rest.len()].copy_from_slice(rest);
            self.word(u32::from_le_bytes(t));
        }
        self
    }
    #[inline]
    pub fn hi(&self) -> u32 {
        (self.h >> 32) as u32
    }
    #[inline]
    pub fn lo(&self) -> u32 {
        self.h as u32
    }
    pub fn hex(&self) -> String {
        format!("{:016x}", self.h)
    }
}

/// `FNV64W(bytes)` from the offset basis.
pub fn fnv64w(b: &[u8]) -> u64 {
    Fnv64w::new().bytes(b).h
}

#[cfg(test)]
mod tests {
    use super::*;
    /// SPEC §1.5 vectors (Python-cross-checked, SPEC §6.4).
    #[test]
    fn spec_vectors() {
        assert_eq!(format!("{:016x}", fnv64w(b"")), "af63bd4c8601b7df");
        assert_eq!(format!("{:016x}", fnv64w(b"a")), "082f4307b4e8c4d7");
        assert_eq!(format!("{:016x}", fnv64w(b"abcd")), "69474fa4aded4ab6");
        assert_eq!(format!("{:016x}", fnv64w(b"abcde")), "b8affed3779cf3a2");
    }
}
