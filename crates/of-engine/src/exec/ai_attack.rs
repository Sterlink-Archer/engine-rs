//! `AiAttackBehavior` (`engine/src/execution/utils/AiAttackBehavior.ts`). Owner: **P9**.
//! **The hook trait and every signature here are FROZEN AT S0** (ARCHITECTURE §4.4).
//!
//! TS: `abstract class AiAttackBehavior` calls hookable methods through `this.` (`:94, 100,
//! 133, 200, 219, 308-309, 317, 379`). `TribeAttackBehavior` overrides nothing;
//! `NationAttackBehavior` overrides seven (`NationAttackBehavior.ts:1032, 1056, 1140, 1149,
//! 1201, 1214, 1272`) and calls `super.` at `:1202, :1218`.
//!
//! Rust:
//! - [`AiAttackHooks`]: the 7 overridable methods as defaulted trait methods (defaults = the
//!   `base_*` free functions = TS base behaviour), plus access to the shared pieces.
//! - Every other base method is a generic free function over `H: AiAttackHooks + ?Sized` and
//!   calls hooks through `h.` **exactly where TS writes `this.`**. Base code never calls a
//!   `base_*` function directly (or a nation override would be skipped). The guard test
//!   `hook_sites_go_through_h` (bottom of this file) checks both rules on this file's source.
//! - `super.sendBoatAttack(t)` → `base_send_boat_attack(self, cx, t)`, etc.
//! - Implemented by `TribeExec` (P9, no overrides) and `NationExec` (P10, all 7).
//!
//! TS-private methods (`hasLandBorderWithTerraNullius`, `sendBoatAttackToNearbyTerraNullius`,
//! `planBoatRoute`, `routeTo`, `otherLandings`) are private generic helpers here. The TS
//! scratch field `nbuf` (`:256`, "always written before it is read") is a local buffer.

use crate::StateClone;
use crate::api::types::unit_groups::STRUCTURES;
use crate::api::types::{Difficulty, PlayerType, Tick, UnitType};
use crate::canon::{Canon, CanonCx, CanonError, Cs, Uncanon, UncanonCx};
use crate::ctx::Ctx;
use crate::exec::Exec;
use crate::exec::attack::AttackExec;
use crate::exec::nation::utils::has_high_starting_gold;
use crate::exec::transport_ship::TransportShipExec;
use crate::exec::util::closest_two_tiles;
use crate::ids::{Owner, SmallId, SmallIdKey, TileRef, UnitRef};
use crate::jsorder::JsMap;
use crate::state::State;
use crate::state::transport_ship_utils::{can_build_transport_ship_cx, target_transport_tile};
use crate::state::unit::WarshipMode;
use of_det::Prng;
use of_det::js;

/// `BoatRoute` (`:34-38`).
#[derive(StateClone, Debug, Default, PartialEq)]
pub struct BoatRoute {
    pub landing: TileRef,
    pub length: f64,
    pub blocker: Option<UnitRef>,
}

/// `sail(..)` result: `{ length, blocker }`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SailResult {
    pub length: f64,
    pub blocker: Option<UnitRef>,
}

/// The `sendTroops: (reserveTroops: number) => number` closure parameter (`:185-199`;
/// `NationAttackBehavior.ts:1149-1152`), as data (PORTING_GUIDE §7). Every TS call site:
/// - default `(reserveTroops) => this.homeTroops() - reserveTroops` (v36 `:224`), and
///   `(targetTroops) => this.homeTroops() - targetTroops` (v36 `NationAttackBehavior.ts:1283`)
///   → [`SendTroops::TroopsMinusReserve`];
/// - `() => this.homeTroops() / 5` (v36 `:249`) → [`SendTroops::TroopsDiv5`];
/// - `() => send` (`NationAttackBehavior.ts:659`) → [`SendTroops::Fixed`].
///
/// rules-v36 (t713, upstream #6297 b850d44de): the closures read `homeTroops()` (troops once this tick's sends have
/// left), no longer `this.player.troops()`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SendTroops {
    TroopsMinusReserve,
    TroopsDiv5,
    Fixed(f64),
}

impl SendTroops {
    /// Evaluate the closure for `h` with argument `reserve` (v36: [`home_troops`] is read only by the closures that
    /// call `homeTroops()`; it is a pure read, so evaluating it lazily is state-identical).
    pub fn eval<H: AiAttackHooks + ?Sized>(&self, h: &mut H, cx: &mut Ctx, reserve: f64) -> f64 {
        match *self {
            SendTroops::Fixed(send) => send,
            _ => self.eval_home(home_troops(h, cx), reserve),
        }
    }

    /// The closure body given `homeTroops()` = `home`.
    pub fn eval_home(&self, home: f64, reserve: f64) -> f64 {
        match *self {
            // TS: v36 AiAttackBehavior.ts:224 `this.homeTroops() - reserveTroops`
            SendTroops::TroopsMinusReserve => home - reserve,
            // TS: v36 AiAttackBehavior.ts:249 `() => this.homeTroops() / 5` (argument ignored)
            SendTroops::TroopsDiv5 => home / 5.0,
            // TS: NationAttackBehavior.ts:659 `() => send`
            SendTroops::Fixed(send) => send,
        }
    }
}

/// `AiAttackBehavior` fields (`:43-52`): the ratios and the per-`maybeAttack` boat-route
/// memo. `random`, `game`, `player` are reached through [`AiAttackHooks`].
#[derive(StateClone, Debug, Default)]
pub struct AiAttackBase {
    pub trigger_ratio: f64,
    pub reserve_ratio: f64,
    pub expand_ratio: f64,
    /// `boatRoutes: Map<Player, BoatRoute | null> | null` (`:43`); only set during a nation's
    /// `maybeAttack`. non-snapshot.
    pub boat_routes: Option<JsMap<SmallIdKey, Option<BoatRoute>>>,
    /// non-snapshot: `sentThisTick: { tick, troops } | null` (rules-v36, t713, upstream #6297; v36 `:44-45`):
    /// troops sent this tick (they only leave home when the executions init at the end of the tick). Only read in
    /// the tick it was written in; `null` on construction and on restore (v36 `:87-88`; a DERIVED field upstream).
    pub sent_this_tick: Option<(Tick, f64)>,
}

