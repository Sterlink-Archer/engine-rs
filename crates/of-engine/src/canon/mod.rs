//! CS v1 bridge (ARCHITECTURE §9, SPEC §1): the `Canon`/`Uncanon` traits, the export key
//! registry ([`CanonCx`]), the load key resolver ([`UncanonCx`]), the [`refs`] handle walk
//! shared with the GC, and helper macros. Owner: **P13** (framework, G/T/M/W/S sections,
//! drain loop, loader). Each state/execution type's `Canon`/`Uncanon` impl lives next to the
//! type and is written by its owner (FILE_MAP §5 rule 7).
//!
//! Modules:
//! - [`leaves`]: `Canon`/`Uncanon` for scalars, containers, handles (SPEC §1.2 keys), enums;
//! - [`probe`]: compile-time dispatch to an owner's impl, or a reported "missing" error when
//!   the owner has not written it yet (so this framework builds and runs before S1 lands);
//! - [`export`]: `Game::canonical_state` — the SPEC §1.2.1 extraction order and drain loop,
//!   table sorting and key uniqueness, the K projection, `canon_k`/`canon_d`;
//! - [`sections`]: the G, T, M, W and S sections (export and import);
//! - [`exec_rows`]: the E rows (44-kind dispatch);
//! - [`load`]: `Game::from_canon` with TS-restore semantics (ARCHITECTURE §9 "Loader").
//!
//! Rules for owners' `Canon` impls:
//! - fields in **snapshot dump order** (`conformance/schema/snapshot_schemas_5b75bd3.txt`),
//!   which is also the order references are visited (SPEC §1.2.1 step 4); non-snapshot
//!   fields are not exported;
//! - every reference becomes its SPEC §1.2 key through [`CanonCx`] (the handle types'
//!   `Canon` impls in [`leaves`] do it), never a Rust slot;
//! - BigInt fields go through [`big`] / [`Big`]; `num` fields are handed over as `f64`
//!   (of-canon encodes safe integers as INT, SPEC §1.1); typed arrays keep their TS element
//!   kind ([`tiles`], [`bytes`], [`u16s`], [`f32s`]);
//! - `Canon::canon` cannot fail: an impl that meets an impossible state (a dangling key)
//!   calls [`CanonCx::fail`], which records the error and returns `null`; the exporter then
//!   returns that error.
//!
//! **Loader contract** for owners' `Uncanon` impls (ARCHITECTURE §9; see [`load`]):
//! - references resolve through [`UncanonCx`] (`UnitRef::uncanon`, `AttackRef::uncanon`, …):
//!   every row of every table already has its handle when any row is filled;
//! - `Player::uncanon` returns the player with **empty** `tiles`/`border_tiles` headers: the
//!   loader adds the row's `tiles`/`borderTiles` through the `PlayerTileOrder` and writes the
//!   owner bits (TS: `new TileSet(s.tiles)` + `map.setOwnerID`, `GameSnapshot.ts:403-407`);
//! - `RailNetwork::uncanon` returns the network with **empty** `station_manager.by_id`,
//!   `railroads`, `railroad_by_id` and `clusters`: the loader places the R tables' rows there
//!   (in table order, so the handles the resolver handed out are the ones allocated);
//! - every other row is returned whole (`Unit`, `Attack`, `Alliance`, `AllianceRequest`,
//!   `TrainStation`, `Railroad`, `Cluster`, each execution struct).

pub mod exec_rows;
pub mod export;
pub mod leaves;
pub mod load;
pub mod probe;
pub mod refs;
pub mod sections;

pub use export::{CanonMode, canonical_state};
pub use of_canon::digest::{CanonState, Section};
pub use of_canon::{Cs, Typed};
pub use refs::{RefLog, RefVisitor, Refs};

use crate::api::types::PlayerInfo;
use crate::exec::ExecStore;
use crate::ids::{
    AllianceId, AttackId, AttackRef, ClusterRef, ExecId, PlayerInfoIdx, RailroadId, RailroadRef, ReqIdx, SmallId,
    StationId, UnitRef,
};
use crate::jsorder::{LookupMap, LookupSet};
use crate::jsval::Tri;
use crate::state::State;
use std::sync::Arc;

