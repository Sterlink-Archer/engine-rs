//! Port of `upstream/tests/TeamAssignment.test.ts` (P3 acceptance ladder T0, FILE_MAP §4). Owner: **P3**.

#![allow(clippy::disallowed_methods, clippy::unwrap_used, clippy::expect_used)] // test code

use super::*;
use crate::api::game_config::TeamCountConfig;
use crate::api::types::{PlayerInfo, PlayerType, colored_teams};

fn on_team(name: &str) -> TeamSlot {
    TeamSlot::Team(name.to_owned())
}

fn two_teams() -> [String; 2] {
    [colored_teams::RED.to_owned(), colored_teams::BLUE.to_owned()]
}

fn duo_teams() -> [String; 3] {
    [colored_teams::RED.to_owned(), colored_teams::BLUE.to_owned(), colored_teams::YELLOW.to_owned()]
}

fn seven_teams() -> [String; 7] {
    [
        colored_teams::RED.to_owned(),
        colored_teams::BLUE.to_owned(),
        colored_teams::YELLOW.to_owned(),
        colored_teams::GREEN.to_owned(),
        colored_teams::PURPLE.to_owned(),
        colored_teams::ORANGE.to_owned(),
        colored_teams::TEAL.to_owned(),
    ]
}

/// `createPlayer(id, clan?)`. An omitted clan is the `PlayerInfo` default `clanTag = null`.
fn create_player(id: &str, clan: Option<&str>) -> PlayerInfo {
    let mut p = PlayerInfo::new(format!("Player {id}"), PlayerType::Human, None, id);
    p.clan_tag = clan.map(str::to_owned);
    p
}

/// `createPlayerWithFriends(id, friends, clan?, clientID = id)`.
fn create_player_with_friends(id: &str, friends: &[&str], clan: Option<&str>, client_id: Option<&str>) -> PlayerInfo {
    let mut p =
        PlayerInfo::new(format!("Player {id}"), PlayerType::Human, Some(client_id.unwrap_or(id).to_owned()), id);
    p.clan_tag = clan.map(str::to_owned);
    p.friends = friends.iter().map(|f| (*f).to_owned()).collect();
    p
}

/// `createPinnedPlayer(id, teamIndex, clan?, friends = [])`. `clientID` is `id`.
fn create_pinned_player(id: &str, team_index: Option<u32>, clan: Option<&str>, friends: &[&str]) -> PlayerInfo {
    let mut p = PlayerInfo::new(format!("Player {id}"), PlayerType::Human, Some(id.to_owned()), id);
    p.clan_tag = clan.map(str::to_owned);
    p.friends = friends.iter().map(|f| (*f).to_owned()).collect();
    p.team_index = team_index;
    p
}

/// `createNation(id)`.
fn create_nation(id: &str) -> PlayerInfo {
    PlayerInfo::new(format!("Nation {id}"), PlayerType::Nation, None, id)
}

/// `human(i, clan = null)` from the `humanTeams` suite.
fn human(i: u32, clan: Option<&str>) -> PlayerInfo {
    let mut p = PlayerInfo::new(format!("p{i}"), PlayerType::Human, Some(format!("c{i}")), format!("id{i}"));
    p.clan_tag = clan.map(str::to_owned);
    p
}

/// Vitest `toThrow(substring)`: the panic is a `TsThrow` whose message contains `needle`.
fn assert_throws_containing(f: impl FnOnce(), needle: &str) {
    let payload = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).expect_err("expected throw");
    let msg = &of_det::throw::as_ts_throw(&*payload).unwrap().msg;
    assert!(msg.contains(needle), "{msg}");
}

// --- assignTeams ---

// TS: TeamAssignment.test.ts:51
#[test]
fn should_assign_players_to_teams_when_no_clans_are_present() {
    let players =
        [create_player("1", None), create_player("2", None), create_player("3", None), create_player("4", None)];
    let result = assign_teams(&players, &two_teams(), false, None);
    assert_eq!(result[0], on_team(colored_teams::RED));
    assert_eq!(result[1], on_team(colored_teams::BLUE));
    assert_eq!(result[2], on_team(colored_teams::RED));
    assert_eq!(result[3], on_team(colored_teams::BLUE));
}

