//! `NationStructurePlacement` (`execution/nation/NationStructurePlacement.ts`). Owner: **P12**.
//! Both caches are reset by `startRound` and only read until the next one (`:29-31`).
//!
//! The TS value functions are closures over values captured when the function is built
//! (`structureSpawnTileValue`, `:112-129`). They are ported as [`ValueFn`], an enum carrying
//! the captured data, with [`ValueFn::eval`] as the closure body (PORTING_GUIDE §7). Building
//! one consumes PRNG draws exactly where TS does (`shouldUseConnectivityScore`, `:264-284`;
//! the SAM coverage roll, `:490-491`).

use crate::api::types::Difficulty;
use crate::config::engine_config::TrainRelation;
use crate::ctx::ts_throw;
use crate::exec::nation::NationExec;
use crate::exec::nation::structure::{can_build_truthy, difficulty, me};
use crate::exec::nation::utils::rand_territory_tile_array;
use crate::exec::prelude::*;
use crate::exec::util::{nearest_tile_dist, nearest_tile_dist_capped};
use crate::ids::ClusterRef;
use crate::jsorder::LookupMap;
use crate::state::State;
use of_det::js;

/// `MIN_PORT_WATER_COMPONENT_SIZE` (`:19`).
const MIN_PORT_WATER_COMPONENT_SIZE: u32 = 3000;

/// `ReachableStation` (`:21-25`).
#[derive(StateClone, Debug, Default, PartialEq)]
pub struct ReachableStation {
    pub tile: TileRef,
    pub cluster: Option<ClusterRef>,
    pub weight: f64,
}

/// `NationStructurePlacement` data.
#[derive(StateClone, Debug, Default)]
pub struct StructurePlacement {
    pub reachable_stations_cache: Option<Vec<ReachableStation>>,
    /// `_sharedWaterComponents: Set<number> | null`.
    pub shared_water_components: Option<JsSet<i32>>,
}

/// `spacingConstants()` (`:556-564`).
#[derive(Clone, Copy, Debug)]
struct Spacing {
    border_spacing: f64,
    structure_spacing: f64,
}

fn spacing_constants(st: &State) -> Spacing {
    let border_spacing = st.config().nuke_magnitudes(UnitType::AtomBomb).outer;
    Spacing { border_spacing, structure_spacing: border_spacing * 2.0 }
}

/// One `{ tile, weight }` of `samLauncherValue`'s `protectEntries` (`:474`).
#[derive(Clone, Copy, Debug)]
struct ProtectEntry {
    tile: TileRef,
    weight: f64,
}

/// The value-function closures of `structureSpawnTileValue` (`:112-129`) with their captures.
enum ValueFn {
    /// `missileSiloValue()` (`:135-165`).
    MissileSilo { other_units: Vec<UnitRef>, sp: Spacing },
    /// `portValue()` (`:171-186`).
    Port { other_units: Vec<UnitRef> },
    /// `factoryValue()` (`:197-258`): `otherUnits` = factories, `cross_tiles` = city tiles.
    /// `cityValue()` (`:398-456`): `otherUnits` = cities, `cross_tiles` = factory tiles. The two
    /// closures differ only in the clamp of the same-type term (`stationRange` for factories,
    /// `structureSpacing` for cities; `:238` vs `:436`).
    Interior {
        is_factory: bool,
        other_units: Vec<UnitRef>,
        cross_tiles: Vec<TileRef>,
        sp: Spacing,
        station_range: f64,
        station_range_squared: f64,
        use_connection_score: bool,
        min_range_squared: f64,
    },
    /// `samLauncherValue()` (`:463-553`).
    SamLauncher {
        other_units: Vec<UnitRef>,
        sp: Spacing,
        difficulty: Difficulty,
        protect_entries: Vec<ProtectEntry>,
        range_squared: f64,
        use_coverage_weighting: bool,
        /// `structureCoverage: Map<TileRef, number> | null`; lookup-only: `set`/`get` only
        /// (`:507`, `:542`).
        structure_coverage: Option<LookupMap<TileRef, f64>>,
    },
}

