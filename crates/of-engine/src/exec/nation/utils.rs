//! `execution/nation/NationUtils.ts`. Owner: **P10**.

use crate::api::types::{Cell, Difficulty, GameMode, unit_groups};
#[allow(unused_imports)]
use crate::exec::prelude::*;
use crate::state::State;
use crate::state::player::BoundingBox;
use of_det::js;

/// `HIGH_STARTING_GOLD = 3_000_000n` (`:13`).
const HIGH_STARTING_GOLD: Gold = 3_000_000;

/// `hasHighStartingGold(game, player)` (`:15-17`).
pub fn has_high_starting_gold(st: &State, player: SmallId) -> bool {
    st.config().starting_gold(st.player(player).info()) >= HIGH_STARTING_GOLD
}

/// `SAVE_UP_HORIZON_MINUTES` (`:20-25`).
fn save_up_horizon_minutes(d: Difficulty) -> f64 {
    match d {
        Difficulty::Easy => f64::INFINITY,
        Difficulty::Medium => f64::INFINITY,
        Difficulty::Hard => 5.0,
        Difficulty::Impossible => 6.0,
    }
}

/// `isSaveUpWithinReach(game, player, target)` (`:28-41`).
pub fn is_save_up_within_reach(st: &State, player: SmallId, target: Gold) -> bool {
    let horizon = save_up_horizon_minutes(st.config().game_config().difficulty);
    // TS: `Number(target - player.gold())` (bigint subtraction, then Number).
    let missing = (target - st.player(player).gold()) as f64;
    if horizon == f64::INFINITY || missing <= 0.0 {
        return true;
    }
    // TS :38 `Math.max(1, game.ticks() / 600)`.
    let minutes = js::max(1.0, f64::from(st.ticks()) / 600.0);
    let income_per_minute = (2.0 * (st.player(player).gold_earned() as f64)) / minutes;
    missing <= income_per_minute * horizon
}

/// `randTerritoryTileArray(random, mg, player, numTiles)` (`:43-58`).
pub fn rand_territory_tile_array(random: &mut Prng, st: &State, player: SmallId, num_tiles: u32) -> Vec<TileRef> {
    // TS :49 `calculateBoundingBox(mg, player.borderTiles())`.
    let bounding_box = crate::util::calculate_bounding_box(&st.map, st.player(player).border_tiles().iter());
    let mut tiles = Vec::new();
    for _ in 0..num_tiles {
        let tile = rand_territory_tile(random, st, player, &bounding_box);
        if let Some(t) = tile {
            tiles.push(t);
        }
    }
    tiles
}

/// `randTerritoryTile(random, mg, p, boundingBox)` (`:60-86`). The only caller passes a box, so
/// the `boundingBox ??= ...` default (`:67`) is never taken.
fn rand_territory_tile(random: &mut Prng, st: &State, p: SmallId, bounding_box: &BoundingBox) -> Option<TileRef> {
    for _ in 0..100 {
        let rand_x = random.next_int(bounding_box.min.x, bounding_box.max.x);
        let rand_y = random.next_int(bounding_box.min.y, bounding_box.max.y);
        // TS :71 `mg.isOnMap(new Cell(randX, randY))`; an empty border gives NaN coordinates.
        if !st.is_on_map(Cell::new(rand_x, rand_y)) {
            continue;
        }
        let rand_tile = st.map.ref_(rand_x, rand_y);
        if st.owner(rand_tile) == p {
            return Some(rand_tile);
        }
    }

    let n = st.player(p).num_tiles_owned();
    if n > 0 && n <= 100 {
        // TS :82 `random.randElement(Array.from(p.tiles()))`.
        let all = st.player(p).tiles().to_vec();
        return Some(rand_elem(random, &all));
    }

    None
}

/// `random.randElement(arr)` (`PseudoRandom.ts:80-85`): throws on an empty array.
#[cfg_attr(feature = "prng-trace", track_caller)]
pub(crate) fn rand_elem<T: Copy>(random: &mut Prng, arr: &[T]) -> T {
    match random.rand_element(arr) {
        Ok(x) => *x,
        Err(_) => of_det::ts_throw!("array must not be empty"),
    }
}

/// `normalize(value, values)` inside `findJuiciestTarget` (`:119-123`).
fn normalize(value: f64, values: &[f64]) -> f64 {
    let min = js::min_n(values);
    let max = js::max_n(values);
    if max > min { (value - min) / (max - min) } else { 0.0 }
}