// TS: TeamAssignment.test.ts:68
#[test]
fn should_keep_clan_members_together_on_the_same_team() {
    let players = [
        create_player("1", Some("CLANA")),
        create_player("2", Some("CLANA")),
        create_player("3", Some("CLANB")),
        create_player("4", Some("CLANB")),
    ];
    let result = assign_teams(&players, &two_teams(), false, None);
    assert_eq!(result[0], on_team(colored_teams::RED));
    assert_eq!(result[1], on_team(colored_teams::RED));
    assert_eq!(result[2], on_team(colored_teams::BLUE));
    assert_eq!(result[3], on_team(colored_teams::BLUE));
}

// TS: TeamAssignment.test.ts:85
#[test]
fn should_handle_mixed_clan_and_non_clan_players() {
    let players = [
        create_player("1", Some("CLANA")),
        create_player("2", Some("CLANA")),
        create_player("3", None),
        create_player("4", None),
    ];
    let result = assign_teams(&players, &two_teams(), false, None);
    assert_eq!(result[0], on_team(colored_teams::RED));
    assert_eq!(result[1], on_team(colored_teams::RED));
    assert_eq!(result[2], on_team(colored_teams::BLUE));
    assert_eq!(result[3], on_team(colored_teams::BLUE));
}

// TS: TeamAssignment.test.ts:102
#[test]
fn should_kick_players_when_teams_are_full() {
    let players = [
        create_player("1", Some("CLANA")),
        create_player("2", Some("CLANA")),
        create_player("3", Some("CLANA")),
        create_player("4", Some("CLANA")),
        create_player("5", Some("CLANB")),
        create_player("6", Some("CLANB")),
    ];
    let result = assign_teams(&players, &two_teams(), false, None);
    assert_eq!(result[0], on_team(colored_teams::RED));
    assert_eq!(result[1], on_team(colored_teams::RED));
    assert_eq!(result[2], on_team(colored_teams::RED));
    assert_eq!(result[3], TeamSlot::Kicked);
    assert_eq!(result[4], on_team(colored_teams::BLUE));
    assert_eq!(result[5], on_team(colored_teams::BLUE));
}

// TS: TeamAssignment.test.ts:125
#[test]
fn should_handle_empty_player_list() {
    let result = assign_teams(&[], &two_teams(), false, None);
    assert_eq!(result.len(), 0);
}

// TS: TeamAssignment.test.ts:130
#[test]
fn should_handle_single_player() {
    let players = [create_player("1", None)];
    let result = assign_teams(&players, &two_teams(), false, None);
    assert_eq!(result[0], on_team(colored_teams::RED));
}

// TS: TeamAssignment.test.ts:136
#[test]
fn should_handle_multiple_clans_with_different_sizes() {
    let players = [
        create_player("1", Some("CLANA")),
        create_player("2", Some("CLANA")),
        create_player("3", Some("CLANA")),
        create_player("4", Some("CLANB")),
        create_player("5", Some("CLANB")),
        create_player("6", Some("CLANC")),
    ];
    let result = assign_teams(&players, &two_teams(), false, None);
    assert_eq!(result[0], on_team(colored_teams::RED));
    assert_eq!(result[1], on_team(colored_teams::RED));
    assert_eq!(result[2], on_team(colored_teams::RED));
    assert_eq!(result[3], on_team(colored_teams::BLUE));
    assert_eq!(result[4], on_team(colored_teams::BLUE));
    assert_eq!(result[5], on_team(colored_teams::BLUE));
}

