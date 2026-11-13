//! Handles: how TS object references become `Copy` integers (ARCHITECTURE §3.2).
//!
//! Rules: every TS reference becomes a handle, every TS-visible identifier is kept as a
//! field, and **a handle is never assumed to equal the TS id** (arena slots are reused
//! after GC; CS keys and JS map keys always use the TS id, PORTING_GUIDE §5.1).
//!
//! | TS object | Rust handle | TS identity |
//! |---|---|---|
//! | Player / TerraNullius | [`SmallId`] (`Owner`), 0 = TerraNullius | `smallID` (`GameImpl.ts:906-918`) |
//! | Unit | [`UnitRef`] (arena slot+generation); [`UnitKey`] = TS `unit.id()` for map keys | `nextUnitID()` (`GameImpl.ts:289-293`) |
//! | Attack | [`AttackRef`]; [`AttackId`] = the 8-char base-36 id as a number | `nextID()` (`PlayerImpl.ts:1879-1898`) |
//! | Alliance | [`AllianceId`] == TS id | `nextAllianceID++` (`GameImpl.ts:483-490`) |
//! | AllianceRequest | [`ReqIdx`] = creation index | object identity |
//! | TrainStation | [`StationId`] == TS id | `StationManager.nextId++` (`RailNetworkImpl.ts:42-48`) |
//! | Railroad | [`RailroadRef`] (arena) + [`RailroadId`] == TS id | `RailNetworkImpl.nextId` (`:303`) |
//! | Cluster | [`ClusterRef`] (arena) | object identity (`new Cluster()`, `:348`) |
//! | Execution | [`ExecId`] `{kind, idx}` into per-kind arenas (§4.2) | object identity |
//! | PlayerInfo | [`PlayerInfoIdx`] into `State.infos` | object identity |
//!
//! Owner: **P1** (FILE_MAP §2 foundations).

use crate::plain_copy;

/// `TileRef`: a grid index `y * width + x`. // int: map coordinates are capped at 65535, so
/// the largest ref is 65535*65535-1 < 2^32-1 (`engine-lib/src/game/TileSet.ts:4-7`).
pub type TileRef = u32;

/// Player small id (`smallID()`); 0 is TerraNullius (`TerraNulliusImpl.ts:5-7`).
/// // int: `nextPlayerID++` from 1, at most a few thousand players.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Debug)]
pub struct SmallId(pub u16);

/// `Player | TerraNullius` (PORTING_GUIDE §10): `x.isPlayer()` is `x != TERRA_NULLIUS`.
pub type Owner = SmallId;
/// Key type for maps keyed by a player (`Map<Player, ..>`, `Map<PlayerID, ..>`; id ↔ smallID is a bijection).
pub type SmallIdKey = SmallId;

impl SmallId {
    pub const TERRA_NULLIUS: SmallId = SmallId(0);
    /// `isPlayer()` (`TerraNulliusImpl.ts:17-19`: TerraNullius returns false).
    #[inline]
    pub fn is_player(self) -> bool {
        self.0 != 0
    }
    /// Index into `State.players` (`_playersBySmallID[smallID - 1]`). Panics for TerraNullius.
    #[inline]
    pub fn player_index(self) -> usize {
        assert!(self.0 != 0, "TerraNullius has no player slot");
        self.0 as usize - 1
    }
    /// The handle of `State.players[i]`.
    #[inline]
    pub fn from_player_index(i: usize) -> SmallId {
        SmallId(u16::try_from(i + 1).expect("smallID fits u16"))
    }
}

/// Generational arena handle behaviour (see [`crate::arena::Arena`]).
pub trait ArenaHandle: Copy + Eq + std::fmt::Debug {
    fn from_parts(slot: u32, generation: u32) -> Self;
    fn slot(self) -> u32;
    fn generation(self) -> u32;
}

