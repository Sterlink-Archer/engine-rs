//! `SAMLauncherExecution` (`engine/src/execution/SAMLauncherExecution.ts`). Owner: **P8**.
//!
//! Fields: the `SAMLauncher v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1).
//!
//! `SAMTargetingSystem` holds `mg` and `sam` (`:49-55`); its `sam` is always the launcher's
//! `this.sam` (created by `??=` once `this.sam` is set, and `this.sam` is never reassigned
//! afterwards), so the methods take the launcher's `sam` as an argument instead.

use crate::api::types::GameType;
use crate::canon::{Canon, CanonCx, Cs};
use crate::ctx::ts_throw;
#[allow(unused_imports)]
use crate::exec::prelude::*;
use crate::exec::sam_missile::SamMissileExec;
use crate::state::State;
use crate::state::unit::UnitParams;
use of_det::js;

/// `CachedInterception` (`SAMLauncherExecution.ts:33-38`, a `type`); snapshot `targeting[]` entry
/// `{ id, tick, tile, minDistSq, lastSeenTick }`.
#[derive(StateClone, Debug, Default, PartialEq)]
pub struct CachedInterception {
    /// `>= 0`: scheduled launch tick, `-1`: unreachable at current upgrade, `-2`: permanently out of reach.
    pub tick: i32,
    pub tile: TileRef,
    pub min_dist_sq: f64,
    pub last_seen_tick: i32,
}

/// `SAMTargetingSystem` (`:43-310`).
#[derive(StateClone, Debug, Default)]
pub struct SamTargetingSystem {
    /// `precomputedNukes: Map<number, CachedInterception>` keyed by nuke unit id (`:45`).
    pub precomputed_nukes: JsMap<u32, CachedInterception>,
    /// non-snapshot: `missileSpeed` (from config).
    pub missile_speed: f64,
}

/// `Target` (`:22-26`).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Target {
    unit: UnitRef,
    tile: TileRef,
    /// `score?: number` (set by `sortTargets`).
    score: Option<f64>,
}

/// `InterceptionTile` (`:28-31`).
#[derive(Clone, Copy, Debug, PartialEq)]
struct InterceptionTile {
    tile: TileRef,
    tick: f64,
}

/// A computed `CachedInterception` before it is stored (`tick` as a JS number).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Interception {
    tick: f64,
    tile: TileRef,
    min_dist_sq: f64,
    last_seen_tick: i32,
}

/// An integer-valued JS number stored in an `int` snapshot field (`zInt`).
#[inline]
fn int_tick(x: f64) -> i32 {
    debug_assert!(x.fract() == 0.0, "interception ticks are integers: {x}");
    js::to_int32(x)
}

impl SamTargetingSystem {
    /// `new SAMTargetingSystem(mg, sam)` (`:49-55`).
    pub fn new(st: &State) -> Self {
        SamTargetingSystem { precomputed_nukes: JsMap::new(), missile_speed: st.config().default_sam_missile_speed() }
    }

    /// `onLevelUp()` (`:74-80`): delete every `tick === -1` entry while iterating.
    pub fn on_level_up(&mut self) {
        // iter-mut: Map for-of with deletes of the current entry (live cursor).
        let mut c = self.precomputed_nukes.cursor();
        while let Some(id) = self.precomputed_nukes.next_live(&mut c) {
            if self.precomputed_nukes.get(&id).is_some_and(|cached| cached.tick == -1) {
                self.precomputed_nukes.remove(&id);
            }
        }
    }

    /// `updateUnreachableNukes(currentTick)` (`:82-88`).
    pub fn update_unreachable_nukes(&mut self, current_tick: i32) {
        // iter-mut: Map for-of with deletes of the current entry (live cursor).
        let mut c = self.precomputed_nukes.cursor();
        while let Some(id) = self.precomputed_nukes.next_live(&mut c) {
            if self.precomputed_nukes.get(&id).is_some_and(|cached| cached.last_seen_tick != current_tick) {
                self.precomputed_nukes.remove(&id);
            }
        }
    }

