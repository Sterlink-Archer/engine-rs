//! `AttackExecution` (`engine/src/execution/AttackExecution.ts`). Owner: **P6**.
//!
//! Fields: the `Attack v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1). The TS caches
//! `ownerSmallID`/`targetSmallID` (`:51-52`) are `owner`/`target` themselves (handles are
//! small ids), and the neighbour buffers `nbuf`/`nbuf2` (`:54-55`) are stack arrays.
//!
//! The hot loop (`tick`, `:312-359`) and `addNeighbors` (`:415-463`) read tile owners through
//! the main map exactly where TS reads `this.map.ownerID`.

#[allow(unused_imports)]
use crate::exec::prelude::*;

use crate::api::types::{Difficulty, Relation, TerrainType};
use crate::config::engine_config::{AttackLogicAttacker, AttackLogicDefender, AttackLogicInput};
use crate::state::stats::StatNum;

/// `malusForRetreat = 25` (`:35`).
const MALUS_FOR_RETREAT: f64 = 25.0;

/// `AttackExecution` data.
#[derive(StateClone, Debug)]
pub struct AttackExec {
    pub active: bool,
    pub initialized: bool,
    /// `toConquer = new FlatBinaryHeap()` (`:38`)
    pub to_conquer: FlatBinaryHeap,
    /// `random = new PseudoRandom(123)`, seeded at construction (`:40`, ARCHITECTURE §4.5)
    pub rng: Prng,
    /// `target: Player | TerraNullius` (set in `init`)
    pub target: Option<Owner>,
    pub attack: Option<AttackRef>,
    pub start_troops: Option<f64>,
    pub owner: SmallId,
    pub target_id: Option<String>,
    pub source_tile: Option<TileRef>,
    pub remove_troops: bool,
}

impl AttackExec {
    /// `new AttackExecution(startTroops = null, owner, targetID, sourceTile = null, removeTroops = true)` (`:57-63`).
    /// The PRNG is seeded here (field initialiser `:40`), not in `init`.
    pub fn new(
        start_troops: Option<f64>,
        owner: SmallId,
        target_id: Option<String>,
        source_tile: Option<TileRef>,
        remove_troops: bool,
    ) -> Self {
        AttackExec {
            active: true,
            initialized: false,
            // TS: `new FlatBinaryHeap()` (default capacity 1024, `FlatBinaryHeap.ts:14`;
            // capacity is not state, PORTING_GUIDE §13).
            to_conquer: FlatBinaryHeap::new(1024),
            rng: Prng::new(123.0),
            target: None,
            attack: None,
            start_troops,
            owner,
            target_id,
            source_tile,
            remove_troops,
        }
    }

    /// `targetID()` (`:65-67`).
    pub fn target_id(&self) -> Option<&str> {
        self.target_id.as_deref()
    }

    /// `owner()` (`:501-503`).
    pub fn owner(&self) -> SmallId {
        self.owner
    }

    /// `this.attack` with TS's `if (this.attack === null) throw new Error("Attack not
    /// initialized")` (`:228-230, :239-241, :273-275, :416-418`).
    #[inline]
    fn attack_ref(&self) -> AttackRef {
        match self.attack {
            Some(a) => a,
            None => of_det::ts_throw!("Attack not initialized"),
        }
    }

    /// `this.target` after `init` set it. Every reader runs after `init` resolved it (TS reads
    /// the field; an unset field is a TS `undefined` deref at the same sites).
    #[inline]
    fn target_owner(&self) -> Owner {
        self.target.ts_expect("AttackExecution.target (unset before init)")
    }

    /// `rejectIncomingAllianceRequests(target)` (`:406-413`).
    fn reject_incoming_alliance_requests(&self, cx: &mut Ctx, target: SmallId) {
        // `incomingAllianceRequests()` is a fresh filter (a copy); `find` takes the first.
        let reqs = cx.st.player(self.owner).incoming_alliance_requests();
        let request = reqs.into_iter().find(|&r| cx.st.alliance_request(r).requestor == target);
        if let Some(r) = request {
            cx.reject_alliance_request(r);
        }
    }

