//! CSE1 — Canonical State Encoding v1 (SPEC §1.4), its DELTA_U32 storage form (§2.5),
//! and a strict decoder.
//!
//! | tag | name | payload |
//! |---|---|---|
//! | 00 | ABSENT | — (array elements only) |
//! | 01 | NULL | — |
//! | 02/03 | FALSE/TRUE | — |
//! | 04 | INT | uvarint(zigzag(x)), x safe integer |
//! | 05 | F64 | 8 bytes LE; NaN canonicalised to 7ff8000000000000; -0 kept |
//! | 06 | BIGINT | uvarint(zigzag(x)) arbitrary precision (here: i128) |
//! | 07 | STR | uvarint(byteLen) + UTF-8 |
//! | 08 | ARRAY | uvarint(count) + elements |
//! | 09 | OBJECT | uvarint(count) + (uvarint(keyLen) + key + value)*, keys sorted by UTF-8 bytes, absent fields omitted |
//! | 0A | TYPED | kind u8 + uvarint(count) + raw LE elements (f32 NaN payloads kept) |
//! | 0B | DELTA_U32 | storage only: uvarint(count) + uvarint(zigzag(a[i]-a[i-1])), a[-1]=0 |

use crate::value::{Cs, MAX_SAFE, Typed, is_safe_int};
use std::fmt;

pub const ABSENT: u8 = 0x00;
pub const NULL: u8 = 0x01;
pub const FALSE: u8 = 0x02;
pub const TRUE: u8 = 0x03;
pub const INT: u8 = 0x04;
pub const F64: u8 = 0x05;
pub const BIGINT: u8 = 0x06;
pub const STR: u8 = 0x07;
pub const ARRAY: u8 = 0x08;
pub const OBJECT: u8 = 0x09;
pub const TYPED: u8 = 0x0a;
pub const DELTA_U32: u8 = 0x0b;

pub const CANON_NAN: u64 = 0x7ff8_0000_0000_0000;

#[derive(Debug, Clone, PartialEq)]
pub enum CseError {
    IntNotSafe(i64),
    AbsentField(String),
    DuplicateKey(String),
    AbsentTopLevel,
    Eof(usize),
    BadTag(u8, usize),
    BadKind(u8, usize),
    VarintOverflow(usize),
    Utf8(usize),
    Trailing(usize),
    NonCanonical(&'static str, usize),
}
impl fmt::Display for CseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CSE1: {self:?}")
    }
}
impl std::error::Error for CseError {}

/// Low-level streaming writer. Callers that emit objects directly must write keys in
/// sorted UTF-8 byte order and count only present fields; [`encode`] does this for trees.
#[derive(Default, Clone)]
pub struct CseWriter {
    pub buf: Vec<u8>,
    /// Storage form: `Typed::U32` is written as DELTA_U32.
    pub storage: bool,
}

#[inline]
fn zigzag64(x: i64) -> u64 {
    ((x << 1) ^ (x >> 63)) as u64
}
#[inline]
fn zigzag128(x: i128) -> u128 {
    ((x << 1) ^ (x >> 127)) as u128
}

