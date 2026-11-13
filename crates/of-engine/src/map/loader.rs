//! `engine-lib/src/game/TerrainMapLoader.ts` + `MapStatics` (ARCHITECTURE §3.1, §8.1).
//! Owner: **P3** (graph0/components0 come from P4a's `WaterGraph::build`).
//!
//! `MapStatics` is the immutable per-map data shared by every game and fork on that map
//! (one `Arc` per game): pristine terrain of both maps, the land index used by
//! `PlayerTileOrder` (§3.6.2), the manifest nations, team spawn areas, and the version-0
//! water graph + components (§3.8).

use crate::StateClone;
use crate::api::map_files::MapFiles;
use crate::api::types::{Cell, GameMapSize, Nation, PlayerInfo, PlayerType, SpawnArea};
use crate::ids::TileRef;
use crate::map::game_map::GameMap;
use crate::map::manifest::{ManifestNation, MapManifest, MapMetadata, TeamGameSpawnAreas};
use crate::tileset::NO_POS;
use crate::water::abstract_graph::WaterGraph;
use crate::water::components::Components;
use std::sync::Arc;

/// Immutable per-map data.
#[derive(StateClone, Debug, Default)]
pub struct MapStatics {
    /// Main map dimensions and pristine terrain.
    pub width: u32,
    pub height: u32,
    pub terrain: Arc<[u8]>,
    pub num_land_tiles: u32,
    /// Minimap dimensions and pristine terrain.
    pub mini_width: u32,
    pub mini_height: u32,
    pub mini_terrain: Arc<[u8]>,
    pub mini_num_land_tiles: u32,
    /// tile → land index, or `tileset::NO_POS` (shared; never cloned per fork).
    pub land_index: Arc<[u32]>,
    /// `TerrainMapData.nations` / `additionalNations` (`TerrainMapLoader.ts:15-24`): the
    /// manifest's `NationSchema` entries (`{name, coordinates?, flag?}`), carried in the S0
    /// type as **templates** ([`nation_template`]): `spawn_cell` = `coordinates`,
    /// `player_info.name` = `name`, `player_info.nation_flag` = `flag`; the template's
    /// `id`/`client_id`/… are placeholders that `create_nations_for_game` never reads (it
    /// builds a fresh `PlayerInfo` with `random.nextID()`, `NationCreation.ts:29-45`).
    pub nations: Arc<[Nation]>,
    pub additional_nations: Arc<[Nation]>,
    /// `_teamGameSpawnAreas`.
    pub team_game_spawn_areas: Option<Arc<TeamGameSpawnAreas>>,
    /// The manifest as loaded (compact maps scaled in place, `:30-119`).
    pub manifest: Option<Arc<MapManifest>>,
    /// Version-0 water graph (`None` when the nav mesh is disabled), built once per process.
    pub graph0: Option<Arc<WaterGraph>>,
    /// Version-0 minimap water components.
    pub components0: Option<Arc<Components>>,
}

/// A manifest nation as a `Nation` template (see [`MapStatics::nations`]).
pub fn nation_template(n: &ManifestNation) -> Nation {
    let mut info = PlayerInfo::new(n.name.clone(), PlayerType::Nation, None, String::new());
    info.nation_flag = n.flag.clone();
    Nation { spawn_cell: n.coordinates.map(|[x, y]| Cell::new(f64::from(x), f64::from(y))), player_info: info }
}

/// `{ coordinates[0] / 2, coordinates[1] / 2 }` floored (`TerrainMapLoader.ts:56-73`).
fn halve_coords(n: &mut ManifestNation) {
    if let Some([x, y]) = n.coordinates {
        // TS: `Math.floor(c / 2)` on non-negative ints = integer halving
        n.coordinates = Some([x / 2, y / 2]);
    }
}

impl MapStatics {
    /// `loadTerrainMap(files)` (`TerrainMapLoader.ts:30-119`) + graph0/components0 (P4a's
    /// `ConnectedComponents.initialize` + `AbstractGraphBuilder.build`, `WaterManager.ts:74-92`).
    /// They are always built: `disableNavMesh` is a game setting, applied by `create_state`.
    pub fn load(files: &MapFiles) -> Result<Arc<MapStatics>, String> {
        let mut st = Self::load_terrain(files)?;
        let mini = GameMap::new(st.mini_width, st.mini_height, st.mini_terrain.clone(), st.mini_num_land_tiles, true);
        // P4a's `WaterManager` constructor body on the pristine minimap (`WaterManager.ts:74-92`),
        // one implementation (request P4a → P3 #1, merge1).
        let (graph, comps) = crate::water::manager::build_version0(&mini);
        st.graph0 = Some(graph);
        st.components0 = Some(comps);
        Ok(Arc::new(st))
    }