// TS: TeamAssignment.test.ts:157
#[test]
fn should_distribute_players_among_a_larger_number_of_teams() {
    let players = [
        create_player("1", Some("CLANA")),
        create_player("2", Some("CLANA")),
        create_player("3", Some("CLANA")),
        create_player("4", Some("CLANB")),
        create_player("5", Some("CLANB")),
        create_player("6", Some("CLANC")),
        create_player("7", None),
        create_player("8", None),
        create_player("9", None),
        create_player("10", None),
        create_player("11", None),
        create_player("12", None),
        create_player("13", None),
        create_player("14", None),
    ];
    let result = assign_teams(&players, &seven_teams(), false, None);
    assert_eq!(result[0], on_team(colored_teams::RED));
    assert_eq!(result[1], on_team(colored_teams::RED));
    assert_eq!(result[2], TeamSlot::Kicked);
    assert_eq!(result[3], on_team(colored_teams::BLUE));
    assert_eq!(result[4], on_team(colored_teams::BLUE));
    assert_eq!(result[5], on_team(colored_teams::YELLOW));
    assert_eq!(result[6], on_team(colored_teams::GREEN));
    assert_eq!(result[7], on_team(colored_teams::PURPLE));
    assert_eq!(result[8], on_team(colored_teams::ORANGE));
    assert_eq!(result[9], on_team(colored_teams::TEAL));
    assert_eq!(result[10], on_team(colored_teams::YELLOW));
    assert_eq!(result[11], on_team(colored_teams::GREEN));
    assert_eq!(result[12], on_team(colored_teams::PURPLE));
    assert_eq!(result[13], on_team(colored_teams::ORANGE));
}

// TS: TeamAssignment.test.ts:205
#[test]
fn should_keep_two_friends_on_the_same_team() {
    let players = [
        create_player_with_friends("1", &["2"], None, None),
        create_player_with_friends("2", &["1"], None, None),
        create_player_with_friends("3", &[], None, None),
        create_player_with_friends("4", &[], None, None),
    ];
    let result = assign_teams(&players, &two_teams(), false, None);
    assert_eq!(result[0], result[1]);
    assert_ne!(result[2], result[0]);
    assert_ne!(result[3], result[0]);
}

// TS: TeamAssignment.test.ts:220
#[test]
fn should_group_a_chain_of_friends_transitively() {
    // 6 players, 2 teams → maxTeamSize = 3 (enough room for a 3-friend chain).
    let players = [
        create_player_with_friends("1", &["2"], None, None),
        create_player_with_friends("2", &["3"], None, None),
        create_player_with_friends("3", &[], None, None),
        create_player_with_friends("4", &[], None, None),
        create_player_with_friends("5", &[], None, None),
        create_player_with_friends("6", &[], None, None),
    ];
    let result = assign_teams(&players, &two_teams(), false, None);
    assert_eq!(result[1], result[0]);
    assert_eq!(result[2], result[0]);
}

// TS: TeamAssignment.test.ts:238
#[test]
fn should_treat_one_directional_friendship_as_a_group() {
    let players = [
        create_player_with_friends("1", &["2"], None, None),
        create_player_with_friends("2", &[], None, None), // doesn't list 1 back
        create_player_with_friends("3", &[], None, None),
        create_player_with_friends("4", &[], None, None),
    ];
    let result = assign_teams(&players, &two_teams(), false, None);
    assert_eq!(result[0], result[1]);
}

// TS: TeamAssignment.test.ts:251
#[test]
fn should_merge_friend_and_clan_groups_when_they_overlap() {
    // 1 and 2 share clan CLANA, 2 is friends with 3 (no clan): all three share a team.
    let players = [
        create_player_with_friends("1", &[], Some("CLANA"), None),
        create_player_with_friends("2", &["3"], Some("CLANA"), None),
        create_player_with_friends("3", &[], None, None),
        create_player_with_friends("4", &[], None, None),
        create_player_with_friends("5", &[], None, None),
        create_player_with_friends("6", &[], None, None),
    ];
    let result = assign_teams(&players, &two_teams(), false, None);
    assert_eq!(result[1], result[0]);
    assert_eq!(result[2], result[0]);
}

// TS: TeamAssignment.test.ts:270
#[test]
fn should_spill_friend_group_overflow_to_other_teams_no_kicks() {
    // 4-player friend group + 2 strangers, maxTeamSize = ceil(6/2) = 3.
    let players = [
        create_player_with_friends("1", &["2", "3", "4"], None, None),
        create_player_with_friends("2", &[], None, None),
        create_player_with_friends("3", &[], None, None),
        create_player_with_friends("4", &[], None, None),
        create_player_with_friends("5", &[], None, None),
        create_player_with_friends("6", &[], None, None),
    ];
    let result = assign_teams(&players, &two_teams(), false, None);
    assert_eq!(result[0], on_team(colored_teams::RED));
    assert_eq!(result[1], on_team(colored_teams::RED));
    assert_eq!(result[2], on_team(colored_teams::RED));
    assert_eq!(result[3], on_team(colored_teams::BLUE));
    assert_eq!(result[4], on_team(colored_teams::BLUE));
    assert_eq!(result[5], on_team(colored_teams::BLUE));
}

