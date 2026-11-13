//! `TransportShipExecution` (`engine/src/execution/TransportShipExecution.ts`). Owner: **P7**.
//!
//! Fields: the `TransportShip v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1).
//!
//! `troops`: TS opens `init` with `this.troops ??= config.boatAttackAmount(..)` (`:117-119`).
//! The frozen constructor takes `troops: f64`, and every TS caller passes a number (the
//! zod-checked intent float, `AiAttackBehavior.ts:228,321`, `NationAttackBehavior.ts:276,1246`),
//! so the `??=` never fires (`NaN` is not nullish) and is not ported.

use crate::canon::{Canon, CanonCx, Cs};
use crate::exec::attack::AttackExec;
use crate::exec::prelude::*;
use crate::exec::trade_ship::water_path_finder_canon;
use crate::path::types::PathResult;
use crate::state::game::ShipKind;
use crate::state::stats::StatNum;
use crate::state::transport_ship_utils::target_transport_tile;
use crate::state::unit::{TransportShipStateUpdate, UnitParams};

/// `const malusForRetreat = 25` (`:35`).
const MALUS_FOR_RETREAT: f64 = 25.0;

/// `retreatDst: TileRef | false` (the `null` case is `None`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetreatDst {
    Tile(TileRef),
    False,
}
crate::plain_copy!(RetreatDst);

/// `TransportShipExecution` data.
#[derive(StateClone, Debug)]
pub struct TransportShipExec {
    pub active: bool,
    /// `initialized: this.mg !== undefined` (set by `init` after the `isValidRef` check).
    pub initialized: bool,
    /// `ticksPerMove = 1`
    pub ticks_per_move: i32,
    pub last_move: Option<i32>,
    pub target: Option<Owner>,
    pub path_finder: Option<WaterPathFinder>,
    pub dst: Tri<TileRef>,
    pub src: Tri<TileRef>,
    /// `retreatDst: TileRef | false | null = null`
    pub retreat_dst: Option<RetreatDst>,
    pub boat: Option<UnitRef>,
    pub motion_plan_id: i32,
    pub motion_plan_dst: Option<TileRef>,
    pub original_owner: SmallId,
    pub attacker: SmallId,
    /// `ref`
    pub ref_: TileRef,
    pub troops: f64,
}

impl TransportShipExec {
    /// `new TransportShipExecution(attacker, ref, troops)` (`:57-63`).
    pub fn new(attacker: SmallId, ref_: TileRef, troops: f64) -> Self {
        // TS: :38-53 field initialisers; :62 `this.originalOwner = this.attacker`.
        // `dst`/`src`/`lastMove`/`target`/`boat`/`pathFinder` are declared without an
        // initialiser, i.e. `undefined`.
        TransportShipExec {
            active: true,
            initialized: false,
            ticks_per_move: 1,
            last_move: None,
            target: None,
            path_finder: None,
            dst: Tri::Absent,
            src: Tri::Absent,
            retreat_dst: None,
            boat: None,
            motion_plan_id: 1,
            motion_plan_dst: None,
            original_owner: attacker,
            attacker,
            ref_,
            troops,
        }
    }

    /// `owner(): Player` (`:333-335`).
    pub fn owner(&self) -> SmallId {
        self.attacker
    }

    fn pf(&mut self) -> &mut WaterPathFinder {
        self.path_finder.as_mut().ts_expect("TransportShipExecution.ts this.pathFinder (set by init)")
    }

    /// `rejectIncomingAllianceRequests(target)` (`:341-348`).
    fn reject_incoming_alliance_requests(&mut self, cx: &mut Ctx, target: SmallId) {
        // TS: :342-344 `attacker.incomingAllianceRequests().find(ar => ar.requestor() === target)`
        let request = cx
            .st
            .player(self.attacker)
            .incoming_alliance_requests()
            .into_iter()
            .find(|&r| cx.st.alliance_request(r).requestor() == target);
        // TS: :345-347
        if let Some(r) = request {
            cx.reject_alliance_request(r);
        }
    }

    /// `fullPath = pathFinder.findPath(from, dst) ?? [from]; if (fullPath.length === 0 ||
    /// fullPath[0] !== from) fullPath.unshift(from)` (`:149-152`, `:314-319`). The path only
    /// feeds the motion-plan record (view), but `findPath` can write the water memo, so the
    /// call is kept; the unshift is pure and dropped.
    fn find_full_path(&mut self, cx: &mut Ctx, from: TileRef, dst: TileRef) {
        let _full_path = self.pf().find_path(cx, &[from], dst);
    }
}

