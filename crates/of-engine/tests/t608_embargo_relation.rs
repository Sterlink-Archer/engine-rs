//! t608 — embargo vs nation relation, one seed, five arms. Observational: it prints, it does
//! not assert H1/H2/H3. `#[ignore]` so a normal `cargo test` skips it.
//!
//! ```text
//! cargo test -p of-engine --test t608_embargo_relation -- --ignored --nocapture
//! ```
//!
//! # Hypotheses
//!
//! - **(H1)** the toggle is free (start legal again right after stop) and changes nothing on
//!   tribes.
//! - **(H2)** hold-then-stop nets a POSITIVE relation change (−20 at the nation's attack tick,
//!   decay 0.05/tick toward 0, +20 refund on stop).
//! - **(H3)** from −40 the embargo flips the nation's target.
//!
//! What a reading of the engine says those lines should show (this file does not enforce it):
//!
//! - H1, engine half. `EmbargoExec::tick` (`exec/embargo.rs`) is `add_embargo` / `stop_embargo`
//!   with no cooldown. After Stop, `has_manual_embargo_against` is false, so the next Start
//!   takes effect again. The v3 mask that makes Start/Stop a free toggle lives in
//!   the research harness's mask code, not in this crate; this binary only shows the engine accepts the
//!   re-start. A tribe has no ports and no embargo reader (`exec/tribe`), so `toggle_tribe`
//!   should leave the nation's relation, and both tile and gold series, on the control path.
//! - H2, timing. The −20 and the +20 are `NationExec::update_relations_from_embargos`
//!   (`exec/nation/mod.rs`): manual only, once, and only on the nation's attack tick
//!   (`tick % attack_rate == attack_tick`, Hard rate 45–60). They do not land on the embargo
//!   tick. Decay is `Player::decay_relations` (`state/player.rs`), 0.05 toward 0 every tick.
//!   Stop does not refund on the stop tick; the +20 is the same function's else-branch on the
//!   next attack tick after the manual flag is gone. From 0, −20 is Distrustful, not Hostile
//!   (`relation_from_value`: Hostile < −50).
//! - H3. `TargetPlayerExec` (`exec/target_player.rs`) applies −40 to the target's relation
//!   toward the requestor when `can_target` passes (`state/player.rs`: not self, not friendly,
//!   no mark inside the target cooldown; no shared border required). −40 + −20 = −60, which
//!   crosses Hostile, so Hard's Hated strategy can change `outgoing_attacks` within one attack
//!   cycle. An attack is not used to preload: `AttackExec` also plants a temporary embargo and
//!   a Hard −80, which already crosses Hostile before the embargo malus.
//!
//! Intents are created at the start of `Game::step` and ticked on the next step (init pass is
//! after the tick pass in `runner`). EmbargoStart sent when `ticks == t0` is visible at `t0 + 1`.
//! EmbargoStop sent at `t0 + 400` is visible at `t0 + 401`. The preload mark is two steps before
//! `t0`, so the −40 has already been applied when the window opens.
//!
//! Map load is the research harness's map loader: Compact = `map4x.bin` + `map16x.bin`.
//! Spawn / step / conquer follow the research harness's test helpers.
//! No `Attack` intent is sent. If the spawn disks (Euclidean radius 4, `exec/util.rs`) do not
//! share a border after the spawn phase, the gap is closed with `Player::conquer` — the same
//! test-setup mutator as `G::give` — before `t0`, identical on every arm.

use of_engine::Game;
use of_engine::api::GameStartInfo;
use of_engine::api::intents::{EmbargoAction, Intent, StampedIntent, Turn};
use of_engine::api::map_files::MapFiles;
use of_engine::api::types::{GameMapSize, PlayerType};
use of_engine::ctx::Ctx;
use of_engine::exec::util::get_spawn_tiles;
use of_engine::ids::{SmallId, TileRef};
use of_engine::map::MapStatics;
use of_engine::map::game_map::GameMap;
use of_engine::map::manifest::MapManifest;
use of_engine::scratch::with_thread_scratch;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const GAME_ID: &str = "t608embargo";
const CLIENT: &str = "RLAG0000";
const WINDOW: i32 = 600;
const HOLD_STOP: i32 = 400;
const TOGGLE_EVERY: i32 = 10;
const MAP_NAME: &str = "Iceland";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Control,
    Hold400,
    Toggle10,
    ToggleTribe,
    PreloadM40,
}