// TS: TeamAssignment.test.ts:292
#[test]
fn should_key_friend_grouping_on_client_id_not_player_info_id() {
    // Friends list clientIDs ("client-2", "client-1"), distinct from PlayerInfo.id.
    let players = [
        create_player_with_friends("player-1", &["client-2"], None, Some("client-1")),
        create_player_with_friends("player-2", &["client-1"], None, Some("client-2")),
        create_player_with_friends("player-3", &[], None, Some("client-3")),
        create_player_with_friends("player-4", &[], None, Some("client-4")),
    ];
    let result = assign_teams(&players, &two_teams(), false, None);
    assert_eq!(result[0], result[1]);
    assert_ne!(result[2], result[0]);
    assert_ne!(result[3], result[0]);
}

// TS: TeamAssignment.test.ts:316
#[test]
fn should_keep_duo_friends_together_when_a_stranger_joined_first() {
    let players = [
        create_player_with_friends("s1", &[], None, None),
        create_player_with_friends("a", &["b"], None, None),
        create_player_with_friends("b", &["a"], None, None),
        create_player_with_friends("s2", &[], None, None),
        create_player_with_friends("s3", &[], None, None),
        create_player_with_friends("s4", &[], None, None),
    ];
    let result = assign_teams(&players, &duo_teams(), true, None);
    assert_eq!(result[1], result[2]);
    assert_ne!(result[0], result[1]);
}

// TS: TeamAssignment.test.ts:332
#[test]
fn should_keep_duo_friends_together_when_they_joined_far_apart() {
    let players = [
        create_player_with_friends("a", &["b"], None, None),
        create_player_with_friends("s1", &[], None, None),
        create_player_with_friends("s2", &[], None, None),
        create_player_with_friends("b", &["a"], None, None),
        create_player_with_friends("s3", &[], None, None),
        create_player_with_friends("s4", &[], None, None),
    ];
    let result = assign_teams(&players, &duo_teams(), true, None);
    assert_eq!(result[0], result[3]);
}

// TS: TeamAssignment.test.ts:347
#[test]
fn should_seat_a_friend_group_on_a_team_with_room_for_all_of_them() {
    // A lone clan member already holds Red, so the trio must start on an empty team.
    let players = [
        create_player_with_friends("c1", &[], Some("CLANA"), None),
        create_player_with_friends("a", &["b", "c"], None, None),
        create_player_with_friends("b", &[], None, None),
        create_player_with_friends("c", &[], None, None),
        create_player_with_friends("s1", &[], None, None),
        create_player_with_friends("s2", &[], None, None),
    ];
    let result = assign_teams(&players, &two_teams(), true, Some(3));
    assert_eq!(result[1], result[2]);
    assert_eq!(result[1], result[3]);
    assert_ne!(result[0], result[1]);
}

// TS: TeamAssignment.test.ts:366
#[test]
fn should_seat_a_clan_members_friend_before_an_unrelated_friend_group() {
    // Clan member c holds Red. f (c's friend) is seated before the a/b pair.
    let players = [
        create_player_with_friends("c", &[], Some("CLANA"), None),
        create_player_with_friends("f", &["c"], None, None),
        create_player_with_friends("a", &["b"], None, None),
        create_player_with_friends("b", &["a"], None, None),
        create_player_with_friends("s1", &[], None, None),
        create_player_with_friends("s2", &[], None, None),
    ];
    let result = assign_teams(&players, &two_teams(), true, Some(3));
    assert_eq!(result[1], result[0]);
    assert_eq!(result[3], result[2]);
}

// TS: TeamAssignment.test.ts:405
#[test]
fn should_honor_pinned_team_index_exactly_matchmade_2v2() {
    let players = [
        create_pinned_player("1", Some(0), None, &[]),
        create_pinned_player("2", Some(1), None, &[]),
        create_pinned_player("3", Some(1), None, &[]),
        create_pinned_player("4", Some(0), None, &[]),
    ];
    let result = assign_teams(&players, &two_teams(), false, None);
    assert_eq!(result[0], on_team(colored_teams::RED));
    assert_eq!(result[1], on_team(colored_teams::BLUE));
    assert_eq!(result[2], on_team(colored_teams::BLUE));
    assert_eq!(result[3], on_team(colored_teams::RED));
}

