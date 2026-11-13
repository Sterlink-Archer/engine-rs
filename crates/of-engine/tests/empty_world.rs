//! S0 empty-world smoke test (FILE_MAP §3 S0 step 5): a 0-player Singleplayer FFA game on the
//! `plains` test map steps 100 ticks.
//!
//! For this config `GameRunner.init` (`GameRunner.ts:183-207`) adds exactly `WinCheck` and
//! `RecomputeRailCluster` (no SpawnTimer in Singleplayer, nations disabled, no random spawn,
//! no bots, no doomsday clock, Factory enabled). Neither is active during the spawn phase,
//! and with no SpawnTimer and no player nothing ends it, so in TS both stay in `unInitExecs`
//! forever, `execs` stays empty and only `_ticks` advances.
//!
//! - `empty_world_steps_100_ticks` runs the real tick loop on that execution set now (the init
//!   executions are built as values; their constructors are S1 work).
//! - `game_new_plains_empty_world` is the same through `Game::new` + `MapStatics::load` (it was
//!   `#[ignore]`d until the S1 constructors landed; runs by default since merge1).

use of_engine::Game;
use of_engine::api::intents::Turn;
use of_engine::api::map_files::MapFiles;
use of_engine::api::types::GameMapSize;
use of_engine::api::{GameStartInfo, game_config::GameConfig};
use of_engine::exec::Exec;
use of_engine::exec::rail_cluster::RecomputeRailClusterExec;
use of_engine::exec::win_check::WinCheckExec;
use of_engine::ids::ExecKind;
use of_engine::map::MapStatics;
use of_engine::map::manifest::MapManifest;
use of_engine::state::State;
use std::path::Path;

const START: &str = r#"{
  "gameID": "s0empty1",
  "lobbyCreatedAt": 0,
  "config": {
    "gameMap": "Asia", "difficulty": "Medium", "donateGold": false, "donateTroops": false,
    "gameType": "Singleplayer", "gameMode": "Free For All", "gameMapSize": "Normal",
    "nations": "disabled", "bots": 0, "infiniteGold": false, "infiniteTroops": false,
    "instantBuild": false, "randomSpawn": false
  },
  "players": []
}"#;

fn start() -> GameStartInfo {
    serde_json::from_str(START).expect("GameStartInfo")
}

fn uninit_kinds(g: &Game) -> Vec<ExecKind> {
    g.state.uninit.iter().map(|id| id.kind).collect()
}

#[test]
fn empty_world_steps_100_ticks() {
    let gs = start();
    let cfg: &GameConfig = &gs.config;
    assert!(gs.players.is_empty() && cfg.bots == 0 && !cfg.random_spawn);
    let mut g = Game::from_state(State::default(), &gs.game_id);
    assert!(g.state.in_spawn_phase());
    let init = [
        Exec::WinCheck(WinCheckExec { active: true, initialized: false, checked_ranked_spawns: false }),
        Exec::RecomputeRailCluster(RecomputeRailClusterExec {}),
    ];
    let ev = g.step_with_execs(&Turn::empty(0), init).expect("tick 0");
    assert_eq!(ev.tick, 1);
    let mut fps = vec![g.fingerprint64()];
    for t in 1..100u64 {
        let ev = g.step(&Turn::empty(t)).expect("tick");
        assert_eq!(ev.tick, t as i32 + 1);
        assert!(ev.winner_set.is_none() && ev.killed.is_empty());
        fps.push(g.fingerprint64());
    }
    assert_eq!(g.state.ticks, 100);
    assert!(g.state.in_spawn_phase());
    assert!(g.execs.live.is_empty());
    assert_eq!(uninit_kinds(&g), [ExecKind::WinCheck, ExecKind::RecomputeRailCluster]);
    assert!(g.stuck.is_none());
    // Only `_ticks` changes, so every fingerprint differs from the previous one.
    assert!(fps.windows(2).all(|w| w[0] != w[1]));
}

fn plains_files(map_label: &str, size: GameMapSize) -> MapFiles {
    let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/testdata/maps/plains");
    let read = |f: &str| std::fs::read(d.join(f)).expect("plains file");
    // Test-map manifests carry no `nations` key (upstream `tests/util/Setup.ts` passes the
    // nation list separately, `nations = []`); give the typed manifest that empty list.
    let mut mj: serde_json::Value = serde_json::from_slice(&read("manifest.json")).expect("manifest json");
    if let Some(o) = mj.as_object_mut() {
        o.entry("nations").or_insert_with(|| serde_json::Value::Array(Vec::new()));
    }
    let manifest: MapManifest = serde_json::from_value(mj).expect("manifest");
    MapFiles {
        map: map_label.to_owned(),
        map_size: size,
        manifest,
        map_bin: Some(read("map.bin")),
        map4x_bin: Some(read("map4x.bin")),
        map16x_bin: Some(read("map16x.bin")),
    }
}

#[test]
fn plains_test_data_parses() {
    let gs = start();
    let files = plains_files(&gs.config.game_map, gs.config.game_map_size);
    of_engine::runner::check_map_files(&gs, &files).expect("files match the config label");
    assert_eq!(files.map_bin.as_ref().map(Vec::len), Some(100 * 100));
    assert_eq!(files.map4x_bin.as_ref().map(Vec::len), Some(50 * 50));
    let wrong = plains_files(&gs.config.game_map, GameMapSize::Compact);
    assert!(of_engine::runner::check_map_files(&gs, &wrong).is_err());
}

#[test]
fn game_new_plains_empty_world() {
    let gs = start();
    let files = plains_files(&gs.config.game_map, gs.config.game_map_size);
    of_engine::runner::check_map_files(&gs, &files).expect("files");
    let statics = MapStatics::load(&files).expect("MapStatics::load");
    let mut g = Game::new(&gs, statics).expect("Game::new");
    assert_eq!(uninit_kinds(&g), [ExecKind::WinCheck, ExecKind::RecomputeRailCluster]);
    for t in 0..100u64 {
        g.step(&Turn::empty(t)).expect("tick");
    }
    assert_eq!(g.state.ticks, 100);
    assert!(g.state.in_spawn_phase() && g.execs.live.is_empty() && g.state.players.is_empty());
    assert_eq!(uninit_kinds(&g), [ExecKind::WinCheck, ExecKind::RecomputeRailCluster]);
}