impl AiAttackBase {
    /// `new AiAttackBehavior(random, game, player, triggerRatio, reserveRatio, expandRatio)`
    /// (`:45-52`).
    pub fn new(trigger_ratio: f64, reserve_ratio: f64, expand_ratio: f64) -> Self {
        // TS: :43 `boatRoutes = null` field initialiser
        AiAttackBase { trigger_ratio, reserve_ratio, expand_ratio, boat_routes: None, sent_this_tick: None }
    }

    /// `writeRecord(w, botAttackTroopsSent, followedLandings)` (`:58-70`): the
    /// `AiAttackBehavior v2` record as embedded in its owner (`{v: 2, d: {...}}`), fields in
    /// dump order. Shared by `TribeAttackBehavior` (`0, []`) and `NationAttackBehavior`.
    pub fn canon_record(&self, cx: &mut CanonCx<'_>, bot_attack_troops_sent: f64, followed_landings: Cs) -> Cs {
        Cs::Obj(vec![
            ("v".to_string(), Cs::Int(2)),
            (
                "d".to_string(),
                Cs::Obj(vec![
                    ("botAttackTroopsSent".to_string(), bot_attack_troops_sent.canon(cx)),
                    ("followedLandings".to_string(), followed_landings),
                    ("triggerRatio".to_string(), self.trigger_ratio.canon(cx)),
                    ("reserveRatio".to_string(), self.reserve_ratio.canon(cx)),
                    ("expandRatio".to_string(), self.expand_ratio.canon(cx)),
                ]),
            ),
        ])
    }

    /// `restoreRecord(raw, ..)` (`:73-91`): reads a `{v, d}` record (v1 migrates by adding
    /// `followedLandings: []`, `:479-482`). Returns the base (with `boatRoutes = null`, `:84`),
    /// `botAttackTroopsSent` and the raw `followedLandings` value (`[]` for v1).
    pub fn uncanon_record(v: &Cs, cx: &mut UncanonCx<'_>) -> Result<(AiAttackBase, f64, Cs), CanonError> {
        let ver = v.get("v").and_then(Cs::as_i64).ok_or_else(|| CanonError::new("AiAttackBehavior: missing v"))?;
        let d = v.get("d").ok_or_else(|| CanonError::new("AiAttackBehavior: missing d"))?;
        let followed = match ver {
            1 => Cs::Arr(vec![]),
            2 => d
                .get("followedLandings")
                .cloned()
                .ok_or_else(|| CanonError::new("AiAttackBehavior: followedLandings"))?,
            _ => return Err(CanonError::new(format!("AiAttackBehavior: unknown version {ver}"))),
        };
        let f = |k: &str, cx: &mut UncanonCx<'_>| -> Result<f64, CanonError> {
            f64::uncanon(d.get(k).ok_or_else(|| CanonError::new(format!("AiAttackBehavior: missing {k}")))?, cx)
        };
        let bot = f("botAttackTroopsSent", cx)?;
        let base = AiAttackBase::new(f("triggerRatio", cx)?, f("reserveRatio", cx)?, f("expandRatio", cx)?);
        Ok((base, bot, followed))
    }
}

/// The 7 virtual methods (+ v36's `troopsAtInit`, rules-v36 t713) + access to the shared pieces. FROZEN AT S0
/// (rules-v36 adds the 8th hook `troops_at_init`, upstream #6297).
pub trait AiAttackHooks {
    /// The base fields.
    fn ai(&mut self) -> &mut AiAttackBase;
    /// The **same** `Prng` as the owner's other behaviours.
    fn rng(&mut self) -> &mut Prng;
    /// `this.player`.
    fn player(&self) -> SmallId;

    /// `shouldAttack(other)` (`:113-120`).
    fn should_attack(&mut self, cx: &mut Ctx, other: Owner) -> bool {
        base_should_attack(self, cx, other)
    }
    /// `ignoresAttacksFrom(attacker)` (`:148-150`).
    fn ignores_attacks_from(&mut self, cx: &mut Ctx, attacker: SmallId) -> bool {
        base_ignores_attacks_from(self, cx, attacker)
    }
    /// `expansionTroopCap()` (`:327-329`).
    fn expansion_troop_cap(&mut self, cx: &mut Ctx) -> f64 {
        base_expansion_troop_cap(self, cx)
    }
    /// `calculateAttackTroops(target, sendTroops, opponent?)` (`:185-192`).
    fn calculate_attack_troops(
        &mut self,
        cx: &mut Ctx,
        target: Owner,
        send_troops: SendTroops,
        opponent: Option<SmallId>,
    ) -> Option<f64> {
        base_calculate_attack_troops(self, cx, target, send_troops, opponent)
    }
    /// `sendBoatAttack(target)` (`:215-254`).
    fn send_boat_attack(&mut self, cx: &mut Ctx, target: SmallId) -> bool {
        base_send_boat_attack(self, cx, target)
    }
    /// `sendExpansionBoat(tile)` (`:316-325`).
    fn send_expansion_boat(&mut self, cx: &mut Ctx, tile: TileRef) -> bool {
        base_send_expansion_boat(self, cx, tile)
    }
    /// `sail(launch, target, warships?)` (`:455-464`). `None` = the TS default argument
    /// (`this.hostileWarships()` in the nation override), evaluated inside the method.
    fn sail(&mut self, cx: &mut Ctx, launch: TileRef, target: TileRef, warships: Option<&[UnitRef]>) -> SailResult {
        base_sail(self, cx, launch, target, warships)
    }
    /// `troopsAtInit()` (rules-v36, upstream #6297; v36 `:164-167`): our troops when the executions added this tick
    /// init. The nation override caps them at max troops (its PlayerExecution ticks after it).
    fn troops_at_init(&mut self, cx: &mut Ctx) -> f64 {
        base_troops_at_init(self, cx)
    }
}

// ---- base implementations of the 7 hooks (`super.` targets) ----

/// TS: v36 `:165-167` `return this.player.troops()` (rules-v36, #6297).
pub fn base_troops_at_init<H: AiAttackHooks + ?Sized>(h: &mut H, cx: &mut Ctx) -> f64 {
    cx.st.player(h.player()).troops()
}

