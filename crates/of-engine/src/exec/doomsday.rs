//! `DoomsdayClockExecution` (`engine/src/execution/DoomsdayClockExecution.ts`). Owner: **P6**.
//!
//! Fields: the `DoomsdayClock v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1).
//!
//! Every quantity the TS stores is an integer-valued JS number (`DoomsdayClockExecution.ts:53-58`);
//! the formulas come from `of_det::util` (VECTORS `util`, re-exported by `config::doomsday`).

#[allow(unused_imports)]
use crate::exec::prelude::*;

use crate::api::types::{GameMode, Team};
use crate::config::config::DoomsdayClockSettings;
use crate::config::doomsday::{
    ROT_NOISE_SCALE, doomsday_clock_drain, doomsday_clock_required_tiles, doomsday_clock_rot_quota,
    doomsday_clock_troop_floor, rot_front_noise, rot_speckle_noise,
};
use crate::state::game::Winner;

/// One `rotState` entry (`DoomsdayClockExecution.ts:95-98`); snapshot `{player, since, held,
/// frontTiles, frontRotted}`.
#[derive(StateClone, Debug, Default)]
pub struct RotState {
    pub since: i32,
    pub held: i32,
    /// `front: Map<TileRef, number>`.
    pub front: JsMap<TileRef, i32>,
}

/// `DoomsdayClockExecution` data.
#[derive(StateClone, Debug)]
pub struct DoomsdayClockExec {
    pub active: bool,
    pub initialized: bool,
    /// `rotState: Map<smallID, {since, held, front}>` (`:95-98`)
    pub rot_state: JsMap<SmallIdKey, RotState>,
}

/// `class LowestN` (`:60-87`): keeps the `n` lowest-keyed tiles seen, in one pass and in key
/// order (ties keep arrival order: the insertion point is after every key `<=` the new one).
struct LowestN {
    tiles: Vec<TileRef>,
    keys: Vec<f64>,
    n: f64,
}

impl LowestN {
    fn new(n: f64) -> Self {
        LowestN { tiles: Vec::new(), keys: Vec::new(), n }
    }
    /// `get size()`.
    fn size(&self) -> f64 {
        self.tiles.len() as f64
    }
    /// `offer(tile, key)` (`:68-83`).
    fn offer(&mut self, tile: TileRef, key: f64) {
        if self.size() == self.n {
            // `key >= this.keys[this.tiles.length - 1]` (an `undefined` key compares false).
            if let Some(&last) = self.tiles.len().checked_sub(1).and_then(|i| self.keys.get(i))
                && key >= last
            {
                return;
            }
        }
        let mut i = self.keys.len();
        while i > 0 && self.keys[i - 1] > key {
            // in-range: 0 < i <= keys.len()
            i -= 1;
        }
        self.tiles.insert(i, tile);
        self.keys.insert(i, key);
        if self.size() > self.n {
            self.tiles.pop();
            self.keys.pop();
        }
    }
    /// `take()`.
    fn take(self) -> Vec<TileRef> {
        self.tiles
    }
}

/// `player.tiles().has(t)`.
#[inline]
fn tiles_has(st: &crate::state::State, p: SmallId, t: TileRef) -> bool {
    st.pos_tiles().has(&st.player_data(p).tiles, &st.statics.land_index, t)
}

/// `player.borderTiles().has(t)`.
#[inline]
fn border_has(st: &crate::state::State, p: SmallId, t: TileRef) -> bool {
    st.pos_border().has(&st.player_data(p).border_tiles, &st.statics.land_index, t)
}

#[allow(clippy::new_without_default)] // a TS no-argument constructor; `Default` would be a placeholder
impl DoomsdayClockExec {
    /// `new DoomsdayClockExecution()`: `active = true`, `mg = null`, `rotState = new Map()`.
    pub fn new() -> Self {
        DoomsdayClockExec { active: true, initialized: false, rot_state: JsMap::new() }
    }

    /// `sides(contenders, ffa)` (`:378-389`): singletons in FFA, by team (first-seen team
    /// order, `Map` insertion order) otherwise; team-less players are skipped.
    fn sides(st: &crate::state::State, contenders: &[SmallId], ffa: bool) -> Vec<Vec<SmallId>> {
        if ffa {
            return contenders.iter().map(|&p| vec![p]).collect();
        }
        let mut by_team: Vec<(Team, Vec<SmallId>)> = Vec::new();
        for &p in contenders {
            let Some(team) = st.player(p).team() else { continue };
            match by_team.iter_mut().find(|(t, _)| t == team) {
                Some((_, members)) => members.push(p),
                None => by_team.push((team.clone(), vec![p])),
            }
        }
        by_team.into_iter().map(|(_, m)| m).collect()
    }

