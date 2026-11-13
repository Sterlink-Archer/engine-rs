# of-engine file map and work split

Engine pin: `upstream/` @ `5b75bd3`. Design v2 (2026-11-05; review log at the end of ARCHITECTURE.md). The source of truth for the file list is `ops/engine_files.txt` (152 files, 48,149 lines including `Maps.gen.ts`).
The table in §1 is **generated** by `engine-rs/tools/filemap.py` (run from the project root: `python3 -I engine-rs/tools/filemap.py ops/engine_files.txt table`; `… sums` prints per-package totals). Re-run it on every upstream bump; it reports every file that is missing (`MISSING`) or no longer present (`EXTRA`) on stderr.

Status values:
- **port**: translate to Rust.
- **external**: owned by `of-det` (conformance workflow); never duplicated.
- **generated**: produced from upstream data by a script.
- **spec-only**: not ported as code. It is the authoritative field list for CS v1 export (P13) and the skeleton (S0).
- **skip**: not ported. The reason is given; ARCHITECTURE §5 covers view code.

Packages (P1–P13) are defined in §2.

---

## 1. Every TS file → Rust module

Modules are relative to the `of_engine` crate root (`engine-rs/crates/of-engine/src/`). Paths are relative to `upstream/packages/`.

| # | TS file (packages/…) | lines | Rust module (`of_engine::…`) | pkg | status | note |
|---|---|---|---|---|---|---|
| 1 | `engine-api/src/Schemas.ts` | 665 | `api::intents, api::game_config` | P1 | port | serde: Intent (tagged by `type`), GameConfig with zod defaults, GameStartInfo, Turn |
| 2 | `engine-api/src/game/GameTypes.ts` | 536 | `api::types` | P1 | port | enums UnitType/PlayerType/…, PlayerInfo, Cell, Team consts |
| 3 | `engine-api/src/game/ReadViews.ts` | 77 | `api::views (traits PlayerLike/UnitLike/GameLike)` | P1 | port | needed so config fns are vector-testable without a Game |
| 4 | `engine-lib/src/Util.ts` | 267 | `util (+ of-det for toInt/minInt/simpleHash/sigmoid)` | P1 | port |  |
| 5 | `engine-lib/src/game/TerraNulliusImpl.ts` | 20 | `ids (SmallId 0 = TN)` | P1 | port | no struct: owner 0 |
| 6 | `engine-lib/src/game/TileSet.ts` | 242 | `tileset` | P1 | port | OrderedTileSet + PlayerTileOrder (skeleton, S0) |
| 7 | `engine/src/GameRunner.ts` | 439 | `runner (Game::new/step)` | P1 | port | view half (placeName, packing) dropped |
| 8 | `engine/src/execution/ExecutionManager.ts` | 174 | `runner::executor` | P1 | port | intent → Exec |
| 9 | `engine/src/execution/NoOpExecution.ts` | 28 | `exec::noop` | P1 | port |  |
| 10 | `engine/src/game/AllianceImpl.ts` | 141 | `state::alliance` | P1 | port |  |
| 11 | `engine/src/game/AllianceRequestImpl.ts` | 94 | `state::alliance_request` | P1 | port |  |
| 12 | `engine/src/game/Game.ts` | 699 | `state::game (types) + exec::mod (Exec trait)` | P1 | port | interfaces → concrete structs |
| 13 | `engine/src/game/GameImpl.ts` | 1894 | `state::game + runner (tick loop)` | P1 | port |  |
| 14 | `engine-api/src/StatsSchemas.ts` | 219 | `state::stats::schema` | P2 | port | index constants (ATTACK_INDEX_SENT …) + shapes |
| 15 | `engine/src/game/PlayerImpl.ts` | 2182 | `state::player` | P2 | port |  |
| 16 | `engine/src/game/Stats.ts` | 192 | `state::stats (interface)` | P2 | port |  |
| 17 | `engine/src/game/StatsImpl.ts` | 609 | `state::stats` | P2 | port |  |
| 18 | `engine/src/game/UnitImpl.ts` | 972 | `state::unit` | P2 | port |  |
| 19 | `engine-api/src/game/GameMap.ts` | 113 | `map::game_map (inherent methods; no trait)` | P3 | port | interface only |
| 20 | `engine-api/src/game/MapFiles.ts` | 25 | `api::map_files` | P3 | port | MapFiles/Nation/AdditionalNation types |
| 21 | `engine-api/src/game/MapManifest.ts` | 267 | `map::manifest` | P3 | port | serde for manifest.json |
| 22 | `engine-api/src/game/Maps.gen.ts` | 2310 | `map::maps_gen (generated)` | P3 | generated | build.rs or codegen script from map-generator/assets/maps/*/info.json |
| 23 | `engine-lib/src/configuration/Config.ts` | 460 | `config::config` | P3 | port |  |
| 24 | `engine-lib/src/execution/utils/TribeNames.ts` | 93 | `setup::tribe_names` | P3 | port |  |
| 25 | `engine-lib/src/game/DoomsdayClock.ts` | 376 | `config::doomsday` | P3 | port |  |
| 26 | `engine-lib/src/game/GameMapImpl.ts` | 788 | `map::game_map` | P3 | port |  |
| 27 | `engine-lib/src/game/NationCount.ts` | 77 | `setup::nation_count` | P3 | port |  |
| 28 | `engine-lib/src/game/TeamAssignment.ts` | 329 | `setup::team_assignment` | P3 | port |  |
| 29 | `engine-lib/src/game/TerrainMapLoader.ts` | 137 | `map::loader` | P3 | port | map/map4x/map16x selection |
| 30 | `engine-lib/src/game/UnitGrid.ts` | 321 | `map::unit_grid` | P3 | port | flat intrusive lists |
| 31 | `engine-lib/src/game/Veterancy.ts` | 23 | `config::veterancy` | P3 | port |  |
| 32 | `engine/src/configuration/EngineConfig.ts` | 830 | `config::engine_config` | P3 | port |  |
| 33 | `engine/src/game/NationCreation.ts` | 655 | `setup::nation_creation` | P3 | port | consumes game PRNG |
| 34 | `engine/src/game/TileTraversalScratch.ts` | 50 | `scratch::traversal` | P3 | port | Scratch field via Ctx; gen reset at step start (ARCH §3.8) |
| 35 | `engine/src/game/WaterManager.ts` | 910 | `water::manager` | P4a | port |  |
| 36 | `engine/src/pathfinding/algorithms/AStar.AbstractGraph.ts` | 244 | `water::astar_abstract` | P4a | port | Float32 gScore |
| 37 | `engine/src/pathfinding/algorithms/AStar.WaterHierarchical.ts` | 475 | `water::hpa` | P4a | port | HPA* on the CURRENT graph; segment cache per-game visibility |
| 38 | `engine/src/pathfinding/algorithms/AbstractGraph.ts` | 744 | `water::abstract_graph` | P4a | port | edge cache: shared OnceLock + per-game filled bitset/overlay |
| 39 | `engine/src/pathfinding/algorithms/BFS.Grid.ts` | 122 | `water::bfs_grid` | P4a | port |  |
| 40 | `engine/src/pathfinding/algorithms/BFS.ts` | 64 | `water::bfs` | P4a | port |  |
| 41 | `engine/src/pathfinding/algorithms/ConnectedComponents.ts` | 362 | `water::components` | P4a | port | u8→u16 promotion is state |
| 42 | `engine/src/pathfinding/algorithms/PriorityQueue.ts` | 179 | `water::priority_queue` | P4a | port | Float32 priorities |
| 43 | `engine/src/pathfinding/PathFinder.Air.ts` | 75 | `path::air` | P4b | port |  |
| 44 | `engine/src/pathfinding/PathFinder.Parabola.ts` | 146 | `path::parabola` | P4b | port |  |
| 45 | `engine/src/pathfinding/PathFinder.Station.ts` | 78 | `path::station` | P4b | port |  |
| 46 | `engine/src/pathfinding/PathFinder.ts` | 373 | `path::pathfinder (WaterPathFinder, chains, memo)` | P4b | port | chain slots + WaterPathMemo are per-game STATE (ARCH §3.8) |
| 47 | `engine/src/pathfinding/PathFinderBuilder.ts` | 50 | `path::builder (static chain types)` | P4b | port |  |
| 48 | `engine/src/pathfinding/PathFinderStepper.ts` | 161 | `path::stepper` | P4b | port | catch_unwind: TsThrow → NOT_FOUND, else re-raise (PG §11) |
| 49 | `engine/src/pathfinding/algorithms/AStar.Rail.ts` | 114 | `path::astar_rail` | P4b | port |  |
| 50 | `engine/src/pathfinding/algorithms/AStar.Water.ts` | 253 | `path::astar_water` | P4b | port |  |
| 51 | `engine/src/pathfinding/algorithms/AStar.WaterBounded.ts` | 331 | `path::astar_water_bounded` | P4b | port |  |
| 52 | `engine/src/pathfinding/algorithms/AStar.ts` | 127 | `path::astar` | P4b | port |  |
| 53 | `engine/src/pathfinding/spatial/SpatialQuery.ts` | 317 | `path::spatial_query` | P4b | port | per-player memo (derived) |
| 54 | `engine/src/pathfinding/transformers/ComponentCheckTransformer.ts` | 35 | `path::transform::component_check` | P4b | port |  |
| 55 | `engine/src/pathfinding/transformers/MiniMapTransformer.ts` | 124 | `path::transform::minimap` | P4b | port | Math.round ×2 |
| 56 | `engine/src/pathfinding/transformers/ShoreCoercingTransformer.ts` | 113 | `path::transform::shore` | P4b | port |  |
| 57 | `engine/src/pathfinding/transformers/SmoothingWaterTransformer.ts` | 347 | `path::transform::smoothing` | P4b | port |  |
| 58 | `engine/src/pathfinding/types.ts` | 32 | `path::types` | P4b | port |  |
| 59 | `engine/src/utilities/Line.ts` | 257 | `path::line` | P4b | port | Math.round ×20, >>8; used by MIRV + Parabola |
| 60 | `engine/src/execution/FactoryExecution.ts` | 88 | `exec::factory` | P5 | port |  |
| 61 | `engine/src/execution/RecomputeRailClusterExecution.ts` | 38 | `exec::rail_cluster` | P5 | port |  |
| 62 | `engine/src/execution/TrainExecution.ts` | 496 | `exec::train` | P5 | port | usedTiles → VecDeque (same order) |
| 63 | `engine/src/execution/TrainStationExecution.ts` | 166 | `exec::train_station` | P5 | port |  |
| 64 | `engine/src/game/RailNetwork.ts` | 17 | `rail::mod (no trait)` | P5 | port | interface |
| 65 | `engine/src/game/RailNetworkImpl.ts` | 987 | `rail::network` | P5 | port | ghost-rail preview = skip (UI) |
| 66 | `engine/src/game/Railroad.ts` | 125 | `rail::railroad` | P5 | port |  |
| 67 | `engine/src/game/RailroadSpatialGrid.ts` | 123 | `rail::grid` | P5 | port | u32 cell keys, JsMap order kept (CS) |
| 68 | `engine/src/game/TrainStation.ts` | 336 | `rail::station` | P5 | port | stop handlers take no PRNG (:59-68) |
| 69 | `engine/src/execution/AttackExecution.ts` | 580 | `exec::attack` | P6 | port | HOT; iter-snapshot :169-178 |
| 70 | `engine/src/execution/DoomsdayClockExecution.ts` | 453 | `exec::doomsday` | P6 | port |  |
| 71 | `engine/src/execution/PlayerExecution.ts` | 1266 | `exec::player` | P6 | port | HOT (removeClusters); iter-snapshot :60-79,:107-111 |
| 72 | `engine/src/execution/RetreatExecution.ts` | 88 | `exec::retreat` | P6 | port |  |
| 73 | `engine/src/execution/SpawnExecution.ts` | 261 | `exec::spawn` | P6 | port |  |
| 74 | `engine/src/execution/SpawnTimerExecution.ts` | 46 | `exec::spawn_timer` | P6 | port |  |
| 75 | `engine/src/execution/TribeSpawner.ts` | 156 | `setup::tribe_spawner` | P6 | port |  |
| 76 | `engine/src/execution/Util.ts` | 230 | `exec::util` | P6 | port | closestTwoTiles sort |
| 77 | `engine/src/execution/WinCheckExecution.ts` | 256 | `exec::win_check` | P6 | port |  |
| 78 | `engine/src/execution/utils/FlatBinaryHeap.ts` | 107 | `exec::flat_heap` | P6 | port | f32 store / f64 compare; vectors `order` |
| 79 | `engine/src/execution/utils/PlayerSpawner.ts` | 25 | `setup::player_spawner` | P6 | port |  |
| 80 | `engine/src/game/AttackImpl.ts` | 251 | `state::attack` | P6 | port |  |
| 81 | `engine/src/execution/BoatRetreatExecution.ts` | 78 | `exec::boat_retreat` | P7 | port |  |
| 82 | `engine/src/execution/CityExecution.ts` | 84 | `exec::city` | P7 | port |  |
| 83 | `engine/src/execution/ConstructionExecution.ts` | 244 | `exec::construction` | P7 | port |  |
| 84 | `engine/src/execution/DefensePostExecution.ts` | 153 | `exec::defense_post` | P7 | port |  |
| 85 | `engine/src/execution/DeleteUnitExecution.ts` | 140 | `exec::delete_unit` | P7 | port |  |
| 86 | `engine/src/execution/MoveWarshipExecution.ts` | 91 | `exec::move_warship` | P7 | port |  |
| 87 | `engine/src/execution/PortExecution.ts` | 201 | `exec::port` | P7 | port |  |
| 88 | `engine/src/execution/ShellExecution.ts` | 183 | `exec::shell` | P7 | port | Math.round |
| 89 | `engine/src/execution/TradeShipExecution.ts` | 294 | `exec::trade_ship` | P7 | port |  |
| 90 | `engine/src/execution/TransportShipExecution.ts` | 426 | `exec::transport_ship` | P7 | port |  |
| 91 | `engine/src/execution/UpgradeStructureExecution.ts` | 94 | `exec::upgrade_structure` | P7 | port |  |
| 92 | `engine/src/execution/WarshipExecution.ts` | 914 | `exec::warship` | P7 | port |  |
| 93 | `engine/src/game/TransportShipUtils.ts` | 52 | `state::transport_ship_utils` | P7 | port |  |
| 94 | `engine-lib/src/execution/NukeAlliance.ts` | 128 | `exec::nuke_alliance` | P8 | port |  |
| 95 | `engine/src/execution/MIRVExecution.ts` | 650 | `exec::mirv` | P8 | port | only exec→exec ref (warheads) |
| 96 | `engine/src/execution/MissileSiloExecution.ts` | 83 | `exec::missile_silo` | P8 | port |  |
| 97 | `engine/src/execution/NukeExecution.ts` | 584 | `exec::nuke` | P8 | port |  |
| 98 | `engine/src/execution/SAMLauncherExecution.ts` | 493 | `exec::sam_launcher` | P8 | port |  |
| 99 | `engine/src/execution/SAMMissileExecution.ts` | 171 | `exec::sam_missile` | P8 | port |  |
| 100 | `engine/src/execution/DonateGoldExecution.ts` | 182 | `exec::donate_gold` | P9 | port |  |
| 101 | `engine/src/execution/DonateTroopExecution.ts` | 182 | `exec::donate_troops` | P9 | port |  |
| 102 | `engine/src/execution/EmbargoAllExecution.ts` | 73 | `exec::embargo_all` | P9 | port |  |
| 103 | `engine/src/execution/EmbargoExecution.ts` | 80 | `exec::embargo` | P9 | port |  |
| 104 | `engine/src/execution/EmojiExecution.ts` | 127 | `exec::emoji` | P9 | port |  |
| 105 | `engine/src/execution/MarkDisconnectedExecution.ts` | 69 | `exec::mark_disconnected` | P9 | port |  |
| 106 | `engine/src/execution/PauseExecution.ts` | 61 | `exec::pause` | P9 | port |  |
| 107 | `engine/src/execution/QuickChatExecution.ts` | 129 | `exec::quick_chat` | P9 | port |  |
| 108 | `engine/src/execution/TargetPlayerExecution.ts` | 78 | `exec::target_player` | P9 | port |  |
| 109 | `engine/src/execution/TribeAttackBehavior.ts` | 75 | `exec::tribe::attack_behavior` | P9 | port |  |
| 110 | `engine/src/execution/TribeExecution.ts` | 215 | `exec::tribe` | P9 | port |  |
| 111 | `engine/src/execution/alliance/AllianceExtensionExecution.ts` | 120 | `exec::alliance::extension` | P9 | port |  |
| 112 | `engine/src/execution/alliance/AllianceRejectExecution.ts` | 86 | `exec::alliance::reject` | P9 | port |  |
| 113 | `engine/src/execution/alliance/AllianceRequestExecution.ts` | 212 | `exec::alliance::request` | P9 | port |  |
| 114 | `engine/src/execution/alliance/BreakAllianceExecution.ts` | 103 | `exec::alliance::break_` | P9 | port |  |
| 115 | `engine/src/execution/utils/AiAttackBehavior.ts` | 483 | `exec::ai_attack` | P9 | port | generic over AiAttackHooks (7 nation overrides, ARCH §4.4) |
| 116 | `engine/src/execution/NationExecution.ts` | 564 | `exec::nation::mod` | P10 | port |  |
| 117 | `engine/src/execution/nation/BotAnnexation.ts` | 182 | `exec::nation::annexation` | P10 | port |  |
| 118 | `engine/src/execution/nation/NationAllianceBehavior.ts` | 514 | `exec::nation::alliance` | P10 | port |  |
| 119 | `engine/src/execution/nation/NationAttackBehavior.ts` | 1598 | `exec::nation::attack` | P10 | port | impl AiAttackHooks for NationExec |
| 120 | `engine/src/execution/nation/NationEmojiBehavior.ts` | 403 | `exec::nation::emoji` | P10 | port |  |
| 121 | `engine/src/execution/nation/NationUtils.ts` | 189 | `exec::nation::utils` | P10 | port |  |
| 122 | `engine/src/execution/nation/NationMIRVBehavior.ts` | 472 | `exec::nation::mirv` | P11 | port |  |
| 123 | `engine/src/execution/nation/NationNukeBehavior.ts` | 1205 | `exec::nation::nuke` | P11 | port |  |
| 124 | `engine/src/execution/nation/NationNukePlanning.ts` | 495 | `exec::nation::nuke_planning` | P11 | port |  |
| 125 | `engine/src/execution/nation/SharedWaterCache.ts` | 186 | `state::shared_water_cache` | P11 | port | TTL cache = STATE (W section) |
| 126 | `engine/src/execution/nation/NationStructureBehavior.ts` | 1138 | `exec::nation::structure` | P12 | port |  |
| 127 | `engine/src/execution/nation/NationStructurePlacement.ts` | 565 | `exec::nation::structure_placement` | P12 | port |  |
| 128 | `engine/src/execution/nation/NationWarshipBehavior.ts` | 631 | `exec::nation::warship` | P12 | port |  |
| 129 | `engine/src/execution/nation/TutorialCollapse.ts` | 24 | `exec::nation::tutorial` | P12 | port |  |
| 130 | `engine/src/execution/nation/TutorialSilo.ts` | 201 | `exec::nation::tutorial` | P12 | port | imul |
| 131 | `engine-lib/src/snapshot/SnapshotType.ts` | 113 | — | P13 | spec-only | zod helpers; field-type grammar for CS export |
| 132 | `engine/src/snapshot/CommonSchemas.ts` | 83 | — | P13 | spec-only |  |
| 133 | `engine/src/snapshot/ExecutionRegistry.ts` | 100 | — | P13 | spec-only | the 44 Exec variants (skeleton S0 reads it) |
| 134 | `engine/src/snapshot/ExecutionSnapshot.ts` | 43 | — | P13 | spec-only |  |
| 135 | `engine/src/snapshot/GameSnapshot.ts` | 408 | — | P13 | spec-only | root assembly order for CS |
| 136 | `engine/src/snapshot/PathfinderSnapshots.ts` | 198 | — | P13 | spec-only | stepper state field list |
| 137 | `engine/src/snapshot/SnapshotContext.ts` | 221 | — | P13 | spec-only |  |
| 138 | `engine/src/snapshot/SnapshotExtractor.ts` | 157 | — | P13 | spec-only |  |
| 139 | `engine-lib/src/DetMath.ts` | 132 | `of-det::detmath (re-export)` | of-det | external | owned by conformance wf |
| 140 | `engine-lib/src/PseudoRandom.ts` | 121 | `of-det::prng (re-export)` | of-det | external | owned by conformance wf; never duplicated |
| 141 | `engine-api/src/game/GameUpdates.ts` | 400 | — | — | skip | view updates; only enums reused for optional `events` feature (P13) |
| 142 | `engine-api/src/game/SnapshotHeader.ts` | 12 | — | — | skip | snapshot codec header; CS v1 replaces it |
| 143 | `engine-lib/src/Format.ts` | 35 | — | — | skip | display strings only (renderNumber/renderTroops feed displayMessage) |
| 144 | `engine-lib/src/game/MotionPlans.ts` | 212 | — | — | skip | view packing; planDrivenUnitIds kept in State (P1) |
| 145 | `engine-lib/src/game/PackedUnitUpdates.ts` | 539 | — | — | skip | view packing |
| 146 | `engine/src/WinnerReplay.ts` | 67 | — | — | skip | server subprocess entry |
| 147 | `engine/src/game/GameUpdateUtils.ts` | 280 | — | — | skip | view diff |
| 148 | `engine/src/game/HumanStats.ts` | 48 | — | — | skip | game-record output (deepCopy of stats) |
| 149 | `engine/src/game/NameBoxCalculator.ts` | 246 | — | — | skip | view (placeName) |
| 150 | `engine/src/game/TerrainSearchMap.ts` | 65 | — | — | skip | dead code at 5b75bd3: no importer (grep) |
| 151 | `engine/src/snapshot/SnapshotCodec.ts` | 365 | — | — | skip | byte codec; CS v1/CSE1 (of-canon) replaces it |
| 152 | `engine/src/utilities/DebugSpan.ts` | 167 | — | — | skip | perf instrumentation |

---

## 2. Packages: ownership, size, dependencies, acceptance

Line counts are TS lines owned, from the generator. "Needs" lists other packages' **APIs** (frozen at S0, §3), not their implementations: every package compiles against `todo!()` stubs from day one.

**Every mutating cross-package method takes `cx: &mut Ctx`** (state + scratch + optional journal). Read-only methods take `&State`, or `&mut Rcx` if they touch scratch (ARCHITECTURE §3.8). The S0 signature fixes which form each method uses. When in doubt, P1 picks the `Ctx` form.

| Pkg | Name | TS lines (files) | Owns | Provides (frozen API, implemented here) | Needs (APIs) |
|---|---|---|---|---|---|
| **P1** | Core state + runner (**skeleton owner**) | 5,276 (13) | `GameImpl`, `Game.ts`, `GameRunner`, `ExecutionManager`, `GameTypes`, `Schemas`, `ReadViews`, `TerraNullius`, `NoOp`, `AllianceImpl`, `AllianceRequestImpl`, `TileSet`, engine-lib `Util` | Crate skeleton (S0), plus the `of-engine-derive` crate (`StateClone`). Foundations: `ids`, `arena`, `ctx` (`Ctx`/`Rcx`), `jsarr` (`JsArr` + `iter_snapshot`), `tileset`, `jsorder`, `scratch` (struct with one field per TS scratch object; fields are filled by their owners). `State`, with the TS `Game` interface methods (≈99, `game/Game.ts:512-699`). `Game::{new, step, step_with, step_with_execs, clone_into, fork_into}` and `StepEvents`. The tick loop with `ExecArenas`/`exec_inbox`/`ExecCmd`/`PendingExec`/pinning (ARCHITECTURE §4.2–4.3). The post-throw handling (§4.6). `conquer`/`relinquish`/`updateBorders`. Alliance accept/reject/break/expire (`alliance_requests: JsArr`). `units()`/`unitCount` memos (`TypeMemo`). `addUnit`/`removeUnit`. `conquerPlayer`. `add_player` (with the duplicate-id assert). Intent → Exec mapping. serde `GameConfig`/`Intent`/`Turn`/`GameStartInfo`. The GC driver (`collect_garbage`, ARCHITECTURE §3.11). The **`of-det` contributions** (ARCHITECTURE §12 G7, t088). | P2 Player/Unit; P3 map/config/setup; P5 `remove_station`; P13 `Canon`/`RefVisitor` framework |
| **P2** | Player, Unit, Stats | 4,174 (5) | `PlayerImpl`, `UnitImpl`, `StatsImpl`, `Stats.ts`, `StatsSchemas` | `PlayerRef`/`PlayerMut` (TS `Player` interface, ≈127 members, `game/Game.ts:314-510`), with `units`/`incoming_attacks`/`outgoing_attacks`/`alliances` as `JsArr`. `UnitRef`/`UnitMut` (TS `Unit`, ≈68 members, `:206-299`), with `delete`/`set_owner` via `filter_reassign`. The view-side-effect shim `unit_touched` (ARCHITECTURE §5) and `warship_state()` with its write. `Stats` (lite with strict conversions at every entry point, and `stats-full`). The `verify-memos` feature. | P1 State/Ctx; P3 `GameMap`/`UnitGrid`/`EngineConfig`; P4b `SpatialQuery`, `TransportShipUtils` callers; P5 `RailNetwork` |
| **P3** | Map, config, setup | 6,854 (16; 4,544 hand-written + 2,310 generated) | `GameMapImpl`, `UnitGrid`, `TerrainMapLoader`, `MapManifest`, `MapFiles`, `Maps.gen` (gen), `TileTraversalScratch`, `Config`, `EngineConfig`, `Veterancy`, `DoomsdayClock` (lib), `NationCount`, `TeamAssignment`, `NationCreation`, `TribeNames` | `GameMap` (engine-api `GameMap` interface, `engine-api/src/game/GameMap.ts`). `TileStateBuf` with the frozen write accessors, and `terrain_gen` (ARCHITECTURE §3.6.1, §8.5). `bfs`/`bfs_search` on `sc.bfs_main`/`sc.bfs_mini` with `busy` flags. `sc.traversal` with the step-start wrap reset (G14). `UnitGrid` (flat intrusive lists, DefensePost counts, ARCHITECTURE §3.7). `MapStatics::load` (terrain, land index; graph0/components0 come from P4a). `EngineConfig`: every public method, `PlayerLike`-generic formulas, strict BigInt conversions. Setup: `create_nations_for_game`, `assign_teams`, tribe names. | of-det; P1 `api::views` traits, `Ctx`; P4a `WaterGraph::build` (for `MapStatics`) |
| **P4a** | Water graph | 3,100 (8) | `WaterManager`, `AbstractGraph`, `ConnectedComponents`, `AStar.WaterHierarchical`, `AStar.AbstractGraph`, `PriorityQueue`, `BFS`, `BFS.Grid` | `WaterGraph` (one `Arc` per version, `built_at_mini_gen`, `seg_shared` OnceLocks). `SegVisibility` (per-game `filled` bitset + overlay) and the hit/fill rule (ARCHITECTURE §3.8). `Components` (u8/u16, COW; the **only** component source). `WaterManager::{queue_tile, tick, component_of, …}`, including the full minimap edit sequence. HPA `find_path` on the **current** graph, with `sc.hpa` (one field per TS A* instance). `sc.water_mgr` (stamp wrap documented). `graph0`/`components0` for `MapStatics`. | P3 `GameMap`; P1 `Ctx`/`Scratch` |
| **P4b** | Path chains + spatial | 2,933 (17) | `PathFinder.ts`, `PathFinderStepper`, `PathFinderBuilder`, `types`, 4 transformers, `AStar`, `AStar.Water`, `AStar.WaterBounded`, `AStar.Rail`, `PathFinder.{Air,Parabola,Station}`, `SpatialQuery`, `utilities/Line` | `ChainSlots`/`ChainSlot`/`ChainKind` + `ensure_current_chain` at the TS `sharedWaterChain` call sites. `WaterPathMemo` (literal LRU, per chain slot; **state**). `WaterPathFinder` (stagger, `chain_version`; no graph ref). `PathFinding::{water, rail, stations, air}`. `Stepper` (with the `catch_unwind` boundary: `TsThrow` → `NotFound`, else re-raise). Parabola/Bezier (`Line`). `SpatialQuery`. | P4a; P3; P5 `TrainStation` getters |
| **P5** | Rail | 2,376 (9) | `RailNetworkImpl`, `RailNetwork.ts`, `Railroad`, `RailroadSpatialGrid`, `TrainStation`, `TrainExecution`, `TrainStationExecution`, `RecomputeRailClusterExecution`, `FactoryExecution` | `RailNetwork` (TS interface, `game/RailNetwork.ts`), stations (no stop-handler PRNG), clusters, railroads (`Arena`, GC-swept). The u32-keyed `RailroadSpatialGrid` (JsMap order kept for CS). `usedTiles` as `VecDeque`. 4 exec variants. | P1; P2; P4b rail/station pathfinders |
| **P6** | Land combat + player lifecycle (**hot path**) | 3,719 (12) | `AttackExecution`, `AttackImpl`, `FlatBinaryHeap` (wrapper over `of_det::heap`), `PlayerExecution`, `RetreatExecution`, `SpawnExecution`, `SpawnTimerExecution`, `WinCheckExecution`, `PlayerSpawner`, `TribeSpawner`, `execution/Util`, `DoomsdayClockExecution` | `Attack` methods (`delete` via `filter_reassign`). `AttackExec::new` (used by P7/P9/P10). `SpawnExec::new` (used by P10/P9). 7 exec variants. **The three `iter-snapshot` sites** (`PlayerExecution.ts:60-79,107-111`, `AttackExecution.ts:169-178`) with unit tests. Flat cluster scratch `sc.clusters`. | P1; P2; P3 |
| **P7** | Naval + structures | 2,954 (13) | `TransportShip`, `BoatRetreat`, `TradeShip`, `Warship`, `MoveWarship`, `Shell`, `Port`, `City`, `DefensePost`, `Construction`, `UpgradeStructure`, `DeleteUnit` executions; `TransportShipUtils` | 12 exec variants (incl. the `touch()` → `unit_touched` sites `WarshipExecution.ts:86,687,707`, `TradeShipExecution.ts:134`). `TransportShipExec::new` and `ConstructionExec::new` (used by P9/P10/P12). | P1; P2; P4b water pathfinding; P5 `TrainStationExec::new`; P6 `AttackExec::new` (landing); P8 `NukeExec`/`MirvExec`/`SamLauncherExec`/`MissileSiloExec::new` (Construction creates them) |
| **P8** | Nukes + air defence | 2,109 (6) | `NukeExecution`, `MIRVExecution`, `MissileSiloExecution`, `SAMLauncherExecution`, `SAMMissileExecution`, `NukeAlliance` | 5 exec variants. Uses P1's MIRV `ExecCmd` bind/cancel/unpin (ARCHITECTURE §4.3). The `touch()` site `NukeExecution.ts:504`. | P1; P2; P4b air/parabola/`Line` |
| **P9** | Diplomacy, social, tribe AI | 2,275 (16) | `alliance/*` (4), `Embargo`, `EmbargoAll`, `DonateGold`, `DonateTroops`, `Emoji`, `QuickChat`, `TargetPlayer`, `MarkDisconnected`, `Pause`, `TribeExecution`, `TribeAttackBehavior`, `AiAttackBehavior` | 14 exec variants. **`AiAttackBase` + `trait AiAttackHooks` (7 hooks, frozen at S0) + every base method as a generic free function** calling hooks through `h.` (ARCHITECTURE §4.4). `impl AiAttackHooks for TribeExec` (defaults only). | P1; P2; P6 `AttackExec`; P7 `TransportShipExec` |
| **P10** | Nation AI 1: core, attack, alliance, emoji | 3,450 (6) | `NationExecution` (mod), `NationAttackBehavior`, `NationAllianceBehavior`, `NationUtils`, `BotAnnexation`, `NationEmojiBehavior` | `NationExec` struct (fields frozen at S0) and these behaviour methods. **`impl AiAttackHooks for NationExec`**: all 7 overrides, with `super.` → `base_*`. | everything; **P9 `AiAttackBehavior` base implementation** (for any nation attack scenario); P11/P12 behaviour method stubs |
| **P11** | Nation AI 2: nukes, MIRV, shared water | 2,358 (4) | `NationNukeBehavior`, `NationNukePlanning`, `NationMIRVBehavior`, `SharedWaterCache` | nuke/MIRV behaviour methods; `SharedWaterCache` (state, W section) | P10 struct; P8 `NukeExec`/`MirvExec`; P7 `ConstructionExec`/`UpgradeStructureExec` |
| **P12** | Nation AI 3: structures, warships, tutorial | 2,559 (5) | `NationStructureBehavior`, `NationStructurePlacement`, `NationWarshipBehavior`, `TutorialSilo`, `TutorialCollapse` | structure/warship behaviour methods | P10 struct; P7 `ConstructionExec`/`MoveWarship`; P11 `SharedWaterCache` |
| **P13** | Conformance bridge + benches | 1,323 spec-only (8) | `snapshot/*` (read as spec), `engine-lib/src/snapshot/SnapshotType`. Owns `canon/` (framework, G/T/M/W/S sections, key assignment, drain loop, loader with TS-restore semantics) and `of-cli` (`conform`, `--probe`, `--prng-trace`, `bench`, `fork-bench`). | `Canon`/`RefVisitor` traits (S0, with P1), including `visit_chain_version`/`visit_railroad`. `Game::canon_k/canon_d/from_canon`. `fingerprint64`. The SPEC §3 divergence reports. `fork-bench` with pooled/unpooled timings, a counting allocator, page faults and per-component bytes (t091). | of-canon, of-trace; every package's `Canon` impls |

Skipped (2,436 lines, 12 files): `SnapshotCodec`, `SnapshotHeader`, `GameUpdates`, `GameUpdateUtils`, `PackedUnitUpdates`, `MotionPlans`, `NameBoxCalculator`, `HumanStats`, `Format`, `DebugSpan`, `WinnerReplay`, `TerrainSearchMap` (dead code: no importer at the pin [V grep]).

`of-det` (external, 253 lines): `PseudoRandom`, `DetMath`.

**Balance check.** Excluding generated and spec-only lines, the 12 porting packages range from 2.1 k to 5.3 k TS lines (median about 2.9 k).
- P1 is the largest because it also writes the skeleton and the `of-det` contributions. Its bodies (`GameImpl` 1,894 + `GameRunner` 439 + `ExecutionManager` 174) are mostly small methods, and the 1,200 lines of `Schemas`/`GameTypes` are type declarations.
- P9's base `AiAttackBehavior` becomes the hook trait plus generic functions; that is no more code, just a different shape.

### 2.1 Who constructs whom, and who calls whom virtually: the cross-package APIs frozen at S0

From SPEC §4.2 "Created by" (verified there with `grep -rlE "new <Class>\("`), plus the inheritance edge found in review:

| Exec constructor or API (owner) | Called from (packages) |
|---|---|
| `AttackExec::new(start_troops, owner, target_id, source_tile, remove_troops)` (P6) | intents (P1), P7 TransportShip landing, P9 AiAttackBehavior, P10 NationAttackBehavior |
| `SpawnExec::new(game_id, info, tile?, from_intent?)` (P6) | intents (P1), P10 Nation, P6 TribeSpawner/PlayerSpawner |
| `TransportShipExec::new(owner, dst, troops)` (P7) | intents (P1), P9 AiAttackBehavior, P10 NationAttackBehavior |
| `ConstructionExec::new(owner, unit, tile, rocket_up?, amount?)` (P7) | intents (P1), P12 NationStructure/NationWarship |
| `UpgradeStructureExec::new(owner, unit_id, amount)` (P7) | intents (P1), P11 NationNuke, P12 NationStructure |
| `NukeExec::new(type, owner, dst, src?, speed?, wait_ticks?)`, `MirvExec::new` (P8) | P7 Construction, P8 MIRV, P11 NationNuke/NationMIRV, P12 TutorialSilo |
| `MissileSiloExec`/`SamLauncherExec`/`CityExec`/`PortExec`/`DefensePostExec`/`FactoryExec::new` (P7/P8/P5) | P7 Construction (+ P12 TutorialSilo for the silo) |
| `TrainStationExec::new` (P5) | P7 City/Port, P5 Factory |
| `DonateTroopsExec::new` (P9) | intents (P1), P10 NationAttackBehavior |
| `AllianceRequestExec`/`AllianceExtensionExec::new` (P9) | intents (P1), P10 NationAllianceBehavior, P9 Tribe |
| `EmojiExec::new` (P9) | intents (P1), P9 Donate*, P10 NationEmojiBehavior |
| `DeleteUnitExec::new` (P7) | intents (P1), P9 Tribe |
| `NationExec::new(game_id, nation)` (P10) | P1 `Game::new` (init) |
| `SpawnTimerExec`, `WinCheckExec`, `DoomsdayClockExec`, `RecomputeRailClusterExec` (P6/P5) | P1 `Game::new` (init, `GameRunner.ts:183-207`) |
| `PlayerExec::new(player)` (P6) | P6 `SpawnExec` |
| `ShellExec` (P7), `SamMissileExec` (P8), `TradeShipExec` (P7), `TrainExec` (P5) | their own package |
| **`trait AiAttackHooks` + `AiAttackBase` + `base_*` free functions** (P9) | **implemented** by P9 (`TribeExec`, defaults) and **P10** (`NationExec`, 7 overrides: `should_attack`, `ignores_attacks_from`, `expansion_troop_cap`, `calculate_attack_troops`, `send_boat_attack`, `send_expansion_boat`, `sail`); base methods called by both owners' ticks |
| `JsArr::filter_reassign`/`iter_snapshot` (P1) | P2 (`delete`/`set_owner`), P6 (`Attack.delete`, the 3 snapshot loops), P1 (alliances, requests) |
| `cx.st.unit_touched(u)` (P2) | P1, P2, P7 (`touch()` sites), P8 (`NukeExecution.ts:504`) |
| `ensure_current_chain`, `WaterPathFinder::new(stagger, memoized)` (P4b) | P7 TransportShip/TradeShip/Warship, P9/P10 boat routing, P11 `SharedWaterCache` |

---

## 3. Stubs-first plan: how 12 packages proceed concurrently

### Stage S0: skeleton (P1 + P13, 1–2 days, one worktree; nothing else starts until it is merged)

Ledger: t092.

1. **Workspace and lints.**
   - `engine-rs/Cargo.toml`, with members `of-det`, `of-canon`, `of-trace`, `of-vectors` (conformance workflow), `of-engine-derive`, `of-engine` and `of-cli`.
   - `rust-toolchain.toml` and `clippy.toml` (PORTING_GUIDE §12, including the `catch_unwind`/`RefCell`/`expect` rules).
   - The CI script `engine-rs/tools/ci.sh`: `cargo fmt --check && cargo clippy -D warnings && cargo test`.
2. **Foundations, fully implemented**, because everyone needs them:
   - `ids` (incl. `ExecId`/`ExecKind`, `PlayerInfoIdx`) and `arena`
   - `ctx` (`Ctx`, `Rcx`)
   - `jsarr` (`JsArr` with `filter_reassign`/`iter_snapshot`, plus tests for push-before-reassign and delete-during-loop)
   - `tileset` (`OrderedTileSet` and `PlayerTileOrder` behind the `pos_*_set` accessors, with a TileSet vitest port and the V-TILESET vectors once t072 lands)
   - `jsorder`
   - `scratch` (the struct with every field declared; `prepare()` with the wrap-reset hook)
   - `of-engine-derive::StateClone` (with a test that `clone_from` reuses capacity)
   - `api::*` serde types plus the typed `Intent`
   - **`of-det` contributions** (t088): the `clear()` fix, the Fx hasher, `clone_from`, `SmallJsMap`/`SmallJsSet`, `TsThrow`/`ts_throw!`/strict BigInt, `prng-trace`, `first_live`. These are merged into the conformance workflow's crate, never copied.
3. **Every state struct with all its fields**, in dump field order. They are typed per the schema dump (`conformance/schema/snapshot_schemas_5b75bd3.txt`, 65 records), plus the non-snapshot fields ARCHITECTURE §3 lists:
   - `State`, `Player`, `Unit`, `Attack`, `Alliance`, `AllianceRequest`
   - `GameMap` (with `terrain_gen`, `TileStateBuf`), `UnitGrid`
   - `WaterManager` (with `graph`, `components`, `seg_vis`, `chains`)
   - `RailNetwork`, `TrainStation`, `Railroad`, `Cluster`
   - `Stats`, `SharedWaterCache`
   - **all 44 execution structs** in `ExecArenas`, with their nested behaviour structs (`NationExec` and its 7 behaviours, `AiAttackBase`).

   The dump is mechanical input, so no package has to change a struct to add state. A missing *non-snapshot* field is added by the owning package in its own file through a `pub(crate)` extension struct, never by editing `State`.
4. **Every cross-package method signature, with a `todo!()` body.** The frozen API is the TS interfaces, translated 1:1, each taking `&mut Ctx`, `&State` or `&mut Rcx` per §2:
   - `game/Game.ts`: `Game` ≈99, `Player` ≈127, `Unit` ≈68 (counts are lines containing `(`), `Attack`/`Alliance`/`AllianceRequest`/`MutableAlliance`
   - `engine-api/src/game/GameMap.ts` (≈64)
   - `game/RailNetwork.ts`, `game/Stats.ts`, `pathfinding/types.ts`
   - `EngineConfig`/`Config` public methods
   - every `XExec::new` in §2.1
   - **`trait AiAttackHooks`** with the 7 hook signatures (ARCHITECTURE §4.4), and the `base_*` free-function signatures
   - every public method of the 7 nation behaviour classes, as `impl NationExec` stubs. Each lives in the owning package's file, so P10–P12 never edit each other's files.
   - the `TileStateBuf`/`pos_*_set` write accessors (ARCHITECTURE §8.5), so copy-on-write chunks can be added later without touching ported code.
5. **Implemented in S0:**
   - the tick loop (`Game::step`, `execute_next_tick`, `drain_inbox`, `remove_inactive_executions`, `apply_exec_cmds`, pinning, the post-throw handling of ARCHITECTURE §4.6)
   - the `ExecArenas` match dispatch
   - `clone_into`/`fork_into`
   - the GC driver (marking delegates to `RefVisitor` stubs)
   - the `Canon`/`RefVisitor` traits with derive-like helper macros (P13)
   - an empty-world smoke test: `Game::new` on `tests/testdata` plains with 0 players steps 100 ticks
   - a `fork_identity` skeleton test (clone at tick 50, compare `fingerprint64` over 50 ticks).
6. **Gate:** `cargo check --all-targets` and `clippy` pass, and `cargo test` passes with the `todo!()`s unreached. P1 tags the skeleton `s0`. After that, **any signature change** goes through a ledger task to P1, merged as a one-line PR that every package rebases onto.

### Stage S1: parallel fill (packages P1–P13 concurrently, one `git worktree` per package)

- Each package edits **only its own files**, as listed in §1. Files are disjoint by construction: the generator asserts that every file has exactly one owner.
- Merge gate into `main`: the CI script, plus the package's ladder rungs reached so far. Rebase daily on `main`.
- **Test ladder per package** (run in order; each rung needs only the APIs listed):

| Rung | What | Needs |
|---|---|---|
| T0 | Ported upstream vitest for the owned files (list in §4), plus the mandatory design tests in §4's last column | the owned code + stubs |
| T1 | VECTORS suites for the owned functions (L0) | — |
| T2 | Module tests (SPEC §5.2, L1) | as listed |
| T3 | Micro scenarios (SPEC §4.5 `M-*`) at L2 (K every tick) | the packages in that scenario's path |
| T4 | L3 (D + dumps) on the same scenarios, with `stats-full` | P13 exporter |

### Stage S2: integration (all packages; P13 drives)

- CI tier fully L2, then L3. Nightly corpus at L4, including S-FFA World with 72 Impossible nations + 400 tribes, and record replays (including any with tick errors: post-throw state is TS-exact, ARCHITECTURE §4.6).
- `verify-memos` and GC-every-tick runs on the CI corpus.
- Cross-arch on an aarch64 host (SPEC R5).
- `fork-bench` and `bench` gates (ARCHITECTURE §12 G1/G2).
- **Only then:** performance work under ARCHITECTURE §11.

### Critical path

```
S0 ─┬─ P3 (map/config: T0-T1 standalone, vectors `config`/`util`/`detmath`)
    ├─ P1 + P2 (state, players, units) ── P6 (attack/player/spawn/win) ──► first L2: M-SPAWN-*, M-ATK-01..08 on test maps
    ├─ P4a ── P4b ──► MOD-PATH-WATER (+ fork water tests) ──┐
    ├─ P5 (rail) ───────────────────────────────────────────┼─► P7 (boats, ships, structures) ─► M-BOAT/M-NAV/M-BLD/M-ECO/M-RAIL
    ├─ P8 (nukes) ──────────────────────────────────────────┘                                   ─► M-NUKE-*
    ├─ P9 (diplomacy + tribes + AiAttackBehavior base) ─► M-DIP/M-DON/M-SOC + every scenario with bots
    │      └──────────────────────────────┐
    └─ P10/P11/P12 (nation AI) ───────────┴─► M-AI-01..03 (with --prng-trace) ─► S-FFA (World) ─► L4
```

The longest chain is S0 → P1/P2 → P6 → P7 → P9 → P10–P12 → S-FFA.
- **P10 cannot pass any nation attack scenario before P9's `AiAttackBehavior` base is done**: its hook overrides run inside P9's base functions. P9 should therefore finish `ai_attack.rs` first, before its other 14 exec variants.
- P3, P4a, P4b, P5 and P8 have no upstream blocker beyond S0 for rungs T0–T2, and they should finish T2 while P1, P2 and P6 converge.
- P10–P12 share one PRNG stream, so their scenarios turn green together. `--prng-trace` (ARCHITECTURE §4.5) localises a divergence to one behaviour file.

---

## 4. Per-package acceptance (T0 upstream tests to port, T3 scenarios)

Upstream tests are in `upstream/tests/` (713 `*.test.ts` files; the relevant ones are listed). Scenario ids are from SPEC §4.5.

| Pkg | T0 upstream vitest (port the assertions) | T1 vectors | T2 modules | T3 scenarios | Mandatory design tests (T0) |
|---|---|---|---|---|---|
| P1 | `tests/core/GameRunner.test.ts`, `tests/core/game/*`, `PlayerAllianceList`, `AllianceRequestExecution`, `ConquerGold`, `UnitMemoInvalidation` | — | MOD-SNAPSHOT (with P13) | all (owner of the loop) | init-pass panic leaves `uninit`/`live` as TS does (§4.6); inbox order == call order; MIRV pin/unpin; `JsArr` push-before-reassign; `add_player` duplicate assert; `StateClone::clone_from` reuses capacity |
| P2 | `PlayerImpl`, `PlayerStats`, `Stats`, `StatsAlliances`, `StatsTiles`, `AttackStats`, `BoatLossStats`, `TransportCaptureStats`, `GoldDonationStats`, `TeamStats`, `WarshipVeterancy` | `config/maxTroops…` (via `PlayerLike`) | — | M-BLD-08, M-ECO-*, M-SOC-05 | stats send+cancel with a fractional troop count leaves `attacks_sent = -1` (floor asymmetry); a NaN stats value raises `TsThrow` in the lite build for a clientID player and nothing for a non-clientID player; `verify-memos` |
| P3 | `TileSet`, `UnitGrid`, `NeighborIteration`, `ImpassableTerrain`, `EngineConfig`, `TeamAssignment`, `Team`, `NationCreation`, `NationName`, `TribeNameThemes`, `MapConsistency`, `MapManifestSchema`, `tests/core/configuration/*` | `config` (79,584), `util` (DoomsdayClock, Veterancy, …) | MOD-SPATIAL | M-TEAM-*, M-CFG-* | traversal gen forced near 2^32 → identical results; nested `bfs_search` on the same map → `TsThrow`, on the other map → OK; `BigInt(Math.floor(Inf))` in config → `TsThrow` (no clamp) |
| P4a | `tests/pathfinding/*`, `tests/core/pathfinding/*` (water/HPA/components) | — | MOD-PATH-WATER, MOD-CC | M-NUKE-WATER-01..03 | **fork test:** fork, water-nuke in one child, query the same edge in both, compare segments (G12); segment fill after a terrain edit goes to the overlay, not `seg_shared`; components read live by an old-version chain |
| P4b | `NukeTrajectory`, `NukeSpeed`, `ReachabilityGating`, `UtilNearestTileDist`, `NationBoatRouteSafety` | `detmath/atan2` (via Parabola) | MOD-PATH-AIR, MOD-PATH-RAIL | M-NAV-05 | stale memoized stepper hits the **old** memo after a rebuild and stores misses back into it; the chain kind is fixed at slot creation; a non-`TsThrow` panic inside the chain propagates (not `NotFound`); memo LRU eviction order at a shrunk budget |
| P5 | `TradeTrainGolden`, `TradeTrainScenarios`, `LiveTradeRevenue` | `config/trainGold`, `trainSpawnRate` | MOD-PATH-RAIL | M-RAIL-01..03 | rail-grid CS key rendering round-trips the TS string order |
| P6 | `Attack`, `AttackScenarios`, `AttackLogicGolden`, `TerritoryCapture`, `FlatBinaryHeap`, `DoomsdayClockExecution`, `Disconnected`, `tests/core/executions/Spawn*` | `order/FlatBinaryHeap`, `config/attackLogic` | MOD-FLOOD, MOD-ATK-FRONT | M-SPAWN-01..05, M-ATK-01..08, M-WIN-01..04, M-CFG-DOOM-* | **the 3 iter-snapshot sites**: deleting/capturing every structure in one `PlayerExecution` tick visits all of them; two alliances expiring in the same tick both expire; merging 3 outgoing attacks on one target merges all 3 |
| P7 | `Warship`, `WarshipMultiSelection`, `WarshipPatrolTileGuard`, `ShellRandom`, `PortExecution`, `DeleteUnitExecution`, `AutoUpgrade`, `FindAndUpgradeNearestBuilding` | `config/tradeShipGold`, `unitCost`, `jssem/Math.round` (Shell) | — | M-BOAT-*, M-NAV-01..04, M-BLD-01..07, M-ECO-01..03 | each `touch()` site writes `isInCombat` (CS U equals TS after a warship engagement) |
| P8 | `tests/nukes/*`, `MissileSilo`, `MirvPrice`, `MirvBetrayal`, `AllianceAcceptNukes` | `config/samRange`, `nukeMagnitudes`, `detmath/nuke_ring_lookup` | MOD-PATH-AIR | M-NUKE-01..03, M-NUKE-MIRV-*, M-NUKE-SAM-01..06 | MIRV cancel of detached warheads; `NukeExecution.ts:504` touch |
| P9 | `AllianceExtensionExecution`, `AllianceDonation`, `CustomAllianceDuration`, `Donate`, `QuickChat`, `TribeBehavior`, `AiAttackBehavior`, `AiAttackBehaviorNukedTerritory` | `jssem/Math.round` (DonateGold) | — | M-DIP-01..07, M-DON-*, M-SOC-01..04 | a test `H: AiAttackHooks` that overrides one hook proves the base calls it through `h.` at each TS `this.` site |
| P10 | `NationAllianceBehavior`, `NationRelation`, `NationEmojiBehavior`, `BotAnnexation`, `NationOverseasTargets`, `NationRunawayLeader`, `NationBeachhead`, `NationSpawn`, `NationEarlyEconomy` | — | — | M-AI-01..03 | `super.` paths of `send_boat_attack`/`send_expansion_boat` reach the base |
| P11 | `NationNukeStrikes`, `NationNukeTargeting`, `NationNukeSamOverwhelm`, `NationCrownNukes`, `NationMIRV`, `NationMirvSams`, `NationWarshipNukeReserve` | — | — | M-AI-*, S-FFA | — |
| P12 | `NationStructureBehavior`, `NationCounterWarshipInfestation`, `NationGoldPerMinute`, `Tutorial`, `tests/core/TutorialScenario.test.ts` | — | — | M-AI-*, M-CFG-TUT-01, S-FFA | — |
| P13 | `tests/core/snapshot/*` (as the CS-equivalence oracle) | CSE1 vectors (SPEC §1.4 table), V-CSE | MOD-SNAPSHOT | the whole corpus; R3 resume | GC-every-tick L3; `fork_identity` (random fork + GC points, 200 ticks, K equal, water-nuke variant); `fork-bench` report fields |

A ported vitest is a Rust test that builds the same setup with `of-engine` (a test-only port of `tests/util/Setup.ts` lives in P1) and checks the same assertions. Where a vitest uses upstream's `TestConfig`, port that config subset **in the test only**. Production always uses `EngineConfig` (SPEC §2.2).

---

## 5. Rules that keep the split conflict-free

1. **One owner per file**, enforced by the generator. Skeleton files created at S0 belong to the package FILE_MAP assigns them to; P1 hands them over at the `s0` tag.
   - On the Rust side, every `.rs` file under `crates/of-engine/src` names its single owner in its leading `//!` block (`Owner: **P6**`). Files §1 does not list (foundations, `mod.rs` module lists, `scratch/*`, `canon/*`) are owned by whoever that header names.
   - `tests/file_owners.rs` enforces both halves: every file has a marker, and every §1 module exists as a file whose marker equals the row's package.
2. **Frozen signatures** (§3, S0.4). A package may add *private* helpers and *new* `pub(crate)` items in its own files. Changing an existing signature takes a ledger task, a P1 PR, and everyone rebasing. The `AiAttackHooks` trait and the `Ctx`/`Rcx` choice per method are frozen signatures.
3. **No cross-package `impl` blocks in foreign files.** Methods on `State` that a package owns live in that package's file as `impl State { .. }`; Rust allows multiple `impl` blocks per type in one crate. For example, P6's `exec/attack.rs` may hold `impl State { pub(crate) fn attack_logic_inputs(..) }`. Trait impls go in the implementer's file (`impl AiAttackHooks for NationExec` in P10's `exec/nation/attack.rs`).
4. **Struct fields are frozen at S0**, because they come from the schema dump. A missing field is a skeleton bug: file it to P1.
5. **`Scratch` fields are declared by P1 at S0 and filled by their owner.** Adding a scratch field is a one-line P1 PR. Never use a thread-local, `RefCell` or `static mut`.
6. **`of-det` changes go to `of-det`** (conformance workflow review), never into `of-engine` (ARCHITECTURE §12 G7).
7. **Canon impls live next to the type** and are written by the type's owner. P13 owns only the framework and the G/T/M/W/S sections. `RefVisitor` must report every handle the type holds: a missed handle is a GC use-after-free in debug builds (generation check) and a CS diff in conformance.
8. **Shared test fixtures** (map loaders, scenario runner, `Setup.ts` port) belong to P1 and P13; others only read them.
