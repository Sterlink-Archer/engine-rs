//! `engine-lib/src/game/NationCount.ts`. Owner: **P3**.

use crate::api::game_config::{GameConfig, NationsConfig, TeamCountConfig};
use crate::api::types::{GameMapSize, GameMode, GameType};

/// `NationPick` (`:12-19`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NationPick {
    None,
    Manifest,
    Random { count: u32 },
}

/// `nationPickForGame(config, manifestNationCount, numHumans)` (`:27-53`).
pub fn nation_pick_for_game(config: &GameConfig, manifest_nation_count: u32, num_humans: u32) -> NationPick {
    match config.nations {
        NationsConfig::Disabled => return NationPick::None,
        // int: zod `uint({min: 1, max: 400})`
        NationsConfig::Count(n) => return NationPick::Random { count: u32::try_from(n).unwrap_or(u32::MAX) },
        NationsConfig::Default => {}
    }
    if config.game_type == GameType::Public {
        if config.game_mode == GameMode::Team && config.player_teams == Some(TeamCountConfig::HumansVsNations) {
            return NationPick::Random { count: num_humans };
        }
        if config.game_map_size == GameMapSize::Compact {
            return NationPick::Random { count: compact_map_nation_count(manifest_nation_count) };
        }
    }
    NationPick::Manifest
}

/// `nationCountForGame(config, manifestNationCount, numHumans)` (`:56-70`).
pub fn nation_count_for_game(config: &GameConfig, manifest_nation_count: u32, num_humans: u32) -> u32 {
    match nation_pick_for_game(config, manifest_nation_count, num_humans) {
        NationPick::None => 0,
        NationPick::Manifest => manifest_nation_count,
        NationPick::Random { count } => count,
    }
}

/// `compactMapNationCount(n)` = `n === 0 ? 0 : Math.max(1, Math.floor(n * 0.25))` (`:72-77`).
fn compact_map_nation_count(manifest_nation_count: u32) -> u32 {
    if manifest_nation_count == 0 {
        return 0;
    }
    // TS: `Math.floor(n * 0.25)`; exact for u32 n (n * 0.25 is exact in f64)
    let q = of_det::js::floor(f64::from(manifest_nation_count) * 0.25);
    of_det::js::max(1.0, q) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    fn gc(f: impl FnOnce(&mut GameConfig)) -> GameConfig {
        let mut g = GameConfig::default();
        f(&mut g);
        g
    }
    #[test]
    fn nation_pick_rules() {
        let d = gc(|g| g.nations = NationsConfig::Disabled);
        assert_eq!(nation_pick_for_game(&d, 40, 3), NationPick::None);
        assert_eq!(nation_count_for_game(&d, 40, 3), 0);
        let n = gc(|g| g.nations = NationsConfig::Count(7));
        assert_eq!(nation_pick_for_game(&n, 40, 3), NationPick::Random { count: 7 });
        let hvn = gc(|g| {
            g.game_type = GameType::Public;
            g.game_mode = GameMode::Team;
            g.player_teams = Some(TeamCountConfig::HumansVsNations);
        });
        assert_eq!(nation_count_for_game(&hvn, 40, 3), 3);
        let compact = gc(|g| {
            g.game_type = GameType::Public;
            g.game_map_size = GameMapSize::Compact;
        });
        assert_eq!(nation_count_for_game(&compact, 41, 3), 10);
        assert_eq!(nation_count_for_game(&compact, 3, 3), 1);
        assert_eq!(nation_count_for_game(&compact, 0, 3), 0);
        // private/singleplayer compact keep the manifest
        let priv_c = gc(|g| {
            g.game_type = GameType::Private;
            g.game_map_size = GameMapSize::Compact;
        });
        assert_eq!(nation_pick_for_game(&priv_c, 41, 3), NationPick::Manifest);
        assert_eq!(nation_count_for_game(&priv_c, 41, 3), 41);
    }
}
