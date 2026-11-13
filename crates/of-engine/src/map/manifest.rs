//! `engine-api/src/game/MapManifest.ts` (serde for `manifest.json`). Owner: **P3**.
//!
//! Only the fields the engine reads are typed; the rest of the zod `strictObject` is kept
//! in `extra` so a round trip is lossless.

use crate::api::types::SpawnArea;
use serde::{Deserialize, Serialize};

/// `MapMetadataSchema` (`:98-102`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapMetadata {
    pub width: u32,
    pub height: u32,
    pub num_land_tiles: u32,
}

/// `NationSchema` (`:47-58`): `coordinates` is `[x, y]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ManifestNation {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coordinates: Option<[u32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flag: Option<String>,
}

/// `CustomTribeSchema` (`:90-96`): a name or `{ name, coordinates? }`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CustomTribe {
    Name(String),
    Placed {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        coordinates: Option<[u32; 2]>,
    },
}

/// `MapManifestSchema` (`:217-260`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MapManifest {
    pub map: MapMetadata,
    pub map4x: MapMetadata,
    pub map16x: MapMetadata,
    /// Required by the zod schema, but a manifest may leave it out (the plains test map
    /// does; `NationCreation.ts:47-48` reads it with `?.`): absent = empty.
    #[serde(default)]
    pub nations: Vec<ManifestNation>,
    #[serde(rename = "additionalNations", default, skip_serializing_if = "Option::is_none")]
    pub additional_nations: Option<Vec<ManifestNation>>,
    /// `record<"<teams>", SpawnArea[]>`; `Object.entries` order = integer keys ascending.
    #[serde(rename = "teamGameSpawnAreas", default, skip_serializing_if = "Option::is_none")]
    pub team_game_spawn_areas: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub themes: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_tribes: Option<Vec<CustomTribe>>,
    /// Every other manifest key (picker metadata, layers, …).
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl crate::plain::PlainState for MapManifest {}
crate::refs_leaf!(MapManifest);

/// Parsed team spawn areas: `(team count, areas)` in `Object.entries` order.
pub type TeamGameSpawnAreas = Vec<(u32, Vec<SpawnArea>)>;

impl MapManifest {
    /// `teamGameSpawnAreas` in `Object.entries` order (`TerrainMapLoader.ts:79`): the keys
    /// match `^[0-9]+$` (`MapManifest.ts:168`), i.e. integer-like, so ascending numeric order
    /// (ES `OrdinaryOwnPropertyKeys`; `of_det::ordmap::object_keys_order`). A key with a
    /// leading zero would be a string key in insertion order, which `serde_json::Map` does not
    /// keep: such a manifest is rejected (no shipped manifest has one).
    pub fn team_game_spawn_areas_entries(&self) -> Result<Option<TeamGameSpawnAreas>, String> {
        let Some(m) = &self.team_game_spawn_areas else { return Ok(None) };
        let keys: Vec<&String> = m.keys().collect();
        let mut out: TeamGameSpawnAreas = Vec::with_capacity(keys.len());
        for k in of_det::ordmap::object_keys_order(&keys) {
            let n = of_det::ordmap::is_array_index(&k)
                .ok_or_else(|| format!("teamGameSpawnAreas key {k:?} is not a canonical integer"))?;
            let areas: Vec<SpawnArea> =
                serde_json::from_value(m[&k].clone()).map_err(|e| format!("teamGameSpawnAreas[{k}]: {e}"))?;
            out.push((n, areas));
        }
        Ok(Some(out))
    }
}
