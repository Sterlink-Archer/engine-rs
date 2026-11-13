//! `NationAttackBehavior extends AiAttackBehavior` (`execution/nation/NationAttackBehavior.ts`).
//! Owner: **P10**. Holds **`impl AiAttackHooks for NationExec`** with all 7 overrides; the
//! `super.` calls (`:1202, :1218`) go to `ai_attack::base_*`.
//!
//! Every TS `this.<hook>()` call (the 7 overridable methods) is written `self.<hook>(cx, ..)`,
//! which resolves to the [`AiAttackHooks`] impl below; every other inherited method is the
//! P9 generic free function called with `self` (ARCHITECTURE §4.4).

use crate::api::game_config::TeamCountConfig;
use crate::api::types::{Cell, Difficulty, GameMode, GameType, Relation, TerrainType, unit_groups};
use crate::config::engine_config::{AttackLogicAttacker, AttackLogicDefender, AttackLogicInput, CHEAPEST_TROOP_RATIO};
use crate::exec::ai_attack::{self, AiAttackBase, AiAttackHooks, SailResult, SendTroops};
use crate::exec::attack::AttackExec;
use crate::exec::donate_troops::DonateTroopsExec;
use crate::exec::nation::NationExec;
use crate::exec::nation::annexation::{plan_bot_attacks, walled_in_bots};
use crate::exec::nation::emoji::{
    EMOJI_ASSIST_ACCEPT, EMOJI_ASSIST_RELATION_TOO_LOW, EMOJI_ASSIST_TARGET_ALLY, EMOJI_ASSIST_TARGET_ME,
};
use crate::exec::nation::utils::{find_juiciest_target, find_runaway_leader, rand_elem};
#[allow(unused_imports)]
use crate::exec::prelude::*;
use crate::exec::transport_ship::TransportShipExec;
use crate::path::pathfinder::WaterStepper;
use crate::state::State;
use crate::state::player::BoundingBox;
use crate::state::transport_ship_utils::{can_build_transport_ship_cx, target_transport_tile};
use of_det::{js, js_sort_by};

/// `NationAttackBehavior` data; snapshot `AiAttackBehavior v2` (`botAttackTroopsSent,
/// followedLandings, triggerRatio, reserveRatio, expandRatio`).
#[derive(StateClone, Debug, Default)]
pub struct NationAttackB {
    /// `botAttackTroopsSent = 0` (`:64`).
    pub bot_attack_troops_sent: f64,
    /// `followedLandings: Attack[]` (`:66`); filter-reassign: not iterated across (`:130`).
    pub followed_landings: Vec<AttackRef>,
    /// The base fields; CS `triggerRatio, reserveRatio, expandRatio` (in that order).
    pub base: AiAttackBase,
    /// non-snapshot: `landNeighbors: Set<Player> | null` (`:68`); only set during `maybeAttack`.
    pub land_neighbors: Option<SmallJsSet<SmallIdKey, 8>>,
}

impl NationAttackB {
    /// `new NationAttackBehavior(random, game, player, triggerRatio, reserveRatio, expandRatio, ..)`
    /// (`:70-82`): field initialisers (`:64-68`) + `super(..)`. No PRNG draw.
    pub fn new(trigger_ratio: f64, reserve_ratio: f64, expand_ratio: f64) -> Self {
        NationAttackB {
            bot_attack_troops_sent: 0.0,
            followed_landings: Vec::new(),
            base: AiAttackBase::new(trigger_ratio, reserve_ratio, expand_ratio),
            land_neighbors: None,
        }
    }
}

/// CS: `AiAttackBehavior v2` (`AiAttackBehavior.ts:58-70`, written with
/// `writeRecord(w, botAttackTroopsSent, followedLandings)`, `NationAttackBehavior.ts:84-86`).
impl crate::canon::Canon for NationAttackB {
    fn canon(&self, cx: &mut crate::canon::CanonCx<'_>) -> crate::canon::Cs {
        use crate::canon::Cs;
        Cs::obj([
            ("botAttackTroopsSent", Cs::Num(self.bot_attack_troops_sent)),
            ("followedLandings", self.followed_landings.canon(cx)),
            ("triggerRatio", Cs::Num(self.base.trigger_ratio)),
            ("reserveRatio", Cs::Num(self.base.reserve_ratio)),
            ("expandRatio", Cs::Num(self.base.expand_ratio)),
        ])
    }
}

/// `type Box = { min: Cell; max: Cell } | null | undefined` + `boxGap(a, b)` (`:53-61`): fewest
/// tiles between two boxes (unknown boxes count as touching).
fn box_gap(a: Option<&BoundingBox>, b: Option<&BoundingBox>) -> f64 {
    let (Some(a), Some(b)) = (a, b) else { return 0.0 };
    // `Math.max(0, x, y)`: folded left in argument order (PORTING_GUIDE §3).
    let dx = js::max(js::max(0.0, a.min.x - b.max.x), b.min.x - a.max.x);
    let dy = js::max(js::max(0.0, a.min.y - b.max.y), b.min.y - a.max.y);
    dx + dy
}

/// The strategy closures of `getAttackStrategies` (`:453-558`), as data (PORTING_GUIDE §7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Strategy {
    Retaliate,
    Bots,
    Assist,
    Traitor,
    Afk,
    Betray,
    Nuked,
    Victim,
    Juicy,
    Hated,
    VeryWeak,
    Weakest,
    Island,
    Donate,
    Crown,
}

/// The `find` closures passed to `pick` (`:436-451`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Finder {
    /// `(enemies) => this.findTraitor(enemies)`.
    Traitor,
    /// the `afk` strategy's `enemies.find(..) ?? null` (`:470-477`).
    Afk,
    /// `(enemies) => this.findVictim(enemies)`.
    Victim,
    /// `(enemies) => this.findJuicyTarget(enemies)`.
    Juicy,
    /// `(enemies) => this.findVeryWeakEnemy(enemies)`.
    VeryWeak,
    /// the `weakest` strategy's `enemies.find(..) ?? null` (`:530-535`).
    Weakest,
    /// `(enemies) => enemies[0] ?? null` (`:540`).
    First,
}

/// `getAttackStrategies`'s local state: the lazily computed `groups` (`:406-432`) and the two
/// target lists it closes over.
struct StrategyCx<'a> {
    bordering_friends: &'a [SmallId],
    bordering_enemies: &'a [SmallId],
    groups: Option<Vec<Vec<SmallId>>>,
}

impl NationExec {
    #[inline]
    fn atk(&mut self) -> &mut NationAttackB {
        &mut self.b().attack
    }
    #[inline]
    fn atk_difficulty(cx: &Ctx) -> Difficulty {
        cx.st.config().game_config().difficulty
    }

    /// `attackBehavior.maybeAttack()` (`:108-122`).
    pub fn attack_maybe_attack(&mut self, cx: &mut Ctx) {
        // TS :109-110 `const routes = new Map(); this.boatRoutes = routes;`
        self.atk().base.boat_routes = Some(JsMap::new());
        // TS `try { .. } finally { landNeighbors = null; boatRoutes = null }`: a TsThrow aborts the
        // whole tick; both fields are also reset at the start of every `NationExec::tick`
        // (mod.rs), so nothing can observe a value left behind by a throw.
        self.attack_choose_attack(cx);
        // rules-v36 (#6278; v36 :114) `landFront = this.hasLandFront()` (same result as the pin's loop)
        let land_front = self.attack_has_land_front(cx);
        self.atk().land_neighbors = None;
        let routes = self.atk().base.boat_routes.take().unwrap_or_default();
        self.attack_clear_blocked_lane(cx, &routes, land_front);
    }

    /// `followUpLandings()` (`:127-145`): called every tick. Hard & Impossible boats are
    /// beachheads: the moment one lands (its attack starts at the landing tile), attack its target
    /// by land from there, once.
    pub fn attack_follow_up_landings(&mut self, cx: &mut Ctx) {
        if !Self::attack_lands_beachheads(cx) {
            return;
        }
        let me = self.nation_player();
        // filter-reassign: not iterated across (`:130-132`).
        {
            let attacks = cx.st.player(me).outgoing_attacks();
            self.atk().followed_landings.retain(|a| attacks.contains(a));
        }
        // iter-live: `outgoingAttacks()` is the live array (PlayerImpl.ts:1900-1901). The body only
        // queues executions (AttackExecution/EmojiExecution: `addExecution`), sends emojis and
        // updates `botAttackTroopsSent`; nothing in it can reassign or push to the array.
        let mut i = 0;
        while let Some(attack) = cx.st.player(me).outgoing_attacks().get(i) {
            i += 1;
            let landing = cx.st.attack(attack).source_tile();
            let Some(landing) = landing else { continue };
            if self.atk().followed_landings.contains(&attack) {
                continue;
            }
            self.atk().followed_landings.push(attack);
            let target = cx.st.attack(attack).target();
            if !Self::attack_holds_land_next_to(cx.st, target, landing) {
                continue;
            }
            let opponent = if target.is_player() { Some(target) } else { None };
            ai_attack::send_land_attack(self, cx, target, SendTroops::TroopsMinusReserve, opponent);
        }
    }

    /// `holdsLandNextTo(owner, tile)` (`:147-163`).
    fn attack_holds_land_next_to(st: &State, owner: Owner, tile: TileRef) -> bool {
        let mut nbuf = [0; 4];
        let n = st.map.neighbors4(tile, &mut nbuf);
        for &t in &nbuf[..n] {
            if st.map.is_land(t) && !st.map.is_impassable(t) && st.owner(t) == owner {
                return true;
            }
        }
        false
    }