/// Key registry and drain queues of one CS extraction (SPEC §1.2, §1.2.1). The queues are
/// the TS `SnapshotWriter` `RefTable`s (`snapshot/SnapshotContext.ts:26-46`): an object is
/// queued at its first reference and becomes a row when the drain loop reaches it.
pub struct CanonCx<'a> {
    pub st: &'a State,
    pub execs: &'a ExecStore,
    /// Full CS or the K projection (SPEC §1.5). Impls never need to look at it: the K
    /// projection is applied to the finished tree (`export`), exactly like
    /// `typedToCount` in `conformance/src/canon.ts`.
    pub mode: CanonMode,
    /// Execution keys: `live` positions, then `uninit`, then detached executions in order of
    /// first reference (SPEC §1.2 "Execution").
    exec_pos: LookupMap<ExecId, u32>,
    /// Executions in key order (index = key).
    pub exec_order: Vec<ExecId>,
    /// Units referenced so far, in first-reference order (rows to emit; dead-but-referenced
    /// units are rows, SPEC §1.2).
    pub unit_queue: Vec<UnitRef>,
    seen_units: LookupSet<UnitRef>,
    /// unit id → handle for every allocated unit (built on the first `UnitKey` lookup that
    /// misses `_unitMap`, i.e. a deleted unit referenced by id).
    units_by_id_all: Option<LookupMap<u32, UnitRef>>,
    /// Attacks referenced so far, in first-reference order.
    pub attack_queue: Vec<AttackRef>,
    seen_attacks: LookupSet<AttackRef>,
    /// Alliances referenced so far (`w.alliance(a)`), first-reference order.
    pub alliance_queue: Vec<AllianceId>,
    seen_alliances: LookupSet<AllianceId>,
    /// Alliance requests referenced so far (`w.allianceRequest(r)`).
    pub request_queue: Vec<ReqIdx>,
    seen_requests: LookupSet<ReqIdx>,
    /// Train stations referenced so far (`w.station(s)`).
    pub station_queue: Vec<StationId>,
    seen_stations: LookupSet<StationId>,
    /// Railroads referenced so far (`w.railroad(r)`), deduplicated by TS id.
    pub railroad_queue: Vec<RailroadRef>,
    seen_railroads: LookupSet<RailroadId>,
    /// Clusters referenced so far (`w.cluster(c)`), with their key fixed at first reference
    /// (`conformance/src/canon.ts` `clusterKeys`).
    pub cluster_queue: Vec<ClusterRef>,
    cluster_keys: LookupMap<ClusterRef, String>,
    n_empty_clusters: u32,
    /// Errors met inside `Canon` impls ([`CanonCx::fail`]); the exporter returns the first.
    pub errors: Vec<CanonError>,
}

impl<'a> CanonCx<'a> {
    /// SPEC §1.2.1 step 1: register `game.execs` in list order, then `game.unInitExecs`.
    pub fn new(st: &'a State, execs: &'a ExecStore) -> Self {
        Self::with_mode(st, execs, CanonMode::Full)
    }
    pub fn with_mode(st: &'a State, execs: &'a ExecStore, mode: CanonMode) -> Self {
        let mut cx = CanonCx {
            st,
            execs,
            mode,
            exec_pos: LookupMap::new(),
            exec_order: Vec::new(),
            unit_queue: Vec::new(),
            seen_units: LookupSet::new(),
            units_by_id_all: None,
            attack_queue: Vec::new(),
            seen_attacks: LookupSet::new(),
            alliance_queue: Vec::new(),
            seen_alliances: LookupSet::new(),
            request_queue: Vec::new(),
            seen_requests: LookupSet::new(),
            station_queue: Vec::new(),
            seen_stations: LookupSet::new(),
            railroad_queue: Vec::new(),
            seen_railroads: LookupSet::new(),
            cluster_queue: Vec::new(),
            cluster_keys: LookupMap::new(),
            n_empty_clusters: 0,
            errors: Vec::new(),
        };
        for &id in execs.live.iter().chain(st.uninit.iter()) {
            cx.exec_key(id);
        }
        cx
    }

    /// Record an impossible state met while exporting and return `null` in its place.
    pub fn fail(&mut self, msg: impl Into<String>) -> Cs {
        self.errors.push(CanonError::new(msg));
        Cs::Null
    }

    /// The key of execution `id` (registering a detached one at the next free position).
    pub fn exec_key(&mut self, id: ExecId) -> i64 {
        if let Some(&p) = self.exec_pos.get(&id) {
            return i64::from(p);
        }
        #[allow(clippy::expect_used)] // rust-invariant: < 2^32 executions
        let p = u32::try_from(self.exec_order.len()).expect("exec key fits u32");
        self.exec_pos.insert(id, p);
        self.exec_order.push(id);
        i64::from(p)
    }

