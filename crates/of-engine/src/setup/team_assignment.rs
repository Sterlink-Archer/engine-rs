//! `engine-lib/src/game/TeamAssignment.ts`. Owner: **P3**. Local PRNG seeded
//! `simpleHash(nationPlayers[0].id)` (`:158`); sorts via `js_sort_by` (`:53, :224`).
//!
//! TS keys its maps by `PlayerInfo` object identity; the port keys them by the player's
//! index in `players`. The result `Map<PlayerInfo, Team | "kicked">` is iterated by
//! `GameImpl.addPlayers` (`GameImpl.ts:250-263`), and its **insertion order** (pinned players,
//! then clans in size order, then friend groups, then the shuffled nations) decides the
//! `addPlayer` order and so every smallID: [`assign_teams_ordered`] returns that order;
//! [`assign_teams`] (frozen S0 signature) returns one slot per player in `players` order.
//!
//! Containers (PORTING_GUIDE §5.1): `teamPlayerCount`, `presentClientIDs`, `teamByClientID`
//! and `byClientID` are lookup-only (never iterated: grep `teamPlayerCount\|teamByClientID`);
//! `clanGroups` and `friendGraph` (and its sets) are iterated, so they keep insertion order
//! (`Vec`s with Map/Set `set`/`add` semantics; setup-time sizes are small).

use crate::api::game_config::TeamCountConfig;
use crate::api::types::{ClientID, PlayerInfo, PlayerType, Team, colored_teams};
use crate::jsorder::LookupMap;
use of_det::{Prng, js, js_sort_by};

/// `Team | "kicked"`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TeamSlot {
    Team(Team),
    Kicked,
}

/// `Map.set(k, v)` on an insertion-ordered `Vec<(K, V)>`: an existing key keeps its position.
fn map_set<K: PartialEq, V>(m: &mut Vec<(K, V)>, k: K, v: V) {
    match m.iter_mut().find(|(x, _)| *x == k) {
        Some(e) => e.1 = v,
        None => m.push((k, v)),
    }
}

/// `friendGraph: Map<ClientID, Set<ClientID>>` (`:86-102`), insertion-ordered.
#[derive(Default)]
struct FriendGraph {
    nodes: Vec<(ClientID, Vec<ClientID>)>,
}
impl FriendGraph {
    fn get(&self, a: &str) -> Option<&[ClientID]> {
        self.nodes.iter().find(|(k, _)| k == a).map(|(_, s)| s.as_slice())
    }
    /// `addEdge(a, b)` (`:87-94`).
    fn add_edge(&mut self, a: &str, b: &str) {
        let s = match self.nodes.iter().position(|(k, _)| k == a) {
            Some(i) => &mut self.nodes[i].1,
            None => {
                self.nodes.push((a.to_owned(), Vec::new()));
                &mut self.nodes.last_mut().expect("just pushed").1
            }
        };
        if !s.iter().any(|x| x == b) {
            s.push(b.to_owned());
        }
    }
}

/// `assignTeams(players, teams, isDuosTriosQuads, maxTeamSize?)` (`:15-225`): one slot per
/// player, in `players` order (use [`assign_teams_ordered`] for the TS Map order).
pub fn assign_teams(
    players: &[PlayerInfo],
    teams: &[Team],
    is_duos_trios_quads: bool,
    max_team_size: Option<u32>,
) -> Vec<TeamSlot> {
    let mut out = vec![TeamSlot::Kicked; players.len()];
    for (i, slot) in assign_teams_ordered(players, teams, is_duos_trios_quads, max_team_size) {
        out[i] = slot; // in-range: i indexes players
    }
    out
}

/// `assignTeams` with the result in the TS `Map` insertion order: `(index into players, slot)`.
/// Every player appears exactly once.
pub fn assign_teams_ordered(
    players: &[PlayerInfo],
    teams: &[Team],
    is_duos_trios_quads: bool,
    max_team_size: Option<u32>,
) -> Vec<(usize, TeamSlot)> {
    // TS default parameter: `getMaxTeamSize(players.length, teams.length)` (Infinity/NaN for 0 teams).
    let max_team_size = match max_team_size {
        Some(m) => f64::from(m),
        None => max_team_size_f64(players.len() as f64, teams.len() as f64),
    };
    assign_teams_impl(players, teams, is_duos_trios_quads, max_team_size)
}