/// `new Set(otherUnits.map(u => u.tile()))` then `.delete(tile)` (`:158-159` and siblings):
/// the members feed only `nearestTileDist`, a min over integer distances, so order and
/// duplicates do not matter; the filter is the `delete`.
fn other_tiles<'a>(st: &'a State, other_units: &'a [UnitRef], tile: TileRef) -> impl Iterator<Item = TileRef> + 'a {
    other_units.iter().map(move |&u| st.unit(u).tile()).filter(move |&t| t != tile)
}

impl ValueFn {
    /// The closure body: `(tile) => number`.
    fn eval(&self, st: &State, player: SmallId, reachable: &[ReachableStation], tile: TileRef) -> f64 {
        let gm = &st.map;
        let border_tiles = st.player(player).border_tiles();
        match self {
            ValueFn::MissileSilo { other_units, sp } => {
                let mut w = 0.0;
                // Prefer higher elevations.
                w += f64::from(gm.magnitude(tile));
                // Prefer to be away from the border (clamped at borderSpacing).
                let closest_border_dist = nearest_tile_dist_capped(gm, border_tiles.iter(), tile, sp.border_spacing);
                w += js::min(closest_border_dist, sp.border_spacing);
                // Prefer to be away from other structures of the same type.
                let d = nearest_tile_dist(gm, other_tiles(st, other_units, tile), tile);
                if d != f64::INFINITY {
                    w += js::min(d, sp.structure_spacing);
                }
                w
            }
            ValueFn::Port { other_units } => {
                let mut w = 0.0;
                // Prefer to be as far as possible from other ports.
                let closest_other_dist = nearest_tile_dist(gm, other_tiles(st, other_units, tile), tile);
                w += closest_other_dist;
                w
            }
            ValueFn::Interior {
                is_factory,
                other_units,
                cross_tiles,
                sp,
                station_range,
                station_range_squared,
                use_connection_score,
                min_range_squared,
            } => {
                let mut w = 0.0;
                w += f64::from(gm.magnitude(tile));

                let closest_border_dist = nearest_tile_dist_capped(gm, border_tiles.iter(), tile, sp.border_spacing);
                w += js::min(closest_border_dist, sp.border_spacing);

                let d = nearest_tile_dist(gm, other_tiles(st, other_units, tile), tile);
                if d != f64::INFINITY {
                    // TS: factories clamp at stationRange (:238), cities at structureSpacing (:436).
                    w += js::min(d, if *is_factory { *station_range } else { sp.structure_spacing });
                }

                // Cross-type spacing (cities for a factory, factories for a city).
                let d2 = nearest_tile_dist(gm, cross_tiles.iter().copied(), tile);
                if d2 != f64::INFINITY {
                    w += js::min(d2, sp.structure_spacing);
                }

                if !*use_connection_score {
                    return w;
                }

                w += compute_connectivity_score(st, tile, reachable, *min_range_squared, *station_range_squared)
                    * sp.structure_spacing;
                w
            }
            ValueFn::SamLauncher {
                other_units,
                sp,
                difficulty,
                protect_entries,
                range_squared,
                use_coverage_weighting,
                structure_coverage,
            } => {
                let mut w = 0.0;
                w += f64::from(gm.magnitude(tile));

                let closest_border_dist = nearest_tile_dist_capped(gm, border_tiles.iter(), tile, sp.border_spacing);
                // Only an empty border set contributes nothing (`:524-528`).
                if border_tiles.size() > 0 {
                    w += js::min(closest_border_dist, sp.border_spacing);
                }

                let d = nearest_tile_dist(gm, other_tiles(st, other_units, tile), tile);
                if d != f64::INFINITY {
                    w += js::min(d, sp.structure_spacing);
                }

                // Prefer to be in range of other structures (skip on easy difficulty).
                if *difficulty != Difficulty::Easy {
                    for entry in protect_entries {
                        let distance_squared = f64::from(gm.euclidean_dist_squared(tile, entry.tile));
                        if distance_squared > *range_squared {
                            continue;
                        }
                        match structure_coverage {
                            Some(cov) if *use_coverage_weighting => {
                                let coverage = cov.get(&entry.tile).copied().unwrap_or(0.0);
                                let coverage_weight = 1.0 / (1.0 + coverage);
                                w += sp.structure_spacing * entry.weight * coverage_weight;
                            }
                            _ => {
                                w += sp.structure_spacing * entry.weight;
                            }
                        }
                    }
                }
                w
            }
        }
    }
}

