//! `NationStructureBehavior` (`execution/nation/NationStructureBehavior.ts`). Owner: **P12**.
//!
//! Every TS method is an `impl NationExec` method named `structure_<snake_case>` (one
//! `&mut self` covers the shared PRNG `self.rng` and the sibling placement data,
//! ARCHITECTURE §4.4). Cross-file names must not collide with the other behaviours'
//! `impl NationExec` blocks, hence the prefix on the private helpers too.
//!
//! TS-semantics traps ported here (PORTING_GUIDE §8, §4):
//! - `if (!player.canBuild(..)) continue` treats a returned **tile 0** as falsy
//!   (`:552`); [`can_build_truthy`] keeps that.
//! - `:881` is `BigInt(Math.ceil(..))` (ceil, not floor; PORTING_GUIDE §14.7 lists the site
//!   under `from_floor_strict`, but TS writes `Math.ceil`): ported as
//!   `from_number_strict(ceil(x))`, which throws on NaN/±Inf like TS.

use crate::api::types::{Difficulty, GameMode, Relation};
use crate::canon::{Canon, CanonCx, CanonError, Cs, Uncanon, UncanonCx};
use crate::ctx::ts_throw;
use crate::exec::construction::ConstructionExec;
use crate::exec::nation::NationExec;
use crate::exec::nation::mirv::is_mirv_worth_saving_for;
use crate::exec::nation::structure_placement::StructurePlacement;
use crate::exec::nation::utils::{has_high_starting_gold, is_save_up_within_reach};
use crate::exec::prelude::*;
use crate::exec::upgrade_structure::UpgradeStructureExec;
use crate::state::State;
use of_det::bigint::from_number_strict;
use of_det::js;

/// `NationStructureBehavior` data, snapshot order (`lastStructureTick, placementsCount,
/// builtCrowdedMapFirstStructure, hasHighStartingGold, postSaveUpStartTick`), then non-snapshot.
#[derive(StateClone, Debug, Default)]
pub struct StructureB {
    pub last_structure_tick: Option<i32>,
    pub placements_count: i32,
    pub built_crowded_map_first_structure: bool,
    /// `_hasHighStartingGold: boolean | null` (`:222`).
    pub has_high_starting_gold: Option<bool>,
    pub post_save_up_start_tick: Option<i32>,
    /// non-snapshot: `placement` (`:218`; per-round caches). GC-only: the station cache holds
    /// `ClusterRef`s that TS keeps alive through the cached array (`:30`); the CS pass skips
    /// it (not in the snapshot, `:234-246`).
    #[refs(gc_only)]
    pub placement: StructurePlacement,
    /// non-snapshot: `mirvWorthSavingMemo: { tick, worth } | null` (`:224`).
    #[refs(gc_only)]
    pub mirv_worth_saving_memo: Option<(i32, bool)>,
}

// CS v1 (`NationStructureBehavior v1`, unwrapped per SPEC N4): the five snapshot fields in
// dump order (`:238-246`, `:1128-1138`).
impl Canon for StructureB {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        crate::canon_obj!(cx;
            "lastStructureTick" => self.last_structure_tick,
            "placementsCount" => self.placements_count,
            "builtCrowdedMapFirstStructure" => self.built_crowded_map_first_structure,
            "hasHighStartingGold" => self.has_high_starting_gold,
            "postSaveUpStartTick" => self.post_save_up_start_tick,
        )
    }
}

/// `restoreSnapshot` (`:249-266`): a fresh placement (empty caches) and a `null` MIRV memo,
/// then the five fields.
impl Uncanon for StructureB {
    fn uncanon(v: &Cs, cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        let f = |k: &str| v.get(k).ok_or_else(|| CanonError::new(format!("NationStructureBehavior: missing {k}")));
        Ok(StructureB {
            last_structure_tick: Option::<i32>::uncanon(f("lastStructureTick")?, cx)?,
            placements_count: i32::uncanon(f("placementsCount")?, cx)?,
            built_crowded_map_first_structure: bool::uncanon(f("builtCrowdedMapFirstStructure")?, cx)?,
            has_high_starting_gold: Option::<bool>::uncanon(f("hasHighStartingGold")?, cx)?,
            post_save_up_start_tick: Option::<i32>::uncanon(f("postSaveUpStartTick")?, cx)?,
            placement: StructurePlacement::default(),
            mirv_worth_saving_memo: None,
        })
    }
}

// ---------------------------------------------------------------------------------------
// Module constants (`:31-215`)

/// `StructureRatioConfig` (`:35-40`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct StructureRatioConfig {
    pub ratio_per_city: f64,
    pub perceived_cost_increase_per_owned: f64,
}

/// `SAM_RATIO_BY_DIFFICULTY` (`:43-48`).
fn sam_ratio_by_difficulty(d: Difficulty) -> f64 {
    match d {
        Difficulty::Easy => 0.15,
        Difficulty::Medium => 0.2,
        Difficulty::Hard => 0.25,
        Difficulty::Impossible => 0.3,
    }
}

/// `TEAM_PORT_RATIO` (`:51`).
const TEAM_PORT_RATIO: f64 = 1.0;

/// `getStructureRatios(difficulty, gameMode)[type]` (`:58-82`). TS builds the record and
/// indexes it; only Port/Factory/SAMLauncher/MissileSilo have entries (`undefined` otherwise).
pub(crate) fn get_structure_ratio(
    difficulty: Difficulty,
    game_mode: GameMode,
    t: UnitType,
) -> Option<StructureRatioConfig> {
    let team_ports = game_mode == GameMode::Team && difficulty != Difficulty::Easy;
    match t {
        UnitType::Port => Some(StructureRatioConfig {
            ratio_per_city: if team_ports { TEAM_PORT_RATIO } else { 0.75 },
            perceived_cost_increase_per_owned: 1.0,
        }),
        UnitType::Factory => {
            Some(StructureRatioConfig { ratio_per_city: 0.75, perceived_cost_increase_per_owned: 1.0 })
        }
        UnitType::SamLauncher => Some(StructureRatioConfig {
            ratio_per_city: sam_ratio_by_difficulty(difficulty),
            perceived_cost_increase_per_owned: 0.3,
        }),
        UnitType::MissileSilo => {
            Some(StructureRatioConfig { ratio_per_city: 0.2, perceived_cost_increase_per_owned: 1.0 })
        }
        _ => None,
    }
}

/// `CITY_PERCEIVED_COST_INCREASE_PER_OWNED` (`:85`).
const CITY_PERCEIVED_COST_INCREASE_PER_OWNED: f64 = 1.0;