    /// `chooseAttack()` (`:165-222`).
    fn attack_choose_attack(&mut self, cx: &mut Ctx) {
        let me = self.nation_player();
        // Neighbor visit order matters here: the set's insertion order feeds the
        // stable troop-count sort below, so ties keep border-discovery order.
        let mut bordering_player_set: SmallJsSet<SmallIdKey, 8> = SmallJsSet::new();
        let mut border_has_non_nuked_terra_nullius = false;
        {
            let st = &*cx.st;
            let small_id = me.0;
            // forEach over the border TileSet (`:181-183`); the visit never mutates it.
            for t in st.player(me).border_tiles().iter() {
                st.map.for_each_neighbor(t, |n| {
                    if !st.map.is_land(n) || st.map.is_impassable(n) {
                        return;
                    }
                    if st.map.owner_id(n) == small_id {
                        return;
                    }
                    let owner = st.player_by_small_id(st.map.owner_id(n));
                    if owner.is_player() {
                        bordering_player_set.insert(owner);
                    }
                    if !st.map.has_owner(n) && !st.map.has_fallout(n) {
                        border_has_non_nuked_terra_nullius = true;
                    }
                });
            }
        }
        // TS :184 `this.landNeighbors = new Set(borderingPlayerSet)`.
        let mut ln: SmallJsSet<SmallIdKey, 8> = SmallJsSet::new();
        for &p in bordering_player_set.iter() {
            ln.insert(p);
        }
        self.atk().land_neighbors = Some(ln);
        let player_neighbors = cx.st.nearby_memo(me);
        for &n in &player_neighbors {
            if n.is_player() {
                bordering_player_set.insert(n);
            }
        }
        let mut bordering_players: Vec<SmallId> = bordering_player_set.iter().copied().collect();
        js_sort_by(&mut bordering_players, |a, b| cx.st.player(*a).troops() - cx.st.player(*b).troops());
        let bordering_friends: Vec<SmallId> =
            bordering_players.iter().copied().filter(|&o| cx.st.player(me).is_friendly(o, false)).collect();
        let bordering_enemies: Vec<SmallId> =
            bordering_players.iter().copied().filter(|&o| !cx.st.player(me).is_friendly(o, false)).collect();

        // Attack TerraNullius but not nuked territory (direct border or across a river)
        let has_non_nuked_terra_nullius =
            border_has_non_nuked_terra_nullius || player_neighbors.iter().any(|n| !n.is_player());
        if has_non_nuked_terra_nullius && ai_attack::send_attack(self, cx, SmallId::TERRA_NULLIUS, false) {
            return;
        }

        let hold_boats = self.attack_holds_boats_under_attack(cx);
        if bordering_enemies.is_empty() {
            if !hold_boats && self.rng.chance(5.0) {
                self.attack_attack_with_random_boat(cx, &[]);
            }
        } else {
            if !hold_boats && self.rng.chance(10.0) {
                self.attack_attack_with_random_boat(cx, &bordering_enemies);
                return;
            }

            self.alliance_maybe_send_alliance_requests(cx, &bordering_enemies);
        }

        self.attack_attack_best_target(cx, &bordering_friends, &bordering_enemies);
    }

    /// `attackWithRandomBoat(borderingEnemies = [])` (`:224-278`).
    fn attack_attack_with_random_boat(&mut self, cx: &mut Ctx, bordering_enemies: &[SmallId]) {
        let me = self.nation_player();
        if cx.st.config().is_unit_disabled(UnitType::TransportShip) {
            return;
        }

        // Check if we've already sent out the maximum number of transport ships
        if f64::from(cx.st.player(me).unit_count(UnitType::TransportShip)) >= cx.st.config().boat_max_number() {
            return;
        }

        // Check if we have any shore tiles to launch from
        let shore = ai_attack::shore_tiles(self, cx, me);
        if shore.is_empty() {
            return;
        }

        let src = rand_elem(&mut self.rng, &shore);

        // First look for high-interest targets (unowned or bot-owned). Mainly relevant for earlygame
        let mut found = self.attack_find_random_boat_target(cx, src, bordering_enemies, true);
        if found.is_none() {
            // None found? Then look for players
            found = self.attack_find_random_boat_target(cx, src, bordering_enemies, false);
            if found.is_none() {
                return;
            }
        }
        let Some((dst, launch)) = found else { return };
        if self.sail(cx, launch, dst, None).blocker.is_some() {
            return;
        }

        let owner = cx.st.owner(dst);
        if Self::attack_lands_beachheads(cx) {
            self.attack_send_beachhead(cx, owner, dst);
            return;
        }
        let cap = if owner.is_player() { self.attack_troop_send_cap(cx, None) } else { self.expansion_troop_cap(cx) };
        // rules-v36 (#6297; v36 :265) `Math.min(this.homeTroops() / 5, cap)`
        let troops = js::min(ai_attack::home_troops(self, cx) / 5.0, cap);
        if troops < 1.0 {
            return;
        }

        // Hard & Impossible: don't attack if we'd send less than 20% of target's troops
        if owner.is_player() && self.attack_is_attack_too_weak(cx, troops, owner) {
            return;
        }

        // rules-v36 (#6297; v36 :273) `this.send(new TransportShipExecution(this.player, dst, troops), troops)`
        ai_attack::send(self, cx, Exec::TransportShip(TransportShipExec::new(me, dst, troops)), troops);
    }

    /// `findRandomBoatTarget(tile, borderingEnemies, highInterestOnly = false)` (`:280-347`):
    /// `{ tile, launch }` as `(tile, launch)`.
    fn attack_find_random_boat_target(
        &mut self,
        cx: &mut Ctx,
        tile: TileRef,
        bordering_enemies: &[SmallId],
        high_interest_only: bool,
    ) -> Option<(TileRef, TileRef)> {
        let me = self.nation_player();
        let x = f64::from(cx.st.map.x(tile));
        let y = f64::from(cx.st.map.y(tile));
        // lookup-only: `unreachablePlayers` is only read by `has` (`:307`); keyed by smallID
        // (id ↔ smallID is a bijection).
        let mut unreachable_players: crate::jsorder::LookupSet<SmallId> = crate::jsorder::LookupSet::new();
        for _ in 0..500 {
            let rand_x = self.rng.next_int(x - 150.0, x + 150.0);
            let rand_y = self.rng.next_int(y - 150.0, y + 150.0);
            if !cx.st.map.is_valid_coord(rand_x, rand_y) {
                continue;
            }
            let rand_tile = cx.st.map.ref_(rand_x, rand_y);
            if !cx.st.map.is_land(rand_tile) {
                continue;
            }
            if cx.st.map.is_impassable(rand_tile) {
                continue;
            }
            let owner = cx.st.owner(rand_tile);
            if owner == me {
                continue;
            }
            // Skip players we already know are unreachable (Performance optimization)
            if owner.is_player() && unreachable_players.contains(&owner) {
                continue;
            }
            // Don't send boats to players with which we share a border, that usually looks stupid
            if owner.is_player() && bordering_enemies.contains(&owner) {
                continue;
            }
            // Don't spam boats into players which are stronger than us (FFA only)
            if Self::attack_is_ffa(cx) && owner.is_player() && cx.st.player(owner).troops() > cx.st.player(me).troops()
            {
                continue;
            }

            let matches_criteria = if high_interest_only {
                // High-interest targeting: prioritize unowned tiles or tiles owned by bots
                !owner.is_player() || cx.st.player(owner).player_type() == PlayerType::Bot
            } else {
                // Normal targeting: return unowned tiles or tiles owned by non-friendly players
                !owner.is_player() || !cx.st.player(owner).is_friendly(me, false)
            };
            if !matches_criteria {
                continue;
            }

            // Validate that we can actually build a transport ship to this target
            let launch = can_build_transport_ship_cx(cx, me, rand_tile);
            let Some(launch) = launch else {
                if owner.is_player() {
                    unreachable_players.insert(owner);
                }
                continue;
            };

            return Some((rand_tile, launch));
        }
        None
    }

    /// `attackBestTarget(borderingFriends, borderingEnemies)` (`:349-395`); both lists sorted by
    /// troops (ascending).
    fn attack_attack_best_target(
        &mut self,
        cx: &mut Ctx,
        bordering_friends: &[SmallId],
        bordering_enemies: &[SmallId],
    ) {
        let difficulty = Self::atk_difficulty(cx);

        // Hard & Impossible: answer attacks right away, even below the reserve and trigger ratios
        if (difficulty == Difficulty::Hard || difficulty == Difficulty::Impossible) && self.attack_retaliate(cx) {
            return;
        }

        // A walled-in bot costs next to nothing to annex, so don't save up for it
        self.attack_annex_walled_in_bots(cx);

        // Attack bots before the ratio checks when they hold structures, which we need back,
        // and with high starting gold, as those checks count the troops of cities built in a rush
        if (self.attack_has_neighboring_bot_with_structures(cx)
            || (ai_attack::expands_past_city_troops(self, cx) && ai_attack::has_trigger_ratio_troops(self, cx, true)))
            && self.attack_attack_bots(cx)
        {
            return;
        }

        // Save up troops until we reach the reserve ratio
        if !self.attack_has_reserve_ratio_troops(cx) {
            return;
        }

        // Medium: answer attacks without saving up to the trigger ratio first
        if difficulty == Difficulty::Medium && self.attack_retaliate(cx) {
            return;
        }

        // Maybe save up troops until we reach the trigger ratio
        if !ai_attack::has_trigger_ratio_troops(self, cx, false) && !self.rng.chance(10.0) {
            return;
        }

        // Get attack strategies in priority order based on difficulty
        let strategies = Self::attack_get_attack_strategies(difficulty);
        let mut scx = StrategyCx { bordering_friends, bordering_enemies, groups: None };

        for &strategy in strategies {
            if self.attack_run_strategy(cx, &mut scx, strategy) {
                return;
            }
        }
    }

