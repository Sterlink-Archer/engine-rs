//! `RailSpatialGrid` (`game/RailroadSpatialGrid.ts`). Owner: **P5**.
//!
//! The TS cell key is the string `${cx}:${cy}` (`:113`, `key()`); it is a bijection with
//! `(cx, cy)`, so it is packed into a `u32` (`cx << 16 | cy`, both < 2^16 because map sides
//! are < 2^16 and `cellSize` ≥ 1). Both maps are iterated by the snapshot (`:86-98`: "Cell
//! and rail entry order both decide query result order"), so they stay JS-ordered and the
//! CS exporter renders the TS string ([`RailSpatialGrid::cell_key_string`]).
//!
//! Railroads are keyed by their TS id (`Map<Railroad, ..>` / `Set<Railroad>` key by object
//! identity; railroad ids are unique, `RailNetworkImpl.nextId++`).

use crate::StateClone;
use crate::ctx::ts_throw;
use crate::ids::{RailroadId, TileRef};
use crate::jsorder::{JsMap, SmallJsSet};
use crate::map::GameMap;

/// Pack `(cx, cy)` (both < 2^16) into the cell key.
#[inline]
pub fn cell_key(cx: u32, cy: u32) -> u32 {
    (cx << 16) | cy
}

/// `RailSpatialGrid` data, snapshot `railGrid { cells, railToCells }` order.
#[derive(StateClone, Debug, Default)]
pub struct RailSpatialGrid {
    /// `cells: Map<string, Set<Railroad>>` (`:5`).
    pub cells: JsMap<u32, SmallJsSet<RailroadId, 4>>,
    /// `railToCells: Map<Railroad, Set<string>>` (`:7`).
    pub rail_to_cells: JsMap<RailroadId, SmallJsSet<u32, 4>>,
    /// non-snapshot: `cellSize` (constructor, `:9-16`).
    pub cell_size: u32,
}

impl RailSpatialGrid {
    /// `new RailSpatialGrid(game, cellSize)` (`:9-16`): throws for `cellSize <= 0`.
    pub fn new(cell_size: u32) -> Self {
        if cell_size == 0 {
            ts_throw!("cellSize must be > 0");
        }
        RailSpatialGrid { cells: JsMap::default(), rail_to_cells: JsMap::default(), cell_size }
    }

    /// `register(rail)` (`:18-41`).
    pub fn register(&mut self, map: &GameMap, rail: RailroadId, tiles: &[TileRef]) {
        self.register_with(rail, tiles, |t| (map.x(t), map.y(t)));
    }

    /// `register(rail)` with `game.x/y` supplied by the caller.
    pub(crate) fn register_with(&mut self, rail: RailroadId, tiles: &[TileRef], xy: impl Fn(TileRef) -> (u32, u32)) {
        let coords: Vec<(u32, u32)> = tiles.iter().map(|&t| xy(t)).collect();
        self.register_coords(rail, &coords);
    }

    /// `register(rail)` (`:18-41`) on the `(game.x(tile), game.y(tile))` of `rail.tiles`, in order.
    pub(crate) fn register_coords(&mut self, rail: RailroadId, coords: &[(u32, u32)]) {
        // Defensive: avoid double-registration but it should never happen (TS: :19-20)
        self.unregister(rail);

        let mut rail_cells: SmallJsSet<u32, 4> = SmallJsSet::default();

        for &(x, y) in coords {
            let (cx, cy) = self.cell_of_tile(x, y);
            let k = cell_key(cx, cy);
            if rail_cells.contains(&k) {
                continue;
            }
            // TS: `let set = cells.get(k); if (!set) { set = new Set(); cells.set(k, set) }` (:29-33)
            if !self.cells.contains_key(&k) {
                self.cells.insert(k, SmallJsSet::default());
            }
            rail_cells.insert(k);
            if let Some(set) = self.cells.get_mut(&k) {
                set.insert(rail);
            }
        }

        if !rail_cells.is_empty() {
            self.rail_to_cells.insert(rail, rail_cells);
        }
    }

