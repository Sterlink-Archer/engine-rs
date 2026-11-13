//! `WarshipExecution` (`engine/src/execution/WarshipExecution.ts`). Owner: **P7**.
//!
//! Fields: the `Warship v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1).
//!
//! **`warshipState()` is a mutating getter** (`UnitImpl.ts:437-442` writes `isInCombat`), so
//! every TS `x.warshipState()` call, on this warship *and* on the other warships the scans
//! look at, goes through [`UnitMut::warship_state`] at the same position ([`ws`]). The
//! `touch()` sites (`:86, :687, :707`) call the `unit_touched` shim (ARCHITECTURE §5).
//!
//! **`isActive()`** is `this.warship?.isActive()` (`:762-764`): it reads the unit, which other
//! executions can delete, but the frozen `ExecBehavior::is_active(&self)` has no state access
//! (`docs/requests/P7.md` R1). [`WarshipExec::is_active_in`] is the exact form; until the
//! dispatcher passes the state, `is_active` reports `warship.is_some()` and `tick` opens with
//! the unit check, which makes the tick pass of an inactive warship a no-op exactly as TS's
//! skip is (the remaining difference is that compaction keeps the execution listed).

use crate::canon::{Canon, CanonCx, Cs};
use crate::exec::prelude::*;
use crate::exec::shell::ShellExec;
use crate::exec::trade_ship::{prng_canon, water_path_finder_canon};
use crate::path::types::PathResult;
use crate::state::State;
use crate::state::unit::{UnitParams, WarshipMode, WarshipState, WarshipStateUpdate};
use crate::util::find_closest_by;

/// `input: (UnitParams<Warship> & OwnerComp) | Unit` (`:49-51`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WarshipInput {
    Unit(UnitRef),
    New { owner: SmallId, patrol_tile: TileRef },
}
crate::plain_copy!(WarshipInput);

/// `WarshipExecution` data.
#[derive(StateClone, Debug)]
pub struct WarshipExec {
    pub input: WarshipInput,
    /// `initialized: this.mg !== undefined` (set by `init`).
    pub initialized: bool,
    /// `PseudoRandom(mg.ticks())` at init (`:56`)
    pub rng: Option<Prng>,
    pub warship: Option<UnitRef>,
    pub pathfinder: Option<WaterPathFinder>,
    pub last_shell_attack: i32,
    pub already_sent_shell: JsSet<UnitRef>,
    pub last_manual_move_tick_retreat_disabled: i32,
    pub last_observed_patrol_tile: Option<TileRef>,
    pub active_healing_remainder: f64,
    pub last_emitted_combat: bool,
}

/// `manualMoveRetreatDisabledDuration = 50` (`:204`).
const MANUAL_MOVE_RETREAT_DISABLED_DURATION: i32 = 50;
/// `maxAttemptBeforeExpand = 500` (`:776`).
const MAX_ATTEMPT_BEFORE_EXPAND: i32 = 500;

/// `u.warshipState()`: the mutating getter (writes `isInCombat`), returned by value.
#[inline]
fn ws(cx: &mut Ctx, u: UnitRef) -> WarshipState {
    cx.unit_mut(u).warship_state().clone()
}

/// JS truthiness of a `TileRef | undefined` (`if (tile)`): tile 0 is falsy.
#[inline]
fn tile_truthy(t: Option<TileRef>) -> bool {
    matches!(t, Some(x) if x != 0)
}

impl WarshipExec {
    /// `new WarshipExecution(input)`.
    pub fn new(input: WarshipInput) -> Self {
        // TS: :42-47 field initialisers
        WarshipExec {
            input,
            initialized: false,
            rng: None,
            warship: None,
            pathfinder: None,
            last_shell_attack: 0,
            already_sent_shell: JsSet::new(),
            last_manual_move_tick_retreat_disabled: 0,
            last_observed_patrol_tile: None,
            active_healing_remainder: 0.0,
            last_emitted_combat: false,
        }
    }

    /// `isActive()` with state access (`:762-764`: `this.warship?.isActive()`; `undefined` is
    /// falsy). See the module docs and `docs/requests/P7.md` R1.
    pub fn is_active_in(&self, st: &State) -> bool {
        match self.warship {
            Some(w) => st.unit(w).is_active(),
            None => false,
        }
    }

    /// `isDocked()` (`:766-768`).
    pub fn is_docked(&self, cx: &mut Ctx) -> bool {
        match self.warship {
            Some(w) => ws(cx, w).state == WarshipMode::Docked,
            None => false, // `?? "patrolling"`
        }
    }

    fn w(&self) -> UnitRef {
        self.warship.ts_expect("WarshipExecution.ts this.warship")
    }

    fn pf(&mut self) -> &mut WaterPathFinder {
        self.pathfinder.as_mut().ts_expect("WarshipExecution.ts this.pathfinder")
    }

    /// `healWarship()` (`:153-184`).
    fn heal_warship(&mut self, cx: &mut Ctx) {
        let w = self.w();
        // TS: :154-158 a doomed side cannot repair its navy
        let owner = cx.st.unit(w).owner();
        if cx.st.player(owner).in_doomsday_clock() {
            return;
        }
        // TS: :159-163
        let passive_healing = cx.st.config().warship_passive_healing();
        let passive_healing_range = cx.st.config().warship_passive_healing_range();
        let passive_healing_range_squared = passive_healing_range * passive_healing_range;
        let warship_tile = cx.st.unit(w).tile();

        // TS: :165-175
        let mut is_near_port = false;
        for port in cx.st.player_units_of_type_memo(owner, UnitType::Port) {
            let dist_squared = f64::from(cx.st.map.euclidean_dist_squared(warship_tile, cx.st.unit(port).tile()));
            if dist_squared <= passive_healing_range_squared {
                is_near_port = true;
                break;
            }
        }

        // TS: :177-179
        if is_near_port {
            cx.unit_mut(w).modify_health(passive_healing, None);
        }

        // TS: :181-183
        if ws(cx, w).state == WarshipMode::Docked {
            self.apply_active_docked_healing(cx);
        }
    }