    /// `tickToReach(currentTile, tile)` (`:90-94`).
    fn tick_to_reach(&self, st: &State, current_tile: TileRef, tile: TileRef) -> f64 {
        js::ceil(f64::from(st.map.manhattan_dist(current_tile, tile)) / self.missile_speed)
    }

    /// `checkDetonationInterception(unit, samTile, ticks)` (`:96-122`).
    fn check_detonation_interception(
        &self,
        st: &State,
        sam: UnitRef,
        unit: UnitRef,
        sam_tile: TileRef,
        ticks: i32,
    ) -> Option<InterceptionTile> {
        let u = st.unit(unit);
        let trajectory = u.trajectory();
        // int: lengths/indices (TS numbers; `length - 2` can be -1 or -2).
        let len = trajectory.len() as i64;
        let max_idx = len - 2;
        let final_tile = *idx(trajectory, len - 1)?;

        let cur_idx = i64::from(u.trajectory_index());
        let wait_ticks = i64::from(u.nuke_state().wait_ticks); // TS: `waitTicks ?? 0`
        let exp_ticks = len - 1 - cur_idx + wait_ticks;
        let range = st.config().dynamic_sam_range(st, sam, tick_i32(i64::from(ticks) + exp_ticks));
        if f64::from(st.map.euclidean_dist_squared(sam_tile, final_tile)) > range * range {
            return None;
        }
        let flight_tile = *idx(trajectory, max_idx)?;

        let nuke_ticks = (max_idx - cur_idx + wait_ticks) as f64;
        let sam_ticks = self.tick_to_reach(st, sam_tile, flight_tile);
        let tick_before_shooting = nuke_ticks - sam_ticks;
        if tick_before_shooting >= 0.0 {
            Some(InterceptionTile { tick: tick_before_shooting, tile: flight_tile })
        } else {
            None
        }
    }

    /// `isTargetableAt(tile, src, dst)` (`:124-131`).
    fn is_targetable_at(&self, st: &State, tile: TileRef, src: TileRef, dst: TileRef) -> bool {
        let r = st.config().default_nuke_targetable_range();
        let target_range_squared = r * r; // TS: `** 2`
        f64::from(st.map.euclidean_dist_squared(tile, dst)) < target_range_squared
            || f64::from(st.map.euclidean_dist_squared(tile, src)) < target_range_squared
    }

