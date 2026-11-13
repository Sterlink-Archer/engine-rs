//! `NationNukeBehavior` (`execution/nation/NationNukeBehavior.ts`). Owner: **P11**.
//!
//! Shared-PRNG draws (`self.rng`, ARCHITECTURE §4.4), in TS order:
//! - `maybeSendNuke`: `randTerritoryTileArray(this.random, …)` (`:215`), after target choice;
//! - `findBestNukeTarget`: `chance(2)` (Impossible + richest nation, `:432`);
//! - `findStrongestTeamTarget`: `randElement` (`:822`), `chance(2)` + `randElement` (`:826-836`);
//! - the emoji behaviour's own draws (`maybeSendEmoji`, `:1115`).
//!
//! Sibling calls: `this.attackBehavior.shouldAttack` is the `AiAttackHooks` override
//! (`self.should_attack`), `findIncomingAttackPlayer` the P9 base free function,
//! `strikeTroops`/`strike` P10's `attack_*` methods, `emojiBehavior.*` P10's `emoji_*`.

use crate::api::types::{Difficulty, EmojiRecipient, GameMode, PlayerType, Relation, UnitType, unit_groups};
use crate::canon::{Canon, CanonCx, CanonError, Cs, Uncanon, UncanonCx};
use crate::exec::ai_attack::{AiAttackHooks, find_incoming_attack_player};
use crate::exec::nation::NationExec;
use crate::exec::nation::emoji::EMOJI_NUKE;
use crate::exec::nation::mirv::is_mirv_worth_saving_for;
use crate::exec::nation::nuke_planning::{
    blast_hits_friendly_land, enemy_land_blast_coverage, is_nuke_already_landing_near,
    is_trajectory_interceptable_by_sam, plan_sam_salvo, silo_to_upgrade_for_salvo,
};
use crate::exec::nation::utils::{find_runaway_leader, is_save_up_within_reach, rand_territory_tile_array};
use crate::exec::nuke::NukeExec;
#[allow(unused_imports)]
use crate::exec::prelude::*;
use crate::exec::upgrade_structure::UpgradeStructureExec;
use crate::exec::util::closest_two_tiles;
use crate::state::State;
use crate::util::bounding_box_tiles;
use of_det::bigint::from_number_strict;
use of_det::js;
use of_det::ts_throw;

/// `HIGH_DENSITY_NUKE_THRESHOLD = 1 / 75` (`:56`): level-weighted structure density above
/// which the richest impossible nation will pre-emptively nuke a player.
const HIGH_DENSITY_NUKE_THRESHOLD: f64 = 1.0 / 75.0;
/// `MIN_LEVEL_SUM_FOR_HIGH_DENSITY_NUKE = 5` (`:59`).
const MIN_LEVEL_SUM_FOR_HIGH_DENSITY_NUKE: f64 = 5.0;
/// `CROWN_NEAR_WIN_SHARE = 0.5` (`:62`).
const CROWN_NEAR_WIN_SHARE: f64 = 0.5;
/// `FULL_BLAST_COVERAGE_VALUE = 50_000` (`:72`).
const FULL_BLAST_COVERAGE_VALUE: f64 = 50_000.0;
/// `STRIKE_MIN_TROOP_RATIO = 0.5` (`:75`).
const STRIKE_MIN_TROOP_RATIO: f64 = 0.5;
/// `STRIKE_MIN_STACK_RATIO = 0.5` (`:78`).
const STRIKE_MIN_STACK_RATIO: f64 = 0.5;
/// `STRIKE_BORDER_MARGIN = 10` (`:81`).
const STRIKE_BORDER_MARGIN: f64 = 10.0;
/// `STRIKE_SPAWN_TICKS = 10` (`:84`).
const STRIKE_SPAWN_TICKS: i32 = 10;
/// `NUKE_PRICE_MEMORY_TICKS = 3000` (`:87`).
pub const NUKE_PRICE_MEMORY_TICKS: i32 = 3000;

/// `MIN_BLAST_COVERAGE: Record<NukeType, number>` (`:65-69`).
fn min_blast_coverage(t: UnitType) -> f64 {
    match t {
        UnitType::AtomBomb => 0.25,
        // Below this, an atom bomb on solid enemy land destroys more tiles per gold
        UnitType::HydrogenBomb => 0.4,
        // TS: `MIN_BLAST_COVERAGE[nukeType]` is only read with a NukeType.
        _ => unreachable!("MIN_BLAST_COVERAGE[{t:?}]"),
    }
}

/// `Strike` (`:89-94`).
#[derive(StateClone, Debug, Default, PartialEq, Eq)]
pub struct Strike {
    pub target: SmallId,
    pub tile: TileRef,
    pub tick: Tick,
    pub nuke: Option<UnitRef>,
}

/// A perceived nuke price (`atomBombPerceivedCost` / `hydrogenBombPerceivedCost`, TS `bigint`).
///
/// **i128, not `Gold` (i64)** every send stores `inflated * 150n / 100n` (atom) or
/// `* 125n / 100n` (hydrogen) of the *inflated* price, and Impossible SAM salvos send several
/// bombs per tick, so within one 3000-tick memory window the price compounds past i64 (World FFA
/// rollouts reached 3.8e15, whose decay product `(perceived - real) * left` is > i64::MAX;
/// a World FFA game stopped at tick 30,776 while TS went on to win at tick 32,011). TS bigints are
/// unbounded; i128 plus the overflow-free [`mul_div_nonneg`] is exact for every perceived price
/// below `i128::MAX / 1.5` (≈ 1.1e38, about 108 more compounding sends than i64 allowed). Beyond
/// that the port still stops loudly (`Fatal::Bug`), never wraps. CS v1 encodes the value as
/// `BIGINT` (zigzag uvarint over the full i128), byte-identical to TS for this whole range.
pub type PerceivedCost = i128;

/// `NationNukeBehavior` data, `NationNukeBehavior v2` snapshot order.
#[derive(StateClone, Debug, Default)]
pub struct NukeB {
    /// `recentlySentNukes: [Tick, TileRef, AtomBomb | HydrogenBomb][]` (`:97-101`).
    pub recently_sent_nukes: Vec<(Tick, TileRef, UnitType)>,
    pub atom_bombs_launched: i32,
    /// `atomBombPerceivedCost = this.cost(AtomBomb)` (bigint, field initialiser `:103`).
    pub atom_bomb_perceived_cost: PerceivedCost,
    pub atom_bomb_last_launch: Option<Tick>,
    pub hydrogen_bombs_launched: i32,
    pub hydrogen_bomb_perceived_cost: PerceivedCost,
    pub hydrogen_bomb_last_launch: Option<Tick>,
    pub strikes: Vec<Strike>,
}

impl NukeB {
    /// `new NationNukeBehavior(random, game, player, attackBehavior, emojiBehavior)`
    /// (`:96-117`): the field initialisers, run at construction
    /// (`NationExecution.initializeBehaviors`, `NationExecution.ts:303-309`). The two perceived
    /// costs are the real prices **at that moment** (`:103`, `:106`). No PRNG draw.
    pub fn new(st: &State, player: SmallId) -> NukeB {
        NukeB {
            recently_sent_nukes: Vec::new(),
            atom_bombs_launched: 0,
            atom_bomb_perceived_cost: PerceivedCost::from(cost(st, player, UnitType::AtomBomb)),
            atom_bomb_last_launch: None,
            hydrogen_bombs_launched: 0,
            hydrogen_bomb_perceived_cost: PerceivedCost::from(cost(st, player, UnitType::HydrogenBomb)),
            hydrogen_bomb_last_launch: None,
            strikes: Vec::new(),
        }
    }

    /// `removeOldNukeEvents()` (`:964-973`): drops sends older than 600 ticks (1 minute).
    fn remove_old_nuke_events(&mut self, tick: Tick) {
        let max_age: i64 = 600; // 600 ticks = 1 minute
        // TS adds in numbers; i64 keeps `sent + 600` exact even next to i32::MAX.
        while !self.recently_sent_nukes.is_empty()
            && i64::from(self.recently_sent_nukes[0].0) + max_age < i64::from(tick)
        {
            self.recently_sent_nukes.remove(0); // TS: shift()
        }
    }

    /// `inflatedNukeCost(type)` (`:900-917`) with `real = this.cost(type)` and
    /// `forgets = this.forgetsNukePrices()` precomputed (both are pure).
    fn inflated_nuke_cost_with(&self, t: UnitType, real: Gold, forgets: bool, ticks: Tick) -> PerceivedCost {
        let real = PerceivedCost::from(real);
        let (perceived, last_launch) = if t == UnitType::AtomBomb {
            (self.atom_bomb_perceived_cost, self.atom_bomb_last_launch)
        } else {
            (self.hydrogen_bomb_perceived_cost, self.hydrogen_bomb_last_launch)
        };
        if perceived <= real {
            return real;
        }
        if !forgets {
            return perceived;
        }
        let left = match last_launch {
            None => 0,
            Some(l) => NUKE_PRICE_MEMORY_TICKS - (ticks - l),
        };
        if left <= 0 {
            return real;
        }
        // TS: `real + ((perceived - real) * BigInt(left)) / BigInt(NUKE_PRICE_MEMORY_TICKS)`
        // (`:913-916`); bigint `/` truncates toward 0 (the divisor is the constant 3000, never 0n).
        // `perceived > real` here and `0 < left <= 3000`, so the operands are non-negative and
        // `mul_div_nonneg` gives the exact floor without forming the (possibly > i128) product.
        big_add(
            real,
            mul_div_nonneg(
                big_sub(perceived, real),
                PerceivedCost::from(from_number_strict(f64::from(left))),
                PerceivedCost::from(from_number_strict(f64::from(NUKE_PRICE_MEMORY_TICKS))),
            ),
        )
    }

