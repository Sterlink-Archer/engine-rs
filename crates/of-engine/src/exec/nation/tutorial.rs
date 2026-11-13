//! `execution/nation/TutorialSilo.ts` + `TutorialCollapse.ts`. Owner: **P12**.
//! `imul(ticks + i, 2654435761) >>> 0` at `TutorialSilo.ts:139` → `of_det::js::imul_f64`.
//!
//! Both are stateless (everything is read off the game each tick, `TutorialSilo.ts:24-25`,
//! `TutorialCollapse.ts:16`), so they are free functions with no CS record.

use crate::exec::missile_silo::MissileSiloExec;
use crate::exec::nuke::NukeExec;
use crate::exec::prelude::*;
use crate::state::State;
use crate::state::unit::UnitParams;
use of_det::js;

/// `TUTORIAL_COLLAPSE_TILES = 250` (`TutorialCollapse.ts:9`).
pub const TUTORIAL_COLLAPSE_TILES: u32 = 250;

/// `STRAIT_X` (`TutorialSilo.ts:12`).
const STRAIT_X: u32 = 142;

/// `PATROL_RADIUS` (`TutorialSilo.ts:96`).
const PATROL_RADIUS: i64 = 6;

/// `game.players().find((p) => p.type() === PlayerType.Human)` (`TutorialSilo.ts:29`,
/// `TutorialCollapse.ts:20`).
fn find_human(st: &State) -> Option<SmallId> {
    st.players().into_iter().find(|&p| st.player(p).player_type() == PlayerType::Human)
}

/// `tutorialSilo(game, nation, fire)` (`TutorialSilo.ts:27-62`).
pub fn tutorial_silo(cx: &mut Ctx, nation: SmallId, fire: bool) {
    let mut fire = fire;
    if !on_players_island(cx.st, nation) {
        return;
    }
    let Some(human) = find_human(cx.st) else {
        return;
    };

    // `nation.units(UnitType.MissileSilo)[0]` (a copy; `undefined` when empty).
    let silo = match cx.st.player(nation).units_of(&[UnitType::MissileSilo]).first().copied() {
        Some(s) => s,
        None => {
            // Built once: a captured silo isn't rebuilt.
            if cx.st.player(nation).units_constructed(UnitType::MissileSilo) > 0 {
                return;
            }
            if !cx.st.player(nation).shares_border_with(human) {
                return;
            }
            let Some(tile) = silo_tile(cx.st, nation) else {
                return;
            };
            top_up_gold(cx, nation, UnitType::MissileSilo);
            let silo = cx.player_mut(nation).build_unit(UnitType::MissileSilo, tile, UnitParams::default());
            cx.st.add_execution(Exec::MissileSilo(MissileSiloExec::new(silo)));
            // The first nuke goes as soon as it's up.
            fire = true;
            silo
        }
    };
    // The empire's harbor goes up with the silo, or once it reaches the shore (`:46-54`).
    for p in cx.st.players() {
        let st: &State = cx.st;
        if st.player(p).player_type() == PlayerType::Nation
            && !on_players_island(st, p)
            && st.player(p).units_constructed(UnitType::Port) == 0
        {
            build_harbor(cx, p, human);
        }
    }

    if !fire || cx.st.unit(silo).is_in_cooldown() {
        return;
    }
    let Some(target) = nuke_target(cx.st, nation, human) else {
        return;
    };
    top_up_gold(cx, nation, UnitType::AtomBomb);
    // TS `canBuild === false` (`:60`): strict.
    if cx.st.player(nation).can_build(cx.sc, UnitType::AtomBomb, target, None).is_none() {
        return;
    }
    // `new NukeExecution(UnitType.AtomBomb, nation, target)`: src undefined, speed -1,
    // waitTicks 0, rocketDirectionUp true (the TS defaults).
    cx.st.add_execution(Exec::Nuke(NukeExec::new(UnitType::AtomBomb, nation, target, Tri::Absent, -1.0, 0, true)));
}

/// `onPlayersIsland(game, nation)` (`TutorialSilo.ts:64-67`).
fn on_players_island(st: &State, nation: SmallId) -> bool {
    match st.player(nation).spawn_tile() {
        Some(spawn) => st.map.x(spawn) < STRAIT_X,
        None => false,
    }
}

