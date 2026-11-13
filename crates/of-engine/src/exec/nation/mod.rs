//! `NationExecution` (`engine/src/execution/NationExecution.ts`) and its 7 behaviours.
//! Owner of this file: **P10**. **`NationExec`'s fields are frozen at S0.**
//!
//! TS: one PRNG shared by 7 behaviour objects that also reference each other
//! (`NationExecution.ts:270-321`). Rust (ARCHITECTURE §4.4): the behaviours' *data* are fields
//! of [`NationBehaviors`]; every behaviour *method* is an `impl NationExec` method in the
//! behaviour's own file (P10: attack/alliance/emoji/utils/annexation; P11: nuke/mirv/
//! nuke_planning; P12: structure/structure_placement/warship/tutorial), so a cross-behaviour
//! call `this.allianceBehavior.foo()` is `self.alliance_foo(cx)` and `this.random` is
//! `self.rng`: one `&mut self` covers the shared stream.
//!
//! Fields: the `Nation v1 (execution)` snapshot record order. `nationIndex`/`nation` in the
//! snapshot are one TS field (`nation`), exported as an index into `game.nations()` when it is
//! one of them (P13 computes `nationIndex`).

pub mod alliance;
pub mod annexation;
pub mod attack;
pub mod emoji;
pub mod mirv;
pub mod nuke;
pub mod nuke_planning;
pub mod structure;
pub mod structure_placement;
pub mod tutorial;
pub mod utils;
pub mod warship;

use crate::api::types::{Difficulty, GameMode, Relation, TerrainType};
use crate::canon::{Canon, CanonCx, Cs};
use crate::exec::ai_attack;
#[allow(unused_imports)]
use crate::exec::prelude::*;
use crate::exec::spawn::SpawnExec;
use of_det::util::simple_hash;

/// The 7 behaviour objects (`initializeBehaviors`, `NationExecution.ts:271-321`). Snapshot
/// `behaviors { emoji, mirv, alliance, warship, attack, nuke, structure }?null` (`null` until
/// `behaviorsInitialized`).
#[derive(StateClone, Debug)]
pub struct NationBehaviors {
    pub emoji: emoji::EmojiB,
    pub mirv: mirv::MirvB,
    pub alliance: alliance::AllianceB,
    pub warship: warship::WarshipB,
    pub attack: attack::NationAttackB,
    pub nuke: nuke::NukeB,
    pub structure: structure::StructureB,
}

/// `NationExecution` data.
#[derive(StateClone, Debug)]
pub struct NationExec {
    pub active: bool,
    /// `gameID`.
    pub game_id: String,
    /// `nation` (`Nation` with `PlayerType.Nation` info).
    pub nation: Nation,
    /// `random = new PseudoRandom(simpleHash(playerID) + simpleHash(gameID))`; **3 ratio draws in
    /// the constructor** (`:73-78`, ARCHITECTURE §4.5).
    pub rng: Prng,
    /// `mg` set by `init`.
    pub initialized: bool,
    pub player: Option<SmallId>,
    /// `attackRate` (assigned in `init`).
    pub attack_rate: Option<f64>,
    pub attack_tick: Option<f64>,
    pub trigger_ratio: f64,
    pub reserve_ratio: f64,
    pub expand_ratio: f64,
    /// `embargoMalusApplied = new Set<PlayerID>()` (`:67`).
    pub embargo_malus_applied: JsSet<String>,
    pub spawn_exec_added: bool,
    /// `None` until `behaviorsInitialized` (`:49`).
    pub behaviors: Option<NationBehaviors>,
}

