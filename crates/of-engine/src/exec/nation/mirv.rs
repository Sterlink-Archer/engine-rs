//! `NationMIRVBehavior` (`execution/nation/NationMIRVBehavior.ts`). Owner: **P11**. Snapshot
//! `{}` (no state of its own; the shared per-target tick map is `State.nation_mirv_targets`).
//! Local PRNG `ticks*1024 + target.smallID()` (`:63`).
//!
//! Every `this.random` draw is the nation's shared `self.rng` (ARCHITECTURE §4.4): exactly one
//! `chance(hesitationOdds)` per `considerMIRV` that gets past the three cheap checks, plus
//! whatever `respondToMIRV` and the emoji behaviour draw.

use crate::api::types::{Difficulty, EmojiRecipient, PlayerType, UnitType};
use crate::canon::{Canon, CanonCx, CanonError, Cs, Uncanon, UncanonCx};
use crate::exec::mirv::MirvExec;
use crate::exec::nation::NationExec;
use crate::exec::nation::emoji::{EMOJI_NUKE, respond_to_mirv};
use crate::exec::nation::utils::rand_territory_tile_array;
#[allow(unused_imports)]
use crate::exec::prelude::*;
use crate::exec::util::calculate_territory_center;
use crate::state::State;
use of_det::js;
use of_det::ts_throw;

/// `MIRV_COOLDOWN_TICKS = 300` (`:32`): 30 seconds at 10 ticks/second.
pub(crate) const MIRV_COOLDOWN_TICKS: i32 = 300;

/// `MIN_MIRV_LEAK_SHARE` (`:35-40`): share of a MIRV's warheads that must get past SAMs for it
/// to be worth saving for or launching.
fn min_mirv_leak_share(d: Difficulty) -> f64 {
    match d {
        Difficulty::Easy => 0.0, // Never checks
        Difficulty::Medium => 0.1,
        Difficulty::Hard => 0.2,
        Difficulty::Impossible => 0.25,
    }
}

/// `MIRV_MAX_WARHEADS = 350` (`:43`).
const MIRV_MAX_WARHEADS: f64 = 350.0;
/// `LAND_TILES_PER_MIRV_WARHEAD = 2000` (`:44`).
const LAND_TILES_PER_MIRV_WARHEAD: f64 = 2000.0;
/// `RIVAL_MIN_TILE_RATIO = 0.5` (`:47`).
const RIVAL_MIN_TILE_RATIO: f64 = 0.5;
/// `MIRV_COVERAGE_SAMPLES = 40` (`:50`).
const MIRV_COVERAGE_SAMPLES: u32 = 40;

/// `NationMIRVBehavior` data (none).
#[derive(StateClone, Debug, Default)]
pub struct MirvB {}

/// CS: `NationMIRVBehavior v1` is `{}` (unwrapped from `{v, d}`, SPEC N4).
impl Canon for MirvB {
    fn canon(&self, _cx: &mut CanonCx<'_>) -> Cs {
        Cs::Obj(Vec::new())
    }
}
impl Uncanon for MirvB {
    fn uncanon(v: &Cs, _cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        match v {
            Cs::Obj(_) => Ok(MirvB {}),
            _ => Err(CanonError::new(format!("NationMIRVBehavior: expected obj, got {v:?}"))),
        }
    }
}

