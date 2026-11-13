//! `State`: the data of `GameImpl` (`engine/src/game/GameImpl.ts:133-180`) and the TS `Game`
//! interface (`engine/src/game/Game.ts:512-699`). Owner: **P1**. ARCHITECTURE §3.1.
//!
//! Field order: the `Game v1` snapshot record (dump order), then the non-snapshot fields of
//! ARCHITECTURE §3.1, each marked `non-snapshot`. `execs` (the live list) is not here: it is
//! `Game.execs.live` (disjoint from `State` while an execution runs, §4.2).
//!
//! Method placement (FILE_MAP §2: every mutating cross-package method takes `cx: &mut Ctx`):
//! - pure readers: `impl State` (`cx.st.ticks()`);
//! - readers that need scratch: `impl Rcx`;
//! - every mutator: `impl Ctx` (`cx.add_player(..)`), so it can reach scratch and the journal.
//!
//! `GameMap` forwards that `GameImpl` passes straight through (`isLand`, `x`, `neighbors4`, …,
//! `GameImpl.ts:1455-1597`) are called on `cx.st.map` directly. The `GameImpl` versions with
//! extra logic (`setFallout`, `setWater`, `setOwnerID`, `queueWaterConversion`,
//! `GameImpl.ts:295-333,1524-1527`) are `Ctx` methods here and must be used instead.
//!
//! Dropped view work (ARCHITECTURE §5): `updates`, `addUpdate`, `display*`, `drainPacked*`,
//! `recordMotionPlan` records, `queueViewResync`, `_nukeImpactQueue`; takeover hooks
//! (`takeoverPlayer`, `convertHumanToNation`, `clear*Human*`, `applySingleplayerConfig`,
//! `removeExecution`, `executions`) are out of scope (§1).

use crate::StateClone;
use crate::api::game_config::GameStartPlayer;
use crate::api::game_config::TeamCountConfig;
use crate::api::types::{Cell, GameMode, Nation, PlayerInfo, PlayerType, SpawnArea, Team, Tick, UnitType};
use crate::arena::Arena;
use crate::config::EngineConfig;
use crate::ctx::{Ctx, Rcx, ts_throw};
use crate::exec::{Exec, ExecCmd};
use crate::ids::{AllianceId, AttackRef, ExecId, Owner, PlayerInfoIdx, SmallId, SmallIdKey, TileRef, UnitRef};
use crate::jsarr::JsArr;
use crate::jsorder::{JsMap, JsSet, LookupMap};
use crate::map::unit_grid::NearbyUnit;
use crate::map::{GameMap, MapStatics, UnitGrid};
use crate::plain_copy;
use crate::rail::RailNetwork;
use crate::state::alliance::Alliance;
use crate::state::alliance_request::AllianceRequest;
use crate::state::attack::Attack;
use crate::state::memo::{Memo, TypeMemo};
use crate::state::player::Player;
use crate::state::shared_water_cache::SharedWaterCache;
use crate::state::stats::{AllianceEndCounter, StatNum, Stats};
use crate::state::unit::Unit;
use crate::tileset::PlayerTileOrder;
use crate::water::WaterManager;
use std::sync::Arc;

/// `_winner: Player | Team | null` (`GameImpl.ts:175`; PORTING_GUIDE §10).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Winner {
    #[default]
    None,
    Player(SmallId),
    Team(Team),
}
impl crate::plain::PlainState for Winner {}
impl crate::canon::Refs for Winner {
    fn visit_refs(&self, v: &mut dyn crate::canon::RefVisitor) {
        if let Winner::Player(p) = self {
            v.visit_player(*p);
        }
    }
}

/// CS G `winner`: `{ player }` | `{ team }` | `null` (`GameImpl.ts:1675-1680`).
impl crate::canon::Canon for Winner {
    fn canon(&self, cx: &mut crate::canon::CanonCx<'_>) -> crate::canon::Cs {
        use crate::canon::Cs;
        match self {
            Winner::None => Cs::Null,
            Winner::Player(p) => Cs::Obj(vec![("player".to_owned(), p.canon(cx))]),
            Winner::Team(t) => Cs::Obj(vec![("team".to_owned(), Cs::Str(t.clone()))]),
        }
    }
}

/// `shipStaggers = { tradeShip: 0, transportShip: 0 }` (`GameImpl.ts:171`).
#[derive(StateClone, Debug, Default, PartialEq, Eq)]
pub struct ShipStaggers {
    pub trade_ship: i32,
    pub transport_ship: i32,
}

crate::impl_canon_record!(ShipStaggers { "tradeShip" => trade_ship, "transportShip" => transport_ship });

/// `nextShipStagger(kind)` argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShipKind {
    TradeShip,
    TransportShip,
}
plain_copy!(ShipKind);

/// Per-step outputs for embedders (ARCHITECTURE §10); cleared at step start, never read by the sim.
#[derive(StateClone, Debug, Default, PartialEq, Eq)]
pub struct StepEventsBuf {
    /// Set when `setWinner` ran this step.
    pub winner_set: Option<Winner>,
    /// Players killed (0 tiles) this step, in order.
    pub killed: Vec<SmallId>,
}

/// All simulation state except the execution arenas.
#[derive(StateClone, Debug, Default)]
pub struct State {
    // ---- `Game v1` snapshot record, dump order ----
    /// `_ticks` (`:133`).
    pub ticks: Tick,
    /// `startTick` (`:134`): `None` = spawn phase (`:536-552`).
    pub start_tick: Option<Tick>,
    /// `_humans` (ctor `:182-190`).
    pub humans: Arc<[PlayerInfo]>,
    /// `_nations`.
    pub nations: Arc<[Nation]>,
    /// `_players` Map == `_playersBySmallID` (index = smallID - 1; §3.1: Map order == smallID
    /// order because `add_player` asserts every id is new).
    pub players: Vec<Player>,
    // `execs` → `Game.execs.live`.
    /// `unInitExecs` (`:136`); order is state.
    pub uninit: Vec<ExecId>,
    /// `allianceRequests` (`:146`): filter-reassigned (`:469, :507`, §4.7).
    pub alliance_requests: JsArr<crate::ids::ReqIdx>,
    /// `nextPlayerID = 1` (`:148`). // int: counter.
    pub next_player_id: u32,
    /// `_nextUnitID = 1` (`:149`).
    pub next_unit_id: u32,
    /// `nextAllianceID = 0` (`:168`).
    pub next_alliance_id: u32,
    /// `units` (snapshot: every unit row); the arena of all units, live and TS-deleted-but-
    /// referenced (§3.2).
    pub units: Arena<Unit, UnitRef>,
    /// `planDrivenUnitIds = new Set<number>()` (`:159`): gates the `toUpdate` shim (§5).
    pub plan_driven_unit_ids: JsSet<u32>,
    /// `unitGrid` (`:160`).
    pub unit_grid: UnitGrid,
    /// `playerTeams` (`:163`).
    pub player_teams: Vec<Team>,
    /// `botTeam = ColoredTeams.Bot` (`:164`).
    pub bot_team: Team,
    /// `_isPaused` (`:174`).
    pub is_paused: bool,
    /// `_winner` (`:175`).
    pub winner: Winner,
    /// `_unitsVersion` (`:1345`). // int: counter.
    pub units_version: u32,
    /// `_territoryVersion` (`:1378`).
    pub territory_version: u32,
    pub ship_staggers: ShipStaggers,
    /// `_nationMirvTargets: Map<PlayerID, Tick>` (`:172`), keyed by smallID.
    pub nation_mirv_targets: JsMap<SmallIdKey, Tick>,
    /// `_mirvsLaunched` (`:170`).
    pub mirvs_launched: u32,
    /// `_railNetwork` (`:165`).
    pub rail: RailNetwork,
    /// `_waterManager` (`:176`), incl. chain slots + memo + segment visibility (§3.8).
    pub water: WaterManager,
    /// `_sharedWaterCache` (`:177`).
    pub shared_water: SharedWaterCache,
    /// `mg.stats()` (§3.10).
    pub stats: Stats,

    // ---- non-snapshot ----
    /// non-snapshot: `_config` (shared).
    pub cfg: Arc<EngineConfig>,
    /// non-snapshot: map statics (dims, pristine terrain, land index, nations, graph0, …).
    pub statics: Arc<MapStatics>,
    /// non-snapshot: every `PlayerInfo`, indexed by `PlayerInfoIdx` (COW-appended by `add_player`).
    pub infos: Arc<Vec<PlayerInfo>>,
    /// non-snapshot here: the main map (`_map`) is its own `GameMap v1` record (CS M.map).
    pub map: GameMap,
    /// non-snapshot here: the minimap (`miniGameMap`), CS M.miniMap; state buffer never written
    /// (§3.6.1).
    pub mini: GameMap,
    /// non-snapshot: attack storage (rows in CS A by (attacker, id)).
    pub attacks: Arena<Attack, AttackRef>,
    /// non-snapshot: alliance storage, index == TS id.
    pub alliances: Vec<Alliance>,
    /// non-snapshot: alliance-request storage, index = creation order.
    pub alliance_reqs: Vec<AllianceRequest>,
    #[refs(gc_only)]
    /// non-snapshot: `_unitMap` (`:161`), lookup-only: `unit(id)` only (iteration order would be
    /// ascending id, §3.2).
    pub unit_by_id: LookupMap<u32, UnitRef>,
    /// non-snapshot (derived, cloned): `_structureVersion` (`:1358`).
    pub structure_version: u32,
    /// non-snapshot: positions of every land tile in its owner's `_tiles` list (§3.6.2).
    /// Written only through [`State::pos_tiles_set`].
    pos_tiles: PlayerTileOrder,
    /// non-snapshot: same for `_borderTiles`. Written only through [`State::pos_border_set`].
    pos_border: PlayerTileOrder,
    /// non-snapshot: `add_execution` target; empty at tick boundaries (§4.2).
    #[refs(skip)]
    pub exec_inbox: Vec<Exec>,
    /// non-snapshot: MIRV commands; empty at tick boundaries (§4.3).
    #[refs(skip)]
    pub exec_cmds: Vec<ExecCmd>,
    /// non-snapshot (derived, cloned): `unitsByTypeMemo` (`:349-392`).
    #[refs(gc_only)]
    pub units_by_type_memo: TypeMemo,
    /// non-snapshot (derived, cloned): `unitCountMemo` (`:418-427`).
    #[refs(gc_only)]
    pub unit_count_memo: [Memo<i32>; 16],
    /// non-snapshot: per-step outputs (§10).
    #[refs(skip)]
    pub events: StepEventsBuf,
    /// non-snapshot (derived, cloned): exact memo of `closestShoreByWater` (P3 perf, t066).
    #[refs(skip)]
    pub shore_memo: crate::path::spatial_query::ShoreMemo,
    /// non-snapshot (t409 H5, not cloned): per-tile component labels (`map::conn4`).
    #[refs(skip)]
    pub comp_labels: crate::map::conn4::CompLabels,
}