impl ExecBehavior for TransportShipExec {
    fn is_active(&self) -> bool {
        self.active
    }
    /// `activeDuringSpawnPhase()` returns `false` (`TransportShipExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    fn init(&mut self, cx: &mut Ctx, ticks: i32) {
        // TS: :70-74 (warn dropped)
        if !cx.st.map.is_valid_ref(f64::from(self.ref_)) {
            self.active = false;
            return;
        }

        // TS: :76-80
        self.last_move = Some(ticks);
        self.initialized = true;
        let target = cx.st.owner(self.ref_);
        self.target = Some(target);
        let stagger = cx.next_ship_stagger(ShipKind::TransportShip);
        self.path_finder = Some(WaterPathFinder::new(cx, stagger, false));

        // TS: :82-95 (displayMessage dropped)
        if f64::from(cx.st.player(self.attacker).unit_count(UnitType::TransportShip))
            >= cx.st.config().boat_max_number()
        {
            self.active = false;
            return;
        }

        // TS: :97-105
        if target.is_player()
            && cx.st.player(target).player_type() != PlayerType::Bot
            && cx.st.player(self.attacker).player_type() != PlayerType::Bot
        {
            self.reject_incoming_alliance_requests(cx, target);
        }

        // TS: :107-110
        if target == self.attacker {
            self.active = false;
            return;
        }

        // TS: :112-115 (`treatAFKFriendly` defaults to false)
        if target.is_player() && !cx.st.player(self.attacker).can_attack_player(target, false) {
            self.active = false;
            return;
        }

        // TS: :117-119 `troops ??= …` never fires (module docs).
        // TS: :120 `Math.min(this.troops, this.attacker.troops())`
        self.troops = of_det::js::min(self.troops, cx.st.player(self.attacker).troops());

        // TS: :122 `targetTransportTile(mg, attacker, ref)`
        let dst = target_transport_tile(&mut cx.rcx(), self.attacker, self.ref_);
        self.dst = match dst {
            Some(d) => Tri::Some(d),
            None => Tri::Null,
        };
        // TS: :124-130 (warn dropped)
        let Some(dst) = dst else {
            self.active = false;
            return;
        };

        // TS: :132 `attacker.canBuild(TransportShip, dst)`
        let src = cx.st.player(self.attacker).can_build(cx.sc, UnitType::TransportShip, dst, None);
        // TS: :134-140
        let Some(src) = src else {
            self.active = false;
            return;
        };
        // TS: :142
        self.src = Tri::Some(src);

        // TS: :144-147 `buildUnit(TransportShip, src, {troops, targetTile: dst})`
        let params = UnitParams { troops: Some(self.troops), target_tile: Some(Some(dst)), ..UnitParams::default() };
        let boat = cx.player_mut(self.attacker).build_unit(UnitType::TransportShip, src, params);
        self.boat = Some(boat);

        // TS: :149-152
        self.find_full_path(cx, src, dst);

        // TS: :154-163 `recordMotionPlan({kind: "grid", unitId: boat.id(), ...})`
        let unit_id = cx.st.unit(boat).id();
        cx.record_motion_plan(unit_id);
        self.motion_plan_dst = Some(dst);

        // TS: :165-173 displayIncomingUnit (view) dropped.