    /// The terrain half of [`MapStatics::load`] (everything but the water graph): maps,
    /// land index, compact scaling, layer validation. TS throws become `Err(message)`.
    pub(crate) fn load_terrain(files: &MapFiles) -> Result<MapStatics, String> {
        let map = &files.map;
        let mut manifest = files.manifest.clone();
        let bin = |data: &Option<Vec<u8>>, name: &str| -> Result<Arc<[u8]>, String> {
            match data {
                Some(d) => Ok(Arc::from(d.as_slice())),
                None => Err(format!("{name} of {map} was not passed")),
            }
        };
        let normal = files.map_size == GameMapSize::Normal;
        // TS: `:37-54` (map.bin+map4x.bin for Normal, map4x.bin+map16x.bin for Compact)
        let (main_md, main_bin) = if normal {
            (manifest.map, bin(&files.map_bin, "map.bin")?)
        } else {
            (manifest.map4x, bin(&files.map4x_bin, "map4x.bin")?)
        };
        let main_terrain = gen_terrain_from_bin_md(&main_md, main_bin)?;
        let (mini_md, mini_bin) = if normal {
            (manifest.map4x, bin(&files.map4x_bin, "map4x.bin")?)
        } else {
            (manifest.map16x, bin(&files.map16x_bin, "map16x.bin")?)
        };
        let mini_terrain = gen_terrain_from_bin_md(&mini_md, mini_bin)?;

        if !normal {
            manifest.nations.iter_mut().for_each(halve_coords);
            if let Some(a) = manifest.additional_nations.as_mut() {
                a.iter_mut().for_each(halve_coords);
            }
        }
        // TS: `:75-88` scale spawn areas for compact maps
        let mut areas = manifest.team_game_spawn_areas_entries()?;
        if !normal && let Some(a) = areas.as_mut() {
            for (_, v) in a.iter_mut() {
                for s in v.iter_mut() {
                    *s = SpawnArea {
                        x: of_det::js::floor(s.x / 2.0),
                        y: of_det::js::floor(s.y / 2.0),
                        width: of_det::js::max(1.0, of_det::js::floor(s.width / 2.0)),
                        height: of_det::js::max(1.0, of_det::js::floor(s.height / 2.0)),
                    };
                }
            }
        }
        validate_layers(map, &manifest)?;

        // The land index (ARCHITECTURE §3.6.2): passable land tiles (`isLand && !isImpassable`,
        // the only tiles `conquer` accepts) numbered in tile order. Their count is the map's
        // `numLandTiles` (checked for all 408 shipped bins at the pin), which sizes the
        // `PlayerTileOrder` position arrays.
        let mut land_index = vec![NO_POS; main_terrain.len()];
        let mut next = 0u32;
        for (t, &b) in main_terrain.iter().enumerate() {
            if b & 0x80 != 0 && (b & 0x1f) != 31 {
                land_index[t] = next;
                next += 1;
            }
        }
        if next != main_md.num_land_tiles {
            return Err(format!(
                "Map {map}: {next} passable land tiles in the bin, the manifest says num_land_tiles = {}",
                main_md.num_land_tiles
            ));
        }

        Ok(MapStatics {
            width: main_md.width,
            height: main_md.height,
            terrain: main_terrain,
            num_land_tiles: main_md.num_land_tiles,
            mini_width: mini_md.width,
            mini_height: mini_md.height,
            mini_terrain,
            mini_num_land_tiles: mini_md.num_land_tiles,
            land_index: Arc::from(land_index),
            nations: manifest.nations.iter().map(nation_template).collect(),
            additional_nations: manifest.additional_nations.iter().flatten().map(nation_template).collect(),
            team_game_spawn_areas: areas.map(Arc::new),
            manifest: Some(Arc::new(manifest)),
            graph0: None,
            components0: None,
        })
    }
    /// Land index of tile `t` (`tileset::NO_POS` for water).
    #[inline]
    pub fn land_index_of(&self, t: TileRef) -> u32 {
        self.land_index[t as usize] // in-range: t is a valid tile ref
    }
}

/// `genTerrainFromBin(mapData, data)` (`TerrainMapLoader.ts:121-137`): the length check, then
/// the `GameMapImpl` constructor's (`GameMapImpl.ts:49-53`).
pub fn gen_terrain_from_bin(width: u32, height: u32, data: &[u8]) -> Result<Arc<[u8]>, String> {
    let md = MapMetadata { width, height, num_land_tiles: 0 };
    gen_terrain_from_bin_md(&md, Arc::from(data))
}

fn gen_terrain_from_bin_md(md: &MapMetadata, data: Arc<[u8]>) -> Result<Arc<[u8]>, String> {
    if data.len() as u64 != u64::from(md.width) * u64::from(md.height) {
        return Err(format!(
            "Invalid data: buffer size {} incorrect for {}x{} terrain plus 4 bytes for dimensions.",
            data.len(),
            md.width,
            md.height
        ));
    }
    Ok(data)
}

/// Layer validation at game start (`TerrainMapLoader.ts:90-109`); `layers` is one of the
/// untyped manifest keys (`MapManifest.extra`).
fn validate_layers(map: &str, manifest: &MapManifest) -> Result<(), String> {
    let Some(layers) = manifest.extra.get("layers").and_then(|l| l.as_array()) else { return Ok(()) };
    for layer in layers {
        let id = layer.get("id").and_then(|v| v.as_str()).unwrap_or("undefined");
        let placement = layer.get("placement").and_then(|v| v.as_str());
        if placement != Some("land") && placement != Some("water") {
            let shown = placement.map_or_else(|| "undefined".to_owned(), str::to_owned);
            return Err(format!(
                "Map {map}: layer \"{id}\" has invalid placement \"{shown}\" (must be \"land\" or \"water\")"
            ));
        }
        if let Some(alpha) = layer.get("alpha") {
            let a = alpha.as_f64();
            if !a.is_some_and(|a| a.is_finite() && (0.0..=1.0).contains(&a)) {
                return Err(format!(
                    "Map {map}: layer \"{id}\" has invalid alpha {alpha} (must be a finite number between 0 and 1)"
                ));
            }
        }
    }
    Ok(())
}
