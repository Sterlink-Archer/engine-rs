//! `NationWarshipBehavior` (`execution/nation/NationWarshipBehavior.ts`). Owner: **P12**.
//!
//! - The four tracking sets are TS `Set<Unit>`s that may hold dead units (`:47`): they are
//!   `SmallJsSet<UnitRef>` (PORTING_GUIDE §5.1 exception) iterated with a live cursor, because
//!   the loops delete the current entry while iterating (`:150-162`, `:174-184`, `:203-260`).
//! - `unit.warshipState()` is the mutating getter (it writes `isInCombat`, `UnitImpl.ts:437-442`);
//!   every TS call is a `cx.unit_mut(u).warship_state()` call at the same position, including
//!   inside `filter` callbacks (PORTING_GUIDE §14.9).
//! - `if (target && ..)` on a `TileRef` treats tile 0 as falsy (`:192`); ported literally.

use crate::api::types::{Difficulty, EmojiRecipient};
use crate::canon::{Canon, CanonCx, Cs};
use crate::ctx::ts_throw;
use crate::exec::construction::ConstructionExec;
use crate::exec::nation::NationExec;
use crate::exec::nation::emoji::EMOJI_WARSHIP_RETALIATION;
use crate::exec::nation::structure::{cost, difficulty};
use crate::exec::prelude::*;
use crate::state::State;
use crate::state::unit::{WarshipMode, WarshipStateUpdate};

/// `NationWarshipBehavior` data, snapshot order (`:31-37`; `Set<Unit>`s, JS order).
#[derive(StateClone, Debug, Default)]
pub struct WarshipB {
    pub tracked_transport_ships: SmallJsSet<UnitRef, 8>,
    pub tracked_trade_ships: SmallJsSet<UnitRef, 8>,
    pub tracked_incoming_transport_ships: SmallJsSet<UnitRef, 8>,
    pub dealt_with_transport_ship: SmallJsSet<UnitRef, 8>,
}

/// `[...set].map(u => w.unit(u))` (`:48`): unit keys in set order (dead units included).
fn canon_unit_set(set: &SmallJsSet<UnitRef, 8>, cx: &mut CanonCx<'_>) -> Cs {
    Cs::Arr(set.iter().map(|&u| cx.unit_key(u)).collect())
}

// CS v1 (`NationWarshipBehavior v1`, unwrapped per SPEC N4), dump order (`:46-55`, `:622-631`).
impl Canon for WarshipB {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        Cs::Obj(vec![
            ("trackedTransportShips".into(), canon_unit_set(&self.tracked_transport_ships, cx)),
            ("trackedTradeShips".into(), canon_unit_set(&self.tracked_trade_ships, cx)),
            ("trackedIncomingTransportShips".into(), canon_unit_set(&self.tracked_incoming_transport_ships, cx)),
            ("dealtWithTransportShip".into(), canon_unit_set(&self.dealt_with_transport_ship, cx)),
        ])
    }
}

/// The `reason` argument of `maybeRetaliateWithWarship` (`:266`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reason {
    Trade,
    Transport,
    Invasion,
}

/// `this.player` (set from `NationExecution.player` at `initializeBehaviors`).
#[inline]
fn me(ne: &NationExec) -> SmallId {
    ne.player.ts_expect("NationWarshipBehavior.player")
}

/// `w.warshipState().patrolTile` through the mutating getter.
#[inline]
fn patrol_tile(cx: &mut Ctx, w: UnitRef) -> Option<TileRef> {
    cx.unit_mut(w).warship_state().patrol_tile
}

/// `spareGold()` (`:605-615`).
fn spare_gold(st: &State, p: SmallId) -> Gold {
    let config = st.config();
    let pr = st.player(p);
    if !config.is_unit_disabled(UnitType::AtomBomb)
        || config.is_unit_disabled(UnitType::HydrogenBomb)
        || pr.unit_count(UnitType::MissileSilo) == 0
    {
        return pr.gold();
    }
    pr.gold() - cost(st, p, UnitType::HydrogenBomb, 0.0)
}

/// `{ player, warship }` (`:494`).
#[derive(Clone, Copy, Debug)]
struct CounterTarget {
    #[allow(dead_code)] // TS carries it; only `warship` is read (`:584-602`)
    player: SmallId,
    warship: UnitRef,
}