/// `mirvLeakShare(game, attacker, target)` (`:53-101`): estimated share of a MIRV's warheads at
/// `target` that SAMs, each shooting its level in warheads, would miss.
pub fn mirv_leak_share(st: &State, attacker: SmallId, target: SmallId) -> f64 {
    let tp = st.player(target);
    let warheads =
        js::min(MIRV_MAX_WARHEADS, js::max(1.0, f64::from(tp.num_tiles_owned()) / LAND_TILES_PER_MIRV_WARHEAD));
    // Own seed, so the estimate is a pure function of the game state
    let mut random = Prng::new(f64::from(st.ticks()) * 1024.0 + f64::from(target.0));
    let samples = rand_territory_tile_array(&mut random, st, target, MIRV_COVERAGE_SAMPLES);
    if samples.is_empty() {
        return 1.0;
    }

    let config = st.config();
    let warheads_per_sample = warheads / samples.len() as f64;
    // TS `Map<Unit, number>` iterated by `forEach` (`:96`): insertion order. Units cannot be
    // freed during this call, so the handle is a faithful identity key.
    let mut warheads_in_range: JsMap<UnitRef, f64> = JsMap::new();
    let mut covered_samples = 0.0;
    for &tile in &samples {
        let mut covered = false;
        for nu in st.nearby_units(tile, config.max_sam_range(), &[UnitType::SamLauncher], |_, _, _| true, false) {
            let sam = st.unit(nu.unit);
            let owner = sam.owner();
            if owner == attacker || st.player(owner).is_friendly(attacker, false) {
                continue;
            }
            let r = config.sam_range(f64::from(sam.level()));
            if nu.dist_squared > r * r {
                // TS: `** 2`
                continue;
            }
            let prev = warheads_in_range.get(&nu.unit).copied().unwrap_or(0.0); // TS: `?? 0`
            warheads_in_range.insert(nu.unit, prev + warheads_per_sample);
            covered = true;
        }
        if covered {
            covered_samples += 1.0;
        }
    }

    let mut shots = 0.0;
    for (&sam, &in_range) in warheads_in_range.iter() {
        shots += js::min(f64::from(st.unit(sam).level()), in_range);
    }
    let intercepted = js::min(covered_samples * warheads_per_sample, shots);
    1.0 - intercepted / warheads
}

/// `mirvGetsThrough(game, attacker, target)` (`:104-114`); Easy doesn't check.
pub fn mirv_gets_through(st: &State, attacker: SmallId, target: SmallId) -> bool {
    let min_leak_share = min_mirv_leak_share(st.config().game_config().difficulty);
    min_leak_share == 0.0 || mirv_leak_share(st, attacker, target) >= min_leak_share
}

/// `isMirvWorthSavingFor(game, player)` (`:117-139`): whether a MIRV would get through at the
/// land or city leader, the players nations MIRV.
pub fn is_mirv_worth_saving_for(st: &State, player: SmallId) -> bool {
    let mut land_leader: Option<SmallId> = None;
    let mut city_leader: Option<SmallId> = None;
    for p in valid_mirv_targets(st, player) {
        let pr = st.player(p);
        if land_leader.is_none_or(|l| pr.num_tiles_owned() > st.player(l).num_tiles_owned()) {
            land_leader = Some(p);
        }
        if city_leader.is_none_or(|c| pr.unit_count(UnitType::City) > st.player(c).unit_count(UnitType::City)) {
            city_leader = Some(p);
        }
    }
    if let Some(l) = land_leader
        && mirv_gets_through(st, player, l)
    {
        return true;
    }
    match city_leader {
        Some(c) => Some(c) != land_leader && mirv_gets_through(st, player, c),
        None => false,
    }
}

/// `validMirvTargets(game, player)` (`:141-151`).
fn valid_mirv_targets(st: &State, player: SmallId) -> Vec<SmallId> {
    let me = st.player(player);
    st.players()
        .into_iter()
        .filter(|&p| {
            p != player && p.is_player() && st.player(p).player_type() != PlayerType::Bot && !me.is_on_same_team(p)
        })
        .collect()
}

/// `this.player` with its TS `"not initialized"` guard (`:253`, `:304`, …).
#[inline]
fn me(n: &NationExec) -> SmallId {
    match n.player {
        Some(p) => p,
        None => ts_throw!("not initialized"),
    }
}

#[inline]
fn difficulty(st: &State) -> Difficulty {
    st.config().game_config().difficulty
}

/// `get hesitationOdds` (`:185-199`).
fn hesitation_odds(st: &State) -> f64 {
    match difficulty(st) {
        Difficulty::Easy => 2.0, // More likely to hesitate
        Difficulty::Medium => 4.0,
        Difficulty::Hard => 8.0,
        Difficulty::Impossible => 16.0, // Rarely hesitates
    }
}

/// `get victoryDenialThresholdPercent` (`:205-219`): whole percent of the land, for exact
/// integer comparison.
fn victory_denial_threshold_percent(st: &State) -> f64 {
    match difficulty(st) {
        Difficulty::Easy => 75.0, // Only react right before the game ends (80%)
        Difficulty::Medium => 65.0,
        Difficulty::Hard => 55.0,
        Difficulty::Impossible => 40.0, // Reacts early
    }
}

