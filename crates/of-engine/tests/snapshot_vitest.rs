//! P13 rung T0 (FILE_MAP §4): `upstream/tests/core/snapshot/CoreSnapshot.test.ts` ported as
//! the CS-equivalence oracle. Upstream snapshots a game, restores it into a fresh game and
//! asserts (1) the restored game equals the original (`diffGraphs`/`diffSnapshots`) and (2)
//! both keep ticking identically for 20 ticks. The Rust oracle is CS v1: `canon_full` of the
//! game, `canon::load::load_into` a fresh `create_state`, `canon_full` again, all 11 section
//! digests equal; then 20 ticks of both, equal again.
//!
//! The setup is `builtGame()` (`CoreSnapshot.test.ts:15-45`) on the `plains` test map with
//! `{infiniteGold, instantBuild}`, through the production `EngineConfig` (upstream's
//! `TestConfig` changes balance values, not what a snapshot must preserve).
//!
//! Both tests need the S1 bodies on their path (P3 map load / `GameMap` / `EngineConfig`,
//! P2 players and units, P1 alliances, P5 rail, P6 `WinCheck`) and every row owner's
//! `Canon`/`Uncanon`, so they are `#[ignore]`d until S2 integration.
//!
//! Owner: **P13**.

use of_engine::Game;
use of_engine::api::intents::Turn;
use of_engine::api::map_files::MapFiles;
use of_engine::api::types::{GameMapSize, PlayerInfo, PlayerType, UnitType};
use of_engine::api::{GameStartInfo, game_config::GameConfig};
use of_engine::canon::load::load_into;
use of_engine::config::EngineConfig;
use of_engine::ctx::Ctx;
use of_engine::exec::Exec;
use of_engine::exec::win_check::WinCheckExec;
use of_engine::map::MapStatics;
use of_engine::map::manifest::MapManifest;
use of_engine::scratch::Scratch;
use of_engine::state::create_state;
use of_engine::state::unit::UnitParams;
use std::path::Path;
use std::sync::Arc;

const START: &str = r#"{
  "gameID": "p13snap",
  "lobbyCreatedAt": 0,
  "config": {
    "gameMap": "Asia", "difficulty": "Medium", "donateGold": false, "donateTroops": false,
    "gameType": "Singleplayer", "gameMode": "Free For All", "gameMapSize": "Normal",
    "nations": "disabled", "bots": 0, "infiniteGold": true, "infiniteTroops": false,
    "instantBuild": true, "randomSpawn": false
  },
  "players": []
}"#;