/// `enemyTeamWarships` value (`:504-507`).
struct TeamData {
    count: f64,
    players: Vec<SmallId>,
}

impl NationExec {
    /// `warshipBehavior.maybeSpawnWarship()` (`:77-107`).
    pub fn warship_maybe_spawn_warship(&mut self, cx: &mut Ctx) -> bool {
        let Some(player) = self.player else {
            ts_throw!("not initialized");
        };
        if cx.st.config().is_unit_disabled(UnitType::Warship) {
            return false;
        }
        if !self.rng.chance(50.0) {
            return false;
        }
        let ports = cx.st.player_units_of_type_memo(player, UnitType::Port);
        let ships = cx.st.player_units_of_type_memo(player, UnitType::Warship);
        if !ports.is_empty()
            && ships.is_empty()
            && spare_gold(cx.st, player) > cost(cx.st, player, UnitType::Warship, 0.0)
        {
            let port = match self.rng.rand_element(&ports) {
                Ok(&p) => p,
                Err(_) => ts_throw!("array must not be empty"),
            };
            let port_tile = cx.st.unit(port).tile();
            let Some(target_tile) = self.warship_spawn_tile(cx, port_tile, 250.0) else {
                return false;
            };
            // TS `canBuild === false` (:98): strict.
            if cx.st.player(player).can_build(cx.sc, UnitType::Warship, target_tile, None).is_none() {
                return false;
            }
            cx.st.add_execution(Exec::Construction(ConstructionExec::new(
                player,
                UnitType::Warship,
                target_tile,
                None,
                None,
            )));
            return true;
        }
        false
    }

    /// `warshipSpawnTile(portTile, radius)` (`:109-130`).
    fn warship_spawn_tile(&mut self, cx: &mut Ctx, port_tile: TileRef, radius: f64) -> Option<TileRef> {
        let gm = &cx.st.map;
        for _attempts in 0..50 {
            let px = f64::from(gm.x(port_tile));
            let rand_x = self.rng.next_int(px - radius, px + radius);
            let py = f64::from(gm.y(port_tile));
            let rand_y = self.rng.next_int(py - radius, py + radius);
            if !gm.is_valid_coord(rand_x, rand_y) {
                continue;
            }
            let tile = gm.ref_(rand_x, rand_y);
            // Sanity check.
            if !gm.is_water(tile) {
                continue;
            }
            return Some(tile);
        }
        None
    }

    /// `trackShipsAndRetaliate()` (`:132-136`).
    pub fn warship_track_ships_and_retaliate(&mut self, cx: &mut Ctx) {
        self.warship_track_transport_ships_and_retaliate(cx);
        self.warship_track_trade_ships_and_retaliate(cx);
        self.warship_track_incoming_transports_and_retaliate(cx);
    }

    /// `trackTransportShipsAndRetaliate()` (`:139-163`).
    fn warship_track_transport_ships_and_retaliate(&mut self, cx: &mut Ctx) {
        if cx.st.config().is_unit_disabled(UnitType::TransportShip) {
            return;
        }
        let player = me(self);
        // Add any currently owned transport ships to our tracking set.
        for u in cx.st.player_units_of_type_memo(player, UnitType::TransportShip) {
            self.b().warship.tracked_transport_ships.insert(u);
        }

        // iter-mut: live Set iteration deleting the current entry (`:150-162`); the body never
        // touches this set otherwise.
        let mut c = self.b().warship.tracked_transport_ships.cursor();
        while let Some(ship) = self.b().warship.tracked_transport_ships.next_live(&mut c) {
            let (active, by_enemy, destroyer, tile) = {
                let s = cx.st.unit(ship);
                (s.is_active(), s.was_destroyed_by_enemy(), s.destroyer(), s.tile())
            };
            if !active {
                // Distinguish between arrival/retreat and enemy destruction.
                if by_enemy && let Some(destroyer) = destroyer {
                    self.warship_maybe_retaliate_with_warship(cx, tile, destroyer, Reason::Transport);
                }
                self.b().warship.tracked_transport_ships.remove(&ship);
            }
        }
    }

