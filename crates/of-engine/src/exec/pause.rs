//! `PauseExecution` (`engine/src/execution/PauseExecution.ts`). Owner: **P9**.
//!
//! Fields: the `Pause v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1).

#[allow(unused_imports)]
use crate::exec::prelude::*;

use crate::api::types::GameType;
use crate::canon::{Canon, CanonCx, CanonError, Cs, Uncanon, UncanonCx};
use crate::exec::ai_attack::p9cs;

/// `PauseExecution` data.
#[derive(StateClone, Debug)]
pub struct PauseExec {
    pub player: SmallId,
    pub paused: bool,
}

impl PauseExec {
    /// `new PauseExecution(player, paused)` (`:13-16`).
    pub fn new(player: SmallId, paused: bool) -> Self {
        PauseExec { player, paused }
    }
}

impl ExecBehavior for PauseExec {
    /// TS: `:18-20` `return false`.
    fn is_active(&self) -> bool {
        false
    }
    /// `activeDuringSpawnPhase()` returns `true` (`PauseExecution.ts:22-24`).
    fn active_during_spawn_phase(&self) -> bool {
        true
    }
    /// TS: `:26-33` (short-circuit `||`).
    fn init(&mut self, cx: &mut Ctx, ticks: i32) {
        let _ = ticks;
        if cx.st.player(self.player).is_lobby_creator()
            || cx.st.config().game_config().game_type == GameType::Singleplayer
        {
            cx.set_paused(self.paused);
        }
    }
    /// TS: `:35` empty.
    fn tick(&mut self, cx: &mut Ctx, ticks: i32) {
        let _ = (cx, ticks);
    }
}

/// `PauseExecutionSnapshot`, `Pause v1 (execution)` field order.
impl Canon for PauseExec {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        Cs::Obj(vec![("player".into(), self.player.canon(cx)), ("paused".into(), self.paused.canon(cx))])
    }
}

impl Uncanon for PauseExec {
    fn uncanon(v: &Cs, _cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        Ok(PauseExec {
            player: p9cs::small_id(p9cs::field(v, "player")?)?,
            paused: p9cs::bool_(p9cs::field(v, "paused")?)?,
        })
    }
}