    /// `isFullyHealed()` (`:186-191`).
    fn is_fully_healed(&self, cx: &mut Ctx) -> bool {
        let v = cx.st.unit(self.w());
        if !v.has_health() {
            return true;
        }
        v.health() >= v.max_health()
    }

    /// `shouldStartRepairRetreat(healthBeforeHealing)` (`:193-223`).
    fn should_start_repair_retreat(&mut self, cx: &mut Ctx, health_before_healing: f64) -> bool {
        let w = self.w();
        // TS: :196-198
        if ws(cx, w).state != WarshipMode::Patrolling {
            return false;
        }
        // TS: :201-203
        let owner = cx.st.unit(w).owner();
        if cx.st.player(owner).in_doomsday_clock() {
            return false;
        }
        // TS: :204-210
        if cx.st.ticks() - self.last_manual_move_tick_retreat_disabled < MANUAL_MOVE_RETREAT_DISABLED_DURATION {
            return false;
        }
        // TS: :213-217 `Math.floor((maxHealth * warshipRetreatHealthPercent) / 100)`
        let retreat_threshold =
            of_det::js::floor((cx.st.unit(w).max_health() * cx.st.config().warship_retreat_health_percent()) / 100.0);
        // TS: :218-220
        if health_before_healing >= retreat_threshold {
            return false;
        }
        // TS: :221-222
        let owner = cx.st.unit(w).owner();
        !cx.st.player_units_of_type_memo(owner, UnitType::Port).is_empty()
    }

    /// `findNearestPort()` (`:225-247`).
    fn find_nearest_port(&mut self, cx: &mut Ctx) -> Option<TileRef> {
        let w = self.w();
        // TS: :226-229
        let owner = cx.st.unit(w).owner();
        let ports = cx.st.player_units_of_type_memo(owner, UnitType::Port);
        if ports.is_empty() {
            return None;
        }
        // TS: :231-235
        let warship_tile = cx.st.unit(w).tile();
        let warship_component = cx.rcx().get_water_component(warship_tile)?;
        // TS: :237-244 `findMinimumBy(ports, euclid(warshipTile, port), comp(port) === warshipComponent)`
        let nearest = {
            let st: &State = &*cx.st;
            let mut rcx = crate::ctx::Rcx::new(st, &mut *cx.sc);
            find_closest_by(
                &ports,
                |&port| f64::from(st.map.euclidean_dist_squared(warship_tile, st.unit(port).tile())),
                Some(|&port: &UnitRef| {
                    let port_component = rcx.get_water_component(st.unit(port).tile());
                    port_component == Some(warship_component)
                }),
            )
        };
        // TS: :246 `nearest?.tile()`; in-range: an index into `ports`.
        nearest.map(|i| cx.st.unit(ports[i]).tile())
    }

    /// `findRetreatAggroTarget()` (`:249-251`).
    fn find_retreat_aggro_target(&mut self, cx: &mut Ctx) -> Option<UnitRef> {
        self.find_best_target(cx, &[UnitType::TransportShip, UnitType::Warship], false)
    }

    /// `findTargetUnit()` (`:253-258`).
    fn find_target_unit(&mut self, cx: &mut Ctx) -> Option<UnitRef> {
        self.find_best_target(cx, &[UnitType::TransportShip, UnitType::Warship, UnitType::TradeShip], true)
    }