    /// The key of unit `u` (`unit.id()`), queueing its row on first reference.
    pub fn unit_key(&mut self, u: UnitRef) -> Cs {
        let Some(unit) = self.st.units.try_get(u) else {
            return self.fail(format!("CS: reference to a freed unit slot {u:?} (GC bug)"));
        };
        let id = unit.id;
        if self.seen_units.insert(u) {
            self.unit_queue.push(u);
        }
        Cs::Int(i64::from(id))
    }

    /// The handle of the unit with TS id `id`: `_unitMap` first, else any allocated unit (a
    /// deleted-but-referenced one). `None` if no allocated unit has that id.
    pub fn unit_by_id(&mut self, id: u32) -> Option<UnitRef> {
        if let Some(&u) = self.st.unit_by_id.get(&id) {
            return Some(u);
        }
        let st = self.st;
        let all = self.units_by_id_all.get_or_insert_with(|| {
            let mut m = LookupMap::with_capacity(st.units.len());
            for (h, unit) in st.units.iter_slots() {
                m.insert(unit.id, h);
            }
            m
        });
        all.get(&id).copied()
    }

    /// The key of the unit with TS id `id` (a `UnitKey` map key), queueing its row.
    pub fn unit_key_by_id(&mut self, id: u32) -> Cs {
        match self.unit_by_id(id) {
            Some(u) => self.unit_key(u),
            None => self.fail(format!("CS: reference to unit id {id}, which no allocated unit has")),
        }
    }

    /// The key of attack `a` (its 8-char id string), queueing its row on first reference.
    pub fn attack_key(&mut self, a: AttackRef) -> Cs {
        let Some(att) = self.st.attacks.try_get(a) else {
            return self.fail(format!("CS: reference to a freed attack slot {a:?} (GC bug)"));
        };
        let id = att.id;
        if self.seen_attacks.insert(a) {
            self.attack_queue.push(a);
        }
        Cs::Str(id.to_ts_string())
    }

    /// The key of alliance `a` (`alliance.id`), queueing its row on first reference.
    pub fn alliance_key(&mut self, a: AllianceId) -> Cs {
        if self.st.alliances.get(a.0 as usize).is_none() {
            return self.fail(format!("CS: reference to missing alliance {}", a.0));
        }
        if self.seen_alliances.insert(a) {
            self.alliance_queue.push(a);
        }
        Cs::Int(i64::from(a.0))
    }

    /// `"<requestor>:<recipient>:<createdAt>"` (`conformance/src/canon.ts` `reqKey`).
    pub fn request_key_str(st: &State, r: ReqIdx) -> Option<String> {
        st.alliance_reqs.get(r.0 as usize).map(|q| format!("{}:{}:{}", q.requestor.0, q.recipient.0, q.created_at))
    }

    /// The key of alliance request `r`, queueing its row on first reference.
    pub fn request_key(&mut self, r: ReqIdx) -> Cs {
        let Some(k) = Self::request_key_str(self.st, r) else {
            return self.fail(format!("CS: reference to missing alliance request #{}", r.0));
        };
        if self.seen_requests.insert(r) {
            self.request_queue.push(r);
        }
        Cs::Str(k)
    }

    /// The key of train station `s` (`station.id`), queueing its row on first reference.
    pub fn station_key(&mut self, s: StationId) -> Cs {
        if self.seen_stations.insert(s) {
            self.station_queue.push(s);
        }
        Cs::Int(i64::from(s.0))
    }

    /// The key of railroad `r` (`railroad.id`), queueing its row on first reference.
    pub fn railroad_key(&mut self, r: RailroadRef) -> Cs {
        let Some(rr) = self.st.rail.railroads.try_get(r) else {
            return self.fail(format!("CS: reference to a freed railroad slot {r:?} (GC bug)"));
        };
        let id = rr.id;
        if self.seen_railroads.insert(id) {
            self.railroad_queue.push(r);
        }
        Cs::Int(i64::from(id.0))
    }

    /// The key of the railroad with TS id `id`.
    pub fn railroad_key_by_id(&mut self, id: RailroadId) -> Cs {
        match self.st.rail.railroad_by_id.get(&id) {
            Some(&r) => self.railroad_key(r),
            None => self.fail(format!("CS: reference to railroad id {}, which has no slot", id.0)),
        }
    }

