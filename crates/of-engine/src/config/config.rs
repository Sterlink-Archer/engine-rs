//! The `Config` base-class methods (`engine-lib/src/configuration/Config.ts:87-460`), on the
//! single [`EngineConfig`] struct. Owner: **P3**.
//!
//! - No base method calls an `EngineConfig` override virtually: the only override is
//!   `unitInfo` (`EngineConfig.ts:338-346`), and nothing in `Config.ts` calls `this.unitInfo`
//!   (checked by grep at the pin), so a plain inherent method per TS method is exact
//!   (PORTING_GUIDE §10).
//! - `unitInfoCache` (`:88`) is the precomputed [`EngineConfig::unit_info`] table, filled by
//!   [`EngineConfig::new`] from [`EngineConfig::base_unit_info`]. The base table depends only
//!   on `instantBuild()`, which is fixed per config, so precomputing is unobservable.
//! - Formula methods take `&impl PlayerLike` (ARCHITECTURE §6), so the `config` vector suite
//!   runs them on duck-typed stubs (`config/vectors.rs`).

use crate::api::game_config::GameConfig;
use crate::api::types::{Difficulty, GameType, PlayerType, Tick, UnitInfo, UnitType};
use crate::api::views::{PlayerLike, UnitLike};
use crate::config::engine_config::EngineConfig;
use of_det::detmath::pow;
use of_det::js;
use of_det::util::DoomsdaySpeed;

/// `NukeMagnitude` (`:15-18`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NukeMagnitude {
    pub inner: f64,
    pub outer: f64,
}

/// `DEFAULT_SPAWN_IMMUNITY_TICKS = 5 * 10` (`:20`).
const DEFAULT_SPAWN_IMMUNITY_TICKS: Tick = 50;

/// `SAM_CONSTRUCTION_TICKS = 30 * 10` (`:23`).
pub const SAM_CONSTRUCTION_TICKS: i32 = 300;
/// `PERCENT_TILES_OWNED_TO_WIN = 80` (`:69`).
pub const PERCENT_TILES_OWNED_TO_WIN: f64 = 80.0;

/// `doomsdayClockConfig()` result (`typeof DOOMSDAY_CLOCK_DEFAULTS`, `:24-41`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DoomsdayClockSettings {
    pub enabled: bool,
    pub speed: DoomsdaySpeed,
    pub warn_seconds: f64,
    pub drain_start_percent: f64,
    pub drain_max_percent: f64,
    pub drain_ramp_seconds: f64,
    pub drain_floor_percent: f64,
    pub floor_start_percent: f64,
    pub floor_decay_seconds: f64,
    pub rot_death_seconds: f64,
    pub rot_grain_seconds: f64,
    pub rot_speckle_percent: f64,
    pub warship_drain_start_percent: f64,
    pub warship_drain_max_percent: f64,
    pub warship_drain_curve_exponent: f64,
}

/// `DOOMSDAY_CLOCK_DEFAULTS` (`:35-68`).
const DOOMSDAY_CLOCK_DEFAULTS: DoomsdayClockSettings = DoomsdayClockSettings {
    enabled: false,
    speed: DoomsdaySpeed::Normal,
    warn_seconds: 30.0,
    drain_start_percent: 2.0,
    drain_max_percent: 5.0,
    drain_ramp_seconds: 90.0,
    drain_floor_percent: 5.0,
    floor_start_percent: 40.0,
    floor_decay_seconds: 90.0,
    rot_death_seconds: 150.0,
    rot_grain_seconds: 10.0,
    rot_speckle_percent: 15.0,
    warship_drain_start_percent: 1.0,
    warship_drain_max_percent: 50.0,
    warship_drain_curve_exponent: 8.0,
};

/// `overtimeConfig()` result (`typeof OVERTIME_DEFAULTS`, `:45-49`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OvertimeSettings {
    pub enabled: bool,
    pub start_minutes: f64,
    pub drop_percent_per_minute: f64,
}

/// `OVERTIME_DEFAULTS` (`:76-80`).
const OVERTIME_DEFAULTS: OvertimeSettings =
    OvertimeSettings { enabled: false, start_minutes: 30.0, drop_percent_per_minute: 2.0 };

