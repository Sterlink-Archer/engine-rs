//! `TradeShipExecution` (`engine/src/execution/TradeShipExecution.ts`). Owner: **P7**.
//!
//! Fields: the `TradeShip v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1).
//!
//! Also holds the P7-private CS helpers for the nested pathfinder/PRNG records that the
//! P7 executions embed (`prng_canon`, `stepper_canon`, `water_path_finder_canon`): the S0
//! skeleton has no `Canon` impls for `Prng`/`StepperState`/`WaterPathFinder` yet
//! (`docs/requests/P7.md` R3).

use crate::canon::{Canon, CanonCx, Cs};
use crate::exec::prelude::*;
use crate::path::stepper::StepperState;
use crate::path::types::PathResult;
use crate::state::game::ShipKind;
use crate::state::unit::UnitParams;
use crate::util::find_closest_by;

/// `TradeShipExecution` data.
#[derive(StateClone, Debug)]
pub struct TradeShipExec {
    pub active: bool,
    /// `initialized: this.mg !== undefined` (set by `init`).
    pub initialized: bool,
    pub trade_ship: Option<UnitRef>,
    pub was_captured: bool,
    /// memoized (`:46`)
    pub path_finder: Option<WaterPathFinder>,
    pub tiles_traveled: i32,
    /// `motionPlanId = 1`
    pub motion_plan_id: i32,
    pub motion_plan_dst: Option<TileRef>,
    pub orig_owner: SmallId,
    pub src_port: UnitRef,
    pub dst_port: UnitRef,
}

impl TradeShipExec {
    /// `new TradeShipExecution(origOwner, srcPort, dstPort)`.
    pub fn new(orig_owner: SmallId, src_port: UnitRef, dst_port: UnitRef) -> Self {
        // TS: :28-35 `active = true`, `wasCaptured = false`, `tilesTraveled = 0`,
        // `motionPlanId = 1`, `motionPlanDst = null`
        TradeShipExec {
            active: true,
            initialized: false,
            trade_ship: None,
            was_captured: false,
            path_finder: None,
            tiles_traveled: 0,
            motion_plan_id: 1,
            motion_plan_dst: None,
            orig_owner,
            src_port,
            dst_port,
        }
    }

    /// `dstPort(): TileRef` (`:233-235`).
    pub fn dst_port_tile(&self, cx: &Ctx) -> TileRef {
        cx.st.unit(self.dst_port).tile()
    }

    fn pf(&mut self) -> &mut WaterPathFinder {
        self.path_finder.as_mut().ts_expect("TradeShipExecution.ts this.pathFinder (set by init)")
    }

    /// `complete()` (`:183-223`).
    fn complete(&mut self, cx: &mut Ctx, trade_ship: UnitRef) {
        // TS: :184-185
        self.active = false;
        cx.unit_mut(trade_ship).delete(Some(false), None);
        // TS: :186-188 `config.tradeShipGold(tilesTraveled, tradeShip.owner())`
        let ship_owner = cx.st.unit(trade_ship).owner();
        let gold = cx.st.config().trade_ship_gold(f64::from(self.tiles_traveled), &cx.st.player(ship_owner));
        let dst_tile = cx.st.unit(self.dst_port).tile();

        if self.was_captured && ship_owner == self.orig_owner {
            // TS: :190-192 retaken by its original owner
            cx.player_mut(self.orig_owner).add_gold(gold, Some(dst_tile));
        } else if self.was_captured {
            // TS: :193-211 (displayMessage dropped)
            cx.player_mut(ship_owner).add_gold(gold, Some(dst_tile));
            let ship_owner = cx.st.unit(trade_ship).owner();
            cx.player_mut(ship_owner).add_piracy_gold(gold);
            let ship_owner = cx.st.unit(trade_ship).owner();
            cx.st.stats_cx().boat_captured_trade(ship_owner, self.orig_owner, gold.into());
        } else {
            // TS: :212-221 (each `this.srcPort.owner()` / `this._dstPort.owner()` re-read)
            let src_owner = cx.st.unit(self.src_port).owner();
            let src_tile = cx.st.unit(self.src_port).tile();
            cx.player_mut(src_owner).add_gold(gold, Some(src_tile));
            let dst_owner = cx.st.unit(self.dst_port).owner();
            let dst_tile = cx.st.unit(self.dst_port).tile();
            cx.player_mut(dst_owner).add_gold(gold, Some(dst_tile));
            let src_owner = cx.st.unit(self.src_port).owner();
            cx.player_mut(src_owner).add_trade_gold(gold);
            let dst_owner = cx.st.unit(self.dst_port).owner();
            cx.player_mut(dst_owner).add_trade_gold(gold);
            let (src_owner, dst_owner) = (cx.st.unit(self.src_port).owner(), cx.st.unit(self.dst_port).owner());
            cx.st.stats_cx().boat_arrive_trade(src_owner, dst_owner, gold.into());
        }
    }
}