struct Arm {
    name: &'static str,
    kind: Kind,
}

const ARMS: [Arm; 5] = [
    Arm { name: "control", kind: Kind::Control },
    Arm { name: "hold400", kind: Kind::Hold400 },
    Arm { name: "toggle10", kind: Kind::Toggle10 },
    Arm { name: "toggle_tribe", kind: Kind::ToggleTribe },
    Arm { name: "preload_m40", kind: Kind::PreloadM40 },
];

struct End {
    name: &'static str,
    t0: i32,
    rel_t0: f64,
    rel_min: f64,
    rel_at_400: f64,
    rel_end: f64,
    jump1_tick: i32,
    jump1_rel: f64,
    jump2_tick: i32,
    jump2_rel: f64,
    tiles_us_t0: u32,
    tiles_us_end: u32,
    tiles_nat_t0: u32,
    tiles_nat_end: u32,
    gold_us_t0: i64,
    gold_us_end: i64,
    gold_nat_t0: i64,
    gold_nat_end: i64,
    emb_flips: u32,
    crossed_hostile: u8,
    friendly_ever: u8,
    targets: String,
}

#[test]
#[ignore]
fn t608_embargo_relation() {
    let gs = start_info();
    let statics = load_statics(&gs);
    println!(
        "T608 META game_id={GAME_ID} map={MAP_NAME} size=Compact difficulty=Hard nations=1 bots=4 client={CLIENT} window={WINDOW}"
    );
    let mut ends = Vec::with_capacity(ARMS.len());
    for arm in &ARMS {
        ends.push(run_arm(arm, &gs, &statics));
    }
    println!("T608 COMPARE arms={}", ends.len());
    for e in &ends {
        println!(
            "T608 COMPARE arm={} t0={} rel_t0={} rel_min={} rel_at_400={} rel_end={} jump1_tick={} jump1_rel={} jump2_tick={} jump2_rel={} d_tiles_us={} d_tiles_nat={} d_gold_us={} d_gold_nat={} emb_flips={} crossed_hostile={} friendly_ever={} targets={}",
            e.name,
            e.t0,
            e.rel_t0,
            e.rel_min,
            e.rel_at_400,
            e.rel_end,
            e.jump1_tick,
            e.jump1_rel,
            e.jump2_tick,
            e.jump2_rel,
            e.tiles_us_end as i64 - e.tiles_us_t0 as i64,
            e.tiles_nat_end as i64 - e.tiles_nat_t0 as i64,
            e.gold_us_end - e.gold_us_t0,
            e.gold_nat_end - e.gold_nat_t0,
            e.emb_flips,
            e.crossed_hostile,
            e.friendly_ever,
            e.targets
        );
    }
}

