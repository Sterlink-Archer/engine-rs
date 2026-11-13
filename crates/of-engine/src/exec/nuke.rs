//! `NukeExecution` (`engine/src/execution/NukeExecution.ts`). Owner: **P8**.
//!
//! Fields: the `Nuke v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`), then non-snapshot fields. `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1).
//!
//! Dropped view work (ARCHITECTURE §5): `displayIncomingUnit` (`:243-256`), `displayMessage`
//! (`:470-489`) and the motion-plan record body (`:328-335`, only its `planDrivenUnitIds`
//! write is kept through `Ctx::record_motion_plan`). Every argument of a dropped call is a
//! pure getter. The `unit.touch()` at `:504` is **kept** (`State::unit_touched`).

use crate::api::types::unit_groups::{self, STRUCTURES};
use crate::ctx::ts_throw;
use crate::exec::nuke_alliance::{NukeAllianceCheck, list_nuke_break_alliance};
#[allow(unused_imports)]
use crate::exec::prelude::*;
use crate::path::parabola::ParabolaOptions;
use crate::path::types::PathResult;
use crate::state::State;
use crate::state::unit::{NukeStateUpdate, UnitParams};
use of_det::js;

/// `SPRITE_RADIUS = 16` (`:39`).
const SPRITE_RADIUS: f64 = 16.0;
/// `NUM_SAMPLES = 16` (`:91`).
const NUM_SAMPLES: usize = 16;

/// `NukeExecution` data.
#[derive(StateClone, Debug)]
pub struct NukeExec {
    pub active: bool,
    pub path_finder: Option<ParabolaPathFinder>,
    pub nuke: Option<UnitRef>,
    pub nuke_type: UnitType,
    pub player: SmallId,
    pub dst: TileRef,
    /// `src?: TileRef | null`
    pub src: Tri<TileRef>,
    /// `speed = -1`
    pub speed: f64,
    pub wait_ticks: i32,
    pub rocket_direction_up: bool,
    /// non-snapshot: `tilesToDestroyCache` (`:45`, the blast set `:108`)
    pub tiles_to_destroy_cache: Option<OrderedTileSet>,
}

impl NukeExec {
    /// `new NukeExecution(nukeType, player, dst, src?, speed = -1, waitTicks = 0, rocketDirectionUp = true)`.
    pub fn new(
        nuke_type: UnitType,
        player: SmallId,
        dst: TileRef,
        src: Tri<TileRef>,
        speed: f64,
        wait_ticks: i32,
        rocket_direction_up: bool,
    ) -> Self {
        // TS: NukeExecution.ts:42-56 (field initialisers + parameter properties); `mg` and
        // `pathFinder` are unset until `init`.
        NukeExec {
            active: true,
            path_finder: None,
            nuke: None,
            nuke_type,
            player,
            dst,
            src,
            speed,
            wait_ticks,
            rocket_direction_up,
            tiles_to_destroy_cache: None,
        }
    }

    /// `target()` (`:69-71`).
    pub fn target(&self, st: &State) -> Owner {
        st.owner(self.dst)
    }

    /// `getNuke()` (`:313-315`).
    pub fn get_nuke(&self) -> Option<UnitRef> {
        self.nuke
    }

    /// `owner()` (`:517-519`).
    pub fn owner(&self) -> SmallId {
        self.player
    }