/// `get steamrollCityGapMultiplier` (`:221-235`).
fn steamroll_city_gap_multiplier(st: &State) -> f64 {
    match difficulty(st) {
        Difficulty::Easy => 2.0, // Needs larger gap to trigger
        Difficulty::Medium => 1.5,
        Difficulty::Hard => 1.25,
        Difficulty::Impossible => 1.15, // Reacts to smaller gaps
    }
}

/// `get steamrollMinLeaderCities` (`:237-250`).
fn steamroll_min_leader_cities(st: &State) -> f64 {
    match difficulty(st) {
        Difficulty::Easy => 20.0, // Needs more cities to trigger
        Difficulty::Medium | Difficulty::Hard => 10.0,
        Difficulty::Impossible => 8.0, // Reacts early
    }
}

impl NationExec {
    /// `mirvBehavior.considerMIRV()` (`:252-293`).
    pub fn mirv_consider_mirv(&mut self, cx: &mut Ctx) -> bool {
        let player = me(self);
        if cx.st.config().is_unit_disabled(UnitType::Mirv) {
            return false;
        }
        if cx.st.player(player).units_of(&[UnitType::MissileSilo]).is_empty() {
            return false;
        }
        if cx.st.player(player).gold() < mirv_cost(cx.st, player, UnitType::Mirv) {
            return false;
        }

        if self.rng.chance(hesitation_odds(cx.st)) {
            return false;
        }

        // TS: `if (target && this.isWorthMirving(target))`; a Player object is always truthy.
        if let Some(t) = mirv_select_counter_mirv_target(cx.st, player)
            && mirv_is_worth_mirving(cx.st, player, t)
        {
            self.mirv_maybe_send_mirv(cx, t);
            return true;
        }

        if let Some(t) = mirv_select_victory_denial_target(cx.st, player)
            && mirv_is_worth_mirving(cx.st, player, t)
        {
            self.mirv_maybe_send_mirv(cx, t);
            return true;
        }

        if let Some(t) = mirv_select_steamroll_stop_target(cx.st, player)
            && mirv_is_worth_mirving(cx.st, player, t)
        {
            self.mirv_maybe_send_mirv(cx, t);
            return true;
        }

        if let Some(t) = mirv_select_rival_target(cx.st, player)
            && mirv_is_worth_mirving(cx.st, player, t)
        {
            self.mirv_maybe_send_mirv(cx, t);
            return true;
        }

        false
    }

    /// `maybeSendMIRV(enemy)` (`:440-452`).
    fn mirv_maybe_send_mirv(&mut self, cx: &mut Ctx, enemy: SmallId) {
        let player = me(self);

        self.emoji_maybe_send_attack_emoji(cx, enemy);

        let center_tile = calculate_territory_center(cx.st, enemy);
        // TS: `if (centerTile && this.player.canBuild(UnitType.MIRV, centerTile))`: both a
        // `TileRef` of 0 and `canBuild`'s `false` are falsy, and so is a spawn tile of 0.
        let Some(center) = center_tile.filter(|&t| t != 0) else { return };
        let spawn = {
            let st = &*cx.st;
            st.player(player).can_build(&mut *cx.sc, UnitType::Mirv, center, None)
        };
        if spawn.is_some_and(|t| t != 0) {
            cx.st.add_execution(Exec::Mirv(MirvExec::new(player, center)));
            mirv_record_mirv_hit(cx.st, enemy);
            self.emoji_send_emoji(cx, EmojiRecipient::AllPlayers, EMOJI_NUKE);
            respond_to_mirv(cx, &mut self.rng, enemy);
        }
    }
}

/// `selectCounterMirvTarget()` (`:303-311`).
fn mirv_select_counter_mirv_target(st: &State, player: SmallId) -> Option<SmallId> {
    let mut attackers: Vec<SmallId> =
        valid_mirv_targets(st, player).into_iter().filter(|&p| is_inbound_mirv_from(st, player, p)).collect();
    if attackers.is_empty() {
        return None;
    }
    of_det::js_sort_by(&mut attackers, |&a, &b| {
        f64::from(st.player(b).num_tiles_owned()) - f64::from(st.player(a).num_tiles_owned())
    }); // TS: NationMIRVBehavior.ts:309
    Some(attackers[0])
}

/// `isWorthMirving(target)` (`:295-300`).
fn mirv_is_worth_mirving(st: &State, player: SmallId, target: SmallId) -> bool {
    !mirv_was_recently_mirved(st, target) && mirv_gets_through(st, player, target)
}