    /// The bookkeeping half of `sendNuke` (`:1098-1111`), with `perceived =
    /// this.inflatedNukeCost(nukeType)` evaluated by the caller at the TS position.
    fn record_send(&mut self, tick: Tick, tile: TileRef, nuke_type: UnitType, perceived: PerceivedCost) {
        self.recently_sent_nukes.push((tick, tile, nuke_type));
        if nuke_type == UnitType::AtomBomb {
            self.atom_bombs_launched += 1;
            // Increase perceived cost by 50% each time to simulate saving up for a MIRV (higher
            // than hydro to make atom bombs less attractive for the lategame)
            self.atom_bomb_perceived_cost = mul_div_nonneg(perceived, 150, 100); // `perceived * 150n / 100n`
            self.atom_bomb_last_launch = Some(tick);
        } else if nuke_type == UnitType::HydrogenBomb {
            self.hydrogen_bombs_launched += 1;
            // Increase perceived cost by 25% each time to simulate saving up for a MIRV
            self.hydrogen_bomb_perceived_cost = mul_div_nonneg(perceived, 125, 100); // `perceived * 125n / 100n`
            self.hydrogen_bomb_last_launch = Some(tick);
        }
    }
}

/// Bigint `a * b` on i64 `Gold` (ARCHITECTURE §7.2), for real prices only
/// (`nuke_planning::plan_sam_salvo`: `atomCost * BigInt(bombsToFire)`, ≤ 10^6 × a few bombs).
/// A product outside i64 has no faithful i64 value, so it is a loud Rust-only failure
/// (`Fatal::Bug`) in **every** profile, never a silent release-mode wrap. Perceived prices,
/// which do compound past i64, use [`PerceivedCost`] (i128) instead (ledger t196).
#[inline]
pub(crate) fn big_mul(a: Gold, b: Gold) -> Gold {
    match a.checked_mul(b) {
        Some(x) => x,
        None => panic!("bigint {a} * {b} outside i64 (real nuke cost)"),
    }
}

/// Bigint `a * m / d` (`/` truncating toward 0, as JS BigInt) for `a, m >= 0`, `d > 0`, computed
/// without forming `a * m`: with `a = q*d + r`, `floor(a*m/d) = q*m + floor(r*m/d)` (exact,
/// since `q*m` is an integer). The result is the TS value whenever *it* fits i128; only a result
/// beyond i128 stops loudly. Negative operands (unreachable: prices are positive) take the
/// direct checked path, which has JS semantics too.
#[inline]
pub(crate) fn mul_div_nonneg(a: PerceivedCost, m: PerceivedCost, d: PerceivedCost) -> PerceivedCost {
    assert!(d != 0, "bigint division by 0n in a perceived nuke cost"); // TS would throw; constant here
    if a < 0 || m < 0 || d < 0 {
        return big_mul128(a, m) / d;
    }
    let (q, r) = (a / d, a % d);
    big_add(big_mul128(q, m), big_mul128(r, m) / d)
}

/// Bigint `a * b` on i128 (see [`PerceivedCost`]): loud beyond i128.
#[inline]
fn big_mul128(a: PerceivedCost, b: PerceivedCost) -> PerceivedCost {
    match a.checked_mul(b) {
        Some(x) => x,
        None => panic!("bigint {a} * {b} outside i128 (NationNukeBehavior perceived cost)"),
    }
}

/// Bigint `a + b` on i128 (see [`PerceivedCost`]).
#[inline]
fn big_add(a: PerceivedCost, b: PerceivedCost) -> PerceivedCost {
    match a.checked_add(b) {
        Some(x) => x,
        None => panic!("bigint {a} + {b} outside i128 (NationNukeBehavior perceived cost)"),
    }
}

/// Bigint `a - b` on i128 (see [`PerceivedCost`]).
#[inline]
fn big_sub(a: PerceivedCost, b: PerceivedCost) -> PerceivedCost {
    match a.checked_sub(b) {
        Some(x) => x,
        None => panic!("bigint {a} - {b} outside i128 (NationNukeBehavior perceived cost)"),
    }
}

/// `this.cost(type)` (`:1156-1158`).
#[inline]
fn cost(st: &State, player: SmallId, t: UnitType) -> Gold {
    st.config().unit_cost(t, st, player, 0.0)
}

/// `this.player` (set at construction in TS; `None` only before `init`).
#[inline]
fn me(n: &NationExec) -> SmallId {
    n.player.ts_expect("NationNukeBehavior.player")
}

#[inline]
fn difficulty(st: &State) -> Difficulty {
    st.config().game_config().difficulty
}

#[inline]
fn hard_or_impossible(d: Difficulty) -> bool {
    d == Difficulty::Hard || d == Difficulty::Impossible
}

/// `this.game.euclideanDistSquared(a, b)` as the TS number.
#[inline]
fn d2(st: &State, a: TileRef, b: TileRef) -> f64 {
    f64::from(st.map.euclidean_dist_squared(a, b))
}

/// `structureLevels(player)` (`:1161-1165`).
fn structure_levels(st: &State, player: SmallId) -> f64 {
    let mut levels = 0.0;
    for s in st.player(player).units_of(unit_groups::STRUCTURES) {
        levels += f64::from(st.unit(s).level());
    }
    levels
}

/// `this.random.randElement(arr)` on an array the caller proved non-empty (TS would throw
/// `"array must not be empty"`, `PseudoRandom.ts:80-85`).
fn rand_element<T: Copy>(rng: &mut Prng, arr: &[T]) -> T {
    match rng.rand_index(arr.len()) {
        Ok(i) => arr[i],
        Err(_) => ts_throw!("array must not be empty"),
    }
}

/// `forgetsNukePrices()` (`:891-898`): Hard & Impossible outside team games.
fn forgets_nuke_prices(st: &State) -> bool {
    let gc = st.config().game_config();
    hard_or_impossible(gc.difficulty) && gc.game_mode != GameMode::Team
}

/// `mirvSaveUpTarget()` (`:929-931`).
fn mirv_save_up_target(st: &State, player: SmallId) -> Gold {
    cost(st, player, UnitType::Mirv) + cost(st, player, UnitType::HydrogenBomb)
}

/// `isUnderHeavyAttack()` (`:951-962`).
fn is_under_heavy_attack(st: &State, player: SmallId) -> bool {
    // Get the total incoming attack troops
    let incoming_attacks = st.player(player).incoming_attacks();
    let mut total_incoming_troops = 0.0;
    for a in incoming_attacks {
        total_incoming_troops += st.attack(a).troops();
    }

    let my_troops = st.player(player).troops();

    total_incoming_troops >= my_troops
}

/// `paysRealNukePrice()` (`:855-888`).
fn pays_real_nuke_price(st: &State, player: SmallId) -> bool {
    // If only 2 players left, use actual cost (no point saving for MIRV)
    if st.players().len() == 2 {
        return true;
    }

    // If MIRVs are disabled, return the actual cost
    if st.config().is_unit_disabled(UnitType::Mirv) {
        return true;
    }

    // Save up a limited amount in team games, synced with NationStructureBehavior
    // Saving up for a MIRV is not relevant
    if st.config().game_config().game_mode == GameMode::Team
        && st.player(player).gold() > cost(st, player, UnitType::HydrogenBomb)
    {
        return true;
    }

    // Return the actual cost if we already have enough gold to buy both a MIRV and a hydro
    if st.player(player).gold() > mirv_save_up_target(st, player) {
        return true;
    }

    // On Hard & Impossible, ignore perceived cost when under heavy attack
    // The nation is probably going to get destroyed soon, so go all-in on nukes
    hard_or_impossible(difficulty(st)) && is_under_heavy_attack(st, player)
}

/// `isHittingUs(other)` (`:934-949`): whether `other` is attacking us or has a bomb flying at
/// our land.
fn is_hitting_us(st: &State, player: SmallId, other: SmallId) -> bool {
    if st.player(player).incoming_attacks().iter().any(|&a| st.attack(a).attacker() == other) {
        return true;
    }
    st.player(other)
        .units_of(&[UnitType::AtomBomb, UnitType::HydrogenBomb, UnitType::Mirv, UnitType::MirvWarhead])
        .iter()
        .any(|&nuke| st.unit(nuke).target_tile().is_some_and(|tile| st.owner(tile) == player))
}

/// `ourTroopsAttacking(target)` (`:600-606`).
fn our_troops_attacking(st: &State, player: SmallId, target: SmallId) -> f64 {
    let mut troops = 0.0;
    // iter-live: read-only loop over the live `outgoingAttacks()` array.
    for &a in st.player(player).outgoing_attacks().as_slice() {
        if st.attack(a).target() == target {
            troops += st.attack(a).troops();
        }
    }
    troops
}