macro_rules! arena_handle {
    ($($(#[$m:meta])* $name:ident),* $(,)?) => {$(
        $(#[$m])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        pub struct $name {
            pub slot: u32,
            pub generation: u32,
        }
        impl Default for $name {
            /// The null handle (slot `u32::MAX`): any access is a stale-handle panic. Used only
            /// as a placeholder in `Default` state (pooled fork targets, field defaults).
            fn default() -> Self {
                $name { slot: u32::MAX, generation: 0 }
            }
        }
        impl ArenaHandle for $name {
            #[inline]
            fn from_parts(slot: u32, generation: u32) -> Self {
                $name { slot, generation }
            }
            #[inline]
            fn slot(self) -> u32 {
                self.slot
            }
            #[inline]
            fn generation(self) -> u32 {
                self.generation
            }
        }
        // A handle: `Refs` reports it (canon/refs.rs), so not `plain_copy!`.
        impl crate::plain::PlainState for $name {}
    )*};
}

arena_handle! {
    /// A unit (`Arena<Unit>`). The TS id is `Unit.id`; use [`UnitKey`] for map keys.
    UnitRef,
    /// An attack (`Arena<Attack>`); the TS id is `Attack.id` ([`AttackId`]).
    AttackRef,
    /// A rail cluster (`Arena<Cluster>`); no TS id (CS key `c<min station id>`/`e<n>`).
    ClusterRef,
    /// A railroad slot (`Arena<Railroad>`); the TS id is [`RailroadId`].
    RailroadRef,
}

macro_rules! id_newtype {
    ($($(#[$m:meta])* $name:ident($t:ty)),* $(,)?) => {$(
        $(#[$m])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Debug)]
        pub struct $name(pub $t);
        impl crate::plain::PlainState for $name {}
    )*};
}

id_newtype! {
    /// TS `unit.id()` as a map key (`Map<Unit, ..>`, `Set<Unit>`): identity survives GC slot reuse.
    /// // int: `nextUnitID()` is a monotonic counter (`GameImpl.ts:289-293`).
    UnitKey(u32),
    /// `Alliance.id()` (`nextAllianceID++`, `GameImpl.ts:483-490`); also the index into `State.alliances`.
    AllianceId(u32),
    /// AllianceRequest creation index (append-only `State.alliance_reqs`; ARCHITECTURE §3.2).
    ReqIdx(u32),
    /// `TrainStation.id` (from 1, `RailNetworkImpl.ts:42-48`); also its slot in `by_id`.
    StationId(u32),
    /// `Railroad.id` (`RailNetworkImpl.ts:303`).
    RailroadId(u32),
    /// Index into `State.infos` (no `Arc` per player, review 1 #7).
    PlayerInfoIdx(u32),
    /// Index into `State.exec_inbox`, returned by `add_execution` (only MIRV uses it, §4.3).
    PendingExec(u32),
}

/// An attack id: `nextID()` = `floor(next() * 36^8).toString(36).padStart(8, "0")`
/// (`PseudoRandom.ts:73-78`). Kept as the number (`< 36^8`), rendered only at the edges;
/// equality on the number equals equality on the padded string (PORTING_GUIDE §9).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Debug)]
pub struct AttackId(pub u64);
plain_copy!(AttackId);

impl AttackId {
    /// `36^8`.
    pub const LIMIT: u64 = 2_821_109_907_456;
    /// The TS string (8 base-36 chars).
    pub fn to_ts_string(self) -> String {
        of_det::js::pad_start_zero(&of_det::js::to_string_radix(self.0 as f64, 36), 8)
    }
    /// Parse a TS attack id string (`cancel_attack.attackID`). `None` if it is not one the
    /// engine could have produced (TS then simply finds no attack with that id).
    pub fn parse(s: &str) -> Option<AttackId> {
        if s.len() != 8 || !s.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase()) {
            return None;
        }
        u64::from_str_radix(s, 36).ok().filter(|&v| v < Self::LIMIT).map(AttackId)
    }
}

macro_rules! exec_kinds {
    ($($v:ident = $name:literal),* $(,)?) => {
        /// The 44 execution types, in `EXECUTION_SNAPSHOT_TYPES` order
        /// (`engine/src/snapshot/ExecutionRegistry.ts:55-100`).
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
        #[repr(u8)]
        pub enum ExecKind {
            $($v,)*
            /// Unit-test-only scripted execution (`exec/probe.rs`); not one of the 44.
            #[cfg(test)]
            Probe,
        }

        impl ExecKind {
            pub const ALL: [ExecKind; 44] = [$(ExecKind::$v),*];
            pub const COUNT: usize = 44;
            /// The registered snapshot type name (`name:` of each `*ExecutionSnapshot`), the `t` of CS E rows.
            pub fn as_str(self) -> &'static str {
                match self {
                    $(ExecKind::$v => $name,)*
                    #[cfg(test)]
                    ExecKind::Probe => "Probe",
                }
            }
            pub fn from_name(s: &str) -> Option<ExecKind> {
                match s { $($name => Some(ExecKind::$v),)* _ => None }
            }
        }
    };
}

exec_kinds! {
    AllianceExtension = "AllianceExtension",
    AllianceReject = "AllianceReject",
    AllianceRequest = "AllianceRequest",
    Attack = "Attack",
    BoatRetreat = "BoatRetreat",
    BreakAlliance = "BreakAlliance",
    City = "City",
    Construction = "Construction",
    DefensePost = "DefensePost",
    DeleteUnit = "DeleteUnit",
    DonateGold = "DonateGold",
    DonateTroops = "DonateTroops",
    DoomsdayClock = "DoomsdayClock",
    EmbargoAll = "EmbargoAll",
    Embargo = "Embargo",
    Emoji = "Emoji",
    Factory = "Factory",
    MarkDisconnected = "MarkDisconnected",
    Mirv = "MIRV",
    MissileSilo = "MissileSilo",
    MoveWarship = "MoveWarship",
    Nation = "Nation",
    NoOp = "NoOp",
    Nuke = "Nuke",
    Pause = "Pause",
    Player = "Player",
    Port = "Port",
    QuickChat = "QuickChat",
    RecomputeRailCluster = "RecomputeRailCluster",
    Retreat = "Retreat",
    SamLauncher = "SAMLauncher",
    SamMissile = "SAMMissile",
    Shell = "Shell",
    Spawn = "Spawn",
    SpawnTimer = "SpawnTimer",
    TargetPlayer = "TargetPlayer",
    TradeShip = "TradeShip",
    Train = "Train",
    TrainStation = "TrainStation",
    TransportShip = "TransportShip",
    Tribe = "Tribe",
    UpgradeStructure = "UpgradeStructure",
    Warship = "Warship",
    WinCheck = "WinCheck",
}

/// An execution: `(kind, slot in that kind's arena)` (ARCHITECTURE §4.2). 8 bytes, `Copy`.
/// Order and CS keys come from `live`/`uninit` positions, never from `idx`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ExecId {
    pub kind: ExecKind,
    pub idx: u32,
}

