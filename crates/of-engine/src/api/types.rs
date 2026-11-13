//! Port of `engine-api/src/game/GameTypes.ts`: enums, `PlayerInfo`, `Cell`, `Nation`,
//! team names, unit-type groups. Owner: P1.
//!
//! String enums keep their TS string as the serde name and as `as_str()` (CS and
//! `Object.entries` ordering use it, PORTING_GUIDE §10). Numeric TS enums keep their
//! discriminants.
//!
//! Not here: `GameMapType` (generated from `Maps.gen.ts`, P3 `map::maps_gen`; configs carry
//! the map name as a string), view-only types (`MessageType`, `PlayerActions`,
//! `BuildableUnit`, `NameViewData`, …), and the per-unit state shapes (`WarshipState`,
//! `NukeState`, …), which are declared with P2's `Unit` (S0 step 3).

use crate::plain_copy;
use serde::{Deserialize, Serialize};

/// `PlayerID` (8-char base-36 from `nextID()` for bots/nations, the clientID-derived id for
/// humans). ASCII, so a Rust `String` is lossless (PORTING_GUIDE §9).
pub type PlayerID = String;
/// `ClientID`.
pub type ClientID = String;
/// `Team` (`type Team = string`).
pub type Team = String;

/// `AllPlayers` (`GameTypes.ts:52`): the emoji recipient meaning "everyone".
pub const ALL_PLAYERS: &str = "AllPlayers";
/// `MAX_UPGRADE_AMOUNT` (`GameTypes.ts:183`).
pub const MAX_UPGRADE_AMOUNT: u32 = 50;

/// Team-count presets (`GameTypes.ts:96-102`).
pub const DUOS: &str = "Duos";
pub const TRIOS: &str = "Trios";
pub const QUADS: &str = "Quads";
pub const HUMANS_VS_NATIONS: &str = "Humans Vs Nations";

/// `ColoredTeams` (`GameTypes.ts:104-115`), in declaration order.
pub mod colored_teams {
    pub const RED: &str = "Red";
    pub const BLUE: &str = "Blue";
    pub const TEAL: &str = "Teal";
    pub const PURPLE: &str = "Purple";
    pub const YELLOW: &str = "Yellow";
    pub const ORANGE: &str = "Orange";
    pub const GREEN: &str = "Green";
    pub const BOT: &str = "Bot";
    pub const HUMANS: &str = "Humans";
    pub const NATIONS: &str = "Nations";
    pub const ALL: [&str; 10] = [RED, BLUE, TEAL, PURPLE, YELLOW, ORANGE, GREEN, BOT, HUMANS, NATIONS];
}