    /// `"c<min member station id>"`, or `"e<n>"` for an empty cluster, n in order of first
    /// reference; the key is fixed at the first reference (`conformance/src/canon.ts:58-67`).
    pub fn cluster_key(&mut self, c: ClusterRef) -> Cs {
        if let Some(k) = self.cluster_keys.get(&c) {
            return Cs::Str(k.clone());
        }
        let Some(cl) = self.st.rail.clusters.try_get(c) else {
            return self.fail(format!("CS: reference to a freed cluster slot {c:?} (GC bug)"));
        };
        // TS: `let m = Infinity; for (const s of c.stations) m = Math.min(m, s.id)`.
        let k = match cl.stations.iter().map(|s| s.0).min() {
            Some(min) => format!("c{min}"),
            None => {
                let n = self.n_empty_clusters;
                self.n_empty_clusters += 1;
                format!("e{n}")
            }
        };
        self.cluster_keys.insert(c, k.clone());
        self.cluster_queue.push(c);
        Cs::Str(k)
    }

    /// The TS `PlayerID` string of player `p` (keys of `Map<PlayerID, ..>` snapshots such as
    /// `G.nationMirvTargets`).
    pub fn player_id_str(&mut self, p: SmallId) -> Cs {
        match player_info(self.st, p) {
            Some(info) => Cs::Str(info.id.clone()),
            None => self.fail(format!("CS: no player with smallID {}", p.0)),
        }
    }
}

/// The `PlayerInfo` of player `p` (`State.infos[players[p].info]`).
pub fn player_info(st: &State, p: SmallId) -> Option<&PlayerInfo> {
    if !p.is_player() {
        return None;
    }
    let pl = st.players.get(p.0 as usize - 1)?;
    st.infos.get(pl.info.0 as usize)
}

/// Load-side context: resolves SPEC §1.2 keys to the handles the loader pre-assigned
/// ([`LoadKeys`]), so every reference resolves even before its row is filled (TS: the
/// shells of `restoreGame` pass 1, `GameSnapshot.ts:300-358`).
pub struct UncanonCx<'a> {
    pub st: &'a mut State,
    pub keys: &'a LoadKeys,
}

/// Key → handle tables of one load (see [`load`]).
#[derive(Debug, Default)]
pub struct LoadKeys {
    /// unit id → handle (every U row).
    pub units: LookupMap<u32, UnitRef>,
    /// attack id → handle (every A row; ids are unique per CS, SPEC §1.2).
    pub attacks: LookupMap<AttackId, AttackRef>,
    /// alliance ids that have a D row.
    pub alliances: LookupSet<u32>,
    /// alliance-request key → index (every D.allianceRequests row).
    pub requests: LookupMap<String, ReqIdx>,
    /// station ids that have an R.stations row.
    pub stations: LookupSet<u32>,
    /// railroad id → handle (every R.railroads row).
    pub railroads: LookupMap<u32, RailroadRef>,
    /// cluster key → handle (every R.clusters row).
    pub clusters: LookupMap<String, ClusterRef>,
    /// execution key (position) → id (every E row).
    pub execs: Vec<ExecId>,
    /// smallIDs that have a P row (index = smallID - 1).
    pub n_players: u16,
}

