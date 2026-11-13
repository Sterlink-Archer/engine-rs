//! t409 H5 guard: `Player::conn4` ("the territory is one 4-connected component") is never `true`
//! while the territory is disconnected, across random `conquer`/`relinquish` sequences on the
//! `plains` test map (corner windows, so map edges are exercised). The flag is set by the test
//! whenever the territory really is connected (standing in for `removeClusters`' full flood), and
//! after EVERY ownership change each flagged player's territory is flooded and must be connected.
//! A second check counts how often the flag survived a loss, so the test cannot pass vacuously.
//! The component labels (`Player::comps`, `State::comp_labels`) are checked the same way: whenever
//! a player's memo is valid, its labels and sizes must name exactly the true components.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use of_engine::Game;
use of_engine::api::GameStartInfo;
use of_engine::api::game_config::GameConfig;
use of_engine::api::map_files::MapFiles;
use of_engine::api::types::{GameMapSize, PlayerInfo, PlayerType};
use of_engine::config::EngineConfig;
use of_engine::ctx::Ctx;
use of_engine::ids::SmallId;
use of_engine::map::MapStatics;
use of_engine::map::manifest::MapManifest;
use of_engine::scratch::Scratch;
use of_engine::state::{State, create_state};
use std::path::Path;
use std::sync::Arc;

const START: &str = r#"{
  "gameID": "h5conn4",
  "lobbyCreatedAt": 0,
  "config": {
    "gameMap": "Asia", "difficulty": "Medium", "donateGold": false, "donateTroops": false,
    "gameType": "Singleplayer", "gameMode": "Free For All", "gameMapSize": "Normal",
    "nations": "disabled", "bots": 0, "infiniteGold": true, "infiniteTroops": false,
    "instantBuild": true, "randomSpawn": false
  },
  "players": []
}"#;

fn plains_game() -> Game {
    let gs: GameStartInfo = serde_json::from_str(START).expect("start");
    let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/testdata/maps/plains");
    let read = |f: &str| std::fs::read(d.join(f)).expect("plains file");
    let mut mj: serde_json::Value = serde_json::from_slice(&read("manifest.json")).expect("manifest json");
    if let Some(o) = mj.as_object_mut() {
        o.entry("nations").or_insert_with(|| serde_json::Value::Array(Vec::new()));
    }
    let manifest: MapManifest = serde_json::from_value(mj).expect("manifest");
    let files = MapFiles {
        map: gs.config.game_map.clone(),
        map_size: GameMapSize::Normal,
        manifest,
        map_bin: Some(read("map.bin")),
        map4x_bin: Some(read("map4x.bin")),
        map16x_bin: Some(read("map16x.bin")),
    };
    let statics = MapStatics::load(&files).expect("MapStatics::load");
    let gc: GameConfig = gs.config.clone();
    let cfg = Arc::new(EngineConfig::new(gc, false, false, false));
    let st = create_state(Vec::new(), Vec::new(), statics, cfg);
    Game::from_state(st, &gs.game_id)
}

/// The 4-connected components of player `p`'s territory: a component index per tile (u32::MAX =
/// not owned) and the component sizes. Plain BFS.
fn components(st: &State, p: SmallId) -> (Vec<u32>, Vec<i32>) {
    let (w, h) = (st.map.width(), st.map.height());
    let n = w * h;
    let mut comp = vec![u32::MAX; n as usize];
    let mut sizes = Vec::new();
    for s in 0..n {
        if st.map.owner_id(s) != p.0 || comp[s as usize] != u32::MAX {
            continue;
        }
        let c = sizes.len() as u32;
        comp[s as usize] = c;
        let mut stack = vec![s];
        let mut cnt = 0;
        while let Some(t) = stack.pop() {
            cnt += 1;
            let (x, y) = (t % w, t / w);
            for (ok, m) in
                [(y > 0, t.wrapping_sub(w)), (y + 1 < h, t + w), (x > 0, t.wrapping_sub(1)), (x + 1 < w, t + 1)]
            {
                if ok && comp[m as usize] == u32::MAX && st.map.owner_id(m) == p.0 {
                    comp[m as usize] = c;
                    stack.push(m);
                }
            }
        }
        sizes.push(cnt);
    }
    (comp, sizes)
}

