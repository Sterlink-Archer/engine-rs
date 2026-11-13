//! `Canon`/`Uncanon` for the framework's vocabulary: scalars, containers, handles (each
//! becomes its SPEC §1.2 key), the string enums of `api::types`, and the small shared
//! records (`PlayerInfo`, `Nation`, `Cell`, the PRNG state, the `FlatBinaryHeap` layout,
//! `OrderedTileSet`). Owner: **P13** (framework). Owners build their records from these.
//!
//! Container shapes follow how the TS snapshots spell them:
//! - an array (`Vec`, `JsArr`, `VecDeque`, slices) is an ARRAY in list order;
//! - a `Map` is `[...map]`: an ARRAY of `[key, value]` pairs in insertion order
//!   (`relations`, `outgoingQuickChats`, `nationMirvTargets`, …); a `Set` is `[...set]`;
//! - a field that the TS snapshot spells differently (`embargoes: [...values()]`, a `Set` as
//!   `tiles`) is the owner's to write by hand.

use super::{Big, Canon, CanonCx, CanonError, Cs, Typed, Uncanon, UncanonCx, arr, field, int, str_, tiles_of};
use crate::api::types::{Cell, Nation, PlayerInfo};
use crate::exec::WarheadRef;
use crate::ids::{
    AllianceId, AttackId, AttackRef, ClusterRef, ExecId, RailroadId, RailroadRef, ReqIdx, SmallId, StationId, UnitKey,
    UnitRef,
};
use crate::jsarr::JsArr;
use crate::jsval::Tri;
use crate::plain::PlainState;
use crate::state::alliance_request::RequestStatus;
use crate::tileset::OrderedTileSet;
use of_det::Prng;
use of_det::heap::{FlatBinaryHeap, HeapState};
use of_det::ordmap::{OrdMap, OrdSet};
use std::collections::VecDeque;
use std::hash::{BuildHasher, Hash};
use std::sync::Arc;

// ---- integers (`zInt`, `zTile`, `uint`) ----
macro_rules! canon_int {
    ($($t:ty),*) => {$(
        impl Canon for $t {
            #[inline]
            fn canon(&self, _cx: &mut CanonCx<'_>) -> Cs { Cs::Int(i64::from(*self)) }
        }
        impl Uncanon for $t {
            fn uncanon(v: &Cs, _cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
                v.as_i64()
                    .and_then(|x| <$t>::try_from(x).ok())
                    .ok_or_else(|| CanonError::new(format!("expected {} int, got {}", stringify!($t), super::short(v))))
            }
        }
    )*};
}
canon_int!(u8, u16, u32, i8, i16, i32);

impl Canon for bool {
    #[inline]
    fn canon(&self, _cx: &mut CanonCx<'_>) -> Cs {
        Cs::Bool(*self)
    }
}
impl Uncanon for bool {
    fn uncanon(v: &Cs, _cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        v.as_bool().ok_or_else(|| CanonError::new(format!("expected bool, got {}", super::short(v))))
    }
}
/// `zNum`: of-canon writes a safe integer as INT (SPEC §1.1).
impl Canon for f64 {
    #[inline]
    fn canon(&self, _cx: &mut CanonCx<'_>) -> Cs {
        Cs::Num(*self)
    }
}
impl Uncanon for f64 {
    fn uncanon(v: &Cs, _cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        v.as_f64().ok_or_else(|| CanonError::new(format!("expected num, got {}", super::short(v))))
    }
}
/// A BigInt value.
impl Canon for Big {
    #[inline]
    fn canon(&self, _cx: &mut CanonCx<'_>) -> Cs {
        super::big(self.0)
    }
}
impl Uncanon for Big {
    fn uncanon(v: &Cs, _cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        super::big_of(v).map(Big)
    }
}

// ---- strings ----
impl Canon for String {
    fn canon(&self, _cx: &mut CanonCx<'_>) -> Cs {
        Cs::Str(self.clone())
    }
}
impl Uncanon for String {
    fn uncanon(v: &Cs, _cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        str_(v).map(str::to_owned)
    }
}
impl Canon for str {
    fn canon(&self, _cx: &mut CanonCx<'_>) -> Cs {
        Cs::Str(self.to_owned())
    }
}
impl Canon for Arc<str> {
    fn canon(&self, _cx: &mut CanonCx<'_>) -> Cs {
        Cs::Str(self.to_string())
    }
}
impl Uncanon for Arc<str> {
    fn uncanon(v: &Cs, _cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        str_(v).map(Arc::from)
    }
}
impl Uncanon for Box<str> {
    fn uncanon(v: &Cs, _cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        str_(v).map(Box::from)
    }
}

// ---- optionals ----
/// `T | null` (`?null` in the dump).
impl<T: Canon> Canon for Option<T> {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        match self {
            Some(x) => x.canon(cx),
            None => Cs::Null,
        }
    }
}
impl<T: Uncanon> Uncanon for Option<T> {
    fn uncanon(v: &Cs, cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        if v.is_null() { Ok(None) } else { T::uncanon(v, cx).map(Some) }
    }
}
/// `T | null | undefined` (`?null?undef`); load with [`super::load_tri`] (an absent field
/// is not a value).
impl<T: Canon> Canon for Tri<T> {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        match self {
            Tri::Absent => Cs::Absent,
            Tri::Null => Cs::Null,
            Tri::Some(x) => x.canon(cx),
        }
    }
}
impl<T: Canon + ?Sized> Canon for &T {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        (**self).canon(cx)
    }
}
impl<T: Canon + ?Sized> Canon for Box<T> {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        (**self).canon(cx)
    }
}