/// `CITIES_BEFORE_SAVING` (`:88-93`).
fn cities_before_saving(d: Difficulty) -> f64 {
    match d {
        Difficulty::Easy => 0.0,
        Difficulty::Medium => 3.0,
        Difficulty::Hard => 4.0,
        Difficulty::Impossible => 5.0,
    }
}

/// `INCOME_STRUCTURE_SAVING_WEIGHT` (`:96-101`).
fn income_structure_saving_weight(d: Difficulty) -> f64 {
    match d {
        Difficulty::Easy => 1.0,
        Difficulty::Medium => 0.8,
        Difficulty::Hard => 0.6,
        Difficulty::Impossible => 0.4,
    }
}

/// `FACTORY_COASTAL_RATIO_MULTIPLIER` (`:104-109`).
fn factory_coastal_ratio_multiplier(d: Difficulty) -> f64 {
    match d {
        Difficulty::Easy => 0.33,
        Difficulty::Medium => 0.45,
        Difficulty::Hard => 0.55,
        Difficulty::Impossible => 0.6,
    }
}

/// `SAM_PROTECTED_TYPES` (`:112-117`).
pub(crate) const SAM_PROTECTED_TYPES: &[UnitType] =
    &[UnitType::City, UnitType::Factory, UnitType::MissileSilo, UnitType::Port];

/// `SAM_UPGRADE_WHEN_COVERED_CHANCE` (`:120-125`).
fn sam_upgrade_when_covered_chance(d: Difficulty) -> f64 {
    match d {
        Difficulty::Easy => 0.0,
        Difficulty::Medium => 50.0,
        Difficulty::Hard => 100.0,
        Difficulty::Impossible => 100.0,
    }
}

/// `MAX_MISSILE_SILOS` (`:128`).
const MAX_MISSILE_SILOS: f64 = 3.0;
/// `FIRST_MISSILE_SILO_RATIO` (`:131`).
const FIRST_MISSILE_SILO_RATIO: f64 = 0.4;
/// `UPGRADE_DENSITY_THRESHOLD = 1 / 1500` (`:134`).
const UPGRADE_DENSITY_THRESHOLD: f64 = 1.0 / 1500.0;

/// `MAX_UPGRADES_AT_ONCE` (`:137-142`).
fn max_upgrades_at_once(d: Difficulty) -> f64 {
    match d {
        Difficulty::Easy => 3.0,
        Difficulty::Medium => 10.0,
        Difficulty::Hard => f64::INFINITY,
        Difficulty::Impossible => f64::INFINITY,
    }
}

/// `DENSITY_STRUCTURE_TYPES = Structures.types.filter(t => t !== DefensePost)` (`:145-147`).
pub(crate) const DENSITY_STRUCTURE_TYPES: &[UnitType] =
    &[UnitType::City, UnitType::SamLauncher, UnitType::MissileSilo, UnitType::Port, UnitType::Factory];

/// `TILES_PER_CITY_EQUIVALENT` (`:150`).
const TILES_PER_CITY_EQUIVALENT: f64 = 2000.0;
/// `HIGH_NATION_DENSITY_THRESHOLD = 1 / 7500` (`:156`).
const HIGH_NATION_DENSITY_THRESHOLD: f64 = 1.0 / 7500.0;
/// `PACED_STARTING_GOLD_LIMIT = 10_000_000n` (`:162`).
const PACED_STARTING_GOLD_LIMIT: Gold = 10_000_000;
/// `HIGH_GOLD_STRUCTURE_COOLDOWN_TICKS` (`:165-171`).
const HIGH_GOLD_STRUCTURE_COOLDOWN_TICKS: [i32; 5] = [0, 0, 250, 150, 100];
/// `TEAM_POST_SAVE_UP_PHASE_TICKS` (`:174`).
const TEAM_POST_SAVE_UP_PHASE_TICKS: i32 = 150;
/// `UNDER_ATTACK_THREAT_RATIO` (`:180`).
const UNDER_ATTACK_THREAT_RATIO: f64 = 0.35;
/// `DEFENSE_POST_RATIO_PER_POST` (`:187`).
const DEFENSE_POST_RATIO_PER_POST: f64 = 0.4;
/// `DEFENSE_POST_FRONT_MIN_DIST = 2 / 3` (`:190`).
const DEFENSE_POST_FRONT_MIN_DIST: f64 = 2.0 / 3.0;
/// `DEFENSE_POST_FRONT_DIST_PER_RATIO = 1 / 3` (`:191`).
const DEFENSE_POST_FRONT_DIST_PER_RATIO: f64 = 1.0 / 3.0;
/// `DEFENSE_POST_FRONT_RATIO_CAP` (`:192`).
const DEFENSE_POST_FRONT_RATIO_CAP: f64 = 2.0;
/// `DEFENSE_POST_FRONT_BAND = 1 / 2` (`:194`).
const DEFENSE_POST_FRONT_BAND: f64 = 1.0 / 2.0;
/// `PROACTIVE_DEFENSE_POST_MIN_DIST = 1 / 2` (`:197`).
const PROACTIVE_DEFENSE_POST_MIN_DIST: f64 = 1.0 / 2.0;
/// `PROACTIVE_DEFENSE_POST_MAX_DIST` (`:198`).
const PROACTIVE_DEFENSE_POST_MAX_DIST: f64 = 1.0;
/// `PROACTIVE_DEFENSE_POST_MIN_CITIES` (`:200`).
const PROACTIVE_DEFENSE_POST_MIN_CITIES: f64 = 2.0;
/// `PROACTIVE_DEFENSE_POSTS_PER_CITY` (`:202`).
const PROACTIVE_DEFENSE_POSTS_PER_CITY: f64 = 0.5;

/// `PROACTIVE_DEFENSE_POST_HOSTILE_TROOP_RATIO` (`:204-209`): `undefined` for Easy/Medium.
fn proactive_defense_post_hostile_troop_ratio(d: Difficulty) -> Option<f64> {
    match d {
        Difficulty::Hard => Some(1.0),
        Difficulty::Impossible => Some(0.5),
        Difficulty::Easy | Difficulty::Medium => None,
    }
}

/// `DEFENSE_POST_SAMPLE_ATTEMPTS` (`:212`).
const DEFENSE_POST_SAMPLE_ATTEMPTS: u32 = 150;

// ---------------------------------------------------------------------------------------
// Shared helpers (module-private free functions: no name clash with other `impl NationExec`)

