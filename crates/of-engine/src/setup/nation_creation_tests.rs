//! Port of `upstream/tests/NationCreation.test.ts` (P3 acceptance ladder T0, FILE_MAP §4). Owner: **P3**.

#![allow(clippy::disallowed_methods, clippy::unwrap_used, clippy::expect_used)] // test code

use super::*;
use crate::api::game_config::{GameConfig, GameStartInfo, NationsConfig};
use crate::api::types::{Difficulty, GameMapSize, GameMode, GameType, Nation};
use crate::jsorder::LookupSet;
use crate::map::loader::nation_template;
use crate::map::manifest::ManifestNation;
use of_det::Prng;

/// A manifest / additional-nation entry as the `Nation` template `create_nations_for_game` reads.
fn tmpl(name: &str, coordinates: Option<[u32; 2]>, flag: Option<&str>) -> Nation {
    nation_template(&ManifestNation { name: name.to_owned(), coordinates, flag: flag.map(str::to_owned) })
}

/// `makeManifestNations`: coordinates `[i, i]`, flag `""`.
fn make_manifest_nations(count: u32) -> Vec<Nation> {
    (0..count).map(|i| tmpl(&format!("Manifest{i}"), Some([i, i]), Some(""))).collect()
}

/// `makeAdditionalNations`: name only (no coordinates, no flag).
fn make_additional_nations(names: &[&str]) -> Vec<Nation> {
    names.iter().map(|name| tmpl(name, None, None)).collect()
}

/// `makeGameStart(targetNations)`.
fn make_game_start(target_nations: u64) -> GameStartInfo {
    GameStartInfo {
        game_id: "test1234".to_owned(),
        lobby_created_at: 0,
        visible_at: None,
        listed: None,
        config: GameConfig {
            game_map: "World".to_owned(),
            game_map_size: GameMapSize::Normal,
            game_mode: GameMode::Ffa,
            game_type: GameType::Singleplayer,
            difficulty: Difficulty::Medium,
            nations: NationsConfig::Count(target_nations),
            donate_gold: false,
            donate_troops: false,
            bots: 0,
            infinite_gold: false,
            infinite_troops: false,
            instant_build: false,
            random_spawn: false,
            ..GameConfig::default()
        },
        players: Vec::new(),
        tribes: None,
    }
}

fn nation_names(nations: &[Nation]) -> Vec<&str> {
    nations.iter().map(|n| n.player_info.name.as_str()).collect()
}

/// `new Set(names).size` without iterating a hash set.
fn distinct_len(names: &[&str]) -> usize {
    let mut set = LookupSet::new();
    for name in names {
        set.insert((*name).to_owned());
    }
    set.len()
}

// TS: NationCreation.test.ts:62
#[test]
fn does_not_draw_from_the_pool_when_manifest_already_covers_the_count() {
    let manifest = make_manifest_nations(4);
    let extras = make_additional_nations(&["ExtraA", "ExtraB", "ExtraC"]);
    let mut random = Prng::new(1.0);

    let nations = create_nations_for_game(&make_game_start(3), &manifest, &extras, 0, &mut random);

    assert_eq!(nations.len(), 3);
    for name in nation_names(&nations) {
        assert!(name.starts_with("Manifest"));
    }
}

// TS: NationCreation.test.ts:81
#[test]
fn fills_the_deficit_entirely_from_the_pool_when_it_is_large_enough() {
    let manifest = make_manifest_nations(2);
    let extras = make_additional_nations(&["ExtraA", "ExtraB", "ExtraC", "ExtraD", "ExtraE"]);
    let mut random = Prng::new(7.0);

    let nations = create_nations_for_game(&make_game_start(5), &manifest, &extras, 0, &mut random);

    assert_eq!(nations.len(), 5);
    let names = nation_names(&nations);
    assert_eq!(names.iter().filter(|n| n.starts_with("Manifest")).count(), 2);
    let from_pool: Vec<&str> = names.iter().copied().filter(|n| n.starts_with("Extra")).collect();
    assert_eq!(from_pool.len(), 3);
    for name in from_pool {
        assert!(extras.iter().any(|e| e.player_info.name == name));
    }
}

// TS: NationCreation.test.ts:112
#[test]
fn randomly_selects_from_the_pool_when_it_has_more_entries_than_needed() {
    let manifest = make_manifest_nations(2);
    let pool = ["ExtraA", "ExtraB", "ExtraC", "ExtraD", "ExtraE", "ExtraF", "ExtraG", "ExtraH"];
    let extras = make_additional_nations(&pool);

    let mut seen = LookupSet::new();
    for seed in 1u32..=25 {
        let mut random = Prng::new(f64::from(seed));
        let nations = create_nations_for_game(&make_game_start(4), &manifest, &extras, 0, &mut random);
        assert_eq!(nations.len(), 4);

        let from_pool: Vec<&str> = nation_names(&nations).into_iter().filter(|n| n.starts_with("Extra")).collect();
        assert_eq!(from_pool.len(), 2);
        for name in &from_pool {
            assert!(pool.contains(name));
        }
        for name in from_pool {
            seen.insert(name.to_owned());
        }
    }
    assert!(seen.len() > 2);
}

// TS: NationCreation.test.ts:150
#[test]
fn falls_back_to_generated_names_when_the_pool_is_too_small() {
    let manifest = make_manifest_nations(1);
    let extras = make_additional_nations(&["ExtraA", "ExtraB"]);
    let mut random = Prng::new(42.0);

    let nations = create_nations_for_game(&make_game_start(5), &manifest, &extras, 0, &mut random);

    assert_eq!(nations.len(), 5);
    let names = nation_names(&nations);
    assert_eq!(names.iter().filter(|n| n.starts_with("Manifest")).count(), 1);
    let from_pool: Vec<&str> =
        names.iter().copied().filter(|n| extras.iter().any(|e| e.player_info.name == *n)).collect();
    assert_eq!(from_pool.len(), 2);
    let generated: Vec<&str> = names
        .iter()
        .copied()
        .filter(|n| !n.starts_with("Manifest") && !extras.iter().any(|e| e.player_info.name == *n))
        .collect();
    assert_eq!(generated.len(), 2);
}