macro_rules! string_enum {
    ($(#[$m:meta])* $name:ident { $($v:ident = $s:literal),* $(,)? }) => {
        $(#[$m])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
        pub enum $name { $(#[serde(rename = $s)] $v),* }
        impl $name {
            /// All variants in TS declaration order.
            pub const ALL: &'static [$name] = &[$($name::$v),*];
            /// The TS string value.
            pub fn as_str(self) -> &'static str {
                match self { $($name::$v => $s),* }
            }
            /// Parse the TS string value.
            pub fn from_ts(s: &str) -> Option<$name> {
                match s { $($s => Some($name::$v),)* _ => None }
            }
        }
        plain_copy!($name);
        /// CS v1: a TS string enum is its string value (request P8 #2).
        impl crate::canon::Canon for $name {
            fn canon(&self, _cx: &mut crate::canon::CanonCx<'_>) -> crate::canon::Cs {
                crate::canon::Cs::Str(self.as_str().to_owned())
            }
        }
        impl crate::canon::Uncanon for $name {
            fn uncanon(
                v: &crate::canon::Cs,
                _cx: &mut crate::canon::UncanonCx<'_>,
            ) -> Result<Self, crate::canon::CanonError> {
                v.as_str().and_then($name::from_ts).ok_or_else(|| {
                    crate::canon::CanonError::new(format!("expected {} string, got {v:?}", stringify!($name)))
                })
            }
        }
    };
}

string_enum! {
    /// `Difficulty` (`GameTypes.ts:73-78`).
    Difficulty { Easy = "Easy", Medium = "Medium", Hard = "Hard", Impossible = "Impossible" }
}
string_enum! {
    /// `GameType` (`GameTypes.ts:140-144`).
    GameType { Singleplayer = "Singleplayer", Public = "Public", Private = "Private" }
}
string_enum! {
    /// `GameMode` (`GameTypes.ts:149-152`).
    GameMode { Ffa = "Free For All", Team = "Team" }
}
string_enum! {
    /// `RankedType` (`GameTypes.ts:154-159`).
    RankedType { OneVOne = "1v1", TwoVTwo = "2v2", FreeForAll = "ffa", ClanWars = "clanwars" }
}
string_enum! {
    /// `GameMapSize` (`GameTypes.ts:169-172`).
    GameMapSize { Compact = "Compact", Normal = "Normal" }
}
string_enum! {
    /// `PlayerType` (`GameTypes.ts:302-306`).
    PlayerType { Bot = "BOT", Human = "HUMAN", Nation = "NATION" }
}
string_enum! {
    /// `TrainType` (`GameTypes.ts:221-225`).
    TrainType { Engine = "Engine", TailEngine = "TailEngine", Carriage = "Carriage" }
}
string_enum! {
    /// `UnitType` (`GameTypes.ts:202-219`), in TS declaration order.
    UnitType {
        TransportShip = "Transport",
        Warship = "Warship",
        Shell = "Shell",
        SamMissile = "SAMMissile",
        Port = "Port",
        AtomBomb = "Atom Bomb",
        HydrogenBomb = "Hydrogen Bomb",
        TradeShip = "Trade Ship",
        MissileSilo = "Missile Silo",
        DefensePost = "Defense Post",
        SamLauncher = "SAM Launcher",
        City = "City",
        Mirv = "MIRV",
        MirvWarhead = "MIRV Warhead",
        Train = "Train",
        Factory = "Factory",
    }
}

impl Default for UnitType {
    /// Placeholder for `Default` state only (the first TS variant).
    fn default() -> Self {
        UnitType::TransportShip
    }
}

impl UnitType {
    pub const COUNT: usize = 16;
    /// Position in TS declaration order (`[Option<V>; 16]` tables, PORTING_GUIDE §5.1).
    #[inline]
    pub fn index(self) -> usize {
        self as usize
    }
}

/// `unitTypeGroup` members (`GameTypes.ts:227-262`), in TS order.
pub mod unit_groups {
    use super::UnitType::{self, *};
    /// `Nukes`.
    pub const NUKES: &[UnitType] = &[AtomBomb, HydrogenBomb, MirvWarhead, Mirv];
    /// `BuildableAttacks`.
    pub const BUILDABLE_ATTACKS: &[UnitType] = &[AtomBomb, HydrogenBomb, Mirv, Warship];
    /// `Structures`.
    pub const STRUCTURES: &[UnitType] = &[City, DefensePost, SamLauncher, MissileSilo, Port, Factory];
    /// `BuildMenus` = `[...Structures, ...BuildableAttacks]`.
    pub const BUILD_MENUS: &[UnitType] =
        &[City, DefensePost, SamLauncher, MissileSilo, Port, Factory, AtomBomb, HydrogenBomb, Mirv, Warship];
    /// `PlayerBuildable` = `[...BuildMenus, TransportShip]`.
    pub const PLAYER_BUILDABLE: &[UnitType] = &[
        City,
        DefensePost,
        SamLauncher,
        MissileSilo,
        Port,
        Factory,
        AtomBomb,
        HydrogenBomb,
        Mirv,
        Warship,
        TransportShip,
    ];
    /// `group.has(t)`.
    #[inline]
    pub fn has(group: &[UnitType], t: UnitType) -> bool {
        group.contains(&t)
    }
}

/// `TerrainType` (numeric, `GameTypes.ts:294-300`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[repr(u8)]
pub enum TerrainType {
    Plains = 0,
    Highland = 1,
    Mountain = 2,
    Ocean = 3,
    Impassable = 4,
}

/// `Relation` (numeric, `GameTypes.ts:266-271`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
#[repr(u8)]
pub enum Relation {
    Hostile = 0,
    Distrustful = 1,
    Neutral = 2,
    Friendly = 3,
}

plain_copy!(TerrainType, Relation);

/// `class PlayerInfo` (`GameTypes.ts:308-331`). Field order = the snapshot record
/// (`Game v1`.humans).
#[derive(crate::StateClone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerInfo {
    /// Display username. `UsernameSchema` limits names to U+0020..U+00FF
    /// (`Schemas.ts` `RENDERABLE_NAME_CHARS`), so no lone surrogate can occur and a Rust
    /// `String` is lossless; nation and tribe names come from UTF-8 resources.
    pub name: String,
    pub player_type: PlayerType,
    /// `null` for tribes.
    #[serde(rename = "clientID")]
    pub client_id: Option<ClientID>,
    pub id: PlayerID,
    pub is_lobby_creator: bool,
    pub clan_tag: Option<String>,
    pub friends: Vec<ClientID>,
    /// Server-pinned team slot; `null` = assign normally.
    pub team_index: Option<u32>,
    pub nation_flag: Option<String>,
}

impl PlayerInfo {
    /// `new PlayerInfo(name, playerType, clientID, id, isLobbyCreator = false,
    /// clanTag = null, friends = [], teamIndex = null, nationFlag = null)`.
    pub fn new(
        name: impl Into<String>,
        player_type: PlayerType,
        client_id: Option<ClientID>,
        id: impl Into<PlayerID>,
    ) -> Self {
        PlayerInfo {
            name: name.into(),
            player_type,
            client_id,
            id: id.into(),
            is_lobby_creator: false,
            clan_tag: None,
            friends: Vec::new(),
            team_index: None,
            nation_flag: None,
        }
    }
    /// `displayName` = `formatPlayerDisplayName(name, clanTag)` (`GameTypes.ts:528-535`):
    /// an empty clan tag is falsy, so it gives the bare name.
    pub fn display_name(&self) -> String {
        match self.clan_tag.as_deref() {
            Some(tag) if !tag.is_empty() => format!("[{tag}] {}", self.name),
            _ => self.name.clone(),
        }
    }
}

/// `class Cell` (`GameTypes.ts:279-292`); coordinates are JS numbers (snapshot: `f64`).
#[derive(Clone, Copy, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct Cell {
    pub x: f64,
    pub y: f64,
}
plain_copy!(Cell);

impl Cell {
    pub fn new(x: f64, y: f64) -> Self {
        Cell { x, y }
    }
}

/// `class Nation` (`GameTypes.ts:273-277`).
#[derive(crate::StateClone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Nation {
    pub spawn_cell: Option<Cell>,
    pub player_info: PlayerInfo,
}