    /// `findBestTarget(types, includeTradeShips = false)` (`:268-354`).
    fn find_best_target(&mut self, cx: &mut Ctx, types: &[UnitType], include_trade_ships: bool) -> Option<UnitRef> {
        let w = self.w();
        // TS: :272-274
        let owner = cx.st.unit(w).owner();
        // TS: :276-280 `mg.nearbyUnits(warship.tile(), warshipTargettingRange(), types)`
        // (no predicate; `includeUnderConstruction` undefined)
        let ships = {
            let wtile = cx.st.unit(w).tile();
            let range = cx.st.config().warship_targetting_range();
            cx.st.nearby_units(wtile, range, types, |_, _, _| true, false)
        };

        // TS: :282-286 trade-ship state, lazily computed (`warshipComponent === undefined`
        // is the "not yet computed" sentinel)
        let mut has_reachable_port = false;
        let mut patrol_tile: Option<TileRef> = None;
        let mut patrol_range_squared = 0.0;
        let mut warship_component: Option<Option<u32>> = None;

        // TS: :288-290
        let mut best_unit: Option<UnitRef> = None;
        let mut best_type_priority = 0;
        let mut best_dist_squared = 0.0;

        // TS: :292-351
        for nu in ships {
            let (unit, dist_squared) = (nu.unit, nu.dist_squared);
            // TS: :293-302 (short-circuit order kept: `unit.warshipState()` runs last)
            let skip = unit == w
                || cx.st.unit(unit).owner() == owner
                || !cx.st.player(owner).can_attack_player(cx.st.unit(unit).owner(), true)
                || self.already_sent_shell.contains(&unit)
                || (cx.st.unit(unit).unit_type() == UnitType::Warship && ws(cx, unit).state == WarshipMode::Docked);
            if skip {
                continue;
            }

            // TS: :304
            let ty = cx.st.unit(unit).unit_type();

            // TS: :306-337
            if include_trade_ships && ty == UnitType::TradeShip {
                if warship_component.is_none() {
                    // TS: :308-321
                    let wtile = cx.st.unit(w).tile();
                    let comp = cx.rcx().get_water_component(wtile);
                    warship_component = Some(comp);
                    has_reachable_port = match comp {
                        None => false,
                        Some(c) => {
                            let ports = cx.st.player_units_of_type_memo(owner, UnitType::Port);
                            let mut any = false;
                            for port in ports {
                                let v = cx.st.unit(port);
                                let ok =
                                    v.is_active() && !v.is_marked_for_deletion() && !v.is_under_construction() && {
                                        let pt = v.tile();
                                        cx.rcx().has_water_component(pt, c)
                                    };
                                if ok {
                                    any = true;
                                    break;
                                }
                            }
                            any
                        }
                    };
                    patrol_tile = ws(cx, w).patrol_tile;
                    // `config.warshipPatrolRange() ** 2` → `x * x` (PORTING_GUIDE §2)
                    let r = cx.st.config().warship_patrol_range();
                    patrol_range_squared = r * r;
                }
                // TS: :323-331 (`?.` chains: an undefined targetUnit is falsy / never equal)
                let target_owner = cx.st.unit(unit).target_unit().map(|t| cx.st.unit(t).owner());
                let skip_trade = !has_reachable_port
                    || patrol_tile.is_none()
                    || cx.st.unit(unit).is_safe_from_pirates()
                    || target_owner == Some(owner)
                    || target_owner.is_some_and(|o| cx.st.player(o).is_friendly(owner, false));
                if skip_trade {
                    continue;
                }
                // TS: :332-336
                let pt = patrol_tile.ts_expect("WarshipExecution.ts:333 patrolTile");
                if f64::from(cx.st.map.euclidean_dist_squared(pt, cx.st.unit(unit).tile())) > patrol_range_squared {
                    continue;
                }
            }

            // TS: :339-340
            let type_priority = match ty {
                UnitType::TransportShip => 0,
                UnitType::Warship => 1,
                _ => 2,
            };

            // TS: :342-350
            if best_unit.is_none()
                || type_priority < best_type_priority
                || (type_priority == best_type_priority && dist_squared < best_dist_squared)
            {
                best_unit = Some(unit);
                best_type_priority = type_priority;
                best_dist_squared = dist_squared;
            }
        }

        // TS: :353
        best_unit
    }

    /// `startRepairRetreat()` (`:356-367`).
    fn start_repair_retreat(&mut self, cx: &mut Ctx) {
        // TS: :357-360
        let Some(port_tile) = self.find_nearest_port(cx) else { return };
        let w = self.w();
        // TS: :361-364
        cx.unit_mut(w).update_warship_state(WarshipStateUpdate {
            retreat_port: Some(Some(port_tile)),
            state: Some(WarshipMode::Retreating),
            ..WarshipStateUpdate::default()
        });
        // TS: :365-366
        self.active_healing_remainder = 0.0;
        cx.unit_mut(w).set_target_unit(None);
    }

    /// `cancelRepairRetreat(clearTargetTile = true)` (`:369-378`).
    fn cancel_repair_retreat(&mut self, cx: &mut Ctx, clear_target_tile: bool) {
        let w = self.w();
        // TS: :370-374
        self.active_healing_remainder = 0.0;
        cx.unit_mut(w).update_warship_state(WarshipStateUpdate {
            state: Some(WarshipMode::Patrolling),
            retreat_port: Some(None),
            ..WarshipStateUpdate::default()
        });
        // TS: :375-377
        if clear_target_tile {
            cx.unit_mut(w).set_target_tile(None);
        }
    }

    /// `handleManualPatrolOverride()` (`:380-392`).
    fn handle_manual_patrol_override(&mut self, cx: &mut Ctx) {
        let w = self.w();
        // TS: :381
        let patrol_tile = ws(cx, w).patrol_tile;
        // TS: :382-390
        if self.last_observed_patrol_tile.is_some() && patrol_tile != self.last_observed_patrol_tile {
            self.last_manual_move_tick_retreat_disabled = cx.st.ticks();
            if ws(cx, w).state != WarshipMode::Patrolling {
                self.cancel_repair_retreat(cx, false);
            }
        }
        // TS: :391
        self.last_observed_patrol_tile = patrol_tile;
    }

