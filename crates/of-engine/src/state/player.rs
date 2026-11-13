//! `Player` (`engine/src/game/PlayerImpl.ts`) and its views. Owner: **P2**.
//!
//! - [`Player`] is the plain data. Fields: the `Player v1` snapshot record, in dump order
//!   (`conformance/schema/snapshot_schemas_5b75bd3.txt`), then the non-snapshot fields of
//!   ARCHITECTURE §3.3 (memos), each marked `non-snapshot`.
//! - [`PlayerRef`] holds `&State` and carries the TS `Player` interface's **pure readers**
//!   (`engine/src/game/Game.ts:314-510`). Readers that TS implements through a scratch
//!   object (`bfsSearch`, A*, `SpatialQuery`) take `sc: &mut Scratch` (`canAttack` →
//!   `bfsSearch`, `PlayerImpl.ts:1945`).
//! - [`PlayerMut`] holds `&mut Ctx` and carries every **mutating** method. Never hold a
//!   `&mut Player` across a call that takes `cx`; re-borrow per statement (PORTING_GUIDE §1).
//! - Exact memos (`myUnitsMemo`, `nearbyMemo`, the SpatialQuery reachable-components WeakMap)
//!   are not observable: readers may answer from a valid memo or recompute, and P2 decides
//!   where memos are refreshed. `verify-memos` checks them (G11).
//!
//! TS overloads `units()`, `units(types[])`, `units(t1, t2?, t3?)` (`PlayerImpl.ts:450-497`):
//! `units()` and `units([])` return the **live** array (§4.7) → [`PlayerRef::units`]; every
//! typed form returns a fresh copy → [`PlayerRef::units_of`].

use crate::StateClone;
use crate::api::types::{
    AllianceInfo, BuildableUnit, Cell, ClientID, EmojiMessage, EmojiRecipient, GameMode, GameType, Gold,
    MAX_UPGRADE_AMOUNT, PlayerInfo, PlayerProfile, PlayerType, Relation, Team, Tick, UnitType, colored_teams,
    unit_groups,
};
use crate::api::views::{PlayerLike, UnitLike};
use crate::canon::{Canon, CanonCx, Cs, big};
use crate::ctx::{Ctx, Rcx};
use crate::ids::{
    AllianceId, AttackId, AttackRef, Owner, PlayerInfoIdx, ReqIdx, SmallId, SmallIdKey, TileRef, UnitRef,
};
use crate::jsarr::JsArr;
use crate::jsorder::SmallJsMap;
use crate::jsval::Tri;
use crate::map::GameMap;
use crate::map::unit_grid::NearbyUnit;
use crate::scratch::Scratch;
use crate::state::attack::Attack;
use crate::state::memo::{Memo, TypeMemo, TypeMemoHdr};
use crate::state::stats::StatNum;
use crate::state::transport_ship_utils::{best_shore_deployment_source, can_build_transport_ship};
use crate::state::unit::{Unit, UnitParams, UnitView};
use crate::state::{State, Winner};
use crate::tileset::{OrderedTileSet, TileListHdr};
use crate::util::min_int;
use of_canon::Typed;
use of_det::Prng;
use of_det::bigint::from_number_strict;
use of_det::js;
use of_det::js_sort_by;
use of_det::util::{find_minimum_by, simple_hash, to_int_strict, within};

/// `Embargo` (`game/Game.ts:301-305`); field order = snapshot `embargoes[]` record.
#[derive(StateClone, Debug, Default, PartialEq, Eq)]
pub struct Embargo {
    pub target: SmallId,
    pub created_at: Tick,
    pub is_temporary: bool,
}

/// `Target` (`PlayerImpl.ts:95-98`): `{ tick, target }`.
#[derive(StateClone, Debug, Default, PartialEq, Eq)]
pub struct Target {
    pub tick: Tick,
    pub target: SmallId,
}

/// `Donation` (`PlayerImpl.ts:100-105`): `{ recipient, tick }`.
#[derive(StateClone, Debug, Default, PartialEq, Eq)]
pub struct Donation {
    pub recipient: SmallId,
    pub tick: Tick,
}

/// `DisconnectSnapshot` (`game/Game.ts:307-312`).
#[derive(StateClone, Debug, Default, PartialEq, Eq)]
pub struct DisconnectSnapshot {
    pub current_tick: Tick,
    /// int: a tile count.
    pub team_tiles: u32,
    /// int: a tile count.
    pub total_land: u32,
    pub was_alive: bool,
}

/// `largestClusterBoundingBox: { min: Cell; max: Cell }` (`game/Game.ts:340`); snapshot
/// `{minX, minY, maxX, maxY}`.
#[derive(StateClone, Debug, Default, PartialEq)]
pub struct BoundingBox {
    pub min: Cell,
    pub max: Cell,
}

/// `nearbyMemo` (`PlayerImpl.ts:600-604`). non-snapshot, exact.
#[derive(StateClone, Debug, Default, PartialEq)]
pub struct NearbyMemo {
    pub version: u32,
    pub water_version: u32,
    pub result: Vec<Owner>,
}

/// SpatialQuery's `reachableComponents` WeakMap entry (`pathfinding/spatial/SpatialQuery.ts:20-25`).
/// non-snapshot, exact (ARCHITECTURE §3.11).
#[derive(StateClone, Debug, Default)]
pub struct ReachableMemo {
    pub tile_version: u32,
    pub water_version: u32,
    pub components: crate::jsorder::JsSet<u32>,
}

/// `PlayerImpl` data.
#[derive(StateClone, Debug)]
pub struct Player {
    // ---- `Player v1` snapshot record, dump order ----
    /// `_smallID`.
    pub small_id: SmallId,
    /// `playerInfo`, as an index into `State.infos` (no `Arc` per player, review 1 #7).
    pub info: PlayerInfoIdx,
    /// `_team`.
    pub team: Option<Team>,
    /// `_lastTileChange` (`:135`).
    pub last_tile_change: Tick,
    /// `_tileChangeVersion` (`:138`). // int: a counter.
    pub tile_change_version: u32,
    /// `_pseudo_random` = `PseudoRandom(simpleHash(id))` (`:219`); attack ids (`nextID()`).
    pub rng: Prng,
    /// `_gold` (bigint, `:141`).
    pub gold: Gold,
    /// `_troops` (bigint, `:142`).
    pub troops: i64,
    pub trade_gold: Gold,
    pub train_gold: Gold,
    pub piracy_gold: Gold,
    pub gold_earned: Gold,
    pub marked_traitor_tick: Tick,
    pub marked_doomsday_clock_tick: Tick,
    pub rotted_at_tick: Tick,
    /// `_betrayalCount`. // int: a counter.
    pub betrayal_count: u32,
    /// `embargoes: Map<PlayerID, Embargo>` (`:159`); keyed by smallID (id ↔ smallID bijection).
    pub embargoes: SmallJsMap<SmallIdKey, Embargo, 4>,
    /// `_tiles` (`:176`): order in `State.pos_tiles` (ARCHITECTURE §3.6.2).
    pub tiles: TileListHdr,
    /// `_borderTiles` (`:161`): order in `State.pos_border`.
    pub border_tiles: TileListHdr,
    /// `_units` (`:163`): filter-reassigned by `UnitImpl.delete`/`setOwner` (§4.7).
    pub units: JsArr<UnitRef>,
    /// `_myUnitsVersion` (`:166`). // int: a counter.
    pub my_units_version: u32,
    pub past_outgoing_alliance_requests: Vec<ReqIdx>,
    pub expired_alliances: Vec<AllianceId>,
    /// `targets_` (`:181`).
    pub targets: Vec<Target>,
    /// `outgoingEmojis_` (`:183`).
    pub outgoing_emojis: Vec<EmojiMessage>,
    /// `outgoingQuickChats_: Map<number, Tick>` (`:184`), keyed by recipient smallID.
    pub outgoing_quick_chats: SmallJsMap<SmallIdKey, Tick, 4>,
    pub sent_donations: Vec<Donation>,
    /// `relations: Map<Player, number>` (`:188`); JS insertion order, keyed by smallID.
    pub relations: SmallJsMap<SmallIdKey, f64, 8>,
    pub last_delete_unit_tick: Tick,
    pub last_embargo_all_tick: Tick,
    /// `_incomingAttacks` (`:193`): filter-reassigned (`AttackImpl.ts:62-66`).
    pub incoming_attacks: JsArr<AttackRef>,
    /// `_outgoingAttacks` (`:194`): filter-reassigned (`AttackImpl.ts:68-70`).
    pub outgoing_attacks: JsArr<AttackRef>,
    /// `_outgoingLandAttacks` (`:195`).
    pub outgoing_land_attacks: Vec<AttackRef>,
    /// `_alliances` (`:197`): filter-reassigned (`GameImpl.ts:1143-1144`).
    pub alliances: JsArr<AllianceId>,
    pub spawn_tile: Option<TileRef>,
    pub is_disconnected: bool,
    pub disconnect_snapshot: Option<DisconnectSnapshot>,
    /// `largestClusterBoundingBox` (`:222`): absent until first computed, then null or a box
    /// (`?null?undef`, `:2024-2033`).
    pub largest_cluster_bbox: Tri<BoundingBox>,
    /// `numUnitsConstructed: Partial<Record<UnitType, number>>` (`:512`): `Object.entries`
    /// order = first-construction order (non-numeric keys, Q-OBJKEYS; `:2034`).
    pub num_units_constructed: Vec<(UnitType, i32)>,
    // ---- non-snapshot (exact memos, ARCHITECTURE §3.3) ----
    /// non-snapshot: `myUnitsMemo` (`:167`).
    pub my_units_memo: TypeMemo,
    /// non-snapshot: the per-type unit-count memo (`unitCount`).
    pub my_unit_count_memo: [Memo<i32>; 16],
    /// non-snapshot: the per-type units-owned memo (`unitsOwned`).
    pub my_units_owned_memo: [Memo<i32>; 16],
    /// non-snapshot: `nearbyMemo` (`:600-604`).
    pub nearby_memo: Option<NearbyMemo>,
    /// non-snapshot: SpatialQuery `reachableComponents` (`SpatialQuery.ts:25`).
    pub reachable_components_memo: Option<ReachableMemo>,
    /// non-snapshot (t409 H5, ARCHITECTURE §11 item 3): `true` only while the tiles with
    /// `ownerID == smallID` are known to form ONE 4-connected component (or none). Set by a full
    /// `floodTerritoryAreas` that found exactly that; kept by `Ctx::conquer`/`relinquish` only
    /// through changes that provably preserve 4-connectivity (`conn4` module), cleared by any
    /// other. While set, `removeClusters` derives the territory areas without flooding.
    pub conn4: bool,
    /// non-snapshot (t409 H5): component labels memo (`map::conn4` module doc).
    pub comps: crate::map::conn4::CompMemo,
}

