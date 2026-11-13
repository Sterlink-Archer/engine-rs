//! `Unit` (`engine/src/game/UnitImpl.ts`) and its views. Owner: **P2**.
//!
//! - [`Unit`]: the plain data, fields in `Unit v1` snapshot dump order.
//! - [`UnitView`]: pure readers of the TS `Unit` interface (`game/Game.ts:206-299`). FILE_MAP
//!   calls it the `UnitRef` view; renamed because [`crate::ids::UnitRef`] is the arena handle.
//! - [`UnitMut`]: every mutating method. `warshipState()` is a **mutating getter** in TS
//!   (`UnitImpl.ts:437-442` writes `isInCombat`), so it lives on `UnitMut` (ARCHITECTURE §5).
//! - `toUpdate()`/`touch()` → `cx.st.unit_touched(u)` (the view side-effect shim, §5).
//!
//! A deleted unit stays readable while referenced (ARCHITECTURE §3.2); only the GC frees it.

use crate::StateClone;
use crate::api::types::{Gold, TrainType, UnitInfo, UnitType};
use crate::api::views::{PlayerLike, UnitLike};
use crate::canon::{Canon, CanonCx, Cs, big};
use crate::ctx::Ctx;
use crate::ids::{Owner, SmallId, TileRef, UnitRef};
use crate::plain_copy;
use crate::state::State;
use crate::state::player::PlayerRef;
use crate::state::stats::StatNum;
use crate::util::within_int;
use of_canon::Typed;
use of_det::TsExpect;
use of_det::js;
use of_det::util::{max_health_with_veterancy, simple_hash, to_int_strict};

/// `WarshipState.state` (`GameTypes.ts:20`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WarshipMode {
    #[default]
    Patrolling,
    Retreating,
    Docked,
}
plain_copy!(WarshipMode);

impl WarshipMode {
    pub fn as_str(self) -> &'static str {
        match self {
            WarshipMode::Patrolling => "patrolling",
            WarshipMode::Retreating => "retreating",
            WarshipMode::Docked => "docked",
        }
    }
}

/// `WarshipState` (`GameTypes.ts:19-30`); snapshot `warshipState` record order.
#[derive(StateClone, Debug, Default, PartialEq, Eq)]
pub struct WarshipState {
    pub state: WarshipMode,
    pub patrol_tile: Option<TileRef>,
    pub retreat_port: Option<TileRef>,
    /// `isInCombat?` — absent right after the constructor / `updateWarshipState` until the
    /// `toUpdate` shim writes it (`UnitImpl.ts:104-110,459-467`; ARCHITECTURE §3.4).
    pub is_in_combat: Option<bool>,
    pub last_combat_tick: i32,
    pub veterancy: i32,
    pub veterancy_progress: i32,
}

/// `Partial<WarshipState>` (`updateWarshipState`, `UnitImpl.ts:445`). `Some(None)` writes
/// `undefined` explicitly.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WarshipStateUpdate {
    pub state: Option<WarshipMode>,
    pub patrol_tile: Option<Option<TileRef>>,
    pub retreat_port: Option<Option<TileRef>>,
    pub is_in_combat: Option<Option<bool>>,
    pub last_combat_tick: Option<i32>,
    pub veterancy: Option<i32>,
    pub veterancy_progress: Option<i32>,
}

/// `TransportShipState` (`GameTypes.ts:32-35`).
#[derive(StateClone, Debug, Default, PartialEq)]
pub struct TransportShipState {
    pub is_retreating: bool,
    pub troops: f64,
}

/// `Partial<TransportShipState>`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TransportShipStateUpdate {
    pub is_retreating: Option<bool>,
    pub troops: Option<f64>,
}

/// `NukeState` (`GameTypes.ts:37-42`); snapshot `{trajectoryTiles, trajectoryIndex,
/// targetedBySam, waitTicks}`.
#[derive(StateClone, Debug, Default, PartialEq, Eq)]
pub struct NukeState {
    pub trajectory: Vec<TileRef>,
    pub trajectory_index: i32,
    pub targeted_by_sam: bool,
    pub wait_ticks: i32,
}

/// `Partial<NukeState>`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NukeStateUpdate {
    pub trajectory: Option<Vec<TileRef>>,
    pub trajectory_index: Option<i32>,
    pub targeted_by_sam: Option<bool>,
    pub wait_ticks: Option<i32>,
}

/// `SamLauncherState` (`GameTypes.ts:44-49`).
#[derive(StateClone, Debug, Default, PartialEq)]
pub struct SamLauncherState {
    pub upgrade_start_tick: Option<i32>,
    pub start_range: f64,
    pub target_level: i32,
    pub duration: i32,
}

/// `UnitParams<T>` (`game/Game.ts:61-124`), flattened: each `Some` is a key present in the
/// TS params object (`"k" in params`, `UnitImpl.ts:66-128`). For keys TS reads with
/// `params.k ?? undefined`, `Some(None)` = key present with value `undefined`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UnitParams {
    /// TransportShip.
    pub troops: Option<f64>,
    /// TransportShip, nukes, MIRV.
    pub target_tile: Option<Option<TileRef>>,
    /// Warship.
    pub patrol_tile: Option<TileRef>,
    /// SAMMissile, TradeShip, Train.
    pub target_unit: Option<Option<UnitRef>>,
    /// AtomBomb, HydrogenBomb, MIRVWarhead.
    pub trajectory: Option<Vec<TileRef>>,
    /// MIRV.
    pub target_player: Option<Option<Owner>>,
    /// TradeShip.
    pub last_set_safe_from_pirates: Option<Option<i32>>,
    /// Train.
    pub train_type: Option<TrainType>,
    /// Train.
    pub loaded: Option<Option<bool>>,
}