    /// `computeInterceptionTile(unit, samTile, ticks)` (`:133-194`).
    fn compute_interception_tile(
        &self,
        st: &State,
        sam: UnitRef,
        unit: UnitRef,
        sam_tile: TileRef,
        ticks: i32,
    ) -> Interception {
        let u = st.unit(unit);
        let trajectory = u.trajectory();
        let cur_idx = i64::from(u.trajectory_index());
        let wait_ticks = i64::from(u.nuke_state().wait_ticks); // TS: `waitTicks ?? 0`
        let len = trajectory.len() as i64; // int: trajectory length
        let max_idx = len - 2;
        let m = st.config().max_sam_range();
        let max_sam_range_sq = m * m; // TS: `** 2`
        let mut min_dist_sq = f64::INFINITY;
        let mut closest_tile = sam_tile;
        let mut inc_steps: i32 = 0;
        let mut last_dist_sq = -1.0;

        let src = idx(trajectory, 0).copied();
        let dst = idx(trajectory, len - 1).copied();

        let mut i = cur_idx;
        while i <= max_idx {
            let Some(&tile) = idx(trajectory, i) else {
                i += 1;
                continue; // TS: `if (tile === undefined) continue;`
            };
            let dist_sq = f64::from(st.map.euclidean_dist_squared(sam_tile, tile));
            if dist_sq < min_dist_sq {
                min_dist_sq = dist_sq;
                closest_tile = tile;
            }
            inc_steps = if last_dist_sq != -1.0 && dist_sq > last_dist_sq { inc_steps + 1 } else { 0 };
            last_dist_sq = dist_sq;

            let nuke_ticks = i - cur_idx + wait_ticks;
            let sam_ticks = self.tick_to_reach(st, sam_tile, tile);
            let allowed = st.config().dynamic_sam_range(st, sam, tick_i32(i64::from(ticks) + nuke_ticks));
            let nuke_ticks = nuke_ticks as f64;
            if dist_sq <= allowed * allowed
                && nuke_ticks >= sam_ticks
                && let (Some(src), Some(dst)) = (src, dst)
                && self.is_targetable_at(st, tile, src, dst)
            {
                return Interception { tick: nuke_ticks - sam_ticks, tile, min_dist_sq, last_seen_tick: ticks };
            }
            if inc_steps > 3 && dist_sq > max_sam_range_sq {
                break;
            }
            i += 1;
        }

        if let Some(det) = self.check_detonation_interception(st, sam, unit, sam_tile, ticks) {
            return Interception { tick: det.tick, tile: det.tile, min_dist_sq, last_seen_tick: ticks };
        }
        Interception {
            tick: if min_dist_sq > max_sam_range_sq { -2.0 } else { -1.0 },
            tile: closest_tile,
            min_dist_sq,
            last_seen_tick: ticks,
        }
    }

    /// `isValidNukeTarget(unit)` (`:205-223`); `isTargetableNearbyUnit` (`:196-203`) is its
    /// `{ unit }` wrapper. `isUnit(unit)` is always true for a grid unit.
    fn is_valid_nuke_target(st: &State, sam: UnitRef, unit: UnitRef) -> bool {
        let u = st.unit(unit);
        let sam_owner = st.unit(sam).owner();
        if u.targeted_by_sam() || u.owner() == sam_owner {
            return false;
        }
        let nuke_owner = u.owner();
        if st.player(sam_owner).is_friendly(nuke_owner, false) {
            // Aftergame fun (nuking teammates once the game is over) is disabled in singleplayer.
            let game_over = !matches!(st.get_winner(), crate::state::game::Winner::None)
                && st.config().game_config().game_type != GameType::Singleplayer;
            return game_over && st.player(sam_owner).is_on_same_team(nuke_owner);
        }
        true
    }

    /// `computeTargetScore(target)` (`:225-252`).
    fn compute_target_score(st: &State, sam: UnitRef, target: &Target) -> f64 {
        let sam_tile = st.unit(sam).tile();
        let u = st.unit(target.unit);
        let trajectory = u.trajectory();
        let current_index = f64::from(u.trajectory_index());
        let len = trajectory.len() as f64; // int: trajectory length
        let time_to_explode = js::max(1.0, len - current_index);

        // TS: `unit.targetTile() ?? (trajectory.length > 0 ? trajectory[length - 1] : samTile)`
        let target_tile = u.target_tile().unwrap_or_else(|| trajectory.last().copied().unwrap_or(sam_tile));

        let dist_to_silo = f64::from(st.map.manhattan_dist(sam_tile, target_tile));

        target_score(u.unit_type(), dist_to_silo, time_to_explode)
    }

    /// `sortTargets(targets)` (`:254-263`).
    fn sort_targets(st: &State, sam: UnitRef, targets: &mut [Target]) {
        if targets.len() <= 1 {
            return;
        }

        for target in targets.iter_mut() {
            target.score = Some(Self::compute_target_score(st, sam, target));
        }

        // Sort by score, js' Timsort guarantees O(n log n)
        // TS: `b.score! - a.score!` (an inconsistent comparator is possible: V8 TimSort port).
        of_det::js_sort_by(targets, |a, b| b.score.unwrap_or(f64::NAN) - a.score.unwrap_or(f64::NAN));
    }