fn assign_teams_impl(
    players: &[PlayerInfo],
    teams: &[Team],
    is_duos_trios_quads: bool,
    max_team_size: f64,
) -> Vec<(usize, TeamSlot)> {
    let mut result: Vec<(usize, TeamSlot)> = Vec::new();
    // lookup-only: get/set by team (never iterated)
    let mut team_player_count: LookupMap<Team, f64> = LookupMap::new();
    let count = |m: &LookupMap<Team, f64>, t: &Team| m.get(t).copied().unwrap_or(0.0);

    // TS: `:28-37` pinned slots first
    let mut unpinned: Vec<usize> = Vec::new();
    for (i, p) in players.iter().enumerate() {
        let pinned = p.team_index.and_then(|ti| teams.get(ti as usize));
        let Some(pinned) = pinned else {
            unpinned.push(i);
            continue;
        };
        map_set(&mut result, i, TeamSlot::Team(pinned.clone()));
        let c = count(&team_player_count, pinned) + 1.0;
        team_player_count.insert(pinned.clone(), c);
    }

    // TS: `:42-51` clans (`if (p.clanTag)`: a non-empty string)
    let mut clan_groups: Vec<(String, Vec<usize>)> = Vec::new();
    let mut non_clan_players: Vec<usize> = Vec::new();
    for &i in &unpinned {
        match players[i].clan_tag.as_deref() {
            Some(tag) if !tag.is_empty() => match clan_groups.iter_mut().find(|(k, _)| k == tag) {
                Some(g) => g.1.push(i),
                None => clan_groups.push((tag.to_owned(), vec![i])),
            },
            _ => non_clan_players.push(i),
        }
    }

    // TS: `:53-55` `Array.from(clanGroups.values()).sort((a, b) => b.length - a.length)`
    let mut sorted_clans: Vec<Vec<usize>> = clan_groups.into_iter().map(|(_, v)| v).collect();
    js_sort_by(&mut sorted_clans, |a, b| b.len() as f64 - a.len() as f64);
    for clan in &sorted_clans {
        let mut team: Option<&Team> = None;
        let mut team_size = 0.0;
        for t in teams {
            let p = count(&team_player_count, t);
            if team.is_some() && team_size <= p {
                continue;
            }
            team_size = p;
            team = Some(t);
        }
        let Some(team) = team else { continue };
        for &player in clan {
            if team_size < max_team_size {
                team_size += 1.0;
                map_set(&mut result, player, TeamSlot::Team(team.clone()));
            } else {
                map_set(&mut result, player, TeamSlot::Kicked);
            }
        }
        team_player_count.insert(team.clone(), team_size);
    }

    // TS: `:82-102` the friend graph over present clients
    let present: Vec<&str> = players.iter().filter_map(|p| p.client_id.as_deref()).collect(); // lookup-only
    let mut friend_graph = FriendGraph::default();
    for p in players {
        let Some(cid) = p.client_id.as_deref() else { continue };
        for friend_id in &p.friends {
            if !present.contains(&friend_id.as_str()) {
                continue;
            }
            friend_graph.add_edge(cid, friend_id);
            friend_graph.add_edge(friend_id, cid);
        }
    }

    // TS: `:104-109` lookup-only
    let mut team_by_client_id: LookupMap<ClientID, Team> = LookupMap::new();
    for (i, slot) in &result {
        if let (Some(cid), TeamSlot::Team(t)) = (players[*i].client_id.as_ref(), slot) {
            team_by_client_id.insert(cid.clone(), t.clone());
        }
    }

    // TS: `:115-152` placePlayer(p, groupLeft)
    let place_player = |pi: usize,
                        group_left: f64,
                        result: &mut Vec<(usize, TeamSlot)>,
                        team_player_count: &mut LookupMap<Team, f64>,
                        team_by_client_id: &mut LookupMap<ClientID, Team>| {
        let p = &players[pi];
        let my_friends = p.client_id.as_deref().and_then(|c| friend_graph.get(c));
        let mut best_team: Option<&Team> = None;
        let mut best_friend_count = -1.0;
        let mut best_group_seats = -1.0;
        let mut best_size = if is_duos_trios_quads { -1.0 } else { f64::INFINITY };
        for t in teams {
            let size = count(team_player_count, t);
            if size >= max_team_size {
                continue;
            }
            let mut friends_on_team = 0.0;
            if let Some(fr) = my_friends {
                for friend_id in fr {
                    if team_by_client_id.get(friend_id) == Some(t) {
                        friends_on_team += 1.0;
                    }
                }
            }
            let group_seats = js::min(group_left, max_team_size - size);
            if friends_on_team > best_friend_count
                || (friends_on_team == best_friend_count
                    && (group_seats > best_group_seats
                        || (group_seats == best_group_seats
                            && (if is_duos_trios_quads { size > best_size } else { size < best_size }))))
            {
                best_friend_count = friends_on_team;
                best_group_seats = group_seats;
                best_size = size;
                best_team = Some(t);
            }
        }
        let Some(best) = best_team else {
            map_set(result, pi, TeamSlot::Kicked);
            return;
        };
        let c = count(team_player_count, best) + 1.0;
        team_player_count.insert(best.clone(), c);
        map_set(result, pi, TeamSlot::Team(best.clone()));
        if let Some(cid) = &p.client_id {
            team_by_client_id.insert(cid.clone(), best.clone());
        }
    };

    // TS: `:154-163`
    let mut nation_players: Vec<usize> =
        non_clan_players.iter().copied().filter(|&i| players[i].player_type == PlayerType::Nation).collect();
    if let Some(&first) = nation_players.first() {
        let mut random = Prng::new(of_det::util::simple_hash(&players[first].id));
        nation_players = random.shuffle_array(&nation_players);
    }
    let other_players: Vec<usize> =
        non_clan_players.iter().copied().filter(|&i| players[i].player_type != PlayerType::Nation).collect();

    // TS: `:169-174`
    for group in friend_groups(players, &other_players, &friend_graph, &team_by_client_id) {
        let n = group.len();
        for (i, &p) in group.iter().enumerate() {
            place_player(p, (n - i) as f64, &mut result, &mut team_player_count, &mut team_by_client_id);
        }
    }
    for &p in &nation_players {
        place_player(p, 1.0, &mut result, &mut team_player_count, &mut team_by_client_id);
    }
    result
}

