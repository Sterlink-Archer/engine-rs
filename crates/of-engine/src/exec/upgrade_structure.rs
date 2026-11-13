//! `UpgradeStructureExecution` (`engine/src/execution/UpgradeStructureExecution.ts`). Owner: **P7**.
//!
//! Fields: the `UpgradeStructure v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`), then non-snapshot fields. `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1).

use crate::canon::{Canon, CanonCx, Cs};
use crate::exec::prelude::*;
use crate::state::State;

/// `UpgradeStructureExecution` data.
#[derive(StateClone, Debug)]
pub struct UpgradeStructureExec {
    pub structure: Option<UnitRef>,
    pub player: SmallId,
    /// `unitId` (snapshot `num`)
    pub unit_id: f64,
    pub amount: f64,
    /// non-snapshot: `cost: bigint` (`:17`), declared but never assigned upstream.
    pub cost: Gold,
}

impl UpgradeStructureExec {
    /// `new UpgradeStructureExecution(player, unitId, amount = 1)`.
    pub fn new(player: SmallId, unit_id: f64, amount: f64) -> Self {
        UpgradeStructureExec { structure: None, player, unit_id, amount, cost: 0 }
    }
}

/// `mg.unit(id)` for a JS number id (`GameImpl.ts:345-347`, `_unitMap.get(id)`): Map keys
/// compare by SameValueZero, so only a non-negative integer below 2^32 (unit ids are `u32`)
/// can match; `-0` matches 0.
pub(crate) fn unit_by_num(st: &State, id: f64) -> Option<UnitRef> {
    if id.is_finite() && id.trunc() == id && (0.0..=f64::from(u32::MAX)).contains(&id) {
        // in-range: checked integral and within u32 just above.
        st.unit_by_id(id as u32)
    } else {
        None
    }
}

impl ExecBehavior for UpgradeStructureExec {
    /// TS: :55-57 `return false`.
    fn is_active(&self) -> bool {
        false
    }
    /// `activeDuringSpawnPhase()` returns `false` (`UpgradeStructureExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    fn init(&mut self, cx: &mut Ctx, _ticks: i32) {
        // TS: :26-30 (warn dropped)
        self.structure = unit_by_num(cx.st, self.unit_id);
        if let Some(s) = self.structure
            && cx.st.unit(s).owner() != self.player
        {
            self.structure = None;
        }

        // TS: :32-35
        let Some(structure) = self.structure else { return };

        // TS: :37-47 `for (let i = 0; i < this.amount; i++)`: `amount` is a JS number (it may
        // be fractional or Infinity), so the counter compares as f64.
        let mut i = 0.0;
        while i < self.amount {
            if !cx.st.player(self.player).can_upgrade_unit(structure) {
                break;
            }
            cx.player_mut(self.player).upgrade_unit(structure);
            i += 1.0;
        }
    }
    /// TS: :51-53.
    fn tick(&mut self, _cx: &mut Ctx, _ticks: i32) {}
}

/// `UpgradeStructure v1`: `{ structure|null, player, unitId: num, amount: num }` (`cost` is
/// never assigned, so not stored).
impl Canon for UpgradeStructureExec {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        crate::canon_obj!(cx;
            "structure" => self.structure,
            "player" => self.player,
            "unitId" => self.unit_id,
            "amount" => self.amount,
        )
    }
}