// TS: NationCreation.test.ts:177
#[test]
fn falls_back_to_generated_names_when_the_pool_is_empty() {
    let manifest = make_manifest_nations(2);
    let mut random = Prng::new(11.0);

    let nations = create_nations_for_game(&make_game_start(4), &manifest, &[], 0, &mut random);

    assert_eq!(nations.len(), 4);
    assert_eq!(nation_names(&nations).iter().filter(|n| n.starts_with("Manifest")).count(), 2);
}

// TS: NationCreation.test.ts:195
#[test]
fn skips_pool_entries_whose_name_collides_with_a_manifest_nation() {
    let manifest = make_manifest_nations(2);
    let extras = make_additional_nations(&["Manifest0", "Manifest1", "UniqueExtra"]);
    let mut random = Prng::new(3.0);

    let nations = create_nations_for_game(&make_game_start(3), &manifest, &extras, 0, &mut random);

    let names = nation_names(&nations);
    assert_eq!(names.len(), 3);
    assert_eq!(distinct_len(&names), 3);
    assert!(names.contains(&"UniqueExtra"));
}

// TS: NationCreation.test.ts:218
#[test]
fn uses_coordinates_from_additional_nations_when_provided() {
    let manifest = make_manifest_nations(1);
    let extras = vec![tmpl("WithCoords", Some([42, 99]), None), tmpl("WithoutCoords", None, None)];
    let mut random = Prng::new(5.0);

    let nations = create_nations_for_game(&make_game_start(3), &manifest, &extras, 0, &mut random);

    let with_coords = nations.iter().find(|n| n.player_info.name == "WithCoords");
    let without_coords = nations.iter().find(|n| n.player_info.name == "WithoutCoords");
    assert!(with_coords.is_some());
    assert!(without_coords.is_some());
    let with_coords = with_coords.unwrap();
    let without_coords = without_coords.unwrap();
    assert_eq!(with_coords.spawn_cell.map(|c| c.x), Some(42.0));
    assert_eq!(with_coords.spawn_cell.map(|c| c.y), Some(99.0));
    assert!(without_coords.spawn_cell.is_none());
}

// TS: NationCreation.test.ts:246
#[test]
fn uses_coordinates_from_manifest_nations_when_provided_undefined_when_omitted() {
    let manifest = vec![tmpl("WithCoords", Some([10, 20]), None), tmpl("WithoutCoords", None, None)];
    let mut random = Prng::new(5.0);

    let nations = create_nations_for_game(&make_game_start(2), &manifest, &[], 0, &mut random);

    assert_eq!(nations.len(), 2);
    let with_coords = nations.iter().find(|n| n.player_info.name == "WithCoords");
    let without_coords = nations.iter().find(|n| n.player_info.name == "WithoutCoords");
    assert!(with_coords.is_some());
    assert!(without_coords.is_some());
    let with_coords = with_coords.unwrap();
    let without_coords = without_coords.unwrap();
    assert_eq!(with_coords.spawn_cell.map(|c| c.x), Some(10.0));
    assert_eq!(with_coords.spawn_cell.map(|c| c.y), Some(20.0));
    assert!(without_coords.spawn_cell.is_none());
}

// TS: NationCreation.test.ts:274
#[test]
fn carries_each_manifest_nations_own_flag_through_even_when_names_collide() {
    // Same display name, distinct flags (India's and Pakistan's "Punjab").
    let manifest = vec![tmpl("Punjab", Some([840, 305]), Some("in")), tmpl("Punjab", Some([637, 464]), Some("pk"))];
    let mut random = Prng::new(5.0);

    let nations = create_nations_for_game(&make_game_start(2), &manifest, &[], 0, &mut random);

    assert_eq!(nations.len(), 2);
    let mut flags: Vec<&str> = nations.iter().map(|n| n.player_info.nation_flag.as_deref().unwrap()).collect();
    flags.sort();
    assert_eq!(flags, ["in", "pk"]);
}

// TS: NationCreation.test.ts:298
#[test]
fn carries_flags_from_additional_nations_through_too() {
    let manifest = make_manifest_nations(1);
    let extras = vec![tmpl("WithFlag", None, Some("fr")), tmpl("WithoutFlag", None, None)];
    let mut random = Prng::new(5.0);

    let nations = create_nations_for_game(&make_game_start(3), &manifest, &extras, 0, &mut random);

    let with_flag = nations.iter().find(|n| n.player_info.name == "WithFlag").unwrap();
    let without_flag = nations.iter().find(|n| n.player_info.name == "WithoutFlag").unwrap();
    assert_eq!(with_flag.player_info.nation_flag.as_deref(), Some("fr"));
    assert_eq!(without_flag.player_info.nation_flag.as_deref(), None);
}

// TS: NationCreation.test.ts:323
#[test]
fn produces_unique_nation_names_overall() {
    let manifest = make_manifest_nations(3);
    let extras = make_additional_nations(&["Ex1", "Ex2", "Ex3"]);
    let mut random = Prng::new(99.0);

    let nations = create_nations_for_game(&make_game_start(8), &manifest, &extras, 0, &mut random);

    let names = nation_names(&nations);
    assert_eq!(names.len(), 8);
    assert_eq!(distinct_len(&names), 8);
}