/// `Player::comps` (valid) must name exactly the true components: same label within a component,
/// different labels across components, and `sizes[label]` = the owned tiles carrying it.
fn check_labels(st: &State, p: SmallId) {
    let (comp, true_sizes) = components(st, p);
    let d = st.player_data(p);
    let labels = &st.comp_labels.v;
    let mut label_of_comp = vec![u32::MAX; true_sizes.len()];
    let mut count = vec![0i32; d.comps.sizes.len()];
    for (t, &c) in comp.iter().enumerate() {
        if c == u32::MAX {
            continue;
        }
        let l = labels[t];
        assert!((l as usize) < d.comps.sizes.len(), "player {p:?}: tile {t} label {l} out of range");
        count[l as usize] += 1;
        if label_of_comp[c as usize] == u32::MAX {
            label_of_comp[c as usize] = l;
        }
        assert_eq!(label_of_comp[c as usize], l, "player {p:?}: component {c} has two labels");
    }
    let mut seen = vec![false; d.comps.sizes.len()];
    for &l in &label_of_comp {
        assert!(!seen[l as usize], "player {p:?}: label {l} spans two components");
        seen[l as usize] = true;
    }
    assert_eq!(count, d.comps.sizes, "player {p:?}: label sizes");
}

/// Is player `p`'s territory (tiles with ownerID == p) 4-connected (or empty)? Plain BFS.
fn connected(st: &State, p: SmallId) -> bool {
    let (w, h) = (st.map.width(), st.map.height());
    let n = w * h;
    let owned: Vec<u32> = (0..n).filter(|&t| st.map.owner_id(t) == p.0).collect();
    let Some(&s) = owned.first() else { return true };
    let mut seen = vec![false; n as usize];
    seen[s as usize] = true;
    let mut stack = vec![s];
    let mut cnt = 0usize;
    while let Some(t) = stack.pop() {
        cnt += 1;
        let (x, y) = (t % w, t / w);
        let mut nb = Vec::with_capacity(4);
        if y > 0 {
            nb.push(t - w);
        }
        if y + 1 < h {
            nb.push(t + w);
        }
        if x > 0 {
            nb.push(t - 1);
        }
        if x + 1 < w {
            nb.push(t + 1);
        }
        for m in nb {
            if !seen[m as usize] && st.map.owner_id(m) == p.0 {
                seen[m as usize] = true;
                stack.push(m);
            }
        }
    }
    cnt == owned.len()
}

