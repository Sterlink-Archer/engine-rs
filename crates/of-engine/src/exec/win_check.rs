//! `WinCheckExecution` (`engine/src/execution/WinCheckExecution.ts`). Owner: **P6**.
//!
//! Fields: the `WinCheck v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1). `setWinner(winner,
//! stats().stats())`: the stats object is output only (ARCHITECTURE §3.10), so the port passes
//! the winner alone (`Ctx::set_winner`). `console.log` lines are dropped.

#[allow(unused_imports)]
use crate::exec::prelude::*;

use crate::api::types::{GameMode, RankedType, Team, colored_teams};
use crate::state::game::Winner;

/// `HARD_TIME_LIMIT_SECONDS = 170 * 60` (`:22`).
const HARD_TIME_LIMIT_SECONDS: f64 = 170.0 * 60.0;

/// `WinCheckExecution` data.
#[derive(StateClone, Debug)]
pub struct WinCheckExec {
    pub active: bool,
    pub initialized: bool,
    pub checked_ranked_spawns: bool,
}

/// `isRankedTeamGame(rankedType)` (`:238-242`).
fn is_ranked_team_game(ranked_type: Option<RankedType>) -> bool {
    ranked_type == Some(RankedType::TwoVTwo) || ranked_type == Some(RankedType::ClanWars)
}

#[allow(clippy::new_without_default)] // a TS no-argument constructor; `Default` would be a placeholder
impl WinCheckExec {
    /// `new WinCheckExecution()`: `active = true`, `mg = null`, `checkedRankedSpawns = false`.
    pub fn new() -> Self {
        WinCheckExec { active: true, initialized: false, checked_ranked_spawns: false }
    }

    /// TS: `if (this.mg === null) throw new Error("Not initialized")`.
    #[inline]
    fn require_init(&self) {
        if !self.initialized {
            of_det::ts_throw!("Not initialized");
        }
    }

    /// `checkRankedTeamCancelled()` (`:53-78`).
    fn check_ranked_team_cancelled(&mut self, cx: &mut Ctx) -> bool {
        self.require_init();
        if self.checked_ranked_spawns {
            return false;
        }
        self.checked_ranked_spawns = true;
        let gc = cx.st.config().game_config();
        if !is_ranked_team_game(gc.ranked_type) {
            return false;
        }
        // TS: :65-67 (allPlayers: tile-less players included)
        let all = cx.st.all_players();
        let spawned = all
            .iter()
            .filter(|&&p| {
                let pr = cx.st.player(p);
                pr.player_type() == PlayerType::Human && pr.has_spawned()
            })
            .count() as f64;
        // TS: :68 `gameConfig.maxPlayers ?? 0`
        let expected = gc.max_players.map_or(0.0, |m| m as f64);
        if spawned >= expected {
            return false;
        }
        cx.set_winner(Winner::None);
        self.active = false;
        true
    }

    /// `checkWinnerFFA()` (`:80-133`).
    pub fn check_winner_ffa(&mut self, cx: &mut Ctx) {
        self.require_init();
        // TS: :82-84 `players().sort((a, b) => b.numTilesOwned() - a.numTilesOwned())`
        let mut sorted = cx.st.players();
        {
            let st = &*cx.st;
            of_det::js_sort_by(&mut sorted, |&a, &b| {
                f64::from(st.player(b).num_tiles_owned()) - f64::from(st.player(a).num_tiles_owned())
            });
        }
        if sorted.is_empty() {
            return;
        }

        // TS: :91-97 (the tutorial is won by conquering its two nations, nothing else ends it)
        if cx.st.config().game_config().tutorial == Some(true) {
            if sorted.iter().all(|&p| cx.st.player(p).player_type() == PlayerType::Human) {
                cx.set_winner(Winner::Player(sorted[0]));
                self.active = false;
            }
            return;
        }

        // TS: :101-115 (ranked 1v1 and FFA end when one connected human is left)
        let ranked_type = cx.st.config().game_config().ranked_type;
        if ranked_type == Some(RankedType::OneVOne) || ranked_type == Some(RankedType::FreeForAll) {
            let humans: Vec<SmallId> = sorted
                .iter()
                .copied()
                .filter(|&p| {
                    let pr = cx.st.player(p);
                    pr.player_type() == PlayerType::Human && !pr.is_disconnected()
                })
                .collect();
            if humans.len() == 1 {
                cx.set_winner(Winner::Player(humans[0]));
                self.active = false;
                return;
            }
        }

        // TS: :120-123 (a bot can't win ranked FFA)
        let contenders: Vec<SmallId> = if ranked_type == Some(RankedType::FreeForAll) {
            sorted.iter().copied().filter(|&p| cx.st.player(p).player_type() == PlayerType::Human).collect()
        } else {
            sorted
        };
        let Some(&max) = contenders.first() else {
            return;
        };
        if self.has_won(cx, f64::from(cx.st.player(max).num_tiles_owned())) {
            cx.set_winner(Winner::Player(max));
            self.active = false;
        }
    }