    /// `trackTradeShipsAndRetaliate()` (`:166-185`).
    fn warship_track_trade_ships_and_retaliate(&mut self, cx: &mut Ctx) {
        let player = me(self);
        for u in cx.st.player_units_of_type_memo(player, UnitType::TradeShip) {
            self.b().warship.tracked_trade_ships.insert(u);
        }

        // iter-mut: live Set iteration deleting the current entry (`:174-184`).
        let mut c = self.b().warship.tracked_trade_ships.cursor();
        while let Some(ship) = self.b().warship.tracked_trade_ships.next_live(&mut c) {
            let (active, owner, tile) = {
                let s = cx.st.unit(ship);
                (s.is_active(), s.owner(), s.tile())
            };
            if !active {
                self.b().warship.tracked_trade_ships.remove(&ship);
                continue;
            }
            // `ship.owner().id() !== this.player.id()`: ids ↔ smallIDs are a bijection.
            if owner != player {
                // Ship was ours and is now owned by someone else -> captured.
                self.warship_maybe_retaliate_with_warship(cx, tile, owner, Reason::Trade);
                self.b().warship.tracked_trade_ships.remove(&ship);
            }
        }
    }

    /// `trackIncomingTransportsAndRetaliate()` (`:187-261`).
    fn warship_track_incoming_transports_and_retaliate(&mut self, cx: &mut Ctx) {
        let player = me(self);
        // Add any transports which are targeting us (`game.units(type)` is a copy).
        for p in cx.st.units_of_type_memo(UnitType::TransportShip) {
            let st: &State = cx.st;
            let u = st.unit(p);
            let target = u.target_tile();
            // TS `target && ..`: tile 0 is falsy (:192).
            if let Some(target) = target
                && target != 0
                && u.is_active()
                && !u.transport_ship_state().is_retreating
                && st.map.owner_id(target) == player.0
                && u.owner().0 != player.0
            {
                self.b().warship.tracked_incoming_transport_ships.insert(p);
            }
        }

        // iter-mut: live Set iteration deleting the current entry (`:203-260`); the body adds
        // to / deletes from `dealtWithTransportShip` only.
        let mut c = self.b().warship.tracked_incoming_transport_ships.cursor();
        while let Some(transport) = self.b().warship.tracked_incoming_transport_ships.next_live(&mut c) {
            let (active, target, retreating, ttile, towner) = {
                let u = cx.st.unit(transport);
                let target = u.target_tile();
                // `||` short-circuit: `transportShipState()` is read only when active and targeted.
                let retreating = u.is_active() && target.is_some() && u.transport_ship_state().is_retreating;
                (u.is_active(), target, retreating, u.tile(), u.owner())
            };
            let target = match target {
                Some(t) if active && !retreating => t,
                _ => {
                    self.b().warship.tracked_incoming_transport_ships.remove(&transport);
                    self.b().warship.dealt_with_transport_ship.remove(&transport);
                    continue;
                }
            };
            // Transport has already been dealt with.
            if self.b().warship.dealt_with_transport_ship.contains(&transport) {
                continue;
            }

            let distance_to_target = cx.st.map.manhattan_dist(ttile, target);
            // Too close to deal with.
            if distance_to_target < 20 {
                self.b().warship.dealt_with_transport_ship.insert(transport);
                continue;
            }

            // Possible dock snipe counter? Too niche?
            if !cx.st.player(towner).is_allied_with(player) {
                let nearby = {
                    let st: &State = cx.st;
                    st.has_unit_nearby(target, 90.0, UnitType::Warship, Some(st.player(player).player_id()), true)
                };
                // `||` short-circuit: the filter (and its warshipState() writes) runs only when
                // no warship is nearby (`:231-245`).
                let covered = nearby || {
                    let mut n = 0usize;
                    for w in cx.st.player_units_of_type_memo(player, UnitType::Warship) {
                        let pt = patrol_tile(cx, w);
                        if let Some(pt) = pt
                            && cx.st.map.manhattan_dist(target, pt) < 90
                        {
                            n += 1;
                        }
                    }
                    n > 0
                };
                if covered {
                    self.b().warship.dealt_with_transport_ship.insert(transport);
                    continue;
                }
                let Some(ocean_tiles) = self.warship_spawn_tile(cx, target, 30.0) else {
                    continue;
                };
                self.warship_maybe_retaliate_with_warship(cx, ocean_tiles, towner, Reason::Invasion);
                self.b().warship.dealt_with_transport_ship.insert(transport);
                break;
            }
        }
    }