impl State {
    // ---- frozen write accessors of the position arrays (ARCHITECTURE §8.5) ----

    /// Test fixtures: size both position indexes for `n` land tiles.
    #[cfg(test)]
    pub(crate) fn test_reset_tile_order(&mut self, n: usize) {
        self.pos_tiles = PlayerTileOrder::new(n);
        self.pos_border = PlayerTileOrder::new(n);
    }

    /// The `_tiles` position index (read).
    #[inline]
    pub fn pos_tiles(&self) -> &PlayerTileOrder {
        &self.pos_tiles
    }
    /// The `_borderTiles` position index (read).
    #[inline]
    pub fn pos_border(&self) -> &PlayerTileOrder {
        &self.pos_border
    }
    /// **The only writer of `pos_tiles`** (frozen at S0): position `p` for land index `li`.
    #[inline]
    pub fn pos_tiles_set(&mut self, li: u32, p: u32) {
        self.pos_tiles.pos_set(li, p);
    }
    /// **The only writer of `pos_border`** (frozen at S0).
    #[inline]
    pub fn pos_border_set(&mut self, li: u32, p: u32) {
        self.pos_border.pos_set(li, p);
    }
    /// Split borrow for `TileSet` ops on player `p`'s `_tiles`: (position index, the player's
    /// header, the land index). Writes still go through `PlayerTileOrder::pos_set`.
    pub fn tiles_parts_mut(&mut self, p: SmallId) -> (&mut PlayerTileOrder, &mut crate::tileset::TileListHdr, &[u32]) {
        let pl = &mut self.players[p.player_index()]; // in-range: smallIDs come from add_player
        (&mut self.pos_tiles, &mut pl.tiles, &self.statics.land_index)
    }
    /// Same for `_borderTiles`.
    pub fn border_parts_mut(&mut self, p: SmallId) -> (&mut PlayerTileOrder, &mut crate::tileset::TileListHdr, &[u32]) {
        let pl = &mut self.players[p.player_index()]; // in-range: smallIDs come from add_player
        (&mut self.pos_border, &mut pl.border_tiles, &self.statics.land_index)
    }

    // ---- Map & dimensions (`Game.ts:513-536`) ----
    /// `isOnMap(cell)` (`GameImpl.ts:948-955`): compares the JS-number cell against
    /// `_width`/`_height` (so NaN or a fraction behaves as in TS).
    pub fn is_on_map(&self, cell: Cell) -> bool {
        let (w, h) = (f64::from(self.width()), f64::from(self.height()));
        cell.x >= 0.0 && cell.x < w && cell.y >= 0.0 && cell.y < h
    }
    /// `width()` (`:1470-1472`) = `_map.width()`: the loaded map's width, fixed for the game
    /// (`MapStatics.width` is the same value; GameMap dimensions never change).
    pub fn width(&self) -> u32 {
        self.statics.width
    }
    /// `height()` (`:1473-1475`).
    pub fn height(&self) -> u32 {
        self.statics.height
    }
    /// `neighborsWithDiag(tile)` (`GameImpl.ts:957-978`): `dx` outer, `dy` inner, centre
    /// skipped, in-bounds only.
    pub fn neighbors_with_diag(&self, tile: TileRef) -> Vec<TileRef> {
        // int: coordinates are small non-negative integers; i64 cannot overflow here.
        let x = i64::from(self.map.x(tile));
        let y = i64::from(self.map.y(tile));
        let (w, h) = (i64::from(self.width()), i64::from(self.height()));
        let mut ns = Vec::with_capacity(8);
        for dx in -1..=1i64 {
            for dy in -1..=1i64 {
                if dx == 0 && dy == 0 {
                    continue; // Skip the center tile
                }
                let new_x = x + dx;
                let new_y = y + dy;
                if new_x >= 0 && new_x < w && new_y >= 0 && new_y < h {
                    ns.push(self.map.ref_(new_x as f64, new_y as f64));
                }
            }
        }
        ns
    }

    // ---- Player management (`Game.ts:538-551`) ----
    /// `_players.get(id)`: `_players` is keyed by PlayerID and `add_player` asserts ids are
    /// unique, so the first match in `_playersBySmallID` order is the Map entry (§3.1).
    pub fn find_player(&self, id: &str) -> Option<SmallId> {
        self.players.iter().find(|p| self.info(p.info).id == id).map(|p| p.small_id)
    }
    /// `player(id)` (`GameImpl.ts:931-937`): throws `Player with id <id> not found`.
    pub fn player_by_id(&self, id: &str) -> SmallId {
        match self.find_player(id) {
            Some(p) => p,
            None => ts_throw!("Player with id {id} not found"),
        }
    }
    /// `hasPlayer(id)` (`:520-522`).
    pub fn has_player(&self, id: &str) -> bool {
        self.find_player(id).is_some()
    }
    /// `players()` (`:775-777`): `_playersBySmallID.filter((p) => p.isAlive())`, a new array;
    /// `isAlive()` is `_tiles.size > 0` (`PlayerImpl.ts:740-742`).
    pub fn players(&self) -> Vec<SmallId> {
        self.players.iter().filter(|p| p.tiles.size() > 0).map(|p| p.small_id).collect()
    }
    /// `allPlayers()` (`:779-781`): `_playersBySmallID.slice()`.
    pub fn all_players(&self) -> Vec<SmallId> {
        self.players.iter().map(|p| p.small_id).collect()
    }
    /// `playerByClientID(id)` (`:939-946`): the first player in `_players` order whose
    /// `clientID()` equals `id` (`PlayerImpl.clientID()` = `playerInfo.clientID`).
    pub fn player_by_client_id(&self, id: &str) -> Option<SmallId> {
        self.players.iter().find(|p| self.info(p.info).client_id.as_deref() == Some(id)).map(|p| p.small_id)
    }
    /// `playerBySmallID(id)` (`:272-277`): 0 = TerraNullius, else `_playersBySmallID[id - 1]`.
    /// `Owner` is the smallID itself. TS returns `undefined` past the end; every in-sim caller
    /// passes a tile owner id (`GameImpl.ts:269`, `PlayerImpl.ts:639,689`,
    /// `PlayerExecution.ts:687,1010`, `NationAttackBehavior.ts:174`, `BotAnnexation.ts:171`,
    /// `NukeExecution.ts:177`), which always names an added player, so that case is a Rust
    /// bug here (a plain panic in every build, never an invalid handle).
    pub fn player_by_small_id(&self, id: u16) -> Owner {
        assert!(id == 0 || usize::from(id) <= self.players.len(), "playerBySmallID: smallID {id} has no player");
        SmallId(id)
    }
    /// `owner(ref)` (`:268-270`) = `playerBySmallID(ownerID(ref))`.
    pub fn owner(&self, t: TileRef) -> Owner {
        self.player_by_small_id(self.map.owner_id(t))
    }
    /// `findPlayerInfo(id)` (`:1619-1625`): the player's info, else a human's, else a nation's.
    pub fn find_player_info(&self, id: &str) -> Option<&PlayerInfo> {
        if let Some(p) = self.find_player(id) {
            return Some(self.info(self.players[p.player_index()].info)); // in-range: from find_player
        }
        if let Some(h) = self.humans.iter().find(|h| h.id == id) {
            return Some(h);
        }
        self.nations.iter().find(|n| n.player_info.id == id).map(|n| &n.player_info)
    }
    /// `PlayerInfo` behind an index.
    #[inline]
    pub fn info(&self, i: PlayerInfoIdx) -> &PlayerInfo {
        &self.infos[i.0 as usize] // in-range: indexes come from add_player
    }
    /// `teams()` (`:1238-1243`): `[botTeam, ...playerTeams]` in team mode, else `[]`.
    pub fn teams(&self) -> Vec<Team> {
        if self.cfg.game_config().game_mode != GameMode::Team {
            return Vec::new();
        }
        std::iter::once(self.bot_team.clone()).chain(self.player_teams.iter().cloned()).collect()
    }
    /// `teamTilesOwned(team)` (`:1245-1251`): over `allPlayers()`, `p.team() === team`.
    pub fn team_tiles_owned(&self, team: &str) -> u32 {
        let mut team_tiles = 0u32;
        for p in &self.players {
            if p.team.as_deref() == Some(team) {
                // int: numTilesOwned() = `_tiles.size` (PlayerImpl.ts:588-590) ≤ map size.
                team_tiles += p.tiles.size() as u32;
            }
        }
        team_tiles
    }
    /// `totalLandTiles()` (`:1253-1255`): `Math.max(0, numLandTiles() - numTilesWithFallout())`.
    pub fn total_land_tiles(&self) -> u32 {
        // int: both are tile counts; `max(0, a - b)` is a saturating subtraction.
        self.map.num_land_tiles().saturating_sub(self.map.num_tiles_with_fallout())
    }
    /// `teamSpawnArea(team)` (`:1257-1271`).
    pub fn team_spawn_area(&self, team: &str) -> Option<SpawnArea> {
        // TS: `if (!this._teamGameSpawnAreas) return undefined` (an empty object is truthy).
        let all = self.statics.team_game_spawn_areas.as_ref()?;
        let num_teams = self.player_teams.len();
        // TS: `this._teamGameSpawnAreas[String(numTeams)]`; `if (!areas)` (an array is truthy).
        let areas = &all.iter().find(|(k, _)| *k as usize == num_teams)?.1;
        let team_index = self.player_teams.iter().position(|t| t == team)?; // indexOf < 0 → undefined
        areas.get(team_index).copied() // `teamIndex >= areas.length` → undefined
    }

    // ---- Immunity / time (`Game.ts:557-560`) ----
    /// `isSpawnImmunityActive()` (`:1147-1152`).
    pub fn is_spawn_immunity_active(&self) -> bool {
        self.in_spawn_phase() || self.ticks_since_start() < f64::from(self.config().spawn_immunity_duration())
    }
    /// `isNationSpawnImmunityActive()` (`:1158-1163`).
    pub fn is_nation_spawn_immunity_active(&self) -> bool {
        self.in_spawn_phase() || self.ticks_since_start() < f64::from(self.config().nation_spawn_immunity_duration())
    }
    /// `elapsedGameSeconds()` (`:1154-1156`): `ticksSinceStart() / 10`.
    pub fn elapsed_game_seconds(&self) -> f64 {
        self.ticks_since_start() / 10.0
    }
    /// `ticksSinceStart()` (`:1165-1171`): 0 in the spawn phase, else
    /// `Math.max(0, ticks() - startTick!)`.
    fn ticks_since_start(&self) -> f64 {
        match self.start_tick {
            None => 0.0,
            Some(st) => of_det::js::max(0.0, f64::from(self.ticks) - f64::from(st)),
        }
    }