impl UncanonCx<'_> {
    pub fn unit(&self, id: i64) -> Result<UnitRef, CanonError> {
        u32::try_from(id)
            .ok()
            .and_then(|i| self.keys.units.get(&i).copied())
            .ok_or_else(|| CanonError::new(format!("reference to missing unit {id}")))
    }
    pub fn attack(&self, key: &str) -> Result<AttackRef, CanonError> {
        AttackId::parse(key)
            .and_then(|a| self.keys.attacks.get(&a).copied())
            .ok_or_else(|| CanonError::new(format!("reference to missing attack {key:?}")))
    }
    pub fn alliance(&self, id: i64) -> Result<AllianceId, CanonError> {
        u32::try_from(id)
            .ok()
            .filter(|i| self.keys.alliances.contains(i))
            .map(AllianceId)
            .ok_or_else(|| CanonError::new(format!("reference to missing alliance {id}")))
    }
    pub fn request(&self, key: &str) -> Result<ReqIdx, CanonError> {
        self.keys
            .requests
            .get(&key.to_owned())
            .copied()
            .ok_or_else(|| CanonError::new(format!("reference to missing alliance request {key:?}")))
    }
    pub fn station(&self, id: i64) -> Result<StationId, CanonError> {
        u32::try_from(id)
            .ok()
            .filter(|i| self.keys.stations.contains(i))
            .map(StationId)
            .ok_or_else(|| CanonError::new(format!("reference to missing train station {id}")))
    }
    pub fn railroad(&self, id: i64) -> Result<RailroadRef, CanonError> {
        u32::try_from(id)
            .ok()
            .and_then(|i| self.keys.railroads.get(&i).copied())
            .ok_or_else(|| CanonError::new(format!("reference to missing railroad {id}")))
    }
    pub fn cluster(&self, key: &str) -> Result<ClusterRef, CanonError> {
        self.keys
            .clusters
            .get(&key.to_owned())
            .copied()
            .ok_or_else(|| CanonError::new(format!("reference to missing cluster {key:?}")))
    }
    pub fn exec(&self, key: i64) -> Result<ExecId, CanonError> {
        usize::try_from(key)
            .ok()
            .and_then(|i| self.keys.execs.get(i).copied())
            .ok_or_else(|| CanonError::new(format!("reference to missing execution {key}")))
    }
    /// `r.player(id)` (`SnapshotContext.ts:154-160`): a player with a P row.
    pub fn player(&self, id: i64) -> Result<SmallId, CanonError> {
        u16::try_from(id)
            .ok()
            .filter(|&i| i >= 1 && i <= self.keys.n_players)
            .map(SmallId)
            .ok_or_else(|| CanonError::new(format!("reference to missing player {id}")))
    }
    /// `r.owner(id)`: 0 is TerraNullius.
    pub fn owner(&self, id: i64) -> Result<SmallId, CanonError> {
        if id == 0 { Ok(SmallId::TERRA_NULLIUS) } else { self.player(id) }
    }
    /// `readPlayerInfo` (`CommonSchemas.ts:57-62`): the game's own `PlayerInfo` with that id
    /// when one exists, else a new entry. Returns its index into `State.infos`.
    pub fn intern_player_info(&mut self, info: PlayerInfo) -> PlayerInfoIdx {
        if let Some(i) = self.st.infos.iter().position(|x| x.id == info.id) {
            return PlayerInfoIdx(i as u32);
        }
        let v = Arc::make_mut(&mut self.st.infos);
        v.push(info);
        PlayerInfoIdx((v.len() - 1) as u32)
    }
}

/// Why a CS value could not be exported or loaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonError(pub String);

impl CanonError {
    pub fn new(msg: impl Into<String>) -> Self {
        CanonError(msg.into())
    }
    /// An owner's `Canon`/`Uncanon` impl that does not exist yet (see [`probe`]).
    pub fn missing(what: &str, ty: &str) -> Self {
        CanonError(format!("{MISSING_PREFIX}{what} for {ty}"))
    }
    /// Whether this error is a missing owner impl (S1 work outstanding), not a real failure
    /// (place prefixes added by [`CanonError::at`] keep it one).
    pub fn is_missing(&self) -> bool {
        self.0.contains(MISSING_PREFIX)
    }
    /// Prefix the message with the place it happened (`P[smallID=3]`, `E[17:Attack]`, …).
    pub fn at(self, place: impl std::fmt::Display) -> Self {
        CanonError(format!("{place}: {}", self.0))
    }
}

const MISSING_PREFIX: &str = "not implemented yet (owner S1): ";

impl std::fmt::Display for CanonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for CanonError {}

/// Export one state value as its CS v1 value (keys for references).
pub trait Canon {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs;
}