    /// `maybeRetaliateWithWarship(tile, enemy, reason)` (`:263-306`).
    fn warship_maybe_retaliate_with_warship(&mut self, cx: &mut Ctx, tile: TileRef, enemy: SmallId, reason: Reason) {
        let player = me(self);
        // Don't retaliate against ourselves.
        if enemy == player {
            return;
        }

        // One of ours already patrols there: don't pile up more.
        if !self.warship_warships_covering(cx, tile).is_empty() {
            return;
        }

        // Don't send too many warships.
        if cx.st.player_units_of_type_memo(player, UnitType::Warship).len() >= 10 {
            self.warship_maybe_move_warship(cx, tile);
            return;
        }

        let difficulty = difficulty(cx.st);
        // At most one draw: only the clause matching the difficulty reaches `nextInt` (`:286-290`).
        if (difficulty == Difficulty::Medium && self.rng.next_int(0.0, 100.0) < 15.0)
            || (difficulty == Difficulty::Hard && self.rng.next_int(0.0, 100.0) < 50.0)
            || (difficulty == Difficulty::Impossible && self.rng.next_int(0.0, 100.0) < 80.0)
        {
            let can_build = cx.st.player(player).can_build(cx.sc, UnitType::Warship, tile, None);
            // Stopping an invasion may dip into the gold kept for nukes.
            let affordable =
                reason == Reason::Invasion || spare_gold(cx.st, player) >= cost(cx.st, player, UnitType::Warship, 0.0);
            // TS `canBuild === false` (:296): strict.
            if can_build.is_none() || !affordable {
                self.warship_maybe_move_warship(cx, tile);
                return;
            }
            cx.st.add_execution(Exec::Construction(ConstructionExec::new(player, UnitType::Warship, tile, None, None)));
            self.emoji_maybe_send_emoji(cx, EmojiRecipient::Player(enemy), EMOJI_WARSHIP_RETALIATION);
            cx.player_mut(player).update_relation(enemy, if reason == Reason::Trade { -7.5 } else { -15.0 });
        }
    }

    /// `clearSeaLane(blocker)` (`:310-371`).
    pub fn warship_clear_sea_lane(&mut self, cx: &mut Ctx, blocker: UnitRef) {
        if cx.st.config().is_unit_disabled(UnitType::Warship) {
            return;
        }
        let difficulty = difficulty(cx.st);
        let target = cx.st.unit(blocker).tile();
        // TS computes the fleet before the switch, for Easy/Medium too (`:314`).
        let fleet = self.warship_hostile_warships_near(cx, target) as f64;
        let wanted: f64 = match difficulty {
            Difficulty::Easy | Difficulty::Medium => return,
            Difficulty::Hard => fleet,
            Difficulty::Impossible => fleet + 1.0,
        };

        let player = me(self);
        let covering = self.warship_warships_covering(cx, target);
        let component = cx.rcx().get_water_component(target);
        // Ships with enemies around their patrol tile are busy (`:333-349`).
        let mut spare: Vec<UnitRef> = Vec::new();
        for w in cx.st.player_units_of_type_memo(player, UnitType::Warship) {
            let (mode, pt) = {
                let mut um = cx.unit_mut(w);
                let s = um.warship_state();
                (s.state, s.patrol_tile)
            };
            // `&&` short-circuit in TS order.
            if covering.contains(&w) || mode != WarshipMode::Patrolling {
                continue;
            }
            let wt = cx.st.unit(w).tile();
            if cx.rcx().get_water_component(wt) != component {
                continue;
            }
            let ok = match pt {
                None => true,
                Some(pt) => self.warship_hostile_warships_near(cx, pt) == 0,
            };
            if ok {
                spare.push(w);
            }
        }
        {
            let st: &State = cx.st;
            of_det::js_sort_by(&mut spare, |&a, &b| {
                f64::from(st.map.manhattan_dist(st.unit(a).tile(), target))
                    - f64::from(st.map.manhattan_dist(st.unit(b).tile(), target))
            });
        }
        // `spare.slice(0, Math.max(0, wanted - covering.length))`: `wanted` is an integer here.
        let take = of_det::js::max(0.0, wanted - covering.len() as f64);
        let recruits: Vec<UnitRef> = spare.iter().copied().take(take as usize).collect();
        let builds = wanted - covering.len() as f64 - recruits.len() as f64;
        // Ships sent in one by one get sunk one by one.
        if builds > 0.0 {
            if cx.st.player_units_of_type_memo(player, UnitType::Warship).len() as f64 + builds > 10.0 {
                return;
            }
            // Each ship we own raises the price of the next.
            let mut total: Gold = 0;
            let mut i = 0.0;
            while i < builds {
                total += cost(cx.st, player, UnitType::Warship, i);
                i += 1.0;
            }
            if spare_gold(cx.st, player) < total {
                return;
            }
            if cx.st.player(player).can_build(cx.sc, UnitType::Warship, target, None).is_none() {
                return;
            }
        }
        for &w in &recruits {
            cx.unit_mut(w)
                .update_warship_state(WarshipStateUpdate { patrol_tile: Some(Some(target)), ..Default::default() });
        }
        let mut i = 0.0;
        while i < builds {
            cx.st.add_execution(Exec::Construction(ConstructionExec::new(
                player,
                UnitType::Warship,
                target,
                None,
                None,
            )));
            i += 1.0;
        }
    }