    /// `rot(mg, player, secondsUnder, cfg)` (`:228-277`).
    fn rot(&mut self, cx: &mut Ctx, player: SmallId, seconds_under: f64, cfg: &DoomsdayClockSettings) {
        let owned = f64::from(cx.st.player(player).num_tiles_owned());
        if owned <= 0.0 {
            return;
        }
        let id = player;
        // TS: :242-246 `rotState.get(id) ?? { since: ticks, held: owned, front: new Map() }`
        let (since, held) = match self.rot_state.get(&id) {
            Some(s) => (s.since, s.held),
            None => (cx.st.ticks(), cx.st.player(player).num_tiles_owned() as i32), // int: a tile count
        };

        // TS: :250
        let even_quota = doomsday_clock_rot_quota(owned, seconds_under, cfg.rot_death_seconds);

        // TS: :254-262 grainy opening, sized off the territory
        let grainy = f64::from(cx.st.ticks() - since) < cfg.rot_grain_seconds * 10.0;
        let specks = if grainy {
            of_det::js::max(
                1.0,
                of_det::js::ceil((f64::from(held) * cfg.rot_speckle_percent) / 100.0 / cfg.rot_grain_seconds),
            )
        } else {
            0.0
        };
        let mut budget = of_det::js::min(owned, of_det::js::max(even_quota, specks));

        // TS: :265-269 keep the front tiles the player still holds, in front order
        let mut front: JsMap<TileRef, i32> = JsMap::new();
        if let Some(s) = self.rot_state.get(&id) {
            for (&tile, &rotted) in s.front.iter() {
                if tiles_has(cx.st, player, tile) {
                    front.insert(tile, rotted);
                }
            }
        }

        // TS: :271-274
        budget -= self.speckle(cx, player, of_det::js::min(specks, budget), &mut front);
        budget -= self.spread(cx, player, budget, &mut front);
        if budget > 0.0 {
            self.speckle(cx, player, budget, &mut front);
        }

        // TS: :276 (`set` keeps an existing key's position)
        self.rot_state.insert(id, RotState { since, held, front });
    }

    /// `speckle(mg, player, count, front)` (`:281-308`).
    fn speckle(&mut self, cx: &mut Ctx, player: SmallId, count: f64, front: &mut JsMap<TileRef, i32>) -> f64 {
        if count <= 0.0 {
            return 0.0;
        }
        let salt = f64::from(player.0);
        let mut interior = LowestN::new(count);
        let mut edge = LowestN::new(count);
        {
            // TS: `player.tiles().forEach(..)`; iter-live: the callback only reads.
            let st = &*cx.st;
            for tile in st.player_data(player).tiles.iter() {
                let key = rot_speckle_noise(f64::from(st.map.x(tile)), f64::from(st.map.y(tile)), salt);
                if border_has(st, player, tile) {
                    edge.offer(tile, key);
                } else {
                    interior.offer(tile, key);
                }
            }
        }
        let picked = if interior.size() >= count {
            interior.take()
        } else {
            let mut v = interior.take();
            v.extend(edge.take());
            v
        };
        let mut opened = 0.0;
        for tile in picked {
            if opened == count {
                break;
            }
            if self.consume(cx, player, tile, front) {
                opened += 1.0;
            }
        }
        opened
    }

    /// `spread(mg, player, count, front)` (`:319-338`).
    fn spread(&mut self, cx: &mut Ctx, player: SmallId, count: f64, front: &mut JsMap<TileRef, i32>) -> f64 {
        if count <= 0.0 || front.is_empty() {
            return 0.0;
        }
        let salt = f64::from(player.0);
        let mut pick = LowestN::new(count);
        for (&tile, &rotted) in front.iter() {
            let key = f64::from(rotted) * ROT_NOISE_SCALE + rot_front_noise(f64::from(tile), salt);
            pick.offer(tile, key);
        }
        let mut taken = 0.0;
        for tile in pick.take() {
            if taken == count {
                break;
            }
            if self.consume(cx, player, tile, front) {
                taken += 1.0;
            }
        }
        taken
    }