impl CseWriter {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_capacity(n: usize) -> Self {
        CseWriter { buf: Vec::with_capacity(n), storage: false }
    }
    #[inline]
    pub fn clear(&mut self) {
        self.buf.clear();
    }
    #[inline]
    pub fn u8(&mut self, b: u8) {
        self.buf.push(b);
    }
    #[inline]
    pub fn uv(&mut self, mut n: u64) {
        while n >= 0x80 {
            self.buf.push((n as u8) | 0x80);
            n >>= 7;
        }
        self.buf.push(n as u8);
    }
    #[inline]
    pub fn uv128(&mut self, mut n: u128) {
        while n >= 0x80 {
            self.buf.push((n as u8) | 0x80);
            n >>= 7;
        }
        self.buf.push(n as u8);
    }
    #[inline]
    pub fn null(&mut self) {
        self.u8(NULL)
    }
    #[inline]
    pub fn absent(&mut self) {
        self.u8(ABSENT)
    }
    #[inline]
    pub fn bool(&mut self, b: bool) {
        self.u8(if b { TRUE } else { FALSE })
    }
    /// INT (caller guarantees |x| <= 2^53-1; checked in debug).
    #[inline]
    pub fn int(&mut self, x: i64) {
        debug_assert!(x.abs() <= MAX_SAFE, "CSE1 INT not safe: {x}");
        self.u8(INT);
        self.uv(zigzag64(x));
    }
    /// A JS `num`: INT if safe integer (and not -0), else F64.
    #[inline]
    pub fn num(&mut self, x: f64) {
        if is_safe_int(x) {
            self.int(x as i64);
        } else {
            self.u8(F64);
            let bits = if x.is_nan() { CANON_NAN } else { x.to_bits() };
            self.buf.extend_from_slice(&bits.to_le_bytes());
        }
    }
    #[inline]
    pub fn big(&mut self, x: i128) {
        self.u8(BIGINT);
        self.uv128(zigzag128(x));
    }
    #[inline]
    pub fn str(&mut self, s: &str) {
        self.u8(STR);
        self.key(s);
    }
    /// Key / string payload without the tag.
    #[inline]
    pub fn key(&mut self, s: &str) {
        self.uv(s.len() as u64);
        self.buf.extend_from_slice(s.as_bytes());
    }
    #[inline]
    pub fn arr_begin(&mut self, n: usize) {
        self.u8(ARRAY);
        self.uv(n as u64);
    }
    #[inline]
    pub fn obj_begin(&mut self, n: usize) {
        self.u8(OBJECT);
        self.uv(n as u64);
    }
    pub fn typed(&mut self, t: &Typed) {
        if self.storage {
            if let Typed::U32(v) = t {
                return self.delta_u32(v);
            }
        }
        self.u8(TYPED);
        self.u8(t.kind());
        self.uv(t.len() as u64);
        match t {
            Typed::I8(v) => self.buf.extend(v.iter().map(|x| *x as u8)),
            Typed::U8(v) => self.buf.extend_from_slice(v),
            Typed::I16(v) => v.iter().for_each(|x| self.buf.extend_from_slice(&x.to_le_bytes())),
            Typed::U16(v) => v.iter().for_each(|x| self.buf.extend_from_slice(&x.to_le_bytes())),
            Typed::I32(v) => v.iter().for_each(|x| self.buf.extend_from_slice(&x.to_le_bytes())),
            Typed::U32(v) => v.iter().for_each(|x| self.buf.extend_from_slice(&x.to_le_bytes())),
            Typed::F32(v) => v.iter().for_each(|x| self.buf.extend_from_slice(&x.to_bits().to_le_bytes())),
            Typed::F64(v) => v.iter().for_each(|x| self.buf.extend_from_slice(&x.to_bits().to_le_bytes())),
        }
    }
    /// U32 typed array as plain TYPED kind 5 (digest form), from a slice.
    pub fn u32s(&mut self, v: &[u32]) {
        if self.storage {
            return self.delta_u32(v);
        }
        self.u8(TYPED);
        self.u8(5);
        self.uv(v.len() as u64);
        for x in v {
            self.buf.extend_from_slice(&x.to_le_bytes());
        }
    }
    pub fn delta_u32(&mut self, v: &[u32]) {
        self.u8(DELTA_U32);
        self.uv(v.len() as u64);
        let mut prev: i64 = 0;
        for &x in v {
            let d = x as i64 - prev;
            prev = x as i64;
            self.uv(zigzag64(d));
        }
    }