    /// `tilesToDestroy()` (`:73-145`): fills `tiles_to_destroy_cache` on the first call.
    fn tiles_to_destroy(&mut self, cx: &mut Ctx) {
        if self.tiles_to_destroy_cache.is_some() {
            return;
        }
        let Some(nuke) = self.nuke else {
            ts_throw!("Not initialized"); // TS: :77-79
        };
        let magnitude = cx.st.config().nuke_magnitudes(cx.st.unit(nuke).unit_type());
        let mut rand = Prng::new(f64::from(cx.st.ticks())); // TS: :81
        let inner2 = magnitude.inner * magnitude.inner;
        let outer2 = magnitude.outer * magnitude.outer;

        if cx.st.config().water_nukes() {
            // TS: :91-102 — 16 random radii, then one smoothing pass over a copy.
            let mut radii_sq = [0.0f64; NUM_SAMPLES];
            for r in radii_sq.iter_mut() {
                *r = rand.next_float(inner2, outer2);
            }
            let prev = radii_sq; // TS: `[...radiiSq]`
            for i in 0..NUM_SAMPLES {
                let l = (i + NUM_SAMPLES - 1) % NUM_SAMPLES; // TS: (i - 1 + N) % N, non-negative
                let r = (i + 1) % NUM_SAMPLES;
                // TS: `prev[i] * 0.6 + prev[l] * 0.2 + prev[r] * 0.2` (left-assoc)
                radii_sq[i] = prev[i] * 0.6 + prev[l] * 0.2 + prev[r] * 0.2;
            }

            let ccx = f64::from(cx.st.map.x(self.dst));
            let ccy = f64::from(cx.st.map.y(self.dst));
            let outer = magnitude.outer;

            let mut result = OrderedTileSet::new();
            let x0 = js::max(0.0, ccx - outer);
            let y0 = js::max(0.0, ccy - outer);
            let x1 = js::min(f64::from(cx.st.width()) - 1.0, ccx + outer);
            let y1 = js::min(f64::from(cx.st.height()) - 1.0, ccy + outer);
            // TS: `for (let py = y0; py <= y1; py++)` over numbers (f64 counters kept).
            let mut py = y0;
            while py <= y1 {
                let mut px = x0;
                while px <= x1 {
                    let keep = ring_keep(&radii_sq, px - ccx, py - ccy, inner2, outer2);
                    if keep {
                        let tile = cx.st.map.ref_(px, py);
                        if !cx.st.map.is_impassable(tile) {
                            result.add(tile);
                        }
                    }
                    px += 1.0;
                }
                py += 1.0;
            }
            self.tiles_to_destroy_cache = Some(result);
        } else {
            // TS: :135-142. `this.mg?.euclideanDistSquared(..) ?? 0`: `mg` is always set here.
            // `GameImpl.euclideanDistSquared`/`isImpassable` delegate to the main map (`gm`).
            let dst = self.dst;
            let set = cx.st.map.bfs(&mut cx.sc.bfs_main, dst, |gm, n| {
                let d2 = f64::from(gm.euclidean_dist_squared(dst, n));
                d2 <= outer2 && (d2 <= inner2 || rand.chance(2.0)) && !gm.is_impassable(n)
            });
            self.tiles_to_destroy_cache = Some(set);
        }
    }

    /// `maybeBreakAlliances()` (`:151-200`).
    fn maybe_break_alliances(&mut self, cx: &mut Ctx) {
        let Some(nuke) = self.nuke else {
            ts_throw!("Not initialized"); // TS: :152-154
        };
        let nuke_unit_type = cx.st.unit(nuke).unit_type();
        if nuke_unit_type == UnitType::MirvWarhead {
            // MIRV warheads shouldn't break alliances
            return;
        }

        let magnitude = cx.st.config().nuke_magnitudes(nuke_unit_type);

        let players_to_break_alliance_with = list_nuke_break_alliance(
            cx.st,
            NukeAllianceCheck {
                target_tile: self.dst,
                magnitude,
                ally_small_ids: None,
                threshold: cx.st.config().nuke_alliance_break_threshold(),
            },
        );

        // Automatically reject incoming alliance requests.
        // TS: :170-174 — `incomingAllianceRequests()` is a fresh `filter` copy.
        for incoming in cx.st.player(self.player).incoming_alliance_requests() {
            let requestor = cx.st.alliance_request(incoming).requestor();
            if players_to_break_alliance_with.contains(&requestor.0) {
                cx.reject_alliance_request(incoming);
            }
        }

        // TS: :176 — the body never touches the set. // iter-live: not mutated in the loop
        for &player_small_id in players_to_break_alliance_with.iter() {
            let attacked_player = cx.st.player_by_small_id(player_small_id);
            if !attacked_player.is_player() {
                continue;
            }

            // Resolves exploit of alliance breaking in which a pending alliance request
            // was accepted in the middle of a missile attack.
            // TS: :184-190 — `.find((ar) => ar.requestor() === this.player)` on a fresh copy.
            let outgoing_alliance_request = cx
                .st
                .player(attacked_player)
                .incoming_alliance_requests()
                .into_iter()
                .find(|&ar| cx.st.alliance_request(ar).requestor() == self.player);
            if let Some(ar) = outgoing_alliance_request {
                cx.reject_alliance_request(ar);
                continue;
            }

            let alliance = cx.st.player(self.player).alliance_with(attacked_player);
            if let Some(alliance) = alliance {
                cx.player_mut(self.player).break_alliance(alliance);
            }
            if attacked_player != self.player {
                cx.player_mut(attacked_player).update_relation(self.player, -100.0);
            }
        }
    }

