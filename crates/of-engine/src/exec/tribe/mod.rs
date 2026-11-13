//! `TribeExecution` (`engine/src/execution/TribeExecution.ts`). Owner: **P9**.
//!
//! Fields: the `Tribe v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1).
//!
//! The PRNG is drawn five times in the **constructor** (`:36-41`), and the same `Prng` is
//! later shared with the attack behaviour (`:66-73`; `AiAttackHooks::rng` returns
//! `&mut self.rng`).

#[allow(unused_imports)]
use crate::exec::prelude::*;

use of_det::ts_throw;

use crate::api::types::unit_groups::STRUCTURES;
use crate::canon::{Canon, CanonCx, CanonError, Cs, Uncanon, UncanonCx};
use crate::exec::ai_attack::{self, AiAttackBase, p9cs};
use crate::exec::alliance::extension::AllianceExtensionExec;
use crate::exec::delete_unit::DeleteUnitExec;
use of_det::util::simple_hash;

pub mod attack_behavior;

pub use attack_behavior::TribeAttackB;

/// `TribeExecution` data.
#[derive(StateClone, Debug)]
pub struct TribeExec {
    pub active: bool,
    pub tribe: SmallId,
    /// `PseudoRandom(simpleHash(tribe.id()))` in the constructor (`:36`)
    pub rng: Prng,
    pub initialized: bool,
    pub neighbors_terra_nullius: bool,
    pub attack_behavior: Option<TribeAttackB>,
    pub attack_rate: f64,
    pub attack_tick: f64,
    pub trigger_ratio: f64,
    pub reserve_ratio: f64,
    pub expand_ratio: f64,
}

impl TribeExec {
    /// `new TribeExecution(tribe)` (`:35-42`); `tribe_id` is `tribe.id()` (PRNG seed).
    pub fn new(tribe: SmallId, tribe_id: &str) -> Self {
        // TS: :36 `new PseudoRandom(simpleHash(tribe.id()))` (seed `| 0` inside Prng::new)
        let mut rng = Prng::new(simple_hash(tribe_id));
        // TS: :37-41, in this order
        let attack_rate = rng.next_int(40.0, 80.0);
        let attack_tick = rng.next_int(0.0, attack_rate);
        let trigger_ratio = rng.next_int(50.0, 60.0) / 100.0;
        let reserve_ratio = rng.next_int(30.0, 40.0) / 100.0;
        let expand_ratio = rng.next_int(10.0, 20.0) / 100.0;
        TribeExec {
            active: true, // TS: :23
            tribe,
            rng,
            initialized: false,            // TS: `mg` unset until init
            neighbors_terra_nullius: true, // TS: :26
            attack_behavior: None,         // TS: :28
            attack_rate,
            attack_tick,
            trigger_ratio,
            reserve_ratio,
            expand_ratio,
        }
    }

    /// `acceptAllAllianceRequests()` (`:92-109`).
    fn accept_all_alliance_requests(&mut self, cx: &mut Ctx) {
        // TS: :94 `incomingAllianceRequests()` is a fresh `filter` copy
        for req in cx.st.player(self.tribe).incoming_alliance_requests() {
            cx.accept_alliance_request(req);
        }
        // TS: :99 `for (const alliance of this.tribe.alliances())` over the live array.
        // iter-live: the body only reads and calls addExecution (inbox), which cannot remove
        // from `alliances`; the length is re-read per step like a JS array iterator.
        let mut i = 0usize;
        while let Some(alliance) = cx.st.player(self.tribe).alliances().get(i) {
            i += 1;
            if !cx.st.alliance(alliance).only_one_agreed_to_extend() {
                continue;
            }
            let human = cx.st.alliance(alliance).other(self.tribe);
            let human_id = cx.st.player(human).player_id().to_string();
            cx.st.add_execution(Exec::AllianceExtension(AllianceExtensionExec::new(self.tribe, human_id)));
        }
    }