/// `UnitImpl` data, `Unit v1` snapshot order.
#[derive(StateClone, Debug, Default)]
pub struct Unit {
    /// `_id` (`nextUnitID()`). // int: monotonic counter.
    pub id: u32,
    /// `_type`.
    pub unit_type: UnitType,
    /// `_owner`.
    pub owner: SmallId,
    /// `_tile`.
    pub tile: TileRef,
    pub last_tile: TileRef,
    /// `_active`.
    pub active: bool,
    pub target_tile: Option<TileRef>,
    /// `Player | TerraNullius | undefined`.
    pub target_player: Option<Owner>,
    pub target_unit: Option<UnitRef>,
    /// `_health` (bigint).
    pub health: i64,
    pub transport_ship_state: Option<TransportShipState>,
    pub warship_state: Option<WarshipState>,
    pub nuke_state: Option<NukeState>,
    pub reached_target: bool,
    pub was_destroyed_by_enemy: bool,
    pub destroyer: Option<SmallId>,
    pub last_set_safe_from_pirates: i32,
    pub under_construction: bool,
    pub last_owner: Option<SmallId>,
    /// `_troops: number`.
    pub troops: f64,
    pub missile_timer_queue: Vec<i32>,
    pub has_train_station: bool,
    pub level: i32,
    pub targetable: bool,
    pub loaded: Option<bool>,
    pub train_type: Option<TrainType>,
    pub deletion_at: Option<i32>,
    pub sam_launcher_state: Option<SamLauncherState>,
}

/// Read view.
#[derive(Clone, Copy)]
pub struct UnitView<'a> {
    pub st: &'a State,
    pub u: UnitRef,
}

/// Mutable view.
pub struct UnitMut<'a, 'c> {
    pub cx: &'a mut Ctx<'c>,
    pub u: UnitRef,
}

/// `isInCombat()` on a warship state (`UnitImpl.ts:470-472`).
#[inline]
fn in_combat(ticks: i32, ws: &WarshipState) -> bool {
    ticks - ws.last_combat_tick <= 3
}

/// `MissileSilo`/`SAMLauncher` (`[..].includes(this.type())`, `UnitImpl.ts:759,767`).
#[inline]
fn has_missile_queue(t: UnitType) -> bool {
    matches!(t, UnitType::MissileSilo | UnitType::SamLauncher)
}

/// The `switch` of `UnitImpl.ts:118-127,252-262,386-395`: structures and warships
/// (`OtherUnitType`).
#[inline]
fn is_other_unit_type(t: UnitType) -> bool {
    matches!(
        t,
        UnitType::Warship
            | UnitType::Port
            | UnitType::MissileSilo
            | UnitType::DefensePost
            | UnitType::SamLauncher
            | UnitType::City
            | UnitType::Factory
    )
}

/// A JS number that TS keeps integral (a `zInt` field fed by config numbers) back to `i32`.
#[inline]
fn int_field(x: f64) -> i32 {
    debug_assert!(x.is_finite() && x.trunc() == x && x.abs() < 2_147_483_648.0, "int field {x}");
    #[allow(clippy::cast_possible_truncation)]
    // rust-invariant: integral and in i32 range (checked above in debug builds)
    let v = x as i32;
    v
}

impl Unit {
    /// `new UnitImpl(type, mg, tile, id, owner, params)` (`UnitImpl.ts:66-128`), field
    /// initialisers first, then the constructor body in TS order, ending with the
    /// `stats().unitBuild` call. Returns the unit data; the caller (`buildUnit`) allocates it.
    pub(crate) fn construct(
        cx: &mut Ctx<'_>,
        t: UnitType,
        tile: TileRef,
        id: u32,
        owner: SmallId,
        params: &UnitParams,
    ) -> Unit {
        let cfg = cx.st.cfg.clone();
        let mut u = Unit {
            id,
            unit_type: t,
            owner,
            tile,
            // field initialisers (`:39-64`)
            active: true,
            target_tile: None,
            target_player: None,
            target_unit: None,
            health: 0,
            transport_ship_state: None,
            warship_state: None,
            nuke_state: None,
            reached_target: false,
            was_destroyed_by_enemy: false,
            destroyer: None,
            last_set_safe_from_pirates: 0,
            under_construction: false,
            last_owner: None,
            troops: 0.0,
            missile_timer_queue: Vec::new(),
            has_train_station: false,
            level: 1,
            targetable: true,
            loaded: None,
            train_type: None,
            deletion_at: None,
            sam_launcher_state: None,
            last_tile: tile, // TS: `this._lastTile = _tile` (`:74`)
        };
        // TS: `toInt(this.mg.unitInfo(_type).maxHealth ?? 1)` (`:75`).
        u.health = to_int_strict(cfg.unit_info(t).max_health.unwrap_or(1.0));
        // `"targetTile" in params ? (params.targetTile ?? undefined) : undefined` (`:76-77`).
        u.target_tile = params.target_tile.flatten();
        u.target_player = params.target_player.flatten();
        // `"trajectory" in params || "waitTicks" in params` (`:80-87`); no UnitParams type has
        // a `waitTicks` key (`Game.ts:59-118`).
        if let Some(traj) = &params.trajectory {
            u.nuke_state = Some(NukeState {
                trajectory: traj.clone(),
                wait_ticks: 0,
                trajectory_index: 0,
                targeted_by_sam: false,
            });
        }
        // `"troops" in params ? (params.troops ?? 0) : 0` (`:88`).
        u.troops = params.troops.unwrap_or(0.0);
        u.last_set_safe_from_pirates = params.last_set_safe_from_pirates.flatten().unwrap_or(0);
        if t == UnitType::TransportShip {
            u.transport_ship_state = Some(TransportShipState { is_retreating: false, troops: 0.0 });
        }
        if t == UnitType::SamLauncher {
            u.sam_launcher_state = Some(SamLauncherState {
                upgrade_start_tick: None,
                start_range: cfg.sam_range(1.0),
                target_level: 1,
                duration: cfg.sam_upgrade_duration(),
            });
        }
        if let Some(patrol_tile) = params.patrol_tile {
            // `:103-111`: no `retreatPort`, no `isInCombat` key.
            u.warship_state = Some(WarshipState {
                state: WarshipMode::Patrolling,
                patrol_tile: Some(patrol_tile),
                retreat_port: None,
                is_in_combat: None,
                last_combat_tick: -100,
                veterancy: 0,
                veterancy_progress: 0,
            });
        }
        u.target_unit = params.target_unit.flatten();
        u.loaded = params.loaded.flatten();
        u.train_type = params.train_type;
        if is_other_unit_type(t) {
            cx.st.stats_cx().unit_build(owner, t); // `:118-127`
        }
        u
    }
}