/// `SpawnArea` (`GameTypes.ts:87-92`), from the map manifest's team spawn areas.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct SpawnArea {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
plain_copy!(SpawnArea);

/// `Tick` (`GameTypes.ts:15`, `type Tick = number`). // int: ticks are a counter from 0,
/// far below 2^31 for any reachable game.
pub type Tick = i32;
/// `Gold` (`GameTypes.ts:17`, `type Gold = bigint`) → `i64` (ARCHITECTURE §7.2).
pub type Gold = i64;

/// `UnitInfo` (`GameTypes.ts:184-189`): the per-type static info. `EngineUnitInfo.cost` (the
/// `(game, player, extraUnits?) => Gold` closure, `Game.ts:197-201`) is not a field: it is
/// `EngineConfig::unit_cost` (ARCHITECTURE §6).
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct UnitInfo {
    pub max_health: Option<f64>,
    pub damage: Option<f64>,
    pub construction_duration: Option<f64>,
    pub upgradable: Option<bool>,
}
plain_copy!(UnitInfo);

/// `EmojiMessage.recipientID`: `number | typeof AllPlayers` (`GameTypes.ts:451-456`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum EmojiRecipient {
    /// A player's smallID.
    Player(crate::ids::SmallId),
    /// `AllPlayers`.
    AllPlayers,
}
plain_copy!(EmojiRecipient);