impl Player {
    /// `new PlayerImpl(mg, smallID, playerInfo, startTroops, team)` (`PlayerImpl.ts:210-220`):
    /// `_troops = toInt(startTroops)`, `_gold = config.startingGold(info)`, PRNG seeded from
    /// `simpleHash(info.id)`. `starting_gold` is computed by the caller (`add_player`).
    pub fn new(
        small_id: SmallId,
        info: PlayerInfoIdx,
        player_info: &PlayerInfo,
        start_troops: f64,
        starting_gold: Gold,
        team: Option<Team>,
    ) -> Self {
        // Field initialisers (`:135-208`) are the struct literal; then the constructor body.
        let troops = to_int_strict(start_troops); // `:217`
        Player {
            small_id,
            info,
            team,
            last_tile_change: 0,
            tile_change_version: 0,
            rng: Prng::new(simple_hash(&player_info.id)), // `:219`
            gold: starting_gold,                          // `:218`
            troops,
            trade_gold: 0,
            train_gold: 0,
            piracy_gold: 0,
            gold_earned: 0,
            marked_traitor_tick: -1,
            marked_doomsday_clock_tick: -1,
            rotted_at_tick: -1,
            betrayal_count: 0,
            embargoes: SmallJsMap::new(),
            tiles: TileListHdr::default(),
            border_tiles: TileListHdr::default(),
            units: JsArr::new(),
            my_units_version: 0,
            past_outgoing_alliance_requests: Vec::new(),
            expired_alliances: Vec::new(),
            targets: Vec::new(),
            outgoing_emojis: Vec::new(),
            outgoing_quick_chats: SmallJsMap::new(),
            sent_donations: Vec::new(),
            relations: SmallJsMap::new(),
            last_delete_unit_tick: -1,
            last_embargo_all_tick: -1,
            incoming_attacks: JsArr::new(),
            outgoing_attacks: JsArr::new(),
            outgoing_land_attacks: Vec::new(),
            alliances: JsArr::new(),
            spawn_tile: None,
            is_disconnected: false,
            disconnect_snapshot: None,
            // `largestClusterBoundingBox` is declared without an initialiser (`:222`): absent.
            largest_cluster_bbox: Tri::Absent,
            num_units_constructed: Vec::new(),
            my_units_memo: TypeMemo::default(),
            my_unit_count_memo: Default::default(),
            my_units_owned_memo: Default::default(),
            nearby_memo: None,
            reachable_components_memo: None,
            conn4: false,
            comps: Default::default(),
        }
    }
}

// ---------------------------------------------------------------------------------------
// Exact memos (ARCHITECTURE §3.3). `memo.rs` (P1) declares the layout; P2's lookups and the
// one-pass rebuild live here so the per-player memos do not depend on P1's bodies.
// ---------------------------------------------------------------------------------------

/// `TypeMemo` hit test: the list for `t` if it was built at `version`.
fn type_memo_get(m: &TypeMemo, t: UnitType, version: u32) -> Option<&[UnitRef]> {
    let h = &m.hdr[t.index()]; // in-range: 16 headers, one per UnitType
    if h.valid && h.version == version { m.units.get(h.off as usize..(h.off + h.len) as usize) } else { None }
}

/// Rebuild all 16 per-type lists from `source` (in source order) at `version`.
fn type_memo_rebuild(m: &mut TypeMemo, source: &[UnitRef], version: u32, type_of: impl Fn(UnitRef) -> UnitType) {
    let mut counts = [0u32; 16];
    for &u in source {
        counts[type_of(u).index()] += 1; // in-range: 16 types
    }
    let mut off = 0u32;
    for (i, h) in m.hdr.iter_mut().enumerate() {
        *h = TypeMemoHdr { off, len: 0, version, valid: true };
        off += counts[i]; // in-range: 16 types
    }
    m.units.clear();
    m.units.resize(source.len(), UnitRef::default());
    for &u in source {
        let h = &mut m.hdr[type_of(u).index()]; // in-range: 16 types
        m.units[(h.off + h.len) as usize] = u; // in-range: sized from the counts above
        h.len += 1;
    }
}

/// `memo !== undefined && memo.version === this._myUnitsVersion`.
#[inline]
fn memo_hit<T: Copy>(m: &Memo<T>, version: u32) -> Option<T> {
    if m.version == Some(version) { Some(m.value) } else { None }
}

/// A set of owners in insertion order (`new Set<Player | TerraNullius>()` + `Array.from`),
/// membership by smallID (12 bits, `TileStateBuf`).
struct OwnerSet {
    order: Vec<Owner>,
    seen: [u64; 64],
}

impl OwnerSet {
    fn new() -> Self {
        OwnerSet { order: Vec::new(), seen: [0; 64] }
    }
    fn add(&mut self, o: Owner) {
        let i = usize::from(o.0);
        let (w, b) = (i / 64, i % 64);
        if let Some(word) = self.seen.get_mut(w) {
            if *word & (1 << b) == 0 {
                *word |= 1 << b;
                self.order.push(o);
            }
        } else if !self.order.contains(&o) {
            self.order.push(o);
        }
    }
}

/// N, S, W, E — `SHORE_DIRECTIONS_DX/DY` (`PlayerImpl.ts:125-127`).
const SHORE_DIRECTIONS_DX: [i64; 4] = [0, 0, -1, 1];
const SHORE_DIRECTIONS_DY: [i64; 4] = [-1, 1, 0, 0];
/// `DECAY_CUE_GRACE_TICKS` (`PlayerImpl.ts:93`).
const DECAY_CUE_GRACE_TICKS: i32 = 30;

/// `relationFromValue(relationValue)` (`PlayerImpl.ts:950-961`).
fn relation_from_value(relation_value: f64) -> Relation {
    if relation_value < -50.0 {
        return Relation::Hostile;
    }
    if relation_value < 0.0 {
        return Relation::Distrustful;
    }
    if relation_value < 50.0 {
        return Relation::Neutral;
    }
    Relation::Friendly
}

// ---------------------------------------------------------------------------------------
// Views
// ---------------------------------------------------------------------------------------

/// Read view (`&State` + smallID).
#[derive(Clone, Copy)]
pub struct PlayerRef<'a> {
    pub st: &'a State,
    pub id: SmallId,
}

/// Mutable view (`&mut Ctx` + smallID).
pub struct PlayerMut<'a, 'c> {
    pub cx: &'a mut Ctx<'c>,
    pub id: SmallId,
}

impl State {
    /// `playerBySmallID(id)` as a player view. Panics (Rust bug) for TerraNullius.
    #[inline]
    pub fn player(&self, id: SmallId) -> PlayerRef<'_> {
        PlayerRef { st: self, id }
    }
    /// The raw data of player `id` (`_playersBySmallID[id - 1]`).
    #[inline]
    pub fn player_data(&self, id: SmallId) -> &Player {
        &self.players[id.player_index()] // in-range: smallIDs come from add_player
    }
    #[inline]
    pub fn player_data_mut(&mut self, id: SmallId) -> &mut Player {
        &mut self.players[id.player_index()] // in-range: smallIDs come from add_player
    }
    /// `mg.players()` = `_playersBySmallID.filter((p) => p.isAlive())` (`GameImpl.ts:775-777`).
    fn alive_player_ids(&self) -> impl Iterator<Item = SmallId> + '_ {
        self.players.iter().filter(|p| p.tiles.size() > 0).map(|p| p.small_id)
    }
}

impl<'c> Ctx<'c> {
    /// Mutable player view.
    #[inline]
    pub fn player_mut(&mut self, id: SmallId) -> PlayerMut<'_, 'c> {
        PlayerMut { cx: self, id }
    }
}