impl NationExec {
    /// `new NationExecution(gameID, nation)` (`:69-79`): seeds the PRNG and draws the three
    /// ratios **here**, not in `init`.
    pub fn new(game_id: &str, nation: Nation) -> Self {
        // TS :73-75 `new PseudoRandom(simpleHash(nation.playerInfo.id) + simpleHash(gameID))`; the
        // sum can exceed 2^31, `Prng::new` applies `seed | 0` (PseudoRandom.ts:14).
        let mut rng = Prng::new(simple_hash(&nation.player_info.id) + simple_hash(game_id));
        let trigger_ratio = rng.next_int(50.0, 60.0) / 100.0;
        let reserve_ratio = rng.next_int(30.0, 40.0) / 100.0;
        let expand_ratio = rng.next_int(10.0, 20.0) / 100.0;
        NationExec {
            active: true,
            game_id: game_id.to_string(),
            nation,
            rng,
            initialized: false,
            player: None,
            attack_rate: None,
            attack_tick: None,
            trigger_ratio,
            reserve_ratio,
            expand_ratio,
            embargo_malus_applied: JsSet::new(),
            spawn_exec_added: false,
            behaviors: None,
        }
    }
    /// `playerID()` (`:81-83`).
    pub fn player_id(&self) -> &str {
        &self.nation.player_info.id
    }
    /// `isInitialized()` (`:85-87`).
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }
    /// `refreshDifficulty()` (`:89-93`).
    pub fn refresh_difficulty(&mut self, cx: &mut Ctx) {
        if !self.initialized {
            return;
        }
        let rate = self.get_attack_rate(cx);
        self.attack_rate = Some(rate);
        self.attack_tick = Some(self.rng.next_int(0.0, rate));
    }
    /// `currentAttackRate()` (`:95-97`); `undefined` before `init` reads as NaN.
    pub fn current_attack_rate(&self) -> f64 {
        self.attack_rate.unwrap_or(f64::NAN)
    }
    /// `currentAttackTick()` (`:99-101`); `undefined` before `init` reads as NaN.
    pub fn current_attack_tick(&self) -> f64 {
        self.attack_tick.unwrap_or(f64::NAN)
    }
    /// The behaviours (`TypeError` in TS if used before `initializeBehaviors`).
    #[inline]
    pub fn b(&mut self) -> &mut NationBehaviors {
        self.behaviors.as_mut().ts_expect("NationExecution behaviors used before initializeBehaviors")
    }
    /// `this.player` as every behaviour sees it (non-null after `init`; P10–P12 share this).
    #[inline]
    pub fn nation_player(&self) -> SmallId {
        self.player.ts_expect("NationExecution.player used before init")
    }

    /// `getAttackRate()` (`:115-129`).
    fn get_attack_rate(&mut self, cx: &Ctx) -> f64 {
        match cx.st.config().game_config().difficulty {
            Difficulty::Easy => self.rng.next_int(65.0, 100.0), // Slower reactions
            Difficulty::Medium => self.rng.next_int(55.0, 70.0),
            Difficulty::Hard => self.rng.next_int(45.0, 60.0),
            Difficulty::Impossible => self.rng.next_int(30.0, 50.0), // Faster reactions
        }
    }

    /// `initializeBehaviors()` (`:271-321`): the 7 behaviours, constructed in TS order. Only
    /// `NationNukeBehavior` evaluates anything at construction (its two perceived-cost field
    /// initialisers, `NationNukeBehavior.ts:103,106`); none draws from the PRNG.
    fn initialize_behaviors(&mut self, cx: &mut Ctx) {
        let Some(player) = self.player else { of_det::ts_throw!("Player not initialized") };

        let emoji = emoji::EmojiB::default();
        let mirv = mirv::MirvB::default();
        let alliance = alliance::AllianceB::default();
        let warship = warship::WarshipB::default();
        let attack = attack::NationAttackB::new(self.trigger_ratio, self.reserve_ratio, self.expand_ratio);
        // TS field initialisers of `NationNukeBehavior` (`:103,106`) via P11's constructor
        // (requests P10 R1 / P11 R1, merge1).
        let nuke = nuke::NukeB::new(cx.st, player);
        let structure = structure::StructureB::default();
        self.behaviors = Some(NationBehaviors { emoji, mirv, alliance, warship, attack, nuke, structure });
    }

    /// `randomSpawnLand()` (`:323-352`).
    fn random_spawn_land(&mut self, cx: &mut Ctx) -> Option<TileRef> {
        let Some(cell) = self.nation.spawn_cell else { of_det::ts_throw!("not initialized") };

        let delta = 25.0;
        let mut tries = 0;
        while tries < 50 {
            tries += 1;
            let x = self.rng.next_int(cell.x - delta, cell.x + delta);
            let y = self.rng.next_int(cell.y - delta, cell.y + delta);
            if !cx.st.map.is_valid_coord(x, y) {
                continue;
            }
            let tile = cx.st.map.ref_(x, y);
            if cx.st.map.is_land(tile) && !cx.st.map.has_owner(tile) && !cx.st.map.is_impassable(tile) {
                if cx.st.map.terrain_type(tile) == TerrainType::Mountain && self.rng.chance(2.0) {
                    continue;
                }
                return Some(tile);
            }
        }
        None
    }

    /// `updateRelationsFromEmbargos()` (`:354-375`).
    fn update_relations_from_embargos(&mut self, cx: &mut Ctx) {
        let Some(player) = self.player else { return };
        // `players()` is a fresh array, then `filter` (`:357`); `forEach` over that copy.
        let others: Vec<SmallId> = cx.st.players().into_iter().filter(|&p| p != player).collect();

        for other in others {
            let embargo_malus = -20.0;
            let other_id = cx.st.player(other).player_id().to_string();
            if cx.st.player(other).has_manual_embargo_against(player) && !self.embargo_malus_applied.contains(&other_id)
            {
                cx.player_mut(player).update_relation(other, embargo_malus);
                self.embargo_malus_applied.insert(other_id);
            } else if !cx.st.player(other).has_manual_embargo_against(player)
                && self.embargo_malus_applied.contains(&other_id)
            {
                cx.player_mut(player).update_relation(other, -embargo_malus);
                self.embargo_malus_applied.remove(&other_id);
            }
        }
    }

    /// `handleEmbargoesToHostileNations()` (`:377-423`).
    // TS :402-421 has two `else if` arms with the same body; kept literal (different conditions).
    #[allow(clippy::if_same_then_else)]
    fn handle_embargoes_to_hostile_nations(&mut self, cx: &mut Ctx) {
        let Some(player) = self.player else { return };
        let others: Vec<SmallId> = cx.st.players().into_iter().filter(|&p| p != player).collect();
        let difficulty = cx.st.config().game_config().difficulty;
        let is_higher_difficulty = difficulty == Difficulty::Hard || difficulty == Difficulty::Impossible;
        let team_game = cx.st.config().game_config().game_mode == GameMode::Team;

        for other in others {
            // In team games on higher difficulties, refuse to trade with anyone
            // not on this nation's team.
            if team_game
                && is_higher_difficulty
                && cx.st.player(other).player_type() != PlayerType::Bot
                && !cx.st.player(player).is_on_same_team(other)
            {
                if !cx.st.player(player).has_embargo_against(other) {
                    cx.player_mut(player).add_embargo(other, false);
                }
                continue;
            }

            /* When player is hostile starts embargo. Do not stop until neutral again */
            if cx.st.player(player).relation(other) <= Relation::Hostile
                && !cx.st.player(player).has_embargo_against(other)
                && !cx.st.player(player).is_on_same_team(other)
            {
                cx.player_mut(player).add_embargo(other, false);
            } else if cx.st.player(player).relation(other) >= Relation::Neutral
                && cx.st.player(player).has_embargo_against(other)
                && difficulty != Difficulty::Hard
                && difficulty != Difficulty::Impossible
            {
                cx.player_mut(player).stop_embargo(other);
            } else if cx.st.player(player).relation(other) >= Relation::Friendly
                && cx.st.player(player).has_embargo_against(other)
                && difficulty != Difficulty::Impossible
            {
                cx.player_mut(player).stop_embargo(other);
            }
        }
    }

    /// `new SpawnExecution(this.gameID, this.nation.playerInfo, tile?)` queued by `tick`.
    fn add_spawn(&mut self, cx: &mut Ctx, tile: Option<TileRef>) {
        let e = SpawnExec::new(&self.game_id, self.nation.player_info.clone(), tile.map(f64::from), false);
        cx.st.add_execution(Exec::Spawn(e));
        self.spawn_exec_added = true;
    }
}