    /// `for (const outgoing of this._owner.outgoingAttacks()) { .. }` (`:169-179`): absorb
    /// every other land attack of `owner` on the same target into `a`.
    fn merge_outgoing_attacks(cx: &mut Ctx, owner: SmallId, a: AttackRef) {
        // TS: :169-179 `for (const outgoing of this._owner.outgoingAttacks())`: the live
        // array; `outgoing.delete()` filter-reassigns it, so the running loop keeps walking
        // the old array object (ARCHITECTURE §4.7).
        // iter-snapshot: filter-reassign (no push to the owner's outgoing list in the body).
        let mut it = cx.st.player_data(owner).outgoing_attacks.iter_snapshot(&mut cx.sc.snap_pool);
        while let Some(outgoing) = it.next(&cx.st.player_data(owner).outgoing_attacks) {
            if outgoing != a
                    && cx.st.attack(outgoing).target() == cx.st.attack(a).target()
                    // Boat attacks (sourceTile is not null) are not combined with other attacks
                    && cx.st.attack(a).source_tile().is_none()
            {
                let t = cx.st.attack(a).troops() + cx.st.attack(outgoing).troops();
                cx.st.attack_mut(a).set_troops(t);
                cx.attack_delete(outgoing);
            }
        }
        it.finish(&mut cx.sc.snap_pool);
    }

    /// `refreshToConquer()` (`:227-236`).
    fn refresh_to_conquer(&mut self, cx: &mut Ctx) {
        let a = self.attack_ref();
        self.to_conquer.clear();
        cx.st.attack_mut(a).clear_border();
        // TS: `this._owner.borderTiles().forEach((tile) => this.addNeighbors(tile))`.
        // iter-live: addNeighbors writes only the attack border and the heap, never the
        // owner's border set; the cursor re-reads the dense length like TileSet.forEach.
        let mut c = cx.st.player_data(self.owner).border_tiles.cursor();
        while let Some(tile) = cx.st.player_data(self.owner).border_tiles.next_live(&mut c) {
            self.add_neighbors(cx, tile);
        }
    }

    /// `retreat(malusPercent = 0)` (`:238-270`).
    fn retreat(&mut self, cx: &mut Ctx, malus_percent: f64) {
        let a = self.attack_ref();
        let deaths = cx.st.attack(a).troops() * (malus_percent / 100.0);
        // TS: :244-252 `if (deaths) displayMessage(..)`: view only (renderTroops is pure).
        if !self.remove_troops && self.source_tile.is_none() {
            // TS: :253-258
            let t = cx.st.attack(a).troops() - self.start_troops.unwrap_or(0.0);
            cx.st.attack_mut(a).set_troops(t);
        }
        let survivors = cx.st.attack(a).troops() - deaths;
        cx.player_mut(self.owner).add_troops(survivors);
        cx.attack_delete(a);
        self.active = false;
        // TS: :266-269 (not all retreats are cancelled attacks)
        if cx.st.attack(a).retreated() {
            let target = self.target_owner();
            cx.st.stats_cx().attack_cancel(self.owner, target, StatNum::Num(survivors));
        }
    }

    /// `attackLogicInput(attackTroops, tile, borderSize)` (`:362-404`).
    fn attack_logic_input(&self, cx: &Ctx, attack_troops: f64, tile: TileRef, border_size: f64) -> AttackLogicInput {
        let st = &*cx.st;
        let target = self.target_owner();
        let defender = if target.is_player() { Some(target) } else { None };
        // TS: :372-379 (`defender !== null && mg.hasUnitNearby(tile, range, DefensePost, defender.id())`)
        let defender_has_defense_post = match defender {
            // t409 H5: the handle form (same result, no id string scan)
            Some(d) => st.has_unit_nearby_of(tile, st.config().defense_post_range(), UnitType::DefensePost, d, false),
            None => false,
        };
        let owner = st.player(self.owner);
        AttackLogicInput {
            terrain: st.map.terrain_type(tile),
            attack_troops,
            attacker: AttackLogicAttacker {
                player_type: owner.player_type(),
                num_tiles: f64::from(owner.num_tiles_owned()),
            },
            defender: defender.map(|d| {
                let dp = st.player(d);
                AttackLogicDefender {
                    player_type: dp.player_type(),
                    num_tiles: f64::from(dp.num_tiles_owned()),
                    troops: dp.troops(),
                    is_traitor: dp.is_traitor(),
                    // TS: :395-396 (`&&` short-circuits)
                    is_disconnected_teammate: dp.is_disconnected() && owner.is_on_same_team(d),
                }
            }),
            defender_has_defense_post,
            // TS: :399-401 `mg.hasFallout(tile) ? numTilesWithFallout / numLandTiles : null`
            fallout_ratio: if st.map.has_fallout(tile) {
                Some(f64::from(st.num_tiles_with_fallout()) / f64::from(st.map.num_land_tiles()))
            } else {
                None
            },
            border_size,
        }
    }