    /// `deleteNextStructure()` (`:111-119`).
    fn delete_next_structure(&mut self, cx: &mut Ctx) {
        if !cx.st.player(self.tribe).can_delete_unit() {
            return;
        }
        // TS: :113 `for (const unit of this.tribe.units())` over the live array.
        // iter-live: the body only reads, then adds one execution and returns.
        let mut i = 0usize;
        while let Some(unit) = cx.st.player(self.tribe).units().get(i) {
            i += 1;
            if !STRUCTURES.contains(&cx.st.unit(unit).unit_type()) {
                continue;
            }
            if cx.st.unit(unit).is_marked_for_deletion() {
                continue;
            }
            let id = cx.st.unit(unit).id();
            cx.st.add_execution(Exec::DeleteUnit(DeleteUnitExec::new(self.tribe, id)));
            return;
        }
    }

    /// `maybeAttack()` (`:121-149`).
    fn maybe_attack(&mut self, cx: &mut Ctx) {
        if self.attack_behavior.is_none() {
            ts_throw!("not initialized");
        }
        // TS: :125 `getNeighborTraitorToAttack(true)`
        if let Some(to_attack) = self.get_neighbor_traitor_to_attack(cx, true) {
            let odds = if cx.st.player(self.tribe).is_friendly(to_attack, false) { 6.0 } else { 3.0 };
            if self.rng.chance(odds) {
                // TS: :130-134 break the alliance before attacking
                if let Some(alliance) = cx.st.player(self.tribe).alliance_with(to_attack) {
                    cx.player_mut(self.tribe).break_alliance(alliance);
                }
                if ai_attack::send_attack(self, cx, to_attack, false) {
                    return;
                }
            }
        }

        if self.neighbors_terra_nullius {
            // TS: :141 `this.tribe.nearby().some((n) => !n.isPlayer())`
            if cx.st.nearby_memo(self.tribe).iter().any(|n| !n.is_player()) {
                if ai_attack::send_attack(self, cx, SmallId::TERRA_NULLIUS, false) {
                    return;
                }
            } else {
                self.neighbors_terra_nullius = false;
            }
        }

        self.attack_random_target(cx);
    }
}

impl ExecBehavior for TribeExec {
    fn is_active(&self) -> bool {
        self.active
    }
    /// `activeDuringSpawnPhase()` returns `false` (`TribeExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    fn init(&mut self, cx: &mut Ctx, ticks: i32) {
        let _ = (cx, ticks);
        // TS: :52-54 `this.mg = mg`
        self.initialized = true;
    }
    fn tick(&mut self, cx: &mut Ctx, ticks: i32) {
        // TS: :57 `if (ticks % this.attackRate !== this.attackTick) return` (f64 `%` = JS `%`)
        if f64::from(ticks) % self.attack_rate != self.attack_tick {
            return;
        }
        if !cx.st.player(self.tribe).is_alive() {
            // TS: :59-63 removeOnDeath is called from the tribe's PlayerExecution
            self.active = false;
            return;
        }
        if self.attack_behavior.is_none() {
            // TS: :66-73 (the behaviour shares `this.random`: `AiAttackHooks::rng`)
            self.attack_behavior = Some(TribeAttackB {
                base: AiAttackBase::new(self.trigger_ratio, self.reserve_ratio, self.expand_ratio),
            });
            // TS: :76 send an attack on the first tick
            ai_attack::send_attack(self, cx, SmallId::TERRA_NULLIUS, false);
            return;
        }
        // TS: :82-85 the tutorial's tribe only grows into free land
        if cx.st.config().game_config().tutorial == Some(true) {
            ai_attack::expand_into_free_land(self, cx);
            return;
        }
        self.accept_all_alliance_requests(cx);
        self.delete_next_structure(cx);
        self.maybe_attack(cx);
    }
}

/// `TribeExecutionSnapshot` (`:155-169`), `Tribe v1 (execution)` field order.
impl Canon for TribeExec {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        let ab = match &self.attack_behavior {
            Some(b) => b.canon(cx),
            None => Cs::Null,
        };
        Cs::Obj(vec![
            ("active".into(), self.active.canon(cx)),
            ("tribe".into(), self.tribe.canon(cx)),
            ("random".into(), p9cs::prng(&self.rng)),
            ("initialized".into(), self.initialized.canon(cx)),
            ("neighborsTerraNullius".into(), self.neighbors_terra_nullius.canon(cx)),
            ("attackBehavior".into(), ab),
            ("attackRate".into(), self.attack_rate.canon(cx)),
            ("attackTick".into(), self.attack_tick.canon(cx)),
            ("triggerRatio".into(), self.trigger_ratio.canon(cx)),
            ("reserveRatio".into(), self.reserve_ratio.canon(cx)),
            ("expandRatio".into(), self.expand_ratio.canon(cx)),
        ])
    }
}

