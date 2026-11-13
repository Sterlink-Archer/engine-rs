//! `engine-lib/src/execution/NukeAlliance.ts`. Owner: **P8**.
//!
//! `GameLike` is the `State` (its `GameMap` half is `st.map`: `GameImpl.circleSearch` and
//! `GameImpl.ownerID` delegate to `_map`, `GameImpl.ts:1518-1519,1568-1573`).

use crate::api::types::unit_groups::STRUCTURES;
use crate::config::config::NukeMagnitude;
use crate::ids::TileRef;
use crate::jsorder::{JsMap, JsSet};
use crate::map::GameMap;
use crate::state::State;

/// `computeNukeBlastCounts({ gm, targetTile, magnitude })` (`:17-36`): smallID → weighted
/// tile count, in `circleSearch` visit order.
pub fn compute_nuke_blast_counts(gm: &GameMap, target_tile: TileRef, magnitude: NukeMagnitude) -> JsMap<u16, f64> {
    let inner2 = magnitude.inner * magnitude.inner; // TS: NukeAlliance.ts:22
    // TS: `new Map<number, number>()` iterated by `listNukeBreakAlliance` (`:114`) → JsMap.
    let mut counts: JsMap<u16, f64> = JsMap::new();
    // TS: :25-33. The returned Set is discarded; the filter always returns true.
    let _ = gm.circle_search(target_tile, magnitude.outer, |tile, d2| {
        let owner_small_id = gm.owner_id(tile);
        if owner_small_id > 0 {
            let weight = if d2 <= inner2 { 1.0 } else { 0.5 };
            let prev = counts.get(&owner_small_id).copied().unwrap_or(0.0); // TS: `?? 0`
            counts.insert(owner_small_id, prev + weight);
        }
        true
    });
    counts
}

/// `NukeAllianceCheckParams` (`:38-44`); `game: GameLike` is the `State`.
#[derive(Clone, Copy, Debug)]
pub struct NukeAllianceCheck<'a> {
    pub target_tile: TileRef,
    pub magnitude: NukeMagnitude,
    pub ally_small_ids: Option<&'a JsSet<u16>>,
    pub threshold: f64,
}

/// `wouldNukeBreakAlliance(params)` (`:50-97`).
pub fn would_nuke_break_alliance(st: &State, params: NukeAllianceCheck<'_>) -> bool {
    let NukeAllianceCheck { target_tile, magnitude, ally_small_ids, threshold } = params;

    // TS: `if (!allySmallIds || allySmallIds.size === 0) return false;` (:55-57)
    let ally_small_ids = match ally_small_ids {
        Some(s) if !s.is_empty() => s,
        _ => return false,
    };

    // TS: :60-66 (`playerId` and `includeUnderConstruction` omitted → undefined).
    let would_destroy_allied_structure = st.any_unit_nearby(
        target_tile,
        magnitude.outer,
        STRUCTURES,
        |st, u| {
            let owner = st.unit(u).owner();
            owner.is_player() && ally_small_ids.contains(&owner.0)
        },
        None,
        false,
    );
    if would_destroy_allied_structure {
        return true;
    }

    let inner2 = magnitude.inner * magnitude.inner;
    // TS: `new Map<number, number>()`, get/set only (:70) // lookup-only: never iterated
    // (grep `allyTileCounts` → :70,:81,:82 only). A JsMap keeps it trivially exact.
    let mut ally_tile_counts: JsMap<u16, f64> = JsMap::new();

    let mut result = false;

    // TS: :74-91. Returning `false` from the filter only leaves the tile out of the returned
    // set: `circleSearch` keeps visiting (GameMapImpl.ts:410-431), so the counts keep
    // accumulating after `result` is set. Ported literally (the comment "Stop searching" in
    // TS is not what the code does).
    let _ = st.map.circle_search(target_tile, magnitude.outer, |tile, d2| {
        let owner_small_id = st.map.owner_id(tile);
        if owner_small_id > 0 && ally_small_ids.contains(&owner_small_id) {
            let weight = if d2 <= inner2 { 1.0 } else { 0.5 };
            let new_count = ally_tile_counts.get(&owner_small_id).copied().unwrap_or(0.0) + weight;
            ally_tile_counts.insert(owner_small_id, new_count);

            if new_count > threshold {
                result = true;
                return false;
            }
        }
        true
    });

    result
}

/// `listNukeBreakAlliance(params)` (`:99-128`). `allySmallIds` is not read.
pub fn list_nuke_break_alliance(st: &State, params: NukeAllianceCheck<'_>) -> JsSet<u16> {
    let NukeAllianceCheck { target_tile, magnitude, threshold, .. } = params;

    // TS: `new Set<number>()`, iterated by NukeExecution.maybeBreakAlliances (:176) → JsSet.
    let mut players_to_break_alliance_with: JsSet<u16> = JsSet::new();

    // TS: :109-118
    let blast_counts = compute_nuke_blast_counts(&st.map, target_tile, magnitude);
    for (&player_small_id, &total_weight) in blast_counts.iter() {
        if total_weight > threshold {
            players_to_break_alliance_with.insert(player_small_id);
        }
    }

    // TS: :121-125 (no predicate; `includeUnderConstruction` undefined).
    for nu in st.nearby_units(target_tile, magnitude.outer, STRUCTURES, |_, _, _| true, false) {
        players_to_break_alliance_with.insert(st.unit(nu.unit).owner().0);
    }

    players_to_break_alliance_with
}