    /// `consume(mg, player, tile, front)` (`:350-375`): false if the player no longer holds
    /// the tile (relinquish throws on unowned land).
    fn consume(&mut self, cx: &mut Ctx, player: SmallId, tile: TileRef, front: &mut JsMap<TileRef, i32>) -> bool {
        if !tiles_has(cx.st, player, tile) {
            return false;
        }
        cx.player_mut(player).relinquish(tile);
        // Must run after relinquish: setFallout throws on owned tiles.
        cx.set_fallout(tile, true);
        cx.player_mut(player).mark_rotted();
        front.remove(&tile);
        // TS: :369-373 `for (const neighbour of mg.neighbors(tile))`
        for neighbour in cx.st.map.neighbors(tile) {
            if tiles_has(cx.st, player, neighbour) {
                let prev = front.get(&neighbour).copied().unwrap_or(0); // `?? 0`
                front.insert(neighbour, prev + 1);
            }
        }
        true
    }
}

impl ExecBehavior for DoomsdayClockExec {
    fn is_active(&self) -> bool {
        self.active
    }
    /// `activeDuringSpawnPhase()` returns `false` (`DoomsdayClockExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    /// `init(mg, ticks)` (`:100-102`).
    fn init(&mut self, _cx: &mut Ctx, _ticks: i32) {
        self.initialized = true;
    }
    /// `tick(ticks)` (`:104-221`).
    fn tick(&mut self, cx: &mut Ctx, ticks: i32) {
        // int: ticks are non-negative, so Rust `%` equals JS `%`.
        if ticks % 10 != 0 {
            return; // once per second
        }
        if !self.initialized {
            of_det::ts_throw!("Not initialized");
        }
        let cfg = cx.st.config().doomsday_clock_config();
        if !cfg.enabled {
            return;
        }
        // TS: :114-118 `warshipDrainCfg = { ...cfg, drainStartPercent: warship…, drainMaxPercent: warship… }`
        let (ws_start, ws_max) = (cfg.warship_drain_start_percent, cfg.warship_drain_max_percent);

        let elapsed = cx.st.elapsed_game_seconds();
        // TS: :124 (players() is alive-only; bots are not subject to the clock)
        let contenders: Vec<SmallId> =
            cx.st.players().into_iter().filter(|&p| cx.st.player(p).player_type() != PlayerType::Bot).collect();

        let ffa = cx.st.config().game_config().game_mode == GameMode::Ffa;
        let sides = Self::sides(cx.st, &contenders, ffa);

        // TS: :132-136 (winner set or one side left: idle)
        if *cx.st.get_winner() != Winner::None || sides.len() < 2 {
            for &p in &contenders {
                cx.player_mut(p).clear_doomsday_clock();
            }
            self.active = false;
            return;
        }

        let land = f64::from(cx.st.map.num_land_tiles()) - f64::from(cx.st.num_tiles_with_fallout());
        // TS: :140-144 (one bar for every side, `teamGame: !ffa`)
        let required = doomsday_clock_required_tiles(cfg.speed, !ffa, land, elapsed);

        // TS: :152-158 (`members.reduce((sum, m) => sum + m.numTilesOwned(), 0)`; first max wins ties)
        let side_tiles: Vec<f64> = sides
            .iter()
            .map(|members| members.iter().fold(0.0, |sum, &m| sum + f64::from(cx.st.player(m).num_tiles_owned())))
            .collect();
        let mut leader_idx = 0usize;
        for i in 1..side_tiles.len() {
            if side_tiles[i] > side_tiles[leader_idx] {
                leader_idx = i;
            }
        }

        for (i, members) in sides.iter().enumerate() {
            if i != leader_idx && side_tiles[i] < required {
                for &m in members {
                    cx.player_mut(m).enter_doomsday_clock();
                    let seconds_under = of_det::js::floor(f64::from(cx.st.player(m).doomsday_clock_ticks()) / 10.0);
                    if seconds_under >= cfg.warn_seconds {
                        let seconds_past_warn = seconds_under - cfg.warn_seconds;
                        // TS: :172-181 crippled, not wiped: clamp to what sits above the floor
                        let max_troops = cx.st.config().max_troops(&cx.st.player(m));
                        let troop_floor = doomsday_clock_troop_floor(
                            max_troops,
                            seconds_past_warn,
                            cfg.drain_floor_percent,
                            cfg.floor_start_percent,
                            cfg.floor_decay_seconds,
                        );
                        let chunk = doomsday_clock_drain(
                            max_troops,
                            seconds_past_warn,
                            cfg.drain_start_percent,
                            cfg.drain_max_percent,
                            cfg.drain_ramp_seconds,
                            1.0,
                        );
                        let removable = of_det::js::max(0.0, cx.st.player(m).troops() - troop_floor);
                        cx.player_mut(m).remove_troops(of_det::js::min(chunk, removable));
                        // TS: :184-190
                        if cfg.rot_death_seconds > 0.0
                            && seconds_past_warn >= cfg.floor_decay_seconds
                            && cx.st.player(m).troops() <= troop_floor
                        {
                            self.rot(cx, m, seconds_under, &cfg);
                        }
                        // TS: :198-211 (`m.units(Warship)` is a copy)
                        let warships = cx.st.player(m).units_of(&[UnitType::Warship]);
                        for ws in warships {
                            let max_health = cx.st.unit(ws).max_health();
                            let ship_floor = of_det::js::floor((max_health * cfg.drain_floor_percent) / 100.0);
                            let removable = of_det::js::max(0.0, cx.st.unit(ws).health() - ship_floor);
                            if removable <= 0.0 {
                                continue;
                            }
                            let dmg = doomsday_clock_drain(
                                max_health,
                                seconds_past_warn,
                                ws_start,
                                ws_max,
                                cfg.drain_ramp_seconds,
                                cfg.warship_drain_curve_exponent,
                            );
                            cx.unit_mut(ws).modify_health(-of_det::js::min(dmg, removable), None);
                        }
                    }
                }
            } else {
                for &m in members {
                    cx.player_mut(m).clear_doomsday_clock();
                    self.rot_state.remove(&m); // recovered: any rot starts over
                }
            }
        }
    }
}