fn run_arm(arm: &Arm, gs: &GameStartInfo, statics: &Arc<MapStatics>) -> End {
    let mut game = Game::new(gs, Arc::clone(statics)).expect("Game::new");
    let (me, nation, tribe, bridge) = prepare(&mut game);
    let nation_id = game.view().player_id(nation).to_string();
    let tribe_id = game.view().player_id(tribe).to_string();
    // Two steps so a TargetPlayer sent on the first is ticked on the second, before t0.
    let mark =
        if arm.kind == Kind::PreloadM40 { Some(Intent::TargetPlayer { target: nation_id.clone() }) } else { None };
    step(&mut game, mark);
    step(&mut game, None);

    let t0 = game.state.ticks;
    println!(
        "T608 ARM arm={} t0={t0} nation_id={nation_id} tribe_id={tribe_id} bridge_tiles={bridge} border={} rel={}",
        arm.name,
        u8::from(game.view().player(me).shares_border_with(nation)),
        game.view().relation_value(nation, me)
    );

    let mut end = End {
        name: arm.name,
        t0,
        rel_t0: game.view().relation_value(nation, me),
        rel_min: game.view().relation_value(nation, me),
        rel_at_400: 0.0,
        rel_end: 0.0,
        jump1_tick: -1,
        jump1_rel: 0.0,
        jump2_tick: -1,
        jump2_rel: 0.0,
        tiles_us_t0: game.view().player(me).num_tiles_owned(),
        tiles_us_end: 0,
        tiles_nat_t0: game.view().player(nation).num_tiles_owned(),
        tiles_nat_end: 0,
        gold_us_t0: game.view().player(me).gold(),
        gold_us_end: 0,
        gold_nat_t0: game.view().player(nation).gold(),
        gold_nat_end: 0,
        emb_flips: 0,
        crossed_hostile: u8::from(game.view().relation_value(nation, me) < -50.0),
        friendly_ever: u8::from(game.view().player(me).is_friendly(nation, false)),
        targets: String::new(),
    };
    let mut seen_targets: Vec<String> = Vec::new();
    let mut prev_rel = end.rel_t0;
    // Flips follow the arm's target: the tribe on toggle_tribe, the nation on the others.
    let flip_at = if arm.kind == Kind::ToggleTribe { tribe } else { nation };
    let mut prev_manual = game.view().player(me).has_manual_embargo_against(flip_at);
    note_targets(&game, nation, &mut seen_targets);
    emit(&game, arm.name, me, nation, tribe);

    for _ in 0..WINDOW {
        let k = game.state.ticks - t0;
        if k == HOLD_STOP {
            end.rel_at_400 = game.view().relation_value(nation, me);
        }
        let intent = window_intent(arm.kind, k, &nation_id, &tribe_id);
        step(&mut game, intent);

        let rel = game.view().relation_value(nation, me);
        let manual = game.view().player(me).has_manual_embargo_against(flip_at);
        if manual != prev_manual {
            end.emb_flips += 1;
            prev_manual = manual;
        }
        let delta = rel - prev_rel;
        #[allow(clippy::manual_range_contains)] // NaN-safe: both comparisons are false for NaN
        if delta > 1.0 || delta < -1.0 {
            let tick = game.state.ticks;
            if end.jump1_tick < 0 {
                end.jump1_tick = tick;
                end.jump1_rel = rel;
            } else if k >= HOLD_STOP && end.jump2_tick < 0 {
                end.jump2_tick = tick;
                end.jump2_rel = rel;
            }
        }
        prev_rel = rel;
        if rel < end.rel_min {
            end.rel_min = rel;
        }
        if rel < -50.0 {
            end.crossed_hostile = 1;
        }
        if game.view().player(me).is_friendly(nation, false) {
            end.friendly_ever = 1;
        }
        note_targets(&game, nation, &mut seen_targets);
        if (game.state.ticks - t0) % TOGGLE_EVERY == 0 {
            emit(&game, arm.name, me, nation, tribe);
        }
    }

    end.rel_end = game.view().relation_value(nation, me);
    end.tiles_us_end = game.view().player(me).num_tiles_owned();
    end.tiles_nat_end = game.view().player(nation).num_tiles_owned();
    end.gold_us_end = game.view().player(me).gold();
    end.gold_nat_end = game.view().player(nation).gold();
    end.targets = if seen_targets.is_empty() { "-".to_string() } else { seen_targets.join(",") };
    println!(
        "T608 SUMMARY arm={} t0={} rel_t0={} rel_min={} rel_at_400={} rel_end={} jump1_tick={} jump1_rel={} jump2_tick={} jump2_rel={} tiles_us={}->{} tiles_nat={}->{} gold_us={}->{} gold_nat={}->{} emb_flips={} crossed_hostile={} friendly_ever={} targets={}",
        end.name,
        end.t0,
        end.rel_t0,
        end.rel_min,
        end.rel_at_400,
        end.rel_end,
        end.jump1_tick,
        end.jump1_rel,
        end.jump2_tick,
        end.jump2_rel,
        end.tiles_us_t0,
        end.tiles_us_end,
        end.tiles_nat_t0,
        end.tiles_nat_end,
        end.gold_us_t0,
        end.gold_us_end,
        end.gold_nat_t0,
        end.gold_nat_end,
        end.emb_flips,
        end.crossed_hostile,
        end.friendly_ever,
        end.targets
    );
    end
}