// ---- lists ----
impl<T: Canon> Canon for [T] {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        Cs::Arr(self.iter().map(|x| x.canon(cx)).collect())
    }
}
impl<T: Canon, const N: usize> Canon for [T; N] {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        self.as_slice().canon(cx)
    }
}
impl<T: Canon> Canon for Vec<T> {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        self.as_slice().canon(cx)
    }
}
impl<T: Uncanon> Uncanon for Vec<T> {
    fn uncanon(v: &Cs, cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        arr(v)?.iter().enumerate().map(|(i, x)| T::uncanon(x, cx).map_err(|e| e.at(format!("[{i}]")))).collect()
    }
}
impl<T: Canon> Canon for Arc<[T]> {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        (**self).canon(cx)
    }
}
impl<T: Uncanon> Uncanon for Arc<[T]> {
    fn uncanon(v: &Cs, cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        Vec::<T>::uncanon(v, cx).map(Arc::from)
    }
}
impl<T: Canon> Canon for VecDeque<T> {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        Cs::Arr(self.iter().map(|x| x.canon(cx)).collect())
    }
}
impl<T: Uncanon> Uncanon for VecDeque<T> {
    fn uncanon(v: &Cs, cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        Vec::<T>::uncanon(v, cx).map(VecDeque::from)
    }
}
/// A filter-reassigned array field: the current array object, in order.
impl<T: Canon + Copy + PartialEq + PlainState> Canon for JsArr<T> {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        self.as_slice().canon(cx)
    }
}
impl<T: Uncanon + Copy + PartialEq + PlainState> Uncanon for JsArr<T> {
    fn uncanon(v: &Cs, cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        let mut a = JsArr::new();
        for x in Vec::<T>::uncanon(v, cx)? {
            a.push(x);
        }
        Ok(a)
    }
}

// ---- tuples (`(A, B)` in the dump) ----
impl<A: Canon, B: Canon> Canon for (A, B) {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        Cs::Arr(vec![self.0.canon(cx), self.1.canon(cx)])
    }
}
impl<A: Uncanon, B: Uncanon> Uncanon for (A, B) {
    fn uncanon(v: &Cs, cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        match arr(v)? {
            [a, b] => Ok((A::uncanon(a, cx)?, B::uncanon(b, cx)?)),
            a => Err(CanonError::new(format!("expected a pair, got {} elements", a.len()))),
        }
    }
}
impl<A: Canon, B: Canon, C: Canon> Canon for (A, B, C) {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        Cs::Arr(vec![self.0.canon(cx), self.1.canon(cx), self.2.canon(cx)])
    }
}
impl<A: Uncanon, B: Uncanon, C: Uncanon> Uncanon for (A, B, C) {
    fn uncanon(v: &Cs, cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        match arr(v)? {
            [a, b, c] => Ok((A::uncanon(a, cx)?, B::uncanon(b, cx)?, C::uncanon(c, cx)?)),
            a => Err(CanonError::new(format!("expected a triple, got {} elements", a.len()))),
        }
    }
}