/// Load one state value from CS v1.
pub trait Uncanon: Sized {
    fn uncanon(v: &Cs, cx: &mut UncanonCx<'_>) -> Result<Self, CanonError>;
}

/// A BigInt field (`gold`, `troops`, `health`, stats counters; ARCHITECTURE §7.2).
#[inline]
pub fn big(x: i64) -> Cs {
    Cs::Big(i128::from(x))
}

/// A BigInt value as a `Canon` field (`canon_obj!(cx; "gold" => Big(self.gold))`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Big(pub i64);

/// `zTiles` (`Uint32Array`, TYPED kind 5).
pub fn tiles(t: impl IntoIterator<Item = u32>) -> Cs {
    Cs::Typed(Typed::U32(t.into_iter().collect()))
}
/// `zBytes` (`Uint8Array`, TYPED kind 1).
pub fn bytes(b: &[u8]) -> Cs {
    Cs::Typed(Typed::U8(b.to_vec()))
}
/// `Uint16Array` (TYPED kind 3).
pub fn u16s(b: &[u16]) -> Cs {
    Cs::Typed(Typed::U16(b.to_vec()))
}
/// `Float32Array` (TYPED kind 6; raw bits, NaN payloads kept).
pub fn f32s(b: &[f32]) -> Cs {
    Cs::Typed(Typed::F32(b.to_vec()))
}

// ---- load-side field helpers (the decoder already rejected malformed CSE1) ----

/// Object field `k` (absent → error).
pub fn field<'v>(v: &'v Cs, k: &str) -> Result<&'v Cs, CanonError> {
    match v {
        Cs::Obj(_) => v.get(k).ok_or_else(|| CanonError::new(format!("missing field {k:?}"))),
        _ => Err(CanonError::new(format!("expected an object with field {k:?}, got {}", short(v)))),
    }
}
/// Object field `k`, `None` when absent (`undefined`).
pub fn opt_field<'v>(v: &'v Cs, k: &str) -> Option<&'v Cs> {
    v.get(k)
}
/// Load field `k` of `v` as `T`.
pub fn load_field<T: Uncanon>(v: &Cs, k: &str, cx: &mut UncanonCx<'_>) -> Result<T, CanonError> {
    T::uncanon(field(v, k)?, cx).map_err(|e| e.at(k))
}
/// Load a `?null?undef` field as [`Tri`] (absent → `Absent`, `null` → `Null`).
pub fn load_tri<T: Uncanon>(v: &Cs, k: &str, cx: &mut UncanonCx<'_>) -> Result<Tri<T>, CanonError> {
    match opt_field(v, k) {
        None => Ok(Tri::Absent),
        Some(Cs::Null) => Ok(Tri::Null),
        Some(x) => T::uncanon(x, cx).map(Tri::Some).map_err(|e| e.at(k)),
    }
}
/// The elements of an ARRAY value.
pub fn arr(v: &Cs) -> Result<&[Cs], CanonError> {
    v.as_arr().map(Vec::as_slice).ok_or_else(|| CanonError::new(format!("expected array, got {}", short(v))))
}
/// An INT value (a `num` that is a safe integer also qualifies).
pub fn int(v: &Cs) -> Result<i64, CanonError> {
    v.as_i64().ok_or_else(|| CanonError::new(format!("expected int, got {}", short(v))))
}
/// A STR value.
pub fn str_(v: &Cs) -> Result<&str, CanonError> {
    v.as_str().ok_or_else(|| CanonError::new(format!("expected str, got {}", short(v))))
}
/// A `zTiles` value (TYPED u32; a DELTA_U32 storage form decodes to it).
pub fn tiles_of(v: &Cs) -> Result<&[u32], CanonError> {
    match v {
        Cs::Typed(Typed::U32(t)) => Ok(t),
        _ => Err(CanonError::new(format!("expected tiles (u32 typed array), got {}", short(v)))),
    }
}
/// A BIGINT value as `i64` (out of range is a load error: ARCHITECTURE §7.2 bounds).
pub fn big_of(v: &Cs) -> Result<i64, CanonError> {
    match v {
        Cs::Big(b) => i64::try_from(*b).map_err(|_| CanonError::new(format!("bigint {b} outside i64"))),
        _ => Err(CanonError::new(format!("expected bigint, got {}", short(v)))),
    }
}
/// A short rendering of a value for error messages.
pub fn short(v: &Cs) -> String {
    let s = format!("{v:?}");
    if s.chars().count() > 80 { format!("{}…", s.chars().take(80).collect::<String>()) } else { s }
}

/// Build a CS object from `"jsName" => expr` pairs in field order:
/// `canon_obj!(cx; "ticks" => self.ticks, "startTick" => self.start_tick)`. A `Cs::Absent`
/// value (from `Tri::Absent`) is omitted by the encoder, like an `undefined` field.
#[macro_export]
macro_rules! canon_obj {
    ($cx:expr; $($k:literal => $v:expr),* $(,)?) => {
        $crate::canon::Cs::Obj(::std::vec![
            $(($k.to_string(), $crate::canon::Canon::canon(&$v, $cx))),*
        ])
    };
}