    /// Encode a value tree.
    pub fn value(&mut self, v: &Cs) -> Result<(), CseError> {
        match v {
            Cs::Absent => self.absent(),
            Cs::Null => self.null(),
            Cs::Bool(b) => self.bool(*b),
            Cs::Int(i) => {
                if i.abs() > MAX_SAFE {
                    return Err(CseError::IntNotSafe(*i));
                }
                self.int(*i)
            }
            Cs::Num(x) => self.num(*x),
            Cs::Big(b) => self.big(*b),
            Cs::Str(s) => self.str(s),
            Cs::Arr(a) => {
                self.arr_begin(a.len());
                for e in a {
                    self.value(e)?;
                }
            }
            Cs::Obj(fields) => {
                let mut present: Vec<&(String, Cs)> = fields.iter().filter(|(_, v)| !matches!(v, Cs::Absent)).collect();
                // str Ord is lexicographic on UTF-8 bytes, shorter first on a common prefix.
                present.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
                for w in present.windows(2) {
                    if w[0].0 == w[1].0 {
                        return Err(CseError::DuplicateKey(w[0].0.clone()));
                    }
                }
                self.obj_begin(present.len());
                for (k, v) in present {
                    self.key(k);
                    self.value(v)?;
                }
            }
            Cs::Typed(t) => self.typed(t),
        }
        Ok(())
    }
}

/// CSE1 of a value (digest form).
pub fn encode(v: &Cs) -> Result<Vec<u8>, CseError> {
    let mut w = CseWriter::new();
    w.value(v)?;
    Ok(w.buf)
}

/// Storage form (dump files): CSE1 with `Typed::U32` as DELTA_U32 (SPEC §2.5).
pub fn encode_storage(v: &Cs) -> Result<Vec<u8>, CseError> {
    let mut w = CseWriter { buf: Vec::new(), storage: true };
    w.value(v)?;
    Ok(w.buf)
}

// ------------------------------------------------------------------ decoder

/// Strict decoder. Accepts DELTA_U32 (-> `Typed::U32`). Rejects: unknown tags/kinds,
/// trailing bytes, an F64 holding a safe integer (non-canonical), non-canonical NaN in F64,
/// unsorted or duplicate object keys, ABSENT outside arrays.
pub struct Decoder<'a> {
    b: &'a [u8],
    pub pos: usize,
}