/// `restoreSnapshot` (`:171-192`).
impl Uncanon for TribeExec {
    fn uncanon(v: &Cs, cx: &mut UncanonCx<'_>) -> Result<Self, CanonError> {
        let f = |k| p9cs::field(v, k);
        let ab = f("attackBehavior")?;
        Ok(TribeExec {
            active: p9cs::bool_(f("active")?)?,
            tribe: p9cs::small_id(f("tribe")?)?,
            rng: p9cs::prng_from(f("random")?)?,
            initialized: p9cs::bool_(f("initialized")?)?,
            neighbors_terra_nullius: p9cs::bool_(f("neighborsTerraNullius")?)?,
            attack_behavior: if ab.is_null() { None } else { Some(TribeAttackB::uncanon(ab, cx)?) },
            attack_rate: p9cs::num(f("attackRate")?)?,
            attack_tick: p9cs::num(f("attackTick")?)?,
            trigger_ratio: p9cs::num(f("triggerRatio")?)?,
            reserve_ratio: p9cs::num(f("reserveRatio")?)?,
            expand_ratio: p9cs::num(f("expandRatio")?)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::Scratch;
    use crate::state::State;

    /// Constructor draws (`:36-41`): five `nextInt`s from `simpleHash(id)`, in order.
    #[test]
    fn constructor_draw_order() {
        let t = TribeExec::new(SmallId(5), "abcd1234");
        let mut r = Prng::new(simple_hash("abcd1234"));
        let rate = r.next_int(40.0, 80.0);
        assert_eq!(t.attack_rate, rate);
        assert_eq!(t.attack_tick, r.next_int(0.0, rate));
        assert_eq!(t.trigger_ratio, r.next_int(50.0, 60.0) / 100.0);
        assert_eq!(t.reserve_ratio, r.next_int(30.0, 40.0) / 100.0);
        assert_eq!(t.expand_ratio, r.next_int(10.0, 20.0) / 100.0);
        assert_eq!(t.rng.get_state(), r.get_state());
        assert!(t.active && !t.initialized && t.neighbors_terra_nullius && t.attack_behavior.is_none());
        assert!((40.0..80.0).contains(&t.attack_rate) && (0.0..t.attack_rate).contains(&t.attack_tick));
    }

    /// Off-phase ticks return before any state read (`:57`), so they run without P2.
    #[test]
    fn off_phase_tick_is_a_noop() {
        let mut st = State::default();
        let mut sc = Scratch::new();
        let mut cx = Ctx::new(&mut st, &mut sc, None);
        let mut t = TribeExec::new(SmallId(5), "abcd1234");
        t.init(&mut cx, 0);
        assert!(t.initialized);
        let before = t.rng.get_state();
        let off = (t.attack_tick as i32 + 1) % t.attack_rate as i32;
        t.tick(&mut cx, off);
        assert_eq!(t.rng.get_state(), before);
        assert!(t.active && t.attack_behavior.is_none());
    }

    #[test]
    fn canon_round_trip() {
        let st = State::default();
        let ex = crate::exec::ExecStore::default();
        let mut t = TribeExec::new(SmallId(5), "abcd1234");
        let mut ccx = CanonCx::new(&st, &ex);
        let a = t.canon(&mut ccx);
        t.attack_behavior =
            Some(TribeAttackB { base: AiAttackBase::new(t.trigger_ratio, t.reserve_ratio, t.expand_ratio) });
        let b = t.canon(&mut ccx);
        let mut st2 = State::default();
        let mut ucx = UncanonCx { st: &mut st2, keys: &crate::canon::LoadKeys::default() };
        for cs in [a, b] {
            let back = TribeExec::uncanon(&cs, &mut ucx).unwrap_or_else(|e| panic!("{e:?}"));
            assert_eq!(back.canon(&mut ccx), cs);
        }
    }
}