/// TS: `:113-115` `return true`.
pub fn base_should_attack<H: AiAttackHooks + ?Sized>(h: &mut H, cx: &mut Ctx, other: Owner) -> bool {
    let _ = (h, cx, other);
    true
}
/// TS: `:148-150` `return false`.
pub fn base_ignores_attacks_from<H: AiAttackHooks + ?Sized>(h: &mut H, cx: &mut Ctx, attacker: SmallId) -> bool {
    let _ = (h, cx, attacker);
    false
}
/// TS: `:327-329` `return Infinity`.
pub fn base_expansion_troop_cap<H: AiAttackHooks + ?Sized>(h: &mut H, cx: &mut Ctx) -> f64 {
    let _ = (h, cx);
    f64::INFINITY
}
/// TS: `:185-192`.
pub fn base_calculate_attack_troops<H: AiAttackHooks + ?Sized>(
    h: &mut H,
    cx: &mut Ctx,
    target: Owner,
    send_troops: SendTroops,
    opponent: Option<SmallId>,
) -> Option<f64> {
    let _ = opponent; // TS: unused by the base (`:188`)
    // TS: :190 `sendTroops(this.reserveTroops(target))`: the argument is evaluated even when
    // the closure ignores it (`TroopsDiv5`, `Fixed`).
    let reserve = reserve_troops(h, cx, target);
    let troops = send_troops.eval(h, cx, reserve);
    // TS: :191 `troops < 1 ? null : troops` (NaN < 1 is false: NaN is sent)
    if troops < 1.0 { None } else { Some(troops) }
}
/// TS: `:215-231`.
pub fn base_send_boat_attack<H: AiAttackHooks + ?Sized>(h: &mut H, cx: &mut Ctx, target: SmallId) -> bool {
    let route = boat_route(h, cx, target);
    // TS: :217 `if (route === null || route.blocker !== null) return false`
    let route = match route {
        Some(r) if r.blocker.is_none() => r,
        _ => return false,
    };
    // TS: v36 :247-250 `this.calculateAttackTroops(target, () => this.homeTroops() / 5)`
    let troops = match h.calculate_attack_troops(cx, target, SendTroops::TroopsDiv5, None) {
        Some(t) => t,
        None => return false,
    };
    let player = h.player();
    // TS: v36 :255-258 `this.send(new TransportShipExecution(..), troops)`
    send(h, cx, Exec::TransportShip(TransportShipExec::new(player, route.landing, troops)), troops);
    true
}
/// TS: `:316-324`.
pub fn base_send_expansion_boat<H: AiAttackHooks + ?Sized>(h: &mut H, cx: &mut Ctx, tile: TileRef) -> bool {
    // TS: v36 :346 `Math.min(this.homeTroops() / 5, this.expansionTroopCap())` (left to right)
    let a = home_troops(h, cx) / 5.0;
    let troops = js::min(a, h.expansion_troop_cap(cx));
    if troops < 1.0 {
        return false;
    }
    let player = h.player();
    // TS: v36 :349 `this.send(new TransportShipExecution(this.player, tile, troops), troops)`
    send(h, cx, Exec::TransportShip(TransportShipExec::new(player, tile, troops)), troops);
    true
}
/// TS: `:455-463`. The base ignores `warships`.
pub fn base_sail<H: AiAttackHooks + ?Sized>(
    h: &mut H,
    cx: &mut Ctx,
    launch: TileRef,
    target: TileRef,
    warships: Option<&[UnitRef]>,
) -> SailResult {
    let _ = warships;
    let player = h.player();
    // TS: :460-461 `targetTransportTile(this.game, this.player, target) ?? target`
    let landing = target_transport_tile(&mut cx.rcx(), player, target).unwrap_or(target);
    // TS: :462
    SailResult { length: f64::from(cx.st.map.manhattan_dist(launch, landing)), blocker: None }
}

// ---- the other base methods (generic; hooks called through `h.`) ----

/// `sendAttack(target, force = false)` (`:93-111`).
pub fn send_attack<H: AiAttackHooks + ?Sized>(h: &mut H, cx: &mut Ctx, target: Owner, force: bool) -> bool {
    // TS: :94 `if (!force && !this.shouldAttack(target)) return false`
    if !force && !h.should_attack(cx, target) {
        return false;
    }
    if target.is_player() {
        // TS: :97
        if cx.shares_border(h.player(), target) {
            send_land_attack(h, cx, target, SendTroops::TroopsMinusReserve, None)
        } else {
            // TS: :100 `this.sendBoatAttack(target)`
            h.send_boat_attack(cx, target)
        }
    } else if has_land_border_with_terra_nullius(h, cx) {
        // TS: :105-106
        send_land_attack(h, cx, target, SendTroops::TroopsMinusReserve, None)
    } else {
        // TS: :108
        send_boat_attack_to_nearby_terra_nullius(h, cx)
    }
}

/// `expandIntoFreeLand()` (`:122-125`).
pub fn expand_into_free_land<H: AiAttackHooks + ?Sized>(h: &mut H, cx: &mut Ctx) -> bool {
    if !has_land_border_with_terra_nullius(h, cx) {
        return false;
    }
    // TS: :124 `this.sendLandAttack(this.game.terraNullius())`
    send_land_attack(h, cx, SmallId::TERRA_NULLIUS, SendTroops::TroopsMinusReserve, None)
}

/// `findIncomingAttackPlayer()` (`:127-146`).
pub fn find_incoming_attack_player<H: AiAttackHooks + ?Sized>(h: &mut H, cx: &mut Ctx) -> Option<SmallId> {
    let player = h.player();
    // TS: :128-134 `incomingAttacks()` is a fresh copy; the whole `filter` runs (hook calls
    // in list order, `isFriendly` short-circuiting the hook) before the max loop.
    let all = cx.st.player(player).incoming_attacks();
    let mut incoming = Vec::with_capacity(all.len());
    for a in all {
        let attacker = cx.st.attack(a).attacker();
        // TS: :132-133 `!isFriendly(attacker) && !this.ignoresAttacksFrom(attacker)`
        if !cx.st.player(player).is_friendly(attacker, false) && !h.ignores_attacks_from(cx, attacker) {
            incoming.push(a);
        }
    }
    let mut largest_attack = 0.0f64;
    let mut largest_attacker: Option<SmallId> = None;
    for a in incoming {
        let troops = cx.st.attack(a).troops();
        // TS: :138 `if (attack.troops() <= largestAttack) continue` (NaN compares false)
        if troops <= largest_attack {
            continue;
        }
        largest_attack = troops;
        largest_attacker = Some(cx.st.attack(a).attacker());
    }
    largest_attacker
}

