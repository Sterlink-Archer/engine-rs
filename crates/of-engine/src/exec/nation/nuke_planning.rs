//! `execution/nation/NationNukePlanning.ts`. Owner: **P11**.
//!
//! Free functions over a read-only game (no PRNG, no mutation): blast coverage, SAM
//! interception along the parabola, and the SAM-overwhelm salvo planner used by
//! `NationNukeBehavior.maybeDestroyEnemySam` (Impossible).
//!
//! Every TS `number` here is `f64` unless marked `// int:`. Tile distances come from
//! `GameMap::{euclidean_dist_squared, manhattan_dist}` as `u32` and are widened to `f64`
//! **before** any subtraction (TS subtracts numbers; a `u32` difference would wrap).

use crate::api::types::UnitType;
#[allow(unused_imports)]
use crate::exec::prelude::*;
use crate::path::parabola::ParabolaOptions;
use crate::state::State;
use of_det::js;

/// `NukeType = UnitType.AtomBomb | UnitType.HydrogenBomb` (`:6`): plain `UnitType` here.
/// `MAX_NATION_SILO_UPGRADE_LEVEL = 5` (`:9`).
pub(crate) const MAX_NATION_SILO_UPGRADE_LEVEL: i32 = 5;

/// `SamSalvo` (`:179-183`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SamSalvo {
    pub target_tile: TileRef,
    /// `waitTicks: number[]`. // int: every element is `max(0, a - b)` of integer tick
    /// counts (`:380-382`), and `0` (`:387`).
    pub wait_ticks: Vec<i32>,
}

/// `FailedSamSalvo` (`:185-190`).
#[derive(Clone, Debug, Default)]
pub struct FailedSamSalvo {
    pub target_tile: TileRef,
    /// `coveringSamIds: Set<number>` (unit ids). Membership-tested only (`excludedSamIds?.has`,
    /// `:165`); insertion order kept anyway (`JsSet`).
    pub covering_sam_ids: JsSet<u32>,
    /// // int: `bombsNeeded + extraBombs` (`:309`), sums of SAM levels.
    pub total_bombs: i32,
}

/// The parabola every AI nuke flies (`:107-111`, `:264-268`): `UniversalPathFinding.Parabola(game,
/// { increment: speed, distanceBasedHeight: true, directionUp: true })`.
fn ai_parabola(speed: f64) -> ParabolaPathFinder {
    ParabolaPathFinder::new(Some(ParabolaOptions {
        increment: Some(speed),
        distance_based_height: Some(true), // Atom/Hydrogen bombs use distance-based height
        direction_up: Some(true),          // AI nukes always go "up" for now
        ignore_map_bounds: None,
    }))
}

/// `game.euclideanDistSquared(a, b)` as the TS number.
#[inline]
fn d2(st: &State, a: TileRef, b: TileRef) -> f64 {
    f64::from(st.map.euclidean_dist_squared(a, b))
}

/// `game.nearbyUnits(tile, range, UnitType.SAMLauncher)`: no predicate, under-construction
/// units excluded (the TS default, `GameImpl.ts:1436-1453`).
#[inline]
fn nearby_sams(st: &State, tile: TileRef, range: f64) -> Vec<crate::map::unit_grid::NearbyUnit> {
    st.nearby_units(tile, range, &[UnitType::SamLauncher], |_, _, _| true, false)
}

/// `isNukeAlreadyLandingNear(game, tile, nukeType)` (`:12-33`): whether our inner blast would
/// overlap the inner blast of any player's nuke in flight.
pub fn is_nuke_already_landing_near(st: &State, tile: TileRef, nuke_type: UnitType) -> bool {
    let config = st.config();
    let our_inner_radius = config.nuke_magnitudes(nuke_type).inner;
    let nukes_in_flight = st.units_of(&[UnitType::AtomBomb, UnitType::HydrogenBomb, UnitType::MirvWarhead]);
    for nuke in nukes_in_flight {
        let u = st.unit(nuke);
        let Some(target_tile) = u.target_tile() else { continue };
        let sum_radius = our_inner_radius + config.nuke_magnitudes(u.unit_type()).inner;
        if d2(st, tile, target_tile) <= sum_radius * sum_radius {
            return true;
        }
    }
    false
}