    /// `recordMotionPlan(ticks)` (`:323-336`): only `planDrivenUnitIds.add(unitId)` is state.
    fn record_motion_plan(&mut self, cx: &mut Ctx, _ticks: i32) {
        let Some(nuke) = self.nuke else { return };
        if self.src.is_nullish() {
            return;
        }
        let unit_id = cx.st.unit(nuke).id();
        cx.record_motion_plan(unit_id);
    }

    /// `getTrajectory(target)` (`:338-340`): `this.pathFinder.findPath(this.src!, target) ?? []`.
    fn get_trajectory(&mut self, cx: &mut Ctx, target: TileRef) -> Vec<TileRef> {
        let src = self.src.get().ts_expect("NukeExecution.ts:339 this.src!");
        let pf = self.path_finder.as_mut().ts_expect("NukeExecution.ts:339 this.pathFinder");
        pf.find_path(&cx.st.map, src, target).unwrap_or_default()
    }

    /// `isTargetable(targetTile, nukeTile, targetRangeSquared)` (`:342-353`).
    fn is_targetable(&self, st: &State, target_tile: TileRef, nuke_tile: TileRef, target_range_squared: f64) -> bool {
        f64::from(st.map.euclidean_dist_squared(nuke_tile, target_tile)) < target_range_squared
            || match self.src.get() {
                Some(src) => f64::from(st.map.euclidean_dist_squared(src, nuke_tile)) < target_range_squared,
                None => false,
            }
    }

    /// `updateNukeTargetable()` (`:355-365`).
    fn update_nuke_targetable(&mut self, cx: &mut Ctx) {
        let Some(nuke) = self.nuke else { return };
        let Some(target_tile) = cx.st.unit(nuke).target_tile() else { return };
        let r = cx.st.config().default_nuke_targetable_range();
        let target_range_squared = r * r; // TS: `** 2`
        let nuke_tile = cx.st.unit(nuke).tile();
        let targetable = self.is_targetable(cx.st, target_tile, nuke_tile, target_range_squared);
        cx.unit_mut(nuke).set_targetable(targetable);
    }