/// `this.player` of a behaviour: set at `initializeBehaviors` from `NationExecution.player`.
#[inline]
pub(crate) fn me(ne: &NationExec) -> SmallId {
    ne.player.ts_expect("NationStructureBehavior.player")
}

/// `game.config().gameConfig().difficulty`.
#[inline]
pub(crate) fn difficulty(st: &State) -> Difficulty {
    st.config().game_config().difficulty
}

/// `this.game.unitInfo(type).cost(this.game, this.player, extraUnits)` (`:784-786`).
#[inline]
pub(crate) fn cost(st: &State, p: SmallId, t: UnitType, extra_units: f64) -> Gold {
    st.config().unit_cost(t, st, p, extra_units)
}

/// JS truthiness of `player.canBuild(..)`'s `TileRef | false` result: `false` **and tile 0**
/// are falsy (`if (!player.canBuild(..))`, `NationStructureBehavior.ts:552`,
/// `NationStructurePlacement.ts:63`). Sites that compare `=== false` use `.is_none()`.
#[inline]
pub(crate) fn can_build_truthy(r: Option<TileRef>) -> bool {
    matches!(r, Some(t) if t != 0)
}

/// `a.reduce((sum, a) => sum + a.troops(), 0)` (`:320`, `:426`): left fold from 0.
fn sum_attack_troops(st: &State, attacks: &[AttackRef]) -> f64 {
    let mut sum = 0.0;
    for &a in attacks {
        sum += st.attack(a).troops();
    }
    sum
}

/// A TS `Set<Player>` used only for `.size` and `.has` (`:437`, `:376`; consumers
/// `:458`, `:467`, `:387`, `:392-393`). lookup-only: never iterated, so a deduplicating
/// `Vec` is exact.
#[derive(Default)]
struct PlayerSet(Vec<SmallId>);

impl PlayerSet {
    fn add(&mut self, p: SmallId) {
        if !self.0.contains(&p) {
            self.0.push(p);
        }
    }
    fn has(&self, p: Owner) -> bool {
        self.0.contains(&p)
    }
    fn size(&self) -> usize {
        self.0.len()
    }
}

impl NationExec {
    /// `structureBehavior.handleStructures()` (`:268-298`).
    pub fn structure_handle_structures(&mut self, cx: &mut Ctx) -> bool {
        // Defense posts are handled outside the normal pacing/counter system.
        if self.b().structure.placements_count > 0 && !cx.st.config().is_unit_disabled(UnitType::DefensePost) {
            if self.structure_try_build_defense_post(cx) {
                return true;
            }
            // If the attack threshold is met, block other structures even when placement failed.
            if self.structure_defense_post_needed(cx) {
                return false;
            }
        }

        if self.structure_is_on_structure_cooldown(cx) {
            return false;
        }
        if self.structure_is_in_post_save_up_blocked_phase(cx) {
            return false;
        }
        let built = self.structure_do_handle_structures(cx);
        if built {
            let t = cx.st.ticks();
            let s = &mut self.b().structure;
            s.last_structure_tick = Some(t);
            s.placements_count += 1;
        }
        built
    }

    /// `tryBuildDefensePost()` (`:307-349`).
    fn structure_try_build_defense_post(&mut self, cx: &mut Ctx) -> bool {
        let difficulty = difficulty(cx.st);
        if difficulty == Difficulty::Easy {
            return false;
        }
        // TS: `difficulty === Medium && !this.random.chance(2)` draws only on Medium (:310).
        if difficulty == Difficulty::Medium && !self.rng.chance(2.0) {
            return false;
        }

        let player = me(self);
        let attacks = cx.st.player(player).incoming_attacks();
        if attacks.is_empty() {
            return false;
        }

        let our_troops = cx.st.player(player).troops();
        if our_troops <= 0.0 {
            return false;
        }

        let incoming_troops = sum_attack_troops(cx.st, &attacks);
        let ratio = incoming_troops / our_troops;
        if ratio < UNDER_ATTACK_THREAT_RATIO {
            return false;
        }

        let allowed: f64 =
            if difficulty == Difficulty::Medium { 1.0 } else { js::ceil(ratio / DEFENSE_POST_RATIO_PER_POST) };

        let range = cx.st.config().defense_post_range();
        let scaled_ratio =
            if difficulty == Difficulty::Medium { 0.0 } else { js::min(ratio, DEFENSE_POST_FRONT_RATIO_CAP) };
        let min_dist = range * (DEFENSE_POST_FRONT_MIN_DIST + DEFENSE_POST_FRONT_DIST_PER_RATIO * scaled_ratio);
        let max_dist = min_dist + range * DEFENSE_POST_FRONT_BAND;

        let front_tiles = self.structure_get_attack_front_tiles(cx, &attacks);
        if self.structure_count_defense_posts_near_front(cx, &front_tiles, max_dist, Some(allowed)) >= allowed {
            return false;
        }

        self.structure_build_defense_post_behind(cx, &front_tiles, min_dist, max_dist)
    }

    /// `maybeBuildProactiveDefensePost()` (`:357-402`).
    fn structure_maybe_build_proactive_defense_post(&mut self, cx: &mut Ctx) -> bool {
        let difficulty = difficulty(cx.st);
        let Some(hostile_troop_ratio) = proactive_defense_post_hostile_troop_ratio(difficulty) else {
            return false;
        };
        if cx.st.config().is_unit_disabled(UnitType::DefensePost) {
            return false;
        }
        let city_count = self.structure_city_count(cx);
        if city_count < PROACTIVE_DEFENSE_POST_MIN_CITIES {
            return false;
        }

        let player = me(self);
        if cx.st.player(player).units_of(&[UnitType::DefensePost]).len() as f64
            >= city_count * PROACTIVE_DEFENSE_POSTS_PER_CITY
        {
            return false;
        }
        if cx.st.player(player).gold() < cost(cx.st, player, UnitType::DefensePost, 0.0) {
            return false;
        }

        let min_troops = cx.st.player(player).troops() * hostile_troop_ratio;
        let mut threats = PlayerSet::default();
        let near = cx.st.nearby_memo(player);
        let st: &State = cx.st;
        let pr = st.player(player);
        for neighbor in near {
            if !neighbor.is_player() || st.player(neighbor).player_type() == PlayerType::Bot {
                continue;
            }
            if pr.is_friendly(neighbor, false) {
                continue;
            }
            if pr.relation(neighbor) == Relation::Hostile && st.player(neighbor).troops() >= min_troops {
                threats.add(neighbor);
            }
        }
        if threats.size() == 0 {
            return false;
        }

        let range = cx.st.config().defense_post_range();
        let front_tiles = self.structure_border_tiles_facing(cx, &threats);
        let n = threats.size() as f64;
        if self.structure_count_defense_posts_near_front(cx, &front_tiles, range * 1.5, Some(n)) >= n {
            return false;
        }

        self.structure_build_defense_post_behind(
            cx,
            &front_tiles,
            range * PROACTIVE_DEFENSE_POST_MIN_DIST,
            range * PROACTIVE_DEFENSE_POST_MAX_DIST,
        )
    }