    /// `hostileWarshipsNear(tile)` (`:373-382`).
    fn warship_hostile_warships_near(&mut self, cx: &mut Ctx, tile: TileRef) -> usize {
        let player = me(self);
        let st: &State = cx.st;
        let range = st.config().warship_targetting_range();
        st.nearby_units(tile, range, &[UnitType::Warship], |_, _, _| true, false)
            .into_iter()
            .filter(|nu| st.player(st.unit(nu.unit).owner()).can_attack_player(player, true))
            .count()
    }

    /// `warshipsCovering(tile)` (`:385-396`): our warships on the same water whose patrol area
    /// includes `tile`.
    fn warship_warships_covering(&mut self, cx: &mut Ctx, tile: TileRef) -> Vec<UnitRef> {
        let player = me(self);
        let component = cx.rcx().get_water_component(tile);
        let patrol_range = cx.st.config().warship_patrol_range();
        let mut out = Vec::new();
        for w in cx.st.player_units_of_type_memo(player, UnitType::Warship) {
            let pt = patrol_tile(cx, w);
            // `&&` short-circuit: the component lookup only runs for a defined patrol tile.
            let Some(pt) = pt else { continue };
            let wt = cx.st.unit(w).tile();
            // `number | null` strict equality: `null === null` holds.
            if cx.rcx().get_water_component(wt) != component {
                continue;
            }
            if f64::from(cx.st.map.manhattan_dist(pt, tile)) <= patrol_range {
                out.push(w);
            }
        }
        out
    }

    /// `maybeMoveWarship(tile)` (`:398-422`).
    fn warship_maybe_move_warship(&mut self, cx: &mut Ctx, tile: TileRef) {
        // Make sure we are targeting water.
        if !cx.st.map.is_water(tile) {
            return;
        }
        let player = me(self);
        let mut candidates = Vec::new();
        for p in cx.st.player_units_of_type_memo(player, UnitType::Warship) {
            let pt = patrol_tile(cx, p);
            // Don't send ships which are already traveling.
            if let Some(pt) = pt
                && cx.st.map.manhattan_dist(cx.st.unit(p).tile(), pt) < 130
            {
                candidates.push(p);
            }
        }
        {
            let st: &State = cx.st;
            // Sort by distance (closest first) (`:411-416`).
            of_det::js_sort_by(&mut candidates, |&a, &b| {
                let dist_a = f64::from(st.map.manhattan_dist(st.unit(a).tile(), tile));
                let dist_b = f64::from(st.map.manhattan_dist(st.unit(b).tile(), tile));
                dist_a - dist_b
            });
        }
        if let Some(&warship) = candidates.first() {
            cx.unit_mut(warship)
                .update_warship_state(WarshipStateUpdate { patrol_tile: Some(Some(tile)), ..Default::default() });
        }
    }

    /// `counterWarshipInfestation()` (`:426-441`).
    pub fn warship_counter_warship_infestation(&mut self, cx: &mut Ctx) {
        if !self.warship_should_counter_warship_infestation(cx) {
            return;
        }
        let is_team_game = cx.st.player(me(self)).team().is_some();
        if !self.warship_is_rich_player(cx, is_team_game) {
            return;
        }
        let target = if is_team_game {
            self.warship_find_team_game_warship_target(cx)
        } else {
            self.warship_find_free_for_all_warship_target(cx)
        };
        if let Some(target) = target {
            self.warship_build_counter_warship(cx, target);
        }
    }