/// `findIncomingNukePlayer()` (`:608-622`).
fn find_incoming_nuke_player(st: &State, player: SmallId) -> Option<SmallId> {
    let nukes = st.units_of(&[UnitType::AtomBomb, UnitType::HydrogenBomb]);
    let me = st.player(player);
    for nuke in nukes {
        let u = st.unit(nuke);
        let sender = u.owner();
        if me.is_friendly(sender, false) {
            continue;
        }
        if u.target_tile().is_some_and(|t| st.owner(t) == player) {
            return Some(sender);
        }
    }
    None
}

/// `isRichestNation()` (`:624-632`).
fn is_richest_nation(st: &State, player: SmallId) -> bool {
    let my_gold = st.player(player).gold();
    for other in st.players() {
        if other == player {
            continue;
        }
        if st.player(other).player_type() != PlayerType::Nation {
            continue;
        }
        if st.player(other).gold() > my_gold {
            return false;
        }
    }
    true
}

/// `findHighDensityTarget()` (`:634-653`).
fn find_high_density_target(st: &State, player: SmallId) -> Option<SmallId> {
    let me = st.player(player);
    let mut best_target: Option<SmallId> = None;
    let mut best_density = HIGH_DENSITY_NUKE_THRESHOLD;
    for other in st.players() {
        if other == player {
            continue;
        }
        if st.player(other).player_type() == PlayerType::Bot {
            continue;
        }
        if me.is_friendly(other, false) {
            continue;
        }
        let tiles_owned = f64::from(st.player(other).num_tiles_owned());
        if tiles_owned == 0.0 {
            continue;
        }
        let level_sum = structure_levels(st, other);
        // Skip players with too few structures regardless of density
        if level_sum < MIN_LEVEL_SUM_FOR_HIGH_DENSITY_NUKE {
            continue;
        }
        let density = level_sum / tiles_owned;
        if density > best_density {
            best_density = density;
            best_target = Some(other);
        }
    }
    best_target
}

/// `game.players().slice().sort((a, b) => b.numTilesOwned() - a.numTilesOwned())` (`:405-408`,
/// `:665-668`).
fn players_sorted_by_tiles(st: &State) -> Vec<SmallId> {
    let mut v = st.players();
    of_det::js_sort_by(&mut v, |&a, &b| {
        f64::from(st.player(b).num_tiles_owned()) - f64::from(st.player(a).num_tiles_owned())
    }); // TS: NationNukeBehavior.ts:408 / :668
    v
}

/// `numLandTiles() - numTilesWithFallout()` (`:402-403`, `:676-677`), as TS numbers.
fn num_tiles_without_fallout(st: &State) -> f64 {
    f64::from(st.map.num_land_tiles()) - f64::from(st.num_tiles_with_fallout())
}

/// `findFFACrownTarget()` (`:655-719`).
fn find_ffa_crown_target(st: &State, player: SmallId) -> Option<SmallId> {
    let gc = st.config().game_config();
    let (difficulty, game_mode) = (gc.difficulty, gc.game_mode);
    if game_mode != GameMode::Ffa {
        return None;
    }

    if st.players().len() <= 1 {
        return None;
    }

    let sorted_by_tiles = players_sorted_by_tiles(st);
    let first_place = sorted_by_tiles[0]; // in-range: players().length > 1
    let me = st.player(player);

    // Don't target ourselves or allies
    if first_place == player || me.is_friendly(first_place, false) {
        return None;
    }

    let n = num_tiles_without_fallout(st);
    if n <= 0.0 {
        return None;
    }

    let first_place_share = f64::from(st.player(first_place).num_tiles_owned()) / n;
    if first_place_share > CROWN_NEAR_WIN_SHARE {
        return Some(first_place);
    }

    // Below that, only a runaway leader that is far enough ahead of us.
    // Its neighbors react first, everyone else once the gap is twice as big.
    if find_runaway_leader(st) != Some(first_place) {
        return None;
    }

    let mut threshold = match difficulty {
        Difficulty::Easy => return None, // Never notices a runaway leader
        Difficulty::Medium => 0.3,       // 30%
        Difficulty::Hard => 0.2,         // 20%
        Difficulty::Impossible => 0.1,   // 10%
    };
    if !me.shares_border_with(first_place) {
        threshold *= 2.0;
    }

    let my_share = f64::from(me.num_tiles_owned()) / n;
    if first_place_share - my_share > threshold {
        return Some(first_place);
    }

    None
}

/// `findTeammateFightTarget()` (`:722-755`): Medium and up, the enemy attacking our teammates
/// hardest, else the one they attack hardest.
fn find_teammate_fight_target(st: &State, player: SmallId) -> Option<SmallId> {
    let gc = st.config().game_config();
    if gc.game_mode != GameMode::Team || gc.difficulty == Difficulty::Easy {
        return None;
    }
    let me = st.player(player);
    let teammates: Vec<SmallId> = st.players().into_iter().filter(|&p| p != player && me.is_on_same_team(p)).collect();
    let is_enemy =
        |p: Owner| p.is_player() && st.player(p).player_type() != PlayerType::Bot && !me.is_friendly(p, false);

    let mut best: Option<SmallId> = None;
    let mut best_troops = 0.0;
    for &mate in &teammates {
        for a in st.player(mate).incoming_attacks() {
            let attacker = st.attack(a).attacker();
            if is_enemy(attacker) && st.attack(a).troops() > best_troops {
                best = Some(attacker);
                best_troops = st.attack(a).troops();
            }
        }
    }
    if best.is_some() {
        return best;
    }
    for &mate in &teammates {
        // iter-live: read-only loop over the live `outgoingAttacks()` array.
        for &a in st.player(mate).outgoing_attacks().as_slice() {
            let target = st.attack(a).target();
            if is_enemy(target) && st.attack(a).troops() > best_troops {
                best = Some(target);
                best_troops = st.attack(a).troops();
            }
        }
    }
    best
}

/// `isValidNukeTile(t, nukeTarget)` (`:975-990`).
fn is_valid_nuke_tile(st: &State, player: SmallId, t: TileRef, nuke_target: SmallId) -> bool {
    let owner = st.owner(t);
    if owner == nuke_target {
        return true;
    }
    // On Hard & Impossible, allow TerraNullius (hit small islands) and other players unless they
    // are teammates or allies (even AFK ones)
    if hard_or_impossible(difficulty(st)) && (!owner.is_player() || !st.player(player).is_friendly(owner, true)) {
        return true;
    }
    // On Easy & Medium, only allow tiles owned by the target player (=> nuke away from the
    // border) to reduce nuke usage
    false
}

/// `nukeTileScore(tile, silos, targets, nukeType)` (`:992-1090`).
fn nuke_tile_score(
    st: &State,
    nb: &NukeB,
    tile: TileRef,
    silos: &[UnitRef],
    targets: &[UnitRef],
    nuke_type: UnitType,
) -> f64 {
    let config = st.config();
    let magnitude = config.nuke_magnitudes(nuke_type);
    // TS: `euclDistFN(tile, magnitude.outer, false)` = `(gm, n) => gm.euclideanDistSquared(tile,
    // n) <= dist * dist` (`GameMapImpl.ts:665-673`).
    let outer2 = magnitude.outer * magnitude.outer;
    let dist = |t: TileRef| d2(st, tile, t) <= outer2;
    let mut tile_value = targets
        .iter()
        .filter(|&&u| dist(st.unit(u).tile()))
        .map(|&u| {
            let unit = st.unit(u);
            let level = f64::from(unit.level());
            match unit.unit_type() {
                UnitType::City => 25_000.0 * level,
                UnitType::DefensePost => 5_000.0 * level,
                UnitType::MissileSilo => 50_000.0 * level,
                UnitType::Port => 15_000.0 * level,
                UnitType::Factory => 15_000.0 * level,
                _ => 0.0,
            }
        })
        .fold(0.0, |prev, cur| prev + cur);

    let difficulty = difficulty(st);
    // On Easy, ignore SAMs entirely.
    // On Medium, apply a simple local SAM penalty.
    // On Hard & Impossible we rely on trajectory-based interception checks instead. See
    // maybeSendNuke().
    if difficulty == Difficulty::Medium {
        let dist50 = |t: TileRef| d2(st, tile, t) <= 50.0 * 50.0; // euclDistFN(tile, 50, false)
        let has_sam =
            targets.iter().any(|&u| st.unit(u).unit_type() == UnitType::SamLauncher && dist50(st.unit(u).tile()));
        if has_sam {
            return -1.0;
        }
    }

    // On Impossible difficulty and a hydrogen bomb, add value for SAMs that can be outranged
    if difficulty == Difficulty::Impossible && nuke_type == UnitType::HydrogenBomb {
        let hydro_magnitude = config.nuke_magnitudes(UnitType::HydrogenBomb);
        let nearby_sams = st.nearby_units(tile, hydro_magnitude.outer, &[UnitType::SamLauncher], |_, _, _| true, false);

        for sam in nearby_sams {
            let sam_level = f64::from(st.unit(sam.unit).level());
            if sam_level >= 5.0 {
                continue; // Can't outrange level 5+ SAMs
            }

            let sam_range = config.sam_range(sam_level);
            let dist_to_sam = js::sqrt(d2(st, tile, st.unit(sam.unit).tile()));

            // Check if we can outrange this SAM
            if dist_to_sam > sam_range {
                // Add significant value for destroying a SAM that we can outrange
                tile_value += 100_000.0 * sam_level;
            }
        }
    }

    // Prefer tiles that are closer to a silo (but preserve structure value)
    let silo_tiles: Vec<TileRef> = silos.iter().map(|&u| st.unit(u).tile()).collect();
    let Some((closest_silo, _)) = closest_two_tiles(&st.map, &silo_tiles, &[tile]) else { ts_throw!("Missing result") };
    let distance_squared = d2(st, tile, closest_silo);
    let distance_to_closest_silo = js::sqrt(distance_squared);
    let distance_penalty = distance_to_closest_silo * 30.0;
    let base_tile_value = tile_value;
    tile_value = js::max(base_tile_value * 0.2, tile_value - distance_penalty); // Keep at least 20% of structure value

    // Don't target near recent targets
    tile_value -= nb
        .recently_sent_nukes
        .iter()
        .filter(|&&(_tick, recent_tile, recent_nuke_type)| {
            let recent_inner_radius = config.nuke_magnitudes(recent_nuke_type).inner;
            let dist_squared = d2(st, tile, recent_tile);
            dist_squared <= recent_inner_radius * recent_inner_radius
        })
        .map(|_| 1_000_000.0)
        .fold(0.0, |prev, cur| prev + cur);

    tile_value
}