    /// `handleRepairRetreat()` (`:394-476`).
    fn handle_repair_retreat(&mut self, cx: &mut Ctx) -> bool {
        let w = self.w();
        // TS: :395-397
        if ws(cx, w).state == WarshipMode::Patrolling {
            return false;
        }

        // TS: :399-404 fire back while retreating
        let retreat_aggro_target = self.find_retreat_aggro_target(cx);
        if let Some(t) = retreat_aggro_target {
            cx.unit_mut(w).set_target_unit(Some(t));
            self.shoot_target(cx);
        }

        // TS: :406-409
        if !self.refresh_retreat_port_tile(cx) {
            self.cancel_repair_retreat(cx, true);
            return false;
        }

        // TS: :411-414
        if retreat_aggro_target.is_none() {
            cx.unit_mut(w).set_target_unit(None);
        }

        // TS: :416-419
        let Some(retreat_port_tile) = ws(cx, w).retreat_port else {
            return false;
        };

        // TS: :421-426
        let docking_radius = cx.st.config().warship_docking_range();
        let docking_radius_sq = docking_radius * docking_radius;
        let dist_to_port = f64::from(cx.st.map.euclidean_dist_squared(cx.st.unit(w).tile(), retreat_port_tile));

        // TS: :428-449
        if dist_to_port <= docking_radius_sq {
            // TS: :430-433 `owner.units(Port).find(p => p.tile() === retreatPortTile)`
            let port = self.port_at(cx, retreat_port_tile);
            let has_capacity = match port {
                Some(p) => !self.is_port_full_of_healing(cx, p, Some(w)),
                None => false,
            };
            if has_capacity {
                // TS: :435-440 dock here
                cx.unit_mut(w).set_target_tile(None);
                cx.unit_mut(w).update_warship_state(WarshipStateUpdate {
                    state: Some(WarshipMode::Docked),
                    ..Default::default()
                });
                return true;
            }
            // TS: :441-448 port full: wait near it, but leave if already fully healed
            if self.is_fully_healed(cx) {
                self.cancel_repair_retreat(cx, true);
                return false;
            }
            return true;
        }

        // TS: :451-452
        cx.unit_mut(w).set_target_tile(Some(retreat_port_tile));
        let wtile = cx.st.unit(w).tile();
        let result = self.pf().next(cx, wtile, retreat_port_tile, None);
        // TS: :453-473
        match result {
            PathResult::Complete(node) => {
                cx.unit_mut(w).move_to(node);
                if node == retreat_port_tile {
                    cx.unit_mut(w).set_target_tile(None);
                }
            }
            PathResult::Next(node) => {
                cx.unit_mut(w).move_to(node);
            }
            PathResult::NotFound => {
                // TS: :464-470
                let new_port = self.find_nearest_available_port_tile(cx);
                cx.unit_mut(w)
                    .update_warship_state(WarshipStateUpdate { retreat_port: Some(new_port), ..Default::default() });
                if new_port.is_none() {
                    self.cancel_repair_retreat(cx, true);
                }
            }
        }

        // TS: :475
        true
    }

    /// `owner.units(Port).find(p => p.tile() === tile)` (`:430-433`, `:500`, `:580-583`).
    fn port_at(&self, cx: &mut Ctx, tile: TileRef) -> Option<UnitRef> {
        let owner = cx.st.unit(self.w()).owner();
        cx.st.player_units_of_type_memo(owner, UnitType::Port).into_iter().find(|&p| cx.st.unit(p).tile() == tile)
    }

    /// `refreshRetreatPortTile()` (`:478-522`).
    fn refresh_retreat_port_tile(&mut self, cx: &mut Ctx) -> bool {
        let w = self.w();
        // TS: :479-482
        let owner = cx.st.unit(w).owner();
        let ports = cx.st.player_units_of_type_memo(owner, UnitType::Port);
        if ports.is_empty() {
            return false;
        }

        // TS: :484
        let current_retreat_port = ws(cx, w).retreat_port;

        // TS: :486-489
        let current_port_exists =
            current_retreat_port.is_some_and(|t| ports.iter().any(|&p| cx.st.unit(p).tile() == t));

        // TS: :491-497
        if !current_port_exists {
            let new_port = self.find_nearest_available_port_tile(cx);
            cx.unit_mut(w)
                .update_warship_state(WarshipStateUpdate { retreat_port: Some(new_port), ..Default::default() });
            return new_port.is_some();
        }

        // TS: :499-510 current port at healing capacity: look for alternatives
        let current_port = ports.iter().copied().find(|&p| Some(cx.st.unit(p).tile()) == current_retreat_port);
        if let Some(cp) = current_port
            && self.is_port_full_of_healing(cx, cp, None)
        {
            let alternative_port = self.find_nearest_available_port(cx);
            // TS: :504 `if (alternativePort)`: a TileRef, so tile 0 is falsy
            if tile_truthy(alternative_port) {
                cx.unit_mut(w).update_warship_state(WarshipStateUpdate {
                    retreat_port: Some(alternative_port),
                    ..Default::default()
                });
            }
            return ws(cx, w).retreat_port.is_some();
        }

        // TS: :512-519 a significantly closer port
        let closer_port = self.find_better_port_tile(cx);
        // TS: :514 `if (closerPort && closerPort !== currentRetreatPort)` (tile 0 is falsy)
        if tile_truthy(closer_port) && closer_port != current_retreat_port {
            cx.unit_mut(w)
                .update_warship_state(WarshipStateUpdate { retreat_port: Some(closer_port), ..Default::default() });
            return true;
        }

        // TS: :521
        true
    }

    /// `isPortFullOfHealing(port, excludeShip?)` (`:524-527`).
    fn is_port_full_of_healing(&mut self, cx: &mut Ctx, port: UnitRef, exclude_ship: Option<UnitRef>) -> bool {
        let max_ships_healing = cx.st.unit(port).level();
        // usize → i64: the docked list is far below 2^31 entries.
        i64::try_from(self.docked_ships_at_port(cx, port, exclude_ship).len()).unwrap_or(i64::MAX)
            >= i64::from(max_ships_healing)
    }

    /// `dockedShipsAtPort(port, excludeShip?)` (`:529-543`).
    fn docked_ships_at_port(&mut self, cx: &mut Ctx, port: UnitRef, exclude_ship: Option<UnitRef>) -> Vec<UnitRef> {
        // TS: :530-531
        let docking_radius = cx.st.config().warship_docking_range();
        let owner = cx.st.unit(self.w()).owner();
        // TS: :533-534 `mg.nearbyUnits(port.tile(), dockingRadius, [Warship])`
        let near = {
            let ptile = cx.st.unit(port).tile();
            cx.st.nearby_units(ptile, docking_radius, &[UnitType::Warship], |_, _, _| true, false)
        };
        // TS: :535-542 filter (in order; `ship.warshipState()` is the mutating getter)
        let mut out = Vec::new();
        for nu in near {
            let ship = nu.unit;
            if exclude_ship.is_some_and(|e| e == ship) {
                continue;
            }
            if cx.st.unit(ship).owner() != owner {
                continue;
            }
            if ws(cx, ship).state == WarshipMode::Patrolling {
                continue;
            }
            if cx.st.unit(ship).target_tile().is_some() {
                continue;
            }
            out.push(ship);
        }
        out
    }