/// Embargo script for the step that starts at `t0 + k`. Effect lands on the next tick.
fn window_intent(kind: Kind, k: i32, nation_id: &str, tribe_id: &str) -> Option<Intent> {
    match kind {
        Kind::Control => None,
        Kind::Hold400 | Kind::PreloadM40 => {
            if k == 0 {
                Some(embargo(nation_id, EmbargoAction::Start))
            } else if k == HOLD_STOP {
                Some(embargo(nation_id, EmbargoAction::Stop))
            } else {
                None
            }
        }
        Kind::Toggle10 => toggle(nation_id, k),
        Kind::ToggleTribe => toggle(tribe_id, k),
    }
}

fn embargo(id: &str, action: EmbargoAction) -> Intent {
    Intent::Embargo { target_id: id.to_string(), action }
}

fn toggle(id: &str, k: i32) -> Option<Intent> {
    if k < 0 || k % TOGGLE_EVERY != 0 {
        return None;
    }
    let action = if (k / TOGGLE_EVERY) % 2 == 0 { EmbargoAction::Start } else { EmbargoAction::Stop };
    Some(embargo(id, action))
}

fn emit(game: &Game, arm: &str, me: SmallId, nation: SmallId, tribe: SmallId) {
    let tick = game.state.ticks;
    let rel = game.view().relation_value(nation, me);
    let tiles_us = game.view().player(me).num_tiles_owned();
    let tiles_nat = game.view().player(nation).num_tiles_owned();
    let gold_us = game.view().player(me).gold();
    let gold_nat = game.view().player(nation).gold();
    let emb_us = u8::from(game.view().player(me).has_embargo_against(nation));
    let emb_nat = u8::from(game.view().player(nation).has_embargo_against(me));
    let emb_us_manual = u8::from(game.view().player(me).has_manual_embargo_against(nation));
    let emb_nat_manual = u8::from(game.view().player(nation).has_manual_embargo_against(me));
    let emb_tribe = u8::from(game.view().player(me).has_embargo_against(tribe));
    let emb_tribe_manual = u8::from(game.view().player(me).has_manual_embargo_against(tribe));
    let friendly = u8::from(game.view().player(me).is_friendly(nation, false));
    let targets = attack_targets(game, nation);
    println!(
        "T608 arm={arm} tick={tick} rel={rel} tiles_us={tiles_us} tiles_nat={tiles_nat} gold_us={gold_us} gold_nat={gold_nat} emb_us={emb_us} emb_nat={emb_nat} emb_us_manual={emb_us_manual} emb_nat_manual={emb_nat_manual} emb_tribe={emb_tribe} emb_tribe_manual={emb_tribe_manual} friendly={friendly} targets={targets}"
    );
}

fn attack_targets(game: &Game, nation: SmallId) -> String {
    let refs: Vec<_> = game.view().player(nation).outgoing_attacks().as_slice().to_vec();
    let mut parts: Vec<String> = Vec::new();
    for a in refs {
        let owner = game.state.attack(a).target();
        let id = if owner.is_player() { game.view().player_id(owner).to_string() } else { "tn".to_string() };
        if !parts.iter().any(|p| p == &id) {
            parts.push(id);
        }
    }
    if parts.is_empty() { "-".to_string() } else { parts.join(",") }
}