/// `homeTroops()` (rules-v36, upstream #6297; v36 `:156-162`): our troops once everything we sent this tick has
/// left.
pub fn home_troops<H: AiAttackHooks + ?Sized>(h: &mut H, cx: &mut Ctx) -> f64 {
    // TS: :158 `const troops = this.troopsAtInit()`
    let troops = h.troops_at_init(cx);
    // TS: :159-161 `if (sent === null || sent.tick !== this.game.ticks()) return troops; return Math.max(0, troops - sent.troops)`
    match h.ai().sent_this_tick {
        Some((tick, sent)) if tick == cx.st.ticks() => js::max(0.0, troops - sent),
        _ => troops,
    }
}

/// `send(exec, troops)` (rules-v36, upstream #6297; v36 `:169-178`): every troop send of the AI goes through here so
/// the next budget this tick sees what already left.
pub fn send<H: AiAttackHooks + ?Sized>(h: &mut H, cx: &mut Ctx, exec: Exec, troops: f64) {
    let tick = cx.st.ticks();
    // TS: :171-172 `this.sentThisTick?.tick === tick ? this.sentThisTick.troops : 0`
    let sent = match h.ai().sent_this_tick {
        Some((t, s)) if t == tick => s,
        _ => 0.0,
    };
    // TS: :173-176 `{ tick, troops: sent + Math.min(troops, this.homeTroops()) }` (homeTroops reads the old value)
    let home = home_troops(h, cx);
    h.ai().sent_this_tick = Some((tick, sent + js::min(troops, home)));
    // TS: :177
    cx.st.add_execution(exec);
}

/// `hasTriggerRatioTroops(expansion = false)` (`:152-157`; v36 `:180-183` reads `homeTroops()`).
pub fn has_trigger_ratio_troops<H: AiAttackHooks + ?Sized>(h: &mut H, cx: &mut Ctx, expansion: bool) -> bool {
    let troops = home_troops(h, cx);
    let ratio = troops / reserve_basis(h, cx, expansion);
    ratio >= h.ai().trigger_ratio
}

/// `reserveBasis(expansion)` (`:159-164`).
pub fn reserve_basis<H: AiAttackHooks + ?Sized>(h: &mut H, cx: &mut Ctx, expansion: bool) -> f64 {
    // TS: :161 `expansion && this.expandsPastCityTroops()` (short-circuit)
    let without_cities = expansion && expands_past_city_troops(h, cx);
    let p = cx.st.player(h.player());
    if without_cities { cx.st.config().max_troops_without_cities(&p) } else { cx.st.config().max_troops(&p) }
}

/// `expandsPastCityTroops()` (`:166-170`).
pub fn expands_past_city_troops<H: AiAttackHooks + ?Sized>(h: &mut H, cx: &mut Ctx) -> bool {
    has_high_starting_gold(cx.st, h.player())
}

/// `reserveTroops(target)` (`:172-183`).
pub fn reserve_troops<H: AiAttackHooks + ?Sized>(h: &mut H, cx: &mut Ctx, target: Owner) -> f64 {
    // TS: :173 `!target.isPlayer() || target.type() === PlayerType.Bot`
    let expansion = !target.is_player() || cx.st.player(target).player_type() == PlayerType::Bot;
    let max_troops = reserve_basis(h, cx, expansion);
    // TS: :175-178 (short-circuit; `units()` is the live array, read only)
    let bot_with_structures = target.is_player()
        && cx.st.player(target).player_type() == PlayerType::Bot
        && cx.st.player(target).units().as_slice().iter().any(|&u| STRUCTURES.contains(&cx.st.unit(u).unit_type()));
    let use_reserve = target.is_player() && !bot_with_structures;
    let reserve_ratio = if use_reserve { h.ai().reserve_ratio } else { h.ai().expand_ratio };
    max_troops * reserve_ratio
}

/// `sendLandAttack(target, sendTroops = TroopsMinusReserve, opponent?)` (`:194-213`).
pub fn send_land_attack<H: AiAttackHooks + ?Sized>(
    h: &mut H,
    cx: &mut Ctx,
    target: Owner,
    send_troops: SendTroops,
    opponent: Option<SmallId>,
) -> bool {
    // TS: :200 `this.calculateAttackTroops(target, sendTroops, opponent)`
    let troops = match h.calculate_attack_troops(cx, target, send_troops, opponent) {
        Some(t) => t,
        None => return false,
    };
    // TS: :209 `target.isPlayer() ? target.id() : this.game.terraNullius().id()` (TN id = null)
    let target_id = if target.is_player() { Some(cx.st.player(target).player_id().to_string()) } else { None };
    let player = h.player();
    // TS: v36 :232-239 `this.send(new AttackExecution(troops, this.player, targetID), troops)` (sourceTile = null,
    // removeTroops = true defaults)
    send(h, cx, Exec::Attack(AttackExec::new(Some(troops), player, target_id, None, true)), troops);
    true
}

/// `hasLandBorderWithTerraNullius()` (`:233-254`, private). `for..of` over the border set
/// with an early exit; the body only reads.
fn has_land_border_with_terra_nullius<H: AiAttackHooks + ?Sized>(h: &mut H, cx: &mut Ctx) -> bool {
    let st: &State = cx.st;
    let map = &st.map;
    let mut nbuf: [TileRef; 4] = [0; 4];
    for border in st.player(h.player()).border_tiles().iter() {
        let n = map.neighbors4(border, &mut nbuf);
        for &neighbor in &nbuf[..n] {
            if map.is_land(neighbor) && !map.is_impassable(neighbor) && !map.has_owner(neighbor) {
                return true;
            }
        }
    }
    false
}