    /// `getValidTargets(ticks)` (`:265-309`).
    fn get_valid_targets(&mut self, st: &State, sam: UnitRef, ticks: i32) -> Vec<Target> {
        let sam_tile = st.unit(sam).tile();
        let detection_range = st.config().max_sam_range() * 4.0;
        let nukes = st.nearby_units(
            sam_tile,
            detection_range,
            &[UnitType::AtomBomb, UnitType::HydrogenBomb, UnitType::MirvWarhead],
            |st, u, _| Self::is_valid_nuke_target(st, sam, u),
            false,
        );

        let mut targets: Vec<Target> = Vec::new();
        for nuke in nukes {
            let id = st.unit(nuke.unit).id();
            if let Some(cached) = self.precomputed_nukes.get_mut(&id) {
                cached.last_seen_tick = ticks;
                let cached_tick = cached.tick;
                let cached_tile = cached.tile;

                if cached_tick == -2 || cached_tick == -1 {
                    continue;
                }

                if cached_tick == ticks || i64::from(cached_tick) == i64::from(ticks) + 1 {
                    targets.push(Target { tile: cached_tile, unit: nuke.unit, score: None });
                    self.precomputed_nukes.remove(&id);
                    continue;
                }

                if cached_tick > ticks {
                    continue;
                }

                self.precomputed_nukes.remove(&id);
            }

            let res = self.compute_interception_tile(st, sam, nuke.unit, sam_tile, ticks);
            if res.tick >= 0.0 && res.tick <= 1.0 {
                targets.push(Target { unit: nuke.unit, tile: res.tile, score: None });
            } else {
                self.precomputed_nukes.insert(
                    id,
                    CachedInterception {
                        tick: int_tick(if res.tick >= 0.0 { res.tick + f64::from(ticks) } else { res.tick }),
                        tile: res.tile,
                        min_dist_sq: res.min_dist_sq,
                        last_seen_tick: ticks,
                    },
                );
            }
        }
        self.update_unreachable_nukes(ticks);
        Self::sort_targets(st, sam, &mut targets);
        targets
    }
}

/// `typeBonus + distanceBonus + urgencyBonus` (`:238-251`).
fn target_score(t: UnitType, dist_to_silo: f64, time_to_explode: f64) -> f64 {
    // Hydro unit type bonus
    // 70,000 offset balances the distance bonus between Hydro at 100 and Atom at 30
    let type_bonus = if t == UnitType::HydrogenBomb { 70_001.0 } else { 0.0 };

    // Distance bonus: Closer to silo higher score (-1,000 pts per unit distance)
    // due to manhattanDist, distToSilo can exceed 150 diagonally, 200000 starting point.
    let distance_bonus = js::max(0.0, 200_000.0 - dist_to_silo * 1000.0);

    // Time based score: +100 pts per tick earlier
    // Since all nukes are already guaranteed to need a SAM response at this tick,
    // this is only a very minor tiebreaker.
    let urgency_bonus = js::max(0.0, 10_000.0 - time_to_explode * 100.0);

    type_bonus + distance_bonus + urgency_bonus
}

/// `trajectory[i]` with JS semantics (`undefined` for a negative or too-large index).
#[inline]
fn idx(trajectory: &[TileRef], i: i64) -> Option<&TileRef> {
    usize::try_from(i).ok().and_then(|i| trajectory.get(i))
}

/// A tick sum (`ticks + n`) as the `Tick` argument of `dynamicSamRange`.
#[inline]
fn tick_i32(t: i64) -> Tick {
    #[allow(clippy::expect_used)] // rust-invariant: game ticks and trajectory lengths are far below 2^31
    i32::try_from(t).expect("tick fits i32")
}