// TS: TeamAssignment.test.ts:421
#[test]
fn should_let_pins_override_clan_grouping() {
    // Two clanmates matched onto opposite teams stay split.
    let players = [
        create_pinned_player("1", Some(0), Some("CLANA"), &[]),
        create_pinned_player("2", Some(1), Some("CLANA"), &[]),
        create_pinned_player("3", Some(1), None, &[]),
        create_pinned_player("4", Some(0), None, &[]),
    ];
    let result = assign_teams(&players, &two_teams(), false, None);
    assert_eq!(result[0], on_team(colored_teams::RED));
    assert_eq!(result[1], on_team(colored_teams::BLUE));
}

// TS: TeamAssignment.test.ts:437
#[test]
fn should_balance_unpinned_players_around_pinned_ones() {
    // Red already holds two pinned players (at capacity for 4/2), so the unpinned player lands on Blue.
    let players = [
        create_pinned_player("1", Some(0), None, &[]),
        create_pinned_player("2", Some(0), None, &[]),
        create_pinned_player("3", Some(1), None, &[]),
        create_pinned_player("4", None, None, &[]),
    ];
    let result = assign_teams(&players, &two_teams(), false, None);
    assert_eq!(result[3], on_team(colored_teams::BLUE));
}

// TS: TeamAssignment.test.ts:452
#[test]
fn should_treat_an_out_of_range_team_index_as_unpinned() {
    let players = [
        create_pinned_player("1", Some(7), None, &[]),
        create_pinned_player("2", None, None, &[]),
        create_pinned_player("3", None, None, &[]),
        create_pinned_player("4", None, None, &[]),
    ];
    let result = assign_teams(&players, &two_teams(), false, None);
    for slot in &result {
        assert!(matches!(slot, TeamSlot::Team(t) if t == colored_teams::RED || t == colored_teams::BLUE), "{slot:?}");
    }
}

// TS: TeamAssignment.test.ts:467
#[test]
fn should_pull_an_unpinned_friend_toward_a_pinned_players_team() {
    let players = [
        create_pinned_player("1", Some(0), None, &[]),
        create_pinned_player("2", None, None, &["1"]),
        create_pinned_player("3", None, None, &[]),
        create_pinned_player("4", None, None, &[]),
    ];
    let result = assign_teams(&players, &two_teams(), false, None);
    assert_eq!(result[1], on_team(colored_teams::RED));
}

// TS: TeamAssignment.test.ts:480
#[test]
fn should_honor_pins_even_past_max_team_size_trust_the_matcher() {
    let players = [
        create_pinned_player("1", Some(0), None, &[]),
        create_pinned_player("2", Some(0), None, &[]),
        create_pinned_player("3", Some(0), None, &[]),
    ];
    let result = assign_teams(&players, &two_teams(), false, None);
    assert_eq!(result[0], on_team(colored_teams::RED));
    assert_eq!(result[1], on_team(colored_teams::RED));
    assert_eq!(result[2], on_team(colored_teams::RED));
}

/// Body of the duos/trios/quads `it` at TeamAssignment.test.ts:504 (`mode.size` humans, then nations).
fn assert_humans_pack_one_team(size: u32) {
    let humans: Vec<PlayerInfo> = (0..size).map(|i| create_player(&(i + 1).to_string(), None)).collect();
    let nations: Vec<PlayerInfo> = (0..size).map(|i| create_nation(&(i + 1).to_string())).collect();
    let n_humans = humans.len();
    let mut players = humans;
    players.extend(nations);
    let result = assign_teams(&players, &two_teams(), true, Some(size));
    for slot in &result[..n_humans] {
        assert_eq!(*slot, on_team(colored_teams::RED));
    }
    for slot in &result[n_humans..] {
        assert_eq!(*slot, on_team(colored_teams::BLUE));
    }
}

// TS: TeamAssignment.test.ts:504
#[test]
fn should_fill_one_team_with_humans_before_putting_nations_on_the_other_duos() {
    assert_humans_pack_one_team(2);
}