    /// `applyActiveDockedHealing()` (`:545-572`).
    fn apply_active_docked_healing(&mut self, cx: &mut Ctx) {
        let w = self.w();
        // TS: :546-549
        let Some(docked_port) = self.current_retreat_port(cx) else { return };

        // TS: :551-554
        let docked_ships = self.docked_ships_at_port(cx, docked_port, None);
        if !docked_ships.contains(&w) {
            return;
        }

        // TS: :556-560
        let healing_pool =
            f64::from(cx.st.unit(docked_port).level()) * cx.st.config().warship_port_healing_bonus_per_level();
        if healing_pool <= 0.0 || docked_ships.is_empty() {
            return;
        }

        // TS: :562-568 fractional split healing with a per-ship remainder
        #[allow(clippy::cast_precision_loss)] // int: a list length (< 2^53)
        let active_healing = healing_pool / (docked_ships.len() as f64);
        self.active_healing_remainder += active_healing;
        let integer_healing = of_det::js::floor(self.active_healing_remainder);
        if integer_healing <= 0.0 {
            return;
        }

        // TS: :570-571
        self.active_healing_remainder -= integer_healing;
        cx.unit_mut(w).modify_health(integer_healing, None);
    }

    /// `currentRetreatPort()` (`:574-584`).
    fn current_retreat_port(&mut self, cx: &mut Ctx) -> Option<UnitRef> {
        let retreat_port = ws(cx, self.w()).retreat_port?;
        self.port_at(cx, retreat_port)
    }

    /// `nearestAvailablePortTile(excludeShip?)` (`:586-619`).
    fn nearest_available_port_tile(&mut self, cx: &mut Ctx, exclude_ship: Option<UnitRef>) -> Option<(TileRef, f64)> {
        let w = self.w();
        // TS: :589-594
        let owner = cx.st.unit(w).owner();
        let ports = cx.st.player_units_of_type_memo(owner, UnitType::Port);
        let warship_tile = cx.st.unit(w).tile();
        let warship_component = cx.rcx().get_water_component(warship_tile)?;

        // TS: :596-614
        let mut best_tile: Option<TileRef> = None;
        let mut best_distance = f64::INFINITY;
        for port in ports {
            if self.is_port_full_of_healing(cx, port, exclude_ship) {
                continue;
            }
            let port_tile = cx.st.unit(port).tile();
            if !cx.rcx().has_water_component(port_tile, warship_component) {
                continue;
            }
            let distance = f64::from(cx.st.map.euclidean_dist_squared(warship_tile, port_tile));
            if distance < best_distance {
                best_distance = distance;
                best_tile = Some(port_tile);
            }
        }

        // TS: :616-618
        best_tile.map(|t| (t, best_distance))
    }

    /// `findNearestAvailablePort()` (`:621-623`).
    fn find_nearest_available_port(&mut self, cx: &mut Ctx) -> Option<TileRef> {
        self.nearest_available_port_tile(cx, None).map(|(t, _)| t)
    }

    /// `findBetterPortTile()` (`:625-645`).
    fn find_better_port_tile(&mut self, cx: &mut Ctx) -> Option<TileRef> {
        // TS: :626-627
        let (tile, dist_squared) = self.nearest_available_port_tile(cx, None)?;
        // TS: :629-636
        let w = self.w();
        let mut current_distance = f64::INFINITY;
        if let Some(current) = ws(cx, w).retreat_port {
            current_distance = f64::from(cx.st.map.euclidean_dist_squared(cx.st.unit(w).tile(), current));
        }
        // TS: :638-644
        if dist_squared < current_distance * cx.st.config().warship_port_switch_threshold() {
            return Some(tile);
        }
        None
    }

    /// `findNearestAvailablePortTile()` (`:647-649`).
    fn find_nearest_available_port_tile(&mut self, cx: &mut Ctx) -> Option<TileRef> {
        let w = self.w();
        self.nearest_available_port_tile(cx, Some(w)).map(|(t, _)| t)
    }

    /// `shootTarget()` (`:651-674`).
    fn shoot_target(&mut self, cx: &mut Ctx) {
        let w = self.w();
        // TS: :652
        cx.unit_mut(w)
            .update_warship_state(WarshipStateUpdate { is_in_combat: Some(Some(true)), ..Default::default() });
        // TS: :653-654
        let shell_attack_rate = cx.st.config().warship_shell_attack_rate();
        if f64::from(cx.st.ticks() - self.last_shell_attack) > shell_attack_rate {
            // TS: :655-658 warships don't reload when attacking transport ships
            let target_type = cx.st.unit(w).target_unit().map(|t| cx.st.unit(t).unit_type());
            if target_type != Some(UnitType::TransportShip) {
                self.last_shell_attack = cx.st.ticks();
            }
            // TS: :659-666 `new ShellExecution(warship.tile(), warship.owner(), warship, targetUnit()!)`
            let (wtile, wowner) = (cx.st.unit(w).tile(), cx.st.unit(w).owner());
            let target = cx.st.unit(w).target_unit().ts_expect("WarshipExecution.ts:664 targetUnit()!");
            cx.st.add_execution(Exec::Shell(ShellExec::new(wtile, wowner, w, target)));
            // TS: :667-672 don't send multiple shells to a target that can be one-shotted
            let target = cx.st.unit(w).target_unit().ts_expect("WarshipExecution.ts:667 targetUnit()!");
            if !cx.st.unit(target).has_health() {
                let target = cx.st.unit(w).target_unit().ts_expect("WarshipExecution.ts:669 targetUnit()!");
                self.already_sent_shell.insert(target);
                cx.unit_mut(w).set_target_unit(None);
            }
        }
    }