// ---- JS Map / Set (`[...map]` / `[...set]`, insertion order) ----
impl<K, V, S, const N: usize> Canon for OrdMap<K, V, S, N>
where
    K: Canon + Hash + Eq + Clone,
    V: Canon,
    S: BuildHasher + Default,
{
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        Cs::Arr(self.iter().map(|(k, v)| Cs::Arr(vec![k.canon(cx), v.canon(cx)])).collect())
    }
}
impl<K, V, S, const N: usize> Uncanon for OrdMap<K, V, S, N>
where
    K: Uncanon + Hash + Eq + Clone,
    V: Uncanon,
    S: BuildHasher + Default,
{
    fn uncanon(v: &Cs, cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        let mut m = OrdMap::new();
        for e in arr(v)? {
            let (k, x) = <(K, V)>::uncanon(e, cx)?;
            m.insert(k, x); // TS: `new Map(entries)`: a repeated key keeps its first position
        }
        Ok(m)
    }
}
impl<K, S, const N: usize> Canon for OrdSet<K, S, N>
where
    K: Canon + Hash + Eq + Clone,
    S: BuildHasher + Default,
{
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        Cs::Arr(self.iter().map(|k| k.canon(cx)).collect())
    }
}
impl<K, S, const N: usize> Uncanon for OrdSet<K, S, N>
where
    K: Uncanon + Hash + Eq + Clone,
    S: BuildHasher + Default,
{
    fn uncanon(v: &Cs, cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        let mut s = OrdSet::new();
        for e in arr(v)? {
            s.insert(K::uncanon(e, cx)?);
        }
        Ok(s)
    }
}