// TS: TeamAssignment.test.ts:504
#[test]
fn should_fill_one_team_with_humans_before_putting_nations_on_the_other_trios() {
    assert_humans_pack_one_team(3);
}

// TS: TeamAssignment.test.ts:504
#[test]
fn should_fill_one_team_with_humans_before_putting_nations_on_the_other_quads() {
    assert_humans_pack_one_team(4);
}

// TS: TeamAssignment.test.ts:524
#[test]
fn should_fill_team_gaps_with_humans_before_placing_nations_trios() {
    // 4 humans + 2 nations, 2 teams, maxTeamSize 3.
    let humans: Vec<PlayerInfo> = (0..4).map(|i| create_player(&(i + 1).to_string(), None)).collect();
    let nations: Vec<PlayerInfo> = (0..2).map(|i| create_nation(&(i + 1).to_string())).collect();
    let n_humans = humans.len();
    let mut players = humans;
    players.extend(nations);
    let result = assign_teams(&players, &two_teams(), true, Some(3));
    let red_humans = result[..n_humans].iter().filter(|s| **s == on_team(colored_teams::RED)).count();
    let blue_humans = result[..n_humans].iter().filter(|s| **s == on_team(colored_teams::BLUE)).count();
    assert_eq!(red_humans, 3);
    assert_eq!(blue_humans, 1);
    for slot in &result[n_humans..] {
        assert_eq!(*slot, on_team(colored_teams::BLUE));
    }
}

// --- resolveTeamsList ---

// TS: TeamAssignment.test.ts:550
#[test]
fn derives_the_team_count_from_attendance_for_named_modes() {
    assert_eq!(resolve_teams_list(TeamCountConfig::Duos, 10).len(), 5);
    assert_eq!(resolve_teams_list(TeamCountConfig::Trios, 9).len(), 3);
    assert_eq!(resolve_teams_list(TeamCountConfig::Quads, 12).len(), 3);
}

/// Body of the named-mode clamp `it` at TeamAssignment.test.ts:565. Rows are `[0, 1, size]`.
fn assert_named_mode_clamps(config: TeamCountConfig, size: u32) {
    for total_players in [0, 1, size] {
        assert_eq!(resolve_teams_list(config, total_players), two_teams());
    }
}

// TS: TeamAssignment.test.ts:565
#[test]
fn clamps_duos_to_2_teams_when_attendance_cant_fill_two() {
    assert_named_mode_clamps(TeamCountConfig::Duos, 2);
}

// TS: TeamAssignment.test.ts:565
#[test]
fn clamps_trios_to_2_teams_when_attendance_cant_fill_two() {
    assert_named_mode_clamps(TeamCountConfig::Trios, 3);
}

// TS: TeamAssignment.test.ts:565
#[test]
fn clamps_quads_to_2_teams_when_attendance_cant_fill_two() {
    assert_named_mode_clamps(TeamCountConfig::Quads, 4);
}

// TS: TeamAssignment.test.ts:575
#[test]
fn always_resolves_humans_vs_nations_to_its_two_fixed_teams() {
    assert_eq!(
        resolve_teams_list(TeamCountConfig::HumansVsNations, 0),
        [colored_teams::HUMANS.to_owned(), colored_teams::NATIONS.to_owned()]
    );
}

// TS: TeamAssignment.test.ts:582
#[test]
fn keeps_numeric_configs_verbatim_independent_of_attendance() {
    assert_eq!(resolve_teams_list(TeamCountConfig::Count(7), 3).len(), 7);
}

// TS: TeamAssignment.test.ts:586
#[test]
fn still_rejects_a_numeric_config_below_2_misconfiguration() {
    // A Team game with playerTeams unset resolves to 0 via Config.playerTeams().
    assert_throws_containing(|| drop(resolve_teams_list(TeamCountConfig::Count(0), 50)), "Too few teams: 0");
    assert_throws_containing(|| drop(resolve_teams_list(TeamCountConfig::Count(1), 50)), "Too few teams: 1");
}

// --- humanTeams ---