impl EngineConfig {
    pub fn is_replay(&self) -> bool {
        self.is_replay
    }
    pub fn is_intentional_spectator(&self) -> bool {
        self.spectator
    }
    pub fn traitor_defense_debuff(&self) -> f64 {
        0.5
    }
    pub fn traitor_duration(&self) -> Tick {
        30 * 10
    }
    /// `doomsdayClockConfig()` (`:107-129`): only `enabled` and `speed` are wire-configurable.
    pub fn doomsday_clock_config(&self) -> DoomsdayClockSettings {
        let c = self.gc.doomsday_clock.as_ref();
        let d = DOOMSDAY_CLOCK_DEFAULTS;
        DoomsdayClockSettings {
            // TS: `c?.enabled ?? d.enabled`
            enabled: c.and_then(|c| c.enabled).unwrap_or(d.enabled),
            // TS: `c?.speed ?? d.speed` (zod restricts the name to the four speeds)
            speed: c.and_then(|c| c.speed).map(|s| DoomsdaySpeed::parse(s.as_str())).unwrap_or(d.speed),
            ..d
        }
    }
    /// `overtimeConfig()` (`:131-140`).
    pub fn overtime_config(&self) -> OvertimeSettings {
        let c = self.gc.overtime.as_ref();
        let d = OVERTIME_DEFAULTS;
        OvertimeSettings {
            enabled: c.and_then(|c| c.enabled).unwrap_or(d.enabled),
            // `startMinutes` is a zod uint: exact in f64.
            start_minutes: c.and_then(|c| c.start_minutes).map(|m| m as f64).unwrap_or(d.start_minutes),
            drop_percent_per_minute: d.drop_percent_per_minute,
        }
    }
    /// `spawnImmunityDuration()` (`:141-145`): `spawnImmunityDuration ?? 50` (`??` folds `null`).
    pub fn spawn_immunity_duration(&self) -> Tick {
        match self.gc.spawn_immunity_duration.flatten() {
            // int: a zod uint tick count; ticks are i32 throughout the port (`Tick`).
            Some(t) => i32::try_from(t).unwrap_or(i32::MAX),
            None => DEFAULT_SPAWN_IMMUNITY_TICKS,
        }
    }
    pub fn nation_spawn_immunity_duration(&self) -> Tick {
        DEFAULT_SPAWN_IMMUNITY_TICKS
    }
    pub fn has_extended_spawn_immunity(&self) -> bool {
        self.spawn_immunity_duration() > DEFAULT_SPAWN_IMMUNITY_TICKS
    }
    pub fn game_config(&self) -> &GameConfig {
        &self.gc
    }
    pub fn city_troop_increase(&self) -> f64 {
        250_000.0
    }
    pub fn ms_per_tick(&self) -> f64 {
        100.0
    }
    /// `SAMCooldown()`.
    pub fn sam_cooldown(&self) -> f64 {
        90.0
    }
    /// `SiloCooldown()`.
    pub fn silo_cooldown(&self) -> f64 {
        90.0
    }
    pub fn defense_post_range(&self) -> f64 {
        30.0
    }
    /// `disabledUnits?.includes(unitType) ?? false` (`:180-182`).
    pub fn is_unit_disabled(&self, t: UnitType) -> bool {
        self.gc.disabled_units.as_ref().is_some_and(|v| v.contains(&t))
    }
    pub fn instant_build(&self) -> bool {
        self.gc.instant_build
    }
    /// `customAllianceDuration === 0 || (disableAlliances ?? false)` (`:186-193`).
    pub fn disable_alliances(&self) -> bool {
        self.gc.custom_alliance_duration.flatten() == Some(0) || self.gc.disable_alliances.flatten().unwrap_or(false)
    }
    pub fn is_random_spawn(&self) -> bool {
        self.gc.random_spawn
    }
    pub fn infinite_troops(&self) -> bool {
        self.gc.infinite_troops
    }
    pub fn donate_troops(&self) -> bool {
        self.gc.donate_troops
    }
    pub fn train_station_max_range(&self) -> f64 {
        110.0
    }
    /// `Config.unitInfo(type)` (`:206-266`), the base table `EngineConfig.unitInfo` extends.
    pub fn base_unit_info(&self, t: UnitType) -> UnitInfo {
        let ib = self.instant_build();
        let cd = |ticks: f64| Some(if ib { 0.0 } else { ticks });
        match t {
            UnitType::Warship => UnitInfo { max_health: Some(1000.0), ..UnitInfo::default() },
            UnitType::Shell => UnitInfo { damage: Some(250.0), ..UnitInfo::default() },
            UnitType::Port => {
                UnitInfo { construction_duration: cd(5.0 * 10.0), upgradable: Some(true), ..UnitInfo::default() }
            }
            UnitType::MissileSilo => {
                UnitInfo { construction_duration: cd(10.0 * 10.0), upgradable: Some(true), ..UnitInfo::default() }
            }
            UnitType::DefensePost => UnitInfo { construction_duration: cd(5.0 * 10.0), ..UnitInfo::default() },
            UnitType::SamLauncher => UnitInfo {
                construction_duration: cd(SAM_CONSTRUCTION_TICKS as f64),
                upgradable: Some(true),
                ..UnitInfo::default()
            },
            UnitType::City | UnitType::Factory => {
                UnitInfo { construction_duration: cd(2.0 * 10.0), upgradable: Some(true), ..UnitInfo::default() }
            }
            UnitType::TransportShip
            | UnitType::SamMissile
            | UnitType::AtomBomb
            | UnitType::HydrogenBomb
            | UnitType::Mirv
            | UnitType::MirvWarhead
            | UnitType::TradeShip
            | UnitType::Train => UnitInfo::default(),
        }
    }
    /// `hasInfiniteTroopsFor(player)` (`:268-274`).
    fn has_infinite_troops_for(&self, player: &impl PlayerLike) -> bool {
        if self.infinite_troops() {
            return true;
        }
        self.gc.host_cheats.as_ref().and_then(|h| h.infinite_troops).unwrap_or(false) && player.is_lobby_creator()
    }
    pub fn deletion_mark_duration(&self) -> Tick {
        30 * 10
    }
    pub fn delete_unit_cooldown(&self) -> Tick {
        30 * 10
    }
    pub fn alliance_request_duration(&self) -> Tick {
        20 * 10
    }
    /// `allianceDuration()` (`:282-287`): `typeof m === "number" && m > 0 ? m * 60 * 10 : 3000`.
    pub fn alliance_duration(&self) -> Tick {
        match self.gc.custom_alliance_duration.flatten() {
            // int: zod uint minutes (1-15 in practice); `m * 600` fits i32.
            Some(m) if m > 0 => i32::try_from(m * 60 * 10).unwrap_or(i32::MAX),
            _ => 300 * 10,
        }
    }
    /// `percentageTilesOwnedToWinTenths(elapsedGameSeconds)` (`:292-314`).
    pub fn percentage_tiles_owned_to_win_tenths(&self, elapsed_game_seconds: f64) -> f64 {
        // TS: `winThresholdTenths ?? PERCENT_TILES_OWNED_TO_WIN * 10`
        let base = match self.gc.win_threshold_tenths.flatten() {
            Some(t) => t as f64, // zod uint: exact
            None => PERCENT_TILES_OWNED_TO_WIN * 10.0,
        };
        let sd = self.overtime_config();
        if !sd.enabled {
            return base;
        }
        let seconds_past_start = js::floor(elapsed_game_seconds) - sd.start_minutes * 60.0;
        if seconds_past_start <= 0.0 {
            return base;
        }
        js::max(0.0, base - js::floor((seconds_past_start * sd.drop_percent_per_minute) / 60.0) * 10.0)
    }
    pub fn percentage_tiles_owned_to_win(&self, elapsed_game_seconds: f64) -> f64 {
        self.percentage_tiles_owned_to_win_tenths(elapsed_game_seconds) / 10.0
    }
    pub fn army_limit_warning_threshold(&self) -> f64 {
        0.8
    }
    /// `numSpawnPhaseTurns()` (`:323-331`).
    pub fn num_spawn_phase_turns(&self) -> f64 {
        if self.gc.game_type == GameType::Singleplayer {
            return 100.0;
        }
        if self.is_random_spawn() {
            return 150.0;
        }
        200.0
    }
    /// `maxTroops(player)` (`:335-338`).
    pub fn max_troops(&self, player: &impl PlayerLike) -> f64 {
        self.troop_cap(player, true)
    }
    pub fn max_troops_without_cities(&self, player: &impl PlayerLike) -> f64 {
        self.troop_cap(player, false)
    }
    /// `troopCap(player, withCities)` (`:345-383`).
    fn troop_cap(&self, player: &impl PlayerLike, with_cities: bool) -> f64 {
        let city_troops = if with_cities {
            // TS: `.filter(!underConstruction).map(level).reduce((a, b) => a + b, 0) * cityTroopIncrease()`
            player
                .units(UnitType::City)
                .filter(|u| !u.is_under_construction())
                .map(|c| c.level())
                .fold(0.0, |a, b| a + b)
                * self.city_troop_increase()
        } else {
            0.0
        };
        let max_troops = if player.player_type() == PlayerType::Human && self.has_infinite_troops_for(player) {
            1_000_000_000.0
        } else {
            2.0 * (pow(player.num_tiles_owned(), 0.6) * 1000.0 + 50000.0) + city_troops
        };

        if self.gc.tutorial == Some(true) && player.player_type() != PlayerType::Human {
            return max_troops * 0.1;
        }
        if player.player_type() == PlayerType::Bot {
            return max_troops / 3.0;
        }
        if player.player_type() == PlayerType::Human {
            return max_troops;
        }
        match self.gc.difficulty {
            Difficulty::Easy => max_troops * 0.5,
            Difficulty::Medium => max_troops * 0.75,
            Difficulty::Hard => max_troops * 1.0,
            Difficulty::Impossible => max_troops * 1.25,
        }
    }
    /// `troopIncreaseRate(player)` (`:385-417`).
    pub fn troop_increase_rate(&self, player: &impl PlayerLike) -> f64 {
        let max = self.max_troops(player);
        let mut to_add = 10.0 + pow(player.troops(), 0.73) / 4.0;
        let ratio = 1.0 - player.troops() / max;
        to_add *= ratio;
        if player.player_type() == PlayerType::Bot {
            to_add *= 0.5;
        }
        if player.player_type() == PlayerType::Nation {
            match self.gc.difficulty {
                Difficulty::Easy => to_add *= 0.9,
                Difficulty::Medium => to_add *= 0.95,
                Difficulty::Hard => to_add *= 1.0,
                Difficulty::Impossible => to_add *= 1.05,
            }
        }
        js::min(player.troops() + to_add, max) - player.troops()
    }
    /// `nukeMagnitudes(unitType)` (`:419-432`); other types throw.
    pub fn nuke_magnitudes(&self, t: UnitType) -> NukeMagnitude {
        match t {
            UnitType::MirvWarhead => NukeMagnitude { inner: 12.0, outer: 18.0 },
            UnitType::AtomBomb => {
                if self.gc.tutorial == Some(true) {
                    return NukeMagnitude { inner: 8.0, outer: 16.0 };
                }
                NukeMagnitude { inner: 12.0, outer: 30.0 }
            }
            UnitType::HydrogenBomb => NukeMagnitude { inner: 80.0, outer: 100.0 },
            _ => of_det::ts_throw!("Unknown nuke type: {}", t.as_str()),
        }
    }
    pub fn nuke_alliance_break_threshold(&self) -> f64 {
        100.0
    }
    /// `samRange(level)` = `maxSamRange() - 480 / (level + 5)` (`:438-441`).
    pub fn sam_range(&self, level: f64) -> f64 {
        self.max_sam_range() - 480.0 / (level + 5.0)
    }
    pub fn max_sam_range(&self) -> f64 {
        150.0
    }
    pub fn structure_min_dist(&self) -> f64 {
        15.0
    }
    pub fn warship_veterancy_health_bonus(&self) -> f64 {
        20.0
    }
    pub fn alliance_extension_prompt_offset(&self) -> f64 {
        300.0
    }
}