fn note_targets(game: &Game, nation: SmallId, seen: &mut Vec<String>) {
    let now = attack_targets(game, nation);
    if now == "-" {
        return;
    }
    for id in now.split(',') {
        if !seen.iter().any(|s| s == id) {
            seen.push(id.to_string());
        }
    }
}

/// Spawn the human next to the nation, then wait out the spawn phase. Returns the bridge
/// length (0 when the spawn disks already touched).
fn prepare(game: &mut Game) -> (SmallId, SmallId, SmallId, u32) {
    let me = game.view().player_by_client_id(CLIENT).expect("human");
    let mut last_sent: Option<u64> = None;
    let mut guard = 0i32;
    while game.view().in_spawn_phase() && guard < 5000 {
        let tick = game.state.ticks;
        let nation = find_one(game, PlayerType::Nation);
        let mut tile: Option<u64> = None;
        let spawned = match nation {
            Some(n) => game.view().player(n).has_spawned(),
            None => false,
        };
        if spawned {
            let n = nation.expect("spawned nation");
            let origin = game.view().player(n).spawn_tile();
            tile = adjacent_spawn(game, n).map(u64::from);
            if tile.is_none()
                && let Some(o) = origin
            {
                tile = Some(u64::from(nearest_land(game, o)));
            }
        }
        if tile.is_none() && tick == 1 {
            let cell = game.state.nations.first().and_then(|n| n.spawn_cell);
            if let Some(cell) = cell
                && let Some(center) = game.state.map.try_ref(cell.x, cell.y)
            {
                tile = Some(u64::from(nearest_land(game, center)));
            }
        }
        let send = match tile {
            Some(t) if last_sent != Some(t) || tick >= 190 => {
                last_sent = Some(t);
                true
            }
            _ => false,
        };
        let intent = if send { tile.map(|t| Intent::Spawn { tile: t }) } else { None };
        step(game, intent);
        guard += 1;
    }
    if game.view().in_spawn_phase() {
        panic!("t608: spawn phase did not end within {guard} ticks");
    }
    let nation = find_one(game, PlayerType::Nation).unwrap_or_else(|| panic!("t608: no nation player"));
    let tribe = find_one(game, PlayerType::Bot).unwrap_or_else(|| panic!("t608: no tribe player"));
    let human_spawned = game.view().player(me).has_spawned();
    let nation_spawned = game.view().player(nation).has_spawned();
    let nation_pid = game.view().player_id(nation).to_string();
    if !human_spawned {
        panic!("t608: human did not spawn (last tile {last_sent:?})");
    }
    if !nation_spawned {
        panic!("t608: nation {nation_pid} did not spawn");
    }
    let bridge = bridge_to_nation(game, me, nation);
    let bordered = game.view().player(me).shares_border_with(nation);
    if !bordered {
        panic!("t608: no shared border with nation {nation_pid} after spawn and bridge ({bridge} tiles)");
    }
    (me, nation, tribe, bridge)
}

/// A spawn center whose disk (radius 4) 4-neighbors the nation. Rings 6..=18.
fn adjacent_spawn(game: &Game, nation: SmallId) -> Option<TileRef> {
    let origin = game.view().player(nation).spawn_tile()?;
    let map = &game.state.map;
    let (x, y) = (map.x(origin), map.y(origin));
    let nid = nation.0;
    for r in 6i64..=18 {
        for dy in -r..=r {
            for dx in -r..=r {
                if dx.abs() != r && dy.abs() != r {
                    continue;
                }
                let (xx, yy) = (i64::from(x) + dx, i64::from(y) + dy);
                if xx < 0 || yy < 0 || xx >= i64::from(map.width()) || yy >= i64::from(map.height()) {
                    continue;
                }
                let t = map.ref_(xx as f64, yy as f64);
                if !map.is_land(t) || map.is_impassable(t) || map.has_owner(t) {
                    continue;
                }
                let Some(disk) = get_spawn_tiles(map, t, false) else { continue };
                if disk.is_empty() {
                    continue;
                }
                if disk_touches(map, &disk, nid) {
                    return Some(t);
                }
            }
        }
    }
    None
}