impl NationExec {
    /// The nuke behaviour's data (`this.nukeBehavior`; a TS `TypeError` before
    /// `initializeBehaviors`). Borrows only `self.behaviors`, so `self.rng` stays free.
    #[inline]
    fn nuke_b(&mut self) -> &mut NukeB {
        &mut self.behaviors.as_mut().ts_expect("NationExecution.nukeBehavior before initializeBehaviors").nuke
    }

    /// `nukeBehavior.maybeSendNuke()` (`:174-251`).
    pub fn nuke_maybe_send_nuke(&mut self, cx: &mut Ctx) {
        let player = me(self);
        let silos = cx.st.player(player).units_of(&[UnitType::MissileSilo]);
        let config = cx.st.config();
        if silos.is_empty()
            || config.is_unit_disabled(UnitType::MissileSilo)
            || (config.is_unit_disabled(UnitType::AtomBomb) && config.is_unit_disabled(UnitType::HydrogenBomb))
        {
            return;
        }

        let Some((nuke_target, strike)) = self.nuke_choose_nuke_target(cx) else { return };

        if cx.st.player(nuke_target).player_type() == PlayerType::Bot // Don't nuke tribes (as opposed to nations and humans)
            || cx.st.player(player).is_on_same_team(nuke_target)
            || !AiAttackHooks::should_attack(self, cx, nuke_target)
        {
            return;
        }

        // While saving up for a MIRV, Hard & Impossible only nuke back at players hitting them,
        // with atom bombs
        let saving = self.nuke_is_saving_for_mirv(cx.st);
        if saving && !is_hitting_us(cx.st, player, nuke_target) {
            return;
        }

        let nuke_types = self.nuke_types_to_try(cx.st, saving);
        if nuke_types.is_empty() {
            return;
        }

        let structures = cx.st.player(nuke_target).units_of(unit_groups::STRUCTURES);
        let difficulty = difficulty(cx.st);
        // Use more random tiles on Impossible difficulty to improve chances of finding a perfect
        // SAM outranging spot
        let num_random_tiles = if difficulty == Difficulty::Impossible { 30 } else { 10 };
        let random_tiles = rand_territory_tile_array(&mut self.rng, cx.st, nuke_target, num_random_tiles);
        // TS: `new Set(randomTiles.concat(structures.map(u => u.tile())))` (`:221-223`): first
        // occurrence wins; iterated only (never mutated) below, so the insertion-ordered list
        // is the set.
        let mut seen: JsSet<TileRef> = JsSet::new();
        let mut candidates: Vec<TileRef> = Vec::new();
        for t in random_tiles.into_iter().chain(structures.iter().map(|&u| cx.st.unit(u).tile())) {
            if seen.insert(t) {
                candidates.push(t);
            }
        }
        let tick = cx.st.ticks();
        self.nuke_b().remove_old_nuke_events(tick);

        for nuke_type in nuke_types {
            let tile =
                self.nuke_find_best_nuke_tile(cx, nuke_type, nuke_target, &candidates, &silos, &structures, strike);
            if let Some(tile) = tile {
                self.nuke_send_nuke(cx, tile, nuke_type, nuke_target, 0);
                if strike && follows_up_strikes(cx.st) {
                    let tick = cx.st.ticks();
                    self.nuke_b().strikes.push(Strike { target: nuke_target, tile, tick, nuke: None });
                }
                return;
            }
        }
        if difficulty == Difficulty::Impossible && !saving {
            self.nuke_maybe_destroy_enemy_sam(cx, nuke_target);
        }
    }

    /// `nukeTypesToTry(atomOnly)` (`:254-275`): hydrogen bomb first; on Hard & Impossible, an
    /// atom bomb if no spot is worth a hydrogen bomb.
    fn nuke_types_to_try(&mut self, st: &State, atom_only: bool) -> Vec<UnitType> {
        let player = me(self);
        let config = st.config();
        let difficulty = config.game_config().difficulty;
        let mut types: Vec<UnitType> = Vec::new();
        if !atom_only
            && !config.is_unit_disabled(UnitType::HydrogenBomb)
            && PerceivedCost::from(st.player(player).gold())
                >= self.nuke_get_perceived_nuke_cost(st, UnitType::HydrogenBomb)
        {
            types.push(UnitType::HydrogenBomb);
        }
        if (types.is_empty() || hard_or_impossible(difficulty))
            && !config.is_unit_disabled(UnitType::AtomBomb)
            && PerceivedCost::from(st.player(player).gold())
                >= self.nuke_get_perceived_nuke_cost(st, UnitType::AtomBomb)
        {
            types.push(UnitType::AtomBomb);
        }
        types
    }