fn plains_files(map_label: &str, size: GameMapSize) -> MapFiles {
    let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/testdata/maps/plains");
    let read = |f: &str| std::fs::read(d.join(f)).expect("plains file");
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

/// `setup(MAP, cfg)`: `createGame` on the test map, no init executions.
fn setup() -> (Game, Arc<MapStatics>, Arc<EngineConfig>) {
    let gs: GameStartInfo = serde_json::from_str(START).expect("start");
    let files = plains_files(&gs.config.game_map, gs.config.game_map_size);
    let statics = MapStatics::load(&files).expect("MapStatics::load");
    let gc: GameConfig = gs.config.clone();
    let cfg = Arc::new(EngineConfig::new(gc, false, false, false));
    let st = create_state(Vec::new(), Vec::new(), statics.clone(), cfg.clone());
    (Game::from_state(st, &gs.game_id), statics, cfg)
}

/// `builtGame()` (`CoreSnapshot.test.ts:15-45`).
fn built_game() -> (Game, Arc<MapStatics>, Arc<EngineConfig>) {
    let (mut game, statics, cfg) = setup();
    let mut sc = Scratch::new();
    let port = {
        let mut cx = Ctx::new(&mut game.state, &mut sc, None);
        let a = cx.add_player(PlayerInfo::new("alice", PlayerType::Human, Some("client_a".into()), "alice"), None);
        let b = cx.add_player(PlayerInfo::new("bob", PlayerType::Human, Some("client_b".into()), "bob"), None);
        let bot = cx.add_player(PlayerInfo::new("bot", PlayerType::Bot, None, "bot"), None);
        let r = |cx: &Ctx, x: u32, y: u32| cx.st.map.ref_(f64::from(x), f64::from(y));
        for x in 5..15 {
            for y in 5..15 {
                let t = r(&cx, x, y);
                cx.player_mut(a).conquer(t);
            }
        }
        for x in 20..30 {
            for y in 5..15 {
                let t = r(&cx, x, y);
                cx.player_mut(b).conquer(t);
            }
        }
        let (ta, tb) = (r(&cx, 10, 10), r(&cx, 25, 10));
        cx.player_mut(a).set_spawn_tile(ta);
        cx.player_mut(b).set_spawn_tile(tb);
        let t = r(&cx, 8, 8);
        cx.player_mut(a).build_unit(UnitType::City, t, UnitParams::default());
        let t = r(&cx, 22, 8);
        let port = cx.player_mut(b).build_unit(UnitType::DefensePost, t, UnitParams::default());
        let t = r(&cx, 29, 14);
        cx.player_mut(b).relinquish(t);
        cx.player_mut(a).add_gold(12345, None);
        cx.player_mut(b).add_troops(777.0);
        cx.player_mut(a).add_embargo(b, false);
        cx.player_mut(a).update_relation(b, -50.0);
        if let Some(req) = cx.create_alliance_request(a, b) {
            cx.accept_alliance_request(req);
        }
        cx.create_alliance_request(b, bot);
        let t = r(&cx, 40, 40);
        cx.set_fallout(t, true);
        cx.st.add_execution(Exec::WinCheck(WinCheckExec::new()));
        port
    };
    for i in 0..5 {
        game.step(&Turn::empty(i)).expect("tick");
    }
    Ctx::new(&mut game.state, &mut sc, None).unit_mut(port).delete(None, None);
    (game, statics, cfg)
}

/// `roundTrip(game, MAP)`: CS → `load_into` a fresh game on the same map and config.
fn round_trip(game: &Game, statics: &Arc<MapStatics>, cfg: &Arc<EngineConfig>) -> Game {
    let cs = game.canon_full().expect("canon_full");
    let base = create_state(Vec::new(), Vec::new(), statics.clone(), cfg.clone());
    load_into(base, &game.game_id, &cs).expect("load_into")
}

fn assert_same(a: &Game, b: &Game, what: &str) {
    let (da, db) = (a.canon_d().expect("D"), b.canon_d().expect("D"));
    let names: Vec<&str> = of_canon::digest::SECTIONS
        .iter()
        .filter(|s| da.sec[**s as usize] != db.sec[**s as usize])
        .map(|s| s.name())
        .collect();
    assert!(names.is_empty() && da.all == db.all, "{what}: sections {names:?} differ");
    assert_eq!(a.canon_k().expect("K"), b.canon_k().expect("K"), "{what}: K");
}

#[test]
#[ignore = "merge1: Game::from_canon stops at the missing Uncanon for Player (P2, ledger t212) and the other rows (t294)"]
fn restores_core_objects_exactly() {
    let (game, statics, cfg) = built_game();
    let restored = round_trip(&game, &statics, &cfg);
    assert_same(&game, &restored, "restore");
    // `diffSnapshots(bytes, again)`: snapshotting the restored game gives the same bytes.
    let again = round_trip(&restored, &statics, &cfg);
    assert_same(&restored, &again, "second restore");
}

#[test]
#[ignore = "merge1: Game::from_canon stops at the missing Uncanon for Player (P2, ledger t212) and the other rows (t294)"]
fn restored_game_keeps_ticking_identically() {
    let (mut game, statics, cfg) = built_game();
    let mut restored = round_trip(&game, &statics, &cfg);
    for t in 5..25 {
        game.step(&Turn::empty(t)).expect("tick");
        restored.step(&Turn::empty(t)).expect("tick");
    }
    assert_same(&game, &restored, "after 20 ticks");
}
