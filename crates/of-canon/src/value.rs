//! CS v1 value tree (SPEC §1.1). A JS-model value: the engine's snapshot data with
//! references replaced by keys (§1.2). Objects keep insertion order in memory; the
//! encoder sorts keys (§1.4), so producers need not.

/// A typed array (`TYPED` tag, SPEC §1.4 kinds 0..7).
#[derive(Clone, Debug, PartialEq)]
pub enum Typed {
    I8(Vec<i8>),
    U8(Vec<u8>),
    I16(Vec<i16>),
    U16(Vec<u16>),
    I32(Vec<i32>),
    U32(Vec<u32>),
    /// Raw f32 values; NaN payloads are NOT canonicalised (SPEC §1.4 kind 6).
    F32(Vec<f32>),
    F64(Vec<f64>),
}

impl Typed {
    pub fn kind(&self) -> u8 {
        match self {
            Typed::I8(_) => 0,
            Typed::U8(_) => 1,
            Typed::I16(_) => 2,
            Typed::U16(_) => 3,
            Typed::I32(_) => 4,
            Typed::U32(_) => 5,
            Typed::F32(_) => 6,
            Typed::F64(_) => 7,
        }
    }
    pub fn len(&self) -> usize {
        match self {
            Typed::I8(v) => v.len(),
            Typed::U8(v) => v.len(),
            Typed::I16(v) => v.len(),
            Typed::U16(v) => v.len(),
            Typed::I32(v) => v.len(),
            Typed::U32(v) => v.len(),
            Typed::F32(v) => v.len(),
            Typed::F64(v) => v.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A CS value.
#[derive(Clone, Debug, PartialEq)]
pub enum Cs {
    /// JS `undefined`: omitted as an object field; `ABSENT` as an array element.
    Absent,
    Null,
    Bool(bool),
    /// CS `int` (`zInt`, `zTile`, `zRef`): must be a safe integer (|x| <= 2^53-1).
    Int(i64),
    /// CS `num` (`zNum`): encodes as INT when it is a safe integer and not -0, else F64.
    Num(f64),
    /// CS `bigint` (gold, troops, health…) on i128 (see `of_det::bigint`).
    Big(i128),
    Str(String),
    Arr(Vec<Cs>),
    /// Object fields in producer order; duplicate keys are an encoding error.
    Obj(Vec<(String, Cs)>),
    Typed(Typed),
}

pub const MAX_SAFE: i64 = 9_007_199_254_740_991;

/// SPEC §1.1 test for the `num` rule.
#[inline]
pub fn is_safe_int(x: f64) -> bool {
    x.is_finite() && x.trunc() == x && x.abs() <= 9_007_199_254_740_991.0 && !(x == 0.0 && x.is_sign_negative())
}

impl Cs {
    pub fn obj<K: Into<String>>(fields: impl IntoIterator<Item = (K, Cs)>) -> Cs {
        Cs::Obj(fields.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }
    /// Field lookup on an object (None for non-objects or missing / absent fields).
    pub fn get(&self, key: &str) -> Option<&Cs> {
        match self {
            Cs::Obj(f) => f.iter().find(|(k, _)| k == key).map(|(_, v)| v).filter(|v| !matches!(v, Cs::Absent)),
            _ => None,
        }
    }
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Cs> {
        match self {
            Cs::Obj(f) => f.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn as_arr(&self) -> Option<&Vec<Cs>> {
        if let Cs::Arr(a) = self { Some(a) } else { None }
    }
    pub fn as_str(&self) -> Option<&str> {
        if let Cs::Str(s) = self { Some(s) } else { None }
    }
    /// Numeric value of an `Int` or `Num`.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Cs::Int(i) => Some(*i as f64),
            Cs::Num(x) => Some(*x),
            _ => None,
        }
    }
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Cs::Int(i) => Some(*i),
            Cs::Num(x) if is_safe_int(*x) => Some(*x as i64),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        if let Cs::Bool(b) = self { Some(*b) } else { None }
    }
    pub fn as_typed(&self) -> Option<&Typed> {
        if let Cs::Typed(t) = self { Some(t) } else { None }
    }
    pub fn is_null(&self) -> bool {
        matches!(self, Cs::Null)
    }
}