plain_copy!(ExecKind);
// Handles: `Refs` reports them (canon/refs.rs).
impl crate::plain::PlainState for SmallId {}
impl crate::plain::PlainState for ExecId {}
// Plain ids (no object behind them that a walker follows).
crate::refs_leaf!(UnitKey, PlayerInfoIdx, PendingExec);

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exec_kind_table() {
        assert_eq!(ExecKind::ALL.len(), ExecKind::COUNT);
        for (i, k) in ExecKind::ALL.iter().enumerate() {
            assert_eq!(*k as usize, i);
            assert_eq!(ExecKind::from_name(k.as_str()), Some(*k));
        }
        assert_eq!(ExecKind::Mirv.as_str(), "MIRV");
        assert_eq!(std::mem::size_of::<ExecId>(), 8);
    }
    #[test]
    fn attack_id_round_trip() {
        // PseudoRandom(…).nextID() values are 8 chars, zero-padded (vectors jssem/toString36).
        let a = AttackId(1_369_859_069_664);
        assert_eq!(a.to_ts_string(), "hhaypyw0");
        assert_eq!(AttackId::parse("hhaypyw0"), Some(a));
        assert_eq!(AttackId(35).to_ts_string(), "0000000z");
        assert_eq!(AttackId::parse("0000000z"), Some(AttackId(35)));
        assert_eq!(AttackId::parse("HHAYPYW0"), None);
        assert_eq!(AttackId::parse("hhaypyw"), None);
    }
    #[test]
    fn small_id() {
        assert!(!SmallId::TERRA_NULLIUS.is_player());
        assert_eq!(SmallId(3).player_index(), 2);
        assert_eq!(SmallId::from_player_index(2), SmallId(3));
    }
}