    /// `huntDownTradeShip()` (`:676-717`).
    fn hunt_down_trade_ship(&mut self, cx: &mut Ctx) {
        let w = self.w();
        // TS: :677
        cx.unit_mut(w)
            .update_warship_state(WarshipStateUpdate { is_in_combat: Some(Some(true)), ..Default::default() });
        // TS: :678-716
        for _ in 0..2 {
            // TS: :679-681
            let target = cx.st.unit(w).target_unit().ts_expect("WarshipExecution.ts:679 targetUnit()!");
            let target_tile = cx.st.unit(target).tile();
            let dist = cx.st.map.manhattan_dist(cx.st.unit(w).tile(), target_tile);

            // TS: :683-689
            if dist <= 5 {
                self.capture_trade_ship(cx, w, target);
                return;
            }

            // TS: :691-699 close: direct greedy movement (the 2x minimap paths converge badly)
            if dist <= 20
                && let Some(next_tile) = self.best_neighbor_toward(cx, target_tile)
            {
                cx.unit_mut(w).move_to(next_tile);
                continue;
            }

            // TS: :701 `pathfinder.next(warship.tile(), targetTile, 5)`
            let wtile = cx.st.unit(w).tile();
            let result = self.pf().next(cx, wtile, target_tile, Some(5.0));
            match result {
                // TS: :703-708
                PathResult::Complete(_) => {
                    self.capture_trade_ship(cx, w, target);
                    return;
                }
                // TS: :709-711
                PathResult::Next(node) => {
                    cx.unit_mut(w).move_to(node);
                }
                // TS: :712-714 (log dropped)
                PathResult::NotFound => {}
            }
        }
    }

    /// The capture block shared by `:684-688` and `:704-708`.
    fn capture_trade_ship(&mut self, cx: &mut Ctx, w: UnitRef, target: UnitRef) {
        let owner = cx.st.unit(w).owner();
        cx.player_mut(owner).capture_unit(target);
        cx.unit_mut(w).record_trade_capture();
        cx.unit_mut(w).set_target_unit(None);
        // `this.warship.touch()` (`:687`, `:707`): the isInCombat shim, never dropped (§5).
        // touch-site: WarshipExecution.ts:687 WarshipExecution.ts:707
        cx.st.unit_touched(w);
    }

    /// `bestNeighborToward(targetTile)` (`:719-732`).
    fn best_neighbor_toward(&self, cx: &mut Ctx, target_tile: TileRef) -> Option<TileRef> {
        let warship_tile = cx.st.unit(self.w()).tile();
        let map = &cx.st.map;
        let mut best: Option<TileRef> = None;
        let mut best_dist = map.manhattan_dist(warship_tile, target_tile);
        // TS: :723-730 `forEachNeighbor` (N, S, W, E); the callback only reads the map.
        map.for_each_neighbor(warship_tile, |neighbor| {
            if !map.is_water(neighbor) {
                return;
            }
            let d = map.manhattan_dist(neighbor, target_tile);
            if d < best_dist {
                best_dist = d;
                best = Some(neighbor);
            }
        });
        best
    }

    /// `patrol()` (`:734-760`).
    fn patrol(&mut self, cx: &mut Ctx) {
        let w = self.w();
        // TS: :735-740
        if cx.st.unit(w).target_tile().is_none() {
            let t = self.random_tile(cx, false);
            cx.unit_mut(w).set_target_tile(t);
            if cx.st.unit(w).target_tile().is_none() {
                return;
            }
        }

        // TS: :742-745
        let wtile = cx.st.unit(w).tile();
        let tt = cx.st.unit(w).target_tile().ts_expect("WarshipExecution.ts:744 targetTile()!");
        let result = self.pf().next(cx, wtile, tt, None);
        // TS: :746-759
        match result {
            PathResult::Complete(node) => {
                cx.unit_mut(w).set_target_tile(None);
                cx.unit_mut(w).move_to(node);
            }
            PathResult::Next(node) => {
                cx.unit_mut(w).move_to(node);
            }
            // log dropped
            PathResult::NotFound => {
                cx.unit_mut(w).set_target_tile(None);
            }
        }
    }

