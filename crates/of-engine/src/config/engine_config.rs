//! `EngineConfig extends Config` (`engine/src/configuration/EngineConfig.ts:125-830`) as one
//! struct. Owner: **P3**. ARCHITECTURE §6.
//!
//! - Built once per game, shared as `Arc<EngineConfig>`. The `unitInfoCache`/
//!   `engineUnitInfoCache` memos become the precomputed `unit_info` table.
//! - `unitInfo(t).cost` (a closure, `:348-460`) is [`EngineConfig::unit_cost`].
//! - Formula functions take `&impl PlayerLike`, so the `config` vector suite runs at L0 without
//!   a `State`; `PlayerLike | TerraNullius` is `Option<&impl PlayerLike>` (`None` = TN).
//! - BigInt conversions are strict (`:281, 464, 684`: `from_floor_strict`/`from_number_strict`).
//! - `setDifficulty`/`setGameType` (`:136-142`) exist only for the out-of-scope takeover hooks.
//! - The `Config.ts` base methods are in `config/config.rs` (same struct, separate `impl`).

use crate::api::game_config::{GameConfig, TeamCountConfig};
use crate::api::types::{Difficulty, GameType, Gold, PlayerInfo, PlayerType, TerrainType, Tick, UnitInfo, UnitType};
use crate::api::views::PlayerLike;
use crate::ids::{SmallId, UnitRef};
use crate::state::State;
use of_det::bigint::{div_strict, from_floor_strict, from_number_strict};
use of_det::detmath::{exp, log, pow2};
use of_det::js;
use of_det::util::{sigmoid, to_int_strict, within};

/// `AttackLogicInput.attacker` (`:28`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AttackLogicAttacker {
    pub player_type: PlayerType,
    pub num_tiles: f64,
}

/// `AttackLogicInput.defender` (`:30-38`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AttackLogicDefender {
    pub player_type: PlayerType,
    pub num_tiles: f64,
    pub troops: f64,
    pub is_traitor: bool,
    pub is_disconnected_teammate: bool,
}

/// `AttackLogicInput` (`:25-44`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AttackLogicInput {
    pub terrain: TerrainType,
    pub attack_troops: f64,
    pub attacker: AttackLogicAttacker,
    /// `null` when attacking TerraNullius.
    pub defender: Option<AttackLogicDefender>,
    pub defender_has_defense_post: bool,
    pub fallout_ratio: Option<f64>,
    pub border_size: f64,
}

/// `AttackLogicResult` (`:46-55`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AttackLogicResult {
    pub attacker_troop_loss: f64,
    pub defender_troop_loss: f64,
    pub tick_fraction: f64,
}

/// `trainGold` relation (`:245-249`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrainRelation {
    SelfOwned,
    Team,
    Ally,
    Other,
}

/// `CHEAPEST_TROOP_RATIO` (`:75`).
pub const CHEAPEST_TROOP_RATIO: f64 = 0.6;

/// The single config object (`Config` + `EngineConfig` fields).
#[derive(Debug, Default)]
pub struct EngineConfig {
    /// `_gameConfig`.
    pub gc: GameConfig,
    /// `_isReplay`.
    pub is_replay: bool,
    /// `listed`.
    pub listed: bool,
    /// `_spectator`.
    pub spectator: bool,
    /// The `unitInfo(t)` table (indexed by `UnitType::index`).
    pub unit_info: [UnitInfo; 16],
}

impl crate::plain::PlainState for EngineConfig {}
crate::refs_leaf!(EngineConfig);

// ---- attackLogic tunables (`:57-89`) ----
const LARGE_TERRITORY_MIDPOINT: f64 = 300_000.0;
const LARGE_TERRITORY_STEEPNESS: f64 = 2.5;
const LARGE_ATTACKER_DEPTH: f64 = 0.7;
const LARGE_DEFENDER_DEPTH: f64 = 0.3;
const BOT_DEFENDER_LOSS_MULT: f64 = 0.7;
const TERRA_NULLIUS_COST_SCALE: f64 = 2000.0;
const TERRA_NULLIUS_MIN_COST: f64 = 5.0;
const TERRA_NULLIUS_MAX_COST: f64 = 100.0;
const ATTACKER_LOSS_BASE: f64 = 0.463;
const ATTACKER_LOSS_PER_DENSITY: f64 = 0.0039;
const SPEED_COST_DIVISOR: f64 = 8.55;
const LARGE_ATTACKER_SPEED_DEPTH: f64 = 0.73;