    // ---- Game state (`Game.ts:562-583`) ----
    pub fn ticks(&self) -> Tick {
        self.ticks
    }
    /// `inSpawnPhase()` (`:536-538`): `startTick === null`; re-read per execution (Q-SPAWNFLIP).
    #[inline]
    pub fn in_spawn_phase(&self) -> bool {
        self.start_tick.is_none()
    }
    /// `getWinner()`.
    pub fn get_winner(&self) -> &Winner {
        &self.winner
    }
    /// `config()`.
    #[inline]
    pub fn config(&self) -> &EngineConfig {
        &self.cfg
    }
    pub fn is_paused(&self) -> bool {
        self.is_paused
    }

    // ---- Units (`Game.ts:585-621`) ----
    /// `unit(id)` (`:345-347`): only live units are in `_unitMap`.
    pub fn unit_by_id(&self, id: u32) -> Option<UnitRef> {
        self.unit_by_id.get(&id).copied()
    }
    /// `units()` / `units([])` (`:363-369, 376-382`): all players' live arrays concatenated in
    /// `_players` order, a new array.
    pub fn all_units(&self) -> Vec<UnitRef> {
        let mut out = Vec::new();
        for p in &self.players {
            // iter-live: read-only walk, nothing mutates `_units` here.
            out.extend_from_slice(p.units.as_slice());
        }
        out
    }
    /// `units(types…)` (`:349-415`). Every TS form gives the same list: the units of the
    /// listed types, in `_players` order then `p.units()` order:
    /// - `[]` → all units (`:376-382`);
    /// - one type (`units(t)`, `:383-398`) → the `unitsByTypeMemo` answer, which equals the
    ///   filter at the current `_unitsVersion`: every `_units` change bumps the version before
    ///   any reader can run (`PlayerImpl.ts:1422-1428`, `UnitImpl.ts:275-280, 353-355`) and a
    ///   unit's type never changes;
    /// - several types (`Set` or `t1|t2|t3`, `:369-375, 399-410`) → the filter.
    ///
    /// So the filter is always computed: `&self` cannot fill the memo, and the memo is
    /// derived state that is never exported (`DERIVED_FIELDS`), so skipping it is
    /// unobservable (ledger: memo fill is perf work under ARCHITECTURE §11).
    pub fn units_of(&self, types: &[UnitType]) -> Vec<UnitRef> {
        if types.is_empty() {
            return self.all_units();
        }
        let mut out = Vec::new();
        for p in &self.players {
            for &u in p.units.as_slice() {
                // `u.type()` = `_type` (UnitImpl), immutable.
                if types.contains(&self.units.get(u).unit_type) {
                    out.push(u);
                }
            }
        }
        out
    }
    /// `unitCount(type)` (`:417-428`): the level-weighted `player.unitCount(type)` summed over
    /// `_players`. TS memoises it on `_unitsVersion`, which also moves on every level change
    /// (`UnitImpl.ts:757, 778`), so the memo equals the sum; computed always, as in
    /// [`State::units_of`].
    pub fn unit_count(&self, t: UnitType) -> i32 {
        let mut total = 0i32;
        for p in &self.players {
            total += self.player(p.small_id).unit_count(t);
        }
        total
    }
    /// [`State::unit_count`] that fills TS's `unitCountMemo` (`GameImpl.ts:418-427`) and, through
    /// [`State::player_unit_count_memo`], every player's `myUnitCountMemo`, exactly where TS
    /// fills them (P3 perf, t066). Same value as `unit_count`: `_unitsVersion` moves on every
    /// unit-list change and level change (`UnitImpl.ts:276-279,354,551,758,779`; checked by
    /// `verify-memos`).
    pub fn unit_count_memo(&mut self, t: UnitType) -> i32 {
        let i = t.index();
        if self.unit_count_memo[i].version == Some(self.units_version) {
            let v = self.unit_count_memo[i].value;
            #[cfg(feature = "verify-memos")]
            assert_eq!(v, self.unit_count(t), "verify-memos: unitCountMemo[{t:?}] stale");
            return v;
        }
        let mut total = 0i32;
        for k in 0..self.players.len() {
            let sid = self.players[k].small_id;
            total += self.player_unit_count_memo(sid, t);
        }
        self.unit_count_memo[i] = Memo { version: Some(self.units_version), value: total };
        total
    }
    /// `player.nearby()` filling TS's `nearbyMemo` (`PlayerImpl.ts:600-626`: keyed on
    /// `territoryVersion` and the map's `waterVersion`; a hit hands out a copy). Same value as
    /// `PlayerRef::nearby` (checked on every hit by `verify-memos`; P3 perf, t066).
    pub fn nearby_memo(&mut self, p: SmallId) -> Vec<Owner> {
        let idx = p.player_index();
        let version = self.territory_version;
        let water_version = self.map.water_version();
        if let Some(m) = &self.players[idx].nearby_memo // in-range: smallIDs come from add_player
            && m.version == version
            && m.water_version == water_version
        {
            // `PlayerRef::nearby` hits the same memo and asserts it equals a fresh compute.
            #[cfg(feature = "verify-memos")]
            assert_eq!(m.result, self.player(p).nearby());
            return m.result.clone();
        }
        let result = self.player(p).nearby();
        self.players[idx].nearby_memo =
            Some(crate::state::player::NearbyMemo { version, water_version, result: result.clone() });
        result
    }
    /// `mg.units(type)` (single-type form) filling TS's `unitsByTypeMemo` (`GameImpl.ts:376-392`,
    /// keyed on `_unitsVersion`; callers get their own copy). Same list as
    /// `units_of(&[type])` (checked by `verify-memos`; P3 perf, t066). The fill rebuilds all 16
    /// per-type lists in one pass, which equals TS's per-type lazy fill (`state::memo` docs).
    pub fn units_of_type_memo(&mut self, t: UnitType) -> Vec<UnitRef> {
        let v = self.units_version;
        if self.units_by_type_memo.get(t, v).is_none() {
            let (units, players) = (&self.units, &self.players);
            self.units_by_type_memo.rebuild(players.iter().flat_map(|p| p.units.as_slice().iter().copied()), v, |u| {
                units.get(u).unit_type
            });
        }
        let list = self.units_by_type_memo.get(t, v).map(<[UnitRef]>::to_vec).unwrap_or_default();
        #[cfg(feature = "verify-memos")]
        assert_eq!(list, self.units_of(&[t]), "verify-memos: unitsByTypeMemo[{t:?}] stale");
        list
    }
    /// `player.units(type)` (single-type form) filling TS's `myUnitsMemo` (`PlayerImpl.ts:482-497`,
    /// keyed on `_myUnitsVersion`; callers get a copy). Same list as `player(p).units_of(&[t])`
    /// (checked by `verify-memos`; P3 perf, t066).
    pub fn player_units_of_type_memo(&mut self, p: SmallId, t: UnitType) -> Vec<UnitRef> {
        let idx = p.player_index();
        let (units, players) = (&self.units, &mut self.players);
        let d = &mut players[idx]; // in-range: smallIDs come from add_player
        let v = d.my_units_version;
        if d.my_units_memo.get(t, v).is_none() {
            d.my_units_memo.rebuild(d.units.as_slice().iter().copied(), v, |u| units.get(u).unit_type);
        }
        let list = d.my_units_memo.get(t, v).map(<[UnitRef]>::to_vec).unwrap_or_default();
        #[cfg(feature = "verify-memos")]
        {
            let fresh: Vec<UnitRef> = self.players[idx]
                .units
                .as_slice()
                .iter()
                .copied()
                .filter(|&u| self.units.get(u).unit_type == t)
                .collect();
            assert_eq!(list, fresh, "verify-memos: myUnitsMemo[{t:?}] stale ({p:?})");
        }
        list
    }
    /// `player.unitCount(type)` filling `myUnitCountMemo` (`PlayerImpl.ts:530-548`; P3 perf).
    pub fn player_unit_count_memo(&mut self, p: SmallId, t: UnitType) -> i32 {
        let (i, idx) = (t.index(), p.player_index());
        let version = self.players[idx].my_units_version; // in-range: smallIDs come from add_player
        if self.players[idx].my_unit_count_memo[i].version == Some(version) {
            let v = self.players[idx].my_unit_count_memo[i].value;
            #[cfg(feature = "verify-memos")]
            assert_eq!(v, self.player(p).unit_count(t), "verify-memos: myUnitCountMemo[{t:?}] stale ({p:?})");
            return v;
        }
        let (units, players) = (&self.units, &mut self.players);
        let d = &mut players[idx];
        let mut total = 0i32;
        for &u in d.units.as_slice() {
            let ud = units.get(u);
            if ud.unit_type == t {
                total += ud.level;
            }
        }
        d.my_unit_count_memo[i] = Memo { version: Some(version), value: total };
        total
    }
    /// `hasUnitNearby(tile, searchRange, type, playerId?, includeUnderConstruction?)` (`:1402-1416`).
    pub fn has_unit_nearby(
        &self,
        tile: TileRef,
        search_range: f64,
        t: UnitType,
        player_id: Option<&str>,
        include_under_construction: bool,
    ) -> bool {
        // TS: `unitGrid.hasUnitNearby(tile, searchRange, type, playerId, iuc)` (`:1409-1415`).
        let Some(player) = self.owner_filter(player_id) else { return false };
        self.unit_grid.has_unit_nearby(
            &self.units,
            self.width(),
            tile,
            search_range,
            t,
            player,
            include_under_construction,
        )
    }
    /// t409 H5: `hasUnitNearby(tile, searchRange, type, player.id(), iuc)` for a player handle.
    /// Player ids are unique (`add_player` asserts it), so `find_player(player.id())` is `player`
    /// and the id round trip of [`Self::has_unit_nearby`] (a string scan over all players) is skipped.
    pub fn has_unit_nearby_of(
        &self,
        tile: TileRef,
        search_range: f64,
        t: UnitType,
        player: SmallId,
        include_under_construction: bool,
    ) -> bool {
        self.unit_grid.has_unit_nearby(
            &self.units,
            self.width(),
            tile,
            search_range,
            t,
            Some(player),
            include_under_construction,
        )
    }
    /// `UnitGrid.unitIsInRange`'s `unit.owner().id() !== playerId` filter as a smallID:
    /// `Some(None)` = no filter; `None` = an id no player has, which no unit's owner can
    /// match, so the (pure) grid query is `false` without being run.
    fn owner_filter(&self, player_id: Option<&str>) -> Option<Option<SmallId>> {
        match player_id {
            None => Some(None),
            Some(id) => self.find_player(id).map(Some),
        }
    }
    /// `anyUnitNearby(tile, searchRange, types, predicate, playerId?, includeUnderConstruction?)`
    /// (`:1418-1434`).
    #[allow(clippy::too_many_arguments)]
    pub fn any_unit_nearby(
        &self,
        tile: TileRef,
        search_range: f64,
        types: &[UnitType],
        mut predicate: impl FnMut(&State, UnitRef) -> bool,
        player_id: Option<&str>,
        include_under_construction: bool,
    ) -> bool {
        // TS: `unitGrid.anyUnitNearby(...)` (`:1426-1433`). An unknown `playerId` fails
        // `unitIsInRange` before the predicate is ever called, so skipping is exact.
        let Some(player) = self.owner_filter(player_id) else { return false };
        self.unit_grid.any_unit_nearby(
            &self.units,
            self.width(),
            tile,
            search_range,
            types,
            |u| predicate(self, u),
            player,
            include_under_construction,
        )
    }
    /// `nearbyUnits(tile, searchRange, types, predicate?, includeUnderConstruction?)` (`:1436-1453`).
    pub fn nearby_units(
        &self,
        tile: TileRef,
        search_range: f64,
        types: &[UnitType],
        mut predicate: impl FnMut(&State, UnitRef, f64) -> bool,
        include_under_construction: bool,
    ) -> Vec<NearbyUnit> {
        // TS: `unitGrid.nearbyUnits(tile, searchRange, types, predicate, iuc)` (`:1443-1452`).
        self.unit_grid.nearby_units(
            &self.units,
            self.width(),
            tile,
            search_range,
            types,
            |u, d| predicate(self, u, d),
            include_under_construction,
        )
    }