    /// `randomTile(allowShoreline = false)` (`:774-841`).
    pub fn random_tile(&mut self, cx: &mut Ctx, allow_shoreline: bool) -> Option<TileRef> {
        let w = self.w();
        // TS: :775-778
        let mut warship_patrol_range = cx.st.config().warship_patrol_range();
        let mut attempts: i32 = 0;
        let mut expand_count: i32 = 0;

        // TS: :781 the warship's water component, for the connectivity check
        let wtile = cx.st.unit(w).tile();
        let warship_component = cx.rcx().get_water_component(wtile);

        // TS: :783-791 bail out on an invalid patrolTile (TS would spin forever otherwise)
        let patrol_tile = ws(cx, w).patrol_tile?;
        if !cx.st.map.is_valid_ref(f64::from(patrol_tile)) {
            return None;
        }

        // TS: :793-832
        while expand_count < 3 {
            // TS: :794-799 `mg.x(patrolTile) + random.nextInt(-range / 2, range / 2)` (x first,
            // then y; `nextInt` floors both bounds)
            let rng = self.rng.as_mut().ts_expect("WarshipExecution.ts:796 this.random");
            let x = f64::from(cx.st.map.x(patrol_tile))
                + rng.next_int(-warship_patrol_range / 2.0, warship_patrol_range / 2.0);
            let y = f64::from(cx.st.map.y(patrol_tile))
                + rng.next_int(-warship_patrol_range / 2.0, warship_patrol_range / 2.0);
            // TS: :800-802
            if !cx.st.map.is_valid_coord(x, y) {
                continue;
            }
            // TS: :803
            let tile = cx.st.map.ref_(x, y);
            // TS: :804-816
            if !cx.st.map.is_water(tile) || (!allow_shoreline && cx.st.map.is_shoreline(tile)) {
                attempts += 1;
                if attempts == MAX_ATTEMPT_BEFORE_EXPAND {
                    expand_count += 1;
                    attempts = 0;
                    warship_patrol_range += of_det::js::floor(warship_patrol_range / 2.0);
                }
                continue;
            }
            // TS: :817-830 water component connectivity
            if let Some(c) = warship_component
                && !cx.rcx().has_water_component(tile, c)
            {
                attempts += 1;
                if attempts == MAX_ATTEMPT_BEFORE_EXPAND {
                    expand_count += 1;
                    attempts = 0;
                    warship_patrol_range += of_det::js::floor(warship_patrol_range / 2.0);
                }
                continue;
            }
            // TS: :831
            return Some(tile);
        }
        // TS: :833-840 (warn dropped): retry allowing the shoreline
        if !allow_shoreline {
            return self.random_tile(cx, true);
        }
        None
    }
}

impl ExecBehavior for WarshipExec {
    /// Shim for `this.warship?.isActive()` (module docs; `docs/requests/P7.md` R1).
    fn is_active(&self) -> bool {
        self.warship.is_some()
    }
    /// `this.warship?.isActive()` (`WarshipExecution.ts:762-764`): the exact predicate the
    /// driver's tick gate and compaction call (request P7 R1, merge1).
    fn is_active_in(&self, st: &State) -> bool {
        WarshipExec::is_active_in(self, st)
    }
    /// `activeDuringSpawnPhase()` returns `false` (`WarshipExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    fn init(&mut self, cx: &mut Ctx, _ticks: i32) {
        // TS: :54-56 (in TS order: mg, pathfinder, random)
        self.initialized = true;
        self.pathfinder = Some(WaterPathFinder::new(cx, 0, false));
        self.rng = Some(Prng::new(f64::from(cx.st.ticks())));
        // TS: :57-75
        let warship = match self.input {
            WarshipInput::Unit(u) => u,
            WarshipInput::New { owner, patrol_tile } => {
                // TS: :60-63 `owner.canBuild(Warship, patrolTile)`
                let spawn = cx.st.player(owner).can_build(cx.sc, UnitType::Warship, patrol_tile, None);
                // TS: :64-69 (warn dropped): no warship, isActive() stays falsy
                let Some(spawn) = spawn else { return };
                // TS: :70-74 `buildUnit(Warship, spawn, this.input)`: of the input keys only
                // `patrolTile` is a Warship unit param (`owner` is ignored by UnitImpl).
                let params = UnitParams { patrol_tile: Some(patrol_tile), ..UnitParams::default() };
                cx.player_mut(owner).build_unit(UnitType::Warship, spawn, params)
            }
        };
        self.warship = Some(warship);
        // TS: :76
        self.last_observed_patrol_tile = ws(cx, warship).patrol_tile;
    }
    fn tick(&mut self, cx: &mut Ctx, _ticks: i32) {
        // Shim half of `isActive()` (module docs): TS never ticks an inactive warship.
        if !self.is_active_in(cx.st) {
            return;
        }
        let w = self.w();
        // TS: :80-83 `this.warship.delete()` (displayMessage, destroyer undefined)
        if cx.st.unit(w).health() <= 0.0 {
            cx.unit_mut(w).delete(None, None);
            return;
        }
        // TS: :84-88 `warshipState().isInCombat ?? false`; touch on combat → idle
        let is_in_combat = ws(cx, w).is_in_combat.unwrap_or(false);
        if self.last_emitted_combat && !is_in_combat {
            // `this.warship.touch()` (:86), never dropped (§5).
            // touch-site: WarshipExecution.ts:86
            cx.st.unit_touched(w);
        }
        self.last_emitted_combat = is_in_combat;
        // TS: :89
        let health_before_healing = cx.st.unit(w).health();

        // TS: :91-92
        self.heal_warship(cx);
        self.handle_manual_patrol_override(cx);

        // TS: :97-102 a doomed side cannot repair: undock / abort any retreat
        let owner = cx.st.unit(w).owner();
        if cx.st.player(owner).in_doomsday_clock() && ws(cx, w).state != WarshipMode::Patrolling {
            self.cancel_repair_retreat(cx, true);
        }

        // TS: :104-114
        if ws(cx, w).state == WarshipMode::Docked {
            if self.current_retreat_port(cx).is_none() {
                self.cancel_repair_retreat(cx, true);
            }
            if self.is_fully_healed(cx) {
                self.cancel_repair_retreat(cx, true);
            }
            if ws(cx, w).state == WarshipMode::Docked {
                return;
            }
        }

        // TS: :116-118
        if self.handle_repair_retreat(cx) {
            return;
        }

        // TS: :120-126 priority 1: heal before anything else
        if self.should_start_repair_retreat(cx, health_before_healing) {
            self.start_repair_retreat(cx);
            if self.handle_repair_retreat(cx) {
                return;
            }
        }

        // TS: :128
        let target = self.find_target_unit(cx);
        cx.unit_mut(w).set_target_unit(target);

        // TS: :130-148 (`targetUnit()?.type()` re-read before each check)
        let tt = |cx: &Ctx| cx.st.unit(w).target_unit().map(|t| cx.st.unit(t).unit_type());
        if tt(cx) == Some(UnitType::TransportShip) {
            self.shoot_target(cx);
            self.patrol(cx);
            return;
        }
        if tt(cx) == Some(UnitType::Warship) {
            self.shoot_target(cx);
            self.patrol(cx);
            return;
        }
        if tt(cx) == Some(UnitType::TradeShip) {
            self.hunt_down_trade_ship(cx);
            return;
        }

        // TS: :150
        self.patrol(cx);
    }
}