/// `friendGroups(players, friendGraph, seated)` (`:184-225`): groups connected by friend
/// edges, each in reach order; anchored groups first, then larger groups (stable sort).
fn friend_groups(
    all: &[PlayerInfo],
    players: &[usize],
    friend_graph: &FriendGraph,
    seated: &LookupMap<ClientID, Team>,
) -> Vec<Vec<usize>> {
    // lookup-only; a later player with the same clientID overwrites (`Map.set`)
    let mut by_client_id: LookupMap<&str, usize> = LookupMap::new();
    for &p in players {
        if let Some(c) = all[p].client_id.as_deref() {
            by_client_id.insert(c, p);
        }
    }
    let has_seated_friend = |p: usize| -> bool {
        let Some(c) = all[p].client_id.as_deref() else { return false };
        friend_graph.get(c).unwrap_or(&[]).iter().any(|f| seated.get(f).is_some())
    };
    let anchored: Vec<usize> = players.iter().copied().filter(|&p| has_seated_friend(p)).collect();
    let starts: Vec<usize> =
        anchored.iter().copied().chain(players.iter().copied().filter(|&p| !has_seated_friend(p))).collect();
    // `grouped: Set<PlayerInfo>` by identity = by index
    let mut grouped = vec![false; all.len()];
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for &start in &starts {
        if grouped[start] {
            continue;
        }
        grouped[start] = true;
        let mut group = vec![start];
        let mut i = 0;
        // iter-live: `group` grows inside the loop (`for (let i = 0; i < group.length; i++)`)
        while i < group.len() {
            if let Some(cid) = all[group[i]].client_id.as_deref() {
                for friend_id in friend_graph.get(cid).unwrap_or(&[]) {
                    let Some(&friend) = by_client_id.get(&friend_id.as_str()) else { continue };
                    if grouped[friend] {
                        continue;
                    }
                    grouped[friend] = true;
                    group.push(friend);
                }
            }
            i += 1;
        }
        groups.push(group);
    }
    let rank = |g: &Vec<usize>| if anchored.contains(&g[0]) { 1.0 } else { 0.0 };
    // TS: `rank(b) - rank(a) || b.length - a.length` (`||` on numbers: 0 falls through)
    js_sort_by(&mut groups, |a, b| js::or_num(rank(b) - rank(a), b.len() as f64 - a.len() as f64));
    groups
}

/// `assignTeamsLobbyPreview(players, teams, teamCount, nationCount)` (`:227-243`).
pub fn assign_teams_lobby_preview(
    players: &[PlayerInfo],
    teams: &[Team],
    team_count: TeamCountConfig,
    nation_count: u32,
) -> Vec<TeamSlot> {
    let max_team_size = max_team_size_f64(players.len() as f64 + f64::from(nation_count), teams.len() as f64);
    let ordered = assign_teams_impl(
        players,
        teams,
        matches!(team_count, TeamCountConfig::Duos | TeamCountConfig::Trios | TeamCountConfig::Quads),
        max_team_size,
    );
    let mut out = vec![TeamSlot::Kicked; players.len()];
    for (i, s) in ordered {
        out[i] = s;
    }
    out
}