fn disk_touches(map: &GameMap, disk: &[TileRef], nation: u16) -> bool {
    for &t in disk {
        let mut nb = [0u32; 4];
        let k = map.neighbors4(t, &mut nb);
        for &u in &nb[..k] {
            if map.owner_id(u) == nation {
                return true;
            }
        }
    }
    false
}

/// `v3_state_util::nearest_land`: passable unowned land, ring search from `origin`.
fn nearest_land(game: &Game, origin: TileRef) -> TileRef {
    let map = &game.state.map;
    let (x, y) = (map.x(origin), map.y(origin));
    for r in 0..200i64 {
        for dy in -r..=r {
            for dx in -r..=r {
                if dx.abs() != r && dy.abs() != r {
                    continue;
                }
                let (xx, yy) = (i64::from(x) + dx, i64::from(y) + dy);
                if xx < 0 || yy < 0 || xx >= i64::from(map.width()) || yy >= i64::from(map.height()) {
                    continue;
                }
                let t = map.ref_(xx as f64, yy as f64);
                if map.is_land(t) && !map.is_impassable(t) && !map.has_owner(t) {
                    return t;
                }
            }
        }
    }
    panic!("t608: no land near ({x},{y})");
}

/// `G::give`: conquer the unowned path from us to a tile that 4-neighbors the nation.
fn bridge_to_nation(game: &mut Game, me: SmallId, nation: SmallId) -> u32 {
    if game.view().player(me).shares_border_with(nation) {
        return 0;
    }
    let n = game.state.map.num_tiles() as usize;
    let mut seen = vec![0u8; n];
    let mut parent = vec![u32::MAX; n];
    let mut q = VecDeque::new();
    let my = me.0;
    let nid = nation.0;
    let ours: Vec<TileRef> = game.view().player(me).tiles().iter().collect();
    if ours.is_empty() {
        panic!("t608: human owns no tiles");
    }
    for &t in &ours {
        seen[t as usize] = 1;
    }
    {
        let map = &game.state.map;
        for &t in &ours {
            let mut nb = [0u32; 4];
            let k = map.neighbors4(t, &mut nb);
            for &u in &nb[..k] {
                if seen[u as usize] == 0 && open_land(map, u) {
                    seen[u as usize] = 1;
                    parent[u as usize] = t;
                    q.push_back(u);
                }
            }
        }
    }
    let mut goal: Option<TileRef> = None;
    while let Some(t) = q.pop_front() {
        let map = &game.state.map;
        let mut nb = [0u32; 4];
        let k = map.neighbors4(t, &mut nb);
        if nb[..k].iter().any(|&u| map.owner_id(u) == nid) {
            goal = Some(t);
            break;
        }
        for &u in &nb[..k] {
            if seen[u as usize] == 0 && open_land(map, u) {
                seen[u as usize] = 1;
                parent[u as usize] = t;
                q.push_back(u);
            }
        }
    }
    let Some(goal) = goal else {
        panic!(
            "t608: no unowned land path from us to nation {} (tiles_us={} tiles_nat={})",
            game.view().player_id(nation),
            game.view().player(me).num_tiles_owned(),
            game.view().player(nation).num_tiles_owned()
        );
    };
    let mut path = Vec::new();
    let mut cur = goal;
    let mut guard = 0usize;
    while game.state.map.owner_id(cur) != my && guard < n {
        path.push(cur);
        let p = parent[cur as usize];
        if p == u32::MAX {
            break;
        }
        cur = p;
        guard += 1;
    }
    let count = path.len() as u32;
    let st = &mut game.state;
    with_thread_scratch(|sc| {
        let mut cx = Ctx::new(st, sc, None);
        for &t in &path {
            if cx.st.map.owner_id(t) != my {
                cx.player_mut(me).conquer(t);
            }
        }
    });
    count
}

