//! `SpawnExecution` (`engine/src/execution/SpawnExecution.ts`). Owner: **P6**.
//!
//! Fields: the `Spawn v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1).

#[allow(unused_imports)]
use crate::exec::prelude::*;

use crate::api::types::{GameType, SpawnArea};
use crate::exec::player::PlayerExec;
use crate::exec::tribe::TribeExec;
use crate::exec::util::get_spawn_tiles_with;

/// `MAX_SPAWN_TRIES = 1_000` (`:36`).
const MAX_SPAWN_TRIES: u32 = 1_000;
/// `RELAX_MIN_DIST_AT = 750` (`:37`).
const RELAX_MIN_DIST_AT: u32 = 750;

/// `type Spawn = { center: TileRef; tiles: TileRef[] }` (`:29`).
struct Spawn {
    center: TileRef,
    tiles: Vec<TileRef>,
}

/// `SpawnExecution` data.
#[derive(StateClone, Debug)]
pub struct SpawnExec {
    /// seeded in the constructor (`:48`)
    pub rng: Prng,
    pub active: bool,
    pub initialized: bool,
    pub queued_during_spawn_phase: bool,
    pub player_info: PlayerInfo,
    /// `tile?: TileRef` (unvalidated intent value; snapshot `num?undef`)
    pub tile: Option<f64>,
    pub from_intent: bool,
}

impl SpawnExec {
    /// `new SpawnExecution(gameID, playerInfo, tile?, fromIntent = false)` (`:39-51`).
    pub fn new(game_id: &str, player_info: PlayerInfo, tile: Option<f64>, from_intent: bool) -> Self {
        // TS: :48-50 `new PseudoRandom(simpleHash(playerInfo.id) + simpleHash(gameID))`; the
        // sum can exceed 2^31, `PseudoRandom` truncates with `| 0` (`Prng::new` → to_int32).
        let seed = of_det::util::simple_hash(&player_info.id) + of_det::util::simple_hash(game_id);
        SpawnExec {
            rng: Prng::new(seed),
            active: true,
            initialized: false,
            queued_during_spawn_phase: false,
            player_info,
            tile,
            from_intent,
        }
    }

    /// `getSpawn(center?)` (`:138-197`).
    fn get_spawn(&mut self, cx: &mut Ctx, center: Option<TileRef>) -> Option<Spawn> {
        if let Some(center) = center {
            // TS: :140-146
            let tiles = get_spawn_tiles_with(&cx.st.map, &mut cx.sc.bfs_main, center, false)
                .ts_expect("getSpawnTiles(requireAllValid = false) returns an array");
            if tiles.is_empty() {
                return None;
            }
            return Some(Spawn { center, tiles });
        }

        let spawn_area = self.get_team_spawn_area(cx);
        let mut tries = 0u32;
        while tries < MAX_SPAWN_TRIES {
            tries += 1;
            let center = self.rand_tile(cx, spawn_area);
            let map = &cx.st.map;
            // TS: :157-163 (`mg.isLand/hasOwner/isBorder` delegate to the main map)
            if !map.is_land(center) || map.has_owner(center) || map.is_border(center) {
                continue;
            }
            if tries <= RELAX_MIN_DIST_AT {
                // TS: :166-180 `allPlayers().filter(id !== mine).some(spawnTile && dist < min)`
                let st = &*cx.st;
                let min_dist = st.config().min_distance_between_players();
                let my_id = self.player_info.id.as_str();
                let nearby = st.all_players().into_iter().filter(|&p| st.player(p).player_id() != my_id).any(|p| {
                    match st.player(p).spawn_tile() {
                        None => false,
                        Some(spawn_tile) => f64::from(st.map.manhattan_dist(spawn_tile, center)) < min_dist,
                    }
                });
                if nearby {
                    continue;
                }
            }
            // TS: :187-191 (null when some spawn tile is outside of the land)
            let Some(tiles) = get_spawn_tiles_with(&cx.st.map, &mut cx.sc.bfs_main, center, true) else {
                continue;
            };
            return Some(Spawn { center, tiles });
        }
        None
    }

    /// `randTile(area?)` (`:199-208`).
    fn rand_tile(&mut self, cx: &Ctx, area: Option<SpawnArea>) -> TileRef {
        let map = &cx.st.map;
        if let Some(area) = area {
            let x = self.rng.next_int(area.x, area.x + area.width);
            let y = self.rng.next_int(area.y, area.y + area.height);
            return map.ref_(x, y);
        }
        let x = self.rng.next_int(0.0, f64::from(map.width()));
        let y = self.rng.next_int(0.0, f64::from(map.height()));
        map.ref_(x, y)
    }

    /// `getTeamSpawnArea()` (`:210-217`).
    fn get_team_spawn_area(&self, cx: &Ctx) -> Option<SpawnArea> {
        let st = &*cx.st;
        let player = st.player_by_id(&self.player_info.id);
        let team = st.player(player).team()?;
        st.team_spawn_area(team)
    }
}