impl State {
    /// A unit read view.
    #[inline]
    pub fn unit(&self, u: UnitRef) -> UnitView<'_> {
        UnitView { st: self, u }
    }
    /// The raw data of `u` (generation-checked).
    #[inline]
    pub fn unit_data(&self, u: UnitRef) -> &Unit {
        self.units.get(u)
    }
    #[inline]
    pub fn unit_data_mut(&mut self, u: UnitRef) -> &mut Unit {
        self.units.get_mut(u)
    }
    /// The `toUpdate()`/`touch()` shim (ARCHITECTURE §5): only the state writes of
    /// `toUpdate`, i.e. `warshipState()`'s `isInCombat` write (`UnitImpl.ts:437-442`).
    /// `toUpdate` calls `warshipState()` only when `_warshipState !== undefined`
    /// (`:169-172`); every other getter it calls is pure.
    pub fn unit_touched(&mut self, u: UnitRef) {
        let ticks = self.ticks;
        if let Some(ws) = &mut self.units.get_mut(u).warship_state {
            ws.is_in_combat = Some(in_combat(ticks, ws));
        }
    }
}

impl<'c> Ctx<'c> {
    #[inline]
    pub fn unit_mut(&mut self, u: UnitRef) -> UnitMut<'_, 'c> {
        UnitMut { cx: self, u }
    }
}

