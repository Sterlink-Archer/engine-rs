//! t615 — city price: `price = min(1_000_000, 2^n * 125_000)`, `n = min(owned, constructed)`
//! (`EngineConfig::unit_cost` → `cost_wrapper` for `UnitType::City`).
//!
//! Expected (prints, does not assert). normal: 125k+250k+500k+1M = 1.875M.
//! sacrifice_retake: A+B = 125k+250k; A's tile goes to the nation (owned drops,
//! constructed does not) so C+D = 250k+500k; total 1.125M. Retake, then the 5th
//! quote is the 1M cap (`n = 4`, `2^4 * 125k = 2M`).
//!
//! cargo test -p of-engine --test t615_city_sacrifice -- --ignored --nocapture
//!
//! Setup, step and player reads follow `t608_embargo_relation`. `infiniteGold` stays
//! off (it zeroes `unit_cost`). Gold is `Player::add_gold`. Tile moves are
//! `Player::conquer` — direct conquest (test shortcut), same mutator as t608's
//! bridge — then one `step` so `PlayerExec` captures the city.

use of_engine::Game;
use of_engine::api::GameStartInfo;
use of_engine::api::intents::{Intent, StampedIntent, Turn};
use of_engine::api::map_files::MapFiles;
use of_engine::api::types::{PlayerType, UnitType};
use of_engine::ctx::Ctx;
use of_engine::exec::util::get_spawn_tiles;
use of_engine::ids::{SmallId, TileRef};
use of_engine::map::MapStatics;
use of_engine::map::manifest::MapManifest;
use of_engine::scratch::with_thread_scratch;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const GAME_ID: &str = "t615city";
const CLIENT: &str = "RLAG0000";
const MAP_NAME: &str = "Iceland";
const MIN_DIST_SQ: u32 = 15 * 15; // `structureMinDist()`

#[test]
#[ignore]
fn t615_city_sacrifice() {
    let gs = start_info();
    let statics = load_statics(&gs);
    println!("T615 META game_id={GAME_ID} map={MAP_NAME} size=Compact client={CLIENT}");
    for arm in ["normal", "sacrifice_retake"] {
        run_arm(arm, &gs, &statics);
    }
}

fn run_arm(arm: &str, gs: &GameStartInfo, statics: &Arc<MapStatics>) {
    let mut game = Game::new(gs, Arc::clone(statics)).expect("Game::new");
    let (me, nation) = prepare(&mut game);
    let path = claim_corridor(&mut game, me);
    give_gold(&mut game, me, 5_000_000);
    let sites = pick_sites(&game, me, &path, 4);
    let mut spent = 0i64;
    let mut pay = |game: &mut Game, city: &str, tile: TileRef| -> TileRef {
        let (owned, constructed) = counts(game, me);
        let price = quote(game, me);
        println!("T615 arm={arm} city={city} owned={owned} constructed={constructed} price={price} spent={spent}");
        let built = build_city(game, me, tile);
        spent += price;
        built
    };
    if arm == "normal" {
        for (i, &tile) in sites.iter().enumerate() {
            pay(&mut game, &format!("{}", i + 1), tile);
        }
    } else {
        let tile_a = pay(&mut game, "A", sites[0]);
        pay(&mut game, "B", sites[1]);
        // direct conquest (test shortcut): A's tile to the nation, then back.
        conquer_to(&mut game, nation, tile_a);
        step(&mut game, None);
        pay(&mut game, "C", sites[2]);
        pay(&mut game, "D", sites[3]);
        conquer_to(&mut game, me, tile_a);
        step(&mut game, None);
        let (owned, constructed) = counts(&game, me);
        println!("T615 arm={arm} fifth owned={owned} constructed={constructed} price={}", quote(&game, me));
    }
    let (owned, constructed) = counts(&game, me);
    println!("T615 SUMMARY arm={arm} spent={spent} owned={owned} constructed={constructed} next={}", quote(&game, me));
}

fn counts(game: &Game, me: SmallId) -> (i32, i32) {
    let p = game.state.player(me);
    (p.units_owned(UnitType::City), p.units_constructed(UnitType::City))
}

fn quote(game: &Game, me: SmallId) -> i64 {
    game.state.config().unit_cost(UnitType::City, &game.state, me, 0.0)
}

