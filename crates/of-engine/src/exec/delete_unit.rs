//! `DeleteUnitExecution` (`engine/src/execution/DeleteUnitExecution.ts`). Owner: **P7**.
//!
//! Fields: the `DeleteUnit v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1).

use crate::canon::{Canon, CanonCx, Cs};
use crate::exec::prelude::*;

/// `DeleteUnitExecution` data.
#[derive(StateClone, Debug)]
pub struct DeleteUnitExec {
    pub active: bool,
    /// `initialized: this.mg !== undefined` (set by `init` after the `active` check).
    pub initialized: bool,
    pub unit: Option<UnitRef>,
    pub player: SmallId,
    /// `unitId` (an intent value; `u32::MAX` when out of range, which matches no unit).
    pub unit_id: u32,
}

impl DeleteUnitExec {
    /// `new DeleteUnitExecution(player, unitId)`.
    pub fn new(player: SmallId, unit_id: u32) -> Self {
        // TS: :17-19 `active = true`, `unit = null`
        DeleteUnitExec { active: true, initialized: false, unit: None, player, unit_id }
    }
}

impl ExecBehavior for DeleteUnitExec {
    fn is_active(&self) -> bool {
        self.active
    }
    /// `activeDuringSpawnPhase()` returns `false` (`DeleteUnitExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    fn init(&mut self, cx: &mut Ctx, _ticks: i32) {
        // TS: :31-33
        if !self.active {
            return;
        }
        // TS: :34
        self.initialized = true;

        // TS: :36-43 `mg.unit(unitId)`; `!unit || unit.owner() !== this.player` (warns dropped)
        let Some(unit) = cx.st.unit_by_id(self.unit_id) else {
            self.active = false;
            return;
        };
        if cx.st.unit(unit).owner() != self.player {
            self.active = false;
            return;
        }

        // TS: :45-49
        if !cx.st.unit(unit).is_active() {
            self.active = false;
            return;
        }
        // TS: :50
        self.unit = Some(unit);

        // TS: :52-58 `!tileOwner.isPlayer() || tileOwner.id() !== this.player.id()` (player
        // ids ↔ smallIDs are a bijection, ARCHITECTURE §3.2)
        let tile = cx.st.unit(unit).tile();
        let tile_owner = cx.st.owner(tile);
        if !tile_owner.is_player() || tile_owner != self.player {
            self.active = false;
            return;
        }

        // TS: :60-64
        if !cx.st.map.is_land(tile) {
            self.active = false;
            return;
        }

        // TS: :66-70
        if cx.st.in_spawn_phase() {
            self.active = false;
            return;
        }

        // TS: :72-76
        if !cx.st.player(self.player).can_delete_unit() {
            self.active = false;
            return;
        }

        // TS: :78-79
        cx.player_mut(self.player).record_delete_unit();
        cx.unit_mut(unit).mark_for_deletion();
    }
    fn tick(&mut self, cx: &mut Ctx, _ticks: i32) {
        // TS: :83-85
        if !self.active {
            return;
        }
        let Some(unit) = self.unit else { return };
        // TS: :86-89
        if !cx.st.unit(unit).is_active() {
            self.active = false;
            return;
        }
        // TS: :90-99 (`displayMessage` dropped, view only)
        if cx.st.unit(unit).is_overdue_deletion() {
            cx.unit_mut(unit).delete(Some(false), None);
            self.active = false;
        }
    }
}

/// `DeleteUnit v1`: `{ active, initialized, unit|null, player, unitId }`.
impl Canon for DeleteUnitExec {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        crate::canon_obj!(cx;
            "active" => self.active,
            "initialized" => self.initialized,
            "unit" => self.unit,
            "player" => self.player,
            "unitId" => self.unit_id,
        )
    }
}