/// `EmojiMessage` (`GameTypes.ts:451-456`). `message` is the emoji string (CS `str`).
#[derive(crate::StateClone, Debug, PartialEq)]
pub struct EmojiMessage {
    pub message: String,
    /// int: a smallID.
    pub sender_id: crate::ids::SmallId,
    pub recipient_id: EmojiRecipient,
    pub created_at: Tick,
}

/// `AllianceInfo` (`GameTypes.ts:431-437`), returned by `Player.allianceInfo` (view/oracle).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AllianceInfo {
    pub expires_at: Tick,
    pub in_extension_window: bool,
    pub my_player_agreed_to_extend: bool,
    pub other_agreed_to_extend: bool,
    pub can_extend: bool,
}

/// `BuildableUnit` (`GameTypes.ts:353-365`), returned by `Player.buildableUnits` (oracle).
/// `canBuild: TileRef | false` → `Option<TileRef>`; `canUpgrade: number | false` → `Option<u32>`.
#[derive(Clone, PartialEq, Debug)]
pub struct BuildableUnit {
    pub can_build: Option<crate::ids::TileRef>,
    pub can_upgrade: Option<u32>,
    pub unit_type: UnitType,
    pub cost: Gold,
    pub upgrade_costs: Option<Vec<Gold>>,
    pub overlapping_railroads: Vec<crate::ids::TileRef>,
    /// Always empty in the port (ghost-rail preview is skipped, ARCHITECTURE §3.9).
    pub ghost_rail_paths: Vec<Vec<crate::ids::TileRef>>,
}

/// `PlayerProfile` (`GameTypes.ts:397-400`; view): `relations` keyed by smallID.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct PlayerProfile {
    pub relations: Vec<(crate::ids::SmallId, Relation)>,
    pub alliances: Vec<crate::ids::SmallId>,
}

/// `NukeType` (`StatsSchemas.ts:8-12` / `NationNukePlanning.ts:6`): one of the nuke unit types.
/// Kept as `UnitType` (callers pass `AtomBomb`/`HydrogenBomb`/`MirvWarhead`/`Mirv`).
pub type NukeType = UnitType;

/// The quick-chat / embargo / pause TS string-literal unions used by execution constructors.
pub use crate::api::intents::EmbargoAction;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unit_type_strings_and_order() {
        assert_eq!(UnitType::ALL.len(), UnitType::COUNT);
        assert_eq!(UnitType::TransportShip.as_str(), "Transport");
        assert_eq!(UnitType::from_ts("MIRV Warhead"), Some(UnitType::MirvWarhead));
        assert_eq!(UnitType::Factory.index(), 15);
        assert_eq!(serde_json::to_string(&UnitType::SamLauncher).expect("ser"), "\"SAM Launcher\"");
        assert_eq!(serde_json::from_str::<GameMode>("\"Free For All\"").expect("de"), GameMode::Ffa);
        for (i, t) in UnitType::ALL.iter().enumerate() {
            assert_eq!(t.index(), i);
        }
    }
    #[test]
    fn display_name() {
        let mut p = PlayerInfo::new("Bob", PlayerType::Human, Some("ABCDEFGH".into()), "ABCDEFGH");
        assert_eq!(p.display_name(), "Bob");
        p.clan_tag = Some(String::new());
        assert_eq!(p.display_name(), "Bob");
        p.clan_tag = Some("XY".into());
        assert_eq!(p.display_name(), "[XY] Bob");
    }
}