    /// `detonate()` (`:367-495`).
    fn detonate(&mut self, cx: &mut Ctx) {
        let Some(nuke) = self.nuke else {
            ts_throw!("Not initialized"); // TS: :368-370
        };

        let cfg = cx.st.cfg.clone(); // `const config = mg.config()` (shared, immutable)

        let magnitude = cfg.nuke_magnitudes(cx.st.unit(nuke).unit_type());
        self.tiles_to_destroy(cx);
        // rust-invariant: filled by `tiles_to_destroy` just above.
        #[allow(clippy::expect_used)]
        let to_destroy = self.tiles_to_destroy_cache.as_ref().expect("tilesToDestroy filled");

        // Retrieve all impacted players and the number of tiles
        // TS: `new Map<Player, number>()`, iterated (:397, :478) → JsMap keyed by smallID.
        // int: tile counts.
        let mut tiles_per_players: JsMap<SmallIdKey, u32> = JsMap::new();
        // TS: :380 — the body never mutates the blast set. // iter-live: not mutated
        for tile in to_destroy.iter() {
            let owner = cx.st.owner(tile);
            if owner.is_player() {
                cx.player_mut(owner).relinquish(tile);
                let n = tiles_per_players.get(&owner).copied().unwrap_or(0) + 1;
                tiles_per_players.insert(owner, n);
            }

            // Queue land tiles for batched water conversion
            if cx.st.map.is_land(tile) {
                cx.queue_water_conversion(tile);
            }

            // Record every tile in the blast radius for nukeable layer destruction.
            cx.queue_nuke_impact(tile);
        }

        // Then compute the explosion effect on each player
        // TS: :397 — the body never mutates the map. // iter-live: not mutated
        for (&player, &num_impacted_tiles) in tiles_per_players.iter() {
            let tiles_before_nuke = cx.st.player(player).num_tiles_owned() + num_impacted_tiles;
            let transport_ships = cx.st.player(player).units_of(&[UnitType::TransportShip]);
            // TS: `new Map<Unit, number>()` filled in `transportShips` order and only `set` on
            // existing keys afterwards, so its iteration order is `transportShips` order:
            // a parallel Vec (one entry per ship; `units(type)` has no duplicates).
            let mut transport_ship_troops: Vec<f64> = transport_ships.iter().map(|&u| cx.st.unit(u).troops()).collect();
            let max_troops = cfg.max_troops(&cx.st.player(player));
            // nukeDeathFactor could compute the complete fallout in a single call instead
            for i in 0..num_impacted_tiles {
                // Diminishing effect as each affected tile has been nuked
                let num_tiles_left = f64::from(tiles_before_nuke - i);
                let troops = cx.st.player(player).troops();
                let deaths = cfg.nuke_death_factor(self.nuke_type, troops, num_tiles_left, max_troops);
                cx.player_mut(player).remove_troops(deaths);
                // TS: `const outgoingAttacks = player.outgoingAttacks()` (the live array) walked
                // with for-of; `attack.setTroops` cannot reassign it. // iter-live: no removal
                let mut k = 0;
                while let Some(attack) = cx.st.player(player).outgoing_attacks().get(k) {
                    let attack_troops = cx.st.attack(attack).troops();
                    let deaths = cfg.nuke_death_factor(self.nuke_type, attack_troops, num_tiles_left, max_troops);
                    cx.st.attack_mut(attack).set_troops(attack_troops - deaths);
                    k += 1;
                }
                for troops in transport_ship_troops.iter_mut() {
                    // TS: `transportShipTroops.get(unit) ?? unit.troops()` — always present.
                    let unit_troops = *troops;
                    let deaths = cfg.nuke_death_factor(self.nuke_type, unit_troops, num_tiles_left, max_troops);
                    *troops = js::max(0.0, unit_troops - deaths);
                }
            }
            for (&unit, &troops) in transport_ships.iter().zip(transport_ship_troops.iter()) {
                cx.unit_mut(unit).set_troops(troops);
            }
        }

        let outer2 = magnitude.outer * magnitude.outer;
        let dst = self.dst;
        // TS: `mg.units()` builds a fresh array (GameImpl.ts:349-374): a snapshot.
        for unit in cx.st.all_units() {
            let t = cx.st.unit(unit).unit_type();
            if t == UnitType::AtomBomb
                || t == UnitType::HydrogenBomb
                || t == UnitType::MirvWarhead
                || t == UnitType::Mirv
                || t == UnitType::SamMissile
            {
                continue;
            }
            if f64::from(cx.st.map.euclidean_dist_squared(dst, cx.st.unit(unit).tile())) < outer2 {
                // treatAFKFriendly matches warship targeting: a disconnected
                // teammate's or ally's units are still not kills.
                let unit_owner = cx.st.unit(unit).owner();
                let friendly = cx.st.player(self.player).is_friendly(unit_owner, true);
                cx.unit_mut(unit).delete(Some(true), if friendly { None } else { Some(self.player) });
            }
        }

        self.redraw_buildings(cx, magnitude.outer + SPRITE_RADIUS);
        self.active = false;
        cx.unit_mut(nuke).set_reached_target();
        cx.unit_mut(nuke).delete(Some(false), None);

        // TS: :470-489 — `displayMessage` per impacted player (view, dropped; its arguments
        // are pure getters).

        // Record stats
        let target = self.target(cx.st);
        let nuke_unit_type = cx.st.unit(nuke).unit_type();
        cx.st.stats_cx().bomb_land(self.player, target, nuke_unit_type);
    }

    /// `redrawBuildings(range)` (`:497-508`): `unit.touch()` → the `toUpdate` shim
    /// (ARCHITECTURE §5; FILE_MAP §4 P8 design test). Never dropped.
    fn redraw_buildings(&mut self, cx: &mut Ctx, range: f64) {
        let range_squared = range * range;
        // TS: `this.mg.units()` is a fresh array (snapshot).
        for unit in cx.st.all_units() {
            if unit_groups::has(STRUCTURES, cx.st.unit(unit).unit_type())
                && f64::from(cx.st.map.euclidean_dist_squared(self.dst, cx.st.unit(unit).tile())) < range_squared
            {
                cx.st.unit_touched(unit); // TS: NukeExecution.ts:504 `unit.touch()`
            }
        }
    }

    /// `cancel()` (`NukeExecution.ts:510-515`); reached only through MIRV's
    /// `ExecCmd::CancelNuke` (ARCHITECTURE §4.3).
    pub fn cancel(&mut self, cx: &mut Ctx) {
        self.active = false;
        if let Some(n) = self.nuke {
            // TS: this.nuke !== null && this.nuke.isActive() → this.nuke.delete(false)
            if cx.st.unit(n).is_active() {
                cx.unit_mut(n).delete(Some(false), None);
            }
        }
    }
}