impl ExecBehavior for NationExec {
    fn is_active(&self) -> bool {
        self.active
    }
    /// `activeDuringSpawnPhase()` returns `true` (`NationExecution.ts:429-431`).
    fn active_during_spawn_phase(&self) -> bool {
        true
    }
    /// `init(mg)` (`:103-113`).
    fn init(&mut self, cx: &mut Ctx, _ticks: i32) {
        self.initialized = true;
        let rate = self.get_attack_rate(cx);
        self.attack_rate = Some(rate);
        self.attack_tick = Some(self.rng.next_int(0.0, rate));

        let id = &self.nation.player_info.id;
        if !cx.st.has_player(id) {
            self.player = Some(cx.add_player(self.nation.player_info.clone(), None));
        } else {
            self.player = Some(cx.st.player_by_id(id));
        }
    }
    /// `tick(ticks)` (`:131-269`).
    fn tick(&mut self, cx: &mut Ctx, ticks: i32) {
        // `NationAttackBehavior.landNeighbors` / `AiAttackBehavior.boatRoutes` are only set during
        // `maybeAttack` and reset by its `finally` (`NationAttackBehavior.ts:112-120`), so they
        // are null at every tick start, even after a throw (attack.rs `attack_maybe_attack`).
        if let Some(b) = self.behaviors.as_mut() {
            b.attack.land_neighbors = None;
            b.attack.base.boat_routes = None;
        }
        // `undefined` attackRate/attackTick (impossible after init) would read as NaN.
        let attack_rate = self.attack_rate.unwrap_or(f64::NAN);
        let attack_tick = self.attack_tick.unwrap_or(f64::NAN);
        let t = f64::from(ticks);
        let gc_difficulty = cx.st.config().game_config().difficulty;
        // TS `!gameConfig().tutorial` (an optional boolean).
        let tutorial = cx.st.config().game_config().tutorial == Some(true);

        // Ship tracking
        let track_ships = match self.player {
            Some(p) if self.behaviors.is_some() => {
                cx.st.player(p).is_alive()
                    && gc_difficulty != Difficulty::Easy
                    && !tutorial
                    && cx.st.player(p).units_constructed(UnitType::Port) != 0
                    && !cx.st.config().is_unit_disabled(UnitType::Warship)
            }
            _ => false,
        };
        if track_ships {
            self.warship_track_ships_and_retaliate(cx);
        }

        let Some(player) = self.player else { return };

        if cx.st.in_spawn_phase() {
            if cx.st.player(player).has_spawned() {
                // Already on the map — periodically re-spawn so the nation
                // visibly hops to different locations during the spawn phase.
                if t % attack_rate != attack_tick {
                    return;
                }
            } else if self.spawn_exec_added {
                // First SpawnExecution already queued, wait for it to land.
                return;
            }
            // Place nations without a spawn cell (Dynamically created for HumansVsNations) randomly by SpawnExecution
            let Some(cell) = self.nation.spawn_cell else {
                self.add_spawn(cx, None);
                return;
            };

            // If team spawn areas are configured and the nation's spawn cell
            // is outside its team's area, spawn randomly within the area instead.
            let team = cx.st.player(player).team().cloned();
            if let Some(team) = team {
                let area = cx.st.team_spawn_area(&team);
                if let Some(area) = area {
                    let in_area = cell.x >= area.x
                        && cell.x < area.x + area.width
                        && cell.y >= area.y
                        && cell.y < area.y + area.height;
                    if !in_area {
                        self.add_spawn(cx, None);
                        return;
                    }
                }
            }

            // Select a tile near the position defined in the map manifest
            let rl = self.random_spawn_land(cx);

            let Some(rl) = rl else {
                // console.warn(`cannot spawn ..`) dropped (PORTING_GUIDE §12).
                return;
            };

            self.add_spawn(cx, Some(rl));
            return;
        }

        // Spawn phase already ended but our SpawnExecution hasn't fired yet — wait.
        if self.spawn_exec_added && !cx.st.player(player).has_spawned() {
            return;
        }

        if !cx.st.player(player).is_alive() {
            //removeOnDeath is called from nation's PlayerExecution
            self.active = false;
            return;
        }

        if self.behaviors.is_none() {
            self.initialize_behaviors(cx);
            self.attack_force_send_attack(cx, SmallId::TERRA_NULLIUS);
            return;
        }

        // The tutorial's nations only grow into free land, and the one on the
        // player's island nukes them from its silo (TutorialSilo): no attacks,
        // boats, alliances or other structures. Each falls once the player has
        // taken most of it (TutorialCollapse).
        if tutorial {
            tutorial::tutorial_collapse(cx, player);
            if !cx.st.player(player).is_alive() {
                return;
            }
            let attack_tick_now = t % attack_rate == attack_tick;
            if attack_tick_now {
                ai_attack::expand_into_free_land(self, cx);
            }
            tutorial::tutorial_silo(cx, player, attack_tick_now);
            tutorial::tutorial_warship(cx, player, ticks);
            return;
        }

        self.attack_follow_up_landings(cx);
        self.nuke_follow_up_strikes(cx);

        if t % attack_rate != attack_tick {
            // Call handleStructures twice between regular attack ticks (at 1/3 and 2/3 of the interval)
            // Otherwise it is possible that we earn more gold than we can spend
            // The alternative is placing multiple structures in handleStructures, but that causes problems
            if cx.st.player(player).is_alive() {
                let offset = t % attack_rate;
                let one_third = (attack_tick + of_det::js::floor(attack_rate / 3.0)) % attack_rate;
                let two_thirds = (attack_tick + of_det::js::floor((attack_rate * 2.0) / 3.0)) % attack_rate;
                if offset == one_third || offset == two_thirds {
                    self.structure_handle_structures(cx);
                }
            }
            return;
        }

        self.emoji_maybe_send_casual_emoji(cx);
        self.update_relations_from_embargos(cx);
        self.alliance_handle_alliance_requests(cx);
        self.alliance_handle_alliance_extension_requests(cx);
        self.mirv_consider_mirv(cx);
        self.structure_handle_structures(cx);
        self.warship_maybe_spawn_warship(cx);
        self.handle_embargoes_to_hostile_nations(cx);
        self.attack_maybe_attack(cx);
        self.warship_counter_warship_infestation(cx);
        self.nuke_maybe_send_nuke(cx);
    }
}