/// `buildHarbor(game, nation, human)` (`TutorialSilo.ts:75-93`).
fn build_harbor(cx: &mut Ctx, nation: SmallId, human: SmallId) {
    let Some(from) = cx.st.player(human).spawn_tile() else {
        return;
    };
    let mut port: Option<TileRef> = None;
    let mut port_dist: f64 = -1.0;
    {
        let st: &State = cx.st;
        // iter-live: read-only loop over `nation.borderTiles()` (`:80-85`).
        for t in st.player(nation).border_tiles().iter() {
            let d = f64::from(st.map.euclidean_dist_squared(t, from));
            if d <= port_dist || !st.map.is_shore(t) {
                continue;
            }
            port = Some(t);
            port_dist = d;
        }
    }
    let Some(port) = port else {
        return;
    };
    let Some(dock) = cx.st.map.neighbors(port).into_iter().find(|&n| cx.st.map.is_ocean(n)) else {
        return;
    };
    top_up_gold(cx, nation, UnitType::Port);
    cx.player_mut(nation).build_unit(UnitType::Port, port, UnitParams::default());
    top_up_gold(cx, nation, UnitType::Warship);
    cx.player_mut(nation).build_unit(
        UnitType::Warship,
        dock,
        UnitParams { patrol_tile: Some(dock), ..Default::default() },
    );
}

/// `tutorialWarship(game, nation, ticks)` (`TutorialSilo.ts:103-130`).
pub fn tutorial_warship(cx: &mut Ctx, nation: SmallId, ticks: i32) {
    // int: ticks are non-negative, so JS `%` equals Rust `%`.
    if ticks % 2 != 0 {
        return;
    }
    let Some(ship) = cx.st.player(nation).units_of(&[UnitType::Warship]).first().copied() else {
        return;
    };
    // `ship?.warshipState().patrolTile`: the mutating getter runs only when a ship exists.
    let Some(home) = cx.unit_mut(ship).warship_state().patrol_tile else {
        return;
    };
    let mut target = cx.st.unit(ship).target_tile();
    let ship_tile = cx.st.unit(ship).tile();
    if target.is_none() || target == Some(ship_tile) {
        let Some(point) = patrol_point(cx.st, home, ticks) else {
            return;
        };
        target = Some(point);
        cx.unit_mut(ship).set_target_tile(target);
    }
    let target = target.ts_expect("TutorialSilo.ts:108");
    // The ocean neighbor nearest the target; if none is nearer, pick another.
    let st: &State = cx.st;
    let mut next: Option<TileRef> = None;
    let mut best = st.map.euclidean_dist_squared(ship_tile, target);
    for n in st.map.neighbors(ship_tile) {
        if !st.map.is_ocean(n) {
            continue;
        }
        let d = st.map.euclidean_dist_squared(n, target);
        if d >= best {
            continue;
        }
        next = Some(n);
        best = d;
    }
    let Some(next) = next else {
        cx.unit_mut(ship).set_target_tile(None);
        return;
    };
    cx.unit_mut(ship).move_to(next);
}

/// The `(dx, dy)` candidate of `patrolPoint` try `i` (`TutorialSilo.ts:139-141`):
/// `h = Math.imul(ticks + i, 2654435761) >>> 0`, `dx = (h % span) - R`,
/// `dy = ((h >>> 8) % span) - R`.
fn patrol_offset(ticks: i32, i: i32) -> (i64, i64) {
    let span = (2 * PATROL_RADIUS + 1) as u32; // int: 13
    // TS: `ticks + i` is a number; `Math.imul` applies ToInt32 to both operands.
    let h = js::to_uint32(f64::from(js::imul_f64(f64::from(ticks) + f64::from(i), 2654435761.0)));
    let dx = i64::from(h % span) - PATROL_RADIUS;
    let dy = i64::from((h >> 8) % span) - PATROL_RADIUS;
    (dx, dy)
}

/// `patrolPoint(game, home, ticks)` (`TutorialSilo.ts:136-150`).
fn patrol_point(st: &State, home: TileRef, ticks: i32) -> Option<TileRef> {
    for i in 0..10 {
        let (dx, dy) = patrol_offset(ticks, i);
        if dx * dx + dy * dy > PATROL_RADIUS * PATROL_RADIUS {
            continue;
        }
        let x = f64::from(st.map.x(home)) + dx as f64;
        let y = f64::from(st.map.y(home)) + dy as f64;
        if !st.map.is_valid_coord(x, y) {
            continue;
        }
        let t = st.map.ref_(x, y);
        if st.map.is_ocean(t) {
            return Some(t);
        }
    }
    None
}