impl<'a> UnitView<'a> {
    #[inline]
    pub fn data(&self) -> &'a Unit {
        self.st.unit_data(self.u)
    }
    /// `id()`. (`isUnit()` is always true for a unit and has no port.)
    pub fn id(&self) -> u32 {
        self.data().id
    }
    /// `type()`.
    pub fn unit_type(&self) -> UnitType {
        self.data().unit_type
    }
    pub fn owner(&self) -> SmallId {
        self.data().owner
    }
    /// `info()` (`EngineUnitInfo`; cost via `EngineConfig::unit_cost`).
    pub fn info(&self) -> UnitInfo {
        self.st.cfg.unit_info(self.data().unit_type)
    }
    /// `isMarkedForDeletion()` (`:324-326`).
    pub fn is_marked_for_deletion(&self) -> bool {
        self.data().deletion_at.is_some()
    }
    /// `isOverdueDeletion()` (`:337-342`).
    pub fn is_overdue_deletion(&self) -> bool {
        let d = self.data();
        if !d.active {
            return false;
        }
        match d.deletion_at {
            Some(at) => self.st.ticks - at > 0,
            None => false,
        }
    }
    pub fn tile(&self) -> TileRef {
        self.data().tile
    }
    pub fn last_tile(&self) -> TileRef {
        self.data().last_tile
    }
    pub fn is_active(&self) -> bool {
        self.data().active
    }
    /// `hash()` (legacy hash, on demand): `tile() + simpleHash(type()) * _id` (`:556-558`).
    pub fn hash(&self) -> f64 {
        let d = self.data();
        f64::from(d.tile) + simple_hash(d.unit_type.as_str()) * f64::from(d.id)
    }
    pub fn has_train_station(&self) -> bool {
        self.data().has_train_station
    }
    pub fn was_destroyed_by_enemy(&self) -> bool {
        self.data().was_destroyed_by_enemy
    }
    pub fn destroyer(&self) -> Option<SmallId> {
        self.data().destroyer
    }
    pub fn train_type(&self) -> Option<TrainType> {
        self.data().train_type
    }
    pub fn is_loaded(&self) -> Option<bool> {
        self.data().loaded
    }
    /// `tileTarget()` (`UnitImpl.ts:151`).
    pub fn tile_target(&self) -> Option<TileRef> {
        self.data().target_tile
    }
    pub fn target_tile(&self) -> Option<TileRef> {
        self.data().target_tile
    }
    pub fn target_player(&self) -> Option<Owner> {
        self.data().target_player
    }
    /// `trajectoryIndex()`: `_nukeState?.trajectoryIndex ?? 0` (`:610-612`).
    pub fn trajectory_index(&self) -> i32 {
        self.data().nuke_state.as_ref().map_or(0, |n| n.trajectory_index)
    }
    /// `trajectory()`: `_nukeState?.trajectory ?? []` (`:614-616`).
    pub fn trajectory(&self) -> &'a [TileRef] {
        self.data().nuke_state.as_ref().map_or(&[], |n| n.trajectory.as_slice())
    }
    pub fn target_unit(&self) -> Option<UnitRef> {
        self.data().target_unit
    }
    /// `targetedBySAM()`: `this._nukeState!.targetedBySam` (`:630-632`; TypeError on a
    /// non-nuke).
    pub fn targeted_by_sam(&self) -> bool {
        self.data().nuke_state.as_ref().ts_expect("UnitImpl.ts:631 _nukeState!.targetedBySam").targeted_by_sam
    }
    pub fn reached_target(&self) -> bool {
        self.data().reached_target
    }
    pub fn is_targetable(&self) -> bool {
        self.data().targetable
    }
    /// `hasHealth()`: `info().maxHealth !== undefined` (`:236-238`).
    pub fn has_health(&self) -> bool {
        self.info().max_health.is_some()
    }
    /// `isInCombat()` (`UnitImpl.ts:470-472`): `this._warshipState!` (TypeError on a
    /// non-warship).
    pub fn is_in_combat(&self) -> bool {
        let ws = self.data().warship_state.as_ref().ts_expect("UnitImpl.ts:471 _warshipState!.lastCombatTick");
        in_combat(self.st.ticks, ws)
    }
    /// `transportShipState()`; throws on a non-transport (`TsThrow`).
    ///
    /// TS returns a **fresh** `{ isRetreating, troops: this._troops }` (`:482-490`); this
    /// returns the stored state, whose `troops` is the snapshot field (always 0). Every TS
    /// caller reads only `.isRetreating` (`TransportShipExecution.ts:218,225`,
    /// `NationWarshipBehavior.ts:194,208`); [`Self::transport_ship_state_value`] gives the TS
    /// value.
    pub fn transport_ship_state(&self) -> &'a TransportShipState {
        match &self.data().transport_ship_state {
            Some(s) => s,
            None => of_det::ts_throw!("transportShipState called on non-transport-ship unit"),
        }
    }
    /// `transportShipState()` as TS returns it (`troops` = the unit's `_troops`).
    pub fn transport_ship_state_value(&self) -> TransportShipState {
        let s = self.transport_ship_state();
        TransportShipState { is_retreating: s.is_retreating, troops: self.data().troops }
    }
    /// `nukeState()` (`:514-519`).
    pub fn nuke_state(&self) -> &'a NukeState {
        match &self.data().nuke_state {
            Some(s) => s,
            None => of_det::ts_throw!("nukeState called on non-nuke unit"),
        }
    }
    /// `health(): number` = `Number(this._health)`.
    pub fn health(&self) -> f64 {
        self.data().health as f64
    }
    /// `maxHealth()` (`:284-292`).
    pub fn max_health(&self) -> f64 {
        let base = self.info().max_health.unwrap_or(1.0);
        max_health_with_veterancy(base, f64::from(self.veterancy()), self.st.cfg.warship_veterancy_health_bonus())
    }
    /// `veterancy()`: `_warshipState?.veterancy ?? 0` (`:657-659`).
    pub fn veterancy(&self) -> i32 {
        self.data().warship_state.as_ref().map_or(0, |w| w.veterancy)
    }
    pub fn troops(&self) -> f64 {
        self.data().troops
    }
    /// `ticksLeftInCooldown()` (`UnitImpl.ts:569`): `missileTimerQueue[0]`.
    pub fn ticks_left_in_cooldown(&self) -> Option<i32> {
        self.data().missile_timer_queue.first().copied()
    }
    /// `isInCooldown()`: `_missileTimerQueue.length === this._level` (`:573-575`).
    pub fn is_in_cooldown(&self) -> bool {
        let d = self.data();
        d.missile_timer_queue.len() as i64 == i64::from(d.level)
    }
    pub fn missile_timer_queue(&self) -> &'a [i32] {
        &self.data().missile_timer_queue
    }
    pub fn sam_launcher_state(&self) -> Option<&'a SamLauncherState> {
        self.data().sam_launcher_state.as_ref()
    }
    /// `isSafeFromPirates()` (`:646-651`).
    pub fn is_safe_from_pirates(&self) -> bool {
        f64::from(self.st.ticks - self.data().last_set_safe_from_pirates) < self.st.cfg.safe_from_pirates_cooldown_max()
    }
    pub fn is_under_construction(&self) -> bool {
        self.data().under_construction
    }
    pub fn level(&self) -> i32 {
        self.data().level
    }
    /// `toString()`: `Unit:${type},owner:${owner().name()}` (`:560-562`; throw messages).
    pub fn ts_to_string(&self) -> String {
        let d = self.data();
        let owner = self.st.player_data(d.owner);
        format!("Unit:{},owner:{}", d.unit_type.as_str(), self.st.info(owner.info).name)
    }
}

/// `Unit extends UnitLike` (`game/Game.ts:204`).
impl UnitLike for UnitView<'_> {
    fn unit_type(&self) -> UnitType {
        UnitView::unit_type(self)
    }
    fn owner(&self) -> impl PlayerLike + '_ {
        PlayerRef { st: self.st, id: UnitView::owner(self) }
    }
    fn tile(&self) -> TileRef {
        UnitView::tile(self)
    }
    fn last_tile(&self) -> TileRef {
        UnitView::last_tile(self)
    }
    fn is_active(&self) -> bool {
        UnitView::is_active(self)
    }
    fn is_under_construction(&self) -> bool {
        UnitView::is_under_construction(self)
    }
    fn level(&self) -> f64 {
        f64::from(UnitView::level(self))
    }
}