/// `findJuiciestTarget(game, candidates)` (`:89-141`; `Math.max(...arr)` at `:120-121`).
pub fn find_juiciest_target(st: &State, candidates: &[SmallId]) -> Option<SmallId> {
    if candidates.is_empty() {
        return None;
    }

    struct Stat {
        player: SmallId,
        structure_count: f64,
        troop_gap_ratio: f64,
        tiles: f64,
    }
    let stats: Vec<Stat> = candidates
        .iter()
        .map(|&p| {
            // TS :98-108: `p.units().reduce(...)` with initial value 0, units in list order.
            let mut structure_count = 0.0;
            for &u in st.player(p).units().as_slice() {
                let ut = st.unit(u).unit_type();
                if unit_groups::has(unit_groups::STRUCTURES, ut)
                    && ut != UnitType::DefensePost
                    && ut != UnitType::MissileSilo
                {
                    structure_count += f64::from(st.unit(u).level());
                }
            }
            let max_troops = st.config().max_troops(&st.player(p));
            let troop_gap_ratio = if max_troops > 0.0 { 1.0 - st.player(p).troops() / max_troops } else { 0.0 };
            Stat { player: p, structure_count, troop_gap_ratio, tiles: f64::from(st.player(p).num_tiles_owned()) }
        })
        .collect();

    let structure_counts: Vec<f64> = stats.iter().map(|s| s.structure_count).collect();
    let troop_gap_ratios: Vec<f64> = stats.iter().map(|s| s.troop_gap_ratio).collect();
    let tile_counts: Vec<f64> = stats.iter().map(|s| s.tiles).collect();

    let mut best: Option<SmallId> = None;
    let mut best_score = f64::NEG_INFINITY;
    for s in &stats {
        let juiciness = normalize(s.structure_count, &structure_counts)
            + normalize(s.troop_gap_ratio, &troop_gap_ratios)
            + normalize(s.tiles, &tile_counts);
        if juiciness > best_score {
            best_score = juiciness;
            best = Some(s.player);
        }
    }
    best
}

/// `runawayLeadFactor(difficulty)` (`:145-158`).
fn runaway_lead_factor(difficulty: Difficulty) -> Option<f64> {
    match difficulty {
        Difficulty::Easy => None,
        Difficulty::Medium => Some(3.0),
        Difficulty::Hard => Some(2.0),
        Difficulty::Impossible => Some(1.5),
    }
}

/// `findRunawayLeader(game)` (`:162-186`).
pub fn find_runaway_leader(st: &State) -> Option<SmallId> {
    let gc = st.config().game_config();
    if gc.game_mode != GameMode::Ffa {
        return None;
    }
    let factor = runaway_lead_factor(gc.difficulty)?;

    let mut leader: Option<SmallId> = None;
    let mut runner_up: Option<SmallId> = None;
    let tiles = |p: SmallId| st.player(p).num_tiles_owned();
    for p in st.players() {
        if st.player(p).player_type() == PlayerType::Bot {
            continue;
        }
        let beats_leader = match leader {
            None => true,
            Some(l) => tiles(p) > tiles(l),
        };
        if beats_leader {
            runner_up = leader;
            leader = Some(p);
        } else if match runner_up {
            None => true,
            Some(r) => tiles(p) > tiles(r),
        } {
            runner_up = Some(p);
        }
    }
    let (leader, runner_up) = (leader?, runner_up?);
    if f64::from(tiles(leader)) >= f64::from(tiles(runner_up)) * factor { Some(leader) } else { None }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // tests may catch panics to assert them
mod tests {
    use super::*;

    #[test]
    fn normalize_matches_ts() {
        // TS: max > min ? (value - min) / (max - min) : 0
        assert_eq!(normalize(5.0, &[0.0, 5.0, 10.0]), 0.5);
        assert_eq!(normalize(3.0, &[3.0, 3.0]), 0.0);
        // NaN anywhere: Math.min/max give NaN, `NaN > NaN` is false -> 0
        assert_eq!(normalize(1.0, &[1.0, f64::NAN]), 0.0);
        // single value: 0
        assert_eq!(normalize(7.0, &[7.0]), 0.0);
    }

    #[test]
    fn rand_elem_draws_like_rand_element() {
        let mut a = Prng::new(42.0);
        let mut b = Prng::new(42.0);
        let xs = [10u32, 20, 30, 40];
        for _ in 0..50 {
            let i = b.next_int(0.0, 4.0) as usize;
            assert_eq!(rand_elem(&mut a, &xs), xs[i]);
        }
    }

    #[test]
    fn rand_elem_empty_is_ts_throw() {
        let mut a = Prng::new(1.0);
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rand_elem::<u32>(&mut a, &[])));
        let p = match r {
            Ok(_) => panic!("must throw"),
            Err(p) => p,
        };
        assert!(of_det::throw::is_ts_throw(&*p));
        // no draw consumed (TS throws before nextInt)
        assert_eq!(a.get_state(), Prng::new(1.0).get_state());
    }

    #[test]
    fn save_up_horizons() {
        assert_eq!(save_up_horizon_minutes(Difficulty::Easy), f64::INFINITY);
        assert_eq!(save_up_horizon_minutes(Difficulty::Medium), f64::INFINITY);
        assert_eq!(save_up_horizon_minutes(Difficulty::Hard), 5.0);
        assert_eq!(save_up_horizon_minutes(Difficulty::Impossible), 6.0);
        assert_eq!(runaway_lead_factor(Difficulty::Easy), None);
        assert_eq!(runaway_lead_factor(Difficulty::Impossible), Some(1.5));
    }
}