/// `computeConnectivityScore(tile, reachableStations, minRangeSquared, stationRangeSquared)`
/// (`:367-390`). `clustersInRange` is iterated (`.values()`, a float sum), so it is a
/// `JsMap` in insertion order.
fn compute_connectivity_score(
    st: &State,
    tile: TileRef,
    reachable_stations: &[ReachableStation],
    min_range_squared: f64,
    station_range_squared: f64,
) -> f64 {
    let mut clusters_in_range: JsMap<ClusterRef, f64> = JsMap::new();
    let mut isolated_weight = 0.0;
    for rs in reachable_stations {
        let dist = f64::from(st.map.euclidean_dist_squared(tile, rs.tile));
        if dist < min_range_squared || dist > station_range_squared {
            continue;
        }
        if let Some(cluster) = rs.cluster {
            let prev = clusters_in_range.get(&cluster).copied().unwrap_or(0.0);
            clusters_in_range.insert(cluster, js::max(prev, rs.weight));
        } else {
            isolated_weight += rs.weight;
        }
    }
    let mut score = isolated_weight;
    for (_, &cw) in clusters_in_range.iter() {
        score += cw;
    }
    score
}

/// `arraySampler(a, sampleSize)` (`:97-110`) collected by `Array.from`. `remaining` is a
/// `Set` built from `a` (whose elements are distinct border tiles) and drawn with
/// `randFromSet` = an index into insertion order; `Vec::remove` keeps that order.
fn array_sampler(rng: &mut Prng, a: Vec<TileRef>, sample_size: usize) -> Vec<TileRef> {
    if a.len() <= sample_size {
        // Return all elements (no draw).
        return a;
    }
    let mut remaining = a;
    let mut out = Vec::with_capacity(sample_size);
    for _ in 0..sample_size {
        let i = match rng.rand_index(remaining.len()) {
            Ok(i) => i,
            Err(_) => ts_throw!("set must not be empty"),
        };
        out.push(remaining.remove(i));
    }
    out
}

/// `shouldUseConnectivityScore(difficulty)` (`:264-284`): always one draw (Easy included).
fn should_use_connectivity_score(rng: &mut Prng, difficulty: Difficulty) -> bool {
    let random_chance = match difficulty {
        Difficulty::Easy => 0.0,
        Difficulty::Medium => 60.0,
        Difficulty::Hard => 75.0,
        Difficulty::Impossible => 100.0,
    };
    rng.next_int(0.0, 100.0) < random_chance
}

impl NationExec {
    /// `placement.startRound()` (`:40-44`).
    pub fn placement_start_round(&mut self, cx: &mut Ctx) {
        self.b().structure.placement.reachable_stations_cache = None;
        let shared = cx.shared_water_components(me(self));
        self.b().structure.placement.shared_water_components = shared.map(|v| v.into_iter().collect());
    }
    /// `placement.hasCoastalTiles()` (`:46-48`).
    pub fn placement_has_coastal_tiles(&mut self, _cx: &mut Ctx) -> bool {
        self.b().structure.placement.shared_water_components.is_some()
    }
    /// `placement.spawnTile(type)` (`:50-69`).
    pub fn placement_spawn_tile(&mut self, cx: &mut Ctx, t: UnitType) -> Option<TileRef> {
        let player = me(self);
        let tiles = if t == UnitType::Port {
            self.placement_rand_coastal_tile_array(cx, 25)
        } else {
            rand_territory_tile_array(&mut self.rng, cx.st, player, 25)
        };
        if tiles.is_empty() {
            return None;
        }
        // TS `if (valueFunction === null) return null` (:57) is unreachable: the switch throws.
        let value_function = self.placement_structure_spawn_tile_value(cx, t);
        let mut best_tile: Option<TileRef> = None;
        let mut best_value = 0.0;
        for &tile in &tiles {
            let v = {
                let reachable = self.b().structure.placement.reachable_stations_cache.as_deref().unwrap_or(&[]);
                value_function.eval(cx.st, player, reachable, tile)
            };
            if v <= best_value && best_tile.is_some() {
                continue;
            }
            // TS `if (!this.player.canBuild(type, t)) continue` (:63): tile 0 is falsy.
            if !can_build_truthy(cx.st.player(player).can_build(cx.sc, t, tile, None)) {
                continue;
            }
            // Found a better tile.
            best_tile = Some(tile);
            best_value = v;
        }
        best_tile
    }