        // TS: :175-178 `stats().boatSendTroops(attacker, target, boat.troops())`
        let boat_troops = cx.st.unit(boat).troops();
        cx.st.stats_cx().boat_send_troops(self.attacker, target, StatNum::Num(boat_troops));
    }
    fn tick(&mut self, cx: &mut Ctx, ticks: i32) {
        // TS: :182-185 `if (this.dst === null)` (strict: an `undefined` dst falls through)
        if matches!(self.dst, Tri::Null) {
            self.active = false;
            return;
        }
        // TS: :186-188
        if !self.active {
            return;
        }
        let boat = self.boat.ts_expect("TransportShipExecution.ts:189 this.boat");
        // TS: :189-192
        if !cx.st.unit(boat).is_active() {
            self.active = false;
            return;
        }
        // TS: :193-195 (`undefined` lastMove: NaN < 1 is false)
        if let Some(last) = self.last_move
            && ticks - last < self.ticks_per_move
        {
            return;
        }
        // TS: :196
        self.last_move = Some(ticks);

        // TS: :198-208 a teammate can conquer a disconnected player and get their ships
        let boat_owner = cx.st.unit(boat).owner();
        if cx.st.player(self.original_owner).is_disconnected()
            && boat_owner != self.original_owner
            && cx.st.player(boat_owner).is_on_same_team(self.original_owner)
        {
            self.attacker = boat_owner;
            self.original_owner = boat_owner; // for when this owner disconnects too
        }

        // TS: :210-212 `if (this.pathFinder.rebuilt)`
        if self.pf().take_rebuilt(cx) {
            self.motion_plan_dst = None; // force motion plan re-recording
        }

        // TS: :214-223 auto-retreat if the destination was destroyed by a nuke (now water)
        if let Tri::Some(d) = self.dst
            && cx.st.map.is_water(d)
        {
            if !cx.st.unit(boat).transport_ship_state().is_retreating {
                cx.unit_mut(boat)
                    .update_transport_ship_state(TransportShipStateUpdate { is_retreating: Some(true), troops: None });
            }
            // Reset the cached retreat destination so it is recomputed from the current position.
            self.retreat_dst = None;
        }

        // TS: :225-246
        if cx.st.unit(boat).transport_ship_state().is_retreating {
            // TS: :227-229 `retreatDst ??= attacker.bestTransportShipSpawn(boat.tile())`
            let retreat_dst = match self.retreat_dst {
                Some(r) => r,
                None => {
                    let btile = cx.st.unit(boat).tile();
                    let r = match cx.st.player(self.attacker).best_transport_ship_spawn(cx.sc, btile) {
                        Some(t) => RetreatDst::Tile(t),
                        None => RetreatDst::False,
                    };
                    self.retreat_dst = Some(r);
                    r
                }
            };
            match retreat_dst {
                // TS: :231-238 (warn dropped)
                RetreatDst::False => {
                    let troops = cx.st.unit(boat).troops();
                    cx.player_mut(self.attacker).add_troops(troops);
                    cx.unit_mut(boat).delete(Some(false), None);
                    self.active = false;
                    return;
                }
                // TS: :239-245
                RetreatDst::Tile(t) => {
                    self.dst = Tri::Some(t);
                    if cx.st.unit(boat).target_tile() != Some(t) {
                        cx.unit_mut(boat).set_target_tile(Some(t));
                    }
                }
            }
        }

        // `dst` is a tile here: `null` returned above, and `undefined` means init returned
        // before `this.dst = …`, i.e. `active` is false and tick returned at :186-188.
        let dst = match self.dst {
            Tri::Some(d) => d,
            _ => of_det::ts_throw!(TypeError, "TransportShipExecution.ts:248 dst is not a tile"),
        };
        let target = self.target.ts_expect("TransportShipExecution.ts:251 this.target");

        // TS: :248
        let btile = cx.st.unit(boat).tile();
        let result = self.pf().next(cx, btile, dst, None);
        match result {
            PathResult::Complete(_) => {
                // TS: :251-272 arrived on own territory: retreat with a malus
                if cx.st.owner(dst) == self.attacker {
                    let boat_troops = cx.st.unit(boat).troops();
                    let deaths = boat_troops * (MALUS_FOR_RETREAT / 100.0);
                    let survivors = cx.st.unit(boat).troops() - deaths;
                    cx.player_mut(self.attacker).add_troops(survivors);
                    cx.unit_mut(boat).delete(Some(false), None);
                    self.active = false;
                    // TS: :258-261
                    cx.st.stats_cx().boat_arrive_troops(self.attacker, target, StatNum::Num(survivors));
                    // TS: :262-270 displayMessage (view) dropped.
                    return;
                }
                // TS: :273
                cx.player_mut(self.attacker).conquer(dst);
                // TS: :274-286
                if target.is_player() && cx.st.player(self.attacker).is_friendly(target, false) {
                    let troops = cx.st.unit(boat).troops();
                    cx.player_mut(self.attacker).add_troops(troops);
                } else {
                    // `new AttackExecution(boat.troops(), attacker, target.id(), dst, false)`;
                    // `target.id()` is null for TerraNullius.
                    let troops = cx.st.unit(boat).troops();
                    let target_id =
                        if target.is_player() { Some(cx.st.player(target).player_id().to_owned()) } else { None };
                    cx.st.add_execution(Exec::Attack(AttackExec::new(
                        Some(troops),
                        self.attacker,
                        target_id,
                        Some(dst),
                        false,
                    )));
                }
                // TS: :287-288
                cx.unit_mut(boat).delete(Some(false), None);
                self.active = false;
                // TS: :290-293
                let troops = cx.st.unit(boat).troops();
                cx.st.stats_cx().boat_arrive_troops(self.attacker, target, StatNum::Num(troops));
                return;
            }
            // TS: :295-297
            PathResult::Next(node) => {
                cx.unit_mut(boat).move_to(node);
            }
            // TS: :298-309 (warn dropped)
            PathResult::NotFound => {
                let troops = cx.st.unit(boat).troops();
                cx.player_mut(self.attacker).add_troops(troops);
                cx.unit_mut(boat).delete(Some(false), None);
                self.active = false;
                return;
            }
        }

        // TS: :312-330
        if let Tri::Some(dst) = self.dst
            && Some(dst) != self.motion_plan_dst
        {
            self.motion_plan_id += 1;
            let btile = cx.st.unit(boat).tile();
            self.find_full_path(cx, btile, dst);
            let unit_id = cx.st.unit(boat).id();
            cx.record_motion_plan(unit_id);
            self.motion_plan_dst = Some(dst);
        }
    }
}