/// `shoreTiles(player)` (`:259-314`): border tiles that are shore, in border-set order.
pub fn shore_tiles<H: AiAttackHooks + ?Sized>(h: &mut H, cx: &mut Ctx, player: SmallId) -> Vec<TileRef> {
    let _ = h;
    let st: &State = cx.st;
    let mut out = Vec::new();
    // TS: :262-264 `player.borderTiles().forEach(t => { if (game.isShore(t)) out.push(t) })`
    for t in st.player(player).border_tiles().iter() {
        if st.map.is_shore(t) {
            out.push(t);
        }
    }
    out
}

/// `sendBoatAttackToNearbyTerraNullius()` (`:270-313`, private).
fn send_boat_attack_to_nearby_terra_nullius<H: AiAttackHooks + ?Sized>(h: &mut H, cx: &mut Ctx) -> bool {
    let player = h.player();
    if cx.st.config().is_unit_disabled(UnitType::TransportShip) {
        return false;
    }
    // TS: :272-276
    if f64::from(cx.st.player(player).unit_count(UnitType::TransportShip)) >= cx.st.config().boat_max_number() {
        return false;
    }
    // TS: :278-283
    const DIRECTIONS: [(f64, f64); 4] = [(0.0, -1.0), (0.0, 1.0), (-1.0, 0.0), (1.0, 0.0)];
    let shores = shore_tiles(h, cx, player);
    // TS: :286 `for (let i = 0; i < shores.length; i += 10)`
    let mut i = 0usize;
    while i < shores.len() {
        let border = shores[i]; // in-range: i < len
        let bx = f64::from(cx.st.map.x(border));
        let by = f64::from(cx.st.map.y(border));
        for (dx, dy) in DIRECTIONS {
            let x1 = bx + dx;
            let y1 = by + dy;
            if !cx.st.map.is_valid_coord(x1, y1) {
                continue;
            }
            if !cx.st.map.is_water(cx.st.map.ref_(x1, y1)) {
                continue;
            }
            let nx = bx + dx * 5.0;
            let ny = by + dy * 5.0;
            if !cx.st.map.is_valid_coord(nx, ny) {
                continue;
            }
            let tile = cx.st.map.ref_(nx, ny);
            if !cx.st.map.is_land(tile) {
                continue;
            }
            if cx.st.map.is_impassable(tile) {
                continue;
            }
            if cx.st.map.has_owner(tile) {
                continue;
            }
            if cx.st.map.has_fallout(tile) {
                continue;
            }
            // TS: :306-307 `canBuildTransportShip(..)` returns `TileRef | false`
            let Some(launch) = can_build_transport_ship_cx(cx, player, tile) else {
                continue;
            };
            // TS: :308 `this.sail(launch, tile).blocker !== null`
            if h.sail(cx, launch, tile, None).blocker.is_some() {
                continue;
            }
            // TS: :309 `return this.sendExpansionBoat(tile)`
            return h.send_expansion_boat(cx, tile);
        }
        i += 10;
    }
    false
}

/// `boatRoute(target)` (`:331-337`).
pub fn boat_route<H: AiAttackHooks + ?Sized>(h: &mut H, cx: &mut Ctx, target: SmallId) -> Option<BoatRoute> {
    // TS: :332-333 `this.boatRoutes?.get(target)`; `undefined` = no map or no entry
    if let Some(known) = h.ai().boat_routes.as_ref().and_then(|m| m.get(&target)) {
        return known.clone();
    }
    let route = plan_boat_route(h, cx, target);
    // TS: :335 `this.boatRoutes?.set(target, route)`
    if let Some(m) = &mut h.ai().boat_routes {
        m.insert(target, route.clone());
    }
    route
}

/// `planBoatRoute(target)` (`:340-373`, private).
fn plan_boat_route<H: AiAttackHooks + ?Sized>(h: &mut H, cx: &mut Ctx, target: SmallId) -> Option<BoatRoute> {
    let player = h.player();
    // TS: :341-347 (short-circuit `||`)
    if cx.st.config().is_unit_disabled(UnitType::TransportShip)
        || f64::from(cx.st.player(player).unit_count(UnitType::TransportShip)) >= cx.st.config().boat_max_number()
    {
        return None;
    }
    let ours = shore_tiles(h, cx, player);
    let theirs = shore_tiles(h, cx, target);
    // TS: :350-351 `closestTwoTiles(..)`: `{x, y}` → `(x, y)`
    let (_, closest_y) = closest_two_tiles(&cx.st.map, &ours, &theirs)?;
    let warships = hostile_warships(h, cx);
    let direct = route_to(h, cx, closest_y, &warships);
    if direct.as_ref().is_some_and(|d| d.blocker.is_none()) {
        return direct;
    }
    let mut best: Option<BoatRoute> = None;
    // TS: :357-371
    for landing in other_landings(h, cx, &ours, &theirs, closest_y, &warships) {
        let route = route_to(h, cx, landing, &warships);
        if let Some(r) = route
            && r.blocker.is_none()
            && best.as_ref().is_none_or(|b| r.length < b.length)
        {
            best = Some(r);
        }
    }
    // TS: :372 `best ?? direct`
    best.or(direct)
}

/// `routeTo(landing, warships)` (`:376-380`, private).
fn route_to<H: AiAttackHooks + ?Sized>(
    h: &mut H,
    cx: &mut Ctx,
    landing: TileRef,
    warships: &[UnitRef],
) -> Option<BoatRoute> {
    let player = h.player();
    let launch = can_build_transport_ship_cx(cx, player, landing)?;
    // TS: :379 `{ landing, ...this.sail(launch, landing, warships) }`
    let s = h.sail(cx, launch, landing, Some(warships));
    Some(BoatRoute { landing, length: s.length, blocker: s.blocker })
}