    /// `shouldCounterWarshipInfestation()` (`:443-478`).
    fn warship_should_counter_warship_infestation(&mut self, cx: &mut Ctx) -> bool {
        let player = me(self);
        let st: &State = cx.st;
        if st.config().is_unit_disabled(UnitType::Warship) {
            return false;
        }
        // Only the smart nations can do this.
        let difficulty = difficulty(st);
        if difficulty != Difficulty::Hard && difficulty != Difficulty::Impossible {
            return false;
        }
        // Quit early if there aren't many warships in the game.
        if st.unit_count(UnitType::Warship) <= 10 {
            return false;
        }
        // Quit early if we can't afford a warship.
        if cost(st, player, UnitType::Warship, 0.0) > spare_gold(st, player) {
            return false;
        }
        // Quit early if we don't have a port to send warships from.
        if st.player(player).units_of(&[UnitType::Port]).is_empty() {
            return false;
        }
        // Don't send too many warships.
        if st.player(player).units_of(&[UnitType::Warship]).len() >= 10 {
            return false;
        }
        true
    }

    /// `isRichPlayer(isTeamGame)` (`:481-490`).
    fn warship_is_rich_player(&mut self, cx: &mut Ctx, is_team_game: bool) -> bool {
        let player = me(self);
        let st: &State = cx.st;
        let my_team = st.player(player).team();
        let mut players: Vec<SmallId> = st
            .players()
            .into_iter()
            .filter(|&p| {
                let pr = st.player(p);
                if pr.player_type() == PlayerType::Human {
                    return false;
                }
                if is_team_game { pr.team() == my_team } else { true }
            })
            .collect();
        // `.sort((a, b) => Number(b.gold() - a.gold()))` (`:487`): bigint difference, then Number.
        of_det::js_sort_by(&mut players, |&a, &b| (st.player(b).gold() - st.player(a).gold()) as f64);
        players.iter().take(3).any(|&p| p == player)
    }

    /// `findTeamGameWarshipTarget()` (`:500-561`).
    fn warship_find_team_game_warship_target(&mut self, cx: &mut Ctx) -> Option<CounterTarget> {
        let player = me(self);
        let st: &State = cx.st;
        // Iterated with `.entries()` (`:536`): insertion order.
        let mut enemy_team_warships: JsMap<String, TeamData> = JsMap::new();

        let mut own_fleet = 0.0;
        let me_ref = st.player(player);
        for p in st.players() {
            // Skip friendly players (our team and allies), but count their warships.
            if me_ref.is_friendly(p, false) || p == player {
                own_fleet += st.player(p).units_of(&[UnitType::Warship]).len() as f64;
                continue;
            }

            let Some(team) = st.player(p).team() else {
                continue;
            };
            let team_key = team.clone(); // `team.toString()`: Team is a string
            let warship_count = st.player(p).units_of(&[UnitType::Warship]).len() as f64;

            if !enemy_team_warships.contains_key(&team_key) {
                enemy_team_warships.insert(team_key.clone(), TeamData { count: 0.0, players: Vec::new() });
            }
            let team_data = enemy_team_warships.get_mut(&team_key).ts_expect("NationWarshipBehavior.ts:530");
            team_data.count += warship_count;
            team_data.players.push(p);
        }

        // Find team with more than 15 warships that outnumbers our team and allies.
        // iter-live: the loop body never touches the map.
        for (_, team_data) in enemy_team_warships.iter() {
            if team_data.count > 15.0 && team_data.count > own_fleet {
                // Find player in that team with most warships (`reduce` from `null`, `:539-546`).
                let mut player_with_most: Option<SmallId> = None;
                for &p in &team_data.players {
                    let count = st.player(p).units_of(&[UnitType::Warship]).len();
                    let max_count = player_with_most.map_or(0, |m| st.player(m).units_of(&[UnitType::Warship]).len());
                    if count > max_count {
                        player_with_most = Some(p);
                    }
                }

                if let Some(pwm) = player_with_most {
                    let warships = st.player(pwm).units_of(&[UnitType::Warship]);
                    if warships.len() > 3 {
                        let warship = match self.rng.rand_element(&warships) {
                            Ok(&w) => w,
                            Err(_) => ts_throw!("array must not be empty"),
                        };
                        return Some(CounterTarget { player: pwm, warship });
                    }
                }
            }
        }

        None
    }