// ---- CS v1 export (SPEC §1, N4: behaviour records unwrapped to `d`) ----

/// `playerInfoData(info)` (`snapshot/CommonSchemas.ts:24-36`).
fn canon_player_info(i: &PlayerInfo) -> Cs {
    let opt_str = |s: &Option<String>| match s {
        Some(s) => Cs::Str(s.clone()),
        None => Cs::Null,
    };
    Cs::obj([
        ("name", Cs::Str(i.name.clone())),
        ("playerType", Cs::Str(i.player_type.as_str().to_string())),
        ("clientID", opt_str(&i.client_id)),
        ("id", Cs::Str(i.id.clone())),
        ("isLobbyCreator", Cs::Bool(i.is_lobby_creator)),
        ("clanTag", opt_str(&i.clan_tag)),
        ("friends", Cs::Arr(i.friends.iter().map(|f| Cs::Str(f.clone())).collect())),
        ("teamIndex", i.team_index.map_or(Cs::Null, |t| Cs::Int(i64::from(t)))),
        ("nationFlag", opt_str(&i.nation_flag)),
    ])
}

/// `nationData(n)` (`snapshot/CommonSchemas.ts:76-81`).
fn canon_nation(n: &Nation) -> Cs {
    Cs::obj([
        ("spawnCell", n.spawn_cell.map_or(Cs::Null, |c| Cs::obj([("x", Cs::Num(c.x)), ("y", Cs::Num(c.y))]))),
        ("playerInfo", canon_player_info(&n.player_info)),
    ])
}