    /// `buildDefensePostBehind(frontTiles, minDist, maxDist)` (`:404-417`).
    fn structure_build_defense_post_behind(
        &mut self,
        cx: &mut Ctx,
        front_tiles: &[TileRef],
        min_dist: f64,
        max_dist: f64,
    ) -> bool {
        let player = me(self);
        if cx.st.player(player).gold() < cost(cx.st, player, UnitType::DefensePost, 0.0) {
            return false;
        }
        let Some(tile) = self.structure_find_defense_post_tile(cx, front_tiles, min_dist, max_dist) else {
            return false;
        };
        cx.st.add_execution(Exec::Construction(ConstructionExec::new(player, UnitType::DefensePost, tile, None, None)));
        true
    }

    /// `defensePostNeeded()` (`:419-428`).
    fn structure_defense_post_needed(&mut self, cx: &mut Ctx) -> bool {
        if difficulty(cx.st) == Difficulty::Easy {
            return false;
        }
        let player = me(self);
        let attacks = cx.st.player(player).incoming_attacks();
        if attacks.is_empty() {
            return false;
        }
        let our_troops = cx.st.player(player).troops();
        if our_troops <= 0.0 {
            return false;
        }
        let incoming_troops = sum_attack_troops(cx.st, &attacks);
        incoming_troops / our_troops >= UNDER_ATTACK_THREAT_RATIO
    }

    /// `getAttackFrontTiles(attacks)` (`:434-452`).
    fn structure_get_attack_front_tiles(&mut self, cx: &mut Ctx, attacks: &[AttackRef]) -> Vec<TileRef> {
        let player = me(self);
        let mut land_attackers = PlayerSet::default();
        for &a in attacks {
            let at = cx.st.attack(a);
            if at.source_tile().is_none() {
                land_attackers.add(at.attacker());
            }
        }

        let mut front_tiles = self.structure_border_tiles_facing(cx, &land_attackers);

        // A land attacker's beachheads are already in its adjacency front above.
        let st: &State = cx.st;
        for &a in attacks {
            let at = st.attack(a);
            if at.source_tile().is_none() || land_attackers.has(at.attacker()) {
                continue;
            }
            // iter-live: `a.borderTiles().forEach` reads only (`:447-449`).
            for t in at.border_tiles().iter() {
                if st.owner(t) == player {
                    front_tiles.push(t);
                }
            }
        }
        front_tiles
    }

    /// `borderTilesFacing(players)` (`:455-474`).
    fn structure_border_tiles_facing(&mut self, cx: &mut Ctx, players: &PlayerSet) -> Vec<TileRef> {
        let mut front_tiles = Vec::new();
        if players.size() == 0 {
            return front_tiles;
        }
        let st: &State = cx.st;
        let mut nbuf: [TileRef; 4] = [0; 4];
        // iter-live: `borderTiles().forEach` with a read-only body (`:463-472`).
        for border_tile in st.player(me(self)).border_tiles().iter() {
            let n = st.map.neighbors4(border_tile, &mut nbuf);
            for &nb in &nbuf[..n] {
                let owner = st.owner(nb);
                if players.has(owner) {
                    front_tiles.push(border_tile);
                    break; // TS: `return` from the forEach callback (`:469`)
                }
            }
        }
        front_tiles
    }

    /// `countDefensePostsNearFront(frontTiles, range, cap?)` (`:480-501`).
    fn structure_count_defense_posts_near_front(
        &mut self,
        cx: &mut Ctx,
        front_tiles: &[TileRef],
        range: f64,
        cap: Option<f64>,
    ) -> f64 {
        if front_tiles.is_empty() {
            return 0.0;
        }
        let st: &State = cx.st;
        let range_squared = range * range; // TS: `range ** 2`
        let mut count = 0.0;
        for dp in st.player(me(self)).units_of(&[UnitType::DefensePost]) {
            let dp_tile = st.unit(dp).tile();
            for &front_tile in front_tiles {
                if f64::from(st.map.euclidean_dist_squared(dp_tile, front_tile)) <= range_squared {
                    count += 1.0;
                    if let Some(cap) = cap
                        && count >= cap
                    {
                        return count;
                    }
                    break;
                }
            }
        }
        count
    }

    /// `findDefensePostTile(frontTiles, minDist, maxDist)` (`:509-556`).
    fn structure_find_defense_post_tile(
        &mut self,
        cx: &mut Ctx,
        front_tiles: &[TileRef],
        min_dist: f64,
        max_dist: f64,
    ) -> Option<TileRef> {
        if front_tiles.is_empty() {
            return None;
        }
        let player = me(self);
        let spread = cx.st.config().defense_post_range() * 1.5;
        let spread_range_squared = spread * spread; // TS: `(range * 1.5) ** 2`
        let existing_dp_tiles: Vec<TileRef> = {
            let st: &State = cx.st;
            st.player(player).units_of(&[UnitType::DefensePost]).into_iter().map(|u| st.unit(u).tile()).collect()
        };
        let near_existing_post = |st: &State, t: TileRef| {
            existing_dp_tiles.iter().any(|&dp| f64::from(st.map.euclidean_dist_squared(t, dp)) < spread_range_squared)
        };

        let anchors: Vec<TileRef> = front_tiles.iter().copied().filter(|&ft| !near_existing_post(cx.st, ft)).collect();
        if anchors.is_empty() {
            return None;
        }

        let min_squared = min_dist * min_dist;
        let max_squared = max_dist * max_dist;
        let search_radius = js::ceil(max_dist);
        for _attempt in 0..DEFENSE_POST_SAMPLE_ATTEMPTS {
            let anchor = match self.rng.rand_element(&anchors) {
                Ok(&a) => a,
                Err(_) => ts_throw!("array must not be empty"),
            };
            let st: &State = cx.st;
            let ax = f64::from(st.map.x(anchor));
            let ay = f64::from(st.map.y(anchor));
            let x = self.rng.next_int(ax - search_radius, ax + search_radius + 1.0);
            let y = self.rng.next_int(ay - search_radius, ay + search_radius + 1.0);
            if !st.map.is_valid_coord(x, y) {
                continue;
            }
            let t = st.map.ref_(x, y);
            if st.owner(t) != player {
                continue;
            }
            if near_existing_post(st, t) {
                continue;
            }
            let mut nearest_squared = f64::INFINITY;
            for &ft in front_tiles {
                nearest_squared = js::min(nearest_squared, f64::from(st.map.euclidean_dist_squared(t, ft)));
                if nearest_squared < min_squared {
                    break;
                }
            }
            if nearest_squared < min_squared || nearest_squared > max_squared {
                continue;
            }
            // TS `if (!player.canBuild(..)) continue` (:552): tile 0 is falsy.
            if !can_build_truthy(cx.st.player(player).can_build(cx.sc, UnitType::DefensePost, t, None)) {
                continue;
            }
            return Some(t);
        }
        None
    }