/// `enemyLandBlastCoverage(game, player, center, nukeType)` (`:40-67`): grid-sampled share of
/// the blast that hits land of players not friendly to `player`; inner-radius samples count
/// double.
pub fn enemy_land_blast_coverage(st: &State, player: SmallId, center: TileRef, nuke_type: UnitType) -> f64 {
    let m = st.config().nuke_magnitudes(nuke_type);
    let (inner, outer) = (m.inner, m.outer);
    let step = js::max(1.0, js::floor(outer / 10.0));
    let cx = f64::from(st.map.x(center));
    let cy = f64::from(st.map.y(center));
    let me = st.player(player);
    let mut total = 0.0;
    let mut enemy_land = 0.0;
    // TS: `for (let dy = -outer; dy <= outer; dy += step)` in numbers (`:52-53`).
    let mut dy = -outer;
    while dy <= outer {
        let mut dx = -outer;
        while dx <= outer {
            let dist_squared = dx * dx + dy * dy;
            if dist_squared > outer * outer {
                dx += step;
                continue;
            }
            let weight = if dist_squared <= inner * inner { 2.0 } else { 1.0 };
            total += weight;
            if !st.map.is_valid_coord(cx + dx, cy + dy) {
                dx += step;
                continue;
            }
            // Water, fallout and unowned land have no player owner
            let owner = st.owner(st.map.ref_(cx + dx, cy + dy));
            if owner.is_player() && !me.is_friendly(owner, true) {
                enemy_land += weight;
            }
            dx += step;
        }
        dy += step;
    }
    enemy_land / total
}

/// `blastHitsFriendlyLand(game, player, center, nukeType, margin = 0)` (`:70-95`): whether any
/// tile in the blast, widened by `margin`, belongs to `player`, a teammate or an ally (even AFK).
pub fn blast_hits_friendly_land(
    st: &State,
    player: SmallId,
    center: TileRef,
    nuke_type: UnitType,
    margin: f64,
) -> bool {
    let me = st.player(player);
    // TS: `new Set(game.players().filter(p => player.isFriendly(p, true)).map(p => p.smallID()))`
    // (`:77-82`). lookup-only: only `friendlyIds.has` (`:91`); a few dozen entries, so a Vec.
    let friendly_ids: Vec<u16> = st.players().into_iter().filter(|&p| me.is_friendly(p, true)).map(|p| p.0).collect();
    let outer = st.config().nuke_magnitudes(nuke_type).outer + margin;
    let cx = f64::from(st.map.x(center));
    let cy = f64::from(st.map.y(center));
    let mut dy = -outer;
    while dy <= outer {
        let mut dx = -outer;
        while dx <= outer {
            if dx * dx + dy * dy > outer * outer || !st.map.is_valid_coord(cx + dx, cy + dy) {
                dx += 1.0;
                continue;
            }
            let owner_id = st.map.owner_id(st.map.ref_(cx + dx, cy + dy));
            if friendly_ids.contains(&owner_id) {
                return true;
            }
            dx += 1.0;
        }
        dy += 1.0;
    }
    false
}

