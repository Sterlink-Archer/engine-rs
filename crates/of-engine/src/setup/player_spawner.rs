//! `engine/src/execution/utils/PlayerSpawner.ts`. Owner: **P6**.

use crate::api::types::PlayerType;
use crate::exec::spawn::SpawnExec;
use crate::state::State;

/// `new PlayerSpawner(gm, gameID).spawnPlayers()` (`:6-25`): one `SpawnExecution` per Human,
/// in `allPlayers()` order, built with `player.info()` and no tile.
pub fn spawn_players(st: &State, game_id: &str) -> Vec<SpawnExec> {
    let mut players = Vec::new();
    for p in st.all_players() {
        let pr = st.player(p);
        if pr.player_type() != PlayerType::Human {
            continue;
        }
        players.push(SpawnExec::new(game_id, pr.info().clone(), None, false));
    }
    players
}