impl<'a> PlayerRef<'a> {
    #[inline]
    pub fn data(&self) -> &'a Player {
        self.st.player_data(self.id)
    }
    #[inline]
    fn map(&self) -> &'a GameMap {
        &self.st.map
    }
    #[inline]
    fn ticks(&self) -> Tick {
        self.st.ticks
    }
    #[inline]
    fn other(&self, id: SmallId) -> PlayerRef<'a> {
        PlayerRef { st: self.st, id }
    }

    // ---- Basic info (`Game.ts:315-327`) ----
    pub fn small_id(&self) -> SmallId {
        self.id
    }
    /// `info()`.
    pub fn info(&self) -> &'a PlayerInfo {
        self.st.info(self.data().info)
    }
    pub fn name(&self) -> &'a str {
        &self.info().name
    }
    pub fn display_name(&self) -> String {
        self.info().display_name()
    }
    pub fn clan_tag(&self) -> Option<&'a str> {
        self.info().clan_tag.as_deref()
    }
    /// `nationFlag()` (`PlayerImpl.ts:435`).
    pub fn nation_flag(&self) -> Option<&'a str> {
        self.info().nation_flag.as_deref()
    }
    pub fn client_id(&self) -> Option<&'a ClientID> {
        self.info().client_id.as_ref()
    }
    /// `id()` (PlayerID).
    pub fn player_id(&self) -> &'a str {
        &self.info().id
    }
    /// `type()`.
    pub fn player_type(&self) -> PlayerType {
        self.info().player_type
    }
    pub fn is_lobby_creator(&self) -> bool {
        self.info().is_lobby_creator
    }
    /// `toString()` (`PlayerImpl.ts:1858-1864`), used in TS throw messages.
    pub fn ts_to_string(&self) -> String {
        let info = self.info();
        let cid = info.client_id.as_deref().unwrap_or("null");
        format!(
            "Player:{{name:{},clientID:{},isAlive:{},troops:{},numTileOwned:{}}}]",
            info.name,
            cid,
            self.is_alive(),
            self.data().troops,
            self.num_tiles_owned()
        )
    }

    // ---- State & properties (`Game.ts:329-357`) ----
    /// `isAlive()`: `_tiles.size > 0` (`:740-742`).
    pub fn is_alive(&self) -> bool {
        self.data().tiles.size() > 0
    }
    /// `isTraitor()` (`:873-875`).
    pub fn is_traitor(&self) -> bool {
        self.get_traitor_remaining_ticks() > 0
    }
    /// `getTraitorRemainingTicks()` (`PlayerImpl.ts:877`).
    pub fn get_traitor_remaining_ticks(&self) -> i32 {
        let d = self.data();
        if d.marked_traitor_tick < 0 {
            return 0;
        }
        let elapsed = self.ticks() - d.marked_traitor_tick;
        let duration = self.st.cfg.traitor_duration();
        let remaining = duration - elapsed;
        if remaining > 0 { remaining } else { 0 }
    }
    /// `inDoomsdayClock()` (`:896-898`).
    pub fn in_doomsday_clock(&self) -> bool {
        self.is_alive() && self.data().marked_doomsday_clock_tick >= 0
    }
    /// `isDecaying()` (`:926-929`).
    pub fn is_decaying(&self) -> bool {
        let d = self.data();
        if !self.in_doomsday_clock() || d.rotted_at_tick < 0 {
            return false;
        }
        self.ticks() - d.rotted_at_tick <= DECAY_CUE_GRACE_TICKS
    }
    /// `doomsdayClockTicks()` (`:901-905`).
    pub fn doomsday_clock_ticks(&self) -> i32 {
        if self.in_doomsday_clock() { self.ticks() - self.data().marked_doomsday_clock_tick } else { 0 }
    }
    /// `largestClusterBoundingBox` (field read; absent and null both read as `None`).
    pub fn largest_cluster_bounding_box(&self) -> Option<&'a BoundingBox> {
        self.data().largest_cluster_bbox.value()
    }
    pub fn last_tile_change(&self) -> Tick {
        self.data().last_tile_change
    }
    pub fn tile_change_version(&self) -> u32 {
        self.data().tile_change_version
    }
    pub fn is_disconnected(&self) -> bool {
        self.data().is_disconnected
    }
    pub fn disconnect_snapshot(&self) -> Option<&'a DisconnectSnapshot> {
        self.data().disconnect_snapshot.as_ref()
    }
    /// `disconnectedAtTick()`: `_disconnectSnapshot?.currentTick ?? null` (`:1848-1850`).
    pub fn disconnected_at_tick(&self) -> Option<Tick> {
        self.data().disconnect_snapshot.as_ref().map(|s| s.current_tick)
    }
    /// `hasSpawned()`: `_spawnTile !== undefined` (`:744-746`).
    pub fn has_spawned(&self) -> bool {
        self.data().spawn_tile.is_some()
    }
    pub fn spawn_tile(&self) -> Option<TileRef> {
        self.data().spawn_tile
    }

    // ---- Territory (`Game.ts:359-363`) ----
    /// `tiles()` (live, TS order; iterate with a cursor if the body mutates, §5.3).
    pub fn tiles(&self) -> &'a TileListHdr {
        &self.data().tiles
    }
    pub fn border_tiles(&self) -> &'a TileListHdr {
        &self.data().border_tiles
    }
    /// `numTilesOwned()`: `_tiles.size` (`:588-590`).
    pub fn num_tiles_owned(&self) -> u32 {
        #[allow(clippy::cast_possible_truncation)] // int: tile counts are below 2^32
        let n = self.data().tiles.size() as u32;
        n
    }

    // ---- Resources & troops (`Game.ts:365-394`) ----
    pub fn gold(&self) -> Gold {
        self.data().gold
    }
    pub fn trade_gold(&self) -> Gold {
        self.data().trade_gold
    }
    pub fn train_gold(&self) -> Gold {
        self.data().train_gold
    }
    pub fn piracy_gold(&self) -> Gold {
        self.data().piracy_gold
    }
    pub fn gold_earned(&self) -> Gold {
        self.data().gold_earned
    }
    /// `troops(): number` = `Number(this._troops)`.
    pub fn troops(&self) -> f64 {
        self.data().troops as f64
    }

    // ---- Units (`Game.ts:396-436`) ----
    /// `units()` / `units([])`: the **live** array (`PlayerImpl.ts:458-459,470-472`).
    pub fn units(&self) -> &'a JsArr<UnitRef> {
        &self.data().units
    }
    /// `units(type…)` / `units(types)` with ≥ 1 type: a fresh copy in `units` order
    /// (`:486-497`). An empty `types` is a Rust bug here (use [`Self::units`]).
    pub fn units_of(&self, types: &[UnitType]) -> Vec<UnitRef> {
        debug_assert!(!types.is_empty(), "units([]) returns the live array: use units()");
        let d = self.data();
        let units = d.units.as_slice();
        if let [t] = types {
            // Single-type form: `myUnitsMemo` (`:482-497`); a hit hands out a copy.
            if let Some(list) = type_memo_get(&d.my_units_memo, *t, d.my_units_version) {
                #[cfg(feature = "verify-memos")]
                {
                    let fresh: Vec<UnitRef> =
                        units.iter().copied().filter(|&u| self.st.unit_data(u).unit_type == *t).collect();
                    assert_eq!(list, fresh.as_slice(), "verify-memos: myUnitsMemo[{t:?}] stale (player {:?})", self.id);
                }
                return list.to_vec();
            }
            return units.iter().copied().filter(|&u| self.st.unit_data(u).unit_type == *t).collect();
        }
        // `ts.has(u.type())` / `t === first || t === second (|| t === third)` (`:469-509`).
        units.iter().copied().filter(|&u| types.contains(&self.st.unit_data(u).unit_type)).collect()
    }
    /// `unitCount(type)`: `Σ level` over owned units of `type` (`:530-548`, memoised).
    pub fn unit_count(&self, t: UnitType) -> i32 {
        let d = self.data();
        let fresh = || {
            let mut total = 0i32;
            for &u in d.units.as_slice() {
                let ud = self.st.unit_data(u);
                if ud.unit_type == t {
                    total += ud.level;
                }
            }
            total
        };
        if let Some(v) = memo_hit(&d.my_unit_count_memo[t.index()], d.my_units_version) {
            #[cfg(feature = "verify-memos")]
            assert_eq!(v, fresh(), "verify-memos: myUnitCountMemo[{t:?}] stale (player {:?})", self.id);
            return v;
        }
        fresh()
    }
    /// `unitsConstructed(type)`: `numUnitsConstructed[type] ?? 0` (`:525-527`).
    pub fn units_constructed(&self, t: UnitType) -> i32 {
        self.data().num_units_constructed.iter().find(|(k, _)| *k == t).map_or(0, |(_, n)| *n)
    }
    /// `unitsOwned(type)`: under construction counts 1, else `level` (`:551-571`, memoised).
    pub fn units_owned(&self, t: UnitType) -> i32 {
        let d = self.data();
        let fresh = || {
            let mut total = 0i32;
            for &u in d.units.as_slice() {
                let ud = self.st.unit_data(u);
                if ud.unit_type == t {
                    if ud.under_construction {
                        total += 1;
                    } else {
                        total += ud.level;
                    }
                }
            }
            total
        };
        if let Some(v) = memo_hit(&d.my_units_owned_memo[t.index()], d.my_units_version) {
            #[cfg(feature = "verify-memos")]
            assert_eq!(v, fresh(), "verify-memos: myUnitsOwnedMemo[{t:?}] stale (player {:?})", self.id);
            return v;
        }
        fresh()
    }
    /// `buildableUnits(tile, units?, includeBuildPreview?)` (oracle; ghost rails are empty).
    /// `units = None` is the TS default `PlayerBuildable.types`.
    pub fn buildable_units(
        &self,
        sc: &mut Scratch,
        tile: Option<TileRef>,
        units: Option<&[UnitType]>,
        include_build_preview: bool,
    ) -> Vec<BuildableUnit> {
        // TS `:1513-1585`.
        let units = units.unwrap_or(unit_groups::PLAYER_BUILDABLE);
        let config = &*self.st.cfg;
        let in_spawn_phase = self.st.in_spawn_phase();
        // `tile !== null && units.some(Structures.has) ? validStructureSpawnTiles(tile) : []`:
        // the empty array is passed on as `validTiles` (not null).
        let valid_tiles: Vec<TileRef> = match tile {
            Some(t) if units.iter().any(|u| unit_groups::has(unit_groups::STRUCTURES, *u)) => {
                self.valid_structure_spawn_tiles(sc, t)
            }
            _ => Vec::new(),
        };
        let mut result = Vec::with_capacity(units.len());
        for &u in units {
            let cost = config.unit_cost(u, self.st, self.id, 0.0);
            let mut can_upgrade: Option<u32> = None;
            let mut can_build: Option<TileRef> = None;
            if let Some(t) = tile
                && self.can_build_unit_type(u, Some(cost))
                && !in_spawn_phase
            {
                if self.can_upgrade_unit_type(u)
                    && let Some(existing) = self.find_existing_unit_to_upgrade(u, t)
                    && self.is_unit_valid_to_upgrade(existing)
                {
                    can_upgrade = Some(self.st.unit_data(existing).id);
                }
                can_build = self.can_spawn_unit_type(sc, u, t, Some(&valid_tiles));
            }
            let build_new = can_build.is_some() && (can_upgrade.is_none() || include_build_preview);
            let upgrade_costs = can_upgrade.map(|_| {
                let mut costs = Vec::with_capacity(MAX_UPGRADE_AMOUNT as usize);
                let mut total: Gold = 0;
                for n in 0..MAX_UPGRADE_AMOUNT {
                    total += config.unit_cost(u, self.st, self.id, f64::from(n));
                    costs.push(total);
                }
                costs
            });
            let overlapping_railroads = match (build_new, can_build) {
                (true, Some(cb)) => self.st.rail_overlapping_railroads(u, cb),
                _ => Vec::new(),
            };
            result.push(BuildableUnit {
                can_build,
                can_upgrade,
                unit_type: u,
                cost,
                upgrade_costs,
                overlapping_railroads,
                // `computeGhostRailPaths` is the skipped dry-run preview (ARCHITECTURE §3.9).
                ghost_rail_paths: Vec::new(),
            });
        }
        result
    }
    /// `canBuild(type, targetTile, validTiles = null): TileRef | false`.
    pub fn can_build(
        &self,
        sc: &mut Scratch,
        t: UnitType,
        target_tile: TileRef,
        valid_tiles: Option<&[TileRef]>,
    ) -> Option<TileRef> {
        if !self.can_build_unit_type(t, None) {
            return None;
        }
        self.can_spawn_unit_type(sc, t, target_tile, valid_tiles)
    }
    /// `canBuildUnitType(unitType, knownCost = null)` (`:1459-1474`).
    fn can_build_unit_type(&self, unit_type: UnitType, known_cost: Option<Gold>) -> bool {
        if self.st.cfg.is_unit_disabled(unit_type) {
            return false;
        }
        let cost = match known_cost {
            Some(c) => c,
            None => self.st.cfg.unit_cost(unit_type, self.st, self.id, 0.0),
        };
        if self.data().gold < cost {
            return false;
        }
        if unit_type != UnitType::MirvWarhead && !self.is_alive() {
            return false;
        }
        true
    }
    /// `canUpgradeUnitType(unitType)`: `Boolean(config.unitInfo(unitType).upgradable)`.
    fn can_upgrade_unit_type(&self, unit_type: UnitType) -> bool {
        self.st.cfg.unit_info(unit_type).upgradable.unwrap_or(false)
    }
    /// `isUnitValidToUpgrade(unit)` (`:1480-1491`).
    fn is_unit_valid_to_upgrade(&self, unit: UnitRef) -> bool {
        let u = self.st.unit(unit);
        if u.is_under_construction() {
            return false;
        }
        if u.is_marked_for_deletion() {
            return false;
        }
        if u.owner() != self.id {
            return false;
        }
        true
    }
    /// `findExistingUnitToUpgrade(type, targetTile)` (`:1441-1457`).
    fn find_existing_unit_to_upgrade(&self, t: UnitType, target_tile: TileRef) -> Option<UnitRef> {
        let near = self.st.nearby_units(target_tile, self.st.cfg.structure_min_dist(), &[t], |_, _, _| true, true);
        find_minimum_by(&near, |e: &NearbyUnit| e.dist_squared, None::<fn(&NearbyUnit) -> bool>).map(|i| near[i].unit)
    }
    /// `canSpawnUnitType(unitType, targetTile, validTiles)` (`:1599-1637`).
    fn can_spawn_unit_type(
        &self,
        sc: &mut Scratch,
        unit_type: UnitType,
        target_tile: TileRef,
        valid_tiles: Option<&[TileRef]>,
    ) -> Option<TileRef> {
        match unit_type {
            UnitType::Mirv => {
                if !self.map().has_owner(target_tile) {
                    return None;
                }
                self.nuke_spawn(sc, target_tile, unit_type)
            }
            UnitType::AtomBomb | UnitType::HydrogenBomb => self.nuke_spawn(sc, target_tile, unit_type),
            UnitType::MirvWarhead => Some(target_tile),
            UnitType::Port => self.port_spawn(sc, target_tile, valid_tiles),
            UnitType::Warship => self.warship_spawn(sc, target_tile),
            UnitType::Shell | UnitType::SamMissile => Some(target_tile),
            UnitType::TransportShip => {
                let mut rcx = Rcx::new(self.st, sc);
                can_build_transport_ship(&mut rcx, self.id, target_tile)
            }
            UnitType::TradeShip => self.trade_ship_spawn(target_tile),
            UnitType::Train => self.land_based_unit_spawn(sc, target_tile),
            UnitType::MissileSilo
            | UnitType::DefensePost
            | UnitType::SamLauncher
            | UnitType::City
            | UnitType::Factory => self.land_based_structure_spawn(sc, target_tile, valid_tiles),
        }
    }
    /// `nukeSpawn(tile, nukeType)` (`PlayerImpl.ts:1639`).
    pub fn nuke_spawn(&self, sc: &mut Scratch, tile: TileRef, nuke_type: UnitType) -> Option<TileRef> {
        let _ = sc;
        let st = self.st;
        if st.is_spawn_immunity_active() {
            return None;
        }
        // Impassable terrain cannot be nuked.
        if st.map.is_impassable(tile) {
            return None;
        }
        let owner = SmallId(st.map.owner_id(tile));
        let config = &*st.cfg;
        let game_over = !matches!(st.winner, Winner::None) && config.game_config().game_type != GameType::Singleplayer;
        if owner.is_player() && self.is_on_same_team(owner) && !game_over {
            return None;
        }
        if config.game_config().game_mode == GameMode::Team && nuke_type != UnitType::Mirv && !game_over {
            let magnitude = config.nuke_magnitudes(nuke_type);
            let me = *self;
            let would_hit_teammate = st.any_unit_nearby(
                tile,
                magnitude.outer,
                unit_groups::STRUCTURES,
                |s, u| {
                    let o = s.unit_data(u).owner;
                    o.is_player() && me.is_on_same_team(o)
                },
                None,
                false,
            );
            if would_hit_teammate {
                return None;
            }
        }
        // Only silos that are not on cooldown and not under construction.
        let mut ready_silos: Vec<UnitRef> = self
            .units_of(&[UnitType::MissileSilo])
            .into_iter()
            .filter(|&s| {
                let v = st.unit(s);
                v.is_active() && !v.is_in_cooldown() && !v.is_under_construction()
            })
            .collect();
        js_sort_by(&mut ready_silos, |a, b| {
            f64::from(st.map.manhattan_dist(st.unit_data(*a).tile, tile))
                - f64::from(st.map.manhattan_dist(st.unit_data(*b).tile, tile))
        });
        ready_silos.first().map(|&s| st.unit_data(s).tile)
    }
    /// `portSpawn(tile, validTiles)` (`:1691`).
    pub fn port_spawn(&self, sc: &mut Scratch, tile: TileRef, valid_tiles: Option<&[TileRef]>) -> Option<TileRef> {
        let owned;
        let tiles = match valid_tiles {
            Some(v) => v,
            None => {
                owned = self.valid_structure_spawn_tiles(sc, tile);
                &owned[..]
            }
        };
        tiles.iter().copied().find(|&t| self.map().is_shore(t))
    }
    /// `warshipSpawn(tile)` (`:1701`).
    pub fn warship_spawn(&self, sc: &mut Scratch, tile: TileRef) -> Option<TileRef> {
        if !self.map().is_water(tile) {
            return None;
        }
        let mut rcx = Rcx::new(self.st, sc);
        let tile_component = rcx.get_water_component(tile);
        // `findClosestBy(units(Port), manhattanDist, isCandidate)` = `findMinimumBy`
        // (`Util.ts:55-87`), inlined: the candidate test needs `&mut Rcx`.
        let ports = self.units_of(&[UnitType::Port]);
        let mut best: Option<UnitRef> = None;
        let mut best_score = f64::INFINITY;
        for &port in &ports {
            let pd = self.st.unit(port);
            let candidate = pd.is_active()
                && !pd.is_under_construction()
                && match tile_component {
                    Some(c) => rcx.has_water_component(pd.tile(), c),
                    None => false,
                };
            if !candidate {
                continue;
            }
            let score = f64::from(self.map().manhattan_dist(pd.tile(), tile));
            if score < best_score {
                best_score = score;
                best = Some(port);
            }
        }
        best.map(|p| self.st.unit_data(p).tile)
    }
    /// `landBasedUnitSpawn(tile)` (`:1720`).
    pub fn land_based_unit_spawn(&self, sc: &mut Scratch, tile: TileRef) -> Option<TileRef> {
        let _ = sc;
        if self.map().is_land(tile) && !self.map().is_impassable(tile) { Some(tile) } else { None }
    }
    /// `landBasedStructureSpawn(tile, validTiles = null)` (`:1724`).
    pub fn land_based_structure_spawn(
        &self,
        sc: &mut Scratch,
        tile: TileRef,
        valid_tiles: Option<&[TileRef]>,
    ) -> Option<TileRef> {
        match valid_tiles {
            Some(v) => v.first().copied(),
            None => self.valid_structure_spawn_tiles(sc, tile).first().copied(),
        }
    }
    /// `validStructureSpawnTiles(tile)` (`:1735-1805`): own tiles within radius 15 of `tile`,
    /// flooded N/S/W/E on the shared traversal scratch, minus those within `structureMinDist`
    /// of a structure, sorted (stable) by distance to `tile`.
    fn valid_structure_spawn_tiles(&self, sc: &mut Scratch, tile: TileRef) -> Vec<TileRef> {
        let st = self.st;
        let map = &st.map;
        if map.owner_id(tile) != self.id.0 {
            return Vec::new();
        }
        let search_radius: u32 = 15;
        let search_radius_squared: i64 = 15 * 15; // `searchRadius ** 2`
        let nearby_units =
            st.nearby_units(tile, f64::from(search_radius * 2), unit_groups::STRUCTURES, |_, _, _| true, true);
        let w = map.width();
        // int: non-negative refs → `%` and `(t / w) | 0` are exact on u32 (`:1755-1756`).
        let cx = i64::from(tile % w);
        let cy = i64::from(tile / w);
        let small_id = self.id.0;
        let inside = |t: TileRef| -> bool {
            let dx = i64::from(t % w) - cx;
            let dy = i64::from(t / w) - cy;
            dx * dx + dy * dy < search_radius_squared && map.owner_id(t) == small_id
        };
        let total = (map.width() as usize) * (map.height() as usize);
        let scratch = sc.traversal.ensure(total);
        let stamp = scratch.bump_generation();
        scratch.stack.clear();
        let mut nearby_tiles: Vec<TileRef> = Vec::new();
        if inside(tile) {
            scratch.visited[tile as usize] = stamp; // in-range: tile < width*height
            nearby_tiles.push(tile);
            scratch.stack.push(tile);
        }
        let mut nbuf = [0u32; 4];
        while let Some(t) = scratch.stack.pop() {
            // `map.forEachNeighbor(stack.pop()!, visit)`: N, S, W, E (`neighbors4` order).
            let n = map.neighbors4(t, &mut nbuf);
            for &nb in &nbuf[..n] {
                if scratch.visited[nb as usize] != stamp && inside(nb) {
                    scratch.visited[nb as usize] = stamp; // in-range: neighbours are on the map
                    nearby_tiles.push(nb);
                    scratch.stack.push(nb);
                }
            }
        }
        let msd = st.cfg.structure_min_dist();
        let min_dist_squared = msd * msd; // `structureMinDist() ** 2`
        let mut valid: Vec<TileRef> = Vec::new();
        for &t in &nearby_tiles {
            let mut blocked = false;
            for e in &nearby_units {
                if f64::from(map.euclidean_dist_squared(st.unit_data(e.unit).tile, t)) < min_dist_squared {
                    blocked = true;
                    break;
                }
            }
            if !blocked {
                valid.push(t);
            }
        }
        js_sort_by(&mut valid, |a, b| {
            f64::from(map.euclidean_dist_squared(*a, tile)) - f64::from(map.euclidean_dist_squared(*b, tile))
        });
        valid
    }
    /// `tradeShipSpawn(targetTile)` (`:1807`).
    pub fn trade_ship_spawn(&self, target_tile: TileRef) -> Option<TileRef> {
        if self.units_of(&[UnitType::Port]).iter().any(|&u| self.st.unit_data(u).tile == target_tile) {
            Some(target_tile)
        } else {
            None
        }
    }
    /// `findUnitToUpgrade(type, targetTile): Unit | false`.
    pub fn find_unit_to_upgrade(&self, t: UnitType, target_tile: TileRef) -> Option<UnitRef> {
        let unit = self.find_existing_unit_to_upgrade(t, target_tile)?;
        if !self.can_upgrade_unit(unit) {
            return None;
        }
        Some(unit)
    }
    /// `canUpgradeUnit(unit)` (`:1493-1504`).
    pub fn can_upgrade_unit(&self, u: UnitRef) -> bool {
        let t = self.st.unit_data(u).unit_type;
        if !self.can_upgrade_unit_type(t) {
            return false;
        }
        if !self.can_build_unit_type(t, None) {
            return false;
        }
        if !self.is_unit_valid_to_upgrade(u) {
            return false;
        }
        true
    }

    // ---- Relations & diplomacy (`Game.ts:438-461`) ----
    /// `nearby()` (exact memo `nearbyMemo`, `:600-626`): keyed on `territoryVersion` and the
    /// map's `waterVersion`; a hit hands out a copy.
    pub fn nearby(&self) -> Vec<Owner> {
        let version = self.st.territory_version;
        let water_version = self.map().water_version();
        if let Some(m) = &self.data().nearby_memo
            && m.version == version
            && m.water_version == water_version
        {
            #[cfg(feature = "verify-memos")]
            assert_eq!(m.result, self.compute_nearby(), "verify-memos: nearbyMemo stale (player {:?})", self.id);
            return m.result.clone();
        }
        self.compute_nearby()
    }
    /// `computeNearby()` (`:627-651`).
    fn compute_nearby(&self) -> Vec<Owner> {
        let mut ns = OwnerSet::new();
        let map = self.map();
        let small_id = self.id.0;
        let v = map.view();
        let mut nbuf = [0u32; 4];
        for border in self.border_tiles().iter() {
            // `map.forEachNeighbor(border, visit)` (N, S, W, E). D1: the same tests on the same operands in the same order, but the
            // player's own tiles (the common neighbour: they add nothing) are told apart first, from one owner read.
            let n = map.neighbors4(border, &mut nbuf);
            for &neighbor in &nbuf[..n] {
                let owner = v.owner_id(neighbor);
                if owner == small_id {
                    continue;
                }
                if v.is_land(neighbor) && !v.is_impassable(neighbor) {
                    if owner == 0 && v.has_fallout(neighbor) {
                        continue; // `!hasOwner && hasFallout`
                    }
                    ns.add(SmallId(owner));
                }
            }
        }
        for n in self.shore_reachable_neighbors() {
            ns.add(n);
        }
        ns.order
    }
    /// `shoreReachableNeighbors()` (`:656-696`): every 10th shore border tile looks 5 tiles
    /// across water in each cardinal direction that immediately enters water.
    fn shore_reachable_neighbors(&self) -> Vec<Owner> {
        let mut ns = OwnerSet::new();
        let map = self.map();
        let mut shore_idx: u64 = 0;
        for border in self.border_tiles().iter() {
            if !map.is_shore(border) {
                continue;
            }
            // `if (shoreIdx++ % 10 !== 0) continue;`
            let idx = shore_idx;
            shore_idx += 1;
            if !idx.is_multiple_of(10) {
                continue;
            }
            let bx = i64::from(map.x(border));
            let by = i64::from(map.y(border));
            for d in 0..4 {
                let dx = SHORE_DIRECTIONS_DX[d];
                let dy = SHORE_DIRECTIONS_DY[d];
                let x1 = bx + dx;
                let y1 = by + dy;
                #[allow(clippy::cast_precision_loss)] // int: map coordinates are small
                let (fx1, fy1) = (x1 as f64, y1 as f64);
                if !map.is_valid_coord(fx1, fy1) || !map.is_water(map.ref_(fx1, fy1)) {
                    continue;
                }
                let nx = bx + dx * 5;
                let ny = by + dy * 5;
                #[allow(clippy::cast_precision_loss)] // int: map coordinates are small
                let (fnx, fny) = (nx as f64, ny as f64);
                if !map.is_valid_coord(fnx, fny) {
                    continue;
                }
                let tile = map.ref_(fnx, fny);
                if !map.is_land(tile) {
                    continue;
                }
                if map.is_impassable(tile) {
                    continue;
                }
                if !map.has_owner(tile) && map.has_fallout(tile) {
                    continue;
                }
                let owner = map.owner_id(tile);
                if owner != self.id.0 {
                    ns.add(SmallId(owner));
                }
            }
        }
        ns.order
    }
    /// `sharesBorderWith(other)` (`:573-586`).
    pub fn shares_border_with(&self, other: Owner) -> bool {
        let map = self.map();
        let other_id = other.0;
        let v = map.view();
        let mut nbuf = [0u32; 4];
        for border in self.border_tiles().iter() {
            let n = map.neighbors4(border, &mut nbuf);
            for &nb in &nbuf[..n] {
                if v.owner_id(nb) == other_id {
                    return true;
                }
            }
        }
        false
    }
    /// D2: [`Self::shares_border_with`] answering from witness hints when one still proves `true`. A hint is a border tile of
    /// this player that touched `other` last time; it is believed only after it is re-checked against the live state (still in the
    /// border list, still next to a tile of `other`), so the answer is the scan's answer; `false` always comes from the scan.
    pub fn shares_border_with_hinted(&self, other: Owner, hints: &mut crate::scratch::hints::BorderHints) -> bool {
        use crate::scratch::hints::BorderHints;
        let map = self.map();
        let other_id = other.0;
        let v = map.view();
        let mut nbuf = [0u32; 4];
        let key = BorderHints::key(self.id.0, other_id);
        let land_index = &self.st.statics.land_index;
        for b in hints.witnesses(key) {
            // a foreign hint (another game's map on this thread) may be out of range here: bounds first
            if !BorderHints::is_none(b)
                && (b as usize) < land_index.len()
                && self.st.pos_border().has(self.border_tiles(), land_index, b)
            {
                let n = map.neighbors4(b, &mut nbuf);
                if nbuf[..n].iter().any(|&nb| v.owner_id(nb) == other_id) {
                    return true;
                }
            }
        }
        for border in self.border_tiles().iter() {
            let n = map.neighbors4(border, &mut nbuf);
            for &nb in &nbuf[..n] {
                if v.owner_id(nb) == other_id {
                    hints.record(key, border);
                    return true;
                }
            }
        }
        false
    }
    /// `relation(other)` (`:942-948`); throws for `other === this`.
    pub fn relation(&self, other: SmallId) -> Relation {
        if other == self.id {
            let s = self.ts_to_string();
            of_det::ts_throw!("cannot get relation with self: {s}");
        }
        let relation = self.data().relations.get(&other).copied().unwrap_or(0.0);
        relation_from_value(relation)
    }
    /// `allRelationsSorted()` (`js_sort_by` at `PlayerImpl.ts:966`).
    pub fn all_relations_sorted(&self) -> Vec<(SmallId, Relation)> {
        let mut rs: Vec<(SmallId, f64)> =
            self.data().relations.iter().map(|(k, v)| (*k, *v)).filter(|(p, _)| self.other(*p).is_alive()).collect();
        js_sort_by(&mut rs, |a, b| a.1 - b.1);
        rs.into_iter().map(|(p, r)| (p, relation_from_value(r))).collect()
    }
    /// `isOnSameTeam(other)` (`:1293-1304`).
    pub fn is_on_same_team(&self, other: SmallId) -> bool {
        if other == self.id {
            return false;
        }
        let (Some(mine), Some(theirs)) = (&self.data().team, &self.other(other).data().team) else {
            return false;
        };
        if mine == colored_teams::BOT || theirs == colored_teams::BOT {
            return false;
        }
        mine == theirs
    }
    /// `isFriendly(other, treatAFKFriendly = false)`.
    pub fn is_friendly(&self, other: SmallId, treat_afk_friendly: bool) -> bool {
        if other == self.id {
            return true;
        }
        if self.other(other).is_disconnected() && !treat_afk_friendly {
            return false;
        }
        self.is_on_same_team(other) || self.is_allied_with(other)
    }
    pub fn team(&self) -> Option<&'a Team> {
        self.data().team.as_ref()
    }
    /// `incomingAllianceRequests()`: `mg.allianceRequests.filter(recipient === this)`.
    pub fn incoming_alliance_requests(&self) -> Vec<ReqIdx> {
        self.st
            .alliance_requests
            .as_slice()
            .iter()
            .copied()
            .filter(|&r| self.st.alliance_request(r).recipient == self.id)
            .collect()
    }
    /// `outgoingAllianceRequests()`: `mg.allianceRequests.filter(requestor === this)`.
    pub fn outgoing_alliance_requests(&self) -> Vec<ReqIdx> {
        self.st
            .alliance_requests
            .as_slice()
            .iter()
            .copied()
            .filter(|&r| self.st.alliance_request(r).requestor == self.id)
            .collect()
    }
    /// `alliances()`: the live array (§4.7).
    pub fn alliances(&self) -> &'a JsArr<AllianceId> {
        &self.data().alliances
    }
    /// `expiredAlliances()` returns a copy (`[...this._expiredAlliances]`); the slice is
    /// immutable, so it is the same value.
    pub fn expired_alliances(&self) -> &'a [AllianceId] {
        &self.data().expired_alliances
    }
    /// `allies()`: `alliances().map((a) => a.other(this))`.
    pub fn allies(&self) -> Vec<SmallId> {
        self.alliances().as_slice().iter().map(|&a| self.st.alliance(a).other(self.id)).collect()
    }
    /// `isAlliedWith(other)` (`:776-781`).
    pub fn is_allied_with(&self, other: SmallId) -> bool {
        if other == self.id {
            return false;
        }
        self.alliance_with(other).is_some()
    }
    /// `allianceWith(other)` (`:783-792`): the first alliance (list order) with `other`.
    pub fn alliance_with(&self, other: SmallId) -> Option<AllianceId> {
        if other == self.id {
            return None;
        }
        self.alliances().as_slice().iter().copied().find(|&a| {
            let al = self.st.alliance(a);
            al.recipient == other || al.requestor == other
        })
    }
    /// `allianceInfo(other)` (`:794-816`).
    pub fn alliance_info(&self, other: SmallId) -> Option<AllianceInfo> {
        let alliance = self.alliance_with(other)?;
        let al = self.st.alliance(alliance);
        let in_extension_window =
            f64::from(al.expires_at) <= f64::from(self.ticks()) + self.st.cfg.alliance_extension_prompt_offset();
        let o = self.other(other);
        let can_extend = !self.is_disconnected()
            && !o.is_disconnected()
            && self.is_alive()
            && o.is_alive()
            && in_extension_window
            && !al.agreed_to_extend(self.id);
        Some(AllianceInfo {
            expires_at: al.expires_at,
            in_extension_window,
            my_player_agreed_to_extend: al.agreed_to_extend(self.id),
            other_agreed_to_extend: al.agreed_to_extend(other),
            can_extend,
        })
    }
    /// `canSendAllianceRequest(other)` (`:818-863`).
    pub fn can_send_alliance_request(&self, other: SmallId) -> bool {
        if self.st.cfg.disable_alliances() {
            return false;
        }
        if other == self.id {
            return false;
        }
        if self.is_disconnected() || self.other(other).is_disconnected() {
            return false;
        }
        if self.is_friendly(other, false) || !self.is_alive() {
            return false;
        }
        let has_pending =
            self.outgoing_alliance_requests().iter().any(|&r| self.st.alliance_request(r).recipient == other);
        if has_pending {
            return false;
        }
        let has_incoming =
            self.incoming_alliance_requests().iter().any(|&r| self.st.alliance_request(r).requestor == other);
        if has_incoming {
            return true;
        }
        let mut recent: Vec<ReqIdx> = self
            .data()
            .past_outgoing_alliance_requests
            .iter()
            .copied()
            .filter(|&r| self.st.alliance_request(r).recipient == other)
            .collect();
        js_sort_by(&mut recent, |a, b| {
            f64::from(self.st.alliance_request(*b).created_at) - f64::from(self.st.alliance_request(*a).created_at)
        });
        let Some(&first) = recent.first() else { return true };
        let delta = self.ticks() - self.st.alliance_request(first).created_at;
        delta >= self.st.cfg.alliance_request_cooldown()
    }
    /// `betrayals()`.
    pub fn betrayals(&self) -> u32 {
        self.data().betrayal_count
    }

    // ---- Targeting (`Game.ts:463-467`) ----
    /// `canTarget(other)` (`:994-1007`).
    pub fn can_target(&self, other: SmallId) -> bool {
        if self.id == other {
            return false;
        }
        if self.is_friendly(other, false) {
            return false;
        }
        for t in &self.data().targets {
            if self.ticks() - t.tick < self.st.cfg.target_cooldown() {
                return false;
            }
        }
        true
    }
    /// `targets()` (a filter of `targets_`, `:1014-1020`).
    pub fn targets(&self) -> Vec<SmallId> {
        let dur = self.st.cfg.target_duration();
        self.data().targets.iter().filter(|t| self.ticks() - t.tick < dur).map(|t| t.target).collect()
    }
    /// `transitiveTargets()` (`:1022-1028`): allies' targets, then own, deduplicated in first
    /// occurrence order (`[...new Set(ts)]`).
    pub fn transitive_targets(&self) -> Vec<SmallId> {
        let mut ts: Vec<SmallId> = Vec::new();
        for &a in self.alliances().as_slice() {
            let ally = self.st.alliance(a).other(self.id);
            ts.extend(self.other(ally).targets());
        }
        ts.extend(self.targets());
        let mut set = OwnerSet::new();
        for t in ts {
            set.add(t);
        }
        set.order
    }

    // ---- Communication (`Game.ts:469-474`) ----
    /// `canSendEmoji(recipient)` (`:1054-1073`).
    pub fn can_send_emoji(&self, recipient: EmojiRecipient) -> bool {
        if recipient == EmojiRecipient::Player(self.id) {
            return false;
        }
        let window = self.st.cfg.emoji_message_window();
        let recent = self
            .data()
            .outgoing_emojis
            .iter()
            .filter(|m| m.recipient_id == recipient && self.ticks() - m.created_at < window)
            .count();
        // Nations pace their own emojis with this check: one per window.
        let limit = if self.player_type() == PlayerType::Human { self.st.cfg.emoji_message_limit() } else { 1.0 };
        #[allow(clippy::cast_precision_loss)] // int: a small count
        let recent = recent as f64;
        recent < limit
    }
    /// `outgoingEmojis()` (a filter of `outgoingEmojis_`, sorted newest first, `:1044-1052`).
    pub fn outgoing_emojis(&self) -> Vec<EmojiMessage> {
        let dur = self.st.cfg.emoji_message_duration();
        let mut v: Vec<EmojiMessage> =
            self.data().outgoing_emojis.iter().filter(|e| self.ticks() - e.created_at < dur).cloned().collect();
        js_sort_by(&mut v, |a, b| f64::from(b.created_at) - f64::from(a.created_at));
        v
    }
    /// `canSendQuickChat(recipient)` (`:1075-1084`).
    pub fn can_send_quick_chat(&self, recipient: SmallId) -> bool {
        if recipient == self.id {
            return false;
        }
        match self.data().outgoing_quick_chats.get(&recipient) {
            None => true,
            Some(&last_sent_at) => self.ticks() - last_sent_at >= self.st.cfg.quick_chat_cooldown(),
        }
    }

    // ---- Donation (`Game.ts:476-485`) ----
    /// `canDonateGold(recipient)` (`:1090-1118`).
    pub fn can_donate_gold(&self, recipient: SmallId) -> bool {
        if recipient == self.id {
            return false;
        }
        let r = self.other(recipient);
        if !self.is_alive() || !r.is_alive() || !self.is_friendly(recipient, false) {
            return false;
        }
        if r.player_type() == PlayerType::Human && !self.st.cfg.donate_gold() {
            return false;
        }
        self.donation_cooldown_ok(recipient)
    }
    /// `canDonateTroops(recipient)` (`:1120-1148`).
    pub fn can_donate_troops(&self, recipient: SmallId) -> bool {
        if recipient == self.id {
            return false;
        }
        let r = self.other(recipient);
        if !self.is_alive() || !r.is_alive() || !self.is_friendly(recipient, false) {
            return false;
        }
        if r.player_type() == PlayerType::Human && !self.st.cfg.donate_troops() {
            return false;
        }
        self.donation_cooldown_ok(recipient)
    }
    /// The shared `for (const donation of this.sentDonations)` cooldown loop.
    fn donation_cooldown_ok(&self, recipient: SmallId) -> bool {
        for donation in &self.data().sent_donations {
            if donation.recipient == recipient && self.ticks() - donation.tick < self.st.cfg.donate_cooldown() {
                return false;
            }
        }
        true
    }
    /// `canDeleteUnit()` (`:1196-1201`).
    pub fn can_delete_unit(&self) -> bool {
        self.ticks() - self.data().last_delete_unit_tick >= self.st.cfg.delete_unit_cooldown()
    }
    /// `canEmbargoAll()` (`:1207-1224`).
    pub fn can_embargo_all(&self) -> bool {
        if self.ticks() - self.data().last_embargo_all_tick < self.st.cfg.embargo_all_cooldown() {
            return false;
        }
        for p in self.st.alive_player_ids() {
            let o = self.other(p);
            if o.player_id() == self.player_id() {
                continue;
            }
            if o.player_type() == PlayerType::Bot {
                continue;
            }
            if self.is_on_same_team(p) {
                continue;
            }
            if self.is_allied_with(p) {
                continue;
            }
            return true;
        }
        false
    }

    // ---- Embargo (`Game.ts:487-496`) ----
    pub fn has_embargo_against(&self, other: SmallId) -> bool {
        self.data().embargoes.contains_key(&other)
    }
    /// `embargoes.get(other.id())?.isTemporary === false`.
    pub fn has_manual_embargo_against(&self, other: SmallId) -> bool {
        self.data().embargoes.get(&other).is_some_and(|e| !e.is_temporary)
    }
    /// `tradingPartners()`: `mg.players().filter(other !== this && canTrade(other))`.
    pub fn trading_partners(&self) -> Vec<SmallId> {
        self.st.alive_player_ids().filter(|&o| o != self.id && self.can_trade(o)).collect()
    }
    /// `getEmbargoes()`: `[...this.embargoes.values()]`.
    pub fn get_embargoes(&self) -> Vec<Embargo> {
        self.data().embargoes.values().cloned().collect()
    }
    /// `canTrade(other)` (`:1238-1242`).
    pub fn can_trade(&self, other: SmallId) -> bool {
        let embargo = self.other(other).has_embargo_against(self.id) || self.has_embargo_against(other);
        !embargo && self.other(other).player_id() != self.player_id()
    }

    // ---- Attacking (`Game.ts:498-512`) ----
    /// `canAttack(tile)` (uses `bfsSearch` on the main map, `PlayerImpl.ts:1945`).
    pub fn can_attack(&self, sc: &mut Scratch, tile: TileRef) -> bool {
        let map = self.map();
        let owner = SmallId(map.owner_id(tile));
        if owner == self.id {
            return false;
        }
        if owner.is_player() && !self.can_attack_player(owner, false) {
            return false;
        }
        if !map.is_land(tile) || map.is_impassable(tile) {
            return false;
        }
        if map.has_owner(tile) {
            return self.shares_border_with(owner);
        }
        let mut found_border = false;
        let me = self.id.0;
        map.bfs_search(
            &mut sc.bfs_main,
            tile,
            // `andFN((gm, t) => !gm.hasOwner(t) && gm.isLand(t) && !gm.isImpassable(t),
            //        manhattanDistFN(tile, 200))`
            |gm, t| {
                !gm.has_owner(t)
                    && gm.is_land(t)
                    && !gm.is_impassable(t)
                    && f64::from(gm.manhattan_dist(tile, t)) <= 200.0
            },
            |t| {
                for n in map.neighbors(t) {
                    if map.owner_id(n) == me {
                        found_border = true;
                        return false;
                    }
                }
                true
            },
        );
        found_border
    }
    /// `canAttackPlayer(player, treatAFKFriendly = false)` (`:1917-1926`).
    pub fn can_attack_player(&self, other: SmallId, treat_afk_friendly: bool) -> bool {
        if self.player_type() != PlayerType::Human {
            // Only human attackers respect PVP immunity.
            return !self.is_friendly(other, treat_afk_friendly);
        }
        !self.other(other).is_immune() && !self.is_friendly(other, treat_afk_friendly)
    }
    /// `isImmune()` (`:1907-1915`).
    pub fn is_immune(&self) -> bool {
        match self.player_type() {
            PlayerType::Human => self.st.is_spawn_immunity_active(),
            PlayerType::Nation => self.st.is_nation_spawn_immunity_active(),
            PlayerType::Bot => false,
        }
    }
    /// `outgoingAttacks()`: the live array (§4.7).
    pub fn outgoing_attacks(&self) -> &'a JsArr<AttackRef> {
        &self.data().outgoing_attacks
    }
    /// `incomingAttacks()`: a fresh `filter` of `_incomingAttacks` (`PlayerImpl.ts:1903-1904`).
    pub fn incoming_attacks(&self) -> Vec<AttackRef> {
        self.data()
            .incoming_attacks
            .as_slice()
            .iter()
            .copied()
            .filter(|&a| self.other(self.st.attacks.get(a).attacker).is_alive())
            .collect()
    }

    // ---- Misc ----
    /// `playerProfile()` (view, `:1866-1877`). `Object.fromEntries` over integer-like
    /// smallID keys enumerates them in ascending order.
    pub fn player_profile(&self) -> PlayerProfile {
        let mut relations = self.all_relations_sorted();
        // Object.fromEntries: a later duplicate key would overwrite; smallIDs are unique.
        js_sort_by(&mut relations, |a, b| f64::from(a.0.0) - f64::from(b.0.0));
        PlayerProfile { relations, alliances: self.allies() }
    }
    /// `bestTransportShipSpawn(tile): TileRef | false` (`PlayerImpl.ts:1964`).
    pub fn best_transport_ship_spawn(&self, sc: &mut Scratch, tile: TileRef) -> Option<TileRef> {
        let mut rcx = Rcx::new(self.st, sc);
        best_shore_deployment_source(&mut rcx, self.id, tile)
    }
    /// `hash()` (legacy hash, on demand, ARCHITECTURE §5; `:1852-1857`).
    pub fn hash(&self) -> f64 {
        let units_hash = self.data().units.as_slice().iter().fold(0.0, |acc, &u| acc + self.st.unit(u).hash());
        simple_hash(self.player_id()) * (self.troops() + f64::from(self.num_tiles_owned())) + units_hash
    }
}

