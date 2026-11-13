//! Flat cluster scratch for `PlayerExecution.removeClusters` (owner: **P6**; declared at
//! S0 by P1, filled by P6).
//!
//! TS builds `clusters: TileRef[][]`, `clusterTileTIds: Int32Array[]`, the bounding boxes
//! and `isPinched` per call (`PlayerExecution.ts:142-151, 342-357, 1063-1090`). ARCHITECTURE
//! §11 item 3 turns them into one reused flat structure (`tiles`, `offsets`, `tids`,
//! `boxes`, `pinched`) with the result order unchanged.
//!
//! Cluster `i` is `tiles[offsets[i]..offsets[i + 1]]` (`offsets` has `len + 1` entries), its
//! box is `boxes[4i..4i + 4]` = `[minX, minY, maxX, maxY]`, and `tids[k]` is the territory
//! id of `tiles[k]` (`clusterTileTIds`, flattened in the same layout).
//!
//! `clusterIndexMap` cleanup (`PlayerExecution.ts:295-305`, a TS `finally`): it lives in the
//! traversal scratch and only `PlayerExecution` reads it. When a TS throw unwinds out of the
//! pinch pass, `index_map_dirty` stays set and the next `removeClusters` (any game on this
//! thread) zeroes the entries for the clusters still held here before it rebuilds them.
//! Deferred cleanup is unobservable because nothing reads the map in between.

use super::StepReset;
use crate::ids::TileRef;

/// `sc.clusters`.
#[derive(Debug, Default)]
pub struct ClusterScratch {
    /// Every cluster's tiles, concatenated in cluster order (each in flood mark order).
    pub tiles: Vec<TileRef>,
    /// Start of cluster `i` in `tiles`; `offsets[len]` = `tiles.len()`.
    pub offsets: Vec<usize>,
    /// `boxes: Int32Array` (`:1063`), 4 per cluster.
    pub boxes: Vec<i32>,
    /// `isPinched: Uint8Array` (`:149`).
    pub pinched: Vec<bool>,
    /// `clusterRemoved: Uint8Array` (`:184`).
    pub removed: Vec<bool>,
    /// `clusterSizes: Int32Array` (`:345`).
    pub sizes: Vec<i32>,
    /// `clusterTileTIds`, flattened like `tiles` (`:346-356`).
    pub tids: Vec<i32>,
    /// `territoryAreas = [0]` (`:347`), indexed by territory id.
    pub territory_areas: Vec<i32>,
    /// Reused result buffer of `floodFillWithGen` for `removeCluster` (`:812, :862`).
    pub fill: Vec<TileRef>,
    /// Reused `componentTiles` of `evaluatePinchCandidates` (`:475`).
    pub component: Vec<TileRef>,
    /// `clusterIndexMap` entries of the current clusters may be non-zero (see module doc).
    pub index_map_dirty: bool,
    /// t409 H5: territory id per component label in the label path of `floodTerritoryAreas`.
    pub tid_of_label: Vec<u32>,
    /// verify-memos (t409 H5): TS's eager `isPinched`, to check the lazy `pinched` against.
    #[cfg(feature = "verify-memos")]
    pub pinched_ts: Vec<bool>,
}

impl ClusterScratch {
    /// Number of clusters.
    #[inline]
    pub fn len(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Tile range of cluster `i`.
    #[inline]
    pub fn range(&self, i: usize) -> std::ops::Range<usize> {
        self.offsets[i]..self.offsets[i + 1] // in-range: i < len()
    }
    /// Reset for a new `calculateClusters` pass (keeps capacity).
    pub fn reset(&mut self) {
        self.tiles.clear();
        self.offsets.clear();
        self.offsets.push(0);
        self.boxes.clear();
        self.pinched.clear();
        self.removed.clear();
        self.sizes.clear();
        self.tids.clear();
        self.territory_areas.clear();
    }
}

impl StepReset for ClusterScratch {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_layout() {
        let mut c = ClusterScratch::default();
        c.reset();
        assert!(c.is_empty());
        c.tiles.extend([5, 6, 7]);
        c.offsets.push(2);
        c.offsets.push(3);
        assert_eq!(c.len(), 2);
        assert_eq!(&c.tiles[c.range(0)], &[5, 6]);
        assert_eq!(&c.tiles[c.range(1)], &[7]);
        c.reset();
        assert_eq!((c.len(), c.tiles.len()), (0, 0));
    }
}