impl ExecBehavior for TradeShipExec {
    fn is_active(&self) -> bool {
        self.active
    }
    /// `activeDuringSpawnPhase()` returns `false` (`TradeShipExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    fn init(&mut self, cx: &mut Ctx, _ticks: i32) {
        // TS: :44
        self.initialized = true;
        // TS: :45-46 `nextShipStagger("tradeShip")`; memoized: port tile to port tile repeats
        let stagger = cx.next_ship_stagger(ShipKind::TradeShip);
        self.path_finder = Some(WaterPathFinder::new(cx, stagger, true));
    }
    fn tick(&mut self, cx: &mut Ctx, ticks: i32) {
        // TS: :50-52 `if (this.pathFinder.rebuilt)` (getter: ensureFresh + read-and-reset)
        if self.pf().take_rebuilt(cx) {
            self.motion_plan_dst = None; // force motion plan re-recording
        }

        // TS: :54-69
        let trade_ship = match self.trade_ship {
            Some(t) => t,
            None => {
                // TS: :55-58 `origOwner.canBuild(TradeShip, srcPort.tile())`
                let src_tile = cx.st.unit(self.src_port).tile();
                let spawn = cx.st.player(self.orig_owner).can_build(cx.sc, UnitType::TradeShip, src_tile, None);
                // TS: :59-63 (warn dropped)
                let Some(spawn) = spawn else {
                    self.active = false;
                    return;
                };
                // TS: :64-67 `buildUnit(TradeShip, spawn, {targetUnit: dstPort,
                // lastSetSafeFromPirates: ticks})`
                let params = UnitParams {
                    target_unit: Some(Some(self.dst_port)),
                    last_set_safe_from_pirates: Some(Some(ticks)),
                    ..UnitParams::default()
                };
                let t = cx.player_mut(self.orig_owner).build_unit(UnitType::TradeShip, spawn, params);
                self.trade_ship = Some(t);
                // TS: :68 `stats().boatSendTrade(origOwner, dstPort.owner())`
                let dst_owner = cx.st.unit(self.dst_port).owner();
                cx.st.stats_cx().boat_send_trade(self.orig_owner, dst_owner);
                t
            }
        };

        // TS: :71-74
        if !cx.st.unit(trade_ship).is_active() {
            self.active = false;
            return;
        }

        // TS: :76-90 (displayMessage dropped)
        let trade_ship_owner = cx.st.unit(trade_ship).owner();
        let dst_port_owner = cx.st.unit(self.dst_port).owner();
        if !self.was_captured && self.orig_owner != trade_ship_owner {
            // Store as variable in case the ship is recaptured by its previous owner.
            self.was_captured = true;
        }

        // TS: :92-98 `dstPortOwner.id() === this.srcPort.owner().id()`: a player captured
        // the other port while trading (ids ↔ smallIDs are a bijection).
        if dst_port_owner == cx.st.unit(self.src_port).owner() {
            cx.unit_mut(trade_ship).delete(Some(false), None);
            self.active = false;
            return;
        }

        // TS: :100-107
        if !self.was_captured
            && (!cx.st.unit(self.dst_port).is_active() || !cx.st.player(trade_ship_owner).can_trade(dst_port_owner))
        {
            cx.unit_mut(trade_ship).delete(Some(false), None);
            self.active = false;
            return;
        }

        // TS: :109
        let cur_tile = cx.st.unit(trade_ship).tile();

        // TS: :111-136
        if self.was_captured && (trade_ship_owner != dst_port_owner || !cx.st.unit(self.dst_port).is_active()) {
            // TS: :115 `mg.getWaterComponent(curTile)`
            let my_component = cx.rcx().get_water_component(cur_tile);
            // TS: :116-125 `findClosestBy(tradeShipOwner.units(Port), manhattan(port, curTile),
            //   isActive && !isMarkedForDeletion && !isUnderConstruction && myComponent !== null
            //   && hasWaterComponent(port.tile(), myComponent))`
            let ports = cx.st.player(trade_ship_owner).units_of(&[UnitType::Port]);
            let nearest = {
                let st: &crate::state::State = &*cx.st;
                let mut rcx = crate::ctx::Rcx::new(st, &mut *cx.sc);
                find_closest_by(
                    &ports,
                    |&port| f64::from(st.map.manhattan_dist(st.unit(port).tile(), cur_tile)),
                    Some(|&port: &UnitRef| {
                        let v = st.unit(port);
                        v.is_active()
                            && !v.is_marked_for_deletion()
                            && !v.is_under_construction()
                            && match my_component {
                                Some(c) => rcx.has_water_component(v.tile(), c),
                                None => false,
                            }
                    }),
                )
            };
            match nearest {
                // TS: :126-129
                None => {
                    cx.unit_mut(trade_ship).delete(Some(false), None);
                    self.active = false;
                    return;
                }
                // TS: :130-135
                Some(i) => {
                    // in-range: find_closest_by returns an index into `ports`.
                    self.dst_port = ports[i];
                    cx.unit_mut(trade_ship).set_target_unit(Some(self.dst_port));
                    // Plan-driven units emit no per-tick unit updates, so TS forces a sync:
                    // `tradeShip.touch()` (the isInCombat shim, ARCHITECTURE §5; never dropped).
                    // touch-site: TradeShipExecution.ts:134
                    cx.st.unit_touched(trade_ship);
                }
            }
        }

        // TS: :138-141 `if (curTile === this.dstPort())`
        if cur_tile == self.dst_port_tile(cx) {
            self.complete(cx, trade_ship);
            return;
        }

        // TS: :143-144
        let dst = cx.st.unit(self.dst_port).tile();
        let result = self.pf().next(cx, cur_tile, dst, None);

        // TS: :146-180
        match result {
            PathResult::Next(node) => {
                // TS: :148-162
                if Some(dst) != self.motion_plan_dst {
                    self.motion_plan_id += 1;
                    // TS: :151 `pathForTraversal(from, dst)`: only the plan record (view)
                    // reads the path, but the call can run `findPath` (memo writes), so it
                    // is kept.
                    let _path = self.pf().path_for_traversal(cx, node, dst);
                    // TS: :153-160 `recordMotionPlan({kind: "grid", unitId, ...})`: the
                    // `planDrivenUnitIds` write is kept (§5).
                    let unit_id = cx.st.unit(trade_ship).id();
                    cx.record_motion_plan(unit_id);
                    self.motion_plan_dst = Some(dst);
                }
                // TS: :163-166 update safeFromPirates
                if cx.st.map.is_water(node) && cx.st.map.is_shoreline(node) {
                    cx.unit_mut(trade_ship).set_safe_from_pirates();
                }
                // TS: :167-168
                cx.unit_mut(trade_ship).move_to(node);
                self.tiles_traveled += 1;
            }
            // TS: :170-172
            PathResult::Complete(_) => {
                self.complete(cx, trade_ship);
            }
            // TS: :173-179 (warn dropped)
            PathResult::NotFound => {
                if cx.st.unit(trade_ship).is_active() {
                    cx.unit_mut(trade_ship).delete(Some(false), None);
                }
                self.active = false;
            }
        }
    }
}