    /// `addNeighbors(tile)` (`:415-463`).
    fn add_neighbors(&mut self, cx: &mut Ctx, tile: TileRef) {
        let a = self.attack_ref();
        let target_small_id = self.target_owner().0;
        let owner_small_id = self.owner.0;
        let tick_now = f64::from(cx.st.ticks()); // cache tick
        let mut nbuf = [0u32; 4];
        let mut nbuf2 = [0u32; 4];
        let num_neighbors = cx.st.map.neighbors4(tile, &mut nbuf);
        for &neighbor in &nbuf[..num_neighbors] {
            let map = &cx.st.map;
            if map.is_water(neighbor) || map.is_impassable(neighbor) || map.owner_id(neighbor) != target_small_id {
                continue;
            }
            cx.st.attack_mut(a).add_border_tile(neighbor);
            let map = &cx.st.map;
            let mut num_owned_by_me = 0u32;
            let num_inner = map.neighbors4(neighbor, &mut nbuf2);
            for &n2 in &nbuf2[..num_inner] {
                if map.owner_id(n2) == owner_small_id {
                    num_owned_by_me += 1;
                }
            }
            let mag: f64 = match map.terrain_type(neighbor) {
                TerrainType::Plains => 1.0,
                TerrainType::Highland => 1.5,
                TerrainType::Mountain => 2.0,
                _ => 0.0,
            };
            // TS: :457-459, expression shape kept literally.
            let priority =
                (self.rng.next_int(0.0, 7.0) + 10.0) * (1.0 - f64::from(num_owned_by_me) * 0.5 + mag / 2.0) + tick_now;
            self.to_conquer.enqueue(neighbor, priority);
        }
    }

    /// `handleDeadDefender()` (`:465-499`).
    fn handle_dead_defender(&mut self, cx: &mut Ctx) {
        let target = self.target_owner();
        if !(target.is_player() && cx.st.player(target).num_tiles_owned() < 100) {
            return;
        }
        let owner = self.owner;
        cx.conquer_player(owner, target);

        const MAX_PASSES: u32 = 100;
        let mut nbuf = [0u32; 4];
        for _pass in 0..MAX_PASSES {
            let mut progressed = false;
            // TS: `for (const tile of target.tiles())` — a live TileSet iteration: tiles the
            // body conquers away are tombstoned (skipped if not yet visited); the cursor
            // re-reads the dense length. iter-mut: deletes only (conquer moves tiles out).
            let mut c = cx.st.player_data(target).tiles.cursor();
            while let Some(tile) = cx.st.player_data(target).tiles.next_live(&mut c) {
                // TS: :475-480 (`mg.owner(t) === this._owner`); the callback only reads.
                let n = cx.st.map.neighbors4(tile, &mut nbuf);
                let mut borders = false;
                for &t in &nbuf[..n] {
                    if !borders && cx.st.map.owner_id(t) == owner.0 {
                        borders = true;
                    }
                }
                if borders {
                    cx.player_mut(owner).conquer(tile);
                    progressed = true;
                } else {
                    // TS: :485-494. The callback conquers `tile` itself, never a neighbour, and
                    // every later callback returns at once (`if (captured) return`), so
                    // walking the pre-filled neighbour buffer is the same sequence.
                    let mut captured = false;
                    for &neighbor in &nbuf[..n] {
                        if captured {
                            continue;
                        }
                        let no = cx.st.map.owner_id(neighbor);
                        let no = SmallId(no);
                        if no.is_player() && no != target && !cx.st.player(no).is_friendly(target, false) {
                            // TS: `this.mg.player(no.id()).conquer(tile)` (same player as `no`)
                            cx.player_mut(no).conquer(tile);
                            captured = true;
                        }
                    }
                    if captured {
                        progressed = true;
                    }
                }
            }
            if !progressed {
                break;
            }
        }
    }
}