/// `humanTeams(humans, playerTeams, nationCount)` (`:251-271`): `None` for a kicked human.
/// Throws as the game does when the config resolves to too few teams.
pub fn human_teams(humans: &[PlayerInfo], player_teams: TeamCountConfig, nation_count: u32) -> Vec<Option<Team>> {
    if player_teams == TeamCountConfig::HumansVsNations {
        return humans.iter().map(|_| Some(colored_teams::HUMANS.to_owned())).collect();
    }
    let total_players = humans.len() as f64 + f64::from(nation_count);
    // int: player counts are far below 2^32
    let teams = resolve_teams_list(player_teams, total_players as u32);
    let ordered = assign_teams_impl(
        humans,
        &teams,
        matches!(player_teams, TeamCountConfig::Duos | TeamCountConfig::Trios | TeamCountConfig::Quads),
        max_team_size_f64(total_players, teams.len() as f64),
    );
    let mut out = vec![None; humans.len()];
    for (i, s) in ordered {
        out[i] = match s {
            TeamSlot::Team(t) => Some(t),
            TeamSlot::Kicked => None,
        };
    }
    out
}

/// `Math.ceil(numPlayers / numTeams)` in f64 (Infinity for 0 teams, NaN for 0/0).
fn max_team_size_f64(num_players: f64, num_teams: f64) -> f64 {
    js::ceil(num_players / num_teams)
}

/// `getMaxTeamSize(numPlayers, numTeams)` = `Math.ceil(numPlayers / numTeams)` (`:273-275`).
/// `numTeams = 0` gives TS `Infinity`/`NaN`; here `u32::MAX` (only reachable through
/// [`assign_teams`]'s default, which uses the f64 value directly).
pub fn get_max_team_size(num_players: u32, num_teams: u32) -> u32 {
    if num_teams == 0 {
        return u32::MAX;
    }
    num_players.div_ceil(num_teams)
}

/// `resolveTeamsList(config, totalPlayers)` (`:277-318`).
pub fn resolve_teams_list(config: TeamCountConfig, total_players: u32) -> Vec<Team> {
    let num_teams: f64 = match config {
        TeamCountConfig::HumansVsNations => {
            return vec![colored_teams::HUMANS.to_owned(), colored_teams::NATIONS.to_owned()];
        }
        TeamCountConfig::Count(n) => n as f64, // zod uint
        preset => {
            let divisor = match preset {
                TeamCountConfig::Duos => 2.0,
                TeamCountConfig::Trios => 3.0,
                _ => 4.0, // Quads
            };
            js::max(2.0, js::ceil(f64::from(total_players) / divisor))
        }
    };
    if num_teams < 2.0 {
        of_det::ts_throw!("Too few teams: {}", of_det::throw::js_number_to_string(num_teams));
    }
    if num_teams < 8.0 {
        let mut teams = vec![colored_teams::RED.to_owned(), colored_teams::BLUE.to_owned()];
        if num_teams >= 3.0 {
            teams.push(colored_teams::YELLOW.to_owned());
        }
        if num_teams >= 4.0 {
            teams.push(colored_teams::GREEN.to_owned());
        }
        if num_teams >= 5.0 {
            teams.push(colored_teams::PURPLE.to_owned());
        }
        if num_teams >= 6.0 {
            teams.push(colored_teams::ORANGE.to_owned());
        }
        if num_teams >= 7.0 {
            teams.push(colored_teams::TEAL.to_owned());
        }
        return teams;
    }
    // int: a zod uint team count (or ceil of a player count)
    (0..num_teams as u64).map(|i| format!("Team {}", i + 1)).collect()
}

/// `adjustForTeams(count, teams)` (`:320-329`).
pub fn adjust_for_teams(count: f64, teams: Option<TeamCountConfig>) -> f64 {
    if count <= 0.0 {
        return count;
    }
    let size = match teams {
        Some(TeamCountConfig::Duos) => 2.0,
        Some(TeamCountConfig::Trios) => 3.0,
        Some(TeamCountConfig::Quads) => 4.0,
        _ => return count,
    };
    let rounded = count - (count % size);
    js::max(size, rounded)
}

#[cfg(test)]
#[path = "team_assignment_tests.rs"]
mod tests;