impl ExecBehavior for SpawnExec {
    fn is_active(&self) -> bool {
        self.active
    }
    /// `activeDuringSpawnPhase()` returns `true` (`SpawnExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        true
    }
    /// `init(mg, ticks)` (`:53-56`).
    fn init(&mut self, cx: &mut Ctx, _ticks: i32) {
        self.initialized = true;
        self.queued_during_spawn_phase = cx.st.in_spawn_phase();
    }
    /// `tick(ticks)` (`:58-128`).
    fn tick(&mut self, cx: &mut Ctx, _ticks: i32) {
        self.active = false;

        // TS: :65-68 (security: reject a fractional / out-of-range intent tile first)
        if let Some(t) = self.tile
            && !cx.st.map.is_valid_ref(t)
        {
            return;
        }

        // TS: :70-75
        let player = if cx.st.has_player(&self.player_info.id) {
            cx.st.player_by_id(&self.player_info.id)
        } else {
            cx.add_player(self.player_info.clone(), None)
        };

        // TS: :85-87 (a client's spawn intent counts only if issued during the spawn phase)
        if self.from_intent && !self.queued_during_spawn_phase {
            return;
        }

        // TS: :90-92 (random spawn: no re-rolls)
        if cx.st.config().is_random_spawn() && cx.st.player(player).has_spawned() {
            return;
        }

        // TS: :94-95 `Array.from(player.tiles())` then relinquish each.
        let prev_tiles: Vec<TileRef> = cx.st.player_data(player).tiles.to_vec();
        for &t in &prev_tiles {
            cx.player_mut(player).relinquish(t);
        }
        // TS: :96-98. `this.tile` passed validation above, so it is a valid integer ref.
        let center = if cx.st.config().is_random_spawn() {
            None
        } else {
            self.tile.map(|t| {
                debug_assert!(t >= 0.0 && t.fract() == 0.0);
                t as TileRef // int: isValidRef held (integer, 0 <= t < numTiles)
            })
        };
        let spawn = self.get_spawn(cx, center);

        let Some(spawn) = spawn else {
            // TS: :100-104
            for &t in &prev_tiles {
                cx.player_mut(player).conquer(t);
            }
            return;
        };

        // TS: :106-108
        for &t in &spawn.tiles {
            cx.player_mut(player).conquer(t);
        }

        // TS: :110-115
        if !cx.st.player(player).has_spawned() {
            cx.st.add_execution(Exec::Player(PlayerExec::new(player)));
            if cx.st.player(player).player_type() == PlayerType::Bot {
                let id = cx.st.player(player).player_id().to_owned();
                cx.st.add_execution(Exec::Tribe(TribeExec::new(player, &id)));
            }
        }

        // TS: :117-118
        cx.player_mut(player).set_spawn_tile(spawn.center);
        cx.st.stats_cx().record_spawn_tile(player, spawn.center);

        // TS: :120-127 (singleplayer: the spawn phase ends when the human picks a tile)
        if cx.st.config().game_config().game_type == GameType::Singleplayer
            && self.player_info.player_type == PlayerType::Human
        {
            cx.end_spawn_phase();
        }
    }
}

/// CS v1 `Spawn v1 (execution)` record (`SpawnExecution.ts:221-231`), dump field order.
/// `tile` is `num?undef`: an absent tile is an omitted field, not `null` (SPEC §1.1, N7).
/// Written at merge1 by the integrator on behalf of P6.
impl crate::canon::Canon for SpawnExec {
    fn canon(&self, cx: &mut crate::canon::CanonCx<'_>) -> crate::canon::Cs {
        use crate::canon::Cs;
        Cs::Obj(vec![
            ("random".into(), self.rng.canon(cx)),
            ("active".into(), Cs::Bool(self.active)),
            ("initialized".into(), Cs::Bool(self.initialized)),
            ("queuedDuringSpawnPhase".into(), Cs::Bool(self.queued_during_spawn_phase)),
            ("playerInfo".into(), self.player_info.canon(cx)),
            ("tile".into(), self.tile.map_or(Cs::Absent, |t| t.canon(cx))),
            ("fromIntent".into(), Cs::Bool(self.from_intent)),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The PRNG is seeded in the constructor from `simpleHash(id) + simpleHash(gameID)`
    /// (`:48-50`), truncated with `| 0` by `PseudoRandom`.
    #[test]
    fn constructor_seed_and_defaults() {
        let info = PlayerInfo::new("tribe", PlayerType::Bot, None, "abcdefgh");
        let mut e = SpawnExec::new("game_id", info.clone(), Some(12.0), true);
        let seed = of_det::util::simple_hash("abcdefgh") + of_det::util::simple_hash("game_id");
        let mut r = Prng::new(seed);
        assert_eq!(e.rng.next(), r.next());
        assert!(e.active && !e.initialized && !e.queued_during_spawn_phase && e.from_intent);
        assert_eq!(e.tile, Some(12.0));
        assert_eq!(e.player_info, info);
        assert!(e.active_during_spawn_phase());
    }

    #[test]
    fn init_records_the_phase() {
        let info = PlayerInfo::new("p", PlayerType::Human, None, "id1");
        let mut e = SpawnExec::new("g", info, None, false);
        let mut st = crate::state::State::default();
        let mut sc = crate::scratch::Scratch::new();
        let mut cx = Ctx::new(&mut st, &mut sc, None);
        e.init(&mut cx, 0);
        assert!(e.initialized && e.queued_during_spawn_phase); // State::default() is in the spawn phase
        cx.st.start_tick = Some(3);
        e.init(&mut cx, 4);
        assert!(!e.queued_during_spawn_phase);
    }
}