/// `largeTerritoryBonus(numTiles, depth)` (`:95-104`). `attack_logic_code` inlines it as
/// `1 - depth * territory_sigmoid(..)` (t409 H5); kept as the reference for the equality test.
#[cfg(test)]
fn large_territory_bonus(num_tiles: f64, depth: f64) -> f64 {
    1.0 - depth * sigmoid(log(num_tiles), LARGE_TERRITORY_STEEPNESS, log(LARGE_TERRITORY_MIDPOINT))
}

/// The depth-free part of `largeTerritoryBonus`: `sigmoid(log(numTiles), STEEPNESS, logMid)` with
/// `logMid = log(LARGE_TERRITORY_MIDPOINT)` passed in (t409 H5: pure DetMath, so computing it once
/// per `attackLogic` and sharing the attacker's value between its two depths gives identical bits).
#[inline]
fn territory_sigmoid(num_tiles: f64, log_mid: f64) -> f64 {
    sigmoid(log(num_tiles), LARGE_TERRITORY_STEEPNESS, log_mid)
}

/// `terrainAttackBase(terrain)` (`:106-122`) on the numeric `TerrainType` value (TS's
/// `default` branch is reachable with any number, e.g. `Ocean = 3`).
pub(crate) fn terrain_attack_base(terrain: u8) -> (f64, f64) {
    match terrain {
        0 => (80.0, 16.5),  // Plains
        1 => (100.0, 20.0), // Highland
        2 => (120.0, 25.0), // Mountain
        4 => of_det::ts_throw!("impassable terrain cannot be attacked"),
        t => of_det::ts_throw!("terrain type {t} not supported"),
    }
}

/// The inputs of the `costWrapper` closures (`:466-484`) that do not come from the config.
pub(crate) struct CostInputs<F: Fn(UnitType) -> (f64, f64)> {
    pub player_type: PlayerType,
    pub is_lobby_creator: bool,
    /// `(player.unitsOwned(t), player.unitsConstructed(t))`.
    pub counts: F,
    pub extra_units: f64,
    /// `game.mirvsLaunched()`.
    pub mirvs_launched: f64,
}

/// `samLauncherState()` as the formula reads it: `(upgradeStartTick?, targetLevel, startRange,
/// duration?)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SamUpgrade {
    pub upgrade_start_tick: Option<f64>,
    pub target_level: f64,
    pub start_range: f64,
    pub duration: Option<f64>,
}

impl EngineConfig {
    /// `new EngineConfig(gameConfig, isReplay, listed = false, spectator = false)`
    /// (`Config.ts:89-94`). The `unitInfo` caches are precomputed here (unobservable: the
    /// table depends only on `instantBuild()`, fixed per config).
    pub fn new(gc: GameConfig, is_replay: bool, listed: bool, spectator: bool) -> Self {
        let mut c = EngineConfig { gc, is_replay, listed, spectator, unit_info: [UnitInfo::default(); 16] };
        for &t in UnitType::ALL {
            c.unit_info[t.index()] = c.base_unit_info(t);
        }
        c
    }