/// `TargetingStateSchema` (`:464-473`): `[...precomputedNukes].map(([id, c]) => ({ id, tick,
/// tile, minDistSq, lastSeenTick }))`, in insertion order (`getState`, `:58-66`).
impl Canon for SamTargetingSystem {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        Cs::Arr(
            self.precomputed_nukes
                .iter()
                .map(|(&id, c)| {
                    crate::canon_obj!(cx;
                        "id" => id,
                        "tick" => c.tick,
                        "tile" => c.tile,
                        "minDistSq" => c.min_dist_sq,
                        "lastSeenTick" => c.last_seen_tick,
                    )
                })
                .collect(),
        )
    }
}

/// `SAMLauncherExecution` data.
#[derive(StateClone, Debug)]
pub struct SamLauncherExec {
    pub active: bool,
    /// `mg !== undefined` (set by `init`).
    pub initialized: bool,
    pub player: SmallId,
    pub tile: Option<TileRef>,
    pub sam: Option<UnitRef>,
    pub last_level: i32,
    /// `targetingSystem` (created in `init`)
    pub targeting: Option<SamTargetingSystem>,
    /// `pseudoRandom ??= new PseudoRandom(sam.id())` (`:385`)
    pub pseudo_random: Option<Prng>,
}

impl SamLauncherExec {
    /// `new SAMLauncherExecution(player, tile, sam = null)`.
    ///
    /// TS (`:321-329`) also runs `if (sam !== null) this.tile = sam.tile()` in the constructor,
    /// which needs the state this frozen signature does not take. The port does that
    /// assignment at the start of `init` instead (`fill_tile_from_sam`), which runs in the
    /// same tick before anything reads `tile` (the field is read by `tick` only when
    /// `sam === null`, and a structure's tile never changes). Signature request:
    /// `docs/requests/P8.md` #1.
    pub fn new(player: SmallId, tile: Option<TileRef>, sam: Option<UnitRef>) -> Self {
        SamLauncherExec {
            active: true,
            initialized: false,
            player,
            tile,
            sam,
            last_level: 1,
            targeting: None,
            pseudo_random: None,
        }
    }

    /// The constructor's `if (sam !== null) this.tile = sam.tile()` (`:326-328`).
    fn fill_tile_from_sam(&mut self, st: &State) {
        if let Some(sam) = self.sam {
            self.tile = Some(st.unit(sam).tile());
        }
    }
}