    // ---- Misc (`Game.ts:661-699`) ----
    /// `nations()` (`:434-436`).
    pub fn nations(&self) -> &[Nation] {
        &self.nations
    }
    /// `numTilesWithFallout()` (`:1578-1580`).
    pub fn num_tiles_with_fallout(&self) -> u32 {
        self.map.num_tiles_with_fallout()
    }
    /// `structureVersion()` (`:1359-1361`).
    pub fn structure_version(&self) -> u32 {
        self.structure_version
    }
    /// `territoryVersion()` (`:1378-1380`).
    pub fn territory_version(&self) -> u32 {
        self.territory_version
    }
    pub fn mirvs_launched(&self) -> u32 {
        self.mirvs_launched
    }
    /// `waterGraphVersion()` (`:1632-1634`).
    pub fn water_graph_version(&self) -> u32 {
        self.water.water_graph_version()
    }
    /// `GameImpl.hash()` (legacy hash, on demand; `:742-751`, §5): `1 + Σ p.hash()` over
    /// `_players` (Map order), then `+ railNetwork.hash()`, summed left to right in f64.
    pub fn legacy_hash(&self) -> f64 {
        let mut hash = 1.0;
        for p in &self.players {
            hash += self.player(p.small_id).hash();
        }
        hash += self.rail_hash();
        hash
    }
    /// `PlayerImpl.toString()` (`PlayerImpl.ts:1858-1864`), for TS error messages that
    /// interpolate a player.
    pub fn player_ts_string(&self, p: SmallId) -> String {
        let d = &self.players[p.player_index()]; // in-range: smallIDs come from add_player
        let info = self.info(d.info);
        let client = info.client_id.as_deref().unwrap_or("null");
        format!(
            "Player:{{name:{},clientID:{},isAlive:{},troops:{},numTileOwned:{}}}]",
            info.name,
            client,
            d.tiles.size() > 0,
            d.troops,
            d.tiles.size()
        )
    }
}

impl Rcx<'_> {
    /// `getWaterComponent(tile)` (`:1635-1637`).
    pub fn get_water_component(&mut self, tile: TileRef) -> Option<u32> {
        self.water_component(tile)
    }
    /// `getWaterComponentSize(tile)` (`:1641-1643`).
    pub fn get_water_component_size(&mut self, tile: TileRef) -> Option<u32> {
        self.water_component_size(tile)
    }
}

