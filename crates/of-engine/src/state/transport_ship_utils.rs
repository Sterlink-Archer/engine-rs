//! `engine/src/game/TransportShipUtils.ts`. Owner: **P7**.
//!
//! Read-only queries over the state with scratch (SpatialQuery / water A*). TS builds a fresh
//! `new SpatialQuery(game)` per call; the port's `SpatialQuery` is a set of free functions
//! over `&mut Rcx` whose only memo (`reachableComponents`) lives on `Player` (P4b), so a fresh
//! instance per call is the same thing.

use crate::api::types::UnitType;
use crate::ctx::Rcx;
use crate::ids::{SmallId, TileRef};
use crate::path::spatial_query;

/// `SpatialQuery.closestReachableShore` / `closestShore` default `maxDist = 50`
/// (`SpatialQuery.ts:103,129`).
const DEFAULT_SHORE_MAX_DIST: u32 = 50;

/// `canBuildTransportShip(game, player, tile): TileRef | false` (`:6-32`); `false` → `None`.
pub fn can_build_transport_ship(rcx: &mut Rcx, player: SmallId, tile: TileRef) -> Option<TileRef> {
    // TS: :11-15 `player.unitCount(TransportShip) >= game.config().boatMaxNumber()`
    if f64::from(rcx.st.player(player).unit_count(UnitType::TransportShip)) >= rcx.st.config().boat_max_number() {
        return None;
    }

    // TS: :17-20
    let dst = target_transport_tile(rcx, player, tile)?;

    // TS: :22-28 (`other === player`; `other.isPlayer() && !player.canAttackPlayer(other)`,
    // `treatAFKFriendly` defaults to false)
    let other = rcx.st.owner(tile);
    if other == player {
        return None;
    }
    if other.is_player() && !rcx.st.player(player).can_attack_player(other, false) {
        return None;
    }

    // TS: :30-31 `spatial.closestShoreByWater(player, dst) ?? false`
    spatial_query::closest_shore_by_water(rcx, player, dst)
}

/// [`can_build_transport_ship`] on `&mut Ctx`: the same checks, with the final
/// `closestShoreByWater` answered through the exact `State::shore_memo` (P3 perf, t066).
pub fn can_build_transport_ship_cx(cx: &mut crate::ctx::Ctx, player: SmallId, tile: TileRef) -> Option<TileRef> {
    if f64::from(cx.st.player(player).unit_count(UnitType::TransportShip)) >= cx.st.config().boat_max_number() {
        return None;
    }
    let dst = target_transport_tile(&mut cx.rcx(), player, tile)?;
    let other = cx.st.owner(tile);
    if other == player {
        return None;
    }
    if other.is_player() && !cx.st.player(player).can_attack_player(other, false) {
        return None;
    }
    spatial_query::closest_shore_by_water_memo(cx, player, dst)
}

/// `targetTransportTile(gm, attacker, tile): TileRef | null` (`:34-43`).
pub fn target_transport_tile(rcx: &mut Rcx, attacker: SmallId, tile: TileRef) -> Option<TileRef> {
    // TS: :42 `spatial.closestReachableShore(gm.owner(tile), attacker, tile)` (maxDist = 50)
    let target_owner = rcx.st.owner(tile);
    spatial_query::closest_reachable_shore(rcx, target_owner, attacker, tile, DEFAULT_SHORE_MAX_DIST)
}

/// `bestShoreDeploymentSource(gm, player, dst): TileRef | null` (`:45-52`).
pub fn best_shore_deployment_source(rcx: &mut Rcx, player: SmallId, dst: TileRef) -> Option<TileRef> {
    // TS: :51
    spatial_query::closest_shore_by_water(rcx, player, dst)
}