/// `Player extends PlayerLike` (`game/Game.ts:314`): config formulas run on `PlayerRef`.
impl PlayerLike for PlayerRef<'_> {
    fn id(&self) -> &str {
        self.player_id()
    }
    fn small_id(&self) -> SmallId {
        self.id
    }
    fn is_player(&self) -> bool {
        true
    }
    fn player_type(&self) -> PlayerType {
        PlayerRef::player_type(self)
    }
    fn is_lobby_creator(&self) -> bool {
        PlayerRef::is_lobby_creator(self)
    }
    fn num_tiles_owned(&self) -> f64 {
        f64::from(PlayerRef::num_tiles_owned(self))
    }
    fn troops(&self) -> f64 {
        PlayerRef::troops(self)
    }
    fn gold(&self) -> i64 {
        PlayerRef::gold(self)
    }
    fn units(&self, t: UnitType) -> impl ExactSizeIterator<Item = impl UnitLike + '_> + '_ {
        let st = self.st;
        self.units_of(&[t]).into_iter().map(move |u| UnitView { st, u })
    }
}

impl<'a, 'c> PlayerMut<'a, 'c> {
    /// The read view of the same player.
    #[inline]
    pub fn r(&self) -> PlayerRef<'_> {
        PlayerRef { st: &*self.cx.st, id: self.id }
    }
    #[inline]
    pub fn data(&mut self) -> &mut Player {
        self.cx.st.player_data_mut(self.id)
    }
    #[inline]
    fn ticks(&self) -> Tick {
        self.cx.st.ticks
    }

    /// Fill the exact per-player unit memos (`myUnitsMemo`, `myUnitCountMemo`,
    /// `myUnitsOwnedMemo`) at the current `_myUnitsVersion`. Readers answer from a valid memo
    /// or recompute; both give the same result because every `_units`/level/construction
    /// change bumps the version (ARCHITECTURE §3.3, G11; checked by `verify-memos`).
    #[allow(dead_code)] // rust-invariant: an opt-in cache fill (tests; ARCHITECTURE §11 perf work)
    pub(crate) fn fill_unit_memos(&mut self) {
        let st = &mut *self.cx.st;
        let idx = self.id.player_index();
        let (units_arena, players) = (&st.units, &mut st.players);
        let p = &mut players[idx]; // in-range: smallIDs come from add_player
        let version = p.my_units_version;
        type_memo_rebuild(&mut p.my_units_memo, p.units.as_slice(), version, |u| units_arena.get(u).unit_type);
        let mut count = [0i32; 16];
        let mut owned = [0i32; 16];
        for &u in p.units.as_slice() {
            let ud = units_arena.get(u);
            let i = ud.unit_type.index();
            count[i] += ud.level; // in-range: 16 types
            owned[i] += if ud.under_construction { 1 } else { ud.level };
        }
        for (m, c) in p.my_unit_count_memo.iter_mut().zip(count) {
            *m = Memo { version: Some(version), value: c };
        }
        for (m, o) in p.my_units_owned_memo.iter_mut().zip(owned) {
            *m = Memo { version: Some(version), value: o };
        }
    }
    /// `nearby()` that also stores the memo (`this.nearbyMemo = { version, waterVersion,
    /// result }`, `PlayerImpl.ts:622-624`).
    #[allow(dead_code)] // rust-invariant: an opt-in cache fill (ARCHITECTURE §11 perf work)
    pub(crate) fn nearby_memoized(&mut self) -> Vec<Owner> {
        let r = self.r();
        let version = r.st.territory_version;
        let water_version = r.st.map.water_version();
        let result = r.nearby();
        self.data().nearby_memo = Some(NearbyMemo { version, water_version, result: result.clone() });
        result
    }

    /// `setPlayerInfo(info)` (takeover hooks only at the pin; ported for completeness).
    pub fn set_player_info(&mut self, info: PlayerInfo) {
        let idx = self.r().data().info;
        std::sync::Arc::make_mut(&mut self.cx.st.infos)[idx.0 as usize] = info; // in-range: from add_player
    }
    /// `markTraitor()` (`:885-891`).
    pub fn mark_traitor(&mut self) {
        let t = self.ticks();
        let d = self.data();
        d.marked_traitor_tick = t;
        d.betrayal_count += 1; // Keep count for Nations too
        let id = self.id;
        self.cx.st.stats_cx().betray(id);
    }
    /// `markRotted()` (`:918-920`).
    pub fn mark_rotted(&mut self) {
        let t = self.ticks();
        self.data().rotted_at_tick = t;
    }
    /// `enterDoomsdayClock()` (`:907-911`).
    pub fn enter_doomsday_clock(&mut self) {
        let t = self.ticks();
        let d = self.data();
        if d.marked_doomsday_clock_tick < 0 {
            d.marked_doomsday_clock_tick = t;
        }
    }
    /// `clearDoomsdayClock()` (`:913-916`).
    pub fn clear_doomsday_clock(&mut self) {
        let d = self.data();
        d.marked_doomsday_clock_tick = -1;
        d.rotted_at_tick = -1;
    }
    /// `largestClusterBoundingBox = …` (written by `PlayerExecution`): `None` writes `null`.
    pub fn set_largest_cluster_bounding_box(&mut self, bbox: Option<BoundingBox>) {
        self.data().largest_cluster_bbox = match bbox {
            Some(b) => Tri::Some(b),
            None => Tri::Null,
        };
    }
    /// `markDisconnected(isDisconnected, snapshot?)` (`PlayerImpl.ts:1824`).
    pub fn mark_disconnected(&mut self, is_disconnected: bool, snapshot: Option<DisconnectSnapshot>) {
        self.data().is_disconnected = is_disconnected;
        if is_disconnected {
            if self.r().data().disconnect_snapshot.is_none() {
                let snap = match snapshot {
                    Some(s) => s,
                    None => {
                        let r = self.r();
                        // `team ? mg.teamTilesOwned(team) : 0`: an empty team string is falsy.
                        let team_tiles = match r.team() {
                            Some(team) if !team.is_empty() => r.st.team_tiles_owned(team),
                            _ => 0,
                        };
                        DisconnectSnapshot {
                            current_tick: r.st.ticks,
                            team_tiles,
                            total_land: r.st.total_land_tiles(),
                            was_alive: r.is_alive(),
                        }
                    }
                };
                self.data().disconnect_snapshot = Some(snap);
            }
        } else {
            self.data().disconnect_snapshot = None;
        }
    }
    /// `setSpawnTile(spawnTile)`.
    pub fn set_spawn_tile(&mut self, spawn_tile: TileRef) {
        self.data().spawn_tile = Some(spawn_tile);
    }

    // ---- Territory ----
    /// `conquer(tile)` → `GameImpl.conquer(this, tile)`.
    pub fn conquer(&mut self, tile: TileRef) {
        let id = self.id;
        self.cx.conquer(id, tile);
    }
    /// `relinquish(tile)` (`:723-728`): throws if `tile` is not this player's.
    pub fn relinquish(&mut self, tile: TileRef) {
        if self.cx.st.map.owner_id(tile) != self.id.0 {
            of_det::ts_throw!("Cannot relinquish tile not owned by this player");
        }
        self.cx.relinquish(tile);
    }

    // ---- Resources & troops ----
    /// `addGold(toAdd, tile?)` (`:1348-1364`); the `BonusEvent` update is view-only.
    pub fn add_gold(&mut self, to_add: Gold, tile: Option<TileRef>) {
        let _ = tile;
        let d = self.data();
        d.gold += to_add;
        d.gold_earned += to_add;
        debug_assert!(d.gold.unsigned_abs() < 1 << 62, "gold outside the port's i64 domain (ARCHITECTURE §7.2)");
    }
    /// `removeGold(toRemove): Gold` (returns the amount removed, `:1366-1373`).
    pub fn remove_gold(&mut self, to_remove: Gold) -> Gold {
        if to_remove <= 0 {
            return 0;
        }
        let d = self.data();
        let actual_removed = min_int(d.gold, to_remove);
        d.gold -= actual_removed;
        actual_removed
    }
    pub fn add_trade_gold(&mut self, to_add: Gold) {
        self.data().trade_gold += to_add;
    }
    pub fn add_train_gold(&mut self, to_add: Gold) {
        self.data().train_gold += to_add;
    }
    pub fn add_piracy_gold(&mut self, to_add: Gold) {
        self.data().piracy_gold += to_add;
    }
    /// `setTroops(troops: number)` (`toInt`, strict).
    pub fn set_troops(&mut self, troops: f64) {
        self.data().troops = to_int_strict(troops);
    }
    /// `addTroops(troops)` (`:1379-1385`).
    pub fn add_troops(&mut self, troops: f64) {
        if troops < 0.0 {
            self.remove_troops(-troops); // TS `-1 * troops`: the same IEEE value
            return;
        }
        let v = to_int_strict(troops);
        let d = self.data();
        d.troops += v;
        debug_assert!(d.troops.unsigned_abs() < 1 << 62, "troops outside the port's i64 domain (ARCHITECTURE §7.2)");
    }
    /// `removeTroops(troops): number` (returns the amount removed, `:1386-1393`).
    pub fn remove_troops(&mut self, troops: f64) -> f64 {
        if troops <= 0.0 {
            return 0.0;
        }
        let want = to_int_strict(troops);
        let d = self.data();
        let to_remove = min_int(d.troops, want);
        d.troops -= to_remove;
        to_remove as f64 // `Number(toRemove)`
    }

    // ---- Units ----
    /// `buildUnit(type, spawnTile, params)` (`PlayerImpl.ts:1402-1431`): allocates the unit,
    /// `addUnit`, the `toUpdate` shim (`:1427`), `recordUnitConstructed`.
    pub fn build_unit(&mut self, t: UnitType, spawn_tile: TileRef, params: UnitParams) -> UnitRef {
        if self.cx.st.cfg.is_unit_disabled(t) {
            let name = self.r().name().to_owned();
            of_det::ts_throw!(
                "Attempted to build disabled unit {} at tile {} by player {}",
                t.as_str(),
                spawn_tile,
                name
            );
        }
        let id = self.id;
        let cost = self.cx.st.cfg.unit_cost(t, &*self.cx.st, id, 0.0); // `:1413`, before the unit exists
        let unit_id = self.cx.next_unit_id();
        let unit = Unit::construct(self.cx, t, spawn_tile, unit_id, id, &params);
        let b = self.cx.st.units.alloc(unit);
        {
            let d = self.data();
            d.units.push(b);
            d.my_units_version += 1;
        }
        self.record_unit_constructed(t);
        self.remove_gold(cost);
        // `"troops" in params ? (params.troops ?? 0) : 0`
        self.remove_troops(params.troops.unwrap_or(0.0));
        self.cx.st.unit_touched(b); // `this.mg.addUpdate(b.toUpdate())` (`:1427`)
        self.cx.add_unit(b);
        b
    }
    /// `recordUnitConstructed(type)` (`:513-519`): the first construction appends the key
    /// (`Object.entries` order, Q-OBJKEYS).
    fn record_unit_constructed(&mut self, t: UnitType) {
        let d = self.data();
        match d.num_units_constructed.iter_mut().find(|(k, _)| *k == t) {
            Some((_, n)) => *n += 1,
            None => d.num_units_constructed.push((t, 1)),
        }
    }
    /// `upgradeUnit(unit)` (`:1506-1511`).
    pub fn upgrade_unit(&mut self, u: UnitRef) {
        let t = self.cx.st.unit_data(u).unit_type;
        let id = self.id;
        let cost = self.cx.st.cfg.unit_cost(t, &*self.cx.st, id, 0.0);
        self.remove_gold(cost);
        self.cx.unit_mut(u).increase_level();
        self.record_unit_constructed(t);
    }
    /// `captureUnit(unit)` (`:1395-1400`): throws if this player already owns it.
    pub fn capture_unit(&mut self, u: UnitRef) {
        if self.cx.st.unit_data(u).owner == self.id {
            let me = self.r().ts_to_string();
            let unit = self.cx.st.unit(u).ts_to_string();
            of_det::ts_throw!("Cannot capture unit, {me} already owns {unit}");
        }
        let id = self.id;
        self.cx.unit_mut(u).set_owner(id);
    }

    // ---- Relations & diplomacy ----
    /// `updateRelation(other, delta)` (`:973-980`).
    pub fn update_relation(&mut self, other: SmallId, delta: f64) {
        if other == self.id {
            let s = self.r().ts_to_string();
            of_det::ts_throw!("cannot update relation with self: {s}");
        }
        let d = self.data();
        let relation = d.relations.get(&other).copied().unwrap_or(0.0);
        let new_relation = within(relation + delta, -100.0, 100.0);
        d.relations.insert(other, new_relation);
    }
    /// `decayRelations()` (`:982-992`): `forEach` in insertion order; `set` on an existing key
    /// keeps its position and the loop adds/deletes nothing, so a key snapshot is exact.
    pub fn decay_relations(&mut self) {
        let d = self.data();
        let keys: Vec<SmallId> = d.relations.keys().copied().collect();
        for p in keys {
            let Some(&r0) = d.relations.get(&p) else { continue };
            let sign = -js::sign(r0); // TS `-1 * Math.sign(r)`: the same IEEE value
            let delta = 0.05;
            let mut r = r0 + sign * delta;
            if js::abs(r) < delta * 2.0 {
                r = 0.0;
            }
            d.relations.insert(p, r);
        }
    }
    /// `breakAlliance(alliance)` → `GameImpl.breakAlliance(this, alliance)`.
    pub fn break_alliance(&mut self, alliance: AllianceId) {
        let id = self.id;
        self.cx.break_alliance(id, alliance);
    }
    /// `removeAllAlliances()` → `GameImpl.removeAlliancesByPlayerSilently(this)`.
    pub fn remove_all_alliances(&mut self) {
        let id = self.id;
        self.cx.remove_alliances_by_player_silently(id);
    }
    /// `createAllianceRequest(recipient): AllianceRequest | null` (`:935-940`).
    pub fn create_alliance_request(&mut self, recipient: SmallId) -> Option<ReqIdx> {
        if self.r().is_allied_with(recipient) {
            of_det::ts_throw!("cannot create alliance request, already allies");
        }
        let id = self.id;
        self.cx.create_alliance_request(id, recipient)
    }

    // ---- Targeting / communication / donation ----
    /// `target(other)` (`:1009-1012`).
    pub fn target(&mut self, other: SmallId) {
        let tick = self.ticks();
        self.data().targets.push(Target { tick, target: other });
        let id = self.id;
        self.cx.target(id, other);
    }
    /// `sendEmoji(recipient, emoji)`; throws on a self-emoji (`PlayerImpl.ts:1030-1043`).
    pub fn send_emoji(&mut self, recipient: EmojiRecipient, emoji: &str) {
        if recipient == EmojiRecipient::Player(self.id) {
            let s = self.r().ts_to_string();
            of_det::ts_throw!("Cannot send emoji to oneself: {s}");
        }
        let msg = EmojiMessage {
            message: emoji.to_owned(),
            sender_id: self.id,
            recipient_id: recipient,
            created_at: self.ticks(),
        };
        self.data().outgoing_emojis.push(msg);
        // `mg.sendEmojiUpdate(msg)` is view-only (`GameImpl.ts`, §5).
    }
    /// `recordQuickChat(recipient)` (`:1086-1088`).
    pub fn record_quick_chat(&mut self, recipient: SmallId) {
        let t = self.ticks();
        self.data().outgoing_quick_chats.insert(recipient, t);
    }
    /// `donateTroops(recipient, troops)` (`:1150-1168`).
    pub fn donate_troops(&mut self, recipient: SmallId, troops: f64) -> bool {
        if recipient == self.id {
            return false;
        }
        if troops <= 0.0 {
            return false;
        }
        let removed = self.remove_troops(troops);
        if removed == 0.0 {
            return false;
        }
        self.cx.player_mut(recipient).add_troops(removed);
        let tick = self.ticks();
        self.data().sent_donations.push(Donation { recipient, tick });
        // The `DonateEvent` update is view-only, but its `amount: BigInt(removed)` argument is
        // evaluated (it cannot throw: `removed` is a `Number(bigint)`).
        let _ = from_number_strict(removed);
        true
    }
    /// `donateGold(recipient, gold)` (`:1170-1194`).
    pub fn donate_gold(&mut self, recipient: SmallId, gold: Gold) -> bool {
        if recipient == self.id {
            return false;
        }
        if gold <= 0 {
            return false;
        }
        let removed = self.remove_gold(gold);
        if removed == 0 {
            return false;
        }
        // Read before addGold: the recipient's balance when the gold landed.
        let recipient_gold_before = self.cx.st.player(recipient).gold();
        self.cx.player_mut(recipient).add_gold(removed, None);
        self.cx.st.stats_cx().gold_donation_received(
            recipient,
            StatNum::Big(removed),
            StatNum::Big(recipient_gold_before),
        );
        let tick = self.ticks();
        self.data().sent_donations.push(Donation { recipient, tick });
        true
    }
    /// `recordDeleteUnit()` (`:1203-1205`).
    pub fn record_delete_unit(&mut self) {
        let t = self.ticks();
        self.data().last_delete_unit_tick = t;
    }
    /// `recordEmbargoAll()` (`:1226-1228`).
    pub fn record_embargo_all(&mut self) {
        let t = self.ticks();
        self.data().last_embargo_all_tick = t;
    }

    // ---- Embargo ----
    /// `addEmbargo(other, isTemporary)` (`:1248-1264`): a manual embargo is never
    /// overwritten; `set` on an existing key keeps its Map position.
    pub fn add_embargo(&mut self, other: SmallId, is_temporary: bool) {
        let t = self.ticks();
        let d = self.data();
        if let Some(e) = d.embargoes.get(&other)
            && !e.is_temporary
        {
            return;
        }
        // The `EmbargoEvent` update is view-only.
        d.embargoes.insert(other, Embargo { target: other, created_at: t, is_temporary });
    }
    /// `stopEmbargo(other)` (`:1266-1274`).
    pub fn stop_embargo(&mut self, other: SmallId) {
        self.data().embargoes.remove(&other);
    }
    /// `endTemporaryEmbargo(other)` (`:1276-1281`).
    pub fn end_temporary_embargo(&mut self, other: SmallId) {
        if let Some(e) = self.r().data().embargoes.get(&other)
            && !e.is_temporary
        {
            return;
        }
        self.stop_embargo(other);
    }

    // ---- Attacking ----
    /// `createAttack(target, troops, sourceTile, border)` (`PlayerImpl.ts:1879-1898`): draws
    /// `nextID()` from this player's PRNG, allocates the attack, pushes it to both lists.
    pub fn create_attack(
        &mut self,
        target: Owner,
        troops: f64,
        source_tile: Option<TileRef>,
        border: OrderedTileSet,
    ) -> AttackRef {
        let id_str = self.data().rng.next_id();
        #[allow(clippy::expect_used)] // rust-invariant: nextID() always yields 8 base-36 chars
        let id = AttackId::parse(&id_str).expect("nextID() is a valid attack id");
        let attack = Attack::new(id, target, self.id, troops, source_tile, border);
        let a = self.cx.st.attacks.alloc(attack);
        self.data().outgoing_attacks.push(a);
        if target.is_player() {
            self.cx.st.player_data_mut(target).incoming_attacks.push(a);
        }
        a
    }
    /// The first outgoing attack whose `id()` equals `attack_id` (`_outgoingAttacks.find`).
    fn find_outgoing_attack(&self, attack_id: &str) -> Option<AttackRef> {
        // `padStart(8, "0")` makes the string form unique, so comparing parsed ids is exact;
        // an unparseable string matches no attack, as in TS.
        let want = AttackId::parse(attack_id)?;
        let st = &*self.cx.st;
        st.player_data(self.id).outgoing_attacks.as_slice().iter().copied().find(|&a| st.attacks.get(a).id == want)
    }
    /// `orderRetreat(attackID)` (`:707-714`); a missing attack is a `console.warn` + return.
    pub fn order_retreat(&mut self, attack_id: &str) {
        let Some(a) = self.find_outgoing_attack(attack_id) else { return };
        self.cx.st.attack_mut(a).order_retreat();
    }
    /// `executeRetreat(attackID)` (`:715-722`).
    pub fn execute_retreat(&mut self, attack_id: &str) {
        let Some(a) = self.find_outgoing_attack(attack_id) else { return };
        self.cx.st.attack_mut(a).execute_retreat();
    }
}

