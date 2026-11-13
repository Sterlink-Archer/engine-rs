//! `MarkDisconnectedExecution` (`engine/src/execution/MarkDisconnectedExecution.ts`). Owner: **P9**.
//!
//! Fields: the `MarkDisconnected v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1).

#[allow(unused_imports)]
use crate::exec::prelude::*;

use crate::canon::{Canon, CanonCx, CanonError, Cs, Uncanon, UncanonCx};
use crate::exec::ai_attack::p9cs;
use crate::state::player::DisconnectSnapshot;

/// `MarkDisconnectedExecution` data.
#[derive(StateClone, Debug)]
pub struct MarkDisconnectedExec {
    pub player: SmallId,
    pub is_disconnected: bool,
}

impl MarkDisconnectedExec {
    /// `new MarkDisconnectedExecution(player, isDisconnected)` (`:12-15`).
    pub fn new(player: SmallId, is_disconnected: bool) -> Self {
        MarkDisconnectedExec { player, is_disconnected }
    }
}

impl ExecBehavior for MarkDisconnectedExec {
    /// TS: `:37-39` `return false`.
    fn is_active(&self) -> bool {
        false
    }
    /// `activeDuringSpawnPhase()` returns `false` (`MarkDisconnectedExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    /// TS: `:17-31`.
    fn init(&mut self, cx: &mut Ctx, ticks: i32) {
        if self.is_disconnected {
            // TS: :20 `team ? mg.teamTilesOwned(team) : 0` (a JS truthiness test: "" is falsy)
            let team_tiles = match cx.st.player(self.player).team() {
                Some(team) if !team.is_empty() => cx.st.team_tiles_owned(team),
                _ => 0,
            };
            let total_land = cx.st.total_land_tiles();
            let was_alive = cx.st.player(self.player).is_alive();
            cx.player_mut(self.player).mark_disconnected(
                true,
                Some(DisconnectSnapshot { current_tick: ticks, team_tiles, total_land, was_alive }),
            );
        } else {
            cx.player_mut(self.player).mark_disconnected(false, None);
        }
    }
    /// TS: `:33-35` no-op.
    fn tick(&mut self, cx: &mut Ctx, ticks: i32) {
        let _ = (cx, ticks);
    }
}

/// `MarkDisconnectedExecutionSnapshot`, `MarkDisconnected v1 (execution)` field order.
impl Canon for MarkDisconnectedExec {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        Cs::Obj(vec![
            ("player".into(), self.player.canon(cx)),
            ("isDisconnected".into(), self.is_disconnected.canon(cx)),
        ])
    }
}

impl Uncanon for MarkDisconnectedExec {
    fn uncanon(v: &Cs, _cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        Ok(MarkDisconnectedExec {
            player: p9cs::small_id(p9cs::field(v, "player")?)?,
            is_disconnected: p9cs::bool_(p9cs::field(v, "isDisconnected")?)?,
        })
    }
}