    /// `randCoastalTileArray(numTiles)` (`:72-95`).
    fn placement_rand_coastal_tile_array(&mut self, cx: &mut Ctx, num_tiles: usize) -> Vec<TileRef> {
        let player = me(self);
        // `Array.from(this.player.borderTiles())` (a snapshot), then `filter` (:74-93).
        let border: Vec<TileRef> = cx.st.player(player).border_tiles().to_vec();
        let difficulty = difficulty(cx.st);
        let mut tiles = Vec::new();
        for t in border {
            if self.placement_is_coastal_candidate(cx, t, difficulty) {
                tiles.push(t);
            }
        }
        array_sampler(&mut self.rng, tiles, num_tiles)
    }

    /// The `filter` callback of `randCoastalTileArray` (`:74-93`).
    fn placement_is_coastal_candidate(&mut self, cx: &mut Ctx, t: TileRef, difficulty: Difficulty) -> bool {
        if !cx.st.map.is_shore(t) {
            return false;
        }
        let Some(shared) = self.b().structure.placement.shared_water_components.as_ref() else {
            return false;
        };
        for neighbor in cx.st.map.neighbors(t) {
            if !cx.st.map.is_water(neighbor) {
                continue;
            }
            // Ocean is always considered shared.
            if cx.st.map.is_ocean(neighbor) {
                return true;
            }
            let comp = cx.rcx().get_water_component(neighbor);
            // int: component ids are small non-negative numbers; the set also holds -1 (ocean).
            match comp {
                Some(c) if shared.contains(&(c as i32)) => {}
                _ => continue,
            }
            // Skip tiny lakes (not on Easy).
            if difficulty != Difficulty::Easy {
                let size = cx.rcx().get_water_component_size(neighbor);
                if let Some(size) = size
                    && size < MIN_PORT_WATER_COMPONENT_SIZE
                {
                    continue;
                }
            }
            return true;
        }
        false
    }

    /// `structureSpawnTileValue(type)` (`:112-129`).
    fn placement_structure_spawn_tile_value(&mut self, cx: &mut Ctx, t: UnitType) -> ValueFn {
        match t {
            UnitType::City => self.placement_interior_value(cx, false),
            UnitType::MissileSilo => self.placement_missile_silo_value(cx),
            UnitType::Factory => self.placement_interior_value(cx, true),
            UnitType::Port => self.placement_port_value(cx),
            UnitType::SamLauncher => self.placement_sam_launcher_value(cx),
            _ => ts_throw!("Value function not implemented for {}", t.as_str()),
        }
    }

    /// `missileSiloValue()` (`:135-165`).
    fn placement_missile_silo_value(&mut self, cx: &mut Ctx) -> ValueFn {
        let other_units = cx.st.player(me(self)).units_of(&[UnitType::MissileSilo]);
        let sp = spacing_constants(cx.st);
        ValueFn::MissileSilo { other_units, sp }
    }

    /// `portValue()` (`:171-186`).
    fn placement_port_value(&mut self, cx: &mut Ctx) -> ValueFn {
        let other_units = cx.st.player(me(self)).units_of(&[UnitType::Port]);
        ValueFn::Port { other_units }
    }