impl<'a> Decoder<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        Decoder { b, pos: 0 }
    }
    #[inline]
    fn byte(&mut self) -> Result<u8, CseError> {
        let x = *self.b.get(self.pos).ok_or(CseError::Eof(self.pos))?;
        self.pos += 1;
        Ok(x)
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], CseError> {
        if self.pos + n > self.b.len() {
            return Err(CseError::Eof(self.pos));
        }
        let s = &self.b[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    pub fn uv(&mut self) -> Result<u64, CseError> {
        let start = self.pos;
        let mut x: u64 = 0;
        let mut shift = 0u32;
        loop {
            let b = self.byte()?;
            if shift >= 64 || (shift == 63 && (b & 0x7f) > 1) {
                return Err(CseError::VarintOverflow(start));
            }
            x |= ((b & 0x7f) as u64) << shift;
            if b & 0x80 == 0 {
                return Ok(x);
            }
            shift += 7;
        }
    }
    fn uv128(&mut self) -> Result<u128, CseError> {
        let start = self.pos;
        let mut x: u128 = 0;
        let mut shift = 0u32;
        loop {
            let b = self.byte()?;
            if shift >= 128 || (shift == 126 && (b & 0x7f) > 3) {
                return Err(CseError::VarintOverflow(start));
            }
            x |= ((b & 0x7f) as u128) << shift;
            if b & 0x80 == 0 {
                return Ok(x);
            }
            shift += 7;
        }
    }
    fn string(&mut self) -> Result<String, CseError> {
        let n = self.uv()? as usize;
        let at = self.pos;
        let s = self.take(n)?;
        String::from_utf8(s.to_vec()).map_err(|_| CseError::Utf8(at))
    }

    pub fn value(&mut self, in_array: bool) -> Result<Cs, CseError> {
        let at = self.pos;
        let tag = self.byte()?;
        Ok(match tag {
            ABSENT => {
                if !in_array {
                    return Err(CseError::NonCanonical("ABSENT outside array", at));
                }
                Cs::Absent
            }
            NULL => Cs::Null,
            FALSE => Cs::Bool(false),
            TRUE => Cs::Bool(true),
            INT => {
                let z = self.uv()?;
                let x = ((z >> 1) as i64) ^ -((z & 1) as i64);
                if x.abs() > MAX_SAFE {
                    return Err(CseError::NonCanonical("INT beyond 2^53-1", at));
                }
                Cs::Int(x)
            }
            F64 => {
                let bits = u64::from_le_bytes(self.take(8)?.try_into().unwrap());
                let x = f64::from_bits(bits);
                if x.is_nan() && bits != CANON_NAN {
                    return Err(CseError::NonCanonical("NaN payload", at));
                }
                if is_safe_int(x) {
                    return Err(CseError::NonCanonical("F64 holding a safe integer", at));
                }
                Cs::Num(x)
            }
            BIGINT => {
                let z = self.uv128()?;
                Cs::Big(((z >> 1) as i128) ^ -((z & 1) as i128))
            }
            STR => Cs::Str(self.string()?),
            ARRAY => {
                let n = self.uv()? as usize;
                let mut v = Vec::with_capacity(n.min(1 << 20));
                for _ in 0..n {
                    v.push(self.value(true)?);
                }
                Cs::Arr(v)
            }
            OBJECT => {
                let n = self.uv()? as usize;
                let mut f: Vec<(String, Cs)> = Vec::with_capacity(n.min(1 << 16));
                for _ in 0..n {
                    let kat = self.pos;
                    let k = self.string()?;
                    if let Some((prev, _)) = f.last() {
                        if prev.as_bytes() >= k.as_bytes() {
                            return Err(CseError::NonCanonical("object keys not strictly sorted", kat));
                        }
                    }
                    let v = self.value(false)?;
                    f.push((k, v));
                }
                Cs::Obj(f)
            }
            TYPED => {
                let kind = self.byte()?;
                let n = self.uv()? as usize;
                let el = match kind {
                    0 | 1 => 1,
                    2 | 3 => 2,
                    4..=6 => 4,
                    7 => 8,
                    k => return Err(CseError::BadKind(k, at)),
                };
                let raw = self.take(n.checked_mul(el).ok_or(CseError::Eof(at))?)?;
                Cs::Typed(match kind {
                    0 => Typed::I8(raw.iter().map(|x| *x as i8).collect()),
                    1 => Typed::U8(raw.to_vec()),
                    2 => Typed::I16(raw.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect()),
                    3 => Typed::U16(raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect()),
                    4 => Typed::I32(raw.chunks_exact(4).map(|c| i32::from_le_bytes(c.try_into().unwrap())).collect()),
                    5 => Typed::U32(raw.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect()),
                    6 => Typed::F32(
                        raw.chunks_exact(4)
                            .map(|c| f32::from_bits(u32::from_le_bytes(c.try_into().unwrap())))
                            .collect(),
                    ),
                    _ => Typed::F64(
                        raw.chunks_exact(8)
                            .map(|c| f64::from_bits(u64::from_le_bytes(c.try_into().unwrap())))
                            .collect(),
                    ),
                })
            }
            DELTA_U32 => {
                let n = self.uv()? as usize;
                let mut v = Vec::with_capacity(n.min(1 << 24));
                let mut prev: i64 = 0;
                for _ in 0..n {
                    let z = self.uv()?;
                    let d = ((z >> 1) as i64) ^ -((z & 1) as i64);
                    prev += d;
                    if !(0..=u32::MAX as i64).contains(&prev) {
                        return Err(CseError::NonCanonical("DELTA_U32 out of u32 range", at));
                    }
                    v.push(prev as u32);
                }
                Cs::Typed(Typed::U32(v))
            }
            t => return Err(CseError::BadTag(t, at)),
        })
    }
}

/// Decode exactly one value spanning all of `b`.
pub fn decode(b: &[u8]) -> Result<Cs, CseError> {
    let mut d = Decoder::new(b);
    let v = d.value(false)?;
    if d.pos != b.len() {
        return Err(CseError::Trailing(d.pos));
    }
    Ok(v)
}

/// Hex helper.
pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