/// `TransportShip v1`: `{ active, initialized, ticksPerMove, lastMove?, target?,
/// pathFinder?, dst?null, src?null, retreatDst: (tile|false)|null, boat?, motionPlanId,
/// motionPlanDst|null, originalOwner, attacker, ref, troops: num }`.
impl Canon for TransportShipExec {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        let tri_tile = |t: Tri<TileRef>| match t {
            Tri::Absent => Cs::Absent,
            Tri::Null => Cs::Null,
            Tri::Some(x) => Cs::Int(i64::from(x)),
        };
        Cs::Obj(vec![
            ("active".into(), Cs::Bool(self.active)),
            ("initialized".into(), Cs::Bool(self.initialized)),
            ("ticksPerMove".into(), Cs::Int(i64::from(self.ticks_per_move))),
            ("lastMove".into(), self.last_move.map_or(Cs::Absent, |t| Cs::Int(i64::from(t)))),
            ("target".into(), self.target.map_or(Cs::Absent, |t| t.canon(cx))),
            ("pathFinder".into(), self.path_finder.as_ref().map_or(Cs::Absent, water_path_finder_canon)),
            ("dst".into(), tri_tile(self.dst)),
            ("src".into(), tri_tile(self.src)),
            (
                "retreatDst".into(),
                match self.retreat_dst {
                    None => Cs::Null,
                    Some(RetreatDst::False) => Cs::Bool(false),
                    Some(RetreatDst::Tile(t)) => Cs::Int(i64::from(t)),
                },
            ),
            ("boat".into(), self.boat.map_or(Cs::Absent, |b| b.canon(cx))),
            ("motionPlanId".into(), Cs::Int(i64::from(self.motion_plan_id))),
            ("motionPlanDst".into(), self.motion_plan_dst.map_or(Cs::Null, |t| Cs::Int(i64::from(t)))),
            ("originalOwner".into(), self.original_owner.canon(cx)),
            ("attacker".into(), self.attacker.canon(cx)),
            ("ref".into(), Cs::Int(i64::from(self.ref_))),
            ("troops".into(), Cs::Num(self.troops)),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TS field initialisers (`:38-62`): `dst`/`src` start `undefined` (not `null`), so a
    /// tick after a failed init falls through the `dst === null` check to `!active`.
    #[test]
    fn constructor_matches_ts_initialisers() {
        let e = TransportShipExec::new(SmallId(3), 77, 12.5);
        assert!(e.active && !e.initialized);
        assert_eq!((e.ticks_per_move, e.motion_plan_id), (1, 1));
        assert_eq!((e.dst, e.src), (Tri::Absent, Tri::Absent));
        assert_eq!((e.retreat_dst, e.motion_plan_dst, e.last_move), (None, None, None));
        assert_eq!((e.original_owner, e.attacker, e.owner()), (SmallId(3), SmallId(3), SmallId(3)));
        assert_eq!((e.ref_, e.troops), (77, 12.5));
    }

    /// `boat.troops() * (malusForRetreat / 100)`: 25/100 is exactly 0.25 in f64, so the
    /// malus split is exact for these values (node: `10 * (25/100)` = 2.5).
    #[test]
    fn retreat_malus_expression() {
        let troops = 10.0;
        let deaths = troops * (MALUS_FOR_RETREAT / 100.0);
        assert_eq!((deaths, troops - deaths), (2.5, 7.5));
    }
}