impl Ctx<'_> {
    // ---- Map writes with GameImpl side effects ----
    /// `GameImpl.setFallout(tile, value)` (`:295-305`): throws if owned; `territoryVersion++`.
    ///
    /// Literal: a tile that already has fallout returns early even when `value` is `false`.
    pub fn set_fallout(&mut self, tile: TileRef, value: bool) {
        if value && self.st.map.has_owner(tile) {
            ts_throw!("cannot set fallout, tile {tile} has owner");
        }
        if self.st.map.has_fallout(tile) {
            return;
        }
        self.st.territory_version += 1;
        self.st.map.set_fallout(tile, value);
        self.journal_tile(tile); // TS: recordTileUpdate(tile)
    }
    /// `GameImpl.setWater(tile)` (`:307-321`).
    pub fn set_water(&mut self, tile: TileRef) {
        if !self.st.map.is_land(tile) {
            return;
        }
        if self.st.map.has_owner(tile) {
            ts_throw!("cannot set water, tile {tile} has owner");
        }
        // Clear fallout if present (water tiles shouldn't have fallout)
        if self.st.map.has_fallout(tile) {
            self.st.map.set_fallout(tile, false);
        }
        self.st.territory_version += 1;
        // Rail paths route around water, so a rail plan must be redone too.
        self.st.structure_version += 1;
        self.st.map.set_water(tile);
        self.journal_tile(tile); // TS: recordTileUpdate(tile)
    }
    /// `GameImpl.setOwnerID(ref, playerId)` (`:1524-1527`): `territoryVersion++`.
    pub fn set_owner_id(&mut self, tile: TileRef, player_id: u16) {
        // t409 H5: a raw owner write keeps no conn4 / component proof for either side.
        for id in [self.st.map.owner_id(tile), player_id] {
            let p = SmallId(id);
            if p.is_player() && p.player_index() < self.st.players.len() {
                let d = self.st.player_data_mut(p);
                d.conn4 = false;
                d.comps.valid = false;
            }
        }
        self.st.territory_version += 1;
        self.st.map.set_owner_id(tile, player_id);
    }
    /// `queueWaterConversion(tile)` (`:323-333`).
    pub fn queue_water_conversion(&mut self, tile: TileRef) {
        if !self.st.map.is_land(tile) {
            return;
        }
        if self.st.map.has_owner(tile) {
            ts_throw!("cannot queue water conversion, tile {tile} has owner");
        }
        if !self.st.config().water_nukes() {
            self.set_fallout(tile, true);
            return;
        }
        self.water_queue_tile(tile); // TS: this._waterManager.queueTile(tile)
    }
    /// `queueNukeImpact(tile)` (`:335-337`): view-only queue, dropped (§5). No-op.
    #[inline]
    pub fn queue_nuke_impact(&mut self, _tile: TileRef) {}

    // ---- Player management ----
    /// `addPlayer(playerInfo, team = null)` (`:906-918`). Asserts the id is new (§3.1: a
    /// duplicate would split `_players` Map order from smallID order, which the port does not
    /// model; a plain panic, not a `TsThrow`).
    pub fn add_player(&mut self, info: PlayerInfo, team: Option<Team>) -> SmallId {
        let dup = self.st.players.iter().any(|p| self.st.info(p.info).id == info.id);
        assert!(!dup, "add_player: duplicate player id {:?} (not modelled, ARCHITECTURE §3.1)", info.id);
        // TS argument order: startManpower(playerInfo), then `team ?? maybeAssignTeam(..)`.
        let start_troops = self.st.cfg.start_manpower(&info);
        let team = match team {
            Some(t) => Some(t),
            None => self.st.maybe_assign_team(&info),
        };
        // PlayerImpl ctor: `_gold = config.startingGold(playerInfo)` (PlayerImpl.ts:210-220).
        let starting_gold = self.st.cfg.starting_gold(&info);
        #[allow(clippy::expect_used)] // rust-invariant: smallIDs fit 12 bits of tile state
        let small_id = SmallId(u16::try_from(self.st.next_player_id).expect("smallID fits u16"));
        #[allow(clippy::expect_used)] // rust-invariant: < 2^32 players
        let idx = PlayerInfoIdx(u32::try_from(self.st.infos.len()).expect("info index fits u32"));
        let player = Player::new(small_id, idx, &info, start_troops, starting_gold, team);
        std::sync::Arc::make_mut(&mut self.st.infos).push(info);
        self.st.players.push(player); // TS: _playersBySmallID.push(player) (+ _players.set)
        self.st.next_player_id += 1;
        small_id
    }
    /// `conquer(owner, tile)` (`:987-1009`).
    pub fn conquer(&mut self, owner: SmallId, tile: TileRef) {
        if !self.st.map.is_land(tile) {
            ts_throw!("cannot conquer water");
        }
        if self.st.map.is_impassable(tile) {
            ts_throw!("cannot conquer impassable terrain");
        }
        let previous_owner = self.st.owner(tile);
        self.conn4_note(tile, previous_owner, owner);
        if previous_owner.is_player() {
            let ticks = self.st.ticks;
            let prev = self.st.player_data_mut(previous_owner);
            prev.last_tile_change = ticks;
            prev.tile_change_version += 1;
            let (pos, hdr, li) = self.st.tiles_parts_mut(previous_owner);
            pos.delete(hdr, li, tile);
            let (pos, hdr, li) = self.st.border_parts_mut(previous_owner);
            pos.delete(hdr, li, tile);
        }
        self.st.territory_version += 1;
        self.st.map.set_owner_id(tile, owner.0);
        let (pos, hdr, li) = self.st.tiles_parts_mut(owner);
        pos.add(hdr, li, tile);
        let ticks = self.st.ticks;
        let o = self.st.player_data_mut(owner);
        o.last_tile_change = ticks;
        o.tile_change_version += 1;
        self.update_borders(tile);
        self.st.map.set_fallout(tile, false); // TS: this._map.setFallout (not GameImpl's)
        self.journal_tile(tile); // TS: recordTileUpdate(tile)
    }
    /// `relinquish(tile)` (`:1011-1032`).
    pub fn relinquish(&mut self, tile: TileRef) {
        if !self.st.map.has_owner(tile) {
            ts_throw!("Cannot relinquish tile because it is unowned");
        }
        if self.st.map.is_water(tile) {
            ts_throw!("Cannot relinquish water");
        }
        let previous_owner = self.st.owner(tile);
        self.conn4_note(tile, previous_owner, SmallId::TERRA_NULLIUS);
        let ticks = self.st.ticks;
        let prev = self.st.player_data_mut(previous_owner);
        prev.last_tile_change = ticks;
        prev.tile_change_version += 1;
        let (pos, hdr, li) = self.st.tiles_parts_mut(previous_owner);
        pos.delete(hdr, li, tile);
        let (pos, hdr, li) = self.st.border_parts_mut(previous_owner);
        pos.delete(hdr, li, tile);
        self.st.territory_version += 1;
        self.st.map.set_owner_id(tile, 0);
        self.update_borders(tile);
        self.journal_tile(tile); // TS: recordTileUpdate(tile)
    }
    /// t409 H5: `Player::conn4` upkeep for `tile` about to change owner `prev` -> `new` (called
    /// before the owner write; rules and proof in `map::conn4`). Reads only; never TS state.
    #[inline]
    fn conn4_note(&mut self, tile: TileRef, prev: SmallId, new: SmallId) {
        use crate::map::conn4;
        if prev == new {
            return; // the territory does not change
        }
        if prev.is_player() {
            let d = self.st.player_data(prev);
            if (d.conn4 || d.comps.valid) && !conn4::loss_keeps(&self.st.map, tile, prev.0) {
                self.conn4_loss_failed(tile, prev);
            } else if d.comps.valid {
                let c = self.st.comp_labels.v[tile as usize] as usize; // in-range: sized while valid
                let d = self.st.player_data_mut(prev);
                d.comps.sizes[c] -= 1; // in-range: labels of owned tiles index sizes
            }
        }
        if new.is_player() {
            let d = self.st.player_data(new);
            if d.conn4 && d.tiles.size() > 0 && !conn4::gain_keeps(&self.st.map, tile, new.0) {
                self.st.player_data_mut(new).conn4 = false;
            }
            if self.st.player_data(new).comps.valid {
                match conn4::gain_label(&self.st.map, &self.st.comp_labels.v, tile, new.0) {
                    None => self.st.player_data_mut(new).comps.valid = false,
                    Some(Some(c)) => {
                        self.st.comp_labels.v[tile as usize] = c; // in-range: sized while valid
                        self.st.player_data_mut(new).comps.sizes[c as usize] += 1;
                    }
                    Some(None) => {
                        let sizes = &mut self.st.player_data_mut(new).comps.sizes;
                        let c = sizes.len() as u32; // int: < number of tiles
                        sizes.push(1);
                        self.st.comp_labels.v[tile as usize] = c; // in-range: sized while valid
                    }
                }
            }
        }
    }
    /// R2: the ring rule failed for `tile` leaving `prev`'s territory. Settle by a bounded search whether the pieces around the
    /// tile stay connected (labels kept, conn4 kept), whether it was a cut tile (closed pockets relabelled, conn4 cleared) or give
    /// up as before (both cleared). Only a state that keeps labels (`comp_labels.enabled`) searches: a fork keeps the old rule.
    #[inline(never)]
    fn conn4_loss_failed(&mut self, tile: TileRef, prev: SmallId) {
        use crate::map::conn4::{self, CompLabels, Loss};
        let st = &mut *self.st;
        let pl = &mut st.players[prev.player_index()]; // in-range: a live player
        let outcome =
            if st.comp_labels.enabled && st.comp_labels.v.len() >= st.map.width() as usize * st.map.height() as usize {
                let CompLabels { v, search, .. } = &mut st.comp_labels;
                if pl.comps.valid {
                    let c = v[tile as usize]; // in-range: sized while valid
                    conn4::loss_search(&st.map, prev.0, tile, v, search, Some((c, &mut pl.comps.sizes)))
                } else {
                    conn4::loss_search(&st.map, prev.0, tile, &mut [], search, None)
                }
            } else {
                Loss::Budget
            };
        match outcome {
            Loss::Connected => {
                if pl.comps.valid {
                    let c = st.comp_labels.v[tile as usize] as usize; // in-range: sized while valid
                    pl.comps.sizes[c] -= 1; // in-range: labels of owned tiles index sizes
                }
            }
            Loss::Split => pl.conn4 = false,
            Loss::Budget => {
                pl.conn4 = false;
                pl.comps.valid = false;
            }
        }
    }
    /// `updateBorders(tile)` (`:1037-1043`): the tile, then its `neighbors4` in order
    /// (`borderNbuf` → a stack buffer).
    fn update_borders(&mut self, tile: TileRef) {
        self.update_border_status(tile);
        let mut nbuf: [TileRef; 4] = [0; 4];
        let n = self.st.map.neighbors4(tile, &mut nbuf);
        for &t in &nbuf[..n] {
            self.update_border_status(t);
        }
    }
    /// `updateBorderStatus(t)` (`:1045-1055`).
    fn update_border_status(&mut self, t: TileRef) {
        if !self.st.map.has_owner(t) {
            return;
        }
        let owner = self.st.owner(t);
        let is_border = self.st.map.is_border(t);
        let (pos, hdr, li) = self.st.border_parts_mut(owner);
        if is_border {
            pos.add(hdr, li, t);
        } else {
            pos.delete(hdr, li, t);
        }
    }
    /// `target(targeter, target)` (`:1057-1063`): only a `TargetPlayer` update (view, §5).
    pub fn target(&mut self, _targeter: SmallId, _target: SmallId) {}
    /// `breakAlliance(breaker, alliance)` (`:1062-1096`). The `BrokeAlliance` update is view
    /// work (§5).
    pub fn break_alliance(&mut self, breaker: SmallId, alliance: AllianceId) {
        let a = self.st.alliance(alliance);
        let other = if a.requestor == breaker { a.recipient } else { a.requestor };
        let created_at = a.created_at;
        if !self.st.player(breaker).is_allied_with(other) {
            let (b, o) = (self.st.player_ts_string(breaker), self.st.player_ts_string(other));
            ts_throw!("{b} not allied with {o}, cannot break alliance");
        }
        let duration = StatNum::Num(f64::from(self.st.ticks - created_at));
        // `other.isDisconnected()` = `_isDisconnected` (PlayerImpl.ts:1820-1822).
        if !self.st.player(other).is_traitor() && !self.st.player_data(other).is_disconnected {
            self.player_mut(breaker).mark_traitor();
            self.st.stats_cx().alliance_ended(other, duration, Some(AllianceEndCounter::BrokenByOther));
        } else {
            self.st.stats_cx().alliance_ended(other, duration, None);
        }
        // The breaker's side is already counted by betray(); this call is only here so their
        // longest-held maximum still sees this alliance.
        self.st.stats_cx().alliance_ended(breaker, duration, None);
        self.detach_alliance(alliance);
    }
    /// `expireAlliance(alliance)` (`:1098-1119`). The `AllianceExpired` update is view work.
    pub fn expire_alliance(&mut self, alliance: AllianceId) {
        let a = self.st.alliance(alliance);
        let (requestor, recipient) = (a.requestor, a.recipient);
        // TS: `new Set(recipient.alliances())`, then `requestor.alliances().filter(has)`.
        let p1_set = self.st.player_data(recipient).alliances.as_slice();
        let alliances: Vec<AllianceId> = self
            .st
            .player_data(requestor)
            .alliances
            .as_slice()
            .iter()
            .copied()
            .filter(|x| p1_set.contains(x))
            .collect();
        if alliances.len() != 1 {
            let n = alliances.len();
            ts_throw!("cannot expire alliance: must have exactly one alliance, have {n}");
        }
        let expiring = alliances[0]; // in-range: len == 1
        let e = self.st.alliance(expiring);
        let (e_req, e_rec) = (e.requestor, e.recipient);
        let duration = StatNum::Num(f64::from(self.st.ticks - e.created_at));
        self.st.stats_cx().alliance_ended(e_req, duration, Some(AllianceEndCounter::Expired));
        self.st.stats_cx().alliance_ended(e_rec, duration, Some(AllianceEndCounter::Expired));
        self.detach_alliance(expiring);
    }
    /// `removeAlliancesByPlayerSilently(player)` (`:1121-1138`): over a snapshot of the
    /// player's `_alliances` (`detachAlliance` reassigns it as it goes).
    pub fn remove_alliances_by_player_silently(&mut self, player: SmallId) {
        let removed: Vec<AllianceId> = self.st.player_data(player).alliances.as_slice().to_vec();
        for alliance in removed {
            let a = self.st.alliance(alliance);
            let (req, rec) = (a.requestor, a.recipient);
            let duration = StatNum::Num(f64::from(self.st.ticks - a.created_at));
            self.st.stats_cx().alliance_ended(req, duration, None);
            self.st.stats_cx().alliance_ended(rec, duration, None);
            self.detach_alliance(alliance);
        }
    }
    /// `detachAlliance(alliance)` (`:1140-1145`): filter-reassign both participants' lists
    /// (ARCHITECTURE §4.7).
    fn detach_alliance(&mut self, alliance: AllianceId) {
        let a = self.st.alliance(alliance);
        let (requestor, recipient) = (a.requestor, a.recipient);
        self.st.player_data_mut(requestor).alliances.filter_reassign(|&x| x != alliance);
        self.st.player_data_mut(recipient).alliances.filter_reassign(|&x| x != alliance);
    }
    /// `conquerPlayer(conqueror, conquered)` (`GameImpl.ts:1772-1846`; reads
    /// `stats.attacks_sent`, ARCHITECTURE §3.10). `displayMessage`/`ConquestEvent` are view
    /// work (§5); their arguments are pure getters, and `displayMessage`'s own
    /// `player(id)` lookups resolve existing players.
    pub fn conquer_player(&mut self, conqueror: SmallId, conquered: SmallId) {
        // TS: :1773-1784 — `conquered.units().filter(..)` is a new array.
        if self.st.player_data(conquered).is_disconnected && self.st.player(conqueror).is_on_same_team(conquered) {
            let ships: Vec<UnitRef> = self
                .st
                .player_data(conquered)
                .units
                .as_slice()
                .iter()
                .copied()
                .filter(|&u| {
                    let t = self.st.unit_data(u).unit_type;
                    t == UnitType::Warship || t == UnitType::TransportShip
                })
                .collect();
            for ship in ships {
                self.player_mut(conqueror).capture_unit(ship);
            }
        }
        // TS: :1787-1797
        let conquered_client = self.st.info(self.st.player_data(conquered).info).client_id.clone();
        let attacks_sent = self.st.stats.attacks_sent(conquered_client.as_deref());
        let conquered_type = self.st.info(self.st.player_data(conquered).info).player_type;
        let skip_gold_transfer = attacks_sent == 0 && conquered_type == PlayerType::Human;
        let gold = if skip_gold_transfer { 0 } else { self.st.player_data(conquered).gold };
        let gold_captured = if skip_gold_transfer {
            0
        } else {
            let cfg = self.st.cfg.clone();
            cfg.conquer_gold_amount(&self.st.player(conquered))
        };
        if !skip_gold_transfer {
            // TS: :1824-1828
            self.player_mut(conqueror).add_gold(gold_captured, None);
            self.player_mut(conquered).remove_gold(gold);
            self.st.stats_cx().gold_war(conqueror, conquered, StatNum::Big(gold_captured));
        }
        // TS: :1831
        let ticks = self.st.ticks;
        self.st.stats_cx().record_kill(conqueror, conquered, StatNum::Num(f64::from(ticks)));
        // TS: :1839-1845
        let conqueror_client = self.st.info(self.st.player_data(conqueror).info).client_id.clone();
        self.st.stats_cx().record_killed_by(conquered, conqueror_client.as_deref());
        let alive_others = self
            .st
            .players()
            .into_iter()
            .filter(|&p| p != conquered && self.st.info(self.st.player_data(p).info).player_type != PlayerType::Bot)
            .count();
        #[allow(clippy::expect_used)] // rust-invariant: player counts fit i32
        let position = i32::try_from(alive_others).expect("player count fits i32") + 1;
        self.st.stats_cx().record_death_position(conquered, position);
    }

    // ---- Game state ----
    /// `endSpawnPhase()` (`:540-549`): sets `startTick` once; the `SpawnPhaseEnd` update is
    /// view-only (§5).
    pub fn end_spawn_phase(&mut self) {
        if self.st.start_tick.is_some() {
            return;
        }
        self.st.start_tick = Some(self.st.ticks);
    }
    /// `setWinner(winner, allPlayersStats)` (`:1180-1201`); the stats object is output only.
    ///
    /// `makeWinner` (the `Win` update payload) only reads pure getters and is view work (§5).
    pub fn set_winner(&mut self, winner: Winner) {
        self.st.winner = winner.clone();
        // OFM: snapshot final tiles for standings (bots skipped in recordFinalTiles).
        for player in self.st.players() {
            let d = self.st.player_data(player);
            let tiles = StatNum::Num(d.tiles.size() as f64); // numTilesOwned()
            // TS: `const standing = player.alliances()` (the live array; nothing mutates it here)
            let mut longest = 0i32;
            for &a in d.alliances.as_slice() {
                let held = self.st.ticks - self.st.alliance(a).created_at;
                if held > longest {
                    longest = held;
                }
            }
            #[allow(clippy::expect_used)] // rust-invariant: alliance counts fit i32
            let standing = i32::try_from(d.alliances.len()).expect("alliance count fits i32");
            self.st.stats_cx().record_final_tiles(player, tiles);
            self.st.stats_cx().record_alliances_at_end(player, standing, StatNum::Num(f64::from(longest)));
        }
        self.st.events.winner_set = Some(winner);
    }
    /// `setPaused(paused)` (`:531-534`); the `GamePaused` update is view work.
    pub fn set_paused(&mut self, paused: bool) {
        self.st.is_paused = paused;
    }

    // ---- Units ----
    /// `nextUnitID()` (`:289-293`).
    pub fn next_unit_id(&mut self) -> u32 {
        let old = self.st.next_unit_id;
        self.st.next_unit_id += 1;
        old
    }
    /// `addUnit(u)` (`:1382-1386`): `_unitMap`, grid, `bumpUnitsVersion`, `noteStructureChange`.
    pub fn add_unit(&mut self, u: UnitRef) {
        self.st.units_version += 1;
        let (t, id) = {
            let d = self.st.unit_data(u);
            (d.unit_type, d.id)
        };
        self.note_structure_change(t);
        let st = &mut *self.st;
        st.unit_grid.add_unit(&st.units, u);
        self.st.unit_by_id.insert(id, u);
    }
    /// `removeUnit(u)` (`:1388-1396`).
    pub fn remove_unit(&mut self, u: UnitRef) {
        self.st.units_version += 1;
        let (t, id, has_station) = {
            let d = self.st.unit_data(u);
            (d.unit_type, d.id, d.has_train_station) // `hasTrainStation()` = `_hasTrainStation`
        };
        self.note_structure_change(t);
        let st = &mut *self.st;
        st.unit_grid.remove_unit(&st.units, u);
        self.st.unit_by_id.remove(&id);
        self.st.plan_driven_unit_ids.remove(&id);
        if has_station {
            self.rail_remove_station(u); // TS: this._railNetwork.removeStation(u)
        }
    }
    /// `updateUnitTile(u)` (`:1398-1400`) → `unitGrid.updateUnitCell`.
    pub fn update_unit_tile(&mut self, u: UnitRef) {
        let st = &mut *self.st;
        st.unit_grid.update_unit_cell(&st.units, u);
    }
    /// `onUnitMoved(unit)` (`:727-730`).
    pub fn on_unit_moved(&mut self, u: UnitRef) {
        self.update_unit_tile(u);
        self.maybe_add_unit_update(u);
    }
    /// `maybeAddUnitUpdate(unit)` (`:717-725`): `addUpdate(unit.toUpdate())` unless the unit
    /// is plan-driven; only `toUpdate`'s state write is kept (the shim, §5).
    pub fn maybe_add_unit_update(&mut self, u: UnitRef) {
        let id = self.st.unit_data(u).id;
        if !self.st.plan_driven_unit_ids.contains(&id) {
            self.st.unit_touched(u);
        }
    }
    /// `recordMotionPlan(record)` (`:702-715`): only the `planDrivenUnitIds` write is kept (§5).
    /// A `"train"` record is one call per id, engine first, then the cars in order.
    pub fn record_motion_plan(&mut self, unit_id: u32) {
        self.st.plan_driven_unit_ids.insert(unit_id);
    }
    /// `bumpUnitsVersion()` (`:1350-1357`).
    pub fn bump_units_version(&mut self) {
        self.st.units_version += 1;
    }
    /// `noteStructureChange(u)` (`:1363-1376`): City, Port and Factory only.
    pub fn note_structure_change(&mut self, t: UnitType) {
        if t == UnitType::City || t == UnitType::Port || t == UnitType::Factory {
            self.st.structure_version += 1;
        }
    }

    // ---- Misc ----
    /// `recordMirvLaunch()` (`:1609-1611`).
    pub fn record_mirv_launch(&mut self) {
        self.st.mirvs_launched += 1;
    }
    /// `nextShipStagger(kind)` (`:1612-1614`): `shipStaggers[kind]++ % STAGGER_SPREAD`.
    pub fn next_ship_stagger(&mut self, kind: ShipKind) -> i32 {
        let s = &mut self.st.ship_staggers;
        let c = match kind {
            ShipKind::TradeShip => &mut s.trade_ship,
            ShipKind::TransportShip => &mut s.transport_ship,
        };
        let old = *c;
        *c += 1;
        // int: a non-negative counter, so JS `%` equals Rust `%`.
        old % crate::path::pathfinder::WaterPathFinder::STAGGER_SPREAD
    }
    /// `sharedWaterComponents(player)` (`:1644-1646`).
    pub fn shared_water_components(&mut self, player: SmallId) -> Option<Vec<i32>> {
        self.shared_water_get(player)
    }
    /// The per-tick bookkeeping (`executeNextTick` `:631-648`): for every player in
    /// `_playersBySmallID` order that `isAlive() && hasSpawned()`, `recordTickSample` (under
    /// `stats-full` only: it writes nothing the simulation reads, §3.10); `toUpdate` is view
    /// work (§5).
    pub fn per_player_bookkeeping(&mut self) {
        #[cfg(feature = "stats-full")]
        for i in 0..self.st.players.len() {
            let id = SmallId::from_player_index(i);
            let p = self.st.player(id);
            if p.is_alive() && p.has_spawned() {
                self.stats_record_tick_sample(id);
            }
        }
    }
    /// End-of-tick maintenance, never observable (§3.6.2): drop `TileSet` tombstones above 25 %
    /// of the live count (players' `_tiles`/`_borderTiles`, attack borders). No cursor survives
    /// a tick boundary.
    pub fn end_of_tick_maintenance(&mut self) {
        // C4: compaction changes no observable (tileset.rs: positions, tombstones and capacities are not part of any state
        // the game can read; `dense_len` is read by no game code), so the lists are only visited every 8th tick: the tombstones
        // of at most 8 ticks of churn wait, and the compaction (the real cost: a random `pos_set` write per live entry) runs
        // at the same 25 % threshold on a list that has had longer to fill.
        if self.st.ticks % 8 != 0 {
            return;
        }
        let st = &mut *self.st;
        let land_index = &st.statics.land_index;
        for p in st.players.iter_mut() {
            st.pos_tiles.maybe_compact(&mut p.tiles, land_index);
            st.pos_border.maybe_compact(&mut p.border_tiles, land_index);
        }
        st.attacks.for_each_mut(|a| {
            a.border.maybe_compact();
        });
    }
}