// ---- P7-private CS helpers for nested records (module docs) ----

/// `zRandom()`: the four sfc32 words `(int, int, int, int)` (`PseudoRandom.getState`).
pub(crate) fn prng_canon(r: &Prng) -> Cs {
    Cs::Arr(r.get_state().iter().map(|&w| Cs::Int(i64::from(w))).collect())
}

/// `PathFinderStepper.getState()`: `{ path: tiles|null, pathIndex: int, lastTo: uint|null }`
/// (`snapshot/PathfinderSnapshots.ts`).
pub(crate) fn stepper_canon(s: &StepperState) -> Cs {
    Cs::Obj(vec![
        ("path".into(), s.path.as_ref().map_or(Cs::Null, |p| Cs::Typed(of_canon::Typed::U32(p.clone())))),
        ("pathIndex".into(), Cs::Int(i64::from(s.path_index))),
        ("lastTo".into(), s.last_to.map_or(Cs::Null, |t| Cs::Int(i64::from(t)))),
    ])
}

/// `waterPathFinderState(pf)`: `{ stagger, memoized, waterGraphVersion, rebuilt,
/// staggerCountdown, pendingVersion, stepper }` (`chain_version` is not exported).
pub(crate) fn water_path_finder_canon(pf: &WaterPathFinder) -> Cs {
    Cs::Obj(vec![
        ("stagger".into(), Cs::Int(i64::from(pf.stagger))),
        ("memoized".into(), Cs::Bool(pf.memoized)),
        ("waterGraphVersion".into(), Cs::Int(i64::from(pf.water_graph_version))),
        ("rebuilt".into(), Cs::Bool(pf.rebuilt)),
        ("staggerCountdown".into(), Cs::Int(i64::from(pf.stagger_countdown))),
        ("pendingVersion".into(), Cs::Int(pf.pending_version)),
        ("stepper".into(), stepper_canon(&pf.stepper)),
    ])
}

