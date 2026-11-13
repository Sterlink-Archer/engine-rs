//! Sections, section digests, D_t and K_t (SPEC §1.3, §1.5).
//!
//! ```text
//! D_t = FNV64W h0 -> step(0x4f464353 "OFCS") -> step(1) -> step(t)
//!       -> for X in G T M P U A D R W E S: step(hi32(d_X)), step(lo32(d_X))
//! d_X = FNV64W(CSE1(X))
//! K_t = same with magic 0x4f46434b "OFCK" over the K projection
//! ```

use crate::cse::{CseError, CseWriter};
use crate::fnv::Fnv64w;
use crate::value::{Cs, Typed};

pub const CS_VERSION: u32 = 1;
pub const MAGIC_FULL: u32 = 0x4f46_4353; // "OFCS"
pub const MAGIC_KEY: u32 = 0x4f46_434b; // "OFCK"

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum Section {
    G = 0,
    T,
    M,
    P,
    U,
    A,
    D,
    R,
    W,
    E,
    S,
}

/// Fixed section order (SPEC §1.3).
pub const SECTIONS: [Section; 11] = [
    Section::G,
    Section::T,
    Section::M,
    Section::P,
    Section::U,
    Section::A,
    Section::D,
    Section::R,
    Section::W,
    Section::E,
    Section::S,
];

impl Section {
    pub fn name(self) -> &'static str {
        ["G", "T", "M", "P", "U", "A", "D", "R", "W", "E", "S"][self as usize]
    }
    pub fn from_name(s: &str) -> Option<Section> {
        SECTIONS.iter().copied().find(|x| x.name() == s)
    }
    /// Divergence-report class (SPEC §1.3 "Classes"): O, I, B, X.
    pub fn class(self) -> &'static str {
        match self {
            Section::G => "O+B",
            Section::T | Section::M | Section::P | Section::U | Section::A | Section::D => "O",
            Section::R => "O+I",
            Section::W => "I",
            Section::E => "O+I",
            Section::S => "X",
        }
    }
}

/// A CS v1 root `{tick, G, T, M, P, U, A, D, R, W, E, S}`.
#[derive(Clone, Debug, PartialEq)]
pub struct CanonState {
    pub tick: u32,
    pub sec: [Cs; 11],
}

impl CanonState {
    pub fn get(&self, s: Section) -> &Cs {
        &self.sec[s as usize]
    }
    pub fn get_mut(&mut self, s: Section) -> &mut Cs {
        &mut self.sec[s as usize]
    }
    /// From a root object value (e.g. a decoded dump, with T possibly missing -> Absent).
    pub fn from_root(root: &Cs) -> Option<CanonState> {
        let tick = root.get("tick")?.as_i64()? as u32;
        let sec = SECTIONS.map(|s| root.get(s.name()).cloned().unwrap_or(Cs::Absent));
        Some(CanonState { tick, sec })
    }
    /// The root object (sections + tick).
    pub fn to_root(&self, omit_t: bool) -> Cs {
        let mut f: Vec<(String, Cs)> = vec![("tick".into(), Cs::Int(self.tick as i64))];
        for s in SECTIONS {
            if omit_t && s == Section::T {
                continue;
            }
            f.push((s.name().into(), self.sec[s as usize].clone()));
        }
        Cs::Obj(f)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Digests {
    pub tick: u32,
    /// Combined digest (D_t or K_t).
    pub all: u64,
    /// Section digests in SECTIONS order.
    pub sec: [u64; 11],
}

impl Digests {
    pub fn hex(&self) -> String {
        format!("{:016x}", self.all)
    }
}

/// Combine section digests into D_t / K_t.
pub fn combine(magic: u32, tick: u32, sec: &[u64; 11]) -> u64 {
    let mut h = Fnv64w::new();
    h.word(magic).word(CS_VERSION).word(tick);
    for d in sec {
        h.word((*d >> 32) as u32).word(*d as u32);
    }
    h.h
}

/// Section digests of a CS (an absent section — impossible in a well-formed CS — encodes
/// as ABSENT and is an error here).
pub fn section_digests(cs: &CanonState, w: &mut CseWriter) -> Result<[u64; 11], CseError> {
    let mut out = [0u64; 11];
    for s in SECTIONS {
        w.clear();
        let v = &cs.sec[s as usize];
        if matches!(v, Cs::Absent) {
            return Err(CseError::AbsentTopLevel);
        }
        w.value(v)?;
        out[s as usize] = Fnv64w::new().bytes(&w.buf).h;
    }
    Ok(out)
}

/// D_t of a full CS.
pub fn digest_full(cs: &CanonState) -> Result<Digests, CseError> {
    let mut w = CseWriter::with_capacity(1 << 20);
    let sec = section_digests(cs, &mut w)?;
    Ok(Digests { tick: cs.tick, all: combine(MAGIC_FULL, cs.tick, &sec), sec })
}

/// K projection (SPEC §1.5 "Key digest"): T -> {map}, S -> null, every TYPED elsewhere -> INT(len).
pub fn key_projection(cs: &CanonState) -> CanonState {
    let mut k = cs.clone();
    for s in SECTIONS {
        let v = &mut k.sec[s as usize];
        match s {
            Section::T => {
                let map = cs.get(Section::T).get("map").cloned().unwrap_or(Cs::Absent);
                *v = Cs::Obj(vec![("map".into(), map)]);
            }
            Section::S => *v = Cs::Null,
            _ => typed_to_count(v),
        }
    }
    k
}

fn typed_to_count(v: &mut Cs) {
    match v {
        Cs::Typed(t) => *v = Cs::Int(t.len() as i64),
        Cs::Arr(a) => a.iter_mut().for_each(typed_to_count),
        Cs::Obj(f) => f.iter_mut().for_each(|(_, x)| typed_to_count(x)),
        _ => {}
    }
}

/// K_t of a full CS (applies [`key_projection`]).
pub fn digest_key(cs: &CanonState) -> Result<Digests, CseError> {
    let k = key_projection(cs);
    let mut w = CseWriter::with_capacity(1 << 20);
    let sec = section_digests(&k, &mut w)?;
    Ok(Digests { tick: cs.tick, all: combine(MAGIC_KEY, cs.tick, &sec), sec })
}

/// Row digest (SPEC §1.5 "Row digests"): FNV64W(CSE1(row)).
pub fn row_digest(row: &Cs) -> Result<u64, CseError> {
    let mut w = CseWriter::new();
    w.value(row)?;
    Ok(Fnv64w::new().bytes(&w.buf).h)
}

/// Element count helper for typed values (used by localisation reports).
pub fn typed_len(t: &Typed) -> usize {
    t.len()
}