/// `GameLike` (`engine-api/src/game/ReadViews.ts:42-58`): what `NukeAlliance` needs from a game.
impl State {
    /// `GameLike.anyUnitNearby` with a `UnitLike` predicate.
    pub fn game_like_any_unit_nearby(
        &self,
        tile: TileRef,
        search_range: f64,
        types: &[UnitType],
        predicate: impl FnMut(&State, UnitRef) -> bool,
        player_id: Option<&str>,
        include_under_construction: bool,
    ) -> bool {
        self.any_unit_nearby(tile, search_range, types, predicate, player_id, include_under_construction)
    }
}

/// `createGame(humans, nations, gameMap, miniGameMap, config, teamGameSpawnAreas)`
/// (`GameImpl.ts:110-127`) = `new GameImpl(.., new StatsImpl(), ..)` (`:182-218`): the field
/// initialisers, the constructor body, `populateTeams` (team mode) and `addPlayers`. Builds
/// the `State` (no executions) for `Game::new`. The maps come from `statics`
/// (`loadTerrainMap`'s `gameMap`/`miniGameMap`).
pub fn create_state(
    humans: Vec<PlayerInfo>,
    nations: Vec<Nation>,
    statics: Arc<MapStatics>,
    cfg: Arc<EngineConfig>,
) -> State {
    create_state_impl(humans, nations, statics, cfg, false)
}

/// `new GameImpl(.., restoring = true)` (`GameImpl.ts:209-214`): the same constructor, but
/// `populateTeams()` and `addPlayers()` are skipped (the snapshot loader fills players,
/// `playerTeams` and `botTeam`). For P13's `Game::from_canon` (request P13 #2).
pub fn create_state_restoring(
    humans: Vec<PlayerInfo>,
    nations: Vec<Nation>,
    statics: Arc<MapStatics>,
    cfg: Arc<EngineConfig>,
) -> State {
    create_state_impl(humans, nations, statics, cfg, true)
}