// TS: TeamAssignment.test.ts:598
#[test]
fn matches_assign_teams_over_humans_and_nations_together() {
    let humans = vec![human(0, Some("AAA")), human(1, None), human(2, Some("AAA")), human(3, None), human(4, None)];
    let nations: Vec<PlayerInfo> =
        (0..3).map(|i| PlayerInfo::new(format!("n{i}"), PlayerType::Nation, None, format!("n{i}"))).collect();
    let teams = resolve_teams_list(TeamCountConfig::Count(3), (humans.len() + nations.len()) as u32);
    let mut all_players = humans.clone();
    all_players.extend(nations);
    let all = assign_teams(&all_players, &teams, false, None);
    let got = human_teams(&humans, TeamCountConfig::Count(3), 3);
    // `all.get(h)` is `Team | "kicked"`. `"kicked"` is not `undefined`, so a kick must not compare equal.
    let expected: Vec<Option<String>> = all[..humans.len()]
        .iter()
        .map(|s| match s {
            TeamSlot::Team(t) => Some(t.clone()),
            TeamSlot::Kicked => Some("kicked".to_owned()),
        })
        .collect();
    assert_eq!(got, expected);
}

// TS: TeamAssignment.test.ts:616
#[test]
fn leaves_kicked_clan_overflow_players_without_a_team() {
    let humans: Vec<PlayerInfo> = (0..4).map(|i| human(i, Some("AAA"))).collect();
    // 4 players, 2 teams of 2: half the clan doesn't fit.
    assert_eq!(
        human_teams(&humans, TeamCountConfig::Count(2), 0),
        vec![Some(colored_teams::RED.to_owned()), Some(colored_teams::RED.to_owned()), None, None]
    );
}

// TS: TeamAssignment.test.ts:627
#[test]
fn puts_every_human_on_humans_in_humans_vs_nations() {
    assert_eq!(
        human_teams(&[human(0, None), human(1, Some("AAA"))], TeamCountConfig::HumansVsNations, 9),
        vec![Some(colored_teams::HUMANS.to_owned()), Some(colored_teams::HUMANS.to_owned())]
    );
}

// TS: TeamAssignment.test.ts:633
#[test]
fn throws_as_the_game_does_for_a_team_config_below_2_teams() {
    assert_throws_containing(|| drop(human_teams(&[human(0, None)], TeamCountConfig::Count(0), 0)), "Too few teams: 0");
}

// --- adjustForTeams ---

// TS: TeamAssignment.test.ts:639
#[test]
fn does_not_change_configured_cap_for_numeric_team_counts() {
    assert_eq!(adjust_for_teams(5.0, Some(TeamCountConfig::Count(3))), 5.0);
    assert_eq!(adjust_for_teams(10.0, Some(TeamCountConfig::Count(4))), 10.0);
}

// TS: TeamAssignment.test.ts:644
#[test]
fn does_not_change_cap_for_undefined_or_humans_vs_nations_or_count_le_0() {
    assert_eq!(adjust_for_teams(5.0, None), 5.0);
    assert_eq!(adjust_for_teams(5.0, Some(TeamCountConfig::HumansVsNations)), 5.0);
    assert_eq!(adjust_for_teams(0.0, Some(TeamCountConfig::Trios)), 0.0);
    assert_eq!(adjust_for_teams(-1.0, Some(TeamCountConfig::Trios)), -1.0);
}

// TS: TeamAssignment.test.ts:651
#[test]
fn adjusts_to_multiples_of_team_size_and_enforces_at_least_one_full_team() {
    assert_eq!(adjust_for_teams(2.0, Some(TeamCountConfig::Trios)), 3.0);
    assert_eq!(adjust_for_teams(3.0, Some(TeamCountConfig::Trios)), 3.0);
    assert_eq!(adjust_for_teams(4.0, Some(TeamCountConfig::Trios)), 3.0);
    assert_eq!(adjust_for_teams(5.0, Some(TeamCountConfig::Trios)), 3.0);
    assert_eq!(adjust_for_teams(6.0, Some(TeamCountConfig::Trios)), 6.0);
    assert_eq!(adjust_for_teams(1.0, Some(TeamCountConfig::Duos)), 2.0);
    assert_eq!(adjust_for_teams(5.0, Some(TeamCountConfig::Duos)), 4.0);
    assert_eq!(adjust_for_teams(2.0, Some(TeamCountConfig::Quads)), 4.0);
}