    /// `factoryValue()` (`:197-258`) when `is_factory`, else `cityValue()` (`:398-456`). The
    /// capture order (and so the PRNG draw in `shouldUseConnectivityScore`, then the station
    /// cache build) is the same in both.
    fn placement_interior_value(&mut self, cx: &mut Ctx, is_factory: bool) -> ValueFn {
        let player = me(self);
        let (own, cross) =
            if is_factory { (UnitType::Factory, UnitType::City) } else { (UnitType::City, UnitType::Factory) };
        let other_units = cx.st.player(player).units_of(&[own]);
        let sp = spacing_constants(cx.st);
        let station_range = cx.st.config().train_station_max_range();
        let station_range_squared = station_range * station_range;
        let difficulty = difficulty(cx.st);
        let use_connection_score = should_use_connectivity_score(&mut self.rng, difficulty);

        if use_connection_score {
            self.placement_get_or_build_reachable_stations(cx);
        }
        let min_range = cx.st.config().train_station_min_range();
        let min_range_squared = min_range * min_range; // TS `** 2`

        // Cross-type spacing tiles (`new Set(player.units(T).map(u => u.tile()))`): min-only use.
        let st: &State = cx.st;
        let cross_tiles: Vec<TileRef> =
            st.player(player).units_of(&[cross]).into_iter().map(|u| st.unit(u).tile()).collect();

        ValueFn::Interior {
            is_factory,
            other_units,
            cross_tiles,
            sp,
            station_range,
            station_range_squared,
            use_connection_score,
            min_range_squared,
        }
    }

    /// `getOrBuildReachableStations()` (`:286-289`): `??=`.
    fn placement_get_or_build_reachable_stations(&mut self, cx: &mut Ctx) {
        if self.b().structure.placement.reachable_stations_cache.is_none() {
            let built = self.placement_build_reachable_stations(cx);
            self.b().structure.placement.reachable_stations_cache = Some(built);
        }
    }

    /// `buildReachableStations()` (`:297-359`).
    fn placement_build_reachable_stations(&mut self, cx: &mut Ctx) -> Vec<ReachableStation> {
        let player = me(self);
        let st: &State = cx.st;
        let config = st.config();

        // Build unit → cluster lookup in one pass. lookup-only: `has`/`get` (:322-325, :348-351).
        let sm = &st.rail.station_manager;
        let mut unit_to_cluster: LookupMap<UnitRef, Option<ClusterRef>> = LookupMap::new();
        for &sid in sm.get_all().iter() {
            let station = sm.get(sid).ts_expect("NationStructurePlacement.ts:304 station");
            unit_to_cluster.insert(station.unit, station.get_cluster());
        }

        let pr = st.player(player);
        let max_trade_gold = js::max(config.train_gold(TrainRelation::Ally, 0.0, &pr) as f64, 1.0);
        let mut result = Vec::new();

        // Own structures, weighted by "self" trade gold.
        let self_weight = config.train_gold(TrainRelation::SelfOwned, 0.0, &pr) as f64 / max_trade_gold;
        for unit in pr.units_of(&[UnitType::City, UnitType::Port, UnitType::Factory]) {
            if let Some(&cluster) = unit_to_cluster.get(&unit) {
                result.push(ReachableStation { tile: st.unit(unit).tile(), cluster, weight: self_weight });
            }
        }

        // Neighbor structures: all non-embargoed non-bot neighbors.
        for neighbor in pr.nearby() {
            if !neighbor.is_player() {
                continue;
            }
            if st.player(neighbor).player_type() == PlayerType::Bot {
                continue;
            }
            if !pr.can_trade(neighbor) {
                continue;
            }
            let rel_type = if pr.is_on_same_team(neighbor) {
                TrainRelation::Team
            } else if pr.is_allied_with(neighbor) {
                TrainRelation::Ally
            } else {
                TrainRelation::Other
            };
            let weight = config.train_gold(rel_type, 0.0, &pr) as f64 / max_trade_gold;
            for unit in st.player(neighbor).units_of(&[UnitType::City, UnitType::Port, UnitType::Factory]) {
                if let Some(&cluster) = unit_to_cluster.get(&unit) {
                    result.push(ReachableStation { tile: st.unit(unit).tile(), cluster, weight });
                }
            }
        }

        result
    }