/// `otherLandings(ours, theirs, tried, warships)` (`:384-435`, private).
fn other_landings<H: AiAttackHooks + ?Sized>(
    h: &mut H,
    cx: &mut Ctx,
    ours: &[TileRef],
    theirs: &[TileRef],
    tried: TileRef,
    warships: &[UnitRef],
) -> Vec<TileRef> {
    // TS: :390-406
    let count: usize = match cx.st.config().game_config().difficulty {
        Difficulty::Easy => return Vec::new(),
        Difficulty::Medium => 1,
        Difficulty::Hard => 2,
        Difficulty::Impossible => 4,
    };
    // TS: :407 `this.dangerRange() ** 2`
    let range = danger_range(h, cx);
    let range_squared = range * range;
    // TS: :408 `Math.max(1, Math.floor(ours.length / 32))`; int: an integer >= 1
    let our_step = js::max(1.0, js::floor(ours.len() as f64 / 32.0)) as usize;
    // TS: :409 `ours.filter((_, i) => i % ourStep === 0)`
    let our_sample: Vec<TileRef> =
        ours.iter().enumerate().filter(|(i, _)| i % our_step == 0).map(|(_, &t)| t).collect();
    let mut candidates: Vec<(TileRef, f64)> = Vec::new();
    // int: an integer >= 1
    let their_step = js::max(1.0, js::floor(theirs.len() as f64 / 32.0)) as usize;
    let map = &cx.st.map;
    let mut i = 0usize;
    while i < theirs.len() {
        let tile = theirs[i]; // in-range: i < len
        // TS: :415-417 `warships.some(w => euclideanDistSquared(w.tile(), tile) <= rangeSquared)`
        if warships.iter().any(|&w| f64::from(map.euclidean_dist_squared(cx.st.unit(w).tile(), tile)) <= range_squared)
        {
            i += their_step;
            continue;
        }
        let mut dist = f64::INFINITY;
        for &s in &our_sample {
            dist = js::min(dist, f64::from(map.manhattan_dist(s, tile)));
        }
        candidates.push((tile, dist));
        i += their_step;
    }
    // TS: :427 `candidates.sort((a, b) => a.dist - b.dist)` (V8 TimSort; ∞ − ∞ = NaN possible)
    of_det::js_sort_by(&mut candidates, |a, b| a.1 - b.1);
    let mut picked: Vec<TileRef> = vec![tried];
    for &(tile, _) in &candidates {
        if picked.len() > count {
            break;
        }
        if picked.iter().any(|&p| map.manhattan_dist(p, tile) < 50) {
            continue;
        }
        picked.push(tile);
    }
    // TS: :434 `picked.slice(1)`
    picked.split_off(1)
}

/// `hostileWarships()` (`:437-448`).
pub fn hostile_warships<H: AiAttackHooks + ?Sized>(h: &mut H, cx: &mut Ctx) -> Vec<UnitRef> {
    let player = h.player();
    // TS: :438-439 `game.units(UnitType.Warship)` is a fresh copy
    let all = cx.st.units_of_type_memo(UnitType::Warship);
    let mut out = Vec::new();
    for w in all {
        let owner = cx.st.unit(w).owner();
        // TS: :442-445, short-circuit `&&`. `w.warshipState()` is the mutating getter (writes
        // `isInCombat`, `UnitImpl.ts:437-442`) and runs only when the first two hold.
        if owner == player || cx.st.unit(w).is_under_construction() {
            continue;
        }
        let docked = cx.unit_mut(w).warship_state().state == WarshipMode::Docked;
        if !docked && cx.st.player(owner).can_attack_player(player, true) {
            out.push(w);
        }
    }
    out
}

/// `dangerRange()` (`:450-452`).
pub fn danger_range<H: AiAttackHooks + ?Sized>(h: &mut H, cx: &mut Ctx) -> f64 {
    let _ = h;
    cx.st.config().warship_targetting_range() + 20.0
}

/// CS v1 helpers shared by the P9 execution records (`Canon`/`Uncanon` next to each type,
/// FILE_MAP §5 rule 7). Framework-level leaves (`Prng`, alliance-request keys) are not in the
/// P13 framework at `s0`; they live here until P13 adds them (docs/requests/P9.md).
pub(crate) mod p9cs {
    use crate::canon::{CanonError, Cs, UncanonCx};
    use crate::ids::ReqIdx;
    use of_det::Prng;