/// `topUpGold(game, nation, type)` (`TutorialSilo.ts:153-156`): gives the nation what it lacks.
fn top_up_gold(cx: &mut Ctx, nation: SmallId, t: UnitType) {
    let cost = {
        let st: &State = cx.st;
        st.config().unit_cost(t, st, nation, 0.0)
    };
    let gold = cx.st.player(nation).gold();
    if gold < cost {
        let missing = cost - cx.st.player(nation).gold();
        cx.player_mut(nation).add_gold(missing, None);
    }
}

/// `siloTile(game, nation)` (`TutorialSilo.ts:162-174`): the nation's tile nearest its spawn.
fn silo_tile(st: &State, nation: SmallId) -> Option<TileRef> {
    let from = st.player(nation).spawn_tile()?;
    let mut best: Option<TileRef> = None;
    let mut best_dist = f64::INFINITY;
    // iter-live: read-only loop over `nation.tiles()`.
    for t in st.player(nation).tiles().iter() {
        let d = f64::from(st.map.euclidean_dist_squared(t, from));
        if d >= best_dist {
            continue;
        }
        best = Some(t);
        best_dist = d;
    }
    best
}

/// `nukeTarget(game, nation, human)` (`TutorialSilo.ts:180-201`).
fn nuke_target(st: &State, nation: SmallId, human: SmallId) -> Option<TileRef> {
    let outer = st.config().nuke_magnitudes(UnitType::AtomBomb).outer;
    let clear = (outer + 1.0) * (outer + 1.0);
    let border: Vec<TileRef> = st.player(nation).border_tiles().to_vec();
    let mut sum_x = 0.0;
    let mut sum_y = 0.0;
    for t in st.player(human).tiles().iter() {
        sum_x += f64::from(st.map.x(t));
        sum_y += f64::from(st.map.y(t));
    }
    let owned = f64::from(st.player(human).num_tiles_owned());
    let cx = sum_x / owned;
    let cy = sum_y / owned;
    let from_middle = |t: TileRef| {
        let x = f64::from(st.map.x(t));
        let y = f64::from(st.map.y(t));
        (x - cx) * (x - cx) + (y - cy) * (y - cy)
    };
    let mut tiles: Vec<TileRef> = st.player(human).tiles().to_vec();
    // `(a, b) => fromMiddle(a) - fromMiddle(b)` (`:194-196`; NaN for an empty human, V8 TimSort).
    of_det::js_sort_by(&mut tiles, |&a, &b| from_middle(a) - from_middle(b));
    tiles.into_iter().find(|&t| border.iter().all(|&b| f64::from(st.map.euclidean_dist_squared(t, b)) > clear))
}

/// `tutorialCollapse(game, nation)` (`TutorialCollapse.ts:17-24`).
pub fn tutorial_collapse(cx: &mut Ctx, nation: SmallId) {
    if cx.st.player(nation).num_tiles_owned() >= TUTORIAL_COLLAPSE_TILES {
        return;
    }
    let Some(human) = find_human(cx.st) else {
        return;
    };
    let attacked = {
        let st: &State = cx.st;
        st.player(nation).incoming_attacks().into_iter().any(|a| st.attack(a).attacker() == human)
    };
    if !attacked {
        return;
    }
    cx.conquer_player(human, nation);
    // `[...nation.tiles()]`: a snapshot, since `conquer` removes from the set.
    for tile in cx.st.player(nation).tiles().to_vec() {
        cx.player_mut(human).conquer(tile);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference values computed in node for `TutorialSilo.ts:139-141`:
    /// `h = Math.imul(t + i, 2654435761) >>> 0; [h % 13 - 6, (h >>> 8) % 13 - 6]`.
    #[test]
    fn patrol_offsets_match_node() {
        // node v-current, `node -e` (2026-11-05); cross-checked with Python big-int arithmetic.
        let cases: &[(i32, i32, i64, i64)] = &[
            (0, 0, -6, -6),
            (0, 1, -3, 5),
            (2, 0, 4, 3),
            (100, 3, -4, 4),
            (2_000_000, 9, 3, -1),
            (7, 4, -1, -1),
            (123_456, 2, 1, -3),
        ];
        for &(t, i, dx, dy) in cases {
            assert_eq!(patrol_offset(t, i), (dx, dy), "ticks={t} i={i}");
        }
    }
}