    // ---- EngineConfig.ts, in source order ----
    pub fn traitor_speed_debuff(&self) -> f64 {
        0.8
    }
    pub fn team_land_share_win_threshold_tenths(&self) -> f64 {
        7.0
    }
    /// `setDifficulty` (`:136-138`; takeover hooks only).
    pub fn set_difficulty(&mut self, difficulty: Difficulty) {
        self.gc.difficulty = difficulty;
    }
    /// `setGameType` (`:140-142`; takeover hooks only).
    pub fn set_game_type(&mut self, game_type: GameType) {
        self.gc.game_type = game_type;
    }
    /// `5 - falloutRatio * 2` (`:144-148`).
    pub fn fallout_defense_modifier(&self, fallout_ratio: f64) -> f64 {
        5.0 - fallout_ratio * 2.0
    }
    pub fn defense_post_defense_bonus(&self) -> f64 {
        5.0
    }
    pub fn defense_post_speed_bonus(&self) -> f64 {
        3.0
    }
    /// `playerTeams()` = `playerTeams ?? 0` (`:158`).
    pub fn player_teams(&self) -> TeamCountConfig {
        self.gc.player_teams.unwrap_or(TeamCountConfig::Count(0))
    }
    /// `nations !== "disabled"`.
    pub fn spawn_nations(&self) -> bool {
        self.gc.nations != crate::api::game_config::NationsConfig::Disabled
    }
    pub fn bots(&self) -> f64 {
        self.gc.bots as f64 // zod uint: exact
    }
    pub fn disable_nav_mesh(&self) -> bool {
        self.gc.disable_nav_mesh.unwrap_or(false)
    }
    pub fn water_nukes(&self) -> bool {
        self.gc.water_nukes.flatten().unwrap_or(false)
    }
    pub fn infinite_gold(&self) -> bool {
        self.gc.infinite_gold
    }
    pub fn donate_gold(&self) -> bool {
        self.gc.donate_gold
    }
    /// `goldMultiplier ?? 1`.
    pub fn gold_multiplier(&self) -> f64 {
        self.gc.gold_multiplier.flatten().unwrap_or(1.0)
    }
    /// `startingGold(playerInfo)` (`:184-189`).
    pub fn starting_gold(&self, info: &PlayerInfo) -> Gold {
        if info.player_type == PlayerType::Bot {
            return 0;
        }
        self.starting_gold_for(info)
    }
    /// `trainSaturation(numTrainUnits)` (`:215-219`; `of_det::config`, VECTORS `config/saturation`).
    pub fn train_saturation(&self, num_train_units: f64) -> f64 {
        of_det::config::train_saturation(num_train_units)
    }
    /// `trainSpawnRate` (`:221-230`).
    pub fn train_spawn_rate(&self, num_player_factories: f64, num_train_units: f64) -> f64 {
        of_det::config::train_spawn_rate(num_player_factories, num_train_units)
    }
    /// `smallEconomySpawnBoost(levels)` (`:240-243`).
    pub fn small_economy_spawn_boost(&self, levels: f64) -> f64 {
        of_det::config::small_economy_spawn_boost(levels)
    }
    /// `trainGold(rel, citiesVisited, player)` (`:245-268`).
    pub fn train_gold(&self, rel: TrainRelation, cities_visited: f64, player: &impl PlayerLike) -> Gold {
        self.train_gold_with(rel, cities_visited, player.is_lobby_creator())
    }
    pub(crate) fn train_gold_with(&self, rel: TrainRelation, cities_visited: f64, is_lobby_creator: bool) -> Gold {
        // No penalty for the first 10 cities.
        let cities_visited = js::max(0.0, cities_visited - 9.0);
        let base_gold = match rel {
            TrainRelation::Ally => 35_000.0,
            TrainRelation::Team | TrainRelation::Other => 25_000.0,
            TrainRelation::SelfOwned => 10_000.0,
        };
        let dist_penalty = cities_visited * 5_000.0;
        let gold = js::max(5000.0, base_gold - dist_penalty);
        to_int_strict(gold * self.gold_multiplier_for_creator(is_lobby_creator))
    }
    pub fn train_station_min_range(&self) -> f64 {
        15.0
    }
    #[allow(clippy::approx_constant)] // TS literal `1.4142` (`:272`), not `Math.SQRT2`
    pub fn railroad_max_size(&self) -> f64 {
        self.train_station_max_range() * 1.4142
    }
    /// `tradeShipGold(dist, player)` (`:277-283`; strict `BigInt(Math.floor(..))`).
    pub fn trade_ship_gold(&self, dist: f64, player: &impl PlayerLike) -> Gold {
        self.trade_ship_gold_with(dist, player.is_lobby_creator())
    }
    pub(crate) fn trade_ship_gold_with(&self, dist: f64, is_lobby_creator: bool) -> Gold {
        let debuff = self.trade_ship_short_range_debuff();
        let base_gold = 75_000.0 / (1.0 + exp(-0.03 * (dist - debuff))) + 50.0 * dist;
        from_floor_strict(base_gold * self.gold_multiplier_for_creator(is_lobby_creator))
    }
    /// `tradeShipSaturation(numTradeShips)` (`:305-309`).
    pub fn trade_ship_saturation(&self, num_trade_ships: f64) -> f64 {
        of_det::config::trade_ship_saturation(num_trade_ships)
    }
    /// `tradeShipSpawnRate` (`:312-329`).
    pub fn trade_ship_spawn_rate(
        &self,
        trade_ship_spawn_rejections: f64,
        num_trade_ships: f64,
        player_port_levels: f64,
    ) -> f64 {
        of_det::config::trade_ship_spawn_rate(trade_ship_spawn_rejections, num_trade_ships, player_port_levels)
    }
    /// `hasInfiniteGoldFor(player)` (`:331-335`).
    fn has_infinite_gold_for(&self, is_lobby_creator: bool) -> bool {
        if self.infinite_gold() {
            return true;
        }
        let hc = self.gc.host_cheats.as_ref();
        hc.and_then(|h| h.infinite_gold).unwrap_or(false) && is_lobby_creator
    }
    /// `override unitInfo(type)` (`:338-346`): the static part (`maxHealth`, `damage`, …).
    pub fn unit_info(&self, t: UnitType) -> UnitInfo {
        self.unit_info[t.index()]
    }
    /// `unitInfo(type).cost(game, player, extraUnits = 0)` (`:348-466`).
    pub fn unit_cost(&self, t: UnitType, st: &State, player: SmallId, extra_units: f64) -> Gold {
        let p = st.player(player);
        self.unit_cost_with(
            t,
            &CostInputs {
                player_type: p.player_type(),
                is_lobby_creator: p.is_lobby_creator(),
                counts: |ty| (f64::from(p.units_owned(ty)), f64::from(p.units_constructed(ty))),
                extra_units,
                mirvs_launched: f64::from(st.mirvs_launched()),
            },
        )
    }
    /// `unitCost(type)` (`:348-417`) applied to one call's inputs.
    pub(crate) fn unit_cost_with<F: Fn(UnitType) -> (f64, f64)>(&self, t: UnitType, inp: &CostInputs<F>) -> Gold {
        use UnitType::*;
        match t {
            Warship => self.cost_wrapper(inp, |n| js::min(1_000_000.0, (n + 1.0) * 250_000.0), &[Warship]),
            Port => self.cost_wrapper(inp, |n| js::min(1_000_000.0, pow2(n) * 125_000.0), &[Port, Factory]),
            AtomBomb => self.cost_wrapper(inp, |_| 750_000.0, &[AtomBomb]),
            HydrogenBomb => self.cost_wrapper(inp, |_| 5_000_000.0, &[HydrogenBomb]),
            Mirv => {
                // TS: `:364-374` (no costWrapper)
                if inp.player_type == PlayerType::Human && self.has_infinite_gold_for(inp.is_lobby_creator) {
                    return 0;
                }
                25_000_000 + from_number_strict(inp.mirvs_launched) * 15_000_000
            }
            MissileSilo => self.cost_wrapper(inp, |_| 1_000_000.0, &[MissileSilo]),
            DefensePost => self.cost_wrapper(inp, |n| js::min(250_000.0, (n + 1.0) * 50_000.0), &[DefensePost]),
            SamLauncher => self.cost_wrapper(inp, |n| js::min(3_000_000.0, (n + 1.0) * 1_500_000.0), &[SamLauncher]),
            City => self.cost_wrapper(inp, |n| js::min(1_000_000.0, pow2(n) * 125_000.0), &[City]),
            Factory => self.cost_wrapper(inp, |n| js::min(1_000_000.0, pow2(n) * 125_000.0), &[Factory, Port]),
            TransportShip | Shell | SamMissile | MirvWarhead | TradeShip | Train => 0,
        }
    }
    /// `hasInfiniteTroopsForInfo(playerInfo)` (`:419-425`).
    fn has_infinite_troops_for_info(&self, info: &PlayerInfo) -> bool {
        if self.infinite_troops() {
            return true;
        }
        self.gc.host_cheats.as_ref().and_then(|h| h.infinite_troops).unwrap_or(false) && info.is_lobby_creator
    }
    /// `goldMultiplierFor(player)` (`:427-434`): `if (hc?.goldMultiplier && isLobbyCreator)`
    /// (truthy: not null/undefined, not 0, not NaN).
    fn gold_multiplier_for_creator(&self, is_lobby_creator: bool) -> f64 {
        let base = self.gold_multiplier();
        let hm = self.gc.host_cheats.as_ref().and_then(|h| h.gold_multiplier.flatten());
        if let Some(m) = hm
            && m != 0.0
            && !m.is_nan()
            && is_lobby_creator
        {
            return m;
        }
        base
    }
    /// `conquerGoldAmount(captured)` (`:436-445`; `gold / 2n` truncates).
    pub fn conquer_gold_amount(&self, captured: &impl PlayerLike) -> Gold {
        self.conquer_gold_amount_with(captured.player_type(), captured.gold())
    }
    pub(crate) fn conquer_gold_amount_with(&self, t: PlayerType, gold: Gold) -> Gold {
        if t == PlayerType::Bot || t == PlayerType::Nation { gold } else { div_strict(gold, 2) }
    }
    /// `startingGoldFor(playerInfo)` (`:447-454`).
    fn starting_gold_for(&self, info: &PlayerInfo) -> Gold {
        // TS: `BigInt(startingGold ?? 0)` (zod uint)
        let base = from_number_strict(self.gc.starting_gold.flatten().map_or(0.0, |g| g as f64));
        let hc_gold = self.gc.host_cheats.as_ref().and_then(|h| h.starting_gold.flatten());
        if let Some(g) = hc_gold
            && g != 0
            && info.is_lobby_creator
        {
            return base + from_number_strict(g as f64);
        }
        base
    }
    /// `costWrapper(costFn, ...types)` (`:456-484`).
    fn cost_wrapper<F: Fn(UnitType) -> (f64, f64)>(
        &self,
        inp: &CostInputs<F>,
        cost_fn: impl Fn(f64) -> f64,
        types: &[UnitType],
    ) -> Gold {
        if inp.player_type == PlayerType::Human && self.has_infinite_gold_for(inp.is_lobby_creator) {
            return 0;
        }
        let num_units = types.iter().fold(0.0, |acc, &ty| {
            let (owned, constructed) = (inp.counts)(ty);
            acc + js::min(owned, constructed)
        });
        from_number_strict(cost_fn(num_units + inp.extra_units))
    }
    /// `Math.floor(sender.troops() / 3)`.
    pub fn default_donation_amount(&self, sender: &impl PlayerLike) -> f64 {
        js::floor(sender.troops() / 3.0)
    }
    pub fn donate_cooldown(&self) -> Tick {
        10 * 10
    }
    pub fn embargo_all_cooldown(&self) -> Tick {
        10 * 10
    }
    pub fn emoji_message_duration(&self) -> Tick {
        5 * 10
    }
    pub fn emoji_message_window(&self) -> Tick {
        5 * 10
    }
    pub fn emoji_message_limit(&self) -> f64 {
        5.0
    }
    pub fn quick_chat_cooldown(&self) -> Tick {
        3 * 10
    }
    pub fn target_duration(&self) -> Tick {
        10 * 10
    }
    pub fn target_cooldown(&self) -> Tick {
        15 * 10
    }
    pub fn alliance_request_cooldown(&self) -> Tick {
        30 * 10
    }
    pub fn temporary_embargo_duration(&self) -> Tick {
        300 * 10
    }
    pub fn min_distance_between_players(&self) -> f64 {
        30.0
    }
    pub fn boat_max_number(&self) -> f64 {
        if self.is_unit_disabled(UnitType::TransportShip) {
            return 0.0;
        }
        3.0
    }
    pub fn num_bots(&self) -> f64 {
        self.bots()
    }
    /// `attackLogic(input)` (`:532-623`).
    pub fn attack_logic(&self, input: &AttackLogicInput) -> AttackLogicResult {
        self.attack_logic_code(input, input.terrain as u8)
    }
    /// `attackLogic` with the terrain as its numeric TS value (the vector suite passes 99).
    pub(crate) fn attack_logic_code(&self, input: &AttackLogicInput, terrain: u8) -> AttackLogicResult {
        let attack_troops = input.attack_troops;
        let attacker = input.attacker;
        let (mut mag, mut tile_cost) = terrain_attack_base(terrain);

        if input.defender.is_some() && input.defender_has_defense_post {
            mag *= self.defense_post_defense_bonus();
            tile_cost *= self.defense_post_speed_bonus();
        }
        if let Some(fr) = input.fallout_ratio {
            let fallout = self.fallout_defense_modifier(fr);
            mag *= fallout;
            tile_cost *= fallout;
        }

        let Some(defender) = input.defender else {
            let tick_budget = input.border_size * 2.0;
            return AttackLogicResult {
                attacker_troop_loss: mag / (if attacker.player_type == PlayerType::Bot { 10.0 } else { 5.0 }),
                defender_troop_loss: 0.0,
                tick_fraction: within(
                    (TERRA_NULLIUS_COST_SCALE * tile_cost) / attack_troops,
                    TERRA_NULLIUS_MIN_COST,
                    TERRA_NULLIUS_MAX_COST,
                ) / tick_budget,
            };
        };

        if defender.is_disconnected_teammate {
            mag = 0.0;
        }
        if (attacker.player_type == PlayerType::Human || attacker.player_type == PlayerType::Nation)
            && defender.player_type == PlayerType::Bot
        {
            mag *= BOT_DEFENDER_LOSS_MULT;
        }

        // largeTerritoryBonus(n, depth) = 1 - depth * S(n) (see `territory_sigmoid`)
        let log_mid = log(LARGE_TERRITORY_MIDPOINT);
        let s_attacker = territory_sigmoid(attacker.num_tiles, log_mid);
        let large_attacker_bonus = 1.0 - LARGE_ATTACKER_DEPTH * s_attacker;
        let large_defender_bonus = 1.0 - LARGE_DEFENDER_DEPTH * territory_sigmoid(defender.num_tiles, log_mid);

        let traitor_loss_mod = if defender.is_traitor { self.traitor_defense_debuff() } else { 1.0 };
        let traitor_cost_mod = if defender.is_traitor { self.traitor_speed_debuff() } else { 1.0 };

        let defender_troop_loss = defender.troops / defender.num_tiles;

        let troop_ratio = defender.troops / attack_troops;
        let attacker_troop_loss = mag
            * traitor_loss_mod
            * within(troop_ratio, CHEAPEST_TROOP_RATIO, 2.0)
            * (ATTACKER_LOSS_BASE * large_attacker_bonus * large_defender_bonus
                + ATTACKER_LOSS_PER_DENSITY * defender_troop_loss);

        let speed_cost = (within(troop_ratio, 0.82, 7.5) * within(troop_ratio / 20.0, 1.0, 50.0)) / SPEED_COST_DIVISOR;
        let large_attacker_speed_bonus = 1.0 - LARGE_ATTACKER_SPEED_DEPTH * s_attacker;
        AttackLogicResult {
            attacker_troop_loss,
            defender_troop_loss,
            tick_fraction: (speed_cost
                * tile_cost
                * large_attacker_speed_bonus
                * large_defender_bonus
                * traitor_cost_mod)
                / input.border_size,
        }
    }
    /// `boatAttackAmount(attacker, defender)` = `Math.floor(attacker.troops() / 5)` (`:625-630`).
    pub fn boat_attack_amount(&self, attacker: &impl PlayerLike, _defender: Option<&impl PlayerLike>) -> f64 {
        js::floor(attacker.troops() / 5.0)
    }
    pub fn warship_shell_lifetime(&self) -> f64 {
        20.0
    }
    pub fn trade_ship_short_range_debuff(&self) -> f64 {
        300.0
    }
    /// `within(totalPorts / 3, 4, totalPorts)`.
    pub fn proximity_bonus_ports_nb(&self, total_ports: f64) -> f64 {
        within(total_ports / 3.0, 4.0, total_ports)
    }
    /// `attackAmount(attacker, defender)` (`:644-650`).
    pub fn attack_amount(&self, attacker: &impl PlayerLike, _defender: Option<&impl PlayerLike>) -> f64 {
        self.attack_amount_with(attacker.player_type(), attacker.troops())
    }
    pub(crate) fn attack_amount_with(&self, t: PlayerType, troops: f64) -> f64 {
        if t == PlayerType::Bot { troops / 20.0 } else { troops / 5.0 }
    }
    /// `startManpower(playerInfo)` (`:652-674`).
    pub fn start_manpower(&self, info: &PlayerInfo) -> f64 {
        if info.player_type == PlayerType::Bot {
            return 10_000.0;
        }
        if info.player_type == PlayerType::Nation {
            return match self.gc.difficulty {
                Difficulty::Easy => 12_500.0,
                Difficulty::Medium => 18_750.0,
                Difficulty::Hard => 25_000.0,
                Difficulty::Impossible => 31_250.0,
            };
        }
        if self.gc.tutorial == Some(true) {
            return 60_000.0;
        }
        if self.has_infinite_troops_for_info(info) { 1_000_000.0 } else { 25_000.0 }
    }
    /// `goldAdditionRate(player)` (`:676-685`; strict `BigInt(Math.floor(..))`).
    pub fn gold_addition_rate(&self, player: &impl PlayerLike) -> Gold {
        self.gold_addition_rate_with(player.player_type(), player.is_lobby_creator())
    }
    pub(crate) fn gold_addition_rate_with(&self, t: PlayerType, is_lobby_creator: bool) -> Gold {
        let multiplier = self.gold_multiplier_for_creator(is_lobby_creator);
        let base_rate: i64 = if t == PlayerType::Bot { 50 } else { 100 };
        from_floor_strict(base_rate as f64 * multiplier)
    }
    /// `nukeSpeed(unitType)` (`:687-698`); other types throw.
    pub fn nuke_speed(&self, t: UnitType) -> f64 {
        match t {
            UnitType::AtomBomb | UnitType::HydrogenBomb => 10.0,
            UnitType::Mirv => 15.0,
            UnitType::MirvWarhead => 22.0,
            _ => of_det::ts_throw!("Unknown nuke type: {}", t.as_str()),
        }
    }
    pub fn mirv_normalize_target_ticks(&self) -> f64 {
        14.0
    }
    pub fn default_nuke_targetable_range(&self) -> f64 {
        150.0
    }
    pub fn default_sam_range(&self) -> f64 {
        70.0
    }
    /// `Math.floor(SAMCooldown() / 2)`.
    pub fn sam_upgrade_duration(&self) -> i32 {
        // int: floor(90 / 2) = 45
        js::floor(self.sam_cooldown() / 2.0) as i32
    }
    /// `dynamicSamRange(sam, currentTick)` (`:716-729`).
    pub fn dynamic_sam_range(&self, st: &State, sam: UnitRef, current_tick: Tick) -> f64 {
        let u = st.unit(sam);
        let state = u.sam_launcher_state().map(|s| SamUpgrade {
            upgrade_start_tick: s.upgrade_start_tick.map(f64::from),
            target_level: f64::from(s.target_level),
            start_range: s.start_range,
            duration: Some(f64::from(s.duration)),
        });
        self.dynamic_sam_range_with(f64::from(u.level()), state, f64::from(current_tick))
    }
    pub(crate) fn dynamic_sam_range_with(&self, level: f64, state: Option<SamUpgrade>, current_tick: f64) -> f64 {
        let Some(state) = state else { return self.sam_range(level) };
        let Some(upgrade_start_tick) = state.upgrade_start_tick else { return self.sam_range(level) };
        let duration = state.duration.unwrap_or(f64::from(self.sam_upgrade_duration()));
        let elapsed = current_tick - upgrade_start_tick;
        if elapsed >= duration {
            return self.sam_range(state.target_level);
        }
        let target_range = self.sam_range(state.target_level);
        let diff = target_range - state.start_range;
        state.start_range + (diff * elapsed) / duration
    }
    pub fn default_sam_missile_speed(&self) -> f64 {
        12.0
    }
    /// `nukeDeathFactor(nukeType, humans, tilesOwned, maxTroops)` (`:736-752`).
    pub fn nuke_death_factor(&self, nuke_type: UnitType, humans: f64, tiles_owned: f64, max_troops: f64) -> f64 {
        if nuke_type != UnitType::MirvWarhead {
            return (5.0 * humans) / js::max(1.0, tiles_owned);
        }
        let target_troops = 0.03 * max_troops;
        let excess_troops = js::max(0.0, humans - target_troops);
        let scaling_factor = 500.0;
        let steepness = 2.0;
        let normalized_excess = excess_troops / max_troops;
        scaling_factor * (1.0 - exp(-steepness * normalized_excess))
    }
    pub fn shell_lifetime(&self) -> f64 {
        50.0
    }
    pub fn warship_patrol_range(&self) -> f64 {
        100.0
    }
    pub fn warship_targetting_range(&self) -> f64 {
        130.0
    }
    pub fn warship_shell_attack_rate(&self) -> f64 {
        20.0
    }
    pub fn warship_docking_range(&self) -> f64 {
        5.0
    }
    pub fn warship_port_healing_bonus_per_level(&self) -> f64 {
        5.0
    }
    pub fn warship_retreat_health_percent(&self) -> f64 {
        75.0
    }
    pub fn warship_passive_healing(&self) -> f64 {
        1.0
    }
    pub fn warship_passive_healing_range(&self) -> f64 {
        150.0
    }
    pub fn warship_port_switch_threshold(&self) -> f64 {
        0.75
    }
    pub fn warship_max_veterancy(&self) -> f64 {
        3.0
    }
    pub fn warship_veterancy_shell_damage_bonus(&self) -> f64 {
        20.0
    }
    pub fn warship_veterancy_transport_kills(&self) -> f64 {
        10.0
    }
    pub fn warship_veterancy_trade_captures(&self) -> f64 {
        25.0
    }
    pub fn defense_post_shell_attack_rate(&self) -> f64 {
        100.0
    }
    pub fn safe_from_pirates_cooldown_max(&self) -> f64 {
        20.0
    }
    pub fn defense_post_targetting_range(&self) -> f64 {
        75.0
    }
}