/// `selectVictoryDenialTarget()` (`:313-357`).
fn mirv_select_victory_denial_target(st: &State, player: SmallId) -> Option<SmallId> {
    let total_land = f64::from(st.map.num_land_tiles());
    if total_land == 0.0 {
        return None;
    }
    // Compared against tiles * 100; severity ranks by tile count, which
    // orders candidates the same as by share (same denominator).
    let scaled_threshold = total_land * victory_denial_threshold_percent(st);
    let mut best: Option<(SmallId, f64)> = None;
    for p in valid_mirv_targets(st, player) {
        let mut severity = 0.0;
        let team = st.player(p).team();
        if let Some(team) = team {
            let team_members: Vec<SmallId> =
                st.players().into_iter().filter(|&x| st.player(x).team() == Some(team) && x.is_player()).collect();
            let team_territory =
                team_members.iter().map(|&x| f64::from(st.player(x).num_tiles_owned())).fold(0.0, |a, b| a + b);
            if team_territory * 100.0 >= scaled_threshold {
                // Only consider the largest team member as the target when team exceeds threshold
                let mut largest_member: Option<SmallId> = None;
                let mut largest_tiles = -1.0;
                for &member in &team_members {
                    let tiles = f64::from(st.player(member).num_tiles_owned());
                    if tiles > largest_tiles {
                        largest_tiles = tiles;
                        largest_member = Some(member);
                    }
                }
                if largest_member == Some(p) {
                    severity = team_territory;
                } else {
                    severity = 0.0; // Skip non-largest members
                }
            }
        } else {
            let tiles = f64::from(st.player(p).num_tiles_owned());
            if tiles * 100.0 >= scaled_threshold {
                severity = tiles;
            }
        }
        if severity > 0.0 && best.is_none_or(|(_, s)| severity > s) {
            best = Some((p, severity));
        }
    }
    best.map(|(p, _)| p)
}

/// `selectSteamrollStopTarget()` (`:359-386`).
fn mirv_select_steamroll_stop_target(st: &State, player: SmallId) -> Option<SmallId> {
    let valid_targets = valid_mirv_targets(st, player);

    if valid_targets.is_empty() {
        return None;
    }

    let mut all_players: Vec<(SmallId, f64)> =
        st.players().into_iter().filter(|p| p.is_player()).map(|p| (p, mirv_count_cities(st, p))).collect();
    of_det::js_sort_by(&mut all_players, |a, b| b.1 - a.1); // TS: NationMIRVBehavior.ts:369

    if all_players.len() < 2 {
        return None;
    }

    let top_player = all_players[0];

    if top_player.1 <= steamroll_min_leader_cities(st) {
        return None;
    }

    let second_highest = all_players[1].1;

    let threshold = second_highest * steamroll_city_gap_multiplier(st);

    if top_player.1 >= threshold {
        return if valid_targets.contains(&top_player.0) { Some(top_player.0) } else { None };
    }

    None
}

/// `selectRivalTarget()` (`:389-405`): Hard & Impossible, the biggest bordering player about
/// our size.
fn mirv_select_rival_target(st: &State, player: SmallId) -> Option<SmallId> {
    let d = difficulty(st);
    if d != Difficulty::Hard && d != Difficulty::Impossible {
        return None;
    }
    let me = st.player(player);
    let min_tiles = f64::from(me.num_tiles_owned()) * RIVAL_MIN_TILE_RATIO;
    let mut best: Option<SmallId> = None;
    for p in valid_mirv_targets(st, player) {
        let tiles = f64::from(st.player(p).num_tiles_owned());
        if me.is_friendly(p, false) || tiles < min_tiles {
            continue;
        }
        if best.is_some_and(|b| tiles <= f64::from(st.player(b).num_tiles_owned())) {
            continue;
        }
        if me.shares_border_with(p) {
            best = Some(p);
        }
    }
    best
}

/// `wasRecentlyMirved(target)` (`:408-412`).
fn mirv_was_recently_mirved(st: &State, target: SmallId) -> bool {
    let Some(&last_tick) = st.nation_mirv_targets.get(&target) else { return false };
    st.ticks() - last_tick < MIRV_COOLDOWN_TICKS
}

/// `recordMirvHit(target)` (`:414-416`): `Map.set` keeps an existing key's position.
fn mirv_record_mirv_hit(st: &mut State, target: SmallId) {
    let t = st.ticks();
    st.nation_mirv_targets.insert(target, t);
}