    /// `samLauncherValue()` (`:463-553`).
    fn placement_sam_launcher_value(&mut self, cx: &mut Ctx) -> ValueFn {
        let player = me(self);
        let st: &State = cx.st;
        let other_units = st.player(player).units_of(&[UnitType::SamLauncher]);
        let sp = spacing_constants(st);

        let difficulty = difficulty(st);
        let weight_by_level = difficulty == Difficulty::Hard || difficulty == Difficulty::Impossible;

        let mut protect_entries = Vec::new();
        // iter-live: `player.units()` read-only (`:475-486`).
        for &unit in st.player(player).units().as_slice() {
            let u = st.unit(unit);
            match u.unit_type() {
                UnitType::City | UnitType::Factory | UnitType::MissileSilo | UnitType::Port => {
                    protect_entries.push(ProtectEntry {
                        tile: u.tile(),
                        weight: if weight_by_level { f64::from(u.level()) } else { 1.0 },
                    });
                }
                _ => {}
            }
        }
        let range = st.config().default_sam_range();
        let range_squared = range * range;

        // TS `difficulty !== Easy && this.random.nextInt(0, 100) < 25` (:490-491): no draw on Easy.
        let use_coverage_weighting = difficulty != Difficulty::Easy && self.rng.next_int(0.0, 100.0) < 25.0;

        // Pre-compute existing SAM coverage for each protectable structure.
        let mut structure_coverage: Option<LookupMap<TileRef, f64>> = None;
        if use_coverage_weighting {
            let mut cov = LookupMap::new();
            let existing_sams = st.player(player).units_of(&[UnitType::SamLauncher]);
            for entry in &protect_entries {
                let mut coverage_score = 0.0;
                for &sam in &existing_sams {
                    let s = st.unit(sam);
                    let sam_range = st.config().sam_range(f64::from(s.level()));
                    let dist = f64::from(st.map.euclidean_dist_squared(entry.tile, s.tile()));
                    if dist <= sam_range * sam_range {
                        coverage_score += f64::from(s.level());
                    }
                }
                cov.insert(entry.tile, coverage_score);
            }
            structure_coverage = Some(cov);
        }

        ValueFn::SamLauncher {
            other_units,
            sp,
            difficulty,
            protect_entries,
            range_squared,
            use_coverage_weighting,
            structure_coverage,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `shouldUseConnectivityScore` draws `nextInt(0, 100)` once for every difficulty (Easy too).
    #[test]
    fn should_use_connectivity_score_always_draws_once() {
        for d in [Difficulty::Easy, Difficulty::Medium, Difficulty::Hard, Difficulty::Impossible] {
            let mut a = Prng::new(42.0);
            let mut b = Prng::new(42.0);
            let got = should_use_connectivity_score(&mut a, d);
            let draw = b.next_int(0.0, 100.0);
            let thr = match d {
                Difficulty::Easy => 0.0,
                Difficulty::Medium => 60.0,
                Difficulty::Hard => 75.0,
                Difficulty::Impossible => 100.0,
            };
            assert_eq!(got, draw < thr);
            assert_eq!(a.next(), b.next(), "same stream position after the call ({d:?})");
        }
    }

    /// `arraySampler` against a literal `Set` port (`new Set(a)`, `randFromSet` walks
    /// insertion order, `delete`), on a 300-element array with tombstones mid-set.
    #[test]
    fn array_sampler_matches_set_semantics() {
        let a: Vec<TileRef> = (0..300).map(|i| i * 7 + 3).collect();
        for seed in [1.0, 99.0, 12345.0] {
            let mut r1 = Prng::new(seed);
            let got = array_sampler(&mut r1, a.clone(), 25);
            let mut r2 = Prng::new(seed);
            let mut remaining: JsSet<TileRef> = a.iter().copied().collect();
            let mut want = Vec::new();
            for _ in 0..25 {
                let idx = r2.next_int(0.0, remaining.len() as f64) as usize;
                let Some(&t) = remaining.iter().nth(idx) else { panic!("index {idx} in range") };
                remaining.remove(&t);
                want.push(t);
            }
            assert_eq!(got, want, "seed {seed}");
            assert_eq!(r1.next(), r2.next(), "same draw count (seed {seed})");
        }
        // `a.length <= sampleSize`: all elements, no draw.
        let mut r = Prng::new(5.0);
        let mut r0 = Prng::new(5.0);
        assert_eq!(array_sampler(&mut r, vec![4, 2, 9], 25), vec![4, 2, 9]);
        assert_eq!(r.next(), r0.next());
    }

    #[test]
    fn connectivity_score_of_no_stations_is_zero() {
        // An empty station list scores 0; no map access needed.
        let st = State::default();
        assert_eq!(compute_connectivity_score(&st, 0, &[], 0.0, 100.0), 0.0);
    }
}