    /// `isOnStructureCooldown()` (`:558-573`).
    fn structure_is_on_structure_cooldown(&mut self, cx: &mut Ctx) -> bool {
        let Some(last) = self.b().structure.last_structure_tick else {
            return false;
        };
        let player = me(self);
        if !self.structure_has_high_starting_gold(cx)
            || cx.st.config().starting_gold(cx.st.player(player).info()) >= PACED_STARTING_GOLD_LIMIT
        {
            return false;
        }
        let pc = self.b().structure.placements_count;
        // TS `HIGH_GOLD_STRUCTURE_COOLDOWN_TICKS[this.placementsCount] ?? 0` (OOB → undefined → 0).
        let required_gap =
            usize::try_from(pc).ok().and_then(|i| HIGH_GOLD_STRUCTURE_COOLDOWN_TICKS.get(i)).copied().unwrap_or(0);
        if required_gap == 0 {
            return false;
        }
        cx.st.ticks() - last < required_gap
    }

    /// `isInPostSaveUpBlockedPhase()` (`:578-597`).
    fn structure_is_in_post_save_up_blocked_phase(&mut self, cx: &mut Ctx) -> bool {
        if cx.st.config().is_unit_disabled(UnitType::MissileSilo) {
            return false;
        }
        let save_up_target = self.structure_get_save_up_target(cx);
        let player = me(self);
        if self.b().structure.post_save_up_start_tick.is_none() {
            if cx.st.player(player).gold() < save_up_target {
                return false;
            }
            let t = cx.st.ticks();
            self.b().structure.post_save_up_start_tick = Some(t);
        }
        if cx.st.player(player).gold() >= save_up_target * 2 {
            return false;
        }
        let start = self.b().structure.post_save_up_start_tick.ts_expect("NationStructureBehavior.ts:592");
        let elapsed = cx.st.ticks() - start;
        // int: tick difference; JS `%` and Rust `%` agree on sign (PORTING_GUIDE §2).
        elapsed % (TEAM_POST_SAVE_UP_PHASE_TICKS * 2) >= TEAM_POST_SAVE_UP_PHASE_TICKS
    }

    /// `doHandleStructures()` (`:599-704`).
    fn structure_do_handle_structures(&mut self, cx: &mut Ctx) -> bool {
        self.placement_start_round(cx);
        let cities_disabled = cx.st.config().is_unit_disabled(UnitType::City);
        let city_count = self.structure_city_count(cx);
        let has_coastal_tiles = self.placement_has_coastal_tiles(cx);

        let missile_silos_enabled = !cx.st.config().is_unit_disabled(UnitType::MissileSilo);

        // High-starting-gold Hard/Impossible nations build a SAM first (`:608-622`); the `&&`
        // chain short-circuits in TS order (the memo write in hasHighStartingGold included).
        let difficulty = difficulty(cx.st);
        if self.b().structure.placements_count == 0
            && (difficulty == Difficulty::Hard || difficulty == Difficulty::Impossible)
            && !cx.st.config().is_unit_disabled(UnitType::AtomBomb)
            && missile_silos_enabled
            && !cx.st.config().is_unit_disabled(UnitType::SamLauncher)
            && self.structure_has_high_starting_gold(cx)
            && self.structure_maybe_spawn_structure(cx, UnitType::SamLauncher)
        {
            return true;
        }

        // Crowded maps / team spawn areas: the first structure is a port or factory (`:624-646`).
        if !cities_disabled
            && !self.b().structure.built_crowded_map_first_structure
            && (self.structure_is_high_nation_density(cx)
                || (difficulty != Difficulty::Easy && self.structure_starts_in_team_spawn_area(cx)))
        {
            let preferred_first = if has_coastal_tiles && !cx.st.config().is_unit_disabled(UnitType::Port) {
                UnitType::Port
            } else {
                UnitType::Factory
            };
            if cx.st.config().is_unit_disabled(preferred_first) {
                self.b().structure.built_crowded_map_first_structure = true;
            } else if self.structure_maybe_spawn_structure(cx, preferred_first) {
                self.b().structure.built_crowded_map_first_structure = true;
                return true;
            }
        }

        if self.structure_maybe_build_proactive_defense_post(cx) {
            return true;
        }

        // Build order for non-city structures (priority order) (`:653-658`).
        const BUILD_ORDER: [UnitType; 4] =
            [UnitType::Port, UnitType::Factory, UnitType::SamLauncher, UnitType::MissileSilo];

        let nukes_enabled = !cx.st.config().is_unit_disabled(UnitType::AtomBomb)
            || !cx.st.config().is_unit_disabled(UnitType::HydrogenBomb)
            || !cx.st.config().is_unit_disabled(UnitType::Mirv);

        for structure_type in BUILD_ORDER {
            if cx.st.config().is_unit_disabled(structure_type) {
                continue;
            }
            if structure_type == UnitType::Port && !has_coastal_tiles {
                continue;
            }
            if !nukes_enabled && (structure_type == UnitType::MissileSilo || structure_type == UnitType::SamLauncher) {
                continue;
            }
            if !missile_silos_enabled && structure_type == UnitType::SamLauncher {
                continue;
            }
            if self.structure_should_build_structure(cx, structure_type, city_count, has_coastal_tiles)
                && self.structure_maybe_spawn_structure(cx, structure_type)
            {
                return true;
            }
        }

        if !cities_disabled && self.structure_maybe_spawn_structure(cx, UnitType::City) {
            return true;
        }

        false
    }