    /// `unregister(rail)` (`:43-58`).
    pub fn unregister(&mut self, rail: RailroadId) {
        let Some(keys) = self.rail_to_cells.get(&rail) else { return };
        // The body mutates only `cells`, never this key set (it is deleted after the loop), so
        // a copy visits exactly the keys the TS `for..of` visits.
        let keys: Vec<u32> = keys.iter().copied().collect();

        for k in keys {
            let Some(set) = self.cells.get_mut(&k) else { continue };
            set.remove(&rail);

            if set.is_empty() {
                self.cells.remove(&k);
            }
        }

        self.rail_to_cells.remove(&rail);
    }

    /// `query(tile, radius)` (`:60-86`): the rails of every cell overlapping the square, in
    /// `Set` insertion order (cells in `cx`-major, `cy`-minor order, rails in cell-set order).
    pub fn query(&self, map: &GameMap, tile: TileRef, radius: u32) -> Vec<RailroadId> {
        self.query_xy(map.x(tile), map.y(tile), radius)
    }

    /// `query(tile, radius)` on the tile's coordinates.
    pub(crate) fn query_xy(&self, x: u32, y: u32, radius: u32) -> Vec<RailroadId> {
        // int: tile coordinates and the radius are small non-negative ints; `x - radius` can be
        // negative, so the cell arithmetic is signed (TS `Math.floor(x / cellSize)`, :120-122).
        let (x, y, r) = (i64::from(x), i64::from(y), i64::from(radius));
        let min_x = x - r;
        let min_y = y - r;
        let max_x = x + r;
        let max_y = y + r;

        let (c0x, c0y) = self.cell_of(min_x, min_y);
        let (c1x, c1y) = self.cell_of(max_x, max_y);

        // `new Set<Railroad>()`, filled in visit order: a deduplicated insertion-ordered list.
        let mut result: SmallJsSet<RailroadId, 8> = SmallJsSet::default();

        let mut cx = c0x;
        while cx <= c1x {
            let mut cy = c0y;
            while cy <= c1y {
                // A negative cell has no key: TS builds the string "-1:…", which no tile ever
                // registered (tile coordinates are ≥ 0), so `cells.get` misses.
                if let (Ok(ux), Ok(uy)) = (u32::try_from(cx), u32::try_from(cy))
                    && ux <= 0xffff
                    && uy <= 0xffff
                    && let Some(set) = self.cells.get(&cell_key(ux, uy))
                {
                    for &rail in set.iter() {
                        result.insert(rail);
                    }
                }
                cy += 1;
            }
            cx += 1;
        }

        result.iter().copied().collect()
    }

    /// The TS string key of a packed cell key (`key(cx, cy)` = `` `${cx}:${cy}` ``, `:112-114`).
    pub fn cell_key_string(key: u32) -> String {
        format!("{}:{}", key >> 16, key & 0xffff)
    }

    /// Parse a TS cell key string back to the packed key (CS loader). `None` for a string the
    /// engine cannot produce.
    pub fn parse_cell_key(s: &str) -> Option<u32> {
        let (a, b) = s.split_once(':')?;
        let cx: u32 = a.parse().ok()?;
        let cy: u32 = b.parse().ok()?;
        // Round-trip check: rejects "01:2", "+1:2" and the like (TS never formats those).
        if cx > 0xffff || cy > 0xffff || format!("{cx}:{cy}") != s {
            return None;
        }
        Some(cell_key(cx, cy))
    }

    /// `cellOf(x, y)` (`:116-122`) for tile coordinates (non-negative: integer division is
    /// `Math.floor`).
    #[inline]
    fn cell_of_tile(&self, x: u32, y: u32) -> (u32, u32) {
        (x / self.cell_size, y / self.cell_size)
    }