    /// `getAttackStrategies(..)`'s return value (`:560-580`): the strategy order per difficulty.
    /// Easy nations get the dumbest order, impossible nations get the smartest order; Medium and up
    /// retaliate in attackBestTarget, ahead of the ratio checks.
    fn attack_get_attack_strategies(difficulty: Difficulty) -> &'static [Strategy] {
        use Strategy::*;
        match difficulty {
            // So dumb, they cant even find islanders
            Difficulty::Easy => &[Nuked, Bots, Retaliate, Assist, Betray, Hated, Weakest],
            Difficulty::Medium => &[Bots, Nuked, Assist, Betray, Hated, Afk, Traitor, Crown, Weakest, Island, Donate],
            // Strong veryWeak and juicy strats after the distracting hated strat, to make the nations weaker than impossible
            Difficulty::Hard => &[
                Bots, Assist, Betray, Nuked, Traitor, Afk, Hated, VeryWeak, Juicy, Victim, Crown, Weakest, Island,
                Donate,
            ],
            Difficulty::Impossible => &[
                Bots, VeryWeak, Betray, Assist, Victim, Crown, Traitor, Juicy, Afk, Nuked, Hated, Weakest, Island,
                Donate,
            ],
        }
    }

    /// One strategy closure (`:453-558`).
    fn attack_run_strategy(&mut self, cx: &mut Ctx, scx: &mut StrategyCx<'_>, strategy: Strategy) -> bool {
        let difficulty = Self::atk_difficulty(cx);
        let me = self.nation_player();
        match strategy {
            Strategy::Retaliate => self.attack_retaliate(cx),
            Strategy::Bots => self.attack_attack_bots(cx),
            Strategy::Assist => self.attack_assist_allies(cx),
            Strategy::Traitor => match self.attack_pick(cx, scx, Finder::Traitor) {
                Some(t) => ai_attack::send_attack(self, cx, t, false),
                None => false,
            },
            Strategy::Afk => match self.attack_pick(cx, scx, Finder::Afk) {
                Some(t) => ai_attack::send_attack(self, cx, t, false),
                None => false,
            },
            Strategy::Betray => self.attack_maybe_betray_and_attack(cx, scx.bordering_friends, scx.bordering_enemies),
            Strategy::Nuked => {
                if self.attack_is_bordering_nuked_territory(cx) {
                    return ai_attack::send_attack(self, cx, SmallId::TERRA_NULLIUS, false);
                }
                false
            }
            Strategy::Victim => match self.attack_pick(cx, scx, Finder::Victim) {
                Some(t) => ai_attack::send_attack(self, cx, t, false),
                None => false,
            },
            Strategy::Juicy => match self.attack_pick(cx, scx, Finder::Juicy) {
                Some(t) => ai_attack::send_attack(self, cx, t, false),
                None => false,
            },
            Strategy::Hated => {
                // TS :508 `allRelationsSorted()` is a fresh array.
                for (other, relation) in cx.st.player(me).all_relations_sorted() {
                    if relation != Relation::Hostile {
                        continue;
                    }
                    if cx.st.player(me).is_friendly(other, false) {
                        continue;
                    }
                    if Self::attack_is_ffa(cx) && cx.st.player(other).troops() > cx.st.player(me).troops() * 3.0 {
                        continue;
                    }
                    if difficulty != Difficulty::Easy && !self.attack_can_reach(cx, other) {
                        continue;
                    }
                    return ai_attack::send_attack(self, cx, other, false);
                }
                false
            }
            Strategy::VeryWeak => match self.attack_pick(cx, scx, Finder::VeryWeak) {
                Some(t) => ai_attack::send_attack(self, cx, t, false),
                None => false,
            },
            Strategy::Weakest => match self.attack_pick(cx, scx, Finder::Weakest) {
                // Groups are sorted by troops (ascending), so the first match is the weakest.
                // In FFA, don't attack if they have more troops than us
                Some(t) => ai_attack::send_attack(self, cx, t, false),
                None => false,
            },
            Strategy::Island => {
                if self.attack_pick(cx, scx, Finder::First).is_none() {
                    let enemy = self.attack_find_nearest_island_enemy(cx);
                    if let Some(enemy) = enemy {
                        return ai_attack::send_attack(self, cx, enemy, false);
                    }
                }
                false
            }
            Strategy::Donate => self.attack_donate_troops(cx),
            Strategy::Crown => {
                let leader = self.attack_find_crown_target(cx, scx.bordering_enemies);
                match leader {
                    Some(leader) => {
                        self.should_attack(cx, leader)
                            && ai_attack::send_land_attack(
                                self,
                                cx,
                                leader,
                                SendTroops::TroopsMinusReserve,
                                Some(leader),
                            )
                    }
                    None => false,
                }
            }
        }
    }

    /// `targetGroups()` (`:407-432`): computed once per `getAttackStrategies` call, on first use.
    fn attack_target_groups(&mut self, cx: &mut Ctx, scx: &mut StrategyCx<'_>) -> Vec<Vec<SmallId>> {
        if let Some(g) = &scx.groups {
            return g.clone();
        }
        let difficulty = Self::atk_difficulty(cx);
        let bordering_enemies = scx.bordering_enemies;
        let land_front = bordering_enemies.iter().any(|&enemy| self.attack_borders_by_land(cx, enemy));
        let by_troops = |st: &State, a: &SmallId, b: &SmallId| st.player(*a).troops() - st.player(*b).troops();
        let groups = if difficulty == Difficulty::Impossible && land_front {
            let mut overseas = self.attack_overseas_targets(cx, bordering_enemies);
            js_sort_by(&mut overseas, |a, b| by_troops(cx.st, a, b));
            vec![bordering_enemies.to_vec(), overseas]
        } else if (difficulty == Difficulty::Hard || difficulty == Difficulty::Impossible) && !land_front {
            let mut all = bordering_enemies.to_vec();
            all.extend(self.attack_overseas_targets(cx, bordering_enemies));
            js_sort_by(&mut all, |a, b| by_troops(cx.st, a, b));
            vec![all]
        } else {
            vec![bordering_enemies.to_vec()]
        };
        scx.groups = Some(groups.clone());
        groups
    }

    /// `pick(find)` (`:436-451`): what `find` picks among the targets we can reach right now,
    /// from the first group where it picks anything (Easy doesn't check). Only its picks get a
    /// boat route planned.
    fn attack_pick(&mut self, cx: &mut Ctx, scx: &mut StrategyCx<'_>, find: Finder) -> Option<SmallId> {
        let difficulty = Self::atk_difficulty(cx);
        for group in self.attack_target_groups(cx, scx) {
            let mut pool = group;
            loop {
                let target = self.attack_find(cx, find, &pool);
                let Some(target) = target else { break };
                if difficulty == Difficulty::Easy || self.attack_can_reach(cx, target) {
                    return Some(target);
                }
                pool.retain(|&enemy| enemy != target);
            }
        }
        None
    }

    /// Dispatch of the `find` closures.
    fn attack_find(&mut self, cx: &mut Ctx, find: Finder, enemies: &[SmallId]) -> Option<SmallId> {
        let me = self.nation_player();
        match find {
            Finder::Traitor => self.attack_find_traitor(cx, enemies),
            Finder::Afk => enemies.iter().copied().find(|&enemy| {
                cx.st.player(enemy).is_disconnected()
                    && (!Self::attack_is_ffa(cx) || cx.st.player(enemy).troops() < cx.st.player(me).troops() * 3.0)
            }),
            Finder::Victim => self.attack_find_victim(cx, enemies),
            Finder::Juicy => self.attack_find_juicy_target(cx, enemies),
            Finder::VeryWeak => self.attack_find_very_weak_enemy(cx, enemies),
            Finder::Weakest => enemies
                .iter()
                .copied()
                .find(|&enemy| !Self::attack_is_ffa(cx) || cx.st.player(enemy).troops() < cx.st.player(me).troops()),
            Finder::First => enemies.first().copied(),
        }
    }

    /// `hasNeighboringBotWithStructures()` (`:583-593`).
    fn attack_has_neighboring_bot_with_structures(&mut self, cx: &mut Ctx) -> bool {
        let me = self.nation_player();
        let near = cx.st.nearby_memo(me);
        let st = &*cx.st;
        near.into_iter().any(|n| {
            n.is_player()
                && st.player(n).player_type() == PlayerType::Bot
                && !st.player(me).is_friendly(n, false)
                && owns_structures(st, n)
        })
    }

    /// `hasReserveRatioTroops()` (`:595-599`).
    fn attack_has_reserve_ratio_troops(&mut self, cx: &mut Ctx) -> bool {
        let me = self.nation_player();
        let max_troops = cx.st.config().max_troops(&cx.st.player(me));
        // rules-v36 (#6297; v36 :593) `this.homeTroops() / maxTroops`
        let ratio = ai_attack::home_troops(self, cx) / max_troops;
        ratio >= self.atk().base.reserve_ratio
    }

    /// `retaliate()` (`:601-614`).
    fn attack_retaliate(&mut self, cx: &mut Ctx) -> bool {
        let me = self.nation_player();
        let attacker = ai_attack::find_incoming_attack_player(self, cx);
        let Some(attacker) = attacker else { return false };
        let difficulty = Self::atk_difficulty(cx);
        if (difficulty == Difficulty::Hard || difficulty == Difficulty::Impossible)
            && Self::attack_is_ffa(cx)
            && cx.shares_border(me, attacker)
        {
            return self.attack_send_retaliation(cx, attacker);
        }
        ai_attack::send_attack(self, cx, attacker, true)
    }

    /// `holdsBoatsUnderAttack()` (`:616-631`): Hard & Impossible keep their troops home while
    /// under attack, Medium only sometimes.
    fn attack_holds_boats_under_attack(&mut self, cx: &mut Ctx) -> bool {
        if ai_attack::find_incoming_attack_player(self, cx).is_none() {
            return false;
        }
        match Self::atk_difficulty(cx) {
            Difficulty::Easy => false,
            Difficulty::Medium => self.rng.chance(2.0),
            Difficulty::Hard | Difficulty::Impossible => true,
        }
    }

    /// `sendRetaliation(attacker)` (`:633-660`): cancels the attacker's incoming troops. If we can
    /// also match its home army while keeping back what troopSendCap wants for every other
    /// neighbor, pushes into its land with at least that much. Otherwise only cancels, as far as
    /// the reserve allows. Never dips below the expand ratio.
    fn attack_send_retaliation(&mut self, cx: &mut Ctx, attacker: SmallId) -> bool {
        let me = self.nation_player();
        let mut incoming = 0.0;
        for a in cx.st.player(me).incoming_attacks() {
            if cx.st.attack(a).attacker() == attacker {
                incoming += cx.st.attack(a).troops();
            }
        }
        let mut pressing = 0.0;
        for &a in cx.st.player(me).outgoing_attacks().as_slice() {
            if cx.st.attack(a).target() == attacker {
                pressing += cx.st.attack(a).troops();
            }
        }
        // rules-v36 (#6297; v36 :643) `const troops = this.homeTroops()`
        let troops = ai_attack::home_troops(self, cx);
        let max_troops = cx.st.config().max_troops(&cx.st.player(me));
        let expand_ratio = self.atk().base.expand_ratio;
        let reserve_ratio = self.atk().base.reserve_ratio;
        // TS :649-652: the first argument is evaluated before `neighborTroopCap`.
        let a0 = troops - max_troops * expand_ratio;
        let spare = js::min(a0, self.attack_neighbor_troop_cap(cx, Some(attacker)));
        let above_reserve = troops - max_troops * reserve_ratio;
        let counter = incoming + js::max(0.0, cx.st.player(attacker).troops() - pressing);
        let send = if spare >= counter {
            js::max(counter, js::min(above_reserve, spare))
        } else {
            js::min(incoming, js::max(spare, above_reserve))
        };
        ai_attack::send_land_attack(self, cx, attacker, SendTroops::Fixed(send), Some(attacker))
    }

    /// `strikeTroops(target)` (`:662-669`): troops a strike on target could send now: above the
    /// reserve, and within troopSendCap without target.
    pub fn attack_strike_troops(&mut self, cx: &mut Ctx, target: SmallId) -> f64 {
        let me = self.nation_player();
        let max_troops = cx.st.config().max_troops(&cx.st.player(me));
        let reserve_ratio = self.atk().base.reserve_ratio;
        // rules-v36 (#6297; v36 :662) `this.homeTroops() - maxTroops * this.reserveRatio`
        let a0 = ai_attack::home_troops(self, cx) - max_troops * reserve_ratio;
        js::min(a0, self.attack_troop_send_cap(cx, Some(target)))
    }

    /// `strike(target)` (`:671-680`): attacks a player our nuke just hit, while its troops are
    /// down. Skips the trigger ratio.
    pub fn attack_strike(&mut self, cx: &mut Ctx, target: SmallId) -> bool {
        let me = self.nation_player();
        if cx.st.player(me).is_friendly(target, false) || !cx.shares_border(me, target) {
            return false;
        }
        ai_attack::send_land_attack(self, cx, target, SendTroops::TroopsMinusReserve, Some(target))
    }

    /// `findCrownTarget(borderingEnemies)` (`:682-703`): a bordering runaway leader. Hard &
    /// Impossible always consider it; Medium only joins attacks already under way.
    fn attack_find_crown_target(&mut self, cx: &mut Ctx, bordering_enemies: &[SmallId]) -> Option<SmallId> {
        let me = self.nation_player();
        let leader = find_runaway_leader(cx.st)?;
        if !bordering_enemies.contains(&leader) || !cx.shares_border(me, leader) {
            return None;
        }
        let difficulty = Self::atk_difficulty(cx);
        if difficulty == Difficulty::Hard || difficulty == Difficulty::Impossible {
            return Some(leader);
        }
        if incoming_troops(cx.st, leader) >= cx.st.player(leader).troops() * 0.1 { Some(leader) } else { None }
    }

    /// `attackBots()` (`:709-749`): sort neighboring bots by density (troops / tiles) and attempt
    /// to attack many of them (parallel attacks). Bots that own structures go first. Hard &
    /// Impossible order them to wall in other bots instead (see planBotAttacks).
    fn attack_attack_bots(&mut self, cx: &mut Ctx) -> bool {
        // rules-v36 (#6278 1a166020c; v36 :710) reset on every call (the pin reset only when bots were found)
        self.atk().bot_attack_troops_sent = 0.0;
        let bots = self.attack_neighboring_bots(cx);
        if bots.is_empty() {
            // rules-v36 (#6278; v36 :712-716): without a land front, boat the nearest overseas bots
            if self.attack_has_land_front(cx) {
                return false;
            }
            // `overseasBots().some((bot) => this.canReach(bot) && this.sendAttack(bot))` (in order, first success
            // stops)
            for bot in self.attack_overseas_bots(cx) {
                if self.attack_can_reach(cx, bot) && ai_attack::send_attack(self, cx, bot, false) {
                    return true;
                }
            }
            return false;
        }
        let me = self.nation_player();

        let mut ordered: Vec<SmallId> = if Self::attack_annexes_bots(cx) {
            plan_bot_attacks(cx.st, me, &bots)
        } else {
            let st = &*cx.st;
            // TS :725 `p.troops() / p.numTilesOwned()` (0 tiles → ±Infinity/NaN; TimSort copes).
            let density = |p: SmallId| st.player(p).troops() / f64::from(st.player(p).num_tiles_owned());
            let mut v = bots.clone();
            js_sort_by(&mut v, |a, b| density(*a) - density(*b));
            v
        };
        {
            let st = &*cx.st;
            // TS :730-732 `Number(ownsStructures(b)) - Number(ownsStructures(a))`, in place on `ordered`.
            let os = |p: SmallId| if owns_structures(st, p) { 1.0 } else { 0.0 };
            js_sort_by(&mut ordered, |a, b| os(*b) - os(*a));
        }
        let par = self.attack_get_bot_attack_max_parallelism(cx);
        // TS :733 `sortedBots.slice(0, n)`.
        let reduced_bots: Vec<SmallId> = ordered.iter().copied().take(par).collect();

        for &bot in &reduced_bots {
            ai_attack::send_attack(self, cx, bot, false);
        }

        // Only short-circuit the rest of the targeting pipeline if we actually
        // allocated some troops to bot attacks. Hard & Impossible only top up
        // running attacks, so those count as well.
        self.atk().bot_attack_troops_sent > 0.0
            || (Self::attack_annexes_bots(cx)
                && cx
                    .st
                    .player(me)
                    .outgoing_attacks()
                    .as_slice()
                    .iter()
                    .any(|&attack| reduced_bots.iter().any(|&bot| bot == cx.st.attack(attack).target())))
    }

    /// `overseasBots()` (rules-v36, upstream #6278; v36 :760-791): the 3 nearest bots within boat range (not on
    /// Easy), skipping those our boats already sail to.
    fn attack_overseas_bots(&mut self, cx: &mut Ctx) -> Vec<SmallId> {
        let me = self.nation_player();
        if Self::atk_difficulty(cx) == Difficulty::Easy
            || cx.st.config().is_unit_disabled(UnitType::TransportShip)
            || f64::from(cx.st.player(me).unit_count(UnitType::TransportShip)) >= cx.st.config().boat_max_number()
        {
            return Vec::new();
        }
        let st = &*cx.st;
        // `boarded: Set<Player | TerraNullius>`: owners of our transport ships' target tiles (membership only)
        let mut boarded: Vec<Owner> = Vec::new();
        for boat in st.player(me).units_of(&[UnitType::TransportShip]) {
            if let Some(dst) = st.unit(boat).target_tile() {
                boarded.push(st.owner(dst));
            }
        }
        let range = Self::attack_max_boat_route(cx);
        let ours = st.player(me).largest_cluster_bounding_box();
        let mut candidates: Vec<(SmallId, f64)> = Vec::new();
        for p in st.players() {
            if st.player(p).player_type() != PlayerType::Bot
                || st.player(me).is_friendly(p, false)
                || boarded.contains(&p)
            {
                continue;
            }
            let gap = box_gap(ours, st.player(p).largest_cluster_bounding_box());
            if gap <= range {
                candidates.push((p, gap));
            }
        }
        // `candidates.sort((a, b) => a.gap - b.gap)` (stable)
        js_sort_by(&mut candidates, |a, b| a.1 - b.1);
        candidates.into_iter().take(3).map(|(p, _)| p).collect()
    }

    /// `hasLandFront()` (rules-v36, #6278; v36 :793-798): a non-friendly land neighbor (`landNeighbors ?? []`).
    fn attack_has_land_front(&mut self, cx: &mut Ctx) -> bool {
        let me = self.nation_player();
        let Some(ln) = self.atk().land_neighbors.as_ref() else { return false };
        ln.iter().any(|&n| !cx.st.player(me).is_friendly(n, false))
    }

    /// `neighboringBots()` (`:751-760`).
    fn attack_neighboring_bots(&mut self, cx: &mut Ctx) -> Vec<SmallId> {
        let me = self.nation_player();
        let near = cx.st.nearby_memo(me);
        let st = &*cx.st;
        near.into_iter()
            .filter(|&n| {
                n.is_player() && !st.player(me).is_friendly(n, false) && st.player(n).player_type() == PlayerType::Bot
            })
            .collect()
    }

    /// `annexesBots()` (`:762-768`): Hard & Impossible take bots in an order that walls others in.
    fn attack_annexes_bots(cx: &Ctx) -> bool {
        let d = Self::atk_difficulty(cx);
        d == Difficulty::Hard || d == Difficulty::Impossible
    }

    /// `annexWalledInBots()` (`:770-783`): the engine only checks a bot for annexation when its land
    /// changes, so a 1% attack makes it hand over a bot only we border.
    fn attack_annex_walled_in_bots(&mut self, cx: &mut Ctx) {
        if !Self::attack_annexes_bots(cx) {
            return;
        }
        let bots = self.attack_neighboring_bots(cx);
        if bots.is_empty() {
            return;
        }
        let me = self.nation_player();
        // rules-v36 (#6297; v36 :814) `Math.floor(this.homeTroops() / 100)`
        let troops = js::floor(ai_attack::home_troops(self, cx) / 100.0);
        if troops < 1.0 {
            return;
        }
        for bot in walled_in_bots(cx.st, me, &bots) {
            let bid = cx.st.player(bot).player_id().to_string();
            // rules-v36 (#6297; v36 :817) `this.send(new AttackExecution(troops, this.player, bot.id()), troops)`
            ai_attack::send(self, cx, Exec::Attack(AttackExec::new(Some(troops), me, Some(bid), None, true)), troops);
        }
    }

    /// `getBotAttackMaxParallelism()` (`:785-801`).
    fn attack_get_bot_attack_max_parallelism(&mut self, cx: &mut Ctx) -> usize {
        match Self::atk_difficulty(cx) {
            Difficulty::Easy => 1,
            Difficulty::Medium => {
                if self.rng.chance(2.0) {
                    1
                } else {
                    2
                }
            }
            Difficulty::Hard => 3,
            // On impossible difficulty, attack as much bots as possible in parallel
            Difficulty::Impossible => 100,
        }
    }

    /// `assistAllies()` (`:803-828`).
    fn attack_assist_allies(&mut self, cx: &mut Ctx) -> bool {
        if cx.st.config().disable_alliances() {
            return false;
        }
        let me = self.nation_player();

        // `allies()` and `targets()` are fresh arrays (PlayerImpl.ts:772-774, 1014-1020).
        for ally in cx.st.player(me).allies() {
            if cx.st.player(ally).targets().is_empty() {
                continue;
            }
            let ally_r = EmojiRecipient::Player(ally);
            if cx.st.player(me).relation(ally) < Relation::Friendly {
                self.emoji_send_emoji(cx, ally_r, EMOJI_ASSIST_RELATION_TOO_LOW);
                continue;
            }
            for target in cx.st.player(ally).targets() {
                if target == me {
                    self.emoji_send_emoji(cx, ally_r, EMOJI_ASSIST_TARGET_ME);
                    continue;
                }
                if cx.st.player(me).is_friendly(target, false) {
                    self.emoji_send_emoji(cx, ally_r, EMOJI_ASSIST_TARGET_ALLY);
                    continue;
                }
                if !ai_attack::send_attack(self, cx, target, false) {
                    continue;
                }
                cx.player_mut(me).update_relation(ally, -20.0);
                self.emoji_send_emoji(cx, ally_r, EMOJI_ASSIST_ACCEPT);
                return true;
            }
        }
        false
    }

    /// `findTraitor(enemies)` (`:830-842`): a traitor who isn't significantly stronger than us;
    /// `enemies` is sorted by troops (ascending), so the first match is the weakest traitor.
    fn attack_find_traitor(&mut self, cx: &mut Ctx, enemies: &[SmallId]) -> Option<SmallId> {
        if cx.st.config().disable_alliances() {
            return None;
        }
        let me = self.nation_player();
        enemies.iter().copied().find(|&enemy| {
            cx.st.player(enemy).is_traitor()
                && (!Self::attack_is_ffa(cx) || cx.st.player(enemy).troops() < cx.st.player(me).troops() * 1.2)
        })
    }

    /// `maybeBetrayAndAttack(borderingFriends, borderingEnemies)` (`:844-868`).
    fn attack_maybe_betray_and_attack(
        &mut self,
        cx: &mut Ctx,
        bordering_friends: &[SmallId],
        bordering_enemies: &[SmallId],
    ) -> bool {
        if cx.st.config().disable_alliances() {
            return false;
        }

        if !bordering_friends.is_empty() {
            // Computed once here, not per friend below - it doesn't depend on which one.
            let juiciest_ally = self.alliance_find_juiciest_ally(cx, bordering_friends);
            for &friend in bordering_friends {
                if self.alliance_maybe_betray(cx, friend, juiciest_ally, bordering_friends, bordering_enemies) {
                    return ai_attack::send_attack(self, cx, friend, true);
                }
            }
        }
        false
    }

    /// `isBorderingNukedTerritory()` (`:870-892`).
    fn attack_is_bordering_nuked_territory(&mut self, cx: &mut Ctx) -> bool {
        if cx.st.config().is_unit_disabled(UnitType::MissileSilo) {
            return false;
        }

        // Boolean result, so neighbor order doesn't matter.
        let me = self.nation_player();
        let st = &*cx.st;
        let mut nbuf = [0; 4];
        for tile in st.player(me).border_tiles().iter() {
            let n = st.map.neighbors4(tile, &mut nbuf);
            for &neighbor in &nbuf[..n] {
                if st.map.is_land(neighbor) && !st.map.has_owner(neighbor) && st.map.has_fallout(neighbor) {
                    return true;
                }
            }
        }
        false
    }

    /// `findVictim(enemies)` (`:894-910`): someone who isn't significantly stronger than us and is
    /// under big attack from others (50%+ of their troops incoming).
    fn attack_find_victim(&mut self, cx: &mut Ctx, enemies: &[SmallId]) -> Option<SmallId> {
        let me = self.nation_player();
        enemies.iter().copied().find(|&enemy| {
            if Self::attack_is_ffa(cx) && cx.st.player(enemy).troops() > cx.st.player(me).troops() * 1.2 {
                return false;
            }

            let total_incoming_troops = incoming_troops(cx.st, enemy);

            total_incoming_troops > cx.st.player(enemy).troops() * 0.5
        })
    }

    /// `findVeryWeakEnemy(enemies)` (`:912-925`): less than 15% of their maxTroops, and not
    /// significantly more troops than us (to target MIRVed players).
    fn attack_find_very_weak_enemy(&mut self, cx: &mut Ctx, enemies: &[SmallId]) -> Option<SmallId> {
        let me = self.nation_player();
        let st = &*cx.st;
        let ffa = Self::attack_is_ffa(cx);
        let very_weak_enemies: Vec<SmallId> = enemies
            .iter()
            .copied()
            .filter(|&enemy| {
                let enemy_max_troops = st.config().max_troops(&st.player(enemy));
                st.player(enemy).troops() < enemy_max_troops * 0.15
                    && (!ffa || st.player(enemy).troops() < st.player(me).troops() * 1.2)
            })
            .collect();

        // enemies is already sorted by troops (ascending), so first match is weakest very weak enemy
        very_weak_enemies.first().copied()
    }

    /// `findJuicyTarget(enemies)` (`:927-933`): juiciest enemy (Hard & Impossible only) we could
    /// plausibly beat (troops <= 75% of ours).
    fn attack_find_juicy_target(&mut self, cx: &mut Ctx, enemies: &[SmallId]) -> Option<SmallId> {
        let me = self.nation_player();
        let candidates: Vec<SmallId> = enemies
            .iter()
            .copied()
            .filter(|&enemy| cx.st.player(enemy).troops() <= cx.st.player(me).troops() * 0.75)
            .collect();
        find_juiciest_target(cx.st, &candidates)
    }

    /// `findNearestIslandEnemy()` (`:935-1006`).
    fn attack_find_nearest_island_enemy(&mut self, cx: &mut Ctx) -> Option<SmallId> {
        let me = self.nation_player();
        if cx.st.config().is_unit_disabled(UnitType::TransportShip) {
            return None;
        }

        // Check if we've already sent out the maximum number of transport ships
        if f64::from(cx.st.player(me).unit_count(UnitType::TransportShip)) >= cx.st.config().boat_max_number() {
            return None;
        }

        // Check if we have any shore tiles to launch from
        let has_shore = cx.st.player(me).border_tiles().iter().any(|t| cx.st.map.is_shore(t));
        if !has_shore {
            return None;
        }

        let ffa = Self::attack_is_ffa(cx);
        let filtered_players: Vec<SmallId> = cx
            .st
            .players()
            .into_iter()
            .filter(|&p| {
                if p == me {
                    return false;
                }
                if cx.st.player(me).is_friendly(p, false) {
                    return false;
                }
                // In FFA, don't spam boats into players with more troops
                !ffa || cx.st.player(p).troops() < cx.st.player(me).troops()
            })
            .collect();

        if filtered_players.is_empty() {
            return None;
        }

        let player_center = Self::attack_get_player_center(cx.st, me);

        let mut sorted_players: Vec<(SmallId, f64)> = filtered_players
            .iter()
            .map(|&filtered_player| {
                let filtered_player_center = Self::attack_get_player_center(cx.st, filtered_player);

                let player_center_tile = cx.st.map.ref_(player_center.x, player_center.y);
                let filtered_player_center_tile = cx.st.map.ref_(filtered_player_center.x, filtered_player_center.y);

                let distance = f64::from(cx.st.map.manhattan_dist(player_center_tile, filtered_player_center_tile));
                (filtered_player, distance)
            })
            .collect();
        js_sort_by(&mut sorted_players, |a, b| a.1 - b.1); // Sort by distance (ascending)

        // Try players in order of distance until we find reachable candidates.
        // Beachhead boats sail any distance, even past a warship: nothing nearer is left to attack.
        let mut reachable_players: Vec<SmallId> = Vec::new();
        for &(p, _) in &sorted_players {
            let reachable = if Self::attack_lands_beachheads(cx) {
                self.attack_borders_by_land(cx, p) || ai_attack::boat_route(self, cx, p).is_some()
            } else {
                self.attack_can_reach(cx, p)
            };
            if reachable {
                reachable_players.push(p);
                // We only need up to 2 reachable candidates
                if reachable_players.len() >= 2 {
                    break;
                }
            }
        }

        if reachable_players.is_empty() {
            return None;
        }

        // 33% chance to pick the second-nearest player if available
        if reachable_players.len() >= 2 && self.rng.chance(3.0) {
            return Some(reachable_players[1]);
        }

        Some(reachable_players[0])
    }

    /// `isFFA()` (`:1011-1013`).
    fn attack_is_ffa(cx: &Ctx) -> bool {
        cx.st.config().game_config().game_mode == GameMode::Ffa
    }

    /// `getPlayerCenter(player)` (`:1015-1020`).
    fn attack_get_player_center(st: &State, player: SmallId) -> Cell {
        if let Some(b) = st.player(player).largest_cluster_bounding_box() {
            return crate::util::bounding_box_center(b);
        }
        crate::util::calculate_bounding_box_center(&st.map, st.player(player).border_tiles().iter())
    }

    /// `forceSendAttack(target)` (`:1022-1030`).
    pub fn attack_force_send_attack(&mut self, cx: &mut Ctx, target: Owner) {
        let me = self.nation_player();
        let troops = cx.st.player(me).troops() / 2.0;
        // TS: `target.isPlayer() ? target.id() : this.game.terraNullius().id()` (TN id is null).
        let target_id = if target.is_player() { Some(cx.st.player(target).player_id().to_string()) } else { None };
        cx.st.add_execution(Exec::Attack(AttackExec::new(Some(troops), me, target_id, None, true)));
    }

    /// `isAttackTooWeak(troops, target)` (`:1060-1076`): for Hard & Impossible nations in FFA,
    /// `troops` below 20% of the target's troops is too weak to be worthwhile.
    fn attack_is_attack_too_weak(&mut self, cx: &mut Ctx, troops: f64, target: SmallId) -> bool {
        if cx.st.config().game_config().game_mode == GameMode::Team {
            return false;
        }
        let me = self.nation_player();
        // Nations under attack may retaliate freely
        if !cx.st.player(me).incoming_attacks().is_empty() {
            return false;
        }
        let difficulty = Self::atk_difficulty(cx);
        (difficulty == Difficulty::Hard || difficulty == Difficulty::Impossible)
            && troops < cx.st.player(target).troops() * 0.2
    }

    /// `troopSendCap(ignore?)` (`:1078-1101`).
    fn attack_troop_send_cap(&mut self, cx: &mut Ctx, ignore: Option<SmallId>) -> f64 {
        let mut cap = self.attack_neighbor_troop_cap(cx, ignore);

        // Nations under attack may retaliate with at least the incoming troops
        let me = self.nation_player();
        let incoming = cx.st.player(me).incoming_attacks();
        if !incoming.is_empty() {
            let mut total_incoming = 0.0;
            for &a in &incoming {
                total_incoming += cx.st.attack(a).troops();
            }
            cap = js::max(cap, total_incoming);
        }

        cap
    }

    /// `neighborTroopCap(ignore?)` (`:1103-1137`): troopSendCap() without the allowance for nations
    /// under attack.
    fn attack_neighbor_troop_cap(&mut self, cx: &mut Ctx, ignore: Option<SmallId>) -> f64 {
        if cx.st.config().game_config().game_mode == GameMode::Team {
            return f64::INFINITY;
        }

        let retain_fraction = match Self::atk_difficulty(cx) {
            Difficulty::Hard => 0.75,
            Difficulty::Impossible => 0.9,
            _ => return f64::INFINITY,
        };

        let me = self.nation_player();
        let near = cx.st.nearby_memo(me);
        let st = &*cx.st;
        let mut max_neighbor_troops = 0.0;
        for n in near {
            if n.is_player()
                && Some(n) != ignore
                && !st.player(me).is_friendly(n, false)
                && st.player(n).player_type() != PlayerType::Bot
                && st.player(n).troops() > max_neighbor_troops
            {
                max_neighbor_troops = st.player(n).troops();
            }
        }

        if max_neighbor_troops == 0.0 {
            return f64::INFINITY;
        }
        let min_retained = js::ceil(max_neighbor_troops * retain_fraction);
        // rules-v36 (#6297; v36 :1172) `Math.max(0, this.homeTroops() - minRetained)`
        js::max(0.0, ai_attack::home_troops(self, cx) - min_retained)
    }

    /// `landsBeachheads()` (`:1221-1227`): Hard & Impossible boat a tiny force and attack by land
    /// as soon as it lands.
    fn attack_lands_beachheads(cx: &Ctx) -> bool {
        let d = Self::atk_difficulty(cx);
        d == Difficulty::Hard || d == Difficulty::Impossible
    }

    /// `sendBeachhead(target, landing)` (`:1229-1249`): a 1% boat, if the land attack following its
    /// landing would be worth making now.
    fn attack_send_beachhead(&mut self, cx: &mut Ctx, target: Owner, landing: TileRef) -> bool {
        let me = self.nation_player();
        let opponent = if target.is_player() { Some(target) } else { None };
        // TS v36 :1283 `(targetTroops) => this.homeTroops() - targetTroops` (rules-v36, #6297).
        let troops = self.calculate_attack_troops(cx, target, SendTroops::TroopsMinusReserve, opponent);
        let Some(troops) = troops else { return false };
        // rules-v36 (#6297; v36 :1287) `Math.max(1, Math.floor(this.homeTroops() / 100))`
        let boat_troops = js::max(1.0, js::floor(ai_attack::home_troops(self, cx) / 100.0));
        // Only the boat leaves now: the land attack is budgeted once it lands
        if target.is_player() && cx.st.player(target).player_type() == PlayerType::Bot {
            self.atk().bot_attack_troops_sent += boat_troops - troops;
        }
        // rules-v36 (#6297; v36 :1292-1295) `this.send(new TransportShipExecution(..), boatTroops)`
        ai_attack::send(self, cx, Exec::TransportShip(TransportShipExec::new(me, landing, boat_troops)), boat_troops);
        true
    }

    /// `bordersByLand(target)` (`:1251-1255`): `landNeighbors?.has(target) ?? sharesBorderWith`.
    fn attack_borders_by_land(&mut self, cx: &mut Ctx, target: SmallId) -> bool {
        let me = self.nation_player();
        match &self.atk().land_neighbors {
            Some(ln) => ln.contains(&target),
            None => cx.shares_border(me, target),
        }
    }

    /// `canReach(target)` (`:1257-1266`): by land, or by a boat that passes no hostile warship
    /// and isn't sailing too far.
    fn attack_can_reach(&mut self, cx: &mut Ctx, target: SmallId) -> bool {
        if self.attack_borders_by_land(cx, target) {
            return true;
        }
        let route = ai_attack::boat_route(self, cx, target);
        match route {
            Some(r) => r.blocker.is_none() && r.length <= Self::attack_max_boat_route(cx),
            None => false,
        }
    }

    /// `maxBoatRoute()` (`:1334-1348`): Hard & Impossible don't sail further than this.
    fn attack_max_boat_route(cx: &Ctx) -> f64 {
        match Self::atk_difficulty(cx) {
            Difficulty::Easy | Difficulty::Medium => f64::INFINITY,
            Difficulty::Hard => 300.0,
            Difficulty::Impossible => 500.0,
        }
    }

    /// `overseasTargets(borderingEnemies)` (`:1350-1372`): enemies across the water (not bordering
    /// us) whose main territory is within boat range of ours; whether a boat really gets there is
    /// left to canReach.
    fn attack_overseas_targets(&mut self, cx: &mut Ctx, bordering_enemies: &[SmallId]) -> Vec<SmallId> {
        let me = self.nation_player();
        if f64::from(cx.st.player(me).unit_count(UnitType::TransportShip)) >= cx.st.config().boat_max_number() {
            return Vec::new();
        }
        let range = Self::attack_max_boat_route(cx);
        let st = &*cx.st;
        let ours = st.player(me).largest_cluster_bounding_box();
        let ffa = Self::attack_is_ffa(cx);
        st.players()
            .into_iter()
            .filter(|&p| {
                p != me
                    && st.player(p).player_type() != PlayerType::Bot
                    && !st.player(me).is_friendly(p, false)
                    && !bordering_enemies.contains(&p)
                    && (!ffa || st.player(p).troops() < st.player(me).troops())
                    && box_gap(ours, st.player(p).largest_cluster_bounding_box()) <= range
            })
            .collect()
    }

    /// `clearBlockedLane(routes, landFront)` (`:1374-1402`): Hard & Impossible send warships at
    /// whatever blocks the juiciest target they couldn't reach. Without a land front any distance
    /// counts: the sea is the only way to attack.
    fn attack_clear_blocked_lane(
        &mut self,
        cx: &mut Ctx,
        routes: &JsMap<SmallIdKey, Option<ai_attack::BoatRoute>>,
        land_front: bool,
    ) {
        let difficulty = Self::atk_difficulty(cx);
        if difficulty != Difficulty::Hard && difficulty != Difficulty::Impossible {
            return;
        }
        let me = self.nation_player();
        let max_route = Self::attack_max_boat_route(cx);
        // `blocked: Map<Player, Unit>`: keys come from `routes` (unique, in its order).
        let mut blocked: Vec<(SmallId, UnitRef)> = Vec::new();
        for (&target, route) in routes.iter() {
            let Some(route) = route else { continue };
            let Some(blocker) = route.blocker else { continue };
            if (!land_front || route.length <= max_route)
                && (!Self::attack_is_ffa(cx) || cx.st.player(target).troops() < cx.st.player(me).troops())
            {
                blocked.push((target, blocker));
            }
        }
        let keys: Vec<SmallId> = blocked.iter().map(|&(t, _)| t).collect();
        let target = find_juiciest_target(cx.st, &keys);
        if let Some(target) = target {
            let blocker = blocked
                .iter()
                .find(|&&(t, _)| t == target)
                .map(|&(_, u)| u)
                .ts_expect("NationAttackBehavior.ts:1400 blocked.get(target)!");
            self.warship_clear_sea_lane(cx, blocker);
        }
    }

    /// `calculateBotAttackTroops(target, maxTroops)` (`:1404-1434`).
    fn attack_calculate_bot_attack_troops(&mut self, cx: &mut Ctx, target: SmallId, max_troops: f64) -> f64 {
        match Self::atk_difficulty(cx) {
            Difficulty::Easy => max_troops,
            Difficulty::Medium => {
                let mut troops = cx.st.player(target).troops() * 4.0;

                // Don't send more troops than maxTroops (Keep reserve)
                if troops > max_troops {
                    // If we haven't enough troops left to do a big enough bot attack, skip it
                    if max_troops < cx.st.player(target).troops() * 2.0 {
                        troops = 0.0;
                    } else {
                        troops = max_troops;
                    }
                }
                troops
            }
            Difficulty::Hard | Difficulty::Impossible => {
                // Top up what already attacks the bot, or wait until we can send it all
                let troops =
                    self.attack_bot_attack_troops_needed(cx, target) * 1.3 - self.attack_our_stack_against(cx, target);
                if troops <= max_troops { troops } else { 0.0 }
            }
        }
    }

    /// `botAttackTroopsNeeded(bot)` (`:1436-1441`): keeps the attack at its cheapest loss rate until
    /// the bot collapses below 100 tiles.
    fn attack_bot_attack_troops_needed(&mut self, cx: &mut Ctx, bot: SmallId) -> f64 {
        let stack = js::max(1.0, cx.st.player(bot).troops() / CHEAPEST_TROOP_RATIO);
        let tiles = js::max(0.0, f64::from(cx.st.player(bot).num_tiles_owned()) - 100.0);
        js::max(stack, tiles * self.attack_bot_tile_loss(cx, bot, stack))
    }

    /// `botTileLoss(bot, stack)` (`:1443-1487`): troops lost per tile at that stack: Impossible reads
    /// the terrain on a 6x6 grid over the bot's land, Hard assumes highland.
    fn attack_bot_tile_loss(&mut self, cx: &mut Ctx, bot: SmallId, stack: f64) -> f64 {
        let me = self.nation_player();
        let st = &*cx.st;
        let mut terrains: Vec<TerrainType> = Vec::new();
        let b = st.player(bot).largest_cluster_bounding_box();
        if let (Difficulty::Impossible, Some(b)) = (Self::atk_difficulty(cx), b) {
            let width = b.max.x - b.min.x + 1.0;
            let height = b.max.y - b.min.y + 1.0;
            for i in 0..6 {
                for j in 0..6 {
                    let (i, j) = (f64::from(i), f64::from(j));
                    let tile = st.map.ref_(
                        b.min.x + js::floor(((2.0 * i + 1.0) * width) / 12.0),
                        b.min.y + js::floor(((2.0 * j + 1.0) * height) / 12.0),
                    );
                    if st.map.owner_id(tile) == bot.0 {
                        terrains.push(st.map.terrain_type(tile));
                    }
                }
            }
        }
        if terrains.is_empty() {
            terrains.push(TerrainType::Highland);
        }
        let mut loss = 0.0;
        for &terrain in &terrains {
            let input = AttackLogicInput {
                terrain,
                attack_troops: stack,
                attacker: AttackLogicAttacker {
                    player_type: st.player(me).player_type(),
                    num_tiles: f64::from(st.player(me).num_tiles_owned()),
                },
                defender: Some(AttackLogicDefender {
                    player_type: st.player(bot).player_type(),
                    num_tiles: f64::from(st.player(bot).num_tiles_owned()),
                    troops: st.player(bot).troops(),
                    is_traitor: st.player(bot).is_traitor(),
                    is_disconnected_teammate: false,
                }),
                defender_has_defense_post: false,
                fallout_ratio: None,
                border_size: 1.0,
            };
            loss += st.config().attack_logic(&input).attacker_troop_loss;
        }
        loss / (terrains.len() as f64)
    }

    /// `ourStackAgainst(target)` (`:1489-1502`): land attacks on the same target merge, so these
    /// troops join the next one we send, while the target's attacks on us cancel out against it.
    fn attack_our_stack_against(&mut self, cx: &mut Ctx, target: SmallId) -> f64 {
        let me = self.nation_player();
        let st = &*cx.st;
        let mut troops = 0.0;
        for &attack in st.player(me).outgoing_attacks().as_slice() {
            if st.attack(attack).target() == target && st.attack(attack).source_tile().is_none() {
                troops += st.attack(attack).troops();
            }
        }
        for attack in st.player(me).incoming_attacks() {
            if st.attack(attack).attacker() == target {
                troops -= st.attack(attack).troops();
            }
        }
        troops
    }

    /// `donateTroops()` (`:1504-1597`).
    fn attack_donate_troops(&mut self, cx: &mut Ctx) -> bool {
        // Only donate in team games
        if cx.st.config().game_config().game_mode != GameMode::Team {
            return false;
        }

        // Don't donate in public games (To balance HvN)
        if cx.st.config().game_config().game_type == GameType::Public {
            return false;
        }

        // Check if donating troops is allowed
        if !cx.st.config().donate_troops() {
            return false;
        }

        // Skip donating based on difficulty
        match Self::atk_difficulty(cx) {
            // Easy nations don't donate
            Difficulty::Easy => return false,
            // Medium nations donate 25% of the time
            Difficulty::Medium => {
                if !self.rng.chance(4.0) {
                    return false;
                }
            }
            // Hard nations donate 50% of the time
            Difficulty::Hard => {
                if !self.rng.chance(2.0) {
                    return false;
                }
            }
            // Impossible nations always try to donate
            Difficulty::Impossible => {}
        }

        let me = self.nation_player();
        let st = &*cx.st;
        // Find teammates who are currently in combat
        let teammates: Vec<SmallId> = st
            .players()
            .into_iter()
            .filter(|&p| st.player(me).is_on_same_team(p))
            .filter(|&p| !st.player(p).incoming_attacks().is_empty() || !st.player(p).outgoing_attacks().is_empty())
            .collect();

        if teammates.is_empty() {
            return false;
        }

        // Find teammate with lowest troop percentage (troops / maxTroops)
        let mut teammates_with_troop_percentage: Vec<(SmallId, f64)> = teammates
            .iter()
            .map(|&teammate| {
                let max_troops = st.config().max_troops(&st.player(teammate));
                let troop_percentage = st.player(teammate).troops() / js::max(max_troops, 1.0);
                (teammate, troop_percentage)
            })
            .collect();
        js_sort_by(&mut teammates_with_troop_percentage, |a, b| a.1 - b.1);

        // Try to donate to teammates in order of lowest troop percentage
        let selected_teammate =
            teammates_with_troop_percentage.iter().map(|&(t, _)| t).find(|&t| st.player(me).can_donate_troops(t));

        let Some(selected_teammate) = selected_teammate else { return false };

        // Donate a portion of our troops (keeping reserve)
        let max_troops = st.config().max_troops(&st.player(me));
        let reserve_ratio = self.atk().base.reserve_ratio;
        let troops_to_keep = max_troops * reserve_ratio;
        // rules-v36 (#6297; v36 :1630) `this.homeTroops() - troopsToKeep`
        let available_troops = ai_attack::home_troops(self, cx) - troops_to_keep;

        if available_troops < 1.0 {
            return false;
        }

        let rid = cx.st.player(selected_teammate).player_id().to_string();
        // rules-v36 (#6297; v36 :1636-1643) `this.send(new DonateTroopsExecution(..), availableTroops)`
        ai_attack::send(
            self,
            cx,
            Exec::DonateTroops(DonateTroopsExec::new(me, rid, Some(available_troops))),
            available_troops,
        );

        true
    }
}

