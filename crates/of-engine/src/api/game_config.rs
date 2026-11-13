//! `GameConfig`, `GameStartInfo` and their parts (`engine-api/src/Schemas.ts`). Owner: P1.
//!
//! Scenario configs are already `GameConfigSchema`-parsed (SPEC §2.2), so this side only
//! needs faithful optionality and must **not** re-apply zod defaults (ARCHITECTURE §6):
//! - `.optional()` → `Option<T>`, omitted when absent;
//! - `.nullable()` (required key) → `Option<T>`, written as `null`;
//! - `.nullable().optional()` → `Option<Option<T>>` (absent ≠ null), via [`double_option`].
//!
//! Numbers: `zb.uint()`/`zb.int()` are zod `.int()` (safe integers) → `u64`/`i64`, which
//! hold every value zod accepts; `zb.float()` → `f64`. Unknown keys are ignored, as zod
//! strips them. Round-tripping every corpus `scenario.json` `gameStart` is a test
//! (`tests/api_corpus.rs`, ARCHITECTURE §12 G8).

use super::types::{ClientID, Difficulty, GameMapSize, GameMode, GameType, RankedType, UnitType};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Deserializer for `Option<Option<T>>` fields: a present `null` is `Some(None)`; an
/// absent key is `None` (with `#[serde(default)]`).
pub fn double_option<'de, T: Deserialize<'de>, D: Deserializer<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

/// `DoomsdayClockConfigSchema` (`Schemas.ts`).
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DoomsdayClockConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// `"slow" | "normal" | "fast" | "veryfast"` (`of_det::util::DoomsdaySpeed::parse`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<DoomsdaySpeedName>,
}

/// `z.enum(["slow", "normal", "fast", "veryfast"])`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DoomsdaySpeedName {
    Slow,
    Normal,
    Fast,
    Veryfast,
}

impl DoomsdaySpeedName {
    pub fn as_str(self) -> &'static str {
        match self {
            DoomsdaySpeedName::Slow => "slow",
            DoomsdaySpeedName::Normal => "normal",
            DoomsdaySpeedName::Fast => "fast",
            DoomsdaySpeedName::Veryfast => "veryfast",
        }
    }
}

/// `OvertimeConfigSchema`.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OvertimeConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_minutes: Option<u64>,
}

/// `GameConfigSchema.publicGameModifiers`.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicGameModifiers {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_compact: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_random_spawn: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_crowded: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_hard_nations: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starting_gold: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gold_multiplier: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_alliances_disabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_ports_disabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_nukes_disabled: Option<bool>,
    #[serde(default, rename = "isSAMsDisabled", skip_serializing_if = "Option::is_none")]
    pub is_sams_disabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_peace_time: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_water_nukes: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_doomsday_clock: Option<bool>,
}

/// `GameConfigSchema.nations`: `zb.uint({min: 1, max: 400}) | "default" | "disabled"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NationsConfig {
    Count(u64),
    Default,
    Disabled,
}

impl Serialize for NationsConfig {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            NationsConfig::Count(n) => s.serialize_u64(*n),
            NationsConfig::Default => s.serialize_str("default"),
            NationsConfig::Disabled => s.serialize_str("disabled"),
        }
    }
}

impl<'de> Deserialize<'de> for NationsConfig {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            N(u64),
            S(String),
        }
        match Raw::deserialize(d)? {
            Raw::N(n) => Ok(NationsConfig::Count(n)),
            Raw::S(s) if s == "default" => Ok(NationsConfig::Default),
            Raw::S(s) if s == "disabled" => Ok(NationsConfig::Disabled),
            Raw::S(s) => Err(serde::de::Error::custom(format!("bad nations value {s:?}"))),
        }
    }
}

/// `TeamCountConfigSchema`: `zb.uint() | "Duos" | "Trios" | "Quads" | "Humans Vs Nations"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TeamCountConfig {
    Count(u64),
    Duos,
    Trios,
    Quads,
    HumansVsNations,
}

impl TeamCountConfig {
    pub fn preset_str(self) -> Option<&'static str> {
        match self {
            TeamCountConfig::Count(_) => None,
            TeamCountConfig::Duos => Some(super::types::DUOS),
            TeamCountConfig::Trios => Some(super::types::TRIOS),
            TeamCountConfig::Quads => Some(super::types::QUADS),
            TeamCountConfig::HumansVsNations => Some(super::types::HUMANS_VS_NATIONS),
        }
    }
}