/// Created this step, ticked the next (`t608`). `build_unit` charges `unit_cost` then.
fn build_city(game: &mut Game, me: SmallId, tile: TileRef) -> TileRef {
    let before = city_tiles(game, me);
    step(
        game,
        Some(Intent::BuildUnit {
            unit: UnitType::City,
            tile: u64::from(tile),
            rocket_direction_up: None,
            amount: None,
        }),
    );
    step(game, None);
    let born: Vec<_> = city_tiles(game, me).into_iter().filter(|t| !before.contains(t)).collect();
    if born.len() != 1 {
        panic!("t615: build at {tile} produced {} cities (gold {})", born.len(), game.view().player(me).gold());
    }
    born[0]
}

fn city_tiles(game: &Game, me: SmallId) -> Vec<TileRef> {
    game.state.player(me).units_of(&[UnitType::City]).into_iter().map(|u| game.state.unit(u).tile()).collect()
}

fn give_gold(game: &mut Game, me: SmallId, amount: i64) {
    let st = &mut game.state;
    with_thread_scratch(|sc| Ctx::new(st, sc, None).player_mut(me).add_gold(amount, None));
}

/// direct conquest (test shortcut): `Player::conquer`, as in t608 `bridge_to_nation`.
fn conquer_to(game: &mut Game, who: SmallId, tile: TileRef) {
    let st = &mut game.state;
    with_thread_scratch(|sc| Ctx::new(st, sc, None).player_mut(who).conquer(tile));
}

/// direct conquest (test shortcut): a land corridor so four cities clear `structureMinDist`.
fn claim_corridor(game: &mut Game, me: SmallId) -> Vec<TileRef> {
    let start = game.view().player(me).spawn_tile().expect("human spawn");
    let mut best = Vec::new();
    for prefer in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
        let path = walk(game, me, start, prefer);
        if path.len() > best.len() {
            best = path;
        }
    }
    let mine = me.0;
    let st = &mut game.state;
    with_thread_scratch(|sc| {
        let mut cx = Ctx::new(st, sc, None);
        for &t in &best {
            if cx.st.map.owner_id(t) != mine {
                cx.player_mut(me).conquer(t);
            }
        }
    });
    best
}

fn walk(game: &Game, me: SmallId, start: TileRef, prefer: (i32, i32)) -> Vec<TileRef> {
    let map = &game.state.map;
    let (w, h) = (map.width() as i32, map.height() as i32);
    let dirs = [prefer, (1, 0), (-1, 0), (0, 1), (0, -1)];
    let (mut x, mut y) = (map.x(start) as i32, map.y(start) as i32);
    let mut path = vec![start];
    while path.len() < 90 {
        let step = dirs.iter().copied().find_map(|(dx, dy)| {
            let (nx, ny) = (x + dx, y + dy);
            if nx < 0 || ny < 0 || nx >= w || ny >= h {
                return None;
            }
            let t = map.ref_(f64::from(nx), f64::from(ny));
            let o = map.owner_id(t);
            ((o == 0 || o == me.0) && !path.contains(&t) && map.is_land(t) && !map.is_impassable(t))
                .then_some((nx, ny, t))
        });
        let Some((nx, ny, t)) = step else { break };
        (x, y) = (nx, ny);
        path.push(t);
    }
    path
}

fn pick_sites(game: &Game, me: SmallId, path: &[TileRef], n: usize) -> Vec<TileRef> {
    let mut chosen = Vec::new();
    with_thread_scratch(|sc| {
        for &t in path.iter().rev() {
            let far = chosen.iter().all(|&c| game.state.map.euclidean_dist_squared(c, t) >= MIN_DIST_SQ);
            if far && game.state.player(me).can_build(sc, UnitType::City, t, None) == Some(t) {
                chosen.push(t);
                if chosen.len() == n {
                    break;
                }
            }
        }
    });
    assert!(chosen.len() == n, "t615: {} city sites, wanted {n} (corridor {})", chosen.len(), path.len());
    chosen.reverse();
    chosen
}

fn prepare(game: &mut Game) -> (SmallId, SmallId) {
    let me = game.view().player_by_client_id(CLIENT).expect("human");
    let mut sent = None;
    let mut guard = 0i32;
    while game.view().in_spawn_phase() && guard < 5000 {
        let tick = game.state.ticks;
        let intent = if game.view().player(me).has_spawned() || (sent.is_some() && tick < 190) {
            None
        } else {
            open_spawn(game).map(|t| {
                sent = Some(t);
                Intent::Spawn { tile: u64::from(t) }
            })
        };
        step(game, intent);
        guard += 1;
    }
    if game.view().in_spawn_phase() {
        panic!("t615: spawn phase did not end within {guard} ticks");
    }
    let nation = find_one(game, PlayerType::Nation).unwrap_or_else(|| panic!("t615: no nation"));
    if !game.view().player(me).has_spawned() {
        panic!("t615: human did not spawn (last tile {sent:?})");
    }
    if !game.view().player(nation).has_spawned() {
        panic!("t615: nation did not spawn");
    }
    (me, nation)
}