    /// `cityCount()` (`:707-715`): cities owned, or a territory equivalent when disabled.
    fn structure_city_count(&mut self, cx: &mut Ctx) -> f64 {
        let player = me(self);
        if cx.st.config().is_unit_disabled(UnitType::City) {
            return js::max(
                1.0,
                js::floor(f64::from(cx.st.player(player).num_tiles_owned()) / TILES_PER_CITY_EQUIVALENT),
            );
        }
        f64::from(cx.st.player(player).units_owned(UnitType::City))
    }

    /// `startsInTeamSpawnArea()` (`:717-720`).
    fn structure_starts_in_team_spawn_area(&mut self, cx: &mut Ctx) -> bool {
        let st: &State = cx.st;
        match st.player(me(self)).team() {
            None => false,
            Some(team) => st.team_spawn_area(team).is_some(),
        }
    }

    /// `hasHighStartingGold()` (`:722-725`): `??=` memo.
    fn structure_has_high_starting_gold(&mut self, cx: &mut Ctx) -> bool {
        if let Some(v) = self.b().structure.has_high_starting_gold {
            return v;
        }
        let v = has_high_starting_gold(cx.st, me(self));
        self.b().structure.has_high_starting_gold = Some(v);
        v
    }

    /// `isHighNationDensity()` (`:727-733`).
    fn structure_is_high_nation_density(&mut self, cx: &mut Ctx) -> bool {
        let land_tiles = f64::from(cx.st.map.num_land_tiles()); // GameImpl.numLandTiles (:1476-1478)
        if land_tiles <= 0.0 {
            return false;
        }
        cx.st.nations().len() as f64 / land_tiles > HIGH_NATION_DENSITY_THRESHOLD
    }

    /// `shouldBuildStructure(type, cityCount, hasCoastalTiles)` (`:735-741`).
    fn structure_should_build_structure(
        &mut self,
        cx: &mut Ctx,
        t: UnitType,
        city_count: f64,
        has_coastal_tiles: bool,
    ) -> bool {
        self.structure_levels_due(cx, t, city_count, has_coastal_tiles) > 0.0
    }

    /// `levelsDue(type, cityCount, hasCoastalTiles)` (`:744-782`).
    fn structure_levels_due(&mut self, cx: &mut Ctx, t: UnitType, city_count: f64, has_coastal_tiles: bool) -> f64 {
        let gc = cx.st.config().game_config();
        let (difficulty, game_mode) = (gc.difficulty, gc.game_mode);
        let Some(config) = get_structure_ratio(difficulty, game_mode, t) else {
            return 0.0;
        };

        let mut ratio = config.ratio_per_city;

        // Reduce factory spawning if we have coastal tiles.
        if t == UnitType::Factory && has_coastal_tiles && !cx.st.config().is_unit_disabled(UnitType::Port) {
            ratio *= factory_coastal_ratio_multiplier(difficulty);
        }

        let owned = f64::from(cx.st.player(me(self)).units_owned(t));
        let target_count = if t == UnitType::MissileSilo {
            if owned == 0.0 {
                ratio = FIRST_MISSILE_SILO_RATIO;
            }
            js::min(js::floor(city_count * ratio), MAX_MISSILE_SILOS)
        } else {
            js::floor(city_count * ratio)
        };

        target_count - owned
    }

    /// `maybeSpawnStructure(type)` (`:788-833`).
    fn structure_maybe_spawn_structure(&mut self, cx: &mut Ctx, t: UnitType) -> bool {
        let player = me(self);
        let perceived_cost = self.structure_get_perceived_cost(cx, t);
        if cx.st.player(player).gold() < perceived_cost {
            return false;
        }

        // Another SAM would cover nothing new, but a higher level stops more of a salvo.
        if t == UnitType::SamLauncher && self.structure_should_upgrade_sam_instead(cx) {
            let sam = self.structure_sam_protecting_most(cx);
            return self.structure_upgrade_structure(cx, sam);
        }

        // Check if we should upgrade instead of building new.
        let structures = cx.st.player(player).units_of(&[t]);
        let upgradable = cx.st.config().unit_info(t).upgradable == Some(true);
        if self.structure_get_total_structure_density(cx) > UPGRADE_DENSITY_THRESHOLD && upgradable {
            if self.structure_maybe_upgrade_structure(cx, &structures) {
                return true;
            }
            // Density too high but couldn't upgrade: wait for construction.
            if !structures.is_empty() {
                return false;
            }
            // No structures of this type exist yet: fall through to build the first one.
        }

        let Some(tile) = self.placement_spawn_tile(cx, t) else {
            // No room for a new one: upgrade one instead (`:820-825`).
            return upgradable
                && difficulty(cx.st) != Difficulty::Easy
                && self.structure_maybe_upgrade_structure(cx, &structures);
        };
        // TS `canBuild === false` (:828): strict, so tile 0 passes.
        if cx.st.player(player).can_build(cx.sc, t, tile, None).is_none() {
            return false;
        }
        cx.st.add_execution(Exec::Construction(ConstructionExec::new(player, t, tile, None, None)));
        true
    }

    /// `getPerceivedCost(type)` (`:841-882`).
    fn structure_get_perceived_cost(&mut self, cx: &mut Ctx, t: UnitType) -> Gold {
        let player = me(self);
        let real_cost = cost(cx.st, player, t, 0.0);

        let save_up_target = self.structure_get_save_up_target(cx);
        if save_up_target == 0
            || cx.st.player(player).gold() >= save_up_target
            || !is_save_up_within_reach(cx.st, player, save_up_target)
        {
            return real_cost;
        }

        // Like humans, nations don't save up before their first few cities stand.
        let gc = cx.st.config().game_config();
        let (difficulty, game_mode) = (gc.difficulty, gc.game_mode);
        if self.structure_city_count(cx) < cities_before_saving(difficulty) {
            return real_cost;
        }

        let owned = f64::from(cx.st.player(player).units_owned(t));

        let mut increase_per_owned = if t == UnitType::City {
            CITY_PERCEIVED_COST_INCREASE_PER_OWNED
        } else {
            // TS `config?.perceivedCostIncreasePerOwned ?? 0.1` (:867).
            get_structure_ratio(difficulty, game_mode, t).map_or(0.1, |c| c.perceived_cost_increase_per_owned)
        };
        // Income structures pay for themselves (`:870-876`).
        if t == UnitType::City || t == UnitType::Port || t == UnitType::Factory {
            increase_per_owned *= income_structure_saving_weight(difficulty);
        }

        let multiplier = 1.0 + increase_per_owned * owned;
        // TS `BigInt(Math.ceil(Number(realCost) * multiplier))` (:881): strict, ceil.
        from_number_strict(js::ceil(real_cost as f64 * multiplier))
    }