/// `TradeShip v1`: `{ active, initialized, tradeShip|null, wasCaptured, pathFinder|null,
/// tilesTraveled, motionPlanId, motionPlanDst|null, origOwner, srcPort, dstPort }`.
impl Canon for TradeShipExec {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        Cs::Obj(vec![
            ("active".into(), Cs::Bool(self.active)),
            ("initialized".into(), Cs::Bool(self.initialized)),
            ("tradeShip".into(), self.trade_ship.canon(cx)),
            ("wasCaptured".into(), Cs::Bool(self.was_captured)),
            ("pathFinder".into(), self.path_finder.as_ref().map_or(Cs::Null, water_path_finder_canon)),
            ("tilesTraveled".into(), Cs::Int(i64::from(self.tiles_traveled))),
            ("motionPlanId".into(), Cs::Int(i64::from(self.motion_plan_id))),
            ("motionPlanDst".into(), self.motion_plan_dst.map_or(Cs::Null, |t| Cs::Int(i64::from(t)))),
            ("origOwner".into(), self.orig_owner.canon(cx)),
            ("srcPort".into(), self.src_port.canon(cx)),
            ("dstPort".into(), self.dst_port.canon(cx)),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_record_helpers_follow_the_dump_shapes() {
        let r = Prng::new(7.0);
        let words = r.get_state();
        assert_eq!(prng_canon(&r), Cs::Arr(words.iter().map(|&w| Cs::Int(i64::from(w))).collect()));
        let s = StepperState { path: Some(vec![3, 4]), path_index: 1, last_to: None };
        let c = stepper_canon(&s);
        assert_eq!(c.get("path"), Some(&Cs::Typed(of_canon::Typed::U32(vec![3, 4]))));
        assert_eq!(c.get("pathIndex"), Some(&Cs::Int(1)));
        assert_eq!(c.get("lastTo"), Some(&Cs::Null));
        let pf = WaterPathFinder { pending_version: -1, ..WaterPathFinder::default() };
        let w = water_path_finder_canon(&pf);
        let keys: Vec<&str> = match &w {
            Cs::Obj(f) => f.iter().map(|(k, _)| k.as_str()).collect(),
            _ => vec![],
        };
        assert_eq!(
            keys,
            ["stagger", "memoized", "waterGraphVersion", "rebuilt", "staggerCountdown", "pendingVersion", "stepper"]
        );
        assert_eq!(w.get("pendingVersion"), Some(&Cs::Int(-1)));
    }
}
