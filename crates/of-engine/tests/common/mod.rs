//! Port of upstream `tests/util/Setup.ts` (+ the `executeTicks`/`playerInfo` helpers) for the
//! ported vitests. Owner: **P1** (FILE_MAP §4/§5 rule 8: the `Setup.ts` port lives in P1).
//!
//! `setup(map, patch, humans, auto_end_spawn_phase, nations)` loads `tests/testdata/maps/<map>`
//! (copies of upstream `tests/testdata/maps`, checked by `SHA256SUMS`), builds the
//! `GameConfig` exactly as `Setup.ts:55-70` does (defaults, then the patch), and calls
//! `createGame` (`create_state`) without `GameRunner.init()`.
//!
//! **TestConfig.** Upstream builds the game with `TestConfig` (`tests/util/TestConfig.ts`).
//! Production Rust has only `EngineConfig` (SPEC §2.2), so the fixture reproduces the one
//! `TestConfig` override that is a `GameConfig` default, `disableNavMesh() = gameConfig()
//! .disableNavMesh ?? true`, by defaulting the key to `true`. The others (spawn immunity 0,
//! flat nuke magnitudes/speeds, `attackLogic` = 1/1/1, …) are not modelled; each ported test
//! that depends on one says so (ledger: request in `docs/requests/P1.md`).
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use of_engine::Game;
use of_engine::api::game_config::GameConfig;
use of_engine::api::intents::Turn;
use of_engine::api::map_files::MapFiles;
use of_engine::api::types::{GameMapSize, Nation, PlayerInfo, PlayerType};
use of_engine::config::EngineConfig;
use of_engine::ctx::Ctx;
use of_engine::exec::Exec;
use of_engine::ids::{SmallId, TileRef};
use of_engine::map::MapStatics;
use of_engine::map::manifest::MapManifest;
use of_engine::scratch::Scratch;
use of_engine::state::create_state;
use std::path::Path;
use std::sync::Arc;

/// A game under test plus its scratch (TS: the `Game` object).
pub struct TestGame {
    pub g: Game,
    pub sc: Scratch,
    turn: u64,
}

impl TestGame {
    /// A `Ctx` for direct state calls (`player.conquer(..)`, `game.conquerPlayer(..)`).
    pub fn cx(&mut self) -> Ctx<'_> {
        Ctx::new(&mut self.g.state, &mut self.sc, None)
    }
    /// `game.addExecution(exec)`: into `unInitExecs` (via the inbox drain).
    pub fn add_execution(&mut self, e: Exec) {
        self.g.state.add_execution(e);
        self.g.execs.drain_inbox(&mut self.g.state);
    }
    /// `game.executeNextTick()` (no intents).
    pub fn tick(&mut self) {
        let t = Turn::empty(self.turn);
        self.turn += 1;
        self.g.step_with(&t, &mut self.sc, None).expect("executeNextTick");
    }
    /// `executeTicks(game, n)` (`tests/util/utils.ts`).
    pub fn ticks(&mut self, n: usize) {
        for _ in 0..n {
            self.tick();
        }
    }
    /// `game.ref(x, y)`.
    pub fn r(&self, x: u32, y: u32) -> TileRef {
        self.g.state.map.ref_(f64::from(x), f64::from(y))
    }
    /// `game.player(id)`.
    pub fn p(&self, id: &str) -> SmallId {
        self.g.state.player_by_id(id)
    }
}

/// `playerInfo(name, type)` (`Setup.ts:82-84`): `new PlayerInfo(name, type, null, name)`.
pub fn player_info(name: &str, t: PlayerType) -> PlayerInfo {
    PlayerInfo::new(name, t, None, name)
}

/// `tests/testdata/maps/<map>` as `MapFiles` (`Setup.ts:36-53`).
pub fn map_files(map: &str) -> MapFiles {
    let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/testdata/maps").join(map);
    let read = |f: &str| std::fs::read(d.join(f)).unwrap_or_else(|e| panic!("{map}/{f}: {e}"));
    // Test manifests carry no `nations` key (Setup.ts passes `nations` separately).
    let mut mj: serde_json::Value = serde_json::from_slice(&read("manifest.json")).unwrap();
    if let Some(o) = mj.as_object_mut() {
        o.entry("nations").or_insert_with(|| serde_json::Value::Array(Vec::new()));
    }
    let manifest: MapManifest = serde_json::from_value(mj).unwrap();
    MapFiles {
        map: "Asia".to_owned(),
        map_size: GameMapSize::Normal,
        manifest,
        map_bin: Some(read("map.bin")),
        map4x_bin: Some(read("map4x.bin")),
        map16x_bin: Some(read("map16x.bin")),
    }
}

/// `Setup.ts:55-70`: the default `GameConfig`, then `..._gameConfig`.
pub fn game_config(patch: serde_json::Value) -> GameConfig {
    let mut base = serde_json::json!({
        "gameMap": "Asia",
        "gameMapSize": "Normal",
        "gameMode": "Free For All",
        "gameType": "Singleplayer",
        "difficulty": "Medium",
        "nations": "default",
        "donateGold": false,
        "donateTroops": false,
        "bots": 0,
        "infiniteGold": false,
        "infiniteTroops": false,
        "instantBuild": false,
        "randomSpawn": false,
        // TestConfig.disableNavMesh() = `gameConfig().disableNavMesh ?? true` (module docs).
        "disableNavMesh": true,
    });
    let (Some(b), Some(p)) = (base.as_object_mut(), patch.as_object()) else { panic!("patch must be an object") };
    for (k, v) in p {
        b.insert(k.clone(), v.clone());
    }
    serde_json::from_value(base).expect("GameConfig")
}

/// `setup(mapName, gameConfig, humans, _, _, autoEndSpawnPhase, nations)` (`Setup.ts:24-80`).
pub fn setup_full(
    map: &str,
    patch: serde_json::Value,
    humans: Vec<PlayerInfo>,
    auto_end_spawn_phase: bool,
    nations: Vec<Nation>,
) -> TestGame {
    let statics = MapStatics::load(&map_files(map)).expect("MapStatics::load");
    // `new ConfigClass(gameConfig, false)`: isReplay false, listed/spectator default false.
    let cfg = Arc::new(EngineConfig::new(game_config(patch), false, false, false));
    let state = create_state(humans, nations, statics, cfg);
    let mut tg = TestGame { g: Game::from_state(state, "game_id"), sc: Scratch::new(), turn: 0 };
    if auto_end_spawn_phase {
        tg.cx().end_spawn_phase();
    }
    tg
}

/// `setup(map, gameConfig, humans)` with the defaults (spawn phase ended, no nations).
pub fn setup(map: &str, patch: serde_json::Value, humans: Vec<PlayerInfo>) -> TestGame {
    setup_full(map, patch, humans, true, Vec::new())
}