    /// `findBestNukeTile(nukeType, nukeTarget, candidates, silos, structures, strike)`
    /// (`:277-344`).
    #[allow(clippy::too_many_arguments)]
    fn nuke_find_best_nuke_tile(
        &mut self,
        cx: &mut Ctx,
        nuke_type: UnitType,
        nuke_target: SmallId,
        candidates: &[TileRef],
        silos: &[UnitRef],
        structures: &[UnitRef],
        strike: bool,
    ) -> Option<TileRef> {
        let player = me(self);
        let nb: &NukeB = self.nuke_b();
        let st: &State = cx.st;
        let difficulty = difficulty(st);
        let range = st.config().nuke_magnitudes(nuke_type).outer;
        let mut scored: Vec<(TileRef, f64)> = Vec::new();

        'outer: for &tile in candidates {
            // Cheap pre-filter, blastHitsFriendlyLand checks the whole blast of the winner
            let mut bounding_box = bounding_box_tiles(&st.map, tile, range);
            // Add radius / 2 in case there is a piece of unwanted territory inside the outer
            // radius that we miss.
            bounding_box.extend(bounding_box_tiles(&st.map, tile, js::floor(range / 2.0)));
            for &t in &bounding_box {
                if !is_valid_nuke_tile(st, player, t, nuke_target) {
                    continue 'outer;
                }
            }
            // TS: `canBuild(..) === false` → continue; a spawn tile of 0 is valid here.
            let Some(spawn_tile) = st.player(player).can_build(&mut *cx.sc, nuke_type, tile, None) else {
                continue;
            };

            // Above Easy, skip spots that are already being nuked or where the blast would be
            // mostly wasted
            let mut coverage = 0.0;
            if difficulty != Difficulty::Easy {
                if is_nuke_already_landing_near(st, tile, nuke_type) {
                    continue;
                }
                coverage = enemy_land_blast_coverage(st, player, tile, nuke_type);
                if coverage < min_blast_coverage(nuke_type) {
                    continue;
                }
            }

            // On Hard & Impossible, avoid trajectories that can be intercepted by enemy SAMs
            if hard_or_impossible(difficulty) && is_trajectory_interceptable_by_sam(st, player, spawn_tile, tile, None)
            {
                continue;
            }

            let value = nuke_tile_score(st, nb, tile, silos, structures, nuke_type);
            // Impossible only nukes spots with structure value, unless it's after the troops
            let skip = if difficulty == Difficulty::Impossible && !strike { value <= 0.0 } else { value < 0.0 };
            if skip {
                continue;
            }
            scored.push((tile, value + coverage * FULL_BLAST_COVERAGE_VALUE));
        }
        // Stable sort: on ties the earlier candidate wins
        of_det::js_sort_by(&mut scored, |a, b| b.1 - a.1); // TS: NationNukeBehavior.ts:337
        let margin = if strike { STRIKE_BORDER_MARGIN } else { 0.0 };
        let best = scored.iter().find(|&&(tile, _)| !blast_hits_friendly_land(st, player, tile, nuke_type, margin));
        best.map(|&(tile, _)| tile)
    }

    /// `chooseNukeTarget()` (`:347-367`): who to nuke, and whether it's a strike.
    fn nuke_choose_nuke_target(&mut self, cx: &mut Ctx) -> Option<(SmallId, bool)> {
        let target = self.nuke_find_best_nuke_target(cx)?;
        let strike = match difficulty(cx.st) {
            Difficulty::Easy => false,
            Difficulty::Medium => our_troops_attacking(cx.st, me(self), target) > 0.0,
            Difficulty::Hard | Difficulty::Impossible => self.nuke_can_strike(cx, target),
        };
        Some((target, strike))
    }

    /// `findBestNukeTarget()` (`:369-480`).
    pub fn nuke_find_best_nuke_target(&mut self, cx: &mut Ctx) -> Option<SmallId> {
        let player = me(self);
        // On Hard & Impossible with only 2 players left, target the only other one
        let diff = difficulty(cx.st);
        if hard_or_impossible(diff) && cx.st.players().len() == 2 {
            let other = cx.st.players().into_iter().find(|&p| p != player);
            if other.is_some() {
                return other;
            }
        }

        // Retaliate against incoming attacks (Most important!)
        if let Some(p) = find_incoming_attack_player(self, cx) {
            return Some(p);
        }

        if let Some(p) = find_incoming_nuke_player(cx.st, player) {
            return Some(p);
        }

        // In Teams, back up teammates in their fights before starting our own
        if let Some(p) = find_teammate_fight_target(cx.st, player) {
            return Some(p);
        }

        // On impossible difficulty, prioritize nuking the crown if they have more than 50% of
        // the map
        let gc = cx.st.config().game_config();
        if gc.difficulty == Difficulty::Impossible && gc.game_mode == GameMode::Ffa {
            let n = num_tiles_without_fallout(cx.st);
            if n > 0.0 {
                let sorted_by_tiles = players_sorted_by_tiles(cx.st);
                // TS: `const crown = sortedByTiles[0]; if (crown && …)`.
                if let Some(&crown) = sorted_by_tiles.first()
                    && crown != player
                    && !cx.st.player(player).is_friendly(crown, false)
                {
                    let crown_share = f64::from(cx.st.player(crown).num_tiles_owned()) / n;
                    if crown_share > CROWN_NEAR_WIN_SHARE {
                        return Some(crown);
                    }
                }
            }
        }

        // Soften up a neighbor about as strong as us before attacking it
        if let Some(p) = self.nuke_find_strike_target(cx) {
            return Some(p);
        }

        // On Impossible, the richest nation hunts very high structure density targets
        // Restricting to the richest nation prevents every impossible nation
        // from piling onto the same compact player.
        if diff == Difficulty::Impossible
            && is_richest_nation(cx.st, player)
            && self.rng.chance(2.0)
            && let Some(p) = find_high_density_target(cx.st, player)
        {
            return Some(p);
        }

        // Assist allies, check their targets (this is basically the same as in assistAllies,
        // but without sending emojis)
        let st = &*cx.st;
        let me_p = st.player(player);
        for ally in me_p.allies() {
            if st.player(ally).targets().is_empty() {
                continue;
            }
            if me_p.relation(ally) < Relation::Friendly {
                continue;
            }

            for target in st.player(ally).targets() {
                if target == player {
                    continue;
                }
                if me_p.is_friendly(target, false) {
                    continue;
                }
                // Found a valid ally target to nuke
                return Some(target);
            }
        }

        // Find the most hated player
        // Ignore much weaker players (we don't need nukes to deal with them)
        let my_max_troops = st.config().max_troops(&me_p);
        for (other, relation) in me_p.all_relations_sorted() {
            if relation != Relation::Hostile {
                continue;
            }
            if me_p.is_friendly(other, false) {
                continue;
            }

            let other_max_troops = st.config().max_troops(&st.player(other));
            if my_max_troops >= other_max_troops * 2.0 {
                continue;
            }

            return Some(other);
        }

        // In FFAs, nuke the crown if they're far enough ahead
        if let Some(p) = find_ffa_crown_target(st, player) {
            return Some(p);
        }

        // In Teams, nuke the strongest team
        if let Some(p) = self.nuke_find_strongest_team_target(cx.st) {
            return Some(p);
        }

        None
    }

    /// `findStrikeTarget()` (`:483-514`): a player not much weaker than us; Medium one it's
    /// attacking, Hard & Impossible the strongest neighbor they can follow up on.
    fn nuke_find_strike_target(&mut self, cx: &mut Ctx) -> Option<SmallId> {
        let player = me(self);
        let my_troops = cx.st.player(player).troops();
        let is_candidate = |st: &State, other: Owner| {
            other.is_player()
                && st.player(other).player_type() != PlayerType::Bot
                && !st.player(player).is_friendly(other, false)
                && st.player(other).troops() >= my_troops * STRIKE_MIN_TROOP_RATIO
        };

        match difficulty(cx.st) {
            Difficulty::Easy => None,
            Difficulty::Medium => {
                // iter-live: read-only loop over the live `outgoingAttacks()` array.
                for &a in cx.st.player(player).outgoing_attacks().as_slice() {
                    let target = cx.st.attack(a).target();
                    if is_candidate(cx.st, target) {
                        return Some(target);
                    }
                }
                None
            }
            Difficulty::Hard | Difficulty::Impossible => {
                let mut best: Option<SmallId> = None;
                // `nearby()` is a fresh array (memo copy); `canStrike` mutates nothing in it.
                for other in cx.st.nearby_memo(player) {
                    if !is_candidate(cx.st, other) {
                        continue;
                    }
                    if best.is_some_and(|b| cx.st.player(other).troops() <= cx.st.player(b).troops()) {
                        continue;
                    }
                    if self.nuke_can_strike(cx, other) {
                        best = Some(other);
                    }
                }
                best
            }
        }
    }

    /// `canStrike(target)` (`:517-525`): whether the attack right after our nuke could send
    /// enough troops into `target` by land. `&&` short-circuits in TS order.
    fn nuke_can_strike(&mut self, cx: &mut Ctx, target: SmallId) -> bool {
        let player = me(self);
        cx.st.player(target).player_type() != PlayerType::Bot
            && !cx.st.player(player).is_friendly(target, false)
            && self.attack_strike_troops(cx, target) >= cx.st.player(target).troops() * STRIKE_MIN_STACK_RATIO
            && cx.st.player(player).shares_border_with(target)
    }

    /// `nukeBehavior.followUpStrikes()` (`:535-568`): called every tick; attacks a strike's
    /// target the moment its bomb, or its MIRV's last warhead, lands.
    pub fn nuke_follow_up_strikes(&mut self, cx: &mut Ctx) {
        let player = me(self);
        if follows_up_strikes(cx.st) && cx.st.player(player).unit_count(UnitType::Mirv) > 0 {
            self.nuke_track_mirv_strikes(cx.st);
        }
        if self.nuke_b().strikes.is_empty() {
            return;
        }
        let mut pending: Vec<Strike> = Vec::new();
        // iter-live: `for (const strike of this.strikes)` over the live array, which the body
        // only mutates in place (`strike.nuke ??= …`); `attackBehavior.strike` never touches
        // it. Re-reading the length keeps JS's live-iterator semantics.
        let mut i = 0;
        while i < self.nuke_b().strikes.len() {
            // TS: `strike.nuke ??= this.findStrikeNuke(strike.tile)` (`:542`): writes back into
            // the array, so later `findStrikeNuke` calls see the claimed bomb.
            if self.nuke_b().strikes[i].nuke.is_none() {
                let tile = self.nuke_b().strikes[i].tile;
                let found = find_strike_nuke(cx.st, player, &self.nuke_b().strikes, tile);
                self.nuke_b().strikes[i].nuke = found;
            }
            let strike = self.nuke_b().strikes[i].clone();
            i += 1;
            let Some(nuke) = strike.nuke else {
                if cx.st.ticks() - strike.tick <= STRIKE_SPAWN_TICKS {
                    pending.push(strike);
                }
                continue;
            };
            if cx.st.unit(nuke).is_active() {
                pending.push(strike);
                continue;
            }
            if cx.st.unit(nuke).unit_type() == UnitType::Mirv {
                // A MIRV deletes itself when it splits into warheads
                if cx.st.player(player).unit_count(UnitType::MirvWarhead) > 0 {
                    pending.push(strike);
                    continue;
                }
            } else if !cx.st.unit(nuke).reached_target() {
                // Intercepted bombs leave the target's troops alone
                continue;
            }
            if cx.st.player(strike.target).is_alive() {
                self.attack_strike(cx, strike.target);
            }
        }
        self.nuke_b().strikes = pending;
    }

    /// `trackMirvStrikes()` (`:571-583`): our MIRVs at bordering players are strikes too.
    fn nuke_track_mirv_strikes(&mut self, st: &State) {
        let player = me(self);
        let tick = st.ticks();
        let nb = self.nuke_b();
        for mirv in st.player(player).units_of(&[UnitType::Mirv]) {
            if nb.strikes.iter().any(|s| s.nuke == Some(mirv)) {
                continue;
            }
            let u = st.unit(mirv);
            let target = u.target_player();
            let tile = u.target_tile();
            let (Some(target), Some(tile)) = (target.filter(|t| t.is_player()), tile) else { continue };
            if st.player(player).is_friendly(target, false) {
                continue;
            }
            if !st.player(player).shares_border_with(target) {
                continue;
            }
            nb.strikes.push(Strike { target, tile, tick, nuke: Some(mirv) });
        }
    }

    /// `findStrongestTeamTarget()` (`:757-838`).
    fn nuke_find_strongest_team_target(&mut self, st: &State) -> Option<SmallId> {
        let player = me(self);
        if st.config().game_config().game_mode != GameMode::Team {
            return None;
        }

        if st.players().len() <= 1 {
            return None;
        }

        // TS `Map<string, number>` iterated by `Array.from(entries())` (`:782`): JsMap.
        let mut team_tiles: JsMap<String, f64> = JsMap::new();
        // lookup-only: `teamPlayers.get/set` (`:774-777`, `:799`), never iterated.
        let mut team_players: crate::jsorder::LookupMap<String, Vec<SmallId>> = crate::jsorder::LookupMap::new();

        for p in st.players() {
            let Some(team) = st.player(p).team() else { continue };

            let prev = team_tiles.get(team).copied().unwrap_or(0.0); // TS: `?? 0`
            team_tiles.insert(team.clone(), prev + f64::from(st.player(p).num_tiles_owned()));
            match team_players.get_mut(team) {
                Some(players) => players.push(p),
                None => {
                    team_players.insert(team.clone(), vec![p]);
                }
            }
        }

        let mut sorted_teams: Vec<(String, f64)> = team_tiles.iter().map(|(k, &v)| (k.clone(), v)).collect();
        of_det::js_sort_by(&mut sorted_teams, |a, b| b.1 - a.1); // TS: NationNukeBehavior.ts:782

        if sorted_teams.is_empty() {
            return None;
        }

        let mut strongest_team = &sorted_teams[0].0;
        if Some(strongest_team) == st.player(player).team() {
            if sorted_teams.len() > 1 {
                strongest_team = &sorted_teams[1].0;
            } else {
                return None;
            }
        }

        let target_team_players =
            team_players.get(strongest_team).ts_expect("NationNukeBehavior.ts:799 teamPlayers.get(..)!");

        // Filter out friendly players
        let me_p = st.player(player);
        let valid_targets: Vec<SmallId> =
            target_team_players.iter().copied().filter(|&p| !me_p.is_friendly(p, false)).collect();

        if valid_targets.is_empty() {
            return None;
        }

        // Hard & Impossible: one of the three players with the most structure levels
        if hard_or_impossible(difficulty(st)) {
            let mut most_structures: Vec<(SmallId, f64)> = valid_targets
                .iter()
                .map(|&p| (p, structure_levels(st, p)))
                .filter(|&(_, levels)| levels > 0.0)
                .collect();
            of_det::js_sort_by(&mut most_structures, |a, b| b.1 - a.1); // TS: NationNukeBehavior.ts:819
            most_structures.truncate(3); // TS: .slice(0, 3)
            if !most_structures.is_empty() {
                return Some(rand_element(&mut self.rng, &most_structures).0);
            }
        }

        if self.rng.chance(2.0) {
            // Strongest player. TS: `validTargets.reduce((prev, current) => maxTroops(prev) >
            // maxTroops(current) ? prev : current)` with no initial value (`:828-833`).
            let config = st.config();
            let mut prev = valid_targets[0];
            for &current in &valid_targets[1..] {
                prev = if config.max_troops(&st.player(prev)) > config.max_troops(&st.player(current)) {
                    prev
                } else {
                    current
                };
            }
            Some(prev)
        } else {
            // Random player
            Some(rand_element(&mut self.rng, &valid_targets))
        }
    }

    /// `getPerceivedNukeCost(type)` (`:841-853`): each launch makes the next nuke of its type
    /// feel pricier, to save up for a MIRV and pace launches.
    fn nuke_get_perceived_nuke_cost(&mut self, st: &State, t: UnitType) -> PerceivedCost {
        let player = me(self);
        if pays_real_nuke_price(st, player) {
            return PerceivedCost::from(cost(st, player, t));
        }
        // Easy & Medium keep the price up for good, so only while a MIRV is worth saving for
        if !forgets_nuke_prices(st) && !is_mirv_worth_saving_for(st, player) {
            return PerceivedCost::from(cost(st, player, t));
        }
        self.nuke_inflated_nuke_cost(st, t)
    }

    /// `inflatedNukeCost(type)` (`:900-917`).
    fn nuke_inflated_nuke_cost(&mut self, st: &State, t: UnitType) -> PerceivedCost {
        let player = me(self);
        let real = cost(st, player, t);
        // `forgetsNukePrices()` is read only once `perceived > real` in TS (`:907`); it is pure,
        // so evaluating it up front changes nothing.
        let forgets = forgets_nuke_prices(st);
        let ticks = st.ticks();
        self.nuke_b().inflated_nuke_cost_with(t, real, forgets, ticks)
    }

    /// `isSavingForMirv()` (`:920-927`): Hard & Impossible in FFA, holding gold back for a MIRV
    /// their income will soon reach.
    fn nuke_is_saving_for_mirv(&mut self, st: &State) -> bool {
        let player = me(self);
        forgets_nuke_prices(st)
            && !pays_real_nuke_price(st, player)
            && is_mirv_worth_saving_for(st, player)
            && is_save_up_within_reach(st, player, mirv_save_up_target(st, player))
    }

    /// `sendNuke(tile, nukeType, targetPlayer, waitTicks = 0)` (`:1092-1116`).
    fn nuke_send_nuke(
        &mut self,
        cx: &mut Ctx,
        tile: TileRef,
        nuke_type: UnitType,
        target_player: SmallId,
        wait_ticks: i32,
    ) {
        let player = me(self);
        let tick = cx.st.ticks();
        // TS pushes to `recentlySentNukes` before computing `perceived` (`:1099-1100`); the
        // push does not affect `inflatedNukeCost`, so `record_send` does both in TS order.
        let perceived = self.nuke_inflated_nuke_cost(cx.st, nuke_type);
        self.nuke_b().record_send(tick, tile, nuke_type, perceived);
        // TS: `new NukeExecution(nukeType, this.player, tile, null, -1, waitTicks)`.
        cx.st.add_execution(Exec::Nuke(NukeExec::new(nuke_type, player, tile, Tri::Null, -1.0, wait_ticks, true)));
        self.emoji_maybe_send_emoji(cx, EmojiRecipient::Player(target_player), EMOJI_NUKE);
    }

    /// `maybeDestroyEnemySam(nukeTarget)` (`:1119-1154`): on Impossible, when no spot is worth a
    /// nuke, overwhelm one of the target's SAMs with atom bombs.
    fn nuke_maybe_destroy_enemy_sam(&mut self, cx: &mut Ctx, nuke_target: SmallId) {
        let player = me(self);
        if cx.st.config().is_unit_disabled(UnitType::AtomBomb) {
            return;
        }

        // Don't launch another salvo if we already have atom bombs in flight
        let our_atom_bombs = cx.st.player(player).units_of(&[UnitType::AtomBomb]);
        if !our_atom_bombs.is_empty() {
            return;
        }

        let (salvo, failed) = plan_sam_salvo(cx.st, player, nuke_target);
        if let Some(salvo) = salvo {
            for &wait_ticks in &salvo.wait_ticks {
                self.nuke_send_nuke(cx, salvo.target_tile, UnitType::AtomBomb, nuke_target, wait_ticks);
            }
            return;
        }

        // Couldn't destroy any SAM — upgrade silos only if capacity was the bottleneck.
        // If we only lack gold, don't waste it upgrading silos — just wait and save.
        let silo = failed.as_ref().and_then(|f| silo_to_upgrade_for_salvo(cx.st, player, f));
        if let Some(silo) = silo {
            // TS: `new UpgradeStructureExecution(this.player, silo.id())` (amount = 1).
            let id = f64::from(cx.st.unit(silo).id());
            cx.st.add_execution(Exec::UpgradeStructure(UpgradeStructureExec::new(player, id, 1.0)));
        }
    }
}