impl ExecBehavior for SamLauncherExec {
    fn is_active(&self) -> bool {
        self.active
    }
    /// `activeDuringSpawnPhase()` returns `false` (`SAMLauncherExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    /// `init(mg, ticks)` (`:331-333`): `this.mg = mg` (+ the constructor's tile read, see `new`).
    fn init(&mut self, cx: &mut Ctx, _ticks: i32) {
        if !self.initialized {
            self.fill_tile_from_sam(cx.st);
        }
        self.initialized = true;
    }
    /// `tick(ticks)` (`:335-419`).
    fn tick(&mut self, cx: &mut Ctx, ticks: i32) {
        // TS: `if (this.mg === null || this.player === null) throw new Error("Not initialized")`
        // — `mg` is `undefined` (never `null`) before init and `player` is never null, so the
        // throw is unreachable; nothing to port.
        let sam = match self.sam {
            Some(s) => s,
            None => {
                let Some(tile) = self.tile else {
                    ts_throw!("tile is null");
                };
                let spawn_tile = cx.st.player(self.player).can_build(cx.sc, UnitType::SamLauncher, tile, None);
                let Some(spawn_tile) = spawn_tile else {
                    // TS: console.warn("cannot build SAM Launcher") (dropped)
                    self.active = false;
                    return;
                };
                let s = cx.player_mut(self.player).build_unit(UnitType::SamLauncher, spawn_tile, UnitParams::default());
                self.sam = Some(s);
                s
            }
        };
        if self.targeting.is_none() {
            self.targeting = Some(SamTargetingSystem::new(cx.st)); // TS: `??=`
        }
        let targeting = self.targeting.as_mut().ts_expect("SAMLauncherExecution.ts:351 targetingSystem");

        let level = cx.st.unit(sam).level();
        if self.last_level != level {
            self.last_level = level;
            targeting.on_level_up();
        }

        if cx.st.unit(sam).is_under_construction() {
            return;
        }

        if !cx.st.unit(sam).is_active() {
            self.active = false;
            return;
        }

        let sam_owner = cx.st.unit(sam).owner();
        if self.player != sam_owner {
            self.player = sam_owner;
        }

        // TS: `while (sam.missileTimerQueue().length > 0)` re-reads the queue every pass.
        while let Some(&front_time) = cx.st.unit(sam).missile_timer_queue().first() {
            let cooldown = cx.st.config().sam_cooldown() - f64::from(cx.st.ticks() - front_time);

            if cooldown > 0.0 {
                break;
            }
            cx.unit_mut(sam).reload_missile();
        }
        if cx.st.unit(sam).is_in_cooldown() {
            return;
        }

        if self.pseudo_random.is_none() {
            // TS: `this.pseudoRandom ??= new PseudoRandom(this.sam.id())` — constructed, never
            // drawn here, but it is snapshot state.
            self.pseudo_random = Some(Prng::new(f64::from(cx.st.unit(sam).id())));
        }

        // No nuke in flight anywhere: nothing to target, skip the grid query. (`:387-400`)
        if cx.st.unit_count_memo(UnitType::AtomBomb) == 0
            && cx.st.unit_count_memo(UnitType::HydrogenBomb) == 0
            && cx.st.unit_count_memo(UnitType::MirvWarhead) == 0
        {
            return;
        }
        // target is already filtered to exclude nukes targeted by other SAMs
        let targets = targeting.get_valid_targets(cx.st, sam, ticks);
        for target in targets {
            if cx.st.unit(sam).is_in_cooldown() {
                break;
            }
            cx.unit_mut(sam).launch();
            cx.unit_mut(target.unit).set_targeted_by_sam(true);
            let sam_tile = cx.st.unit(sam).tile();
            let sam_owner = cx.st.unit(sam).owner();
            cx.st.add_execution(Exec::SamMissile(SamMissileExec::new(
                sam_tile,
                sam_owner,
                sam,
                target.unit,
                target.tile,
            )));
        }
    }
}

// CS v1 `SAMLauncher v1 (execution)` record (`SAMLauncherExecution.ts:429-445`), dump field
// order; `targeting`/`pseudoRandom` are `null` until created. Written at merge1 by the
// integrator on behalf of P8 (the SamTargetingSystem rows above are P8's).
crate::impl_canon_record!(SamLauncherExec {
    "active" => active,
    "initialized" => initialized,
    "player" => player,
    "tile" => tile,
    "sam" => sam,
    "lastLevel" => last_level,
    "targeting" => targeting,
    "pseudoRandom" => pseudo_random,
});

#[cfg(test)]
mod tests {
    use super::*;

    fn ci(tick: i32, last_seen_tick: i32) -> CachedInterception {
        CachedInterception { tick, tile: 0, min_dist_sq: 0.0, last_seen_tick }
    }

    #[test]
    fn new_matches_ts_field_initialisers() {
        let e = SamLauncherExec::new(SmallId(2), None, Some(UnitRef::default()));
        assert!(e.active && !e.initialized && e.is_active() && !e.active_during_spawn_phase());
        assert_eq!((e.player, e.last_level), (SmallId(2), 1));
        assert!(e.targeting.is_none() && e.pseudo_random.is_none());
    }