/// `isTrajectoryInterceptableBySam(game, player, spawnTile, targetTile, excludedSamIds?)`
/// (`:99-176`): whether a SAM hostile to `player` can shoot down a bomb flying from
/// `spawn_tile` to `target_tile`.
pub fn is_trajectory_interceptable_by_sam(
    st: &State,
    player: SmallId,
    spawn_tile: TileRef,
    target_tile: TileRef,
    excluded_sam_ids: Option<&JsSet<u32>>,
) -> bool {
    let config = st.config();
    let speed = config.nuke_speed(UnitType::AtomBomb);
    let mut path_finder = ai_parabola(speed);

    let trajectory = path_finder.find_path(&st.map, spawn_tile, target_tile).unwrap_or_default();
    if trajectory.is_empty() {
        return false;
    }

    let r = config.default_nuke_targetable_range();
    let target_range_squared = r * r; // TS: `** 2`

    // int: indexes into `trajectory`, or the TS `-1` sentinel.
    let mut untargetable_start: i64 = -1;
    let mut untargetable_end: i64 = -1;
    for (i, &tile) in trajectory.iter().enumerate() {
        if untargetable_start == -1 {
            if d2(st, tile, spawn_tile) > target_range_squared {
                if d2(st, tile, target_tile) < target_range_squared {
                    // Overlapping spawn & target range – no untargetable segment.
                    break;
                } else {
                    untargetable_start = i as i64;
                }
            }
        } else if d2(st, tile, target_tile) < target_range_squared {
            untargetable_end = i as i64;
            break;
        }
    }

    let me = st.player(player);
    let max_sam_range = config.max_sam_range();
    let len = trajectory.len() as i64;
    let mut i: i64 = 0;
    while i < len {
        // Skip the mid-air untargetable portion (TS: `i = untargetableEnd - 1; continue;`, then
        // the loop's `i++`, `:143-150`).
        if untargetable_start != -1 && untargetable_end != -1 && i == untargetable_start {
            i = untargetable_end - 1;
            i += 1;
            continue;
        }

        let tile = trajectory[i as usize]; // in-range: 0 <= i < len
        for sam in nearby_sams(st, tile, max_sam_range) {
            let owner = st.unit(sam.unit).owner();
            if owner == player || me.is_friendly(owner, false) {
                continue;
            }
            // Skip SAMs we're intentionally overwhelming
            if excluded_sam_ids.is_some_and(|s| s.contains(&st.unit(sam.unit).id())) {
                continue;
            }
            let range = config.sam_range(f64::from(st.unit(sam.unit).level()));
            let range_squared = range * range; // TS: `** 2`
            if sam.dist_squared <= range_squared {
                return true;
            }
        }
        i += 1;
    }

    false
}

/// One silo in `planSamSalvo`'s `allAvailableSilos` (`:245-250`).
#[derive(Clone, Copy, Debug)]
struct AvailableSilo {
    silo: UnitRef,
    slots: f64,
    flight_ticks: f64,
    interceptable: bool,
}

/// One bomb of `launchSequence` (`:289-292`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LaunchSlot {
    pub flight_ticks: f64,
    pub interceptable: bool,
}

/// One entry of `unblockedBombs` (`:313`).
#[derive(Clone, Copy, Debug, PartialEq)]
struct UnblockedBomb {
    /// // int: an index into `launchSequence`.
    index: usize,
    flight_ticks: f64,
}

/// What `planSamSalvo` decides for one target SAM once `launchSequence` is known (`:302-389`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum SalvoWindow {
    /// `unblockedBombs.length < totalBombs` or `bestWindowCount < totalBombs`: record `failed`.
    Short { total_bombs: i32 },
    /// Enough bombs arrive within one window: the per-bomb waits and how many bombs fire.
    Fire { wait_ticks: Vec<i32>, bombs_to_fire: usize },
}