/// CS v1 `Nation v1 (execution)` record (request P10 R2, merge1): `behaviors` keys in the
/// order of `NationExecution.ts:452-460` (emoji, mirv, alliance, warship, attack, nuke,
/// structure). Each behaviour is a `w.versioned(..)` record `{v, d}`, and the TS CS writer
/// keeps that wrapper (`conformance/src/canon.ts` unwraps only R/W/S, `:89-155`), so the
/// golden digests have it too. SPEC N4 says these are unwrapped; the corpus is the oracle
/// (found at merge1: M-AI-01 K mismatch at tick 202, the first tick with behaviours).
/// Versions: `version:` of each `snapshotType` (Emoji/MIRV/Alliance/Warship/Structure 1,
/// AiAttackBehavior 2, NationNukeBehavior 2).
impl Canon for NationExec {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        fn ver(v: i64, d: Cs) -> Cs {
            Cs::obj([("v", Cs::Int(v)), ("d", d)])
        }
        self.canon_with_behaviors(cx, |b, cx| {
            Cs::obj([
                ("emoji", ver(1, b.emoji.canon(cx))),
                ("mirv", ver(1, b.mirv.canon(cx))),
                ("alliance", ver(1, b.alliance.canon(cx))),
                ("warship", ver(1, b.warship.canon(cx))),
                ("attack", ver(2, b.attack.canon(cx))),
                ("nuke", ver(2, b.nuke.canon(cx))),
                ("structure", ver(1, b.structure.canon(cx))),
            ])
        })
    }
}