    /// `getSaveUpTarget()` (`:888-921`).
    fn structure_get_save_up_target(&mut self, cx: &mut Ctx) -> Gold {
        let player = me(self);
        let st: &State = cx.st;
        let config = st.config();

        if config.is_unit_disabled(UnitType::MissileSilo) {
            return cost(st, player, UnitType::SamLauncher, 0.0);
        }

        if config.game_config().game_mode == GameMode::Team {
            return cost(st, player, UnitType::HydrogenBomb, 0.0);
        }

        let mirv_enabled = !config.is_unit_disabled(UnitType::Mirv);
        let hydro_enabled = !config.is_unit_disabled(UnitType::HydrogenBomb);
        let atom_enabled = !config.is_unit_disabled(UnitType::AtomBomb);

        if mirv_enabled && self.structure_is_mirv_worth_saving_for(cx) {
            let st: &State = cx.st;
            return cost(st, player, UnitType::Mirv, 0.0) + cost(st, player, UnitType::HydrogenBomb, 0.0);
        }
        let st: &State = cx.st;
        if hydro_enabled {
            return cost(st, player, UnitType::HydrogenBomb, 0.0) * 5;
        }
        if atom_enabled {
            return cost(st, player, UnitType::AtomBomb, 0.0) * 20;
        }
        cost(st, player, UnitType::SamLauncher, 0.0)
    }

    /// `isMirvWorthSavingFor()` (`:923-932`): memo valid for one tick.
    fn structure_is_mirv_worth_saving_for(&mut self, cx: &mut Ctx) -> bool {
        let tick = cx.st.ticks();
        // TS `this.mirvWorthSavingMemo?.tick !== tick` (:925).
        if self.b().structure.mirv_worth_saving_memo.map(|(t, _)| t) != Some(tick) {
            let worth = is_mirv_worth_saving_for(cx.st, me(self));
            self.b().structure.mirv_worth_saving_memo = Some((tick, worth));
        }
        self.b().structure.mirv_worth_saving_memo.map(|(_, w)| w).ts_expect("NationStructureBehavior.ts:931")
    }

    /// `maybeUpgradeStructure(structures)` (`:935-937`).
    fn structure_maybe_upgrade_structure(&mut self, cx: &mut Ctx, structures: &[UnitRef]) -> bool {
        let best = self.structure_find_best_structure_to_upgrade(cx, structures);
        self.structure_upgrade_structure(cx, best)
    }

    /// `upgradeStructure(structure)` (`:939-952`).
    fn structure_upgrade_structure(&mut self, cx: &mut Ctx, structure: Option<UnitRef>) -> bool {
        let Some(structure) = structure else {
            return false;
        };
        let player = me(self);
        let (id, t) = {
            let u = cx.st.unit(structure);
            (f64::from(u.id()), u.unit_type())
        };
        let amount = self.structure_upgrade_amount(cx, t);
        cx.st.add_execution(Exec::UpgradeStructure(UpgradeStructureExec::new(player, id, amount)));
        true
    }

    /// `upgradeAmount(type)` (`:955-976`).
    fn structure_upgrade_amount(&mut self, cx: &mut Ctx, t: UnitType) -> f64 {
        let difficulty = difficulty(cx.st);
        // Cities set the ratios, so only gold limits them.
        let due = if t == UnitType::City {
            f64::INFINITY
        } else {
            let city_count = self.structure_city_count(cx);
            let coastal = self.placement_has_coastal_tiles(cx);
            self.structure_levels_due(cx, t, city_count, coastal)
        };
        let max = js::min(max_upgrades_at_once(difficulty), due);
        let player = me(self);
        let mut spare = cx.st.player(player).gold() - self.structure_get_save_up_target(cx);
        let mut amount = 0.0;
        while amount < max {
            let c = cost(cx.st, player, t, amount);
            if c > spare {
                break;
            }
            spare -= c;
            amount += 1.0;
        }
        js::max(1.0, amount)
    }

    /// `shouldUpgradeSamInstead()` (`:979-999`).
    fn structure_should_upgrade_sam_instead(&mut self, cx: &mut Ctx) -> bool {
        let difficulty = difficulty(cx.st);
        let chance = sam_upgrade_when_covered_chance(difficulty);
        let player = me(self);
        let st: &State = cx.st;
        let sams = st.player(player).units_of(&[UnitType::SamLauncher]);
        if chance == 0.0 || sams.is_empty() {
            return false;
        }
        let config = st.config();
        for unit in st.player(player).units_of(SAM_PROTECTED_TYPES) {
            let ut = st.unit(unit).tile();
            let covered = sams.iter().any(|&sam| {
                let s = st.unit(sam);
                let r = config.sam_range(f64::from(s.level()));
                f64::from(st.map.euclidean_dist_squared(ut, s.tile())) <= r * r // TS `** 2`
            });
            if !covered {
                return false;
            }
        }
        chance == 100.0 || self.rng.next_int(0.0, 100.0) < chance
    }

    /// `samProtectingMost()` (`:1002-1025`).
    fn structure_sam_protecting_most(&mut self, cx: &mut Ctx) -> Option<UnitRef> {
        let player = me(self);
        let st: &State = cx.st;
        let config = st.config();
        let pr = st.player(player);
        let protectable = pr.units_of(SAM_PROTECTED_TYPES);
        let mut best: Option<UnitRef> = None;
        let mut best_levels = -1.0;
        for sam in pr.units_of(&[UnitType::SamLauncher]) {
            if !pr.can_upgrade_unit(sam) {
                continue;
            }
            let s = st.unit(sam);
            let r = config.sam_range(f64::from(s.level()));
            let range_squared = r * r; // TS `** 2`
            let mut levels = 0.0;
            for &unit in &protectable {
                let u = st.unit(unit);
                if f64::from(st.map.euclidean_dist_squared(u.tile(), s.tile())) <= range_squared {
                    levels += f64::from(u.level());
                }
            }
            if levels > best_levels {
                best = Some(sam);
                best_levels = levels;
            }
        }
        best
    }

    /// `getTotalStructureDensity()` (`:1030-1035`).
    fn structure_get_total_structure_density(&mut self, cx: &mut Ctx) -> f64 {
        let pr = cx.st.player(me(self));
        let tiles_owned = f64::from(pr.num_tiles_owned());
        if tiles_owned > 0.0 { pr.units_of(DENSITY_STRUCTURE_TYPES).len() as f64 / tiles_owned } else { 0.0 }
    }