// ---------------------------------------------------------------------------------------
// CS v1 (`Player v1`, `PlayerImpl.ts:1968-2038`), fields in dump order.
// ---------------------------------------------------------------------------------------

/// `playerInfoData(info)` (`snapshot/CommonSchemas.ts:24-36`).
pub(crate) fn canon_player_info(info: &PlayerInfo) -> Cs {
    let opt_str = |s: &Option<String>| s.as_ref().map_or(Cs::Null, |s| Cs::Str(s.clone()));
    Cs::Obj(vec![
        ("name".to_owned(), Cs::Str(info.name.clone())),
        ("playerType".to_owned(), Cs::Str(info.player_type.as_str().to_owned())),
        ("clientID".to_owned(), opt_str(&info.client_id)),
        ("id".to_owned(), Cs::Str(info.id.to_string())),
        ("isLobbyCreator".to_owned(), Cs::Bool(info.is_lobby_creator)),
        ("clanTag".to_owned(), opt_str(&info.clan_tag)),
        ("friends".to_owned(), Cs::Arr(info.friends.iter().map(|f| Cs::Str(f.clone())).collect())),
        ("teamIndex".to_owned(), info.team_index.map_or(Cs::Null, |i| Cs::Int(i64::from(i)))),
        ("nationFlag".to_owned(), opt_str(&info.nation_flag)),
    ])
}