    /// `cellOf(x, y)` (`:116-122`) for possibly negative coordinates: `Math.floor` division.
    #[inline]
    fn cell_of(&self, x: i64, y: i64) -> (i64, i64) {
        let c = i64::from(self.cell_size);
        (x.div_euclid(c), y.div_euclid(c))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Width-100 map coordinates.
    fn xy(t: TileRef) -> (u32, u32) {
        (t % 100, t / 100)
    }

    #[test]
    fn cell_key_round_trips_the_ts_string() {
        // FILE_MAP §4 P5 mandatory design test: rail-grid CS key rendering round-trips the TS
        // string (`${cx}:${cy}`, RailroadSpatialGrid.ts:112-114) and the packed order is the
        // JsMap insertion order, not a numeric order.
        for (cx, cy) in [(0, 0), (1, 0), (0, 1), (12, 345), (16383, 16383), (65535, 65535)] {
            let k = cell_key(cx, cy);
            let s = RailSpatialGrid::cell_key_string(k);
            assert_eq!(s, format!("{cx}:{cy}"));
            assert_eq!(RailSpatialGrid::parse_cell_key(&s), Some(k));
        }
        for bad in ["1,2", "01:2", "1:", ":1", "-1:0", "65536:0", "1:2:3", "+1:2"] {
            assert_eq!(RailSpatialGrid::parse_cell_key(bad), None, "{bad}");
        }
        // Insertion order is kept: cell (5,0) registered before cell (0,0).
        let mut g = RailSpatialGrid::new(4);
        g.register_with(RailroadId(7), &[20, 0], xy);
        let keys: Vec<String> = g.cells.keys().map(|&k| RailSpatialGrid::cell_key_string(k)).collect();
        assert_eq!(keys, ["5:0", "0:0"]);
        let r2c: Vec<(u32, Vec<String>)> = g
            .rail_to_cells
            .iter()
            .map(|(r, ks)| (r.0, ks.iter().map(|&k| RailSpatialGrid::cell_key_string(k)).collect()))
            .collect();
        assert_eq!(r2c, [(7, vec!["5:0".to_string(), "0:0".to_string()])]);
    }

    #[test]
    #[should_panic]
    fn zero_cell_size_throws() {
        let _ = RailSpatialGrid::new(0);
    }

    #[test]
    fn register_dedups_cells_and_reregister_moves_to_end() {
        let mut g = RailSpatialGrid::new(4);
        // Tiles 0,1,2,3 share cell 0:0; 4 is cell 1:0.
        g.register_with(RailroadId(1), &[0, 1, 2, 3, 4], xy);
        g.register_with(RailroadId(2), &[4, 5], xy);
        assert_eq!(g.cells.len(), 2);
        assert_eq!(
            g.cells.get(&cell_key(1, 0)).map(|s| s.iter().copied().collect::<Vec<_>>()),
            Some(vec![RailroadId(1), RailroadId(2)])
        );
        // Re-registering rail 1 unregisters it first: it moves behind rail 2 in cell 1:0, and its
        // railToCells entry moves to the end.
        g.register_with(RailroadId(1), &[4], xy);
        assert_eq!(
            g.cells.get(&cell_key(1, 0)).map(|s| s.iter().copied().collect::<Vec<_>>()),
            Some(vec![RailroadId(2), RailroadId(1)])
        );
        assert!(!g.cells.contains_key(&cell_key(0, 0)), "cell 0:0 emptied and deleted");
        assert_eq!(g.rail_to_cells.keys().copied().collect::<Vec<_>>(), [RailroadId(2), RailroadId(1)]);
        // An empty rail registers nothing (TS: railCells.size === 0 → no railToCells entry).
        g.register_with(RailroadId(3), &[], xy);
        assert!(!g.rail_to_cells.contains_key(&RailroadId(3)));
        g.unregister(RailroadId(2));
        g.unregister(RailroadId(1));
        g.unregister(RailroadId(9)); // unknown: no-op
        assert!(g.cells.is_empty() && g.rail_to_cells.is_empty());
    }

    #[test]
    fn query_square_dedup_order_and_negative_cells() {
        let mut g = RailSpatialGrid::new(4);
        // rail 1 in cells 0:0 and 1:0, rail 2 in cell 0:1 (tile 400 = (0,4)), rail 3 far away.
        g.register_with(RailroadId(1), &[1, 5], xy);
        g.register_with(RailroadId(2), &[400], xy);
        g.register_with(RailroadId(3), &[9999], xy);
        // From (1,1) radius 3: x in [-2, 4] → cells -1..=1, y in [-2, 4] → cells -1..=1.
        // Visit order: cx=0: cy=0 (rail 1), cy=1 (rail 2); cx=1: cy=0 (rail 1 again).
        assert_eq!(g.query_xy(1, 1, 3), [RailroadId(1), RailroadId(2)]);
        assert_eq!(g.query_xy(5, 0, 0), [RailroadId(1)]);
        assert!(g.query_xy(50, 50, 3).is_empty());
        assert_eq!(g.query_xy(99, 99, 0), [RailroadId(3)]);
    }
}
