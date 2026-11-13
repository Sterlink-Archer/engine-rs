//! `EmbargoAllExecution` (`engine/src/execution/EmbargoAllExecution.ts`). Owner: **P9**.
//!
//! Fields: the `EmbargoAll v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1).

#[allow(unused_imports)]
use crate::exec::prelude::*;

use crate::canon::{Canon, CanonCx, CanonError, Cs, Uncanon, UncanonCx};
use crate::exec::ai_attack::p9cs;

/// `EmbargoAllExecution` data.
#[derive(StateClone, Debug)]
pub struct EmbargoAllExec {
    pub player: SmallId,
    pub action: EmbargoAction,
}

impl EmbargoAllExec {
    /// `new EmbargoAllExecution(player, action)` (`:13-16`).
    pub fn new(player: SmallId, action: EmbargoAction) -> Self {
        EmbargoAllExec { player, action }
    }
}

impl ExecBehavior for EmbargoAllExec {
    /// TS: `:41-43` `return false`.
    fn is_active(&self) -> bool {
        false
    }
    /// `activeDuringSpawnPhase()` returns `false` (`EmbargoAllExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    /// TS: `:18-37`.
    fn init(&mut self, cx: &mut Ctx, ticks: i32) {
        let _ = ticks;
        if !cx.st.player(self.player).can_embargo_all() {
            return;
        }
        let me = self.player;
        // TS: :23 `mg.players()` is a fresh array (alive players, `_players` order)
        for p in cx.st.players() {
            // TS: :24 `p.id() === me.id()` (player ids and smallIDs are a bijection)
            if p == me {
                continue;
            }
            if cx.st.player(p).player_type() == PlayerType::Bot {
                continue;
            }
            if cx.st.player(me).is_on_same_team(p) {
                continue;
            }
            if cx.st.player(me).is_allied_with(p) {
                continue;
            }
            match self.action {
                EmbargoAction::Start => {
                    if !cx.st.player(me).has_embargo_against(p) {
                        cx.player_mut(me).add_embargo(p, false);
                    }
                }
                EmbargoAction::Stop => {
                    if cx.st.player(me).has_embargo_against(p) {
                        cx.player_mut(me).stop_embargo(p);
                    }
                }
            }
        }
        cx.player_mut(self.player).record_embargo_all(); // TS: :36
    }
    /// TS: `:39` empty.
    fn tick(&mut self, cx: &mut Ctx, ticks: i32) {
        let _ = (cx, ticks);
    }
}

/// `EmbargoAllExecutionSnapshot`, `EmbargoAll v1 (execution)` field order.
impl Canon for EmbargoAllExec {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        Cs::Obj(vec![("player".into(), self.player.canon(cx)), ("action".into(), p9cs::embargo_action_cs(self.action))])
    }
}

impl Uncanon for EmbargoAllExec {
    fn uncanon(v: &Cs, _cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        Ok(EmbargoAllExec {
            player: p9cs::small_id(p9cs::field(v, "player")?)?,
            action: p9cs::embargo_action(p9cs::field(v, "action")?)?,
        })
    }
}