/// `isInboundMIRVFrom(attacker)` (`:424-437`).
fn is_inbound_mirv_from(st: &State, player: SmallId, attacker: SmallId) -> bool {
    let enemy_mirvs = st.player(attacker).units_of(&[UnitType::Mirv]);
    for mirv in enemy_mirvs {
        // TS: `if (!dst) continue;`: `undefined` and tile 0 are both falsy.
        let dst = match st.unit(mirv).target_tile() {
            Some(t) if t != 0 => t,
            _ => continue,
        };
        if !st.map.has_owner(dst) {
            continue;
        }
        if st.owner(dst) == player {
            return true;
        }
    }
    false
}

/// `countCities(p)` (`:454-456`).
fn mirv_count_cities(st: &State, p: SmallId) -> f64 {
    f64::from(st.player(p).unit_count(UnitType::City))
}

/// `cost(type)` (`:462-465`).
fn mirv_cost(st: &State, player: SmallId, t: UnitType) -> Gold {
    st.config().unit_cost(t, st, player, 0.0)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::exec::ExecStore;

    #[test]
    fn leak_share_table_and_difficulty_ladders() {
        assert_eq!(Difficulty::ALL.iter().map(|&d| min_mirv_leak_share(d)).collect::<Vec<_>>(), [0.0, 0.1, 0.2, 0.25]);
        assert_eq!(MIRV_COOLDOWN_TICKS, 300);
    }

    #[test]
    fn mirv_behavior_canon_is_empty_object() {
        let st = State::default();
        let ex = ExecStore::default();
        let mut cx = CanonCx::new(&st, &ex);
        assert_eq!(MirvB {}.canon(&mut cx), Cs::Obj(vec![]));
        let mut st2 = State::default();
        assert!(
            MirvB::uncanon(&Cs::Obj(vec![]), &mut UncanonCx { st: &mut st2, keys: &crate::canon::LoadKeys::default() })
                .is_ok()
        );
        assert!(
            MirvB::uncanon(&Cs::Null, &mut UncanonCx { st: &mut st2, keys: &crate::canon::LoadKeys::default() })
                .is_err()
        );
    }

    /// `wasRecentlyMirved`: strictly fewer than 300 ticks since the record; `recordMirvHit`
    /// overwrites in place (JS `Map.set` keeps the key's position, which is CS G order).
    #[test]
    fn mirv_cooldown_and_record_order() {
        let mut st = State::default();
        st.ticks = 1000;
        mirv_record_mirv_hit(&mut st, SmallId(5));
        mirv_record_mirv_hit(&mut st, SmallId(2));
        st.ticks = 1299;
        assert!(mirv_was_recently_mirved(&st, SmallId(5)));
        st.ticks = 1300;
        assert!(!mirv_was_recently_mirved(&st, SmallId(5)));
        assert!(!mirv_was_recently_mirved(&st, SmallId(9)));
        mirv_record_mirv_hit(&mut st, SmallId(5));
        let order: Vec<(u16, i32)> = st.nation_mirv_targets.iter().map(|(k, &v)| (k.0, v)).collect();
        assert_eq!(order, [(5, 1300), (2, 1000)]);
        assert!(mirv_was_recently_mirved(&st, SmallId(5)));
    }

    /// The `"not initialized"` guard is a TS throw (`TsThrow`), not a Rust bug.
    #[test]
    #[allow(clippy::disallowed_methods)] // asserting a TsThrow payload
    fn uninitialized_player_is_a_ts_throw() {
        let r = std::panic::catch_unwind(|| {
            let n = NationExec {
                active: true,
                game_id: String::new(),
                nation: Nation { spawn_cell: None, player_info: test_info() },
                rng: Prng::new(1.0),
                initialized: false,
                player: None,
                attack_rate: None,
                attack_tick: None,
                trigger_ratio: 0.0,
                reserve_ratio: 0.0,
                expand_ratio: 0.0,
                embargo_malus_applied: JsSet::new(),
                spawn_exec_added: false,
                behaviors: None,
            };
            me(&n)
        });
        let Err(p) = r else { panic!("must throw") };
        assert_eq!(p.downcast_ref::<crate::ctx::TsThrow>().map(|t| t.msg.to_string()), Some("not initialized".into()));
    }

    fn test_info() -> PlayerInfo {
        PlayerInfo::new("n", PlayerType::Nation, None, "n_id")
    }
}