/// The water-nuke ring test of `tilesToDestroy` (`:115-127`) for offset `(dx, dy)`: `false`
/// where TS `continue`s before `mg.ref`. The angle lookup is `of_det::detmath::nuke_ring_lookup`
/// (the exact `:120-124` expressions, checked by the `detmath/nuke_ring_lookup` vectors).
fn ring_keep(radii_sq: &[f64; NUM_SAMPLES], dx: f64, dy: f64, inner2: f64, outer2: f64) -> bool {
    let d2 = dx * dx + dy * dy;
    if d2 > outer2 {
        return false;
    }
    if d2 > inner2 {
        // TS: angle = atan2(dy, dx) + Math.PI; t = (angle / (2 * Math.PI)) * NUM_SAMPLES;
        //     i0 = Math.floor(t) % NUM_SAMPLES; frac = t - Math.floor(t)
        let (_t, i0, frac) = of_det::detmath::nuke_ring_lookup(dy, dx);
        // in-range: d2 > inner2 >= 0 so (dx, dy) != (0, 0); t ∈ [0, 16], i0 = floor(t) % 16 ∈ [0, 15].
        debug_assert!((0.0..16.0).contains(&i0) && i0.fract() == 0.0);
        let i0 = i0 as usize;
        let i1 = (i0 + 1) % NUM_SAMPLES;
        let threshold = radii_sq[i0] * (1.0 - frac) + radii_sq[i1] * frac;
        if d2 > threshold {
            return false;
        }
    }
    true
}