impl<'a, 'c> UnitMut<'a, 'c> {
    #[inline]
    pub fn r(&self) -> UnitView<'_> {
        UnitView { st: &*self.cx.st, u: self.u }
    }
    #[inline]
    pub fn data(&mut self) -> &mut Unit {
        self.cx.st.unit_data_mut(self.u)
    }
    /// `this.mg.addUpdate(this.toUpdate())` → the shim (ARCHITECTURE §5).
    #[inline]
    fn shim_update(&mut self) {
        self.cx.st.unit_touched(self.u);
    }

    /// `markForDeletion()` (`:328-335`).
    pub fn mark_for_deletion(&mut self) {
        if !self.r().is_active() {
            return;
        }
        let at = self.cx.st.ticks + self.cx.st.cfg.deletion_mark_duration();
        self.data().deletion_at = Some(at);
        self.shim_update(); // UnitImpl.ts:334
    }
    /// `clearPendingDeletion()` (`UnitImpl.ts:320`).
    pub fn clear_pending_deletion(&mut self) {
        self.data().deletion_at = None;
    }
    /// `delete(displayMessage?, destroyer?)` (`UnitImpl.ts:344-400`); throws if inactive.
    pub fn delete(&mut self, display_message: Option<bool>, destroyer: Option<SmallId>) {
        if !self.r().is_active() {
            let s = self.r().ts_to_string();
            of_det::ts_throw!("cannot delete {s} not active");
        }
        let u = self.u;
        {
            let d = self.data();
            d.was_destroyed_by_enemy = destroyer.is_some();
            d.destroyer = destroyer;
        }
        let owner = self.r().owner();
        {
            // TS: `this._owner._units = this._owner._units.filter((b) => b !== this)` (`:353`).
            let p = self.cx.st.player_data_mut(owner);
            p.units.filter_reassign(|x| *x != u);
            p.my_units_version += 1;
        }
        self.data().active = false;
        self.shim_update(); // UnitImpl.ts:356
        self.cx.remove_unit(u);

        let was_destroyed = display_message != Some(false); // `displayMessage !== false`
        let t = self.r().unit_type();
        if was_destroyed {
            // `displayMessageOnDeleted()` is view-only (`:400-423`).
            if matches!(t, UnitType::TransportShip | UnitType::TradeShip) {
                self.cx.st.stats_cx().boat_lose(owner, t);
            }
        }
        if let Some(destroyer) = destroyer {
            match t {
                UnitType::TransportShip => {
                    let troops = self.r().troops();
                    self.cx.st.stats_cx().boat_destroy_troops(destroyer, owner, StatNum::Num(troops));
                }
                UnitType::TradeShip => self.cx.st.stats_cx().boat_destroy_trade(destroyer, owner),
                UnitType::City
                | UnitType::DefensePost
                | UnitType::MissileSilo
                | UnitType::Port
                | UnitType::SamLauncher
                | UnitType::Warship
                | UnitType::Factory => {
                    self.cx.st.stats_cx().unit_destroy(destroyer, t);
                    let owner_now = self.r().owner(); // TS: `this.owner()` (unchanged)
                    self.cx.st.stats_cx().unit_lose(owner_now, t);
                }
                _ => {}
            }
        }
    }
    /// `move(tile)` (→ `UnitGrid.updateUnitCell`, `:213-220`). TS throws on `null`; a Rust
    /// `TileRef` cannot be null.
    pub fn move_to(&mut self, tile: TileRef) {
        let d = self.data();
        d.last_tile = d.tile;
        d.tile = tile;
        let u = self.u;
        self.cx.on_unit_moved(u);
    }
    /// `setOwner(owner)` (`UnitImpl.ts:250-282`): filter-reassigns the old owner's `_units`.
    pub fn set_owner(&mut self, owner: SmallId) {
        self.clear_pending_deletion();
        let t = self.r().unit_type();
        let old = self.r().owner();
        if is_other_unit_type(t) {
            self.cx.st.stats_cx().unit_capture(owner, t);
            self.cx.st.stats_cx().unit_lose(old, t);
        } else if t == UnitType::TransportShip {
            self.cx.st.stats_cx().boat_captured_troops(owner, old);
        }
        let u = self.u;
        self.data().last_owner = Some(old);
        {
            // `this._lastOwner._units = this._lastOwner._units.filter((u) => u !== this)` (`:275`).
            let p = self.cx.st.player_data_mut(old);
            p.units.filter_reassign(|x| *x != u);
            p.my_units_version += 1;
        }
        self.data().owner = owner;
        {
            let p = self.cx.st.player_data_mut(owner);
            p.units.push(u);
            p.my_units_version += 1;
        }
        self.cx.bump_units_version();
        self.shim_update(); // UnitImpl.ts:281
    }
    /// `touch()` (`UnitImpl.ts:145-147`) = the shim.
    pub fn touch(&mut self) {
        self.shim_update();
    }
    /// `setTrainStation(trainStation)` (`:735-738`).
    pub fn set_train_station(&mut self, train_station: bool) {
        self.data().has_train_station = train_station;
        self.shim_update(); // UnitImpl.ts:737
    }
    /// `setLoaded(loaded)` (`:791-796`).
    pub fn set_loaded(&mut self, loaded: bool) {
        if self.r().is_loaded() != Some(loaded) {
            self.data().loaded = Some(loaded);
            self.shim_update(); // UnitImpl.ts:794
        }
    }
    /// `setTileTarget(tile)` (`UnitImpl.ts:148`).
    pub fn set_tile_target(&mut self, tile: Option<TileRef>) {
        self.data().target_tile = tile;
    }
    pub fn set_target_tile(&mut self, tile: Option<TileRef>) {
        self.data().target_tile = tile;
    }
    /// `setTrajectoryIndex(i)` (`:602-608`): clamps to `[0, trajectory.length - 1]` (an empty
    /// trajectory stores -1).
    pub fn set_trajectory_index(&mut self, i: i32) {
        if self.r().data().nuke_state.is_none() {
            of_det::ts_throw!("setTrajectoryIndex called on non-nuke unit");
        }
        #[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
        // int: a trajectory is far shorter than 2^31 tiles
        let max = self.r().trajectory().len() as i32 - 1;
        let v = if i < 0 {
            0
        } else if i > max {
            max
        } else {
            i
        };
        if let Some(n) = &mut self.data().nuke_state {
            n.trajectory_index = v;
        }
    }
    pub fn set_target_unit(&mut self, unit: Option<UnitRef>) {
        self.data().target_unit = unit;
    }
    /// `setTargetedBySAM(targeted)`: `this._nukeState!.targetedBySam = targeted` (`:626-628`).
    pub fn set_targeted_by_sam(&mut self, targeted: bool) {
        self.data().nuke_state.as_mut().ts_expect("UnitImpl.ts:627 _nukeState!.targetedBySam").targeted_by_sam =
            targeted;
    }
    pub fn set_reached_target(&mut self) {
        self.data().reached_target = true;
    }
    /// `setTargetable(targetable)` (`:130-135`).
    pub fn set_targetable(&mut self, targetable: bool) {
        if self.r().is_targetable() != targetable {
            self.data().targetable = targetable;
            self.shim_update(); // UnitImpl.ts:133
        }
    }
    /// `warshipState()`: the **mutating** getter (writes `isInCombat`, `UnitImpl.ts:437-442`).
    pub fn warship_state(&mut self) -> &WarshipState {
        let ticks = self.cx.st.ticks;
        match &mut self.data().warship_state {
            Some(ws) => {
                ws.is_in_combat = Some(in_combat(ticks, ws));
                ws
            }
            None => of_det::ts_throw!("warshipState called on non-warship unit"),
        }
    }
    /// `updateWarshipState(update)` (`:445-467`), including the trailing shim.
    pub fn update_warship_state(&mut self, update: WarshipStateUpdate) {
        if self.r().data().warship_state.is_none() {
            of_det::ts_throw!("updateWarshipState called on non-warship unit");
        }
        // `if (update.isInCombat)`: truthy only for `true`.
        if update.is_in_combat == Some(Some(true)) {
            self.mark_in_combat();
        }
        #[allow(clippy::expect_used)] // rust-invariant: checked above
        let cur = self.r().data().warship_state.clone().expect("warship state");
        // `merged = { ...this._warshipState, ...update }`: a key present in `update` wins,
        // even with value `undefined`.
        let m_state = update.state.unwrap_or(cur.state);
        let m_patrol = update.patrol_tile.unwrap_or(cur.patrol_tile);
        let m_retreat = update.retreat_port.unwrap_or(cur.retreat_port);
        if m_state == cur.state && m_patrol == cur.patrol_tile && m_retreat == cur.retreat_port {
            return;
        }
        self.data().warship_state = Some(WarshipState {
            state: m_state,
            patrol_tile: m_patrol,
            retreat_port: m_retreat,
            is_in_combat: None, // not copied (`:459-466`)
            last_combat_tick: cur.last_combat_tick,
            veterancy: cur.veterancy,
            veterancy_progress: cur.veterancy_progress,
        });
        self.shim_update(); // UnitImpl.ts:467
    }
    /// `markInCombat()` (`:474-480`).
    fn mark_in_combat(&mut self) {
        let was_in_combat = self.r().is_in_combat();
        let ticks = self.cx.st.ticks;
        if let Some(ws) = &mut self.data().warship_state {
            ws.last_combat_tick = ticks;
        }
        if !was_in_combat {
            self.shim_update(); // UnitImpl.ts:478
        }
    }
    /// `updateTransportShipState(update)` (`:492-512`): only `isRetreating` is applied.
    pub fn update_transport_ship_state(&mut self, update: TransportShipStateUpdate) {
        let Some(cur) = self.r().data().transport_ship_state.as_ref().map(|s| s.is_retreating) else {
            of_det::ts_throw!("updateTransportShipState called on non-transport-ship unit");
        };
        let mut changed = false;
        if let Some(r) = update.is_retreating
            && cur != r
        {
            if let Some(s) = &mut self.data().transport_ship_state {
                s.is_retreating = r; // `{ ...state, isRetreating }`
            }
            changed = true;
        }
        if changed {
            self.shim_update(); // UnitImpl.ts:510
        }
    }
    /// `updateNukeState(update)` (`:521-539`): `trajectory` in `update` is ignored.
    pub fn update_nuke_state(&mut self, update: NukeStateUpdate) {
        let Some(cur) =
            self.r().data().nuke_state.as_ref().map(|n| (n.targeted_by_sam, n.trajectory_index, n.wait_ticks))
        else {
            of_det::ts_throw!("updateNukeState called on non-nuke unit");
        };
        let m_sam = update.targeted_by_sam.unwrap_or(cur.0);
        let m_idx = update.trajectory_index.unwrap_or(cur.1);
        let m_wait = update.wait_ticks.unwrap_or(cur.2);
        if m_sam == cur.0 && m_idx == cur.1 && m_wait == cur.2 {
            return;
        }
        if let Some(n) = &mut self.data().nuke_state {
            n.targeted_by_sam = m_sam;
            n.trajectory_index = m_idx;
            n.wait_ticks = m_wait;
        }
        self.shim_update(); // UnitImpl.ts:538
    }
    /// `modifyHealth(delta, attacker?)` (`:294`).
    pub fn modify_health(&mut self, delta: f64, attacker: Option<SmallId>) {
        let previous_health = self.r().data().health;
        // `withinInt(this._health + toInt(delta), 0n, toInt(this.maxHealth()))`
        let sum = previous_health + to_int_strict(delta);
        let max = to_int_strict(self.r().max_health());
        let next_health = within_int(sum, 0, max);
        if next_health == previous_health {
            return;
        }
        if attacker.is_some() && delta < 0.0 {
            let ticks = self.cx.st.ticks;
            if let Some(ws) = &mut self.data().warship_state {
                ws.last_combat_tick = ticks;
            }
        }
        self.data().health = next_health;
        self.shim_update(); // UnitImpl.ts:314
        if self.r().data().health == 0 {
            self.delete(Some(true), attacker);
        }
    }
    /// `recordKill(targetType)` (`:677-689`).
    pub fn record_kill(&mut self, target_type: UnitType) {
        if self.r().data().warship_state.is_none() {
            return;
        }
        if target_type == UnitType::Warship {
            if let Some(ws) = &mut self.data().warship_state {
                ws.veterancy_progress = 0;
            }
            self.increase_veterancy();
        } else if target_type == UnitType::TransportShip {
            self.add_veterancy_progress(UnitType::TransportShip);
        }
    }
    /// `recordTradeCapture()` (`:691-696`).
    pub fn record_trade_capture(&mut self) {
        if self.r().data().warship_state.is_none() {
            return;
        }
        self.add_veterancy_progress(UnitType::TradeShip);
    }
    /// `increaseVeterancy()` (`:664-675`).
    fn increase_veterancy(&mut self) {
        let Some(vet) = self.r().data().warship_state.as_ref().map(|w| w.veterancy) else { return };
        if f64::from(vet) >= self.cx.st.cfg.warship_max_veterancy() {
            return;
        }
        if let Some(ws) = &mut self.data().warship_state {
            ws.veterancy += 1;
        }
        self.shim_update(); // UnitImpl.ts:674
    }
    /// `addVeterancyProgress(source)` (`:708-733`). Config thresholds are JS numbers; the
    /// progress meter is a `zInt` field, so the f64 sums are stored back as integers.
    fn add_veterancy_progress(&mut self, source: UnitType) {
        if self.r().data().warship_state.is_none() {
            return;
        }
        let cfg = self.cx.st.cfg.clone();
        let max_veterancy = cfg.warship_max_veterancy();
        let vet = |s: &Self| s.r().data().warship_state.as_ref().map_or(0, |w| w.veterancy);
        let prog = |s: &Self| s.r().data().warship_state.as_ref().map_or(0, |w| w.veterancy_progress);
        if f64::from(vet(self)) >= max_veterancy {
            return;
        }
        let transport_threshold = cfg.warship_veterancy_transport_kills();
        let capture_threshold = cfg.warship_veterancy_trade_captures();
        let points_per_level = transport_threshold * capture_threshold;
        let add = if source == UnitType::TransportShip { capture_threshold } else { transport_threshold };
        let np = int_field(f64::from(prog(self)) + add);
        if let Some(ws) = &mut self.data().warship_state {
            ws.veterancy_progress = np;
        }
        while f64::from(prog(self)) >= points_per_level && f64::from(vet(self)) < max_veterancy {
            let np = int_field(f64::from(prog(self)) - points_per_level);
            if let Some(ws) = &mut self.data().warship_state {
                ws.veterancy_progress = np;
            }
            self.increase_veterancy();
        }
        if f64::from(vet(self)) >= max_veterancy
            && let Some(ws) = &mut self.data().warship_state
        {
            ws.veterancy_progress = 0;
        }
    }
    /// `setTroops(troops)` (`:222-229`): `Math.max(0, troops)`; NaN stays NaN and always
    /// differs.
    pub fn set_troops(&mut self, troops: f64) {
        let next_troops = js::max(0.0, troops);
        #[allow(clippy::float_cmp)] // TS `===` on numbers
        let same = self.r().data().troops == next_troops;
        if same {
            return;
        }
        self.data().troops = next_troops;
        self.shim_update(); // UnitImpl.ts:228
    }
    /// `launch()` (`:564-567`).
    pub fn launch(&mut self) {
        let t = self.cx.st.ticks;
        self.data().missile_timer_queue.push(t);
        self.shim_update(); // UnitImpl.ts:566
    }
    /// `reloadMissile()` (`:585-588`): `shift()` (a no-op on an empty queue).
    pub fn reload_missile(&mut self) {
        let q = &mut self.data().missile_timer_queue;
        if !q.is_empty() {
            q.remove(0);
        }
        self.shim_update(); // UnitImpl.ts:587
    }
    /// `setSafeFromPirates()` (`:642-644`).
    pub fn set_safe_from_pirates(&mut self) {
        let t = self.cx.st.ticks;
        self.data().last_set_safe_from_pirates = t;
    }
    /// `setUnderConstruction(underConstruction)` (`:545-554`).
    pub fn set_under_construction(&mut self, under_construction: bool) {
        if self.r().is_under_construction() != under_construction {
            self.data().under_construction = under_construction;
            let t = self.r().unit_type();
            self.cx.note_structure_change(t);
            let owner = self.r().owner();
            self.cx.st.player_data_mut(owner).my_units_version += 1;
            self.shim_update(); // UnitImpl.ts:552
        }
    }
    /// `increaseLevel()` (`:744-763`).
    pub fn increase_level(&mut self) {
        let t = self.r().unit_type();
        if t == UnitType::SamLauncher {
            let current_tick = self.cx.st.ticks;
            let current_range = self.cx.st.cfg.dynamic_sam_range(&*self.cx.st, self.u, current_tick);
            let level = self.r().level();
            let duration = self.cx.st.cfg.sam_upgrade_duration();
            self.data().sam_launcher_state = Some(SamLauncherState {
                upgrade_start_tick: Some(current_tick),
                start_range: current_range,
                target_level: level + 1,
                duration,
            });
        }
        self.data().level += 1;
        self.cx.bump_units_version();
        let owner = self.r().owner();
        self.cx.st.player_data_mut(owner).my_units_version += 1;
        if has_missile_queue(t) {
            let tick = self.cx.st.ticks;
            self.data().missile_timer_queue.push(tick);
        }
        self.shim_update(); // UnitImpl.ts:762
    }
    /// `decreaseLevel(destroyer?)` (`:765`).
    pub fn decrease_level(&mut self, destroyer: Option<SmallId>) {
        self.data().level -= 1;
        let t = self.r().unit_type();
        if has_missile_queue(t) {
            self.data().missile_timer_queue.pop();
        }
        if t == UnitType::SamLauncher {
            self.data().sam_launcher_state = None;
        }
        if self.r().level() <= 0 {
            self.delete(Some(true), destroyer);
            return;
        }
        self.cx.bump_units_version();
        let owner = self.r().owner();
        self.cx.st.player_data_mut(owner).my_units_version += 1;
        self.shim_update(); // UnitImpl.ts:780
    }
}

