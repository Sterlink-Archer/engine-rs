//! `BoatRetreatExecution` (`engine/src/execution/BoatRetreatExecution.ts`). Owner: **P7**.
//!
//! Fields: the `BoatRetreat v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1).

use crate::canon::{Canon, CanonCx, Cs};
use crate::exec::prelude::*;
use crate::state::unit::TransportShipStateUpdate;

/// `BoatRetreatExecution` data.
#[derive(StateClone, Debug)]
pub struct BoatRetreatExec {
    pub active: bool,
    pub player: SmallId,
    pub unit_id: u32,
}

impl BoatRetreatExec {
    /// `new BoatRetreatExecution(player, unitID)`.
    pub fn new(player: SmallId, unit_id: u32) -> Self {
        // TS: :13 `private active = true`
        BoatRetreatExec { active: true, player, unit_id }
    }
}

impl ExecBehavior for BoatRetreatExec {
    fn is_active(&self) -> bool {
        self.active
    }
    /// `activeDuringSpawnPhase()` returns `false` (`BoatRetreatExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    /// TS: :19 `init(mg, ticks): void {}`.
    fn init(&mut self, _cx: &mut Ctx, _ticks: i32) {}
    fn tick(&mut self, cx: &mut Ctx, _ticks: i32) {
        // TS: :22-27 `this.player.units().find(u => u.id() === unitID && u.type() === TransportShip)`
        // over the live array; `find` does not mutate it.
        let unit = {
            let st = &*cx.st;
            st.player(self.player).units().as_slice().iter().copied().find(|&u| {
                let v = st.unit(u);
                v.id() == self.unit_id && v.unit_type() == UnitType::TransportShip
            })
        };

        // TS: :29-33 (console.warn dropped)
        let Some(unit) = unit else {
            self.active = false;
            return;
        };

        // TS: :35-36
        cx.unit_mut(unit)
            .update_transport_ship_state(TransportShipStateUpdate { is_retreating: Some(true), troops: None });
        self.active = false;
    }
}

/// `BoatRetreat v1`: `{ active, player, unitID }`.
impl Canon for BoatRetreatExec {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        crate::canon_obj!(cx; "active" => self.active, "player" => self.player, "unitID" => self.unit_id)
    }
}