impl ExecBehavior for AttackExec {
    fn is_active(&self) -> bool {
        self.active
    }
    /// `activeDuringSpawnPhase()` returns `false` (`AttackExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    /// `init(mg, ticks)` (`:73-225`).
    fn init(&mut self, cx: &mut Ctx, _ticks: i32) {
        if !self.active {
            return;
        }
        self.initialized = true; // TS: `this.mg = mg; this.map = mg.map()`
        let owner = self.owner;

        // TS: :80-84
        if let Some(tid) = self.target_id.as_deref()
            && !cx.st.has_player(tid)
        {
            self.active = false;
            return;
        }

        // TS: :86-89 (`_targetID === terraNullius().id()`: TerraNullius' id is `null`)
        let target: Owner = match self.target_id.as_deref() {
            None => SmallId::TERRA_NULLIUS,
            Some(tid) => cx.st.player_by_id(tid),
        };
        self.target = Some(target);

        // TS: :93-97
        if owner == target {
            self.active = false;
            return;
        }

        // TS: :99-109 ALLIANCE CHECK
        if target.is_player() && cx.st.player(owner).is_friendly(target, false) {
            self.active = false;
            return;
        }

        // TS: :111-121
        if target.is_player()
            && cx.st.player(target).player_type() != PlayerType::Bot
            && cx.st.player(owner).player_type() != PlayerType::Bot
        {
            cx.player_mut(target).add_embargo(owner, true);
            self.reject_incoming_alliance_requests(cx, target);
        }

        // TS: :123-126
        if target.is_player() && !cx.st.player(owner).can_attack_player(target, false) {
            self.active = false;
            return;
        }

        // TS: :128-130 `startTroops ??= config.attackAmount(owner, target)`
        let mut start_troops = match self.start_troops {
            Some(s) => s,
            None => {
                let st = &*cx.st;
                let att = st.player(owner);
                if target.is_player() {
                    st.config().attack_amount(&att, Some(&st.player(target)))
                } else {
                    st.config().attack_amount(&att, None::<&crate::state::player::PlayerRef<'_>>)
                }
            }
        };
        self.start_troops = Some(start_troops);
        if self.remove_troops {
            // TS: :132-137
            start_troops = of_det::js::min(cx.st.player(owner).troops(), start_troops);
            self.start_troops = Some(start_troops);
            start_troops = cx.player_mut(owner).remove_troops(start_troops);
            self.start_troops = Some(start_troops);
        }
        // TS: :139-144
        let a = cx.player_mut(owner).create_attack(target, start_troops, self.source_tile, OrderedTileSet::new());
        self.attack = Some(a);

        // TS: :146-150
        match self.source_tile {
            Some(st) => self.add_neighbors(cx, st),
            None => self.refresh_to_conquer(cx),
        }

        // TS: :153 Record stats
        cx.st.stats_cx().attack(owner, target, StatNum::Num(start_troops));

        // TS: :155-168. `incomingAttacks()` is a fresh filter (a copy, PlayerImpl.ts:1903-1904).
        let incoming_list = cx.st.player(owner).incoming_attacks();
        for incoming in incoming_list {
            if cx.st.attack(incoming).attacker() == target {
                // Target has opposing attack, cancel them out
                let inc_troops = cx.st.attack(incoming).troops();
                let my_troops = cx.st.attack(a).troops();
                if inc_troops > my_troops {
                    cx.st.attack_mut(incoming).set_troops(inc_troops - my_troops);
                    cx.attack_delete(a);
                    self.active = false;
                    return;
                } else {
                    cx.st.attack_mut(a).set_troops(my_troops - inc_troops);
                    cx.attack_delete(incoming);
                }
            }
        }

        // TS: :169-179
        Self::merge_outgoing_attacks(cx, owner, a);

        // TS: :186-189
        if self.to_conquer.size() == 0 {
            self.retreat(cx, 0.0);
            return;
        }

        // TS: :196
        let troops = cx.st.attack(a).troops();
        cx.st.stats_cx().attack_max_incoming(target, StatNum::Num(troops));

        // TS: :198
        if !target.is_player() {
            return;
        }
        let difficulty = cx.st.config().game_config().difficulty;
        // TS: :201-206 (hitting back at whoever just attacked us doesn't anger them, except on Easy)
        if difficulty != Difficulty::Easy && cx.st.player(owner).relation(target) == Relation::Hostile {
            return;
        }
        let relation_change = match difficulty {
            Difficulty::Easy => -60.0,
            Difficulty::Medium => -70.0,
            Difficulty::Hard => -80.0,
            Difficulty::Impossible => -100.0,
        };
        cx.player_mut(target).update_relation(owner, relation_change);
    }

    /// `tick(ticks)` (`:272-360`).
    fn tick(&mut self, cx: &mut Ctx, _ticks: i32) {
        let a = self.attack_ref();
        let mut troop_count = cx.st.attack(a).troops(); // cache troop count
        let target = self.target_owner();
        let target_is_player = target.is_player(); // cache target type
        let target_player = if target_is_player { Some(target) } else { None };

        // TS: :283-286 (must come before the retreat refund)
        if !cx.st.attack(a).is_active() {
            self.active = false;
            return;
        }

        // TS: :288-296
        if cx.st.attack(a).retreated() {
            if target_is_player {
                self.retreat(cx, MALUS_FOR_RETREAT);
            } else {
                self.retreat(cx, 0.0);
            }
            self.active = false;
            return;
        }

        // TS: :298-300
        if cx.st.attack(a).retreating() {
            return;
        }

        // TS: :302-306 (a new alliance was created AFTER the attack started)
        if let Some(tp) = target_player
            && cx.st.player(self.owner).is_friendly(tp, false)
        {
            self.retreat(cx, 0.0);
            return;
        }

        // TS: :308 (`borderSize() + random.nextInt(0, 5)`, a JS number)
        let border_size = f64::from(cx.st.attack(a).border_size()) + self.rng.next_int(0.0, 5.0);
        // Each tile consumes a fraction of the tick; conquer until it is spent.
        let mut tick_budget = 1.0f64;
        let owner_small_id = self.owner.0;
        let target_small_id = target.0;
        let mut nbuf = [0u32; 4];

        while tick_budget > 0.0 {
            // TS: :313-317
            if troop_count < 1.0 {
                cx.attack_delete(a);
                self.active = false;
                return;
            }

            // TS: :319-323
            if self.to_conquer.size() == 0 {
                self.refresh_to_conquer(cx);
                self.retreat(cx, 0.0);
                return;
            }

            // TS: :325 (`dequeue()` throws "heap empty" only on an empty heap, excluded above)
            let tile_to_conquer = match self.to_conquer.dequeue() {
                Ok(t) => t,
                Err(_) => of_det::ts_throw!("heap empty"),
            };
            cx.st.attack_mut(a).remove_border_tile(tile_to_conquer);

            // TS: :328-335
            let map = &cx.st.map;
            let mut on_border = false;
            let num_neighbors = map.neighbors4(tile_to_conquer, &mut nbuf);
            for &n in &nbuf[..num_neighbors] {
                if map.owner_id(n) == owner_small_id {
                    on_border = true;
                    break;
                }
            }
            if map.owner_id(tile_to_conquer) != target_small_id || !on_border {
                continue;
            }
            if !map.is_land(tile_to_conquer) || map.is_impassable(tile_to_conquer) {
                continue;
            }
            self.add_neighbors(cx, tile_to_conquer);
            // TS: :346-350
            let input = self.attack_logic_input(cx, troop_count, tile_to_conquer, border_size);
            let r = cx.st.config().attack_logic(&input);
            tick_budget -= r.tick_fraction;
            troop_count -= r.attacker_troop_loss;
            cx.st.attack_mut(a).set_troops(troop_count);
            if let Some(tp) = target_player {
                cx.player_mut(tp).remove_troops(r.defender_troop_loss);
            }
            cx.player_mut(self.owner).conquer(tile_to_conquer);
            self.handle_dead_defender(cx);
        }
    }
}

// CS v1 `Attack v1 (execution)` record (`AttackExecution.ts:509-523`), dump field order.
// `toConquer` is the heap layout without `capacity` (SPEC N3, P13's `FlatBinaryHeap` leaf);
// `target` is `null` before `init` (`this.target === undefined ? null : w.owner(..)`);
// `attack` is the attack key. Written at merge1 by the integrator on behalf of P6.
crate::impl_canon_record!(AttackExec {
    "active" => active,
    "initialized" => initialized,
    "toConquer" => to_conquer,
    "random" => rng,
    "target" => target,
    "attack" => attack,
    "startTroops" => start_troops,
    "owner" => owner,
    "targetID" => target_id,
    "sourceTile" => source_tile,
    "removeTroops" => remove_troops,
});

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// The PRNG is seeded at construction (`AttackExecution.ts:40`): two fresh executions draw
    /// the same stream as `new PseudoRandom(123)`.
    #[test]
    fn constructor_seeds_prng_and_defaults() {
        let mut e = AttackExec::new(None, SmallId(3), Some("abc".into()), None, true);
        let mut r = Prng::new(123.0);
        assert_eq!(e.rng.next_int(0.0, 7.0), r.next_int(0.0, 7.0));
        assert!(e.active && !e.initialized && e.attack.is_none() && e.target.is_none());
        assert_eq!(e.to_conquer.size(), 0);
        assert_eq!((e.owner(), e.target_id()), (SmallId(3), Some("abc")));
        assert!(e.is_active());
        assert!(!e.active_during_spawn_phase());
    }