/// The pure middle of `planSamSalvo` (`:302-389`), split out unchanged so the window search and
/// the stagger can be unit-tested without a game. `bombs_needed = totalInterceptions + 1`.
pub(crate) fn plan_salvo_window(launch_sequence: &[LaunchSlot], bombs_needed: f64, sam_cooldown: f64) -> SalvoWindow {
    // Use half the SAM cooldown as the max total arrival spread to be safe.
    let max_total_arrival_spread = js::floor(sam_cooldown / 2.0);

    // Add extra bombs: 1 for every 5 to account for enemy building more SAMs
    // while our bombs are in flight
    let extra_bombs = js::floor(bombs_needed / 5.0);
    let total_bombs = bombs_needed + extra_bombs;
    // int: a sum of SAM levels + 1 + its fifth, floored.
    let total_bombs_i = js::to_int32(total_bombs);

    // Collect bombs from silos whose trajectory to the target is NOT blocked
    // by enemy SAMs other than the covering SAMs we're trying to overwhelm.
    let mut unblocked_bombs: Vec<UnblockedBomb> = Vec::new();
    for (i, b) in launch_sequence.iter().enumerate() {
        if !b.interceptable {
            unblocked_bombs.push(UnblockedBomb { index: i, flight_ticks: b.flight_ticks });
        }
    }

    if (unblocked_bombs.len() as f64) < total_bombs {
        return SalvoWindow::Short { total_bombs: total_bombs_i };
    }

    // Sort unblocked bombs by flight time to find a sliding window
    // of maxTotalArrivalSpread that captures the most bombs.
    let mut sorted_by_flight = unblocked_bombs.clone();
    of_det::js_sort_by(&mut sorted_by_flight, |a, b| a.flight_ticks - b.flight_ticks); // TS: NationNukePlanning.ts:330

    let mut best_window_start = 0usize;
    let mut best_window_count = 0usize;
    for start in 0..sorted_by_flight.len() {
        let mut end = start;
        while end < sorted_by_flight.len()
            && sorted_by_flight[end].flight_ticks - sorted_by_flight[start].flight_ticks <= max_total_arrival_spread
        {
            end += 1;
        }
        if end - start > best_window_count {
            best_window_count = end - start;
            best_window_start = start;
        }
    }

    if (best_window_count as f64) < total_bombs {
        return SalvoWindow::Short { total_bombs: total_bombs_i };
    }

    // From the window, pick totalBombs with the lowest launch-sequence
    // indices to minimise how many bombs we need to fire (minimise gold cost).
    let window_bombs = &sorted_by_flight[best_window_start..best_window_start + best_window_count];
    let mut window_by_index = window_bombs.to_vec();
    of_det::js_sort_by(&mut window_by_index, |a, b| a.index as f64 - b.index as f64); // TS: :362
    // TS: `windowByIndex.slice(0, totalBombs)`; totalBombs <= bestWindowCount here.
    let take = (total_bombs as usize).min(window_by_index.len());
    let selected = &window_by_index[..take];
    // lookup-only: `selectedSet.has(i)` (`:379`).
    let selected_set: Vec<usize> = selected.iter().map(|b| b.index).collect();
    let last_selected_index = selected[selected.len() - 1].index; // in-range: total_bombs >= 1
    let bombs_to_fire = last_selected_index + 1;

    // Compute per-bomb waitTicks so all selected bombs arrive in the window.
    // Target: spread arrivals evenly, anchored at the earliest flight time
    // in the selected set.
    let selected_flights: Vec<f64> = selected.iter().map(|b| b.flight_ticks).collect();
    let selected_flight_min = js::min_n(&selected_flights); // TS: Math.min(...arr) (:371)
    let stagger_interval = js::max(1.0, js::floor(max_total_arrival_spread / total_bombs));
    let mut selected_idx = 0.0;
    let mut wait_ticks_per_bomb: Vec<i32> = Vec::new();
    for (i, slot) in launch_sequence.iter().enumerate().take(bombs_to_fire) {
        if selected_set.contains(&i) {
            let target_arrival = selected_flight_min + selected_idx * stagger_interval;
            // int: integer tick counts (trajectory lengths, floored spreads).
            wait_ticks_per_bomb.push(js::to_int32(js::max(0.0, target_arrival - slot.flight_ticks)));
            selected_idx += 1.0;
        } else {
            // Wasted bomb (interceptable or out-of-window) — launch immediately
            wait_ticks_per_bomb.push(0);
        }
    }
    SalvoWindow::Fire { wait_ticks: wait_ticks_per_bomb, bombs_to_fire }
}