// ---- references: SPEC §1.2 keys ----
impl Canon for SmallId {
    #[inline]
    fn canon(&self, _cx: &mut CanonCx<'_>) -> Cs {
        Cs::Int(i64::from(self.0))
    }
}
/// `r.owner(id)`: 0 = TerraNullius, else a player with a P row (`SnapshotContext.ts:154-172`).
/// Fields typed `Player` (never TerraNullius) are checked the same way: smallID 0 cannot be
/// stored there by TS.
impl Uncanon for SmallId {
    fn uncanon(v: &Cs, cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        cx.owner(int(v)?)
    }
}
impl Canon for UnitRef {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        cx.unit_key(*self)
    }
}
impl Uncanon for UnitRef {
    fn uncanon(v: &Cs, cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        cx.unit(int(v)?)
    }
}
/// A `Map<Unit, ..>`/`Set<Unit>` key held as the TS id: its key is the unit's (the unit row
/// is queued like any other reference).
impl Canon for UnitKey {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        cx.unit_key_by_id(self.0)
    }
}
impl Uncanon for UnitKey {
    fn uncanon(v: &Cs, cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        let id = int(v)?;
        cx.unit(id)?;
        Ok(UnitKey(id as u32))
    }
}
impl Canon for AttackRef {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        cx.attack_key(*self)
    }
}
impl Uncanon for AttackRef {
    fn uncanon(v: &Cs, cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        cx.attack(str_(v)?)
    }
}
/// An attack id as a value (`Attack.id`, `cancel_attack` ids): the 8-char string.
impl Canon for AttackId {
    fn canon(&self, _cx: &mut CanonCx<'_>) -> Cs {
        Cs::Str(self.to_ts_string())
    }
}
impl Uncanon for AttackId {
    fn uncanon(v: &Cs, _cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        let s = str_(v)?;
        AttackId::parse(s).ok_or_else(|| CanonError::new(format!("not an attack id: {s:?}")))
    }
}
impl Canon for AllianceId {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        cx.alliance_key(*self)
    }
}
impl Uncanon for AllianceId {
    fn uncanon(v: &Cs, cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        cx.alliance(int(v)?)
    }
}
impl Canon for ReqIdx {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        cx.request_key(*self)
    }
}
impl Uncanon for ReqIdx {
    fn uncanon(v: &Cs, cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        cx.request(str_(v)?)
    }
}
impl Canon for StationId {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        cx.station_key(*self)
    }
}
impl Uncanon for StationId {
    fn uncanon(v: &Cs, cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        cx.station(int(v)?)
    }
}
impl Canon for RailroadRef {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        cx.railroad_key(*self)
    }
}
impl Uncanon for RailroadRef {
    fn uncanon(v: &Cs, cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        cx.railroad(int(v)?)
    }
}
impl Canon for RailroadId {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        cx.railroad_key_by_id(*self)
    }
}
impl Uncanon for RailroadId {
    fn uncanon(v: &Cs, cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        let id = int(v)?;
        cx.railroad(id)?;
        Ok(RailroadId(id as u32))
    }
}
impl Canon for ClusterRef {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        cx.cluster_key(*self)
    }
}
impl Uncanon for ClusterRef {
    fn uncanon(v: &Cs, cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        cx.cluster(str_(v)?)
    }
}
impl Canon for ExecId {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        Cs::Int(cx.exec_key(*self))
    }
}
impl Uncanon for ExecId {
    fn uncanon(v: &Cs, cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        cx.exec(int(v)?)
    }
}
/// A MIRV warhead (`warheadExecutions: [uint]`): only bound ids exist at a tick boundary.
impl Canon for WarheadRef {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        match self {
            WarheadRef::Id(id) => Cs::Int(cx.exec_key(*id)),
            WarheadRef::Pending(p) => cx.fail(format!("CS: unbound MIRV warhead {p:?} at a tick boundary (Rust bug)")),
        }
    }
}
impl Uncanon for WarheadRef {
    fn uncanon(v: &Cs, cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        ExecId::uncanon(v, cx).map(WarheadRef::Id)
    }
}

// ---- string enums ----
// `Canon`/`Uncanon` for every `string_enum!` type (UnitType, PlayerType, TrainType, Difficulty,
// GameType, GameMode, RankedType, GameMapSize) live next to the type in `api/types.rs` (P1,
// FILE_MAP §5 rule 7; merge1). `RequestStatus` is not a `string_enum!`, so it stays here.
impl Canon for RequestStatus {
    fn canon(&self, _cx: &mut CanonCx<'_>) -> Cs {
        Cs::Str(self.as_str().to_owned())
    }
}
impl Uncanon for RequestStatus {
    fn uncanon(v: &Cs, _cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        match str_(v)? {
            "pending" => Ok(RequestStatus::Pending),
            "accepted" => Ok(RequestStatus::Accepted),
            "rejected" => Ok(RequestStatus::Rejected),
            s => Err(CanonError::new(format!("not a request status: {s:?}"))),
        }
    }
}