fn create_state_impl(
    humans: Vec<PlayerInfo>,
    nations: Vec<Nation>,
    statics: Arc<MapStatics>,
    cfg: Arc<EngineConfig>,
    restoring: bool,
) -> State {
    let map = GameMap::new(statics.width, statics.height, statics.terrain.clone(), statics.num_land_tiles, false);
    let mini = GameMap::new(
        statics.mini_width,
        statics.mini_height,
        statics.mini_terrain.clone(),
        statics.mini_num_land_tiles,
        true,
    );
    // TS: :202-206 `new WaterManager(map, miniMap, config.disableNavMesh())`; the version-0
    // graph and components are built once per map (MapStatics, §3.8).
    let water = if cfg.disable_nav_mesh() {
        WaterManager::new(None, None)
    } else {
        WaterManager::new(statics.graph0.clone(), statics.components0.clone())
    };
    let num_land = statics.num_land_tiles as usize;
    let mut st = State {
        // field initialisers (`:133-180`)
        ticks: 0,
        start_tick: None,
        humans: Arc::from(humans),
        nations: Arc::from(nations),
        players: Vec::new(),
        uninit: Vec::new(),
        alliance_requests: JsArr::new(),
        next_player_id: 1,
        next_unit_id: 1,
        next_alliance_id: 0,
        units: Arena::new(),
        plan_driven_unit_ids: JsSet::default(),
        // TS: :199 `new UnitGrid<Unit>(this._map)`
        unit_grid: UnitGrid::new(statics.width, statics.height),
        player_teams: Vec::new(),
        bot_team: crate::api::types::colored_teams::BOT.to_owned(),
        is_paused: false,
        winner: Winner::None,
        units_version: 0,
        territory_version: 0,
        ship_staggers: ShipStaggers::default(),
        nation_mirv_targets: JsMap::default(),
        mirvs_launched: 0,
        // TS: :166 `createRailNetwork(this)`
        rail: RailNetwork::new(),
        water,
        // TS: :207 `new SharedWaterCache(this)`: `tick = -Infinity`, `byPlayer = null`.
        shared_water: SharedWaterCache::default(),
        // createGame: `new StatsImpl()`
        stats: Stats::default(),
        cfg,
        statics,
        infos: Arc::new(Vec::new()),
        map,
        mini,
        attacks: Arena::new(),
        alliances: Vec::new(),
        alliance_reqs: Vec::new(),
        unit_by_id: LookupMap::new(),
        structure_version: 0,
        pos_tiles: PlayerTileOrder::new(num_land),
        pos_border: PlayerTileOrder::new(num_land),
        exec_inbox: Vec::new(),
        exec_cmds: Vec::new(),
        units_by_type_memo: TypeMemo::default(),
        unit_count_memo: Default::default(),
        shore_memo: Default::default(),
        comp_labels: Default::default(),
        events: StepEventsBuf::default(),
    };
    // TS: :209-214 `if (!restoring) { if (team mode) populateTeams(); addPlayers(); }`
    if restoring {
        return st;
    }
    let mut sc = crate::scratch::Scratch::new();
    let mut cx = Ctx::new(&mut st, &mut sc, None);
    if cx.st.cfg.game_config().game_mode == GameMode::Team {
        cx.populate_teams();
    }
    cx.add_players();
    st
}

impl State {
    /// `maybeAssignTeam(player)` (`GameImpl.ts:920-929`).
    fn maybe_assign_team(&self, player: &PlayerInfo) -> Option<Team> {
        if self.cfg.game_config().game_mode != GameMode::Team {
            return None;
        }
        if player.player_type == PlayerType::Bot {
            return Some(self.bot_team.clone());
        }
        let rand = of_det::util::simple_hash(&player.id);
        // TS: `playerTeams[rand % playerTeams.length]`; simpleHash is a non-negative integer.
        // An empty team list gives `undefined` (index NaN), modelled as no team.
        let n = self.player_teams.len() as f64;
        let i = rand % n;
        if i.is_nan() { None } else { self.player_teams.get(i as usize).cloned() }
    }
}

impl Ctx<'_> {
    /// `populateTeams()` (`GameImpl.ts:220-226`).
    fn populate_teams(&mut self) {
        #[allow(clippy::expect_used)] // rust-invariant: < 2^32 players
        let total = u32::try_from(self.st.humans.len() + self.st.nations.len()).expect("player count fits u32");
        self.st.player_teams = crate::setup::team_assignment::resolve_teams_list(self.st.cfg.player_teams(), total);
    }
    /// `addPlayers()` (`GameImpl.ts:228-265`).
    fn add_players(&mut self) {
        let humans = self.st.humans.clone();
        let nations = self.st.nations.clone();
        if self.st.cfg.game_config().game_mode == GameMode::Ffa {
            for p in humans.iter() {
                self.add_player(p.clone(), None);
            }
            for n in nations.iter() {
                self.add_player(n.player_info.clone(), None);
            }
            return;
        }
        let pt = self.st.cfg.player_teams();
        if pt == TeamCountConfig::HumansVsNations {
            for p in humans.iter() {
                self.add_player(p.clone(), Some(crate::api::types::colored_teams::HUMANS.to_owned()));
            }
            for n in nations.iter() {
                self.add_player(n.player_info.clone(), Some(crate::api::types::colored_teams::NATIONS.to_owned()));
            }
            return;
        }
        // Team mode (`:243-264`)
        let all: Vec<PlayerInfo> =
            humans.iter().cloned().chain(nations.iter().map(|n| n.player_info.clone())).collect();
        let is_duos_trios_quads = matches!(pt, TeamCountConfig::Duos | TeamCountConfig::Trios | TeamCountConfig::Quads);
        let teams = self.st.player_teams.clone();
        // TS: `for (const [playerInfo, team] of playerToTeam.entries())` (`GameImpl.ts:255-262`).
        // The Map's insertion order is NOT `all` order: `assignTeams` inserts pinned players,
        // then clans (largest first), then friend groups, then the shuffled rest
        // (`TeamAssignment.ts:28-176`). `assign_teams_ordered` returns `(index into all, slot)`
        // in that Map order (request P3 R1, merge1); it fixes addPlayer order and every smallID.
        let ordered = crate::setup::team_assignment::assign_teams_ordered(&all, &teams, is_duos_trios_quads, None);
        for (i, slot) in ordered {
            match slot {
                crate::setup::team_assignment::TeamSlot::Kicked => continue, // console.warn dropped
                crate::setup::team_assignment::TeamSlot::Team(t) => {
                    self.add_player(all[i].clone(), Some(t)); // in-range: i indexes all
                }
            }
        }
    }
}

/// The human `PlayerInfo` of a lobby roster entry (`GameRunner.ts:58-69`):
/// `new PlayerInfo(p.username, Human, p.clientID, id, p.isLobbyCreator ?? false, p.clanTag,
/// p.friends ?? [], p.teamIndex ?? null)`.
pub fn human_player_info(p: &GameStartPlayer, id: &str) -> PlayerInfo {
    let mut info = PlayerInfo::new(p.username.clone(), PlayerType::Human, Some(p.client_id.clone()), id);
    info.is_lobby_creator = p.is_lobby_creator.unwrap_or(false);
    info.clan_tag = p.clan_tag.clone();
    info.friends = p.friends.clone().unwrap_or_default();
    // A zod uint team slot beyond u32 is as out-of-range as u32::MAX for any team list.
    info.team_index = p.team_index.map(|t| u32::try_from(t).unwrap_or(u32::MAX));
    info
}

#[cfg(test)]
mod tests {
    //! P1 state methods that run on data alone (the rest need P2/P3 bodies and are covered by
    //! the ported vitests in `tests/p1_vitest_*.rs`, ignored until S2).
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::canon::{Canon, CanonCx, Cs};
    use crate::exec::ExecStore;
    use crate::testkit::{give_tile, raw_player, state_with_map};

    fn info(id: &str, t: PlayerType, client: Option<&str>) -> PlayerInfo {
        PlayerInfo::new(format!("n_{id}"), t, client.map(str::to_owned), id)
    }

    fn panic_msg(f: impl FnOnce() + std::panic::UnwindSafe) -> String {
        #[allow(clippy::disallowed_methods)] // test-only: observe a TsThrow
        let e = std::panic::catch_unwind(f).expect_err("should throw");
        let t = e.downcast_ref::<of_det::throw::TsThrow>().expect("a TsThrow, not a Rust panic");
        t.msg.to_string()
    }

    #[test]
    fn player_lookups_follow_players_map_order() {
        let mut st = state_with_map(4, 4);
        let a = raw_player(&mut st, info("aaaa", PlayerType::Human, Some("c1")), None);
        let b = raw_player(&mut st, info("bbbb", PlayerType::Nation, None), None);
        let c = raw_player(&mut st, info("cccc", PlayerType::Bot, Some("c1")), None);
        assert_eq!((a, b, c), (SmallId(1), SmallId(2), SmallId(3)));
        // players(): alive only (`_tiles.size > 0`), in smallID order; allPlayers(): all.
        assert!(st.players().is_empty());
        give_tile(&mut st, c, 5);
        give_tile(&mut st, a, 6);
        assert_eq!(st.players(), [a, c]);
        assert_eq!(st.all_players(), [a, b, c]);
        assert!(st.has_player("bbbb") && !st.has_player("zzzz"));
        assert_eq!(st.player_by_id("cccc"), c);
        // `playerByClientID` returns the first in `_players` order.
        assert_eq!(st.player_by_client_id("c1"), Some(a));
        assert_eq!(st.player_by_client_id("nope"), None);
        // `player(id)` throws a TS Error with the TS message (GameImpl.ts:931-937).
        let st2 = std::panic::AssertUnwindSafe(&st);
        assert_eq!(
            panic_msg(move || {
                let _ = st2.player_by_id("zzzz");
            }),
            "Player with id zzzz not found"
        );
        // playerBySmallID / owner of TN.
        assert_eq!(st.player_by_small_id(0), SmallId::TERRA_NULLIUS);
        assert_eq!(st.player_by_small_id(2), b);
    }