/// `followsUpStrikes()` (`:527-532`): Hard & Impossible.
fn follows_up_strikes(st: &State) -> bool {
    hard_or_impossible(difficulty(st))
}

/// `findStrikeNuke(tile)` (`:585-598`): our first atom/hydrogen bomb aimed at `tile` that no
/// strike has claimed yet.
fn find_strike_nuke(st: &State, player: SmallId, strikes: &[Strike], tile: TileRef) -> Option<UnitRef> {
    st.player(player)
        .units_of(&[UnitType::AtomBomb, UnitType::HydrogenBomb])
        .into_iter()
        .find(|&nuke| st.unit(nuke).target_tile() == Some(tile) && !strikes.iter().any(|s| s.nuke == Some(nuke)))
}

// ---- CS (`snapshot`, `:119-138`; `NationNukeBehavior v2`, unwrapped from `{v, d}`) ----

impl Canon for Strike {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        crate::canon_obj!(cx; "target" => self.target, "tile" => self.tile, "tick" => self.tick, "nuke" => self.nuke)
    }
}

impl Canon for NukeB {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        let recent = Cs::Arr(
            self.recently_sent_nukes
                .iter()
                .map(|&(tick, tile, t)| {
                    Cs::Arr(vec![Cs::Int(i64::from(tick)), Cs::Int(i64::from(tile)), Cs::Str(t.as_str().to_string())])
                })
                .collect(),
        );
        Cs::Obj(vec![
            ("recentlySentNukes".to_string(), recent),
            ("atomBombsLaunched".to_string(), self.atom_bombs_launched.canon(cx)),
            ("atomBombPerceivedCost".to_string(), Cs::Big(self.atom_bomb_perceived_cost)),
            ("atomBombLastLaunch".to_string(), self.atom_bomb_last_launch.canon(cx)),
            ("hydrogenBombsLaunched".to_string(), self.hydrogen_bombs_launched.canon(cx)),
            ("hydrogenBombPerceivedCost".to_string(), Cs::Big(self.hydrogen_bomb_perceived_cost)),
            ("hydrogenBombLastLaunch".to_string(), self.hydrogen_bomb_last_launch.canon(cx)),
            ("strikes".to_string(), self.strikes.canon(cx)),
        ])
    }
}