    /// `findFreeForAllWarshipTarget()` (`:563-582`).
    fn warship_find_free_for_all_warship_target(&mut self, cx: &mut Ctx) -> Option<CounterTarget> {
        let player = me(self);
        let st: &State = cx.st;
        let me_ref = st.player(player);
        let enemies: Vec<SmallId> =
            st.players().into_iter().filter(|&p| !me_ref.is_friendly(p, false) && p != player).collect();

        for enemy in enemies {
            let enemy_warships = st.player(enemy).units_of(&[UnitType::Warship]);
            if enemy_warships.len() > 10 {
                let warship = match self.rng.rand_element(&enemy_warships) {
                    Ok(&w) => w,
                    Err(_) => ts_throw!("array must not be empty"),
                };
                return Some(CounterTarget { player: enemy, warship });
            }
        }

        None
    }

    /// `buildCounterWarship(target)` (`:584-602`).
    fn warship_build_counter_warship(&mut self, cx: &mut Ctx, target: CounterTarget) {
        let player = me(self);
        let wtile = cx.st.unit(target.warship).tile();
        // TS `canBuild === false` (:589): strict.
        if cx.st.player(player).can_build(cx.sc, UnitType::Warship, wtile, None).is_none() {
            self.warship_maybe_move_warship(cx, wtile);
            return;
        }

        cx.st.add_execution(Exec::Construction(ConstructionExec::new(player, UnitType::Warship, wtile, None, None)));
        self.emoji_send_emoji(cx, EmojiRecipient::AllPlayers, EMOJI_WARSHIP_RETALIATION);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canon::{RefLog, Refs};
    use crate::exec::ExecStore;
    use crate::state::unit::Unit;

    fn unit(st: &mut State, id: u32) -> UnitRef {
        st.units.alloc(Unit { id, ..Default::default() })
    }

    /// CS export order = set insertion order, dead units included (`:47-54`), and the GC/CS
    /// handle walk visits every tracked unit in the same field order.
    #[test]
    fn canon_and_refs_follow_set_order() {
        let mut st = State::default();
        let a = unit(&mut st, 7);
        let b = unit(&mut st, 3);
        let c = unit(&mut st, 9);
        let mut w = WarshipB::default();
        w.tracked_transport_ships.insert(a);
        w.tracked_transport_ships.insert(b);
        w.tracked_trade_ships.insert(c);
        w.tracked_incoming_transport_ships.insert(b);
        w.dealt_with_transport_ship.insert(c);
        // delete + re-add moves to the end (JS Set semantics)
        w.tracked_transport_ships.remove(&a);
        w.tracked_transport_ships.insert(a);

        let ex = ExecStore::default();
        let mut cx = CanonCx::new(&st, &ex);
        let got = w.canon(&mut cx);
        let arr = |xs: &[i64]| Cs::Arr(xs.iter().map(|&x| Cs::Int(x)).collect());
        assert_eq!(
            got,
            Cs::Obj(vec![
                ("trackedTransportShips".into(), arr(&[3, 7])),
                ("trackedTradeShips".into(), arr(&[9])),
                ("trackedIncomingTransportShips".into(), arr(&[3])),
                ("dealtWithTransportShip".into(), arr(&[9])),
            ])
        );
        // First-reference order of unit rows.
        assert_eq!(cx.unit_queue, vec![b, a, c]);

        let mut log = RefLog::default();
        w.visit_refs(&mut log);
        assert_eq!(log.units, vec![b, a, c, b, c]);
    }

    /// The live-cursor pattern used by the three tracking loops: deleting the current entry
    /// visits every other entry exactly once (JS `Set` semantics).
    #[test]
    fn delete_current_during_iteration_visits_all() {
        let mut st = State::default();
        let us: Vec<UnitRef> = (0..20).map(|i| unit(&mut st, i)).collect();
        let mut set: SmallJsSet<UnitRef, 8> = SmallJsSet::default();
        for &u in &us {
            set.insert(u);
        }
        let mut seen = Vec::new();
        let mut c = set.cursor();
        while let Some(u) = set.next_live(&mut c) {
            seen.push(u);
            if st.units.get(u).id % 2 == 0 {
                set.remove(&u);
            }
        }
        assert_eq!(seen, us);
        let left: Vec<u32> = set.iter().map(|&u| st.units.get(u).id).collect();
        assert_eq!(left, (0..20).filter(|i| i % 2 == 1).collect::<Vec<_>>());
    }
}