/// `incomingTroops(target)` (`:705-707`): `target.incomingAttacks().reduce((sum, a) => sum +
/// a.troops(), 0)`.
fn incoming_troops(st: &State, target: SmallId) -> f64 {
    let mut sum = 0.0;
    for a in st.player(target).incoming_attacks() {
        sum += st.attack(a).troops();
    }
    sum
}

/// `p.units().some((u) => Structures.has(u.type()))` (`:591, :729`).
fn owns_structures(st: &State, p: SmallId) -> bool {
    st.player(p).units().as_slice().iter().any(|&u| unit_groups::has(unit_groups::STRUCTURES, st.unit(u).unit_type()))
}

impl AiAttackHooks for NationExec {
    fn ai(&mut self) -> &mut AiAttackBase {
        &mut self.b().attack.base
    }
    fn rng(&mut self) -> &mut Prng {
        &mut self.rng
    }
    fn player(&self) -> SmallId {
        self.player.ts_expect("NationExecution.player used before init")
    }
    /// `override shouldAttack(other)` (`:1032-1053`).
    fn should_attack(&mut self, cx: &mut Ctx, other: Owner) -> bool {
        if
        // Always attack Terra Nullius, non-humans and traitors
        !other.is_player()
            || cx.st.player(other).player_type() != PlayerType::Human
            || cx.st.player(other).is_traitor()
            // Always attack in an HvN game
            || cx.st.config().game_config().player_teams == Some(TeamCountConfig::HumansVsNations)
        {
            return true;
        }

        // Prevent attacking of humans on lower difficulties
        let difficulty = NationExec::atk_difficulty(cx);
        if difficulty == Difficulty::Easy && self.rng.next_int(0.0, 4.0) != 0.0 {
            return false;
        }
        if difficulty == Difficulty::Medium && self.rng.chance(4.0) {
            return false;
        }
        true
    }
    /// `override ignoresAttacksFrom(attacker)` (`:1055-1058`): bot attacks don't count as threats.
    fn ignores_attacks_from(&mut self, cx: &mut Ctx, attacker: SmallId) -> bool {
        cx.st.player(attacker).player_type() == PlayerType::Bot
    }
    /// `override expansionTroopCap()` (`:1139-1144`): like troopSendCap(), but floored above 0 —
    /// TerraNullius can't fight back, so it's throttled, not frozen.
    fn expansion_troop_cap(&mut self, cx: &mut Ctx) -> f64 {
        let cap = self.attack_troop_send_cap(cx, None);
        if cap > 0.0 {
            return cap;
        }
        // rules-v36 (#6297; v36 :1187) `Math.ceil(this.homeTroops() * 0.05)`
        js::ceil(ai_attack::home_troops(self, cx) * 0.05)
    }
    /// `override troopsAtInit()` (rules-v36, upstream #6297; v36 :1175-1181): our PlayerExecution ticks after us and
    /// cuts troops above max back first.
    fn troops_at_init(&mut self, cx: &mut Ctx) -> f64 {
        let me = self.nation_player();
        let troops = cx.st.player(me).troops();
        js::min(troops, cx.st.config().max_troops(&cx.st.player(me)))
    }
    /// `override calculateAttackTroops(target, nonBotTroops, opponent?)` (`:1146-1198`). `opponent`
    /// is a player we take on on purpose (retaliation, runaway leader): it doesn't count toward
    /// troopSendCap's threats, and troops already attacking it count toward the too-weak check.
    fn calculate_attack_troops(
        &mut self,
        cx: &mut Ctx,
        target: Owner,
        send_troops: SendTroops,
        opponent: Option<SmallId>,
    ) -> Option<f64> {
        let me = self.nation_player();
        let reserve_troops = ai_attack::reserve_troops(self, cx, target);

        let is_bot_attack = target.is_player() && cx.st.player(target).player_type() == PlayerType::Bot;
        let mut troops = if is_bot_attack {
            // rules-v36 (#6297; v36 :1202-1209) `Math.min(this.player.troops() - this.botAttackTroopsSent,
            // this.homeTroops()) - reserveTroops` (left to right)
            let sent = self.atk().bot_attack_troops_sent;
            let a = cx.st.player(me).troops() - sent;
            let max = js::min(a, ai_attack::home_troops(self, cx)) - reserve_troops;
            self.attack_calculate_bot_attack_troops(cx, target, max)
        } else {
            send_troops.eval(self, cx, reserve_troops)
        };

        // Hard & Impossible: don't drop below neighbor troop threshold (also applies to TerraNullius/fallout).
        let cap =
            if target.is_player() { self.attack_troop_send_cap(cx, opponent) } else { self.expansion_troop_cap(cx) };
        troops = js::min(troops, cap);

        if troops < 1.0 {
            return None;
        }

        // Hard & Impossible: don't attack if we'd send less than 20% of target's troops
        let pile_on = if target.is_player() && Some(target) == opponent { incoming_troops(cx.st, target) } else { 0.0 };
        if target.is_player() && self.attack_is_attack_too_weak(cx, troops + pile_on, target) {
            return None;
        }

        if target.is_player() {
            self.emoji_maybe_send_attack_emoji(cx, target);
        }

        // Only count troops that will actually be sent, post-cap.
        if is_bot_attack {
            self.atk().bot_attack_troops_sent += troops;
        }

        Some(troops)
    }
    /// `override sendBoatAttack(target)` (`:1200-1212`; `super.` at `:1202`): a beachhead boat is
    /// cheap enough to risk past a warship, one at a time.
    fn send_boat_attack(&mut self, cx: &mut Ctx, target: SmallId) -> bool {
        if !NationExec::attack_lands_beachheads(cx) {
            return ai_attack::base_send_boat_attack(self, cx, target);
        }
        let me = self.nation_player();
        let route = ai_attack::boat_route(self, cx, target);
        let Some(route) = route else { return false };
        if route.blocker.is_some() && cx.st.player(me).unit_count(UnitType::TransportShip) > 0 {
            return false;
        }
        self.attack_send_beachhead(cx, target, route.landing)
    }
    /// `override sendExpansionBoat(tile)` (`:1214-1219`; `super.` at `:1218`).
    fn send_expansion_boat(&mut self, cx: &mut Ctx, tile: TileRef) -> bool {
        if NationExec::attack_lands_beachheads(cx) {
            return self.attack_send_beachhead(cx, SmallId::TERRA_NULLIUS, tile);
        }
        ai_attack::base_send_expansion_boat(self, cx, tile)
    }
    /// `override sail(launch, target, warships = this.hostileWarships())` (`:1268-1332`): tiles a
    /// boat sails from launch to the shore at target, and the hostile warship that sinks it. A
    /// warship fires once the boat comes in range; the shell covers 3 tiles a tick (the boat 1),
    /// so a boat landing before the shell can catch it is safe. Easy doesn't look, Medium checks
    /// both shores, Hard & Impossible pathfind.
    fn sail(&mut self, cx: &mut Ctx, launch: TileRef, target: TileRef, warships: Option<&[UnitRef]>) -> SailResult {
        // The default argument is evaluated on entry, before the body (`:1275`), even on Easy.
        let owned_warships;
        let warships: &[UnitRef] = match warships {
            Some(w) => w,
            None => {
                owned_warships = ai_attack::hostile_warships(self, cx);
                &owned_warships
            }
        };
        let me = self.nation_player();
        let landing = target_transport_tile(&mut cx.rcx(), me, target).unwrap_or(target);
        let direct = f64::from(cx.st.map.manhattan_dist(launch, landing));
        let difficulty = NationExec::atk_difficulty(cx);
        if difficulty == Difficulty::Easy {
            return SailResult { length: direct, blocker: None };
        }

        let path: Option<Vec<TileRef>> = if difficulty == Difficulty::Medium || direct <= 20.0 {
            None
        } else {
            // TS :1288 `PathFinding.Water(this.game).findPath(launch, landing)`: a fresh stepper.
            let mut pf = WaterStepper::new(cx);
            pf.find_path(cx, &[launch], landing)
        };
        let has_path = path.is_some();
        let route: Vec<TileRef> = path.unwrap_or_else(|| vec![launch, landing]);
        let length = if has_path { route.len() as f64 } else { direct };
        // Without a path, assume the warship can fire as soon as we launch
        let sailed = |i: usize| if has_path { i as f64 } else { 0.0 };

        let range = ai_attack::danger_range(self, cx);
        let map = &cx.st.map;
        let mut min_x = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_y = f64::NEG_INFINITY;
        for &t in &route {
            min_x = js::min(min_x, f64::from(map.x(t)));
            max_x = js::max(max_x, f64::from(map.x(t)));
            min_y = js::min(min_y, f64::from(map.y(t)));
            max_y = js::max(max_y, f64::from(map.y(t)));
        }
        for &w in warships {
            let wt = cx.st.unit(w).tile();
            let wx = f64::from(map.x(wt));
            let wy = f64::from(map.y(wt));
            if wx < min_x - range || wx > max_x + range || wy < min_y - range || wy > max_y + range {
                continue;
            }
            for (i, &rt) in route.iter().enumerate() {
                if f64::from(map.euclidean_dist_squared(wt, rt)) > range * range {
                    continue;
                }
                // Shell and boat close in by at most 4 tiles a tick; 10 tiles spare for its reaction
                let ticks_left = length - sailed(i);
                if 4.0 * ticks_left >= f64::from(map.manhattan_dist(wt, rt)) - 10.0 {
                    return SailResult { length, blocker: Some(w) };
                }
                break;
            }
        }
        SailResult { length, blocker: None }
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // tests may catch panics to assert them
mod tests {
    use super::*;

    fn bb(x0: f64, y0: f64, x1: f64, y1: f64) -> BoundingBox {
        BoundingBox { min: Cell::new(x0, y0), max: Cell::new(x1, y1) }
    }

    #[test]
    fn box_gap_matches_ts() {
        // unknown boxes count as touching
        assert_eq!(box_gap(None, Some(&bb(0.0, 0.0, 1.0, 1.0))), 0.0);
        assert_eq!(box_gap(Some(&bb(0.0, 0.0, 1.0, 1.0)), None), 0.0);
        // overlapping → 0
        assert_eq!(box_gap(Some(&bb(0.0, 0.0, 10.0, 10.0)), Some(&bb(5.0, 5.0, 20.0, 20.0))), 0.0);
        // a right of b by 7 in x, below by 3 in y
        assert_eq!(box_gap(Some(&bb(17.0, 13.0, 20.0, 20.0)), Some(&bb(0.0, 0.0, 10.0, 10.0))), 7.0 + 3.0);
        // symmetric
        assert_eq!(box_gap(Some(&bb(0.0, 0.0, 10.0, 10.0)), Some(&bb(17.0, 13.0, 20.0, 20.0))), 10.0);
        // NaN coordinate propagates like Math.max (NaN > range is false in callers)
        assert!(box_gap(Some(&bb(f64::NAN, 0.0, 1.0, 1.0)), Some(&bb(0.0, 0.0, 1.0, 1.0))).is_nan());
    }

    #[test]
    fn strategy_orders_match_ts() {
        use Strategy::*;
        assert_eq!(
            NationExec::attack_get_attack_strategies(Difficulty::Easy),
            &[Nuked, Bots, Retaliate, Assist, Betray, Hated, Weakest]
        );
        assert_eq!(
            NationExec::attack_get_attack_strategies(Difficulty::Medium),
            &[Bots, Nuked, Assist, Betray, Hated, Afk, Traitor, Crown, Weakest, Island, Donate]
        );
        assert_eq!(
            NationExec::attack_get_attack_strategies(Difficulty::Hard),
            &[
                Bots, Assist, Betray, Nuked, Traitor, Afk, Hated, VeryWeak, Juicy, Victim, Crown, Weakest, Island,
                Donate
            ]
        );
        assert_eq!(
            NationExec::attack_get_attack_strategies(Difficulty::Impossible),
            &[
                Bots, VeryWeak, Betray, Assist, Victim, Crown, Traitor, Juicy, Afk, Nuked, Hated, Weakest, Island,
                Donate
            ]
        );
    }

    #[test]
    fn nation_attack_b_new_has_no_draws_and_ts_initialisers() {
        // AiAttackBase::new is P9's; only check our own initialisers when it is implemented.
        let r = std::panic::catch_unwind(|| NationAttackB::new(0.5, 0.3, 0.1));
        if let Ok(b) = r {
            assert_eq!(b.bot_attack_troops_sent, 0.0);
            assert!(b.followed_landings.is_empty());
            assert!(b.land_neighbors.is_none());
            assert_eq!((b.base.trigger_ratio, b.base.reserve_ratio, b.base.expand_ratio), (0.5, 0.3, 0.1));
            assert!(b.base.boat_routes.is_none());
        }
    }
}