// ---- shared records ----
/// `w.random(r)` = `r.getState()` (`SnapshotContext.ts:119-121`): the four sfc32 words.
impl Canon for Prng {
    fn canon(&self, _cx: &mut CanonCx<'_>) -> Cs {
        Cs::Arr(self.get_state().iter().map(|&w| Cs::Int(i64::from(w))).collect())
    }
}
/// `PseudoRandom.fromState(state)`: each word `| 0` (`PseudoRandom.ts:40-47`).
impl Uncanon for Prng {
    fn uncanon(v: &Cs, _cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        match arr(v)? {
            [a, b, c, d] => {
                let w = |x: &Cs| x.as_f64().ok_or_else(|| CanonError::new("PRNG word is not a number"));
                Ok(Prng::from_state_f64([w(a)?, w(b)?, w(c)?, w(d)?]))
            }
            a => Err(CanonError::new(format!("PRNG state has {} words, expected 4", a.len()))),
        }
    }
}
/// `FlatBinaryHeap.getState()` minus `capacity` (SPEC §1.6 N3: allocation size, not state):
/// `{pri: f32s, tiles}`, live entries in heap array order (`FlatBinaryHeap.ts:76-82`).
impl Canon for FlatBinaryHeap {
    fn canon(&self, _cx: &mut CanonCx<'_>) -> Cs {
        let s = self.get_state();
        Cs::Obj(vec![("pri".into(), Cs::Typed(Typed::F32(s.pri))), ("tiles".into(), Cs::Typed(Typed::U32(s.tiles)))])
    }
}
/// `FlatBinaryHeap.fromState(s)` (`:84-94`). The stored capacity is gone from CS (N3); the
/// constructor default (1024, `:14`) or the entry count, whichever is larger, is used.
impl Uncanon for FlatBinaryHeap {
    fn uncanon(v: &Cs, _cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        let pri = match field(v, "pri")? {
            Cs::Typed(Typed::F32(p)) => p.clone(),
            x => return Err(CanonError::new(format!("heap pri: expected f32s, got {}", super::short(x)))),
        };
        let tiles = tiles_of(field(v, "tiles")?)?.to_vec();
        if pri.len() != tiles.len() {
            return Err(CanonError::new("heap pri/tiles lengths differ"));
        }
        let capacity = tiles.len().max(1024);
        Ok(FlatBinaryHeap::from_state(&HeapState { pri, tiles, capacity }))
    }
}
/// A JS `Set<TileRef>` snapshotted with `w.tiles(set)` (attack borders, …): `zTiles` in
/// iteration order.
impl Canon for OrderedTileSet {
    fn canon(&self, _cx: &mut CanonCx<'_>) -> Cs {
        Cs::Typed(Typed::U32(self.to_vec()))
    }
}
/// `new Set(tiles)` / `new TileSet(tiles)`: insertion in order.
impl Uncanon for OrderedTileSet {
    fn uncanon(v: &Cs, _cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        Ok(tiles_of(v)?.iter().copied().collect())
    }
}
/// `cellData(c)` (`CommonSchemas.ts:66-68`).
impl Canon for Cell {
    fn canon(&self, _cx: &mut CanonCx<'_>) -> Cs {
        Cs::Obj(vec![("x".into(), Cs::Num(self.x)), ("y".into(), Cs::Num(self.y))])
    }
}
impl Uncanon for Cell {
    fn uncanon(v: &Cs, cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        Ok(Cell { x: super::load_field(v, "x", cx)?, y: super::load_field(v, "y", cx)? })
    }
}
/// `playerInfoData(info)` (`CommonSchemas.ts:24-36`), the `PlayerInfoSchema` field order.
impl Canon for PlayerInfo {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        Cs::Obj(vec![
            ("name".into(), self.name.canon(cx)),
            ("playerType".into(), self.player_type.canon(cx)),
            ("clientID".into(), self.client_id.canon(cx)),
            ("id".into(), self.id.canon(cx)),
            ("isLobbyCreator".into(), Cs::Bool(self.is_lobby_creator)),
            ("clanTag".into(), self.clan_tag.canon(cx)),
            ("friends".into(), self.friends.canon(cx)),
            ("teamIndex".into(), self.team_index.canon(cx)),
            ("nationFlag".into(), self.nation_flag.canon(cx)),
        ])
    }
}
/// `newPlayerInfo(d)` (`CommonSchemas.ts:38-50`).
impl Uncanon for PlayerInfo {
    fn uncanon(v: &Cs, cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        use super::load_field as f;
        Ok(PlayerInfo {
            name: f(v, "name", cx)?,
            player_type: f(v, "playerType", cx)?,
            client_id: f(v, "clientID", cx)?,
            id: f(v, "id", cx)?,
            is_lobby_creator: f(v, "isLobbyCreator", cx)?,
            clan_tag: f(v, "clanTag", cx)?,
            friends: f(v, "friends", cx)?,
            team_index: f(v, "teamIndex", cx)?,
            nation_flag: f(v, "nationFlag", cx)?,
        })
    }
}
/// `nationData(n)` (`CommonSchemas.ts:77-82`).
impl Canon for Nation {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        Cs::Obj(vec![
            ("spawnCell".into(), self.spawn_cell.canon(cx)),
            ("playerInfo".into(), self.player_info.canon(cx)),
        ])
    }
}
/// `new Nation(spawnCell ? newCell(spawnCell) : undefined, newPlayerInfo(playerInfo))`
/// (`GameSnapshot.ts:268-274`).
impl Uncanon for Nation {
    fn uncanon(v: &Cs, cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        Ok(Nation {
            spawn_cell: super::load_field(v, "spawnCell", cx)?,
            player_info: super::load_field(v, "playerInfo", cx)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::{PlayerType, UnitType};
    use crate::canon::LoadKeys;
    use crate::exec::ExecStore;
    use crate::state::State;
    use of_det::JsMap;

    fn roundtrip<T: Canon + Uncanon + PartialEq + std::fmt::Debug>(x: &T) -> Cs {
        let st = State::default();
        let ex = ExecStore::default();
        let mut cx = CanonCx::new(&st, &ex);
        let v = x.canon(&mut cx);
        assert!(cx.errors.is_empty(), "{:?}", cx.errors);
        let keys = LoadKeys { n_players: 9, ..LoadKeys::default() };
        let mut st2 = State::default();
        let mut ucx = UncanonCx { st: &mut st2, keys: &keys };
        assert_eq!(&T::uncanon(&v, &mut ucx).expect("uncanon"), x);
        v
    }

    #[test]
    fn leaves_round_trip() {
        assert_eq!(roundtrip(&UnitType::AtomBomb), Cs::Str("Atom Bomb".into()));
        assert_eq!(roundtrip(&Big(-12_345_678_901)), Cs::Big(-12_345_678_901));
        assert_eq!(roundtrip(&(SmallId(3), -0.25f64)), Cs::Arr(vec![Cs::Int(3), Cs::Num(-0.25)]));
        let mut m: JsMap<SmallId, i32> = JsMap::default();
        m.insert(SmallId(5), 1);
        m.insert(SmallId(2), 7);
        assert_eq!(
            roundtrip(&m.iter().map(|(k, v)| (*k, *v)).collect::<Vec<_>>()),
            Cs::Arr(vec![Cs::Arr(vec![Cs::Int(5), Cs::Int(1)]), Cs::Arr(vec![Cs::Int(2), Cs::Int(7)]),])
        );
        let info = PlayerInfo::new("é zed", PlayerType::Human, Some("c1".into()), "abcd1234");
        let v = roundtrip(&info);
        assert_eq!(v.get("clientID"), Some(&Cs::Str("c1".into())));
        assert_eq!(v.get("teamIndex"), Some(&Cs::Null));
    }

    #[test]
    fn prng_words_and_heap_layout() {
        let st = State::default();
        let ex = ExecStore::default();
        let mut cx = CanonCx::new(&st, &ex);
        let r = Prng::new(123.0);
        let v = r.canon(&mut cx);
        let words: Vec<i64> = v.as_arr().expect("arr").iter().map(|x| x.as_i64().expect("int")).collect();
        assert_eq!(words, r.get_state().iter().map(|&w| i64::from(w)).collect::<Vec<_>>());
        let keys = LoadKeys::default();
        let mut st2 = State::default();
        let mut ucx = UncanonCx { st: &mut st2, keys: &keys };
        let mut r2 = Prng::uncanon(&v, &mut ucx).expect("prng");
        let mut r1 = r;
        assert_eq!(r1.next(), r2.next());

        let mut h = FlatBinaryHeap::new(4);
        for (t, p) in [(10u32, 3.0), (11, 1.0), (12, 1.0), (13, 0.1)] {
            h.enqueue(t, p);
        }
        let hv = h.canon(&mut cx);
        assert!(hv.get("capacity").is_none(), "N3: capacity is not state");
        let mut h2 = FlatBinaryHeap::uncanon(&hv, &mut ucx).expect("heap");
        let (a, b): (Vec<u32>, Vec<u32>) =
            ((0..4).map(|_| h.dequeue().expect("deq")).collect(), (0..4).map(|_| h2.dequeue().expect("deq")).collect());
        assert_eq!(a, b);
    }
}