/// CS v1 `DoomsdayClock v1 (execution)` record (`DoomsdayClockExecution.ts:399-412`): both
/// maps in insertion order; `frontTiles` = `w.tiles(front.keys())`, `frontRotted` =
/// `[...front.values()]`. Written at merge1 by the integrator on behalf of P6.
impl crate::canon::Canon for DoomsdayClockExec {
    fn canon(&self, cx: &mut crate::canon::CanonCx<'_>) -> crate::canon::Cs {
        use crate::canon::Cs;
        let rot: Vec<Cs> = self
            .rot_state
            .iter()
            .map(|(player, st)| {
                Cs::Obj(vec![
                    ("player".into(), player.canon(cx)),
                    ("since".into(), st.since.canon(cx)),
                    ("held".into(), st.held.canon(cx)),
                    ("frontTiles".into(), crate::canon::tiles(st.front.iter().map(|(t, _)| *t))),
                    ("frontRotted".into(), Cs::Arr(st.front.iter().map(|(_, r)| Cs::Int(i64::from(*r))).collect())),
                ])
            })
            .collect();
        Cs::Obj(vec![
            ("active".into(), Cs::Bool(self.active)),
            ("initialized".into(), Cs::Bool(self.initialized)),
            ("rotState".into(), Cs::Arr(rot)),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer_all(n: f64, items: &[(TileRef, f64)]) -> Vec<TileRef> {
        let mut l = LowestN::new(n);
        for &(t, k) in items {
            l.offer(t, k);
        }
        l.take()
    }

    /// `LowestN` keeps the n lowest keys in key order; equal keys keep arrival order, and a key
    /// equal to the current maximum of a full set is rejected (`key >= keys[last]`).
    #[test]
    fn lowest_n_matches_ts() {
        assert_eq!(offer_all(3.0, &[(1, 5.0), (2, 1.0), (3, 4.0), (4, 2.0), (5, 9.0)]), [2, 4, 3]);
        assert_eq!(offer_all(2.0, &[(1, 1.0), (2, 1.0), (3, 1.0)]), [1, 2]);
        assert_eq!(offer_all(3.0, &[(1, 2.0), (2, 1.0), (3, 2.0)]), [2, 1, 3]);
        assert_eq!(offer_all(5.0, &[(7, 3.0)]), [7]);
        // Full at n = 2 with keys [1, 3]: a 2 goes in the middle and evicts 3.
        assert_eq!(offer_all(2.0, &[(1, 1.0), (2, 3.0), (3, 2.0)]), [1, 3]);
    }

    #[test]
    fn constructor_and_off_second_noop() {
        let mut e = DoomsdayClockExec::new();
        assert!(e.active && !e.initialized && e.rot_state.is_empty());
        let mut st = crate::state::State::default();
        let mut sc = crate::scratch::Scratch::new();
        let mut cx = Ctx::new(&mut st, &mut sc, None);
        e.init(&mut cx, 0);
        e.tick(&mut cx, 3); // not a whole second: returns before reading config
        assert!(e.is_active() && e.initialized);
        assert!(!e.active_during_spawn_phase());
    }
}