impl NationExec {
    /// The `Nation v1 (execution)` record (`NationExecution.ts:433-464`), in dump field order.
    /// `behaviors` is built by the caller from the 7 behaviour records (`EmojiB`, `AllianceB`,
    /// `NationAttackB` have `Canon` here; `MirvB`/`WarshipB`/`NukeB`/`StructureB` belong to
    /// P11/P12), so `impl Canon for NationExec` is a one-liner once those exist.
    pub fn canon_with_behaviors(
        &self,
        cx: &mut CanonCx<'_>,
        behaviors: impl FnOnce(&NationBehaviors, &mut CanonCx<'_>) -> Cs,
    ) -> Cs {
        // TS :436 `w.game.nations().indexOf(this.nation)` (identity; nations are distinct values).
        let nation_index = cx.st.nations().iter().position(|n| *n == self.nation);
        let random = Cs::Arr(self.rng.get_state().iter().map(|&w| Cs::Int(i64::from(w))).collect());
        let behaviors = match &self.behaviors {
            Some(b) => behaviors(b, cx),
            None => Cs::Null,
        };
        Cs::obj([
            ("active", Cs::Bool(self.active)),
            ("gameID", Cs::Str(self.game_id.clone())),
            ("nationIndex", nation_index.map_or(Cs::Null, |i| Cs::Int(i as i64))),
            ("nation", if nation_index.is_none() { canon_nation(&self.nation) } else { Cs::Null }),
            ("random", random),
            ("initialized", Cs::Bool(self.initialized)),
            ("player", self.player.canon(cx)),
            ("attackRate", self.attack_rate.canon(cx)),
            ("attackTick", self.attack_tick.canon(cx)),
            ("triggerRatio", Cs::Num(self.trigger_ratio)),
            ("reserveRatio", Cs::Num(self.reserve_ratio)),
            ("expandRatio", Cs::Num(self.expand_ratio)),
            ("embargoMalusApplied", Cs::Arr(self.embargo_malus_applied.iter().map(|s| Cs::Str(s.clone())).collect())),
            ("spawnExecAdded", Cs::Bool(self.spawn_exec_added)),
            ("behaviors", behaviors),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::{Cell, PlayerInfo};

    fn nation(id: &str) -> Nation {
        Nation {
            spawn_cell: Some(Cell::new(10.0, 20.0)),
            player_info: PlayerInfo::new("Nation X", PlayerType::Nation, None, id),
        }
    }

    /// The constructor seeds `PseudoRandom(simpleHash(id) + simpleHash(gameID))` and draws the
    /// three ratios (`NationExecution.ts:73-78`); nothing else is drawn before `init`.
    #[test]
    fn constructor_draws_three_ratios() {
        let n = NationExec::new("game123", nation("abcdEFGH"));
        let mut r = Prng::new(simple_hash("abcdEFGH") + simple_hash("game123"));
        assert_eq!(n.trigger_ratio, r.next_int(50.0, 60.0) / 100.0);
        assert_eq!(n.reserve_ratio, r.next_int(30.0, 40.0) / 100.0);
        assert_eq!(n.expand_ratio, r.next_int(10.0, 20.0) / 100.0);
        assert_eq!(n.rng.get_state(), r.get_state());
        assert!(n.active && !n.initialized && n.player.is_none() && n.behaviors.is_none());
        assert!(n.attack_rate.is_none() && n.attack_tick.is_none() && !n.spawn_exec_added);
        assert!((0.5..0.6).contains(&n.trigger_ratio));
        assert!((0.3..0.4).contains(&n.reserve_ratio));
        assert!((0.1..0.2).contains(&n.expand_ratio));
        assert_eq!(n.player_id(), "abcdEFGH");
        assert!(n.current_attack_rate().is_nan());
    }

    /// Seeds whose sum passes 2^31 wrap through `seed | 0` (PseudoRandom.ts:14).
    #[test]
    fn seed_sum_uses_to_int32() {
        let (a, b) = ("zzzzzzzzzzzzzzzz", "yyyyyyyyyyyyyyyy");
        let sum = simple_hash(a) + simple_hash(b);
        let n = NationExec::new(b, nation(a));
        let mut r = Prng::from_seed_i32(of_det::js::to_int32(sum));
        r.next_int(50.0, 60.0);
        r.next_int(30.0, 40.0);
        r.next_int(10.0, 20.0);
        assert_eq!(n.rng.get_state(), r.get_state());
    }

    /// CS record field order = the `Nation v1 (execution)` dump; a nation that is one of
    /// `game.nations()` is stored by index, any other inline (`NationExecution.ts:436-441`).
    #[test]
    fn canon_record_order_and_nation_index() {
        use crate::exec::ExecStore;
        use crate::state::State;
        let mut st = State::default();
        let n0 = nation("aaaaaaaa");
        st.nations = vec![nation("zzzzzzzz"), n0.clone()].into();
        let ex = ExecStore::default();
        let mut cx = CanonCx::new(&st, &ex);
        let n = NationExec::new("g1", n0);
        let cs = n.canon_with_behaviors(&mut cx, |_, _| unreachable!("behaviors are null before init"));
        let Cs::Obj(fields) = &cs else { panic!("record is an object") };
        let keys: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            keys,
            [
                "active",
                "gameID",
                "nationIndex",
                "nation",
                "random",
                "initialized",
                "player",
                "attackRate",
                "attackTick",
                "triggerRatio",
                "reserveRatio",
                "expandRatio",
                "embargoMalusApplied",
                "spawnExecAdded",
                "behaviors"
            ]
        );
        assert_eq!(fields[2].1, Cs::Int(1));
        assert_eq!(fields[3].1, Cs::Null);
        assert_eq!(fields[6].1, Cs::Null);
        assert_eq!(fields[7].1, Cs::Null);
        assert_eq!(fields[14].1, Cs::Null);
        let st_words: Vec<Cs> = n.rng.get_state().iter().map(|&w| Cs::Int(i64::from(w))).collect();
        assert_eq!(fields[4].1, Cs::Arr(st_words));
        // a nation that is not in game.nations() is stored inline
        let other = NationExec::new("g1", nation("qqqqqqqq"));
        let cs = other.canon_with_behaviors(&mut cx, |_, _| Cs::Null);
        let Cs::Obj(fields) = &cs else { panic!("record is an object") };
        assert_eq!(fields[2].1, Cs::Null);
        let Cs::Obj(nf) = &fields[3].1 else { panic!("inline nation") };
        assert_eq!(nf[0], ("spawnCell".to_string(), Cs::obj([("x", Cs::Num(10.0)), ("y", Cs::Num(20.0))])));
    }

    /// Behaviour records owned by P10 (`NationEmojiBehavior v1`, `NationAllianceBehavior v1`).
    #[test]
    fn canon_behaviour_records() {
        use crate::exec::ExecStore;
        use crate::state::State;
        let st = State::default();
        let ex = ExecStore::default();
        let mut cx = CanonCx::new(&st, &ex);
        let mut e = emoji::EmojiB::default();
        e.last_emoji_sent.insert(SmallId(7), 900);
        e.last_emoji_sent.insert(SmallId(3), 1200);
        e.last_emoji_sent.insert(SmallId(7), 1500); // `set` on an existing key keeps its position
        e.game_over = true;
        assert_eq!(
            e.canon(&mut cx),
            Cs::obj([
                (
                    "lastEmojiSent",
                    Cs::Arr(vec![Cs::Arr(vec![Cs::Int(7), Cs::Int(1500)]), Cs::Arr(vec![Cs::Int(3), Cs::Int(1200)])])
                ),
                ("gameOver", Cs::Bool(true)),
            ])
        );
        assert_eq!(alliance::AllianceB::default().canon(&mut cx), Cs::Obj(vec![]));
    }

    /// `ticks % attackRate` on the spawn/attack schedule is JS `%` on numbers (f64 fmod).
    #[test]
    fn schedule_thirds() {
        let (rate, tick) = (45.0_f64, 44.0_f64);
        let one_third = (tick + of_det::js::floor(rate / 3.0)) % rate;
        let two_thirds = (tick + of_det::js::floor((rate * 2.0) / 3.0)) % rate;
        assert_eq!((one_third, two_thirds), (14.0, 29.0));
    }
}
