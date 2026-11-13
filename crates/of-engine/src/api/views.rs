//! Port of `engine-api/src/game/ReadViews.ts`: the read-only views that rules code
//! (`Config`, `UnitGrid`, `NukeAlliance`) runs on. Owner: P1.
//!
//! The engine's `PlayerRef`/`UnitRef` views implement these traits (P2), and so do the
//! duck-typed stubs of the `config` vector suite, which is why config formulas can be
//! checked at L0 before any `State` exists (ARCHITECTURE §6).
//!
//! JS `number` results are `f64` here, including counts like `numTilesOwned`: the vector
//! stubs may pass any number, and the formulas consume them as `f64` anyway. `Gold`
//! (`bigint`) is `i64` (ARCHITECTURE §7.2).
//!
//! `GameLike` (`extends GameMap` + `owner`/`anyUnitNearby`/`nearbyUnits`) is not a trait: its
//! only engine user (`NukeAlliance`, P8) takes `&State`, whose `map`/`owner`/`any_unit_nearby`/
//! `nearby_units` are that surface (`state::game`). `PlayerRef`/`UnitView` implement the two
//! traits below (`state::player`, `state::unit`, P2).

use super::types::{PlayerType, UnitType};
use crate::ids::{SmallId, TileRef};

/// `interface PlayerLike` (`ReadViews.ts:20-30`).
pub trait PlayerLike {
    /// `id()`.
    fn id(&self) -> &str;
    /// `smallID()`.
    fn small_id(&self) -> SmallId;
    /// `isPlayer()`.
    fn is_player(&self) -> bool;
    /// `type()`.
    fn player_type(&self) -> PlayerType;
    /// `isLobbyCreator()`.
    fn is_lobby_creator(&self) -> bool;
    /// `numTilesOwned()`.
    fn num_tiles_owned(&self) -> f64;
    /// `troops()`.
    fn troops(&self) -> f64;
    /// `gold()` (`Gold` = bigint).
    fn gold(&self) -> i64;
    /// `units(type)`: a copy in the player's `units` order (`PlayerImpl.ts:486-497`).
    fn units(&self, t: UnitType) -> impl ExactSizeIterator<Item = impl UnitLike + '_> + '_;
}

/// `interface UnitLike` (`ReadViews.ts:32-40`).
pub trait UnitLike {
    /// `type()`.
    fn unit_type(&self) -> UnitType;
    /// `owner()`.
    fn owner(&self) -> impl PlayerLike + '_;
    /// `tile()`.
    fn tile(&self) -> TileRef;
    /// `lastTile()`.
    fn last_tile(&self) -> TileRef;
    /// `isActive()`.
    fn is_active(&self) -> bool;
    /// `isUnderConstruction()`.
    fn is_under_construction(&self) -> bool;
    /// `level()`.
    fn level(&self) -> f64;
}

#[cfg(test)]
mod tests {
    use super::*;

    // A duck-typed stub, the shape the `config` vector suite uses.
    struct P {
        cities: Vec<U>,
    }
    #[derive(Clone, Copy)]
    struct U {
        level: f64,
    }
    impl PlayerLike for P {
        fn id(&self) -> &str {
            "abcdefgh"
        }
        fn small_id(&self) -> SmallId {
            SmallId(1)
        }
        fn is_player(&self) -> bool {
            true
        }
        fn player_type(&self) -> PlayerType {
            PlayerType::Nation
        }
        fn is_lobby_creator(&self) -> bool {
            false
        }
        fn num_tiles_owned(&self) -> f64 {
            10.0
        }
        fn troops(&self) -> f64 {
            1.0
        }
        fn gold(&self) -> i64 {
            0
        }
        fn units(&self, t: UnitType) -> impl ExactSizeIterator<Item = impl UnitLike + '_> + '_ {
            let v: &[U] = if t == UnitType::City { &self.cities } else { &[] };
            v.iter().copied()
        }
    }
    impl UnitLike for U {
        fn unit_type(&self) -> UnitType {
            UnitType::City
        }
        fn owner(&self) -> impl PlayerLike + '_ {
            P { cities: vec![] }
        }
        fn tile(&self) -> TileRef {
            0
        }
        fn last_tile(&self) -> TileRef {
            0
        }
        fn is_active(&self) -> bool {
            true
        }
        fn is_under_construction(&self) -> bool {
            false
        }
        fn level(&self) -> f64 {
            self.level
        }
    }

    /// Shape of a `Config` formula over `PlayerLike` (sum of city levels, in units order).
    fn city_levels(p: &impl PlayerLike) -> f64 {
        p.units(UnitType::City).map(|u| u.level()).fold(0.0, |a, b| a + b)
    }

    #[test]
    fn stub_player_works_through_the_trait() {
        let p = P { cities: vec![U { level: 1.0 }, U { level: 2.5 }] };
        assert_eq!(city_levels(&p), 3.5);
        assert_eq!(p.units(UnitType::Port).len(), 0);
    }
}