    /// `onLevelUp` (`:74-80`) drops exactly the `-1` entries, keeping the order of the rest;
    /// deleting during the Map for-of visits every entry once.
    #[test]
    fn on_level_up_drops_unreachable_at_level() {
        let mut t = SamTargetingSystem::default();
        for (id, tick) in [(5, -1), (6, 30), (7, -2), (8, -1), (9, -1), (10, 0)] {
            t.precomputed_nukes.insert(id, ci(tick, 1));
        }
        t.on_level_up();
        let left: Vec<(u32, i32)> = t.precomputed_nukes.iter().map(|(&k, v)| (k, v.tick)).collect();
        assert_eq!(left, vec![(6, 30), (7, -2), (10, 0)]);
    }

    /// `updateUnreachableNukes` (`:82-88`) drops every entry not seen this tick.
    #[test]
    fn update_unreachable_drops_unseen() {
        let mut t = SamTargetingSystem::default();
        for (id, seen) in [(1, 9), (2, 10), (3, 8), (4, 10)] {
            t.precomputed_nukes.insert(id, ci(0, seen));
        }
        t.update_unreachable_nukes(10);
        let left: Vec<u32> = t.precomputed_nukes.keys().copied().collect();
        assert_eq!(left, vec![2, 4]);
        // delete + set moves a key to the end (`getValidTargets` re-insert, `:292-304`).
        t.precomputed_nukes.remove(&2);
        t.precomputed_nukes.insert(2, ci(5, 10));
        let left: Vec<u32> = t.precomputed_nukes.keys().copied().collect();
        assert_eq!(left, vec![4, 2]);
    }

    /// `computeTargetScore` arithmetic (`:238-251`).
    #[test]
    fn target_score_terms() {
        // Atom 30 tiles out, 20 ticks to impact: 0 + (200000 - 30000) + (10000 - 2000).
        assert_eq!(target_score(UnitType::AtomBomb, 30.0, 20.0), 178_000.0);
        // Hydrogen 100 tiles out: 70001 + 100000 + 8000 — beats the Atom above by 1.
        assert_eq!(target_score(UnitType::HydrogenBomb, 100.0, 20.0), 178_001.0);
        // both bonuses clamp at 0 (Math.max).
        assert_eq!(target_score(UnitType::MirvWarhead, 250.0, 150.0), 0.0);
        assert_eq!(target_score(UnitType::MirvWarhead, 0.0, 1.0), 209_900.0);
    }

    #[test]
    fn js_index_semantics() {
        let tr = [10, 11, 12];
        assert_eq!(idx(&tr, -1), None);
        assert_eq!(idx(&tr, 0), Some(&10));
        assert_eq!(idx(&tr, 2), Some(&12));
        assert_eq!(idx(&tr, 3), None);
        assert_eq!(int_tick(-2.0), -2);
        assert_eq!(int_tick(41.0), 41);
    }

    /// CS `targeting`: insertion-ordered rows `{ id, tick, tile, minDistSq, lastSeenTick }`
    /// (`getState`, `:58-66`; `minDistSq` may be Infinity).
    #[test]
    fn targeting_canon_rows() {
        let st = State::default();
        let ex = crate::exec::ExecStore::default();
        let mut cx = CanonCx::new(&st, &ex);
        let mut t = SamTargetingSystem::default();
        t.precomputed_nukes
            .insert(9, CachedInterception { tick: -2, tile: 44, min_dist_sq: f64::INFINITY, last_seen_tick: 3 });
        t.precomputed_nukes.insert(4, CachedInterception { tick: 17, tile: 45, min_dist_sq: 81.0, last_seen_tick: 3 });
        let row = |id: i64, tick: i64, tile: i64, d: f64| {
            Cs::Obj(vec![
                ("id".into(), Cs::Int(id)),
                ("tick".into(), Cs::Int(tick)),
                ("tile".into(), Cs::Int(tile)),
                ("minDistSq".into(), Cs::Num(d)),
                ("lastSeenTick".into(), Cs::Int(3)),
            ])
        };
        assert_eq!(t.canon(&mut cx), Cs::Arr(vec![row(9, -2, 44, f64::INFINITY), row(4, 17, 45, 81.0)]));
    }
}
