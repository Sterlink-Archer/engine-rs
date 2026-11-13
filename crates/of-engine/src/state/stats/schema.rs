//! Port of `engine-api/src/StatsSchemas.ts`: index constants and unit-key maps. Owner: **P2**.
//! The full `PlayerStats` record shape (`PlayerStatsSchema`, `:167-200`) is
//! `state::stats::PlayerStatsRec` (`stats-full`).

use crate::api::types::UnitType;

pub const ATTACK_INDEX_SENT: usize = 0;
pub const ATTACK_INDEX_RECV: usize = 1;
pub const ATTACK_INDEX_CANCEL: usize = 2;
pub const ATTACK_INDEX_MAX_RECV: usize = 3;

pub const TILE_INDEX_PEAK: usize = 0;
pub const TILE_INDEX_DRAWDOWN_PEAK: usize = 1;
pub const TILE_INDEX_DRAWDOWN_TROUGH: usize = 2;

pub const ALLIANCE_INDEX_FORMED: usize = 0;
pub const ALLIANCE_INDEX_BROKEN_BY_OTHER: usize = 1;
pub const ALLIANCE_INDEX_EXPIRED: usize = 2;
pub const ALLIANCE_INDEX_HELD_TO_END: usize = 3;
pub const ALLIANCE_INDEX_PEAK_CONCURRENT: usize = 4;
pub const ALLIANCE_INDEX_LONGEST_HELD: usize = 5;

pub const PLAYER_INDEX_HUMAN: usize = 0;
pub const PLAYER_INDEX_NATION: usize = 1;
pub const PLAYER_INDEX_BOT: usize = 2;

pub const BOAT_INDEX_SENT: usize = 0;
pub const BOAT_INDEX_ARRIVE: usize = 1;
pub const BOAT_INDEX_CAPTURE: usize = 2;
pub const BOAT_INDEX_DESTROY: usize = 3;
pub const BOAT_INDEX_LOST: usize = 4;

pub const BOMB_INDEX_LAUNCH: usize = 0;
pub const BOMB_INDEX_LAND: usize = 1;
pub const BOMB_INDEX_INTERCEPT: usize = 2;

pub const GOLD_INDEX_WORK: usize = 0;
pub const GOLD_INDEX_WAR: usize = 1;
pub const GOLD_INDEX_TRADE: usize = 2;
pub const GOLD_INDEX_STEAL: usize = 3;
pub const GOLD_INDEX_TRAIN_SELF: usize = 4;
pub const GOLD_INDEX_TRAIN_OTHER: usize = 5;
pub const GOLD_INDEX_DONATE_RECV: usize = 6;

pub const DONATION_INDEX_GOLD_RECV: usize = 0;
pub const DONATION_INDEX_GOLD_RECV_BROKE: usize = 1;
/// `DONATION_BROKE_GOLD_THRESHOLD = 50_000n`.
pub const DONATION_BROKE_GOLD_THRESHOLD: i64 = 50_000;

pub const OTHER_INDEX_BUILT: usize = 0;
pub const OTHER_INDEX_DESTROY: usize = 1;
pub const OTHER_INDEX_CAPTURE: usize = 2;
pub const OTHER_INDEX_LOST: usize = 3;
pub const OTHER_INDEX_UPGRADE: usize = 4;

/// `bombUnits` (`StatsSchemas.ts:5`).
pub const BOMB_UNITS: [&str; 4] = ["abomb", "hbomb", "mirv", "mirvw"];
/// `boatUnits` (`StatsSchemas.ts:21`).
pub const BOAT_UNITS: [&str; 2] = ["trade", "trans"];
/// `otherUnits` (`StatsSchemas.ts:31-39`).
pub const OTHER_UNITS: [&str; 7] = ["city", "defp", "port", "wshp", "silo", "saml", "fact"];

/// `unitTypeToBombUnit` (`StatsSchemas.ts:14-19`).
pub fn bomb_unit(t: UnitType) -> Option<&'static str> {
    match t {
        UnitType::AtomBomb => Some("abomb"),
        UnitType::HydrogenBomb => Some("hbomb"),
        UnitType::Mirv => Some("mirv"),
        UnitType::MirvWarhead => Some("mirvw"),
        _ => None,
    }
}

/// `unitTypeToBoatUnit` (`StatsSchemas.ts:26-29`).
pub fn boat_unit(t: UnitType) -> Option<&'static str> {
    match t {
        UnitType::TradeShip => Some("trade"),
        UnitType::TransportShip => Some("trans"),
        _ => None,
    }
}

/// `unitTypeToOtherUnit` (`StatsSchemas.ts:51-59`).
pub fn other_unit(t: UnitType) -> Option<&'static str> {
    match t {
        UnitType::City => Some("city"),
        UnitType::DefensePost => Some("defp"),
        UnitType::MissileSilo => Some("silo"),
        UnitType::Port => Some("port"),
        UnitType::SamLauncher => Some("saml"),
        UnitType::Warship => Some("wshp"),
        UnitType::Factory => Some("fact"),
        _ => None,
    }
}