#[test]
fn conn4_flag_never_claims_a_disconnected_territory() {
    let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let (mut kept_on_loss, mut kept_on_gain, mut cleared, mut checks) = (0u64, 0u64, 0u64, 0u64);
    let (mut label_checks, mut multi_comp_checks) = (0u64, 0u64);
    // R2: losses where the ring rule fails and the labels (and conn4) survive: through a connected result or a split with relabelled pockets
    let (mut kept_labels_split, mut kept_labels_nosplit, mut kept_conn4_ring_fail) = (0u64, 0u64, 0u64);
    // (x0, y0, size): two corners (map edges) and the middle; small windows => dense contact
    for &(x0, y0, sz) in &[(0u32, 0u32, 14u32), (86, 86, 14), (40, 40, 10), (0, 50, 18)] {
        for round in 0..6 {
            let mut game = plains_game();
            let mut sc = Scratch::new();
            let mut cx = Ctx::new(&mut game.state, &mut sc, None);
            let ps: Vec<SmallId> = (0..3)
                .map(|i| {
                    let id = format!("p{round}_{i}");
                    cx.add_player(PlayerInfo::new(&id, PlayerType::Human, Some(format!("c{i}")), &id), None)
                })
                .collect();
            let w = cx.st.map.width();
            // seed blocks: player i owns a 3x3 block along the window's diagonal
            for (i, &p) in ps.iter().enumerate() {
                let o = (i as u32) * (sz - 3) / 2;
                for dy in 0..3 {
                    for dx in 0..3 {
                        cx.player_mut(p).conquer((y0 + o + dy) * w + x0 + o + dx);
                    }
                }
            }
            for _step in 0..4000 {
                let t = (y0 + (next() % u64::from(sz)) as u32) * w + x0 + (next() % u64::from(sz)) as u32;
                let owner_before = SmallId(cx.st.map.owner_id(t));
                let flags_before: Vec<bool> = ps.iter().map(|&p| cx.st.player_data(p).conn4).collect();
                // R2: (labels valid, component count) before the step, for the survival counters below
                let comps_before: Vec<(bool, usize)> = ps
                    .iter()
                    .map(|&p| {
                        (
                            cx.st.player_data(p).comps.valid,
                            if cx.st.player_data(p).comps.valid { components(cx.st, p).1.len() } else { 0 },
                        )
                    })
                    .collect();
                if next() % 25 == 0 {
                    if owner_before.is_player() {
                        cx.player_mut(owner_before).relinquish(t);
                    }
                } else {
                    // mostly frontier moves (the owner of a random neighbour takes t), like attacks
                    let (x, y) = (t % w, t / w);
                    let nb = match next() % 4 {
                        0 if y > 0 => t - w,
                        1 if x > 0 => t - 1,
                        2 if x + 1 < w => t + 1,
                        _ if y + 1 < cx.st.map.height() => t + w,
                        _ => t,
                    };
                    let no = SmallId(cx.st.map.owner_id(nb));
                    // 1 in 60: an isolated grab by anyone (boat landings, annexations)
                    if no.is_player() && next() % 60 != 0 {
                        cx.player_mut(no).conquer(t);
                    } else if next() % 60 == 0 {
                        cx.player_mut(ps[(next() % 3) as usize]).conquer(t);
                    }
                }
                for (i, &p) in ps.iter().enumerate() {
                    let flag = cx.st.player_data(p).conn4;
                    if flag {
                        checks += 1;
                        assert!(connected(cx.st, p), "conn4 set on a disconnected territory: player {p:?} tile {t}");
                        if flags_before[i] && owner_before == p && cx.st.map.owner_id(t) != p.0 {
                            kept_on_loss += 1;
                            if !of_engine::map::conn4::loss_keeps(&cx.st.map, t, p.0) {
                                kept_conn4_ring_fail += 1; // the ring rule failed and the search still proved connectivity (R2)
                            }
                        }
                        if flags_before[i] && owner_before != p && cx.st.map.owner_id(t) == p.0 {
                            kept_on_gain += 1;
                        }
                    } else if flags_before[i] {
                        cleared += 1;
                    }
                    // stand-in for removeClusters' flood: re-arm when really connected
                    if !flag && next() % 3 == 0 && connected(cx.st, p) {
                        cx.st.player_data_mut(p).conn4 = true;
                    }
                    if cx.st.player_data(p).comps.valid {
                        if comps_before[i].0 && owner_before == p && cx.st.map.owner_id(t) != p.0 {
                            let after = components(cx.st, p).1.len();
                            if after > comps_before[i].1 {
                                kept_labels_split += 1; // the territory split and the labels were kept (R2 relabelled the pocket)
                            } else {
                                kept_labels_nosplit += 1;
                            }
                        }
                        check_labels(cx.st, p);
                        label_checks += 1;
                        if cx.st.player_data(p).comps.sizes.iter().filter(|&&s| s > 0).count() > 1 {
                            multi_comp_checks += 1;
                        }
                    } else if next() % 3 == 0 {
                        // stand-in for the flood that writes the labels
                        let (comp, sizes) = components(cx.st, p);
                        let n = comp.len();
                        cx.st.comp_labels.ensure(n);
                        for (t, &c) in comp.iter().enumerate() {
                            if c != u32::MAX {
                                cx.st.comp_labels.v[t] = c;
                            }
                        }
                        let d = cx.st.player_data_mut(p);
                        d.comps.sizes = sizes;
                        d.comps.valid = true;
                    }
                }
            }
        }
    }
    eprintln!(
        "conn4 fuzz: checks {checks}, kept on loss {kept_on_loss}, on gain {kept_on_gain}, cleared {cleared}; \
         label checks {label_checks} (multi-component {multi_comp_checks}); R2: labels kept across a split {kept_labels_split}, across a non-split loss {kept_labels_nosplit}, conn4 kept across a ring failure {kept_conn4_ring_fail}"
    );
    assert!(kept_on_loss > 1000 && kept_on_gain > 1000 && cleared > 100, "fuzz too weak");
    assert!(label_checks > 20_000 && multi_comp_checks > 5_000, "label fuzz too weak");
    assert!(
        kept_labels_split > 200 && kept_conn4_ring_fail > 50,
        "R2 fuzz too weak (the search never settled a split / a ring failure)"
    );
}
