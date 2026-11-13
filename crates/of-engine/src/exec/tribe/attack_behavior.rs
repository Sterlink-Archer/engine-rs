//! `TribeAttackBehavior extends AiAttackBehavior` (`engine/src/execution/TribeAttackBehavior.ts`).
//! Owner: **P9**. No overrides: `impl AiAttackHooks for TribeExec` uses every default.
//! Snapshot: the `AiAttackBehavior v2` record with the nation-only fields empty.

use crate::StateClone;
use crate::api::types::PlayerType;
use crate::canon::{Canon, CanonCx, CanonError, Cs, Uncanon, UncanonCx};
use crate::ctx::{Ctx, TsExpect};
use crate::exec::ai_attack::{self, AiAttackBase, AiAttackHooks};
use crate::exec::tribe::TribeExec;
use crate::ids::SmallId;
use of_det::Prng;
use of_det::ts_throw;

/// `TribeAttackBehavior` data (only the base).
#[derive(StateClone, Debug, Default)]
pub struct TribeAttackB {
    pub base: AiAttackBase,
}

/// `snapshot(w)` = `writeRecord(w, 0, [])` (`TribeAttackBehavior.ts:10-12`).
impl Canon for TribeAttackB {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        self.base.canon_record(cx, 0.0, Cs::Arr(vec![]))
    }
}

/// `restoreSnapshot` = `restoreRecord` (`:15-22`): only the ratios are kept.
impl Uncanon for TribeAttackB {
    fn uncanon(v: &Cs, cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        AiAttackBase::uncanon_record(v, cx).map(|(base, _, _)| TribeAttackB { base })
    }
}

impl AiAttackHooks for TribeExec {
    fn ai(&mut self) -> &mut AiAttackBase {
        // The behaviour exists whenever base code runs: it is created before the first
        // `sendAttack` (`TribeExecution.ts:65-77`).
        &mut self.attack_behavior.as_mut().ts_expect("TribeExecution.attackBehavior").base
    }
    fn rng(&mut self) -> &mut Prng {
        &mut self.rng
    }
    fn player(&self) -> SmallId {
        self.tribe
    }
}

impl TribeExec {
    /// `attackBehavior.attackRandomTarget()` (`TribeAttackBehavior.ts:26-60`).
    pub fn attack_random_target(&mut self, cx: &mut Ctx) {
        // TS: :28 save up troops until we reach the trigger ratio
        if !ai_attack::has_trigger_ratio_troops(self, cx, false) {
            return;
        }
        // TS: :31-34 retaliate against incoming attacks
        if let Some(incoming) = ai_attack::find_incoming_attack_player(self, cx)
            && ai_attack::send_attack(self, cx, incoming, true)
        {
            return;
        }
        // TS: :37-42 select a traitor as an enemy
        if let Some(to_attack) = self.get_neighbor_traitor_to_attack(cx, false)
            && self.rng.chance(3.0)
            && ai_attack::send_attack(self, cx, to_attack, false)
        {
            return;
        }
        // TS: :45-59 choose a new enemy randomly (`shuffleArray` copies `nearby()`)
        let neighbors = cx.st.nearby_memo(self.tribe);
        for neighbor in self.rng.shuffle_array(&neighbors) {
            if !neighbor.is_player() {
                continue;
            }
            if cx.st.player(self.tribe).is_friendly(neighbor, false) {
                continue;
            }
            let ty = cx.st.player(neighbor).player_type();
            if (ty == PlayerType::Nation || ty == PlayerType::Human) && self.rng.chance(2.0) {
                continue;
            }
            if ai_attack::send_attack(self, cx, neighbor, false) {
                return;
            }
        }
    }

    /// `attackBehavior.getNeighborTraitorToAttack(includeAllies = false)` (`:62-75`).
    pub fn get_neighbor_traitor_to_attack(&mut self, cx: &mut Ctx, include_allies: bool) -> Option<SmallId> {
        if cx.st.config().disable_alliances() {
            return None;
        }
        // TS: :65-72 `nearby().filter(n => n.isPlayer() && (includeAllies || !isFriendly(n)) && n.isTraitor())`
        let near = cx.st.nearby_memo(self.tribe);
        let me = cx.st.player(self.tribe);
        let traitors: Vec<SmallId> = near
            .into_iter()
            .filter(|&n| n.is_player() && (include_allies || !me.is_friendly(n, false)) && cx.st.player(n).is_traitor())
            .collect();
        if traitors.is_empty() {
            return None;
        }
        // TS: :73 `this.random.randElement(traitors)`
        match self.rng.rand_element(&traitors) {
            Ok(&t) => Some(t),
            Err(_) => ts_throw!("array must not be empty"),
        }
    }
}