/// `planSamSalvo(game, player, target)` (`:198-402`): an atom bomb salvo from `player` that
/// destroys one of `target`'s SAMs, easiest (lowest level) first. Returns `(salvo, failed)`:
/// `failed` is the first SAM our silos lacked the capacity for (kept even when a later SAM
/// succeeds, as TS returns `{ salvo, failed }` at `:398`).
pub fn plan_sam_salvo(st: &State, player: SmallId, target: SmallId) -> (Option<SamSalvo>, Option<FailedSamSalvo>) {
    let config = st.config();
    let atom_cost = config.unit_cost(UnitType::AtomBomb, st, player, 0.0);
    let enemy_sams = st.player(target).units_of(&[UnitType::SamLauncher]);
    if enemy_sams.is_empty() {
        return (None, None);
    }

    let our_silos: Vec<UnitRef> = st
        .player(player)
        .units_of(&[UnitType::MissileSilo])
        .into_iter()
        .filter(|&silo| !st.unit(silo).is_under_construction())
        .collect();
    if our_silos.is_empty() {
        return (None, None);
    }

    // Try each enemy SAM as a target, easiest (lowest level) first
    let mut sorted_sams = enemy_sams.clone();
    of_det::js_sort_by(&mut sorted_sams, |&a, &b| f64::from(st.unit(a).level()) - f64::from(st.unit(b).level())); // TS: NationNukePlanning.ts:217
    // Track the first failed attempt so we can upgrade a silo that would
    // actually have helped that plan (rather than an unrelated silo).
    let mut failed: Option<FailedSamSalvo> = None;

    for target_sam in sorted_sams {
        let target_tile = st.unit(target_sam).tile();
        // Hitting an ally's land would break the alliance
        if blast_hits_friendly_land(st, player, target_tile, UnitType::AtomBomb, 0.0) {
            continue;
        }

        // Find all enemy SAMs whose range covers the target tile (they will all try to intercept)
        let covering_sams = find_enemy_sams_covering_tile(st, player, target_tile);
        let covering_sam_ids: JsSet<u32> = covering_sams.iter().map(|&s| st.unit(s).id()).collect();

        // Total interception capacity = sum of covering SAM levels
        let total_interceptions = covering_sams.iter().fold(0.0, |sum, &sam| sum + f64::from(st.unit(sam).level()));
        let bombs_needed = total_interceptions + 1.0;

        // NukeExecution always picks the closest non-cooldown silo by Manhattan
        // distance to target (via nukeSpawn). Our planning must mirror that order.
        let nuke_speed = config.nuke_speed(UnitType::AtomBomb);
        let mut all_available_silos: Vec<AvailableSilo> = Vec::new();
        for &silo in &our_silos {
            let s = st.unit(silo);
            let available_slots = f64::from(s.level()) - s.missile_timer_queue().len() as f64;
            if available_slots <= 0.0 {
                continue;
            }
            let interceptable =
                is_trajectory_interceptable_by_sam(st, player, s.tile(), target_tile, Some(&covering_sam_ids));
            // Compute actual parabolic flight time in ticks
            let mut path_finder = ai_parabola(nuke_speed);
            let trajectory = path_finder.find_path(&st.map, s.tile(), target_tile).unwrap_or_default();
            if trajectory.is_empty() {
                continue;
            }
            all_available_silos.push(AvailableSilo {
                silo,
                slots: available_slots,
                flight_ticks: trajectory.len() as f64,
                interceptable,
            });
        }

        // Sort by Manhattan distance to target (matching nukeSpawn's pick order)
        of_det::js_sort_by(&mut all_available_silos, |a, b| {
            f64::from(st.map.manhattan_dist(st.unit(a.silo).tile(), target_tile))
                - f64::from(st.map.manhattan_dist(st.unit(b.silo).tile(), target_tile))
        }); // TS: NationNukePlanning.ts:280

        // Flatten into a per-bomb launch sequence matching NukeExecution's order.
        // Each silo contributes `slots` consecutive bombs before NukeExecution
        // moves to the next silo.
        let mut launch_sequence: Vec<LaunchSlot> = Vec::new();
        for entry in &all_available_silos {
            let mut s = 0.0;
            while s < entry.slots {
                launch_sequence
                    .push(LaunchSlot { flight_ticks: entry.flight_ticks, interceptable: entry.interceptable });
                s += 1.0;
            }
        }

        let sam_cooldown = config.sam_cooldown();
        let (wait_ticks, bombs_to_fire) = match plan_salvo_window(&launch_sequence, bombs_needed, sam_cooldown) {
            SalvoWindow::Short { total_bombs } => {
                // TS: `failed ??= { targetTile, coveringSamIds, totalBombs }` (`:324`, `:352`).
                if failed.is_none() {
                    failed = Some(FailedSamSalvo { target_tile, covering_sam_ids, total_bombs });
                }
                continue;
            }
            SalvoWindow::Fire { wait_ticks, bombs_to_fire } => (wait_ticks, bombs_to_fire),
        };

        // Check gold for all fired bombs (including wasted ones)
        // TS: `atomCost * BigInt(bombsToFire)` (`:392`).
        let total_cost =
            crate::exec::nation::nuke::big_mul(atom_cost, of_det::bigint::from_number_strict(bombs_to_fire as f64));
        if st.player(player).gold() < total_cost {
            continue;
        }

        // NukeExecution will pick silos in the same Manhattan distance order we planned.
        return (Some(SamSalvo { target_tile, wait_ticks }), failed);
    }

    (None, failed)
}