/// `impl Canon for T` as the object `{ "jsName": self.field, … }` (fields in dump order).
#[macro_export]
macro_rules! impl_canon_record {
    ($t:ty { $($k:literal => $f:ident),* $(,)? }) => {
        impl $crate::canon::Canon for $t {
            fn canon(&self, cx: &mut $crate::canon::CanonCx<'_>) -> $crate::canon::Cs {
                $crate::canon_obj!(cx; $($k => self.$f),*)
            }
        }
    };
}

/// `impl Uncanon for T` from the object `{ "jsName": field, … }`, the inverse of
/// [`impl_canon_record!`] (every field must implement `Uncanon`; the struct has exactly these
/// fields).
#[macro_export]
macro_rules! impl_uncanon_record {
    ($t:ident { $($k:literal => $f:ident),* $(,)? }) => {
        impl $crate::canon::Uncanon for $t {
            fn uncanon(
                v: &$crate::canon::Cs,
                cx: &mut $crate::canon::UncanonCx<'_>,
            ) -> ::std::result::Result<Self, $crate::canon::CanonError> {
                Ok($t { $($f: $crate::canon::load_field(v, $k, cx)?),* })
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ExecKind;

    #[derive(Debug, PartialEq)]
    struct Rec {
        a: u32,
        b: f64,
        c: Option<SmallId>,
        d: Vec<bool>,
    }
    impl_canon_record!(Rec { "a" => a, "b" => b, "c" => c, "d" => d });
    impl_uncanon_record!(Rec { "a" => a, "b" => b, "c" => c, "d" => d });

    #[test]
    fn record_macro_and_exec_keys() {
        let st = State::default();
        let mut ex = ExecStore::default();
        let l0 = ExecId { kind: ExecKind::NoOp, idx: 5 };
        let u0 = ExecId { kind: ExecKind::NoOp, idx: 2 };
        ex.live.push(l0);
        let mut st2 = st;
        st2.uninit.push(u0);
        let mut cx = CanonCx::new(&st2, &ex);
        assert_eq!((cx.exec_key(l0), cx.exec_key(u0)), (0, 1));
        let detached = ExecId { kind: ExecKind::Nuke, idx: 0 };
        assert_eq!(cx.exec_key(detached), 2);
        assert_eq!(cx.exec_key(detached), 2);
        let r = Rec { a: 7, b: -0.5, c: None, d: vec![true] };
        let v = r.canon(&mut cx);
        assert_eq!(
            v,
            Cs::Obj(vec![
                ("a".into(), Cs::Int(7)),
                ("b".into(), Cs::Num(-0.5)),
                ("c".into(), Cs::Null),
                ("d".into(), Cs::Arr(vec![Cs::Bool(true)])),
            ])
        );
        assert!(cx.errors.is_empty());
        assert_eq!(big(-3), Cs::Big(-3));
        // Round trip through the inverse macro.
        let keys = LoadKeys { n_players: 0, ..LoadKeys::default() };
        let mut st3 = State::default();
        let mut ucx = UncanonCx { st: &mut st3, keys: &keys };
        assert_eq!(Rec::uncanon(&v, &mut ucx), Ok(r));
    }

    #[test]
    fn tri_fields_and_errors() {
        let keys = LoadKeys::default();
        let mut st = State::default();
        let mut cx = UncanonCx { st: &mut st, keys: &keys };
        let v = Cs::Obj(vec![("x".into(), Cs::Null), ("y".into(), Cs::Int(3))]);
        assert_eq!(load_tri::<i32>(&v, "x", &mut cx), Ok(Tri::Null));
        assert_eq!(load_tri::<i32>(&v, "y", &mut cx), Ok(Tri::Some(3)));
        assert_eq!(load_tri::<i32>(&v, "z", &mut cx), Ok(Tri::Absent));
        assert!(load_field::<i32>(&v, "z", &mut cx).is_err());
        // An unknown player key is a load error (SnapshotReader.player throws).
        assert!(cx.player(1).is_err());
        assert_eq!(cx.owner(0), Ok(SmallId::TERRA_NULLIUS));
        assert!(CanonError::missing("Canon", "X").is_missing());
        assert!(CanonError::missing("Canon", "X").at("P[smallID=1]").is_missing());
        assert!(!CanonError::new("boom").is_missing());
    }
}