    /// FILE_MAP §4 P1 design test: `add_player` asserts the id is new. A duplicate would make
    /// TS overwrite the `_players` Map entry in place while still appending to
    /// `_playersBySmallID` (`GameImpl.ts:906-918`), splitting the two orders; the port refuses
    /// with a Rust panic (not a `TsThrow`), before any state is touched.
    #[test]
    fn add_player_rejects_a_duplicate_id_before_any_write() {
        let mut st = state_with_map(2, 2);
        raw_player(&mut st, info("dup", PlayerType::Human, None), None);
        let (n_players, n_infos, next) = (st.players.len(), st.infos.len(), st.next_player_id);
        let mut sc = crate::scratch::Scratch::new();
        let r = {
            let mut cx = Ctx::new(&mut st, &mut sc, None);
            let mut cx = std::panic::AssertUnwindSafe(&mut cx);
            #[allow(clippy::disallowed_methods)] // test-only: observe the panic class
            std::panic::catch_unwind(move || {
                cx.add_player(info("dup", PlayerType::Bot, None), None);
            })
        };
        let e = r.expect_err("duplicate id must panic");
        assert!(e.downcast_ref::<of_det::throw::TsThrow>().is_none(), "a Rust panic, not a TS throw");
        let msg = e.downcast_ref::<String>().cloned().unwrap_or_default();
        assert!(msg.contains("duplicate player id"), "{msg}");
        assert_eq!((st.players.len(), st.infos.len(), st.next_player_id), (n_players, n_infos, next));
    }

    /// Guard (review finding): an out-of-range smallID never becomes a handle, in any build.
    #[test]
    fn player_by_small_id_out_of_range_is_a_rust_bug_panic() {
        let mut st = state_with_map(2, 2);
        raw_player(&mut st, info("a", PlayerType::Human, None), None);
        assert_eq!(st.player_by_small_id(1), SmallId(1));
        let st2 = std::panic::AssertUnwindSafe(&st);
        #[allow(clippy::disallowed_methods)] // test-only: observe the panic class
        let e = std::panic::catch_unwind(move || {
            let _ = st2.player_by_small_id(2);
        })
        .expect_err("smallID past the end must panic");
        assert!(e.downcast_ref::<of_det::throw::TsThrow>().is_none());
    }

    #[test]
    fn find_player_info_falls_back_to_humans_then_nations() {
        let mut st = state_with_map(2, 2);
        let h_only = info("h1", PlayerType::Human, Some("x"));
        let n_only = Nation { spawn_cell: None, player_info: info("n1", PlayerType::Nation, None) };
        st.humans = Arc::from(vec![h_only.clone()]);
        st.nations = Arc::from(vec![n_only.clone()]);
        raw_player(&mut st, info("p1", PlayerType::Bot, None), None);
        assert_eq!(st.find_player_info("p1").map(|i| i.player_type), Some(PlayerType::Bot));
        assert_eq!(st.find_player_info("h1"), Some(&h_only));
        assert_eq!(st.find_player_info("n1"), Some(&n_only.player_info));
        assert_eq!(st.find_player_info("zz"), None);
    }

    #[test]
    fn is_on_map_compares_js_numbers() {
        let st = state_with_map(10, 5);
        assert!(st.is_on_map(Cell::new(0.0, 0.0)));
        assert!(st.is_on_map(Cell::new(9.0, 4.0)));
        assert!(st.is_on_map(Cell::new(9.5, 4.5))); // TS checks bounds only
        assert!(!st.is_on_map(Cell::new(10.0, 0.0)));
        assert!(!st.is_on_map(Cell::new(0.0, 5.0)));
        assert!(!st.is_on_map(Cell::new(-0.5, 0.0)));
        assert!(!st.is_on_map(Cell::new(f64::NAN, 0.0)));
        assert!(st.is_on_map(Cell::new(-0.0, 0.0))); // -0 >= 0
    }

    #[test]
    fn ticks_since_start_and_elapsed_seconds() {
        let mut st = State { ticks: 57, ..Default::default() };
        assert_eq!(st.elapsed_game_seconds(), 0.0); // spawn phase
        st.start_tick = Some(20);
        assert_eq!(st.elapsed_game_seconds(), 3.7);
        // `Math.max(0, ticks - startTick)`
        st.start_tick = Some(80);
        assert_eq!(st.elapsed_game_seconds(), 0.0);
    }

    #[test]
    fn team_spawn_area_lookup() {
        let mut st = State::default();
        assert_eq!(st.team_spawn_area("red"), None); // no areas
        let a = |x: f64| SpawnArea { x, y: 0.0, width: 1.0, height: 1.0 };
        let statics = MapStatics {
            team_game_spawn_areas: Some(Arc::new(vec![(2, vec![a(1.0), a(2.0)]), (3, vec![a(9.0)])])),
            ..Default::default()
        };
        st.statics = Arc::new(statics);
        st.player_teams = vec!["red".into(), "blue".into()];
        assert_eq!(st.team_spawn_area("red"), Some(a(1.0)));
        assert_eq!(st.team_spawn_area("blue"), Some(a(2.0)));
        assert_eq!(st.team_spawn_area("green"), None); // indexOf < 0
        st.player_teams = vec!["red".into(), "blue".into(), "green".into()];
        assert_eq!(st.team_spawn_area("red"), Some(a(9.0)));
        assert_eq!(st.team_spawn_area("blue"), None); // teamIndex >= areas.length
        st.player_teams = vec!["red".into()];
        assert_eq!(st.team_spawn_area("red"), None); // no key "1"
    }

    #[test]
    fn team_tiles_owned_sums_all_players_of_the_team() {
        let mut st = state_with_map(4, 4);
        let a = raw_player(&mut st, info("a", PlayerType::Human, None), Some("red".into()));
        let b = raw_player(&mut st, info("b", PlayerType::Human, None), Some("blue".into()));
        let c = raw_player(&mut st, info("c", PlayerType::Human, None), Some("red".into()));
        for t in [0, 1, 2] {
            give_tile(&mut st, a, t);
        }
        give_tile(&mut st, b, 3);
        give_tile(&mut st, c, 4);
        assert_eq!(st.team_tiles_owned("red"), 4);
        assert_eq!(st.team_tiles_owned("blue"), 1);
        assert_eq!(st.team_tiles_owned("Bot"), 0);
    }

    fn add_unit_data(st: &mut State, owner: SmallId, id: u32, t: UnitType) -> UnitRef {
        let u = st.units.alloc(Unit { id, unit_type: t, owner, active: true, ..Default::default() });
        st.player_data_mut(owner).units.push(u);
        u
    }

    #[test]
    fn units_of_walks_players_then_unit_lists() {
        let mut st = state_with_map(2, 2);
        let a = raw_player(&mut st, info("a", PlayerType::Human, None), None);
        let b = raw_player(&mut st, info("b", PlayerType::Human, None), None);
        let u1 = add_unit_data(&mut st, b, 1, UnitType::City);
        let u2 = add_unit_data(&mut st, a, 2, UnitType::Port);
        let u3 = add_unit_data(&mut st, a, 3, UnitType::City);
        let u4 = add_unit_data(&mut st, b, 4, UnitType::Warship);
        // `_players` order (a then b), then each `_units` array order.
        assert_eq!(st.all_units(), [u2, u3, u1, u4]);
        assert_eq!(st.units_of(&[]), [u2, u3, u1, u4]);
        assert_eq!(st.units_of(&[UnitType::City]), [u3, u1]);
        assert_eq!(st.units_of(&[UnitType::Warship, UnitType::Port]), [u2, u4]);
    }

    #[test]
    fn version_counters_and_plan_driven_gate() {
        let mut st = state_with_map(2, 2);
        let a = raw_player(&mut st, info("a", PlayerType::Human, None), None);
        let u = add_unit_data(&mut st, a, 7, UnitType::Train);
        let mut sc = crate::scratch::Scratch::new();
        let mut cx = Ctx::new(&mut st, &mut sc, None);
        for (t, bump) in [
            (UnitType::City, 1),
            (UnitType::Port, 1),
            (UnitType::Factory, 1),
            (UnitType::DefensePost, 0),
            (UnitType::Train, 0),
            (UnitType::Warship, 0),
        ] {
            let before = cx.st.structure_version;
            cx.note_structure_change(t);
            assert_eq!(cx.st.structure_version - before, bump, "{t:?}");
        }
        cx.bump_units_version();
        assert_eq!(cx.st.units_version, 1);
        cx.set_paused(true);
        assert!(cx.st.is_paused);
        // A plan-driven unit gets no toUpdate (so no shim call: unit_touched is P2's).
        cx.record_motion_plan(7);
        cx.maybe_add_unit_update(u);
        assert_eq!(cx.st.plan_driven_unit_ids.iter().copied().collect::<Vec<_>>(), [7]);
    }

    #[test]
    fn player_ts_string_matches_player_impl_to_string() {
        let mut st = state_with_map(2, 2);
        let a = raw_player(&mut st, info("a", PlayerType::Human, Some("cid")), None);
        let b = raw_player(&mut st, info("b", PlayerType::Nation, None), None);
        st.player_data_mut(a).troops = 1234;
        give_tile(&mut st, a, 0);
        assert_eq!(st.player_ts_string(a), "Player:{name:n_a,clientID:cid,isAlive:true,troops:1234,numTileOwned:1}]");
        assert_eq!(st.player_ts_string(b), "Player:{name:n_b,clientID:null,isAlive:false,troops:0,numTileOwned:0}]");
    }

    #[test]
    fn winner_and_ship_stagger_canon() {
        let st = State::default();
        let ex = ExecStore::default();
        let mut cx = CanonCx::new(&st, &ex);
        assert_eq!(Winner::None.canon(&mut cx), Cs::Null);
        assert_eq!(Winner::Player(SmallId(3)).canon(&mut cx), Cs::Obj(vec![("player".into(), Cs::Int(3))]));
        assert_eq!(Winner::Team("Red".into()).canon(&mut cx), Cs::Obj(vec![("team".into(), Cs::Str("Red".into()))]));
        let s = ShipStaggers { trade_ship: 4, transport_ship: 9 };
        assert_eq!(
            s.canon(&mut cx),
            Cs::Obj(vec![("tradeShip".into(), Cs::Int(4)), ("transportShip".into(), Cs::Int(9))])
        );
    }

    #[test]
    fn next_ship_stagger_counts_per_kind() {
        let mut st = State::default();
        let mut sc = crate::scratch::Scratch::new();
        let mut cx = Ctx::new(&mut st, &mut sc, None);
        let spread = crate::path::pathfinder::WaterPathFinder::STAGGER_SPREAD;
        let got: Vec<i32> = (0..spread + 2).map(|_| cx.next_ship_stagger(ShipKind::TradeShip)).collect();
        assert_eq!(got[0], 0);
        assert_eq!(got[spread as usize], 0); // `shipStaggers[kind]++ % STAGGER_SPREAD`
        assert_eq!(cx.next_ship_stagger(ShipKind::TransportShip), 0);
        assert_eq!(cx.st.ship_staggers, ShipStaggers { trade_ship: spread + 2, transport_ship: 1 });
    }
}