/// `upstream/tests/EngineConfig.test.ts`, `tests/core/configuration/SpawnPhaseTurns.test.ts`
/// (`ConfigLoader.test.ts` is client env parsing, not engine) and the mandatory design test
/// "`BigInt(Math.floor(Inf))` in config → `TsThrow` (no clamp)" (FILE_MAP §4).
#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_methods)] // catch_unwind in tests
    use super::*;

    fn cfg(f: impl FnOnce(&mut GameConfig)) -> EngineConfig {
        let mut gc = GameConfig::default();
        f(&mut gc);
        EngineConfig::new(gc, false, false, false)
    }

    /// t409 H5: the inlined `1 - depth * territory_sigmoid(n, log(mid))` is bit-identical to
    /// `largeTerritoryBonus(n, depth)` for every depth `attackLogic` uses.
    #[test]
    fn territory_sigmoid_matches_large_territory_bonus() {
        let log_mid = log(LARGE_TERRITORY_MIDPOINT);
        let mut n = 0.0f64;
        while n < 3.0e6 {
            let s = territory_sigmoid(n, log_mid);
            for d in [LARGE_ATTACKER_DEPTH, LARGE_DEFENDER_DEPTH, LARGE_ATTACKER_SPEED_DEPTH] {
                assert_eq!((1.0 - d * s).to_bits(), large_territory_bonus(n, d).to_bits(), "n={n} depth={d}");
            }
            n = if n < 2000.0 { n + 1.0 } else { (n * 1.01).floor() };
        }
    }

    // TS: EngineConfig.test.ts:8-20 (unitInfo = Config.unitInfo + cost; cost is `unit_cost`)
    #[test]
    fn unit_info_is_the_base_table() {
        for ib in [false, true] {
            let c = cfg(|g| g.instant_build = ib);
            for &t in UnitType::ALL {
                assert_eq!(c.unit_info(t), c.base_unit_info(t));
            }
            assert_eq!(c.unit_info(UnitType::City).construction_duration, Some(if ib { 0.0 } else { 20.0 }));
        }
    }

    // TS: SpawnPhaseTurns.test.ts:6-26
    #[test]
    fn spawn_phase_turns() {
        assert_eq!(cfg(|g| g.game_type = GameType::Public).num_spawn_phase_turns(), 200.0);
        let rs = cfg(|g| {
            g.game_type = GameType::Public;
            g.random_spawn = true;
        });
        assert_eq!(rs.num_spawn_phase_turns(), 150.0);
        let sp = cfg(|g| {
            g.game_type = GameType::Singleplayer;
            g.random_spawn = true;
        });
        assert_eq!(sp.num_spawn_phase_turns(), 100.0);
    }

    /// `BigInt(Math.floor(x))` at `EngineConfig.ts:281,684` must throw for ±Inf/NaN (a
    /// `toInt` would clamp), while `toInt` at `:267` clamps ±Inf and throws on NaN.
    #[test]
    fn strict_bigint_sites_throw_on_infinity() {
        let msg = |f: &dyn Fn()| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
                .err()
                .map(|p| of_det::throw::as_ts_throw(&*p).expect("TsThrow").kind)
        };
        let inf = cfg(|g| g.gold_multiplier = Some(Some(f64::INFINITY)));
        assert!(
            msg(&|| {
                let _ = inf.trade_ship_gold_with(100.0, false);
            })
            .is_some(),
            "tradeShipGold :281"
        );
        assert!(
            msg(&|| {
                let _ = inf.gold_addition_rate_with(PlayerType::Human, false);
            })
            .is_some(),
            "goldAdditionRate :684"
        );
        // toInt clamps Infinity to MAX_SAFE_INTEGER (Util.ts:232-240)
        assert_eq!(inf.train_gold_with(TrainRelation::Ally, 0.0, false), 9_007_199_254_740_991);
        let nan = cfg(|g| g.gold_multiplier = Some(Some(f64::NAN)));
        assert!(
            msg(&|| {
                let _ = nan.train_gold_with(TrainRelation::Ally, 0.0, false);
            })
            .is_some(),
            "toInt(NaN)"
        );
        assert!(
            msg(&|| {
                let _ = nan.trade_ship_gold_with(1.0, false);
            })
            .is_some()
        );
    }

    #[test]
    fn host_cheat_multiplier_is_truthy_checked() {
        use crate::api::game_config::HostCheats;
        let c = cfg(|g| {
            g.gold_multiplier = Some(Some(2.0));
            g.host_cheats = Some(HostCheats { gold_multiplier: Some(Some(0.0)), ..HostCheats::default() });
        });
        // `hc.goldMultiplier = 0` is falsy → base multiplier
        assert_eq!(c.gold_addition_rate_with(PlayerType::Human, true), 200);
        let c = cfg(|g| g.host_cheats = Some(HostCheats { gold_multiplier: Some(Some(3.0)), ..HostCheats::default() }));
        assert_eq!(c.gold_addition_rate_with(PlayerType::Human, true), 300);
        assert_eq!(c.gold_addition_rate_with(PlayerType::Human, false), 100);
        assert_eq!(c.gold_addition_rate_with(PlayerType::Bot, true), 150);
    }
}