fn field<'v>(fields: &'v [(String, Cs)], k: &str) -> Result<&'v Cs, CanonError> {
    fields
        .iter()
        .find(|(n, _)| n == k)
        .map(|(_, v)| v)
        .ok_or_else(|| CanonError::new(format!("NationNukeBehavior: missing {k}")))
}

fn as_obj<'v>(v: &'v Cs, what: &str) -> Result<&'v [(String, Cs)], CanonError> {
    match v {
        Cs::Obj(f) => Ok(f),
        _ => Err(CanonError::new(format!("{what}: expected obj, got {v:?}"))),
    }
}

/// A perceived-cost bigint from CS (the full i128 that `Cs::Big` holds; ledger t196).
fn big_i128(v: &Cs, what: &str) -> Result<PerceivedCost, CanonError> {
    match v {
        Cs::Big(b) => Ok(*b),
        _ => v
            .as_i64()
            .map(PerceivedCost::from)
            .ok_or_else(|| CanonError::new(format!("{what}: expected bigint, got {v:?}"))),
    }
}

/// `restoreSnapshot` (`:141-172`). A strike's `nuke` is a unit key (`unit.id()`); it resolves
/// through `unit_by_id` (live units only) until P13's loader provides a resolver for
/// TS-deleted-but-referenced units (docs/requests/P11.md).
impl Uncanon for NukeB {
    fn uncanon(v: &Cs, cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        let f = as_obj(v, "NationNukeBehavior")?;
        let mut recently_sent_nukes = Vec::new();
        for e in field(f, "recentlySentNukes")?.as_arr().ok_or_else(|| CanonError::new("recentlySentNukes"))? {
            let a = e.as_arr().filter(|a| a.len() == 3).ok_or_else(|| CanonError::new("recentlySentNukes entry"))?;
            let tick = Tick::uncanon(&a[0], cx)?;
            let tile = TileRef::uncanon(&a[1], cx)?;
            let t = a[2]
                .as_str()
                .and_then(UnitType::from_ts)
                .filter(|t| matches!(t, UnitType::AtomBomb | UnitType::HydrogenBomb))
                .ok_or_else(|| CanonError::new("recentlySentNukes type"))?;
            recently_sent_nukes.push((tick, tile, t));
        }
        let mut strikes = Vec::new();
        for s in field(f, "strikes")?.as_arr().ok_or_else(|| CanonError::new("strikes"))? {
            let sf = as_obj(s, "Strike")?;
            let nuke_v = field(sf, "nuke")?;
            let nuke = if nuke_v.is_null() {
                None
            } else {
                let id = u32::uncanon(nuke_v, cx)?;
                Some(cx.st.unit_by_id(id).ok_or_else(|| CanonError::new(format!("Strike.nuke: unit {id} not live")))?)
            };
            strikes.push(Strike {
                target: SmallId::uncanon(field(sf, "target")?, cx)?,
                tile: TileRef::uncanon(field(sf, "tile")?, cx)?,
                tick: Tick::uncanon(field(sf, "tick")?, cx)?,
                nuke,
            });
        }
        Ok(NukeB {
            recently_sent_nukes,
            atom_bombs_launched: i32::uncanon(field(f, "atomBombsLaunched")?, cx)?,
            atom_bomb_perceived_cost: big_i128(field(f, "atomBombPerceivedCost")?, "atomBombPerceivedCost")?,
            atom_bomb_last_launch: Option::<Tick>::uncanon(field(f, "atomBombLastLaunch")?, cx)?,
            hydrogen_bombs_launched: i32::uncanon(field(f, "hydrogenBombsLaunched")?, cx)?,
            hydrogen_bomb_perceived_cost: big_i128(
                field(f, "hydrogenBombPerceivedCost")?,
                "hydrogenBombPerceivedCost",
            )?,
            hydrogen_bomb_last_launch: Option::<Tick>::uncanon(field(f, "hydrogenBombLastLaunch")?, cx)?,
            strikes,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::exec::ExecStore;

    fn nb(atom: PerceivedCost, hydro: PerceivedCost) -> NukeB {
        NukeB { atom_bomb_perceived_cost: atom, hydrogen_bomb_perceived_cost: hydro, ..NukeB::default() }
    }

    /// `removeOldNukeEvents`: `sent + 600 < tick` is dropped, `== tick` kept, front-first only.
    #[test]
    fn old_nuke_events_shift_from_the_front() {
        let mut b = NukeB {
            recently_sent_nukes: vec![
                (100, 1, UnitType::AtomBomb),
                (400, 2, UnitType::HydrogenBomb),
                (50, 3, UnitType::AtomBomb),
            ],
            ..NukeB::default()
        };
        b.remove_old_nuke_events(700);
        // 100 + 600 = 700 is not < 700: kept, and the later (older) 50 stays behind it.
        assert_eq!(b.recently_sent_nukes.len(), 3);
        b.remove_old_nuke_events(701);
        assert_eq!(b.recently_sent_nukes, vec![(400, 2, UnitType::HydrogenBomb), (50, 3, UnitType::AtomBomb)]);
        b.remove_old_nuke_events(5000);
        assert!(b.recently_sent_nukes.is_empty());
    }

    /// `inflatedNukeCost`: real when perceived <= real; perceived when prices are not
    /// forgotten; otherwise decays linearly over 3000 ticks with bigint truncation.
    #[test]
    fn inflated_cost_decay_is_bigint_exact() {
        let real: Gold = 750_000;
        let real_big = PerceivedCost::from(real);
        let mut b = nb(1_125_000, 5_000_000);
        // Not above real → real.
        assert_eq!(nb(700_000, 0).inflated_nuke_cost_with(UnitType::AtomBomb, real, true, 10), real_big);
        // Easy/Medium (not forgetting) keep the full perceived price.
        assert_eq!(b.inflated_nuke_cost_with(UnitType::AtomBomb, real, false, 10), 1_125_000);
        // Never launched: left = 0 → real.
        assert_eq!(b.inflated_nuke_cost_with(UnitType::AtomBomb, real, true, 10), real_big);
        b.atom_bomb_last_launch = Some(1000);
        // left = 3000 - 1 = 2999: 750000 + 375000*2999/3000 = 750000 + 374875 (exact).
        assert_eq!(b.inflated_nuke_cost_with(UnitType::AtomBomb, real, true, 1001), 1_124_875);
        // left = 7: 375000*7/3000 = 875 exactly; left = 1: 125.
        assert_eq!(b.inflated_nuke_cost_with(UnitType::AtomBomb, real, true, 3993), 750_875);
        assert_eq!(b.inflated_nuke_cost_with(UnitType::AtomBomb, real, true, 3999), 750_125);
        // Truncation: (1_000_001 - 1)*1/3000 = 333.33 → 333.
        let mut c = nb(1_000_001, 0);
        c.atom_bomb_last_launch = Some(0);
        assert_eq!(c.inflated_nuke_cost_with(UnitType::AtomBomb, 1, true, 2999), 1 + 333);
        // left <= 0 → real.
        assert_eq!(b.inflated_nuke_cost_with(UnitType::AtomBomb, real, true, 4000), real_big);
        // The hydrogen fields are separate.
        b.hydrogen_bomb_last_launch = Some(1000);
        assert_eq!(b.inflated_nuke_cost_with(UnitType::HydrogenBomb, 5_000_000, true, 1001), 5_000_000);
    }

    /// `sendNuke` bookkeeping: ×150/100 (atom) and ×125/100 (hydrogen) on the inflated price,
    /// bigint-truncated, plus the launch tick.
    #[test]
    fn send_bookkeeping_inflates_by_type() {
        let mut b = NukeB::default();
        b.record_send(42, 7, UnitType::AtomBomb, 1_000_001);
        assert_eq!(
            (b.atom_bombs_launched, b.atom_bomb_perceived_cost, b.atom_bomb_last_launch),
            (1, 1_500_001, Some(42))
        );
        b.record_send(43, 8, UnitType::HydrogenBomb, 5_000_003);
        assert_eq!(
            (b.hydrogen_bombs_launched, b.hydrogen_bomb_perceived_cost, b.hydrogen_bomb_last_launch),
            (1, 6_250_003, Some(43))
        );
        assert_eq!(b.recently_sent_nukes, vec![(42, 7, UnitType::AtomBomb), (43, 8, UnitType::HydrogenBomb)]);
    }

    /// CS: dump order, bigint costs as BIG, union type strings, `null` launches and nukes.
    #[test]
    fn canon_matches_the_v2_schema_and_round_trips() {
        let st = State::default();
        let ex = ExecStore::default();
        let mut b = nb(1_500_000, 6_250_000);
        b.recently_sent_nukes = vec![(42, 7, UnitType::AtomBomb), (43, 8, UnitType::HydrogenBomb)];
        b.atom_bombs_launched = 1;
        b.atom_bomb_last_launch = Some(42);
        b.strikes = vec![Strike { target: SmallId(3), tile: 11, tick: 40, nuke: None }];
        let cs = b.canon(&mut CanonCx::new(&st, &ex));
        let want = Cs::Obj(vec![
            (
                "recentlySentNukes".into(),
                Cs::Arr(vec![
                    Cs::Arr(vec![Cs::Int(42), Cs::Int(7), Cs::Str("Atom Bomb".into())]),
                    Cs::Arr(vec![Cs::Int(43), Cs::Int(8), Cs::Str("Hydrogen Bomb".into())]),
                ]),
            ),
            ("atomBombsLaunched".into(), Cs::Int(1)),
            ("atomBombPerceivedCost".into(), Cs::Big(1_500_000)),
            ("atomBombLastLaunch".into(), Cs::Int(42)),
            ("hydrogenBombsLaunched".into(), Cs::Int(0)),
            ("hydrogenBombPerceivedCost".into(), Cs::Big(6_250_000)),
            ("hydrogenBombLastLaunch".into(), Cs::Null),
            (
                "strikes".into(),
                Cs::Arr(vec![Cs::Obj(vec![
                    ("target".into(), Cs::Int(3)),
                    ("tile".into(), Cs::Int(11)),
                    ("tick".into(), Cs::Int(40)),
                    ("nuke".into(), Cs::Null),
                ])]),
            ),
        ]);
        assert_eq!(cs, want);
        let mut st2 = State::default();
        let back = NukeB::uncanon(
            &cs,
            &mut UncanonCx { st: &mut st2, keys: &crate::canon::LoadKeys { n_players: 8, ..Default::default() } },
        )
        .unwrap();
        assert_eq!(back.recently_sent_nukes, b.recently_sent_nukes);
        assert_eq!(back.strikes, b.strikes);
        assert_eq!(back.canon(&mut CanonCx::new(&st, &ex)), cs);
    }

    /// `sent + 600 < tick` is a JS number add: no i32 wrap next to `i32::MAX` (Grok review).
    #[test]
    fn old_nuke_events_near_i32_max_do_not_wrap() {
        let mut b = NukeB { recently_sent_nukes: vec![(i32::MAX - 599, 1, UnitType::AtomBomb)], ..NukeB::default() };
        b.remove_old_nuke_events(i32::MAX);
        assert_eq!(b.recently_sent_nukes.len(), 1);
    }

    /// A perceived price past i128 is a loud Rust-only failure (not a `TsThrow`, not a wrap).
    #[test]
    #[allow(clippy::disallowed_methods)] // asserting the panic class
    fn perceived_cost_overflow_is_loud() {
        let mut b = NukeB::default();
        let r = std::panic::catch_unwind(move || b.record_send(1, 1, UnitType::AtomBomb, i128::MAX / 100 * 70));
        let Err(p) = r else { panic!("must panic") };
        assert!(p.downcast_ref::<crate::ctx::TsThrow>().is_none());
    }

    /// The values that stopped World FFA rollouts (`bigint 3804992140530486 * 2952
    /// outside i64`, at tick 30,776) are now exact JS BigInt results. Expected
    /// values are TS BigInt arithmetic, written out: `750000n + (3804992140530486n - 750000n) *
    /// 2952n / 3000n` = 3744112266293998n (node -p), and the send `* 150n / 100n` of it.
    #[test]
    fn perceived_cost_past_i64_is_exact() {
        let real: Gold = 750_000;
        let p: PerceivedCost = 3_804_992_140_530_486;
        let b = NukeB { atom_bomb_perceived_cost: p, atom_bomb_last_launch: Some(1000), ..NukeB::default() };
        // left = 3000 - (1048 - 1000) = 2952
        let v = b.inflated_nuke_cost_with(UnitType::AtomBomb, real, true, 1048);
        assert_eq!(v, 3_744_112_266_293_998);
        assert_eq!(v, PerceivedCost::from(real) + (p - PerceivedCost::from(real)) * 2952 / 3000);
        let mut b2 = b;
        b2.record_send(1048, 9, UnitType::AtomBomb, v);
        assert_eq!(b2.atom_bomb_perceived_cost, 5_616_168_399_440_997); // 3744112266293998n * 150n / 100n (node -p)
        // far past i64: the split form equals the direct i128 form wherever the latter fits
        for &a in &[i128::from(i64::MAX) * 3 + 7, 10i128.pow(30) + 12_345, i128::MAX / 3001] {
            for &(m, d) in &[(150, 100), (125, 100), (2952, 3000), (1, 3000), (3000, 3000)] {
                assert_eq!(mul_div_nonneg(a, m, d), a * m / d, "{a} * {m} / {d}");
            }
        }
        // and where the product itself leaves i128 but the result does not
        let a = i128::MAX / 2;
        assert_eq!(mul_div_nonneg(a, 2952, 3000), a / 3000 * 2952 + (a % 3000) * 2952 / 3000);
        // CS export is the exact bigint (BIGINT tag, i128 payload), and it round-trips
        let b3 = NukeB { atom_bomb_perceived_cost: 10i128.pow(25) + 1, ..NukeB::default() };
        assert_eq!(big_i128(&Cs::Big(b3.atom_bomb_perceived_cost), "x").unwrap(), 10i128.pow(25) + 1);
    }

    /// t196: CS v1 encodes a perceived price beyond i64 byte-identically to TS. The rows are TS
    /// `conformance/src/cse.ts` `encode({ atomBombPerceivedCost: v })` and `fnv64w` of it, run on
    /// the TS harness (2026-11-06): BIGINT = zigzag LEB128 of the arbitrary-size bigint.
    #[test]
    fn perceived_cost_cse_matches_ts_beyond_i64() {
        let rows: &[(&str, &str, &str)] = &[
            (
                "9223372036854775808",
                "09011561746f6d426f6d62506572636569766564436f73740680808080808080808002",
                "f8694dacda572147",
            ),
            (
                "-9223372036854775809",
                "09011561746f6d426f6d62506572636569766564436f73740681808080808080808002",
                "52ee4da7f9247647",
            ),
            (
                "11414976421591458000",
                "09011561746f6d426f6d62506572636569766564436f737406a083d986f1a492eabc02",
                "8596f32513d62a33",
            ),
            (
                "10000000000000000000000001",
                "09011561746f6d426f6d62506572636569766564436f737406828080a089d28094acd4ac8401",
                "9f3abaaca41a4d4a",
            ),
            (
                "-1000000000000000000000000000000",
                "09011561746f6d426f6d62506572636569766564436f737406ffffffffc7faf6f48cc1e6c9e5a706",
                "b9eb9f19647f867a",
            ),
            (
                "85070591730234615865843651857942065209",
                "09011561746f6d426f6d62506572636569766564436f737406f2c08180808080808080808080808080808002",
                "4c103547db575d48",
            ),
            (
                "170141183460469231731687303715884105727",
                "09011561746f6d426f6d62506572636569766564436f737406feffffffffffffffffffffffffffffffffff03",
                "d9ae6657e13225a4",
            ),
            (
                "-170141183460469231731687303715884105728",
                "09011561746f6d426f6d62506572636569766564436f737406ffffffffffffffffffffffffffffffffffff03",
                "1d14778749c7c2a4",
            ),
        ];
        for &(v, hex, fnv) in rows {
            let x: PerceivedCost = v.parse().unwrap();
            let cs = Cs::Obj(vec![("atomBombPerceivedCost".to_string(), Cs::Big(x))]);
            let b = of_canon::cse::encode(&cs).unwrap();
            assert_eq!(of_canon::cse::hex(&b), hex, "{v}");
            assert_eq!(format!("{:016x}", of_canon::fnv::fnv64w(&b)), fnv, "{v}");
            assert_eq!(of_canon::cse::decode(&b).unwrap(), cs);
        }
    }

    #[test]
    fn blast_coverage_table() {
        assert_eq!(min_blast_coverage(UnitType::AtomBomb), 0.25);
        assert_eq!(min_blast_coverage(UnitType::HydrogenBomb), 0.4);
        assert_eq!(HIGH_DENSITY_NUKE_THRESHOLD, 1.0 / 75.0);
    }
}