/// `findEnemySamsCoveringTile(game, player, tile)` (`:407-430`): SAMs hostile to `player`
/// whose range covers `tile`, in `nearbyUnits` order.
pub fn find_enemy_sams_covering_tile(st: &State, player: SmallId, tile: TileRef) -> Vec<UnitRef> {
    let config = st.config();
    let me = st.player(player);
    let mut result = Vec::new();
    for sam in nearby_sams(st, tile, config.max_sam_range()) {
        let owner = st.unit(sam.unit).owner();
        if owner == player || me.is_friendly(owner, false) {
            continue;
        }
        let range = config.sam_range(f64::from(st.unit(sam.unit).level()));
        if sam.dist_squared <= range * range {
            result.push(sam.unit);
        }
    }
    result
}

/// `siloToUpgradeForSalvo(game, player, failed)` (`:438-495`): a missile silo whose upgrade
/// would actually have helped the failed salvo, best protected by our own SAMs.
pub fn silo_to_upgrade_for_salvo(st: &State, player: SmallId, failed: &FailedSamSalvo) -> Option<UnitRef> {
    let me = st.player(player);
    let silos = me.units_of(&[UnitType::MissileSilo]);
    if silos.is_empty() {
        return None;
    }

    // First pass: find silos with an unblocked trajectory to the failed
    // target. Only these contribute slots to the overwhelm plan.
    // "Unblocked" means not interceptable by non-covering enemy SAMs.
    let mut unblocked_silos: Vec<UnitRef> = Vec::new();
    for &silo in &silos {
        if !is_trajectory_interceptable_by_sam(
            st,
            player,
            st.unit(silo).tile(),
            failed.target_tile,
            Some(&failed.covering_sam_ids),
        ) {
            unblocked_silos.push(silo);
        }
    }
    if unblocked_silos.is_empty() {
        return None;
    }

    // Bail out if the target is unreachable even at max silo level —
    // crazy amounts of covering SAMs, upgrading is wasted gold.
    let max_achievable_slots = unblocked_silos.len() as f64 * f64::from(MAX_NATION_SILO_UPGRADE_LEVEL);
    if max_achievable_slots < f64::from(failed.total_bombs) {
        return None;
    }

    let our_sams = me.units_of(&[UnitType::SamLauncher]);
    let config = st.config();
    let mut best_silo: Option<UnitRef> = None;
    let mut best_protection = -1.0;

    for &silo in &unblocked_silos {
        if st.unit(silo).level() >= MAX_NATION_SILO_UPGRADE_LEVEL {
            continue;
        }
        if !me.can_upgrade_unit(silo) {
            continue;
        }

        let mut protection = 0.0;
        for &sam in &our_sams {
            let range = config.sam_range(f64::from(st.unit(sam).level()));
            let dist_squared = d2(st, st.unit(silo).tile(), st.unit(sam).tile());
            if dist_squared <= range * range {
                protection += f64::from(st.unit(sam).level());
            }
        }

        if protection > best_protection {
            best_protection = protection;
            best_silo = Some(silo);
        }
    }

    best_silo
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seq(v: &[(f64, bool)]) -> Vec<LaunchSlot> {
        v.iter().map(|&(f, i)| LaunchSlot { flight_ticks: f, interceptable: i }).collect()
    }

    /// One covering level-1 SAM: bombsNeeded 2, totalBombs 2 (floor(2/5) = 0). Cooldown 75 →
    /// spread 37, stagger max(1, floor(37/2)) = 18. Two silos at flight 40 and 50 → waits
    /// 0 (arrive 40) and 40+18-50 = 8.
    #[test]
    fn window_staggers_arrivals_from_the_earliest_flight() {
        let ls = seq(&[(40.0, false), (50.0, false)]);
        assert_eq!(plan_salvo_window(&ls, 2.0, 75.0), SalvoWindow::Fire { wait_ticks: vec![0, 8], bombs_to_fire: 2 });
    }

    /// Interceptable bombs still fire (they occupy NukeExecution slots) with wait 0.
    #[test]
    fn interceptable_slots_are_wasted_with_zero_wait() {
        let ls = seq(&[(30.0, true), (60.0, false), (61.0, false)]);
        assert_eq!(
            plan_salvo_window(&ls, 2.0, 75.0),
            SalvoWindow::Fire { wait_ticks: vec![0, 0, 17], bombs_to_fire: 3 }
        );
    }

    /// `unblockedBombs.length < totalBombs` and `bestWindowCount < totalBombs` both report the
    /// planned total; 5 levels → 6 needed + floor(6/5) = 7.
    #[test]
    fn short_salvos_report_total_bombs() {
        let ls = seq(&[(10.0, false); 6]);
        assert_eq!(plan_salvo_window(&ls, 6.0, 75.0), SalvoWindow::Short { total_bombs: 7 });
        // Two bombs, but 100 ticks apart with a spread of 37: the best window holds one.
        let ls = seq(&[(10.0, false), (110.0, false)]);
        assert_eq!(plan_salvo_window(&ls, 2.0, 75.0), SalvoWindow::Short { total_bombs: 2 });
    }

    /// The window picks the lowest launch indices inside the densest flight window; bombs before
    /// the last selected index that are outside the window fire wasted (wait 0).
    #[test]
    fn window_prefers_low_launch_indices() {
        // Flights by launch index: 100, 10, 12, 14. Spread 37 → densest window {10,12,14}
        // (indices 1..=3); totalBombs 2 → indices 1 and 2; index 0 fires wasted.
        let ls = seq(&[(100.0, false), (10.0, false), (12.0, false), (14.0, false)]);
        // stagger floor(37/2)=18: arrivals 10 and 28 → waits 0 and 16.
        assert_eq!(
            plan_salvo_window(&ls, 2.0, 75.0),
            SalvoWindow::Fire { wait_ticks: vec![0, 0, 16], bombs_to_fire: 3 }
        );
    }

    /// Ties in flight time keep launch order (V8 TimSort is stable), so the earliest index of
    /// a tied group starts the window.
    #[test]
    fn equal_flights_keep_launch_order() {
        let ls = seq(&[(20.0, false), (20.0, false), (20.0, false)]);
        assert_eq!(
            plan_salvo_window(&ls, 1.0, 10.0),
            // totalBombs 1, spread 5, stagger max(1, floor(5/1)) = 5.
            SalvoWindow::Fire { wait_ticks: vec![0], bombs_to_fire: 1 }
        );
    }
}
