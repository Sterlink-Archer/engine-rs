//! `engine-api/src/game/MapFiles.ts` (FILE_MAP row 20). Owner: **P3**.

use crate::api::types::GameMapSize;
use crate::map::manifest::MapManifest;

/// `MapFiles` (`MapFiles.ts:9-16`): the files one map needs at one size, already loaded.
/// Normal games use `map.bin` + `map4x.bin`, compact ones `map4x.bin` + `map16x.bin`.
#[derive(Clone, Debug)]
pub struct MapFiles {
    /// `map: GameMapType` (the map name).
    pub map: String,
    pub map_size: GameMapSize,
    pub manifest: MapManifest,
    pub map_bin: Option<Vec<u8>>,
    pub map4x_bin: Option<Vec<u8>>,
    pub map16x_bin: Option<Vec<u8>>,
}