impl ExecBehavior for NukeExec {
    fn is_active(&self) -> bool {
        self.active
    }
    /// `activeDuringSpawnPhase()` returns `false` (`NukeExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    /// `init(mg, ticks)` (`:58-67`).
    fn init(&mut self, cx: &mut Ctx, _ticks: i32) {
        if self.speed == -1.0 {
            self.speed = cx.st.config().nuke_speed(self.nuke_type);
        }
        // TS: `UniversalPathFinding.Parabola(mg, { increment, directionUp })` — the other
        // option keys are absent (`?undef`).
        self.path_finder = Some(ParabolaPathFinder::new(Some(ParabolaOptions {
            increment: Some(self.speed),
            distance_based_height: None,
            direction_up: Some(self.rocket_direction_up),
            ignore_map_bounds: None,
        })));
    }
    /// `tick(ticks)` (`:202-311`).
    fn tick(&mut self, cx: &mut Ctx, ticks: i32) {
        let nuke = match self.nuke {
            Some(n) => n,
            None => {
                let spawn = cx.st.player(self.player).can_build(cx.sc, self.nuke_type, self.dst, None);
                let Some(spawn) = spawn else {
                    // TS: console.warn(`cannot build Nuke`) (dropped)
                    self.active = false;
                    return;
                };
                // The launch tile can be overridden by the caller (e.g. MIRV warheads
                // launch from the MIRV separation point, not a silo).
                if self.src.is_nullish() {
                    self.src = Tri::Some(spawn); // TS: `this.src ??= spawn`
                }
                let silo = cx
                    .st
                    .player(self.player)
                    .units_of(&[UnitType::MissileSilo])
                    .into_iter()
                    .find(|&s| cx.st.unit(s).tile() == spawn);
                // Stacked purchases launch several nukes across ticks; delay each missile
                // so launches from the same silo trail each other instead of overlapping,
                // need to check the entire queue because even if nukes have waitticks,
                // the silo queue will be filled with the same tick.
                if let Some(silo) = silo {
                    // int: ticks. `Math.max(launchTick + 1, lastDep + 1)` on integers.
                    let mut last_dep: i32 = 0;
                    for &launch_tick in cx.st.unit(silo).missile_timer_queue() {
                        last_dep = std::cmp::max(launch_tick + 1, last_dep + 1);
                    }
                    if last_dep > cx.st.ticks() {
                        self.wait_ticks += last_dep - cx.st.ticks();
                    }
                }
                // TS: the params object (and with it `getTrajectory`) is evaluated before
                // `buildUnit` runs (`:229-232`).
                let trajectory = self.get_trajectory(cx, self.dst);
                let src = self.src.get().ts_expect("NukeExecution.ts:229 this.src");
                let n = cx.player_mut(self.player).build_unit(
                    self.nuke_type,
                    src,
                    UnitParams {
                        target_tile: Some(Some(self.dst)),
                        trajectory: Some(trajectory),
                        ..UnitParams::default()
                    },
                );
                self.nuke = Some(n);
                cx.unit_mut(n).update_nuke_state(NukeStateUpdate {
                    wait_ticks: Some(self.wait_ticks),
                    ..NukeStateUpdate::default()
                });
                self.record_motion_plan(cx, ticks);
                if cx.st.unit(n).unit_type() != UnitType::MirvWarhead {
                    self.maybe_break_alliances(cx);
                }
                if cx.st.map.has_owner(self.dst) {
                    let target = cx.st.owner(self.dst);
                    // TS: :240-256 — terra nullius is ignored; the `displayIncomingUnit`
                    // calls for Atom/Hydrogen bombs are view (dropped).

                    if self.nuke_type == UnitType::Mirv {
                        cx.record_mirv_launch();
                    }
                    // Record stats
                    cx.st.stats_cx().bomb_launch(self.player, target, self.nuke_type);
                }

                // after sending a nuke set the missilesilo on cooldown
                if let Some(silo) = silo {
                    cx.unit_mut(silo).launch();
                }
                return;
            }
        };

        // make the nuke unactive if it was intercepted
        if !cx.st.unit(nuke).is_active() {
            self.active = false;
            return;
        }

        if self.wait_ticks > 0 {
            self.wait_ticks -= 1; // TS: `--this.waitTicks`
            cx.unit_mut(nuke)
                .update_nuke_state(NukeStateUpdate { wait_ticks: Some(self.wait_ticks), ..NukeStateUpdate::default() });
            return;
        }

        // Move to next tile
        let src = self.src.get().ts_expect("NukeExecution.ts:284 this.src!");
        let pf = self.path_finder.as_mut().ts_expect("NukeExecution.ts:284 this.pathFinder");
        let result = pf.next(&cx.st.map, src, self.dst, Some(self.speed));

        match result {
            PathResult::Complete(node) => {
                // move it afterward for visual effect
                cx.unit_mut(nuke).move_to(node);

                // Check for very close SAM missiles that are targeting this.
                // The SAM logic should be the main source of truth, since missiles can skip pixels
                // and be affected by execution order
                // TS: :293-303 (`isUnit(unit)` is always true for a grid unit).
                let nuke_owner = cx.st.unit(nuke).owner();
                let nuke_id = cx.st.unit(nuke).id();
                let should_be_destroyed = !cx
                    .st
                    .nearby_units(
                        self.dst,
                        cx.st.config().default_sam_missile_speed(),
                        &[UnitType::SamMissile],
                        |st, u, _| {
                            if st.unit(u).owner() == nuke_owner {
                                return false;
                            }
                            st.unit(u).target_unit().map(|t| st.unit(t).id()) == Some(nuke_id)
                        },
                        false,
                    )
                    .is_empty(); // TS: `.length >= 1`
                if !should_be_destroyed {
                    self.detonate(cx);
                }
            }
            PathResult::Next(node) => {
                self.update_nuke_targetable(cx);
                cx.unit_mut(nuke).move_to(node);
                // Update index so SAM can interpolate future position
                // rust-invariant: `path_finder` was used just above.
                let idx = self.path_finder.as_ref().map_or(0, |pf| pf.current_index());
                cx.unit_mut(nuke).set_trajectory_index(idx);
            }
            PathResult::NotFound => {}
        }
    }
}