    /// `tick` before `init` resolved an attack is TS's `throw new Error("Attack not
    /// initialized")` (`:273-275`): a `TsThrow`, not a Rust bug.
    #[test]
    #[allow(clippy::disallowed_methods)] // tests may catch panics to assert them
    fn tick_without_attack_is_a_ts_throw() {
        let mut st = crate::state::State::default();
        let mut sc = crate::scratch::Scratch::new();
        let mut e = AttackExec::new(Some(5.0), SmallId(1), None, None, true);
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut cx = Ctx::new(&mut st, &mut sc, None);
            e.tick(&mut cx, 0);
        }));
        let p = r.expect_err("must throw");
        let t = of_det::throw::as_ts_throw(&*p).expect("a TsThrow payload");
        assert_eq!(&*t.msg, "Attack not initialized");
    }

    /// `init` on an inactive execution returns before touching anything (`:74-76`), and
    /// `initialized` stays false (`this.mg` is never set).
    #[test]
    fn init_on_inactive_is_a_noop() {
        let mut st = crate::state::State::default();
        let mut sc = crate::scratch::Scratch::new();
        let mut e = AttackExec::new(Some(5.0), SmallId(1), Some("x".into()), None, true);
        e.active = false;
        let mut cx = Ctx::new(&mut st, &mut sc, None);
        e.init(&mut cx, 0);
        assert!(!e.initialized && e.target.is_none());
    }

    /// A bare `Player` record (no P2 constructor needed): only the list fields matter here.
    pub(crate) fn blank_player(small_id: SmallId) -> crate::state::player::Player {
        use crate::state::player::Player;
        Player {
            small_id,
            info: crate::ids::PlayerInfoIdx(0),
            team: None,
            last_tile_change: 0,
            tile_change_version: 0,
            rng: Prng::new(0.0),
            gold: 0,
            troops: 0,
            trade_gold: 0,
            train_gold: 0,
            piracy_gold: 0,
            gold_earned: 0,
            marked_traitor_tick: -1,
            marked_doomsday_clock_tick: -1,
            rotted_at_tick: -1,
            betrayal_count: 0,
            embargoes: Default::default(),
            tiles: Default::default(),
            border_tiles: Default::default(),
            units: Default::default(),
            my_units_version: 0,
            past_outgoing_alliance_requests: Vec::new(),
            expired_alliances: Vec::new(),
            targets: Vec::new(),
            outgoing_emojis: Vec::new(),
            outgoing_quick_chats: Default::default(),
            sent_donations: Vec::new(),
            relations: Default::default(),
            last_delete_unit_tick: -1,
            last_embargo_all_tick: -1,
            incoming_attacks: Default::default(),
            outgoing_attacks: Default::default(),
            outgoing_land_attacks: Vec::new(),
            alliances: Default::default(),
            spawn_tile: None,
            is_disconnected: false,
            disconnect_snapshot: None,
            largest_cluster_bbox: Default::default(),
            num_units_constructed: Vec::new(),
            my_units_memo: Default::default(),
            my_unit_count_memo: Default::default(),
            my_units_owned_memo: Default::default(),
            nearby_memo: None,
            reachable_components_memo: None,
            conn4: false,
            comps: Default::default(),
        }
    }

    fn add_attack(st: &mut crate::state::State, id: u64, attacker: SmallId, target: Owner, troops: f64) -> AttackRef {
        use crate::state::attack::Attack;
        let a = st.attacks.alloc(Attack::new(
            crate::ids::AttackId(id),
            target,
            attacker,
            troops,
            None,
            OrderedTileSet::new(),
        ));
        st.player_data_mut(attacker).outgoing_attacks.push(a);
        if target.is_player() {
            st.player_data_mut(target).incoming_attacks.push(a);
        }
        a
    }

    /// Mandatory design test (FILE_MAP §4 P6): the `AttackExecution.ts:169-178` iter-snapshot
    /// site. A new attack on a target the owner already attacks three times absorbs all three
    /// (each `outgoing.delete()` filter-reassigns the owner's list mid-loop; a retain + live
    /// cursor would skip every second one), and an attack on another target is left alone.
    #[test]
    fn merging_three_outgoing_attacks_merges_all_three() {
        let mut st = crate::state::State::default();
        for i in 1..=3 {
            st.players.push(blank_player(SmallId(i)));
        }
        let (me, tgt, other) = (SmallId(1), SmallId(2), SmallId(3));
        let o1 = add_attack(&mut st, 1, me, tgt, 10.0);
        let x = add_attack(&mut st, 2, me, other, 100.0);
        let o2 = add_attack(&mut st, 3, me, tgt, 20.0);
        let o3 = add_attack(&mut st, 4, me, tgt, 30.0);
        let a = add_attack(&mut st, 5, me, tgt, 5.0);
        let mut sc = crate::scratch::Scratch::new();
        let mut cx = Ctx::new(&mut st, &mut sc, None);
        AttackExec::merge_outgoing_attacks(&mut cx, me, a);
        assert_eq!(cx.st.attack(a).troops(), 65.0);
        for o in [o1, o2, o3] {
            assert!(!cx.st.attack(o).is_active());
        }
        assert!(cx.st.attack(x).is_active());
        assert_eq!(cx.st.player_data(me).outgoing_attacks.as_slice(), &[x, a]);
        assert_eq!(cx.st.player_data(tgt).incoming_attacks.as_slice(), &[a]);
        // Three reassignments of the owner's list happened (one per merged attack).
        assert_eq!(cx.st.player_data(me).outgoing_attacks.generation(), 3);
    }

    /// A boat attack (`sourceTile !== null`) never absorbs others (`:173-174`).
    #[test]
    fn boat_attacks_are_not_merged() {
        let mut st = crate::state::State::default();
        st.players.push(blank_player(SmallId(1)));
        let me = SmallId(1);
        let o1 = add_attack(&mut st, 1, me, SmallId::TERRA_NULLIUS, 10.0);
        let a = add_attack(&mut st, 2, me, SmallId::TERRA_NULLIUS, 5.0);
        st.attack_mut(a).source_tile = Some(9);
        let mut sc = crate::scratch::Scratch::new();
        let mut cx = Ctx::new(&mut st, &mut sc, None);
        AttackExec::merge_outgoing_attacks(&mut cx, me, a);
        assert_eq!(cx.st.attack(a).troops(), 5.0);
        assert!(cx.st.attack(o1).is_active());
    }
}