impl Serialize for TeamCountConfig {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match (self, self.preset_str()) {
            (TeamCountConfig::Count(n), _) => s.serialize_u64(*n),
            (_, Some(p)) => s.serialize_str(p),
            (_, None) => unreachable!("every non-Count variant has a preset string"),
        }
    }
}

impl<'de> Deserialize<'de> for TeamCountConfig {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            N(u64),
            S(String),
        }
        Ok(match Raw::deserialize(d)? {
            Raw::N(n) => TeamCountConfig::Count(n),
            Raw::S(s) => match s.as_str() {
                "Duos" => TeamCountConfig::Duos,
                "Trios" => TeamCountConfig::Trios,
                "Quads" => TeamCountConfig::Quads,
                "Humans Vs Nations" => TeamCountConfig::HumansVsNations,
                _ => return Err(serde::de::Error::custom(format!("bad playerTeams value {s:?}"))),
            },
        })
    }
}

/// `GameConfigSchema.hostCheats`.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostCheats {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub infinite_gold: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub infinite_troops: Option<bool>,
    #[serde(default, deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
    pub gold_multiplier: Option<Option<f64>>,
    #[serde(default, deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
    pub starting_gold: Option<Option<u64>>,
}

/// `PoolConfigSchema`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PoolConfig {
    pub id: String,
    pub siblings: Vec<String>,
}

/// `GameConfigSchema` (field order as in `Schemas.ts`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameConfig {
    /// `z.enum(GameMapType)`: the map name; the generated enum is P3's `map::maps_gen`.
    pub game_map: String,
    pub difficulty: Difficulty,
    pub donate_gold: bool,
    pub donate_troops: bool,
    pub game_type: GameType,
    pub game_mode: GameMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ranked_type: Option<RankedType>,
    pub game_map_size: GameMapSize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doomsday_clock: Option<DoomsdayClockConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overtime: Option<OvertimeConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_game_modifiers: Option<PublicGameModifiers>,
    pub nations: NationsConfig,
    pub bots: u64,
    pub infinite_gold: bool,
    pub infinite_troops: bool,
    pub instant_build: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disable_nav_mesh: Option<bool>,
    #[serde(default, deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
    pub disable_alliances: Option<Option<bool>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disable_clan_tags: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_stats_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anonymize_names: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_reveals: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_reveal_public_ids: Option<Vec<String>>,
    #[serde(default, deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
    pub water_nukes: Option<Option<bool>>,
    pub random_spawn: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tutorial: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_players: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_public_ids: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trusted: Option<bool>,
    /// Minutes.
    #[serde(default, deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
    pub max_timer_value: Option<Option<u64>>,
    /// Tenths of a percent (755 = 75.5 %).
    #[serde(default, deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
    pub win_threshold_tenths: Option<Option<u64>>,
    /// Minutes; 0 disables alliances.
    #[serde(default, deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
    pub custom_alliance_duration: Option<Option<u64>>,
    /// Seconds.
    #[serde(default, deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
    pub start_delay: Option<Option<u64>>,
    /// Ticks.
    #[serde(default, deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
    pub spawn_immunity_duration: Option<Option<u64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled_units: Option<Vec<UnitType>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub player_teams: Option<TeamCountConfig>,
    #[serde(default, deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
    pub gold_multiplier: Option<Option<f64>>,
    #[serde(default, deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
    pub starting_gold: Option<Option<u64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_cheats: Option<HostCheats>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool: Option<PoolConfig>,
}

/// `PlayerSchema` (the lobby roster entry of `GameStartInfo.players`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameStartPlayer {
    #[serde(rename = "clientID")]
    pub client_id: ClientID,
    pub username: String,
    /// `ClanTagSchema` is `.nullable()` (required key): written as `null`.
    pub clan_tag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_lobby_creator: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub friends: Option<Vec<ClientID>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_index: Option<u64>,
}

/// `TribeSchema` (`.loose()`: unknown keys are kept).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tribe {
    pub name: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// `GameStartInfoSchema`. `gameID` is kept as given: corpus scenarios use short ids