/// `Warship v1`: `{ input: {unit}|{owner, patrolTile}, initialized, random|null,
/// warship|null, pathfinder|null, lastShellAttack, alreadySentShell[], lastManualMove…,
/// lastObservedPatrolTile?, activeHealingRemainder: num, lastEmittedCombat }`.
impl Canon for WarshipExec {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        let input = match self.input {
            WarshipInput::Unit(u) => Cs::Obj(vec![("unit".into(), u.canon(cx))]),
            WarshipInput::New { owner, patrol_tile } => {
                Cs::Obj(vec![("owner".into(), owner.canon(cx)), ("patrolTile".into(), Cs::Int(i64::from(patrol_tile)))])
            }
        };
        Cs::Obj(vec![
            ("input".into(), input),
            ("initialized".into(), Cs::Bool(self.initialized)),
            ("random".into(), self.rng.as_ref().map_or(Cs::Null, prng_canon)),
            ("warship".into(), self.warship.canon(cx)),
            ("pathfinder".into(), self.pathfinder.as_ref().map_or(Cs::Null, water_path_finder_canon)),
            ("lastShellAttack".into(), Cs::Int(i64::from(self.last_shell_attack))),
            ("alreadySentShell".into(), Cs::Arr(self.already_sent_shell.iter().map(|u| u.canon(cx)).collect())),
            (
                "lastManualMoveTickRetreatDisabled".into(),
                Cs::Int(i64::from(self.last_manual_move_tick_retreat_disabled)),
            ),
            (
                "lastObservedPatrolTile".into(),
                self.last_observed_patrol_tile.map_or(Cs::Absent, |t| Cs::Int(i64::from(t))),
            ),
            ("activeHealingRemainder".into(), Cs::Num(self.active_healing_remainder)),
            ("lastEmittedCombat".into(), Cs::Bool(self.last_emitted_combat)),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FILE_MAP §4 P7 design guard: every TS `touch()` call site in the P7 executions
    /// (`grep -n '\.touch()' WarshipExecution.ts TradeShipExecution.ts` at the pin: 86, 687, 707,
    /// 134; ARCHITECTURE §5) is marked and calls the `unit_touched` shim on the next line, and
    /// no marker or shim call exists outside that list.
    #[test]
    fn every_ts_touch_site_calls_the_shim() {
        const EXPECTED: [&str; 4] = [
            "WarshipExecution.ts:86",
            "WarshipExecution.ts:687",
            "WarshipExecution.ts:707",
            "TradeShipExecution.ts:134",
        ];
        let files = [include_str!("warship.rs"), include_str!("trade_ship.rs")];
        let mut seen: Vec<String> = Vec::new();
        let mut shim_calls = 0;
        for src in files {
            let lines: Vec<&str> = src.lines().collect();
            for (i, l) in lines.iter().enumerate() {
                let t = l.trim_start();
                if let Some(rest) = t.strip_prefix("// touch-site: ") {
                    assert!(
                        lines.get(i + 1).is_some_and(|n| n.trim_start().starts_with("cx.st.unit_touched(")),
                        "marker {rest:?} not followed by the shim call"
                    );
                    seen.extend(rest.split_whitespace().map(str::to_owned));
                }
                if t.starts_with("cx.st.unit_touched(") {
                    shim_calls += 1;
                }
            }
        }
        seen.sort();
        let mut want: Vec<String> = EXPECTED.iter().map(|s| (*s).to_owned()).collect();
        want.sort();
        assert_eq!(seen, want);
        // :687 and :707 share one Rust call site (`capture_trade_ship`)
        assert_eq!(shim_calls, 3);
    }

    #[test]
    fn tile_truthiness_matches_js() {
        assert!(!tile_truthy(None));
        assert!(!tile_truthy(Some(0)), "TileRef 0 is falsy in `if (alternativePort)`");
        assert!(tile_truthy(Some(1)));
    }

    #[test]
    fn constructor_matches_ts_initialisers() {
        let e = WarshipExec::new(WarshipInput::New { owner: SmallId(2), patrol_tile: 9 });
        assert!(!e.initialized && e.rng.is_none() && e.warship.is_none() && e.pathfinder.is_none());
        assert_eq!((e.last_shell_attack, e.last_manual_move_tick_retreat_disabled), (0, 0));
        assert!(e.already_sent_shell.is_empty() && e.last_observed_patrol_tile.is_none());
        assert_eq!((e.active_healing_remainder, e.last_emitted_combat), (0.0, false));
        // `this.warship?.isActive()` is undefined (falsy) before init / after a failed spawn
        assert!(!e.is_active_in(&State::default()));
    }
}
