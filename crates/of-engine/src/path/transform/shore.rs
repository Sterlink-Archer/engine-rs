//! `ShoreCoercingTransformer` (`:1-113`). Owner: **P4b**.
//!
//! `waterToOriginal` is a `Map` used only for `get/set/delete` (`:22-37, :64`): kept as an
//! operation log here (one `get` at the end; never iterated).
#![allow(clippy::collapsible_if)] // nested ifs kept as the TS writes them

use crate::ids::TileRef;
use crate::map::GameMap;
use crate::path::MapRead;
use crate::path::types::{Src, delegate_src};

/// Coerce a land shore endpoint to an adjacent water tile (`None` if none):
/// `bestWaterNeighbor` (`:81-102`).
pub fn coerce_to_water(mini: &GameMap, t: TileRef) -> Option<TileRef> {
    best_water_neighbor(mini, t)
}

/// `bestWaterNeighbor(tile)` (`:81-102`): the water neighbour with the most water
/// neighbours, first wins on ties (N, S, W, E order); `-1` → `None`.
pub fn best_water_neighbor<M: MapRead>(map: &M, tile: TileRef) -> Option<TileRef> {
    let mut best: Option<TileRef> = None;
    let mut max_score: i32 = -1;
    let mut nbuf = [0; 4];
    let num_neighbors = map.neighbors4(tile, &mut nbuf);
    for &n in &nbuf[..num_neighbors] {
        if !map.is_water(n) {
            continue;
        }
        let score = count_water_neighbors(map, n);
        if score > max_score {
            max_score = score;
            best = Some(n);
        }
    }
    best
}

/// `countWaterNeighbors(tile)` (`:104-112`).
fn count_water_neighbors<M: MapRead>(map: &M, tile: TileRef) -> i32 {
    let mut count = 0;
    let mut nbuf = [0; 4];
    let num_neighbors = map.neighbors4(tile, &mut nbuf);
    for &n in &nbuf[..num_neighbors] {
        if map.is_water(n) {
            count += 1;
        }
    }
    count
}

/// `findPath(from, to)` (`:20-75`) of the transformer wrapping `inner`, on `map` (the minimap).
pub fn find_path<M: MapRead>(
    map: &M,
    from: Src<'_>,
    to: TileRef,
    inner: impl FnOnce(Src<'_>, TileRef) -> Option<Vec<TileRef>>,
) -> Option<Vec<TileRef>> {
    let mut one = [0];
    let from_array = from.as_slice(&mut one);
    // `waterToOriginal` is only written by set/delete (`:31`, `:36`) and read by ONE `get`
    // (`:64`), so it is kept as its operation log: the last op on a key decides that `get`
    // (`set` → the value, `delete` → undefined). O(1) per op instead of the former linear-list
    // map, which was quadratic in the shore count (P3 perf, t066).
    let mut water_to_original: Vec<(TileRef, Option<TileRef>)> = Vec::with_capacity(from_array.len());
    let mut water_from: Vec<TileRef> = Vec::with_capacity(from_array.len());

    for &f in from_array {
        if map.is_water(f) {
            water_from.push(f);
            // TS: :31 `waterToOriginal.delete(f)`
            water_to_original.push((f, None));
        } else if let Some(water) = best_water_neighbor(map, f) {
            water_from.push(water);
            // TS: :36 `waterToOriginal.set(water, f)`
            water_to_original.push((water, Some(f)));
        }
    }

    if water_from.is_empty() {
        return None;
    }

    // TS: :45-55
    let mut water_to = to;
    let mut original_to: Option<TileRef> = None;
    if !map.is_water(to) {
        water_to = best_water_neighbor(map, to)?;
        original_to = Some(to);
    }

    let mut path = inner(delegate_src(&water_from), water_to)?;
    if path.is_empty() {
        return None;
    }

    // TS: :64-67 restore original start shore tile
    if let Some(original_shore) = water_to_original.iter().rev().find(|(w, _)| *w == path[0]).and_then(|&(_, o)| o) {
        path.insert(0, original_shore);
    }
    // TS: :70-72
    if let Some(ot) = original_to {
        if path[path.len() - 1] != ot {
            path.push(ot);
        }
    }
    Some(path)
}