/// (e.g. `"i1"`) that the zod `ID` regex would reject, and the engine only hashes it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameStartInfo {
    #[serde(rename = "gameID")]
    pub game_id: String,
    pub lobby_created_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listed: Option<bool>,
    pub config: GameConfig,
    pub players: Vec<GameStartPlayer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tribes: Option<Vec<Tribe>>,
}

impl Default for GameConfig {
    /// A **placeholder** config (empty map name, first variant of every required enum, every
    /// optional key absent). Only for `Default` state: pooled fork targets (overwritten by
    /// `clone_from`) and unit tests. Never a substitute for a zod-parsed `GameConfig`.
    fn default() -> Self {
        GameConfig {
            game_map: String::new(),
            difficulty: Difficulty::Easy,
            donate_gold: false,
            donate_troops: false,
            game_type: GameType::Singleplayer,
            game_mode: GameMode::Ffa,
            ranked_type: None,
            game_map_size: GameMapSize::Normal,
            doomsday_clock: None,
            overtime: None,
            public_game_modifiers: None,
            nations: NationsConfig::Default,
            bots: 0,
            infinite_gold: false,
            infinite_troops: false,
            instant_build: false,
            disable_nav_mesh: None,
            disable_alliances: None,
            disable_clan_tags: None,
            live_stats_enabled: None,
            anonymize_names: None,
            name_reveals: None,
            name_reveal_public_ids: None,
            water_nukes: None,
            random_spawn: false,
            tutorial: None,
            max_players: None,
            allowed_public_ids: None,
            trusted: None,
            max_timer_value: None,
            win_threshold_tenths: None,
            custom_alliance_duration: None,
            start_delay: None,
            spawn_immunity_duration: None,
            disabled_units: None,
            player_teams: None,
            gold_multiplier: None,
            starting_gold: None,
            host_cheats: None,
            pool: None,
        }
    }
}

// Plain deep-clone data (they live inside `Arc<EngineConfig>` / `State`).
crate::plain_copy!(DoomsdaySpeedName, NationsConfig, TeamCountConfig);
impl crate::plain::PlainState for DoomsdayClockConfig {}
impl crate::plain::PlainState for OvertimeConfig {}
impl crate::plain::PlainState for PublicGameModifiers {}
impl crate::plain::PlainState for HostCheats {}
impl crate::plain::PlainState for PoolConfig {}
impl crate::plain::PlainState for GameConfig {}
impl crate::plain::PlainState for GameStartPlayer {}
crate::refs_leaf!(
    DoomsdayClockConfig,
    OvertimeConfig,
    PublicGameModifiers,
    HostCheats,
    PoolConfig,
    GameConfig,
    GameStartPlayer
);

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absent_null_and_value_are_distinct() {
        let base = r#"{"gameMap":"World","difficulty":"Hard","donateGold":true,"donateTroops":false,
            "gameType":"Singleplayer","gameMode":"Team","gameMapSize":"Normal","bots":400,
            "infiniteGold":false,"infiniteTroops":false,"instantBuild":false,"randomSpawn":false"#;
        let a: GameConfig = serde_json::from_str(&format!("{base},\"nations\":\"default\"}}")).expect("absent");
        let n: GameConfig =
            serde_json::from_str(&format!("{base},\"nations\":\"disabled\",\"maxTimerValue\":null}}")).expect("null");
        let v: GameConfig = serde_json::from_str(&format!(
            "{base},\"maxTimerValue\":15,\"playerTeams\":\"Humans Vs Nations\",\"nations\":12}}"
        ))
        .expect("value");
        assert_eq!(a.max_timer_value, None);
        assert_eq!(n.max_timer_value, Some(None));
        assert_eq!(v.max_timer_value, Some(Some(15)));
        assert_eq!(v.player_teams, Some(TeamCountConfig::HumansVsNations));
        assert_eq!(v.nations, NationsConfig::Count(12));
        let ser = serde_json::to_value(&n).expect("ser");
        assert!(ser["maxTimerValue"].is_null() && ser.as_object().expect("obj").contains_key("maxTimerValue"));
        assert!(!serde_json::to_value(&a).expect("ser").as_object().expect("obj").contains_key("maxTimerValue"));
    }
}