    /// `zRandom`: the four sfc32 words as an array of ints (SPEC §1.1 tuples are ARRAYs).
    pub(crate) fn prng(r: &Prng) -> Cs {
        Cs::Arr(r.get_state().iter().map(|&w| Cs::Int(i64::from(w))).collect())
    }
    pub(crate) fn prng_from(v: &Cs) -> Result<Prng, CanonError> {
        let a = v.as_arr().ok_or_else(|| CanonError::new(format!("expected PRNG words, got {v:?}")))?;
        if a.len() != 4 {
            return Err(CanonError::new(format!("expected 4 PRNG words, got {}", a.len())));
        }
        let mut w = [0i32; 4];
        for (o, x) in w.iter_mut().zip(a) {
            *o = x
                .as_i64()
                .and_then(|x| i32::try_from(x).ok())
                .ok_or_else(|| CanonError::new(format!("bad PRNG word {x:?}")))?;
        }
        Ok(Prng::from_state(w))
    }
    /// The request whose key is `v` (keys are unique, SPEC §1.2).
    pub(crate) fn req_from(v: &Cs, cx: &UncanonCx<'_>) -> Result<ReqIdx, CanonError> {
        let k = v.as_str().ok_or_else(|| CanonError::new(format!("expected request key, got {v:?}")))?;
        let mut found = None;
        for (i, q) in cx.st.alliance_reqs.iter().enumerate() {
            if format!("{}:{}:{}", q.requestor.0, q.recipient.0, q.created_at) == k {
                if found.is_some() {
                    return Err(CanonError::new(format!("duplicate alliance request key {k}")));
                }
                found = Some(i);
            }
        }
        let i = found.ok_or_else(|| CanonError::new(format!("unknown alliance request key {k}")))?;
        u32::try_from(i).map(ReqIdx).map_err(|_| CanonError::new("request index"))
    }
    /// Field `k` of record `v`.
    pub(crate) fn field<'v>(v: &'v Cs, k: &str) -> Result<&'v Cs, CanonError> {
        v.get(k).ok_or_else(|| CanonError::new(format!("missing field {k}")))
    }
    /// A `player: uint` reference.
    pub(crate) fn small_id(v: &Cs) -> Result<crate::ids::SmallId, CanonError> {
        v.as_i64()
            .and_then(|x| u16::try_from(x).ok())
            .map(crate::ids::SmallId)
            .ok_or_else(|| CanonError::new(format!("expected smallID, got {v:?}")))
    }
    /// A `uint?null` player reference.
    pub(crate) fn opt_small_id(v: &Cs) -> Result<Option<crate::ids::SmallId>, CanonError> {
        if v.is_null() { Ok(None) } else { small_id(v).map(Some) }
    }
    pub(crate) fn bool_(v: &Cs) -> Result<bool, CanonError> {
        v.as_bool().ok_or_else(|| CanonError::new(format!("expected bool, got {v:?}")))
    }
    pub(crate) fn num(v: &Cs) -> Result<f64, CanonError> {
        v.as_f64().ok_or_else(|| CanonError::new(format!("expected num, got {v:?}")))
    }
    pub(crate) fn str_(v: &Cs) -> Result<String, CanonError> {
        v.as_str().map(str::to_owned).ok_or_else(|| CanonError::new(format!("expected str, got {v:?}")))
    }
    /// An `enum{"start","stop"}` embargo action.
    pub(crate) fn embargo_action(v: &Cs) -> Result<crate::api::intents::EmbargoAction, CanonError> {
        use crate::api::intents::EmbargoAction;
        match v.as_str() {
            Some("start") => Ok(EmbargoAction::Start),
            Some("stop") => Ok(EmbargoAction::Stop),
            _ => Err(CanonError::new(format!("expected embargo action, got {v:?}"))),
        }
    }
    pub(crate) fn embargo_action_cs(a: crate::api::intents::EmbargoAction) -> Cs {
        use crate::api::intents::EmbargoAction;
        Cs::Str(
            match a {
                EmbargoAction::Start => "start",
                EmbargoAction::Stop => "stop",
            }
            .to_string(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::Scratch;

    /// FILE_MAP §4 P9 mandatory design test, static half: in the generic base code every TS
    /// `this.<hook>(` site is an `h.<hook>(` call, and no base function body calls a
    /// `base_*` function (only the trait defaults may). The runtime half needs P2/P3 bodies
    /// (`hook_dispatch_runtime` below).
    #[test]
    fn hook_sites_go_through_h() {
        let src = include_str!("ai_attack.rs");
        let code = src.split("#[cfg(test)]").next().unwrap_or(src);
        let (_, after_trait) = code.split_once("// ---- base implementations of the 7 hooks").unwrap_or(("", ""));
        assert!(!after_trait.is_empty());
        // 1. No `base_<hook>(` call outside the trait's default bodies.
        for hook in [
            "should_attack",
            "ignores_attacks_from",
            "expansion_troop_cap",
            "calculate_attack_troops",
            "send_boat_attack",
            "send_expansion_boat",
            "sail",
            "troops_at_init",
        ] {
            let call = format!("base_{hook}(");
            let defs = after_trait.matches(&format!("pub fn base_{hook}<")).count();
            let calls = after_trait.matches(&call).count();
            assert_eq!(calls, 0, "{call} called from base code: TS calls it through `this.`, use `h.{hook}(`");
            assert_eq!(defs, 1, "base_{hook} must be defined once");
        }
        // 2. Every TS `this.<hook>(` site of AiAttackBehavior.ts maps to an `h.<hook>(` call:
        //    :94 shouldAttack, :100 sendBoatAttack, :133 ignoresAttacksFrom,
        //    :200 + :219 calculateAttackTroops, :308 sail + :379 sail, :309 sendExpansionBoat,
        //    :317 expansionTroopCap; rules-v36: v36 :158 troopsAtInit (in homeTroops).
        for (hook, n) in [
            ("should_attack", 1),
            ("send_boat_attack", 1),
            ("ignores_attacks_from", 1),
            ("calculate_attack_troops", 2),
            ("sail", 2),
            ("send_expansion_boat", 1),
            ("expansion_troop_cap", 1),
            ("troops_at_init", 1),
        ] {
            let got = after_trait.matches(&format!("h.{hook}(cx")).count();
            assert_eq!(got, n, "expected {n} `h.{hook}(` call sites (one per TS `this.` site), found {got}");
        }
    }

    #[test]
    fn base_new_and_send_troops_fixed() {
        let b = AiAttackBase::new(0.55, 0.38, 0.13);
        assert_eq!((b.trigger_ratio, b.reserve_ratio, b.expand_ratio), (0.55, 0.38, 0.13));
        assert!(b.boat_routes.is_none());
        assert!(b.sent_this_tick.is_none());
        assert_eq!(SendTroops::Fixed(12.5).eval_home(1000.0, 99.0), 12.5);
        assert_eq!(SendTroops::TroopsMinusReserve.eval_home(1000.0, 99.0), 901.0);
        assert_eq!(SendTroops::TroopsDiv5.eval_home(1000.0, 99.0), 200.0);
    }

    /// rules-v36 (t713, upstream #6297): a hook implementer whose `troopsAtInit()` is a fixed number (the nation
    /// override's `min(troops, maxTroops)` reduces to this once the cut is applied).
    struct Home {
        base: AiAttackBase,
        rng: Prng,
        at_init: f64,
    }
    impl AiAttackHooks for Home {
        fn ai(&mut self) -> &mut AiAttackBase {
            &mut self.base
        }
        fn rng(&mut self) -> &mut Prng {
            &mut self.rng
        }
        fn player(&self) -> SmallId {
            SmallId(1)
        }
        fn troops_at_init(&mut self, _cx: &mut Ctx) -> f64 {
            self.at_init
        }
    }

    /// v36 `homeTroops()`/`send()` (AiAttackBehavior.ts:156-178), the mechanism behind upstream's new tests "a second
    /// attack sent in the same tick keeps the reserve home" and "keeps the reserve when troops above max are cut back":
    /// the second budget of a tick sees what the first send took; a send larger than what is home counts only what is
    /// home; the next tick starts from troopsAtInit again; home never goes below 0.
    #[test]
    fn home_troops_and_send_track_this_tick() {
        let mut st = State::default();
        let mut sc = Scratch::new();
        let mut cx = Ctx::new(&mut st, &mut sc, None);
        let mut h = Home { base: AiAttackBase::new(0.5, 0.3, 0.1), rng: Prng::new(1.0), at_init: 1000.0 };
        let t0 = cx.st.ticks();
        assert_eq!(home_troops(&mut h, &mut cx), 1000.0);
        // a strike sends 700 (above the 30% reserve): the regular decision then budgets from 300, not 1000
        send(&mut h, &mut cx, Exec::Attack(AttackExec::new(Some(700.0), SmallId(1), None, None, true)), 700.0);
        assert_eq!(h.base.sent_this_tick, Some((t0, 700.0)));
        assert_eq!(home_troops(&mut h, &mut cx), 300.0);
        assert_eq!(SendTroops::TroopsMinusReserve.eval(&mut h, &mut cx, 300.0), 0.0);
        // a send of more than is home counts only what is home (Math.min(troops, homeTroops()))
        send(&mut h, &mut cx, Exec::Attack(AttackExec::new(Some(500.0), SmallId(1), None, None, true)), 500.0);
        assert_eq!(h.base.sent_this_tick, Some((t0, 1000.0)));
        assert_eq!(home_troops(&mut h, &mut cx), 0.0);
        // troopsAtInit falling below what was sent (e.g. the nation's cut to max) floors at 0
        h.at_init = 600.0;
        assert_eq!(home_troops(&mut h, &mut cx), 0.0);
        assert_eq!(cx.st.exec_inbox.len(), 2);
        // a record from an earlier tick is ignored (`sent.tick !== this.game.ticks()`)
        h.base.sent_this_tick = Some((t0 - 1, 999.0));
        assert_eq!(home_troops(&mut h, &mut cx), 600.0);
        send(&mut h, &mut cx, Exec::Attack(AttackExec::new(Some(100.0), SmallId(1), None, None, true)), 100.0);
        assert_eq!(h.base.sent_this_tick, Some((t0, 100.0)));
        // the trigger-ratio check reads homeTroops too
        assert_eq!(home_troops(&mut h, &mut cx), 500.0);
    }

    /// A hook implementer that overrides `should_attack` to refuse and records the call.
    struct Probe {
        base: AiAttackBase,
        rng: Prng,
        calls: Vec<&'static str>,
    }
    impl AiAttackHooks for Probe {
        fn ai(&mut self) -> &mut AiAttackBase {
            &mut self.base
        }
        fn rng(&mut self) -> &mut Prng {
            &mut self.rng
        }
        fn player(&self) -> SmallId {
            SmallId(1)
        }
        fn should_attack(&mut self, _cx: &mut Ctx, _other: Owner) -> bool {
            self.calls.push("should_attack");
            false
        }
    }

    /// `sendAttack` without `force` consults the (overridden) hook first (`:94`), and with
    /// `force` skips it. Runs without any other package's bodies: the refusal returns before
    /// any state read.
    #[test]
    fn send_attack_calls_should_attack_through_h() {
        let mut st = State::default();
        let mut sc = Scratch::new();
        let mut cx = Ctx::new(&mut st, &mut sc, None);
        let mut p = Probe { base: AiAttackBase::new(0.5, 0.3, 0.1), rng: Prng::new(1.0), calls: vec![] };
        assert!(!send_attack(&mut p, &mut cx, SmallId(2), false));
        assert_eq!(p.calls, ["should_attack"]);
        assert!(cx.st.exec_inbox.is_empty());
    }

    /// `boatRoute` returns a memoised entry (including a memoised `null`) without planning
    /// (`:332-333`); planning would reach the P3 config stubs.
    #[test]
    fn boat_route_memo_hit_skips_planning() {
        let mut st = State::default();
        let mut sc = Scratch::new();
        let mut cx = Ctx::new(&mut st, &mut sc, None);
        let mut p = Probe { base: AiAttackBase::new(0.5, 0.3, 0.1), rng: Prng::new(1.0), calls: vec![] };
        let mut m = JsMap::new();
        let r = BoatRoute { landing: 7, length: 3.0, blocker: None };
        m.insert(SmallId(2), Some(r.clone()));
        m.insert(SmallId(3), None);
        p.base.boat_routes = Some(m);
        assert_eq!(boat_route(&mut p, &mut cx, SmallId(2)), Some(r));
        assert_eq!(boat_route(&mut p, &mut cx, SmallId(3)), None);
    }

    /// The `AiAttackBehavior v2` record shape (`{v, d}`, dump field order).
    #[test]
    fn canon_record_round_trip() {
        let st = State::default();
        let ex = crate::exec::ExecStore::default();
        let mut ccx = CanonCx::new(&st, &ex);
        let b = AiAttackBase::new(0.55, 0.38, 0.13);
        let cs = b.canon_record(&mut ccx, 0.0, Cs::Arr(vec![]));
        let mut st2 = State::default();
        let mut ucx = UncanonCx { st: &mut st2, keys: &crate::canon::LoadKeys::default() };
        let (b2, bot, fl) = AiAttackBase::uncanon_record(&cs, &mut ucx).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!((b2.trigger_ratio, b2.reserve_ratio, b2.expand_ratio, bot), (0.55, 0.38, 0.13, 0.0));
        assert_eq!(fl, Cs::Arr(vec![]));
        // v1 migration (`:479-482`)
        let v1 = Cs::Obj(vec![
            ("v".into(), Cs::Int(1)),
            (
                "d".into(),
                Cs::Obj(vec![
                    ("botAttackTroopsSent".into(), Cs::Int(5)),
                    ("triggerRatio".into(), Cs::Num(0.5)),
                    ("reserveRatio".into(), Cs::Num(0.3)),
                    ("expandRatio".into(), Cs::Num(0.1)),
                ]),
            ),
        ]);
        let (_, bot1, fl1) = AiAttackBase::uncanon_record(&v1, &mut ucx).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!((bot1, fl1), (5.0, Cs::Arr(vec![])));
    }
}