/// `Gold` re-export used by `EngineUnitInfo.cost` callers.
pub type UnitCost = Gold;

// ---------------------------------------------------------------------------------------
// CS v1 (`Unit v1`, `UnitImpl.ts:798-863`), fields in dump order.
// ---------------------------------------------------------------------------------------

impl Canon for WarshipState {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        // `patrolTile`/`retreatPort`/`isInCombat` are `?undef`: absent when undefined.
        let opt_int = |x: Option<TileRef>| x.map_or(Cs::Absent, |t| Cs::Int(i64::from(t)));
        Cs::Obj(vec![
            ("state".to_owned(), Cs::Str(self.state.as_str().to_owned())),
            ("patrolTile".to_owned(), opt_int(self.patrol_tile)),
            ("retreatPort".to_owned(), opt_int(self.retreat_port)),
            ("isInCombat".to_owned(), self.is_in_combat.map_or(Cs::Absent, Cs::Bool)),
            ("lastCombatTick".to_owned(), self.last_combat_tick.canon(cx)),
            ("veterancy".to_owned(), self.veterancy.canon(cx)),
            ("veterancyProgress".to_owned(), self.veterancy_progress.canon(cx)),
        ])
    }
}

impl Canon for Unit {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        let tile = |t: TileRef| Cs::Int(i64::from(t));
        let mut o: Vec<(String, Cs)> = Vec::with_capacity(28);
        let mut put = |k: &str, v: Cs| o.push((k.to_owned(), v));
        put("id", Cs::Int(i64::from(self.id)));
        put("type", Cs::Str(self.unit_type.as_str().to_owned()));
        put("owner", self.owner.canon(cx));
        put("tile", tile(self.tile));
        put("lastTile", tile(self.last_tile));
        put("active", Cs::Bool(self.active));
        put("targetTile", self.target_tile.map_or(Cs::Null, tile));
        put("targetPlayer", self.target_player.canon(cx));
        put("targetUnit", self.target_unit.canon(cx));
        put("health", big(self.health));
        put(
            "transportShipState",
            match &self.transport_ship_state {
                Some(s) => Cs::Obj(vec![
                    ("isRetreating".to_owned(), Cs::Bool(s.is_retreating)),
                    ("troops".to_owned(), Cs::Num(s.troops)),
                ]),
                None => Cs::Null,
            },
        );
        put("warshipState", self.warship_state.canon(cx));
        put(
            "nukeState",
            match &self.nuke_state {
                Some(n) => Cs::Obj(vec![
                    ("trajectoryTiles".to_owned(), Cs::Typed(Typed::U32(n.trajectory.clone()))),
                    ("trajectoryIndex".to_owned(), n.trajectory_index.canon(cx)),
                    ("targetedBySam".to_owned(), Cs::Bool(n.targeted_by_sam)),
                    ("waitTicks".to_owned(), n.wait_ticks.canon(cx)),
                ]),
                None => Cs::Null,
            },
        );
        put("reachedTarget", Cs::Bool(self.reached_target));
        put("wasDestroyedByEnemy", Cs::Bool(self.was_destroyed_by_enemy));
        put("destroyer", self.destroyer.canon(cx));
        put("lastSetSafeFromPirates", self.last_set_safe_from_pirates.canon(cx));
        put("underConstruction", Cs::Bool(self.under_construction));
        put("lastOwner", self.last_owner.canon(cx));
        put("troops", Cs::Num(self.troops));
        put("missileTimerQueue", self.missile_timer_queue.canon(cx));
        put("hasTrainStation", Cs::Bool(self.has_train_station));
        put("level", self.level.canon(cx));
        put("targetable", Cs::Bool(self.targetable));
        put("loaded", self.loaded.map_or(Cs::Null, Cs::Bool));
        put("trainType", self.train_type.map_or(Cs::Null, |t| Cs::Str(t.as_str().to_owned())));
        put("deletionAt", self.deletion_at.canon(cx));
        put(
            "samLauncherState",
            match &self.sam_launcher_state {
                Some(s) => Cs::Obj(vec![
                    ("upgradeStartTick".to_owned(), s.upgrade_start_tick.map_or(Cs::Absent, |t| Cs::Int(i64::from(t)))),
                    ("startRange".to_owned(), Cs::Num(s.start_range)),
                    ("targetLevel".to_owned(), s.target_level.canon(cx)),
                    ("duration".to_owned(), s.duration.canon(cx)),
                ]),
                None => Cs::Null,
            },
        );
        Cs::Obj(o)
    }
}

#[cfg(test)]
mod tests;