// CS v1 `Nuke v1 (execution)` record (`NukeExecution.ts:531-545`), dump field order.
// `pathFinder` is `null` until `init`; `src` keeps undefined apart from null (`Tri`). Written at
// merge1 by the integrator on behalf of P8 (request P8 #2: the Parabola leaf now exists).
crate::impl_canon_record!(NukeExec {
    "active" => active,
    "pathFinder" => path_finder,
    "nuke" => nuke,
    "nukeType" => nuke_type,
    "player" => player,
    "dst" => dst,
    "src" => src,
    "speed" => speed,
    "waitTicks" => wait_ticks,
    "rocketDirectionUp" => rocket_direction_up,
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_matches_ts_field_initialisers() {
        // TS: NukeExecution.ts:42-56 — `active = true`, `nuke = null`, no pathfinder before init.
        let e = NukeExec::new(UnitType::AtomBomb, SmallId(3), 77, Tri::Absent, -1.0, 0, true);
        assert!(e.active && e.is_active() && !e.active_during_spawn_phase());
        assert!(e.nuke.is_none() && e.path_finder.is_none() && e.tiles_to_destroy_cache.is_none());
        assert_eq!(e.nuke_type, UnitType::AtomBomb);
        assert_eq!((e.player, e.dst, e.src), (SmallId(3), 77, Tri::Absent));
        assert_eq!((e.speed, e.wait_ticks, e.rocket_direction_up), (-1.0, 0, true));
        assert_eq!(e.owner(), SmallId(3));
        assert_eq!(e.get_nuke(), None);
    }

    #[test]
    fn cancel_without_nuke_only_deactivates() {
        // TS: NukeExecution.ts:510-515 with `this.nuke === null`: no unit is touched.
        let mut st = State::default();
        let mut sc = crate::scratch::Scratch::new();
        let mut cx = Ctx::new(&mut st, &mut sc, None);
        let mut e = NukeExec::new(UnitType::MirvWarhead, SmallId(1), 5, Tri::Some(9), 6.0, 3, true);
        e.cancel(&mut cx);
        assert!(!e.is_active());
        e.cancel(&mut cx); // a second cancel (detached warhead) is a no-op
        assert!(!e.is_active());
    }

    /// The water-nuke ring (`:113-127`) with flat radii equals the plain disc test.
    #[test]
    fn ring_keep_flat_radii_is_disc() {
        let inner2 = 100.0; // inner 10
        let outer2 = 400.0; // outer 20
        let radii = [outer2; NUM_SAMPLES];
        for dy in -21..=21 {
            for dx in -21..=21 {
                let (dx, dy) = (f64::from(dx), f64::from(dy));
                let d2 = dx * dx + dy * dy;
                if d2 == outer2 {
                    // `400 * (1 - frac) + 400 * frac` may be 1 ulp off 400: the boundary
                    // depends on rounding, so it is not asserted here.
                    continue;
                }
                assert_eq!(ring_keep(&radii, dx, dy, inner2, outer2), d2 < outer2, "({dx},{dy})");
            }
        }
        // radii at inner2 everywhere: only the inner disc survives.
        let radii = [inner2; NUM_SAMPLES];
        for dy in -21..=21 {
            for dx in -21..=21 {
                let (dx, dy) = (f64::from(dx), f64::from(dy));
                let d2 = dx * dx + dy * dy;
                assert_eq!(ring_keep(&radii, dx, dy, inner2, outer2), d2 <= inner2, "({dx},{dy})");
            }
        }
    }

    /// One sample raised: the interpolation (`radiiSq[i0] * (1 - frac) + radiiSq[i1] * frac`)
    /// keeps a ring tile exactly when `d2 <= threshold`, with i0/frac from the vector-checked
    /// lookup. Offset (-15, -1): atan2(-1,-15)+π ≈ 0.0666 → t ≈ 0.1697, i0 = 0, i1 = 1.
    #[test]
    fn ring_keep_interpolates_between_samples() {
        let inner2 = 100.0;
        let outer2 = 400.0;
        let mut radii = [inner2; NUM_SAMPLES];
        radii[0] = outer2;
        let (dx, dy) = (-15.0, -1.0);
        let (t, i0, frac) = of_det::detmath::nuke_ring_lookup(dy, dx);
        assert_eq!(i0, 0.0);
        assert!(t > 0.0 && t < 1.0);
        let threshold = radii[0] * (1.0 - frac) + radii[1] * frac;
        let d2 = dx * dx + dy * dy; // 226
        assert_eq!(ring_keep(&radii, dx, dy, inner2, outer2), d2 <= threshold);
        assert!(d2 <= threshold, "226 <= {threshold}");
        // the mirror offset (-15, +1) maps to t ≈ 15.83 (i0 = 15, i1 = 0): also near sample 0.
        let (_, i0b, _) = of_det::detmath::nuke_ring_lookup(1.0, -15.0);
        assert_eq!(i0b, 15.0);
        assert!(ring_keep(&radii, -15.0, 1.0, inner2, outer2));
        // the opposite side (+15, 0): t = 8 → sample 8 = inner2 → dropped.
        assert!(!ring_keep(&radii, 15.0, 0.0, inner2, outer2));
    }
}