impl Canon for Player {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        let st = cx.st;
        let mut o: Vec<(String, Cs)> = Vec::with_capacity(40);
        macro_rules! put {
            ($k:literal, $v:expr) => {
                o.push(($k.to_owned(), $v))
            };
        }
        put!("smallID", Cs::Int(i64::from(self.small_id.0)));
        put!("info", canon_player_info(st.info(self.info)));
        put!("team", self.team.as_ref().map_or(Cs::Null, |t| Cs::Str(t.clone())));
        put!("lastTileChange", self.last_tile_change.canon(cx));
        put!("tileChangeVersion", self.tile_change_version.canon(cx));
        put!("random", Cs::Arr(self.rng.get_state().iter().map(|w| Cs::Int(i64::from(*w))).collect()));
        put!("gold", big(self.gold));
        put!("troops", big(self.troops));
        put!("tradeGold", big(self.trade_gold));
        put!("trainGold", big(self.train_gold));
        put!("piracyGold", big(self.piracy_gold));
        put!("goldEarned", big(self.gold_earned));
        put!("markedTraitorTick", self.marked_traitor_tick.canon(cx));
        put!("markedDoomsdayClockTick", self.marked_doomsday_clock_tick.canon(cx));
        put!("rottedAtTick", self.rotted_at_tick.canon(cx));
        put!("betrayalCount", self.betrayal_count.canon(cx));
        put!(
            "embargoes",
            Cs::Arr(
                self.embargoes
                    .values()
                    .map(|e| {
                        Cs::Obj(vec![
                            ("target".to_owned(), Cs::Int(i64::from(e.target.0))),
                            ("createdAt".to_owned(), Cs::Int(i64::from(e.created_at))),
                            ("isTemporary".to_owned(), Cs::Bool(e.is_temporary)),
                        ])
                    })
                    .collect()
            )
        );
        put!("tiles", Cs::Typed(Typed::U32(self.tiles.to_vec())));
        put!("borderTiles", Cs::Typed(Typed::U32(self.border_tiles.to_vec())));
        put!("units", self.units.as_slice().canon(cx));
        put!("unitsVersion", self.my_units_version.canon(cx));
        put!(
            "pastOutgoingAllianceRequests",
            // ReqIdx / AllianceId leaves (P13): key + queue the D row (merge1: the hand-written
            // keys did not queue, so D lacked every request/alliance referenced only from P).
            Cs::Arr(self.past_outgoing_alliance_requests.iter().map(|r| r.canon(cx)).collect())
        );
        put!("expiredAlliances", Cs::Arr(self.expired_alliances.iter().map(|a| a.canon(cx)).collect()));
        put!(
            "targets",
            Cs::Arr(
                self.targets
                    .iter()
                    .map(|t| {
                        Cs::Obj(vec![
                            ("tick".to_owned(), Cs::Int(i64::from(t.tick))),
                            ("target".to_owned(), Cs::Int(i64::from(t.target.0))),
                        ])
                    })
                    .collect()
            )
        );
        put!(
            "outgoingEmojis",
            Cs::Arr(
                self.outgoing_emojis
                    .iter()
                    .map(|e| {
                        Cs::Obj(vec![
                            ("message".to_owned(), Cs::Str(e.message.clone())),
                            ("senderID".to_owned(), Cs::Int(i64::from(e.sender_id.0))),
                            (
                                "recipientID".to_owned(),
                                match e.recipient_id {
                                    EmojiRecipient::Player(p) => Cs::Int(i64::from(p.0)),
                                    EmojiRecipient::AllPlayers => Cs::Str("AllPlayers".to_owned()),
                                },
                            ),
                            ("createdAt".to_owned(), Cs::Int(i64::from(e.created_at))),
                        ])
                    })
                    .collect()
            )
        );
        put!(
            "outgoingQuickChats",
            Cs::Arr(
                self.outgoing_quick_chats
                    .iter()
                    .map(|(k, v)| Cs::Arr(vec![Cs::Int(i64::from(k.0)), Cs::Int(i64::from(*v))]))
                    .collect()
            )
        );
        put!(
            "sentDonations",
            Cs::Arr(
                self.sent_donations
                    .iter()
                    .map(|d| {
                        Cs::Obj(vec![
                            ("recipient".to_owned(), Cs::Int(i64::from(d.recipient.0))),
                            ("tick".to_owned(), Cs::Int(i64::from(d.tick))),
                        ])
                    })
                    .collect()
            )
        );
        put!(
            "relations",
            Cs::Arr(self.relations.iter().map(|(k, v)| Cs::Arr(vec![Cs::Int(i64::from(k.0)), Cs::Num(*v)])).collect())
        );
        put!("lastDeleteUnitTick", self.last_delete_unit_tick.canon(cx));
        put!("lastEmbargoAllTick", self.last_embargo_all_tick.canon(cx));
        put!("incomingAttacks", self.incoming_attacks.as_slice().canon(cx));
        put!("outgoingAttacks", self.outgoing_attacks.as_slice().canon(cx));
        put!("outgoingLandAttacks", self.outgoing_land_attacks.canon(cx));
        put!("alliances", Cs::Arr(self.alliances.as_slice().iter().map(|a| a.canon(cx)).collect()));
        put!("spawnTile", self.spawn_tile.map_or(Cs::Null, |t| Cs::Int(i64::from(t))));
        put!("isDisconnected", Cs::Bool(self.is_disconnected));
        put!(
            "disconnectSnapshot",
            match &self.disconnect_snapshot {
                Some(s) => Cs::Obj(vec![
                    ("currentTick".to_owned(), Cs::Int(i64::from(s.current_tick))),
                    ("teamTiles".to_owned(), Cs::Int(i64::from(s.team_tiles))),
                    ("totalLand".to_owned(), Cs::Int(i64::from(s.total_land))),
                    ("wasAlive".to_owned(), Cs::Bool(s.was_alive)),
                ]),
                None => Cs::Null,
            }
        );
        // `box ? {minX, minY, maxX, maxY} : box` keeps null apart from undefined (`:2024-2033`).
        put!(
            "largestClusterBoundingBox",
            match &self.largest_cluster_bbox {
                Tri::Absent => Cs::Absent,
                Tri::Null => Cs::Null,
                Tri::Some(b) => Cs::Obj(vec![
                    ("minX".to_owned(), Cs::Num(b.min.x)),
                    ("minY".to_owned(), Cs::Num(b.min.y)),
                    ("maxX".to_owned(), Cs::Num(b.max.x)),
                    ("maxY".to_owned(), Cs::Num(b.max.y)),
                ]),
            }
        );
        put!(
            "numUnitsConstructed",
            Cs::Arr(
                self.num_units_constructed
                    .iter()
                    .map(|(t, n)| Cs::Arr(vec![Cs::Str(t.as_str().to_owned()), Cs::Int(i64::from(*n))]))
                    .collect()
            )
        );
        Cs::Obj(o)
    }
}

#[cfg(test)]
mod tests;