fn open_spawn(game: &Game) -> Option<TileRef> {
    let nation = find_one(game, PlayerType::Nation)?;
    let origin = game.view().player(nation).spawn_tile()?;
    let map = &game.state.map;
    let (ox, oy) = (map.x(origin) as i32, map.y(origin) as i32);
    let (w, h) = (map.width() as i32, map.height() as i32);
    (8i32..60).find_map(|r: i32| {
        (-r..=r).find_map(|dy: i32| {
            (-r..=r).find_map(|dx: i32| {
                if dx.abs() != r && dy.abs() != r {
                    return None;
                }
                let (x, y) = (ox + dx, oy + dy);
                if x < 0 || y < 0 || x >= w || y >= h {
                    return None;
                }
                let t = map.ref_(f64::from(x), f64::from(y));
                (map.is_land(t)
                    && !map.is_impassable(t)
                    && !map.has_owner(t)
                    && get_spawn_tiles(map, t, false).is_some_and(|d| !d.is_empty()))
                .then_some(t)
            })
        })
    })
}

fn find_one(game: &Game, ty: PlayerType) -> Option<SmallId> {
    let v = game.view();
    v.all_players().find(|&p| v.player_type(p) == ty)
}

fn step(game: &mut Game, intent: Option<Intent>) {
    let intents = intent.into_iter().map(|intent| StampedIntent { client_id: CLIENT.to_string(), intent }).collect();
    game.step(&Turn { turn_number: game.state.ticks as u64, intents, hash: None }).expect("step");
}

fn start_info() -> GameStartInfo {
    serde_json::from_str(&format!(
        r#"{{"gameID":"{GAME_ID}","lobbyCreatedAt":0,"config":{{"gameMap":"{MAP_NAME}","difficulty":"Hard","donateGold":false,"donateTroops":false,"gameType":"Public","gameMode":"Free For All","gameMapSize":"Compact","nations":1,"bots":4,"infiniteGold":false,"infiniteTroops":false,"instantBuild":false,"randomSpawn":false,"spawnImmunityDuration":0,"disabledUnits":[]}},"players":[{{"clientID":"{CLIENT}","username":"agent0","clanTag":null}}]}}"#
    ))
    .expect("GameStartInfo")
}

fn load_statics(gs: &GameStartInfo) -> Arc<MapStatics> {
    let c = &gs.config;
    let key: String = c.game_map.to_lowercase().chars().filter(|ch| ch.is_ascii_alphanumeric()).collect();
    let dir = map_dir(&key);
    let read = |f: &str| std::fs::read(dir.join(f)).unwrap_or_else(|e| panic!("t615: {}: {e}", dir.join(f).display()));
    let mut mj: serde_json::Value = serde_json::from_slice(&read("manifest.json")).expect("manifest json");
    if let Some(o) = mj.as_object_mut() {
        o.entry("nations").or_insert_with(|| serde_json::Value::Array(Vec::new()));
    }
    let files = MapFiles {
        map: c.game_map.clone(),
        map_size: c.game_map_size,
        manifest: serde_json::from_value::<MapManifest>(mj).expect("manifest"),
        map_bin: None,
        map4x_bin: Some(read("map4x.bin")),
        map16x_bin: Some(read("map16x.bin")),
    };
    of_engine::runner::check_map_files(gs, &files).expect("check_map_files");
    MapStatics::load(&files).unwrap_or_else(|e| panic!("MapStatics::load: {e}"))
}

fn map_dir(key: &str) -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut roots = vec![manifest.join("../../../upstream"), manifest.join("../../../../upstream")];
    if let Some(d) = std::env::var_os("OF_UPSTREAM_DIR") {
        roots.insert(0, PathBuf::from(d));
    }
    for root in &roots {
        for rel in ["resources/maps", "packages/resources/maps"] {
            let dir = root.join(rel).join(key);
            if dir.join("manifest.json").is_file() {
                return dir;
            }
        }
    }
    panic!("t615: {MAP_NAME} Compact not found. Set OF_UPSTREAM_DIR (roots {roots:?})");
}