fn open_land(map: &GameMap, t: TileRef) -> bool {
    map.is_land(t) && !map.is_impassable(t) && !map.has_owner(t)
}

fn find_one(game: &Game, ty: PlayerType) -> Option<SmallId> {
    let v = game.view();
    v.all_players().find(|&p| v.player_type(p) == ty)
}

fn step(game: &mut Game, intent: Option<Intent>) {
    let turn = game.state.ticks as u64;
    let intents = match intent {
        Some(intent) => vec![StampedIntent { client_id: CLIENT.to_string(), intent }],
        None => Vec::new(),
    };
    game.step(&Turn { turn_number: turn, intents, hash: None }).expect("step");
}

fn start_info() -> GameStartInfo {
    serde_json::from_str(&format!(
        r#"{{
            "gameID": "{GAME_ID}",
            "lobbyCreatedAt": 0,
            "config": {{
                "gameMap": "{MAP_NAME}",
                "difficulty": "Hard",
                "donateGold": false,
                "donateTroops": false,
                "gameType": "Public",
                "gameMode": "Free For All",
                "gameMapSize": "Compact",
                "nations": 1,
                "bots": 4,
                "infiniteGold": false,
                "infiniteTroops": false,
                "instantBuild": false,
                "randomSpawn": false,
                "spawnImmunityDuration": 0,
                "disabledUnits": []
            }},
            "players": [{{"clientID": "{CLIENT}", "username": "agent0", "clanTag": null}}]
        }}"#
    ))
    .expect("GameStartInfo")
}

/// The research harness's map loader: Compact reads `map4x.bin` + `map16x.bin`.
fn load_statics(gs: &GameStartInfo) -> Arc<MapStatics> {
    let c = &gs.config;
    let key: String = c.game_map.to_lowercase().chars().filter(|ch| ch.is_ascii_alphanumeric()).collect();
    let dir = map_dir(&key);
    let read = |f: &str| {
        std::fs::read(dir.join(f)).unwrap_or_else(|e| panic!("t608: map file {}: {e}", dir.join(f).display()))
    };
    let mut mj: serde_json::Value = serde_json::from_slice(&read("manifest.json")).expect("manifest json");
    if let Some(o) = mj.as_object_mut() {
        o.entry("nations").or_insert_with(|| serde_json::Value::Array(Vec::new()));
    }
    let manifest: MapManifest = serde_json::from_value(mj).expect("manifest");
    let (map_bin, map16x_bin) = match c.game_map_size {
        GameMapSize::Normal => (Some(read("map.bin")), None),
        GameMapSize::Compact => (None, Some(read("map16x.bin"))),
    };
    let files = MapFiles {
        map: c.game_map.clone(),
        map_size: c.game_map_size,
        manifest,
        map_bin,
        map4x_bin: Some(read("map4x.bin")),
        map16x_bin,
    };
    of_engine::runner::check_map_files(gs, &files).expect("check_map_files");
    MapStatics::load(&files).unwrap_or_else(|e| panic!("MapStatics::load: {e}"))
}

fn map_dir(key: &str) -> PathBuf {
    let mut tried = Vec::new();
    for root in upstream_roots() {
        for rel in ["resources/maps", "packages/resources/maps"] {
            let dir = root.join(rel).join(key);
            let manifest = dir.join("manifest.json");
            if manifest.is_file() {
                return dir;
            }
            tried.push(manifest);
        }
    }
    let list = tried.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(" ");
    panic!(
        "t608: {MAP_NAME} Compact not found. Set OF_UPSTREAM_DIR to the checkout that contains resources/maps. Tried: {list}"
    );
}

fn upstream_roots() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(d) = std::env::var_os("OF_UPSTREAM_DIR") {
        v.push(PathBuf::from(d));
    }
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    // Default location of an upstream checkout, relative to this crate (a sibling of the repository root).
    v.push(manifest.join("../../../upstream"));
    // One level further up, for checkouts that keep upstream next to the repository.
    v.push(manifest.join("../../../../upstream"));
    v
}