    /// `findBestStructureToUpgrade(structures)` (`:1041-1125`).
    fn structure_find_best_structure_to_upgrade(&mut self, cx: &mut Ctx, structures: &[UnitRef]) -> Option<UnitRef> {
        if structures.is_empty() {
            return None;
        }
        let player = me(self);
        let st: &State = cx.st;

        // Filter to only upgradable structures.
        let upgradable: Vec<UnitRef> =
            structures.iter().copied().filter(|&s| st.player(player).can_upgrade_unit(s)).collect();
        if upgradable.is_empty() {
            return None;
        }

        let random_chance = match difficulty(st) {
            Difficulty::Easy => 70.0,
            Difficulty::Medium => 40.0,
            Difficulty::Hard => 25.0,
            Difficulty::Impossible => 10.0,
        };

        if self.rng.next_int(0.0, 100.0) < random_chance {
            return match self.rng.rand_element(&upgradable) {
                Ok(&u) => Some(u),
                Err(_) => ts_throw!("array must not be empty"),
            };
        }

        let sam_launchers = st.player(player).units_of(&[UnitType::SamLauncher]);

        // Score each structure based on SAM protection.
        let mut scored: Vec<(UnitRef, f64)> = Vec::with_capacity(upgradable.len());
        for &structure in &upgradable {
            let mut score = 0.0;
            let stile = st.unit(structure).tile();
            for &sam in &sam_launchers {
                let s = st.unit(sam);
                let sam_range = st.config().sam_range(f64::from(s.level()));
                let sam_range_squared = sam_range * sam_range;
                let dist_squared = f64::from(st.map.euclidean_dist_squared(stile, s.tile()));
                if dist_squared <= sam_range_squared {
                    score += 10.0;
                    if s.level() > 1 {
                        score += f64::from(s.level() - 1) * 7.5;
                    }
                }
            }
            // Add small random factor to break ties.
            score += self.rng.next_int(0.0, 5.0);
            scored.push((structure, score));
        }

        if scored.is_empty() {
            return None;
        }

        // Sort descending by score (`:1113`; comparator `b.score - a.score`, V8 TimSort).
        of_det::js_sort_by(&mut scored, |a, b| b.1 - a.1);

        // 50% of the time, pick the second or third best for variety.
        if scored.len() >= 2 && self.rng.chance(2.0) {
            let pick_index = if scored.len() >= 3 { self.rng.next_int(1.0, 3.0) as usize } else { 1 };
            return Some(scored[pick_index].0); // in-range: pick_index ∈ {1, 2} < len
        }

        Some(scored[0].0) // in-range: scored is non-empty
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::unit_groups;

    #[test]
    fn structure_ratios_match_ts_table() {
        // `getStructureRatios` (:58-82).
        let r = |d, m, t| get_structure_ratio(d, m, t);
        assert_eq!(r(Difficulty::Easy, GameMode::Ffa, UnitType::Port).map(|c| c.ratio_per_city), Some(0.75));
        assert_eq!(r(Difficulty::Easy, GameMode::Team, UnitType::Port).map(|c| c.ratio_per_city), Some(0.75));
        assert_eq!(r(Difficulty::Medium, GameMode::Team, UnitType::Port).map(|c| c.ratio_per_city), Some(1.0));
        assert_eq!(r(Difficulty::Hard, GameMode::Ffa, UnitType::Port).map(|c| c.ratio_per_city), Some(0.75));
        assert_eq!(
            r(Difficulty::Impossible, GameMode::Ffa, UnitType::SamLauncher),
            Some(StructureRatioConfig { ratio_per_city: 0.3, perceived_cost_increase_per_owned: 0.3 })
        );
        assert_eq!(r(Difficulty::Easy, GameMode::Ffa, UnitType::SamLauncher).map(|c| c.ratio_per_city), Some(0.15));
        assert_eq!(
            r(Difficulty::Hard, GameMode::Ffa, UnitType::MissileSilo),
            Some(StructureRatioConfig { ratio_per_city: 0.2, perceived_cost_increase_per_owned: 1.0 })
        );
        assert_eq!(
            r(Difficulty::Hard, GameMode::Ffa, UnitType::Factory),
            Some(StructureRatioConfig { ratio_per_city: 0.75, perceived_cost_increase_per_owned: 1.0 })
        );
        for t in [UnitType::City, UnitType::DefensePost, UnitType::Warship, UnitType::AtomBomb] {
            assert_eq!(r(Difficulty::Hard, GameMode::Ffa, t), None, "{t:?}");
        }
    }

    #[test]
    fn constants_match_ts() {
        assert_eq!(UPGRADE_DENSITY_THRESHOLD, 1.0 / 1500.0);
        assert_eq!(HIGH_NATION_DENSITY_THRESHOLD, 1.0 / 7500.0);
        assert_eq!(DEFENSE_POST_FRONT_MIN_DIST + DEFENSE_POST_FRONT_DIST_PER_RATIO * 2.0, 2.0 / 3.0 + 1.0 / 3.0 * 2.0);
        assert_eq!(max_upgrades_at_once(Difficulty::Hard), f64::INFINITY);
        assert_eq!(cities_before_saving(Difficulty::Impossible), 5.0);
        assert_eq!(income_structure_saving_weight(Difficulty::Hard), 0.6);
        assert_eq!(factory_coastal_ratio_multiplier(Difficulty::Easy), 0.33);
        assert_eq!(sam_upgrade_when_covered_chance(Difficulty::Medium), 50.0);
        assert_eq!(proactive_defense_post_hostile_troop_ratio(Difficulty::Medium), None);
        assert_eq!(proactive_defense_post_hostile_troop_ratio(Difficulty::Impossible), Some(0.5));
        // `Structures.types` minus DefensePost, in TS order.
        let mut expect = unit_groups::STRUCTURES.to_vec();
        expect.retain(|&t| t != UnitType::DefensePost);
        assert_eq!(DENSITY_STRUCTURE_TYPES, &expect[..]);
    }

    #[test]
    fn can_build_truthiness_treats_tile_zero_as_false() {
        assert!(!can_build_truthy(None));
        assert!(!can_build_truthy(Some(0)));
        assert!(can_build_truthy(Some(1)));
    }

    #[test]
    fn player_set_dedups_and_never_contains_terra_nullius() {
        let mut s = PlayerSet::default();
        s.add(SmallId(3));
        s.add(SmallId(3));
        s.add(SmallId(1));
        assert_eq!(s.size(), 2);
        assert!(s.has(SmallId(1)) && !s.has(SmallId::TERRA_NULLIUS));
    }
}