    /// `hasWon(tilesOwned)` (`:137-161`).
    fn has_won(&self, cx: &Ctx, tiles_owned: f64) -> bool {
        self.require_init();
        let st = &*cx.st;
        let time_elapsed = st.elapsed_game_seconds();
        // TS: :141-148 (null = no timer, same as undefined)
        if let Some(Some(max_timer_value)) = st.config().game_config().max_timer_value
            && time_elapsed >= max_timer_value as f64 * 60.0
        {
            return true;
        }
        if time_elapsed >= HARD_TIME_LIMIT_SECONDS {
            return true;
        }
        let num_tiles_without_fallout = f64::from(st.map.num_land_tiles()) - f64::from(st.num_tiles_with_fallout());
        // TS: :156-160 (cross-multiplied; exact in f64 at these magnitudes)
        tiles_owned * 1000.0
            > num_tiles_without_fallout * st.config().percentage_tiles_owned_to_win_tenths(time_elapsed)
    }

    /// `checkWinnerTeam()` (`:163-211`).
    pub fn check_winner_team(&mut self, cx: &mut Ctx) {
        self.require_init();

        if is_ranked_team_game(cx.st.config().game_config().ranked_type) {
            // TS: :169-185 `teamsRemaining = new Set<Team>()` (insertion order; `[winner]` is
            // the first element). Local, append-only: a Vec with a membership check.
            let mut teams_remaining: Vec<Team> = Vec::new();
            for p in cx.st.players() {
                let pr = cx.st.player(p);
                if pr.player_type() != PlayerType::Human || pr.is_disconnected() {
                    continue;
                }
                if let Some(team) = pr.team()
                    && !teams_remaining.contains(team)
                {
                    teams_remaining.push(team.clone());
                }
            }
            if teams_remaining.len() == 1 {
                let winner = teams_remaining.swap_remove(0);
                cx.set_winner(Winner::Team(winner));
                self.active = false;
                return;
            }
        }

        // TS: :188-197 `teamToTiles = new Map<Team, number>()` (insertion order).
        let mut team_to_tiles: Vec<(Team, f64)> = Vec::new();
        for p in cx.st.players() {
            let pr = cx.st.player(p);
            // Sanity check, team should not be null here
            let Some(team) = pr.team() else { continue };
            let n = f64::from(pr.num_tiles_owned());
            match team_to_tiles.iter_mut().find(|(t, _)| t == team) {
                Some(e) => e.1 += n, // `(teamToTiles.get(team) ?? 0) + numTilesOwned()`
                None => team_to_tiles.push((team.clone(), 0.0 + n)),
            }
        }
        // TS: :198-200 `Array.from(entries).sort((a, b) => b[1] - a[1])`
        of_det::js_sort_by(&mut team_to_tiles, |a, b| b.1 - a.1);
        if team_to_tiles.is_empty() {
            return;
        }
        let (max_team, max_tiles) = team_to_tiles.swap_remove(0);
        if self.has_won(cx, max_tiles) {
            if max_team == colored_teams::BOT {
                return;
            }
            cx.set_winner(Winner::Team(max_team));
            self.active = false;
        }
    }
}

impl ExecBehavior for WinCheckExec {
    fn is_active(&self) -> bool {
        self.active
    }
    /// `activeDuringSpawnPhase()` returns `false` (`WinCheckExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    /// `init(mg, ticks)` (`:26-28`).
    fn init(&mut self, _cx: &mut Ctx, _ticks: i32) {
        self.initialized = true;
    }
    /// `tick(ticks)` (`:30-45`).
    fn tick(&mut self, cx: &mut Ctx, ticks: i32) {
        // int: ticks are non-negative, so Rust `%` equals JS `%`.
        if ticks % 10 != 0 {
            return;
        }
        self.require_init();
        if self.check_ranked_team_cancelled(cx) {
            return;
        }
        if cx.st.config().game_config().game_mode == GameMode::Ffa {
            self.check_winner_ffa(cx);
        } else {
            self.check_winner_team(cx);
        }
    }
}

// CS v1 export (`WinCheck v1 (execution)` record, dump field order).
crate::impl_canon_record!(WinCheckExec { "active" => active, "initialized" => initialized, "checkedRankedSpawns" => checked_ranked_spawns });

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranked_team_game_predicate() {
        assert!(is_ranked_team_game(Some(RankedType::TwoVTwo)));
        assert!(is_ranked_team_game(Some(RankedType::ClanWars)));
        assert!(!is_ranked_team_game(Some(RankedType::OneVOne)));
        assert!(!is_ranked_team_game(Some(RankedType::FreeForAll)));
        assert!(!is_ranked_team_game(None));
    }

    #[test]
    fn constructor_and_off_tick_noop() {
        let mut e = WinCheckExec::new();
        assert!(e.active && !e.initialized && !e.checked_ranked_spawns);
        let mut st = crate::state::State::default();
        let mut sc = crate::scratch::Scratch::new();
        let mut cx = Ctx::new(&mut st, &mut sc, None);
        e.init(&mut cx, 0);
        assert!(e.initialized);
        e.tick(&mut cx, 7); // `ticks % 10 !== 0`: returns before reading the game
        assert!(!e.checked_ranked_spawns && e.is_active());
    }
}
