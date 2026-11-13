//! `ShellExecution` (`engine/src/execution/ShellExecution.ts`). Owner: **P7**.
//!
//! Fields: the `Shell v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1).

use crate::canon::{Canon, CanonCx, Cs};
use crate::exec::prelude::*;
use crate::exec::trade_ship::{prng_canon, stepper_canon};
use crate::path::types::PathResult;
use crate::state::unit::UnitParams;

/// The `init` block of the Shell snapshot (`pathFinder`, `random`).
#[derive(StateClone, Debug)]
pub struct ShellInit {
    pub path_finder: AirStepper,
    /// `PseudoRandom(mg.ticks())` (`:45`).
    pub rng: Prng,
}

/// `ShellExecution` data.
#[derive(StateClone, Debug)]
pub struct ShellExec {
    pub active: bool,
    pub init: Option<ShellInit>,
    pub shell: Option<UnitRef>,
    /// `destroyAtTick = -1`
    pub destroy_at_tick: i32,
    pub spawn: TileRef,
    pub owner: SmallId,
    pub owner_unit: UnitRef,
    pub target: UnitRef,
}

impl ShellExec {
    /// `new ShellExecution(spawn, owner, ownerUnit, target)`.
    pub fn new(spawn: TileRef, owner: SmallId, owner_unit: UnitRef, target: UnitRef) -> Self {
        // TS: :28-32 `active = true`, `destroyAtTick = -1`
        ShellExec { active: true, init: None, shell: None, destroy_at_tick: -1, spawn, owner, owner_unit, target }
    }

    /// `effectOnTarget()` (`:97-115`).
    fn effect_on_target(&mut self, cx: &mut Ctx) -> f64 {
        // TS: :98-99 `const { damage } = config.unitInfo(Shell)`
        let damage = cx.st.config().unit_info(UnitType::Shell).damage;
        // TS: :101 `random.nextInt(1, 6)`
        let rng = &mut self.init.as_mut().ts_expect("ShellExecution.ts:101 this.random").rng;
        let roll = rng.next_int(1.0, 6.0);
        // TS: :106 `this.ownerUnit.veterancy()`
        let veterancy = cx.st.unit(self.owner_unit).veterancy();
        // TS: :108 (read only when veterancy > 0)
        let bonus_percent = if veterancy > 0 { cx.st.config().warship_veterancy_shell_damage_bonus() } else { 0.0 };
        shell_damage(damage, roll, veterancy, bonus_percent)
    }

    /// `getEffectOnTargetForTesting()` (`:117-119`).
    pub fn get_effect_on_target_for_testing(&mut self, cx: &mut Ctx) -> f64 {
        self.effect_on_target(cx)
    }
}

/// The arithmetic of `effectOnTarget` (`:99-114`) after its two reads: `damage ?? 250`, the
/// roll in `[1, 6)`, the firing unit's veterancy and (when `veterancy > 0`)
/// `warshipVeterancyShellDamageBonus()`.
pub(crate) fn shell_damage(damage: Option<f64>, roll: f64, veterancy: i32, bonus_percent: f64) -> f64 {
    // TS: :99
    let base_damage = damage.unwrap_or(250.0);
    // TS: :102
    let mut damage_multiplier = (roll - 1.0) * 25.0 + 200.0;
    // TS: :107-112
    if veterancy > 0 {
        damage_multiplier =
            of_det::js::floor((damage_multiplier * (100.0 + f64::from(veterancy) * bonus_percent)) / 100.0);
    }
    // TS: :114 `Math.round((baseDamage / 250) * damageMultiplier)` (PORTING_GUIDE §14.1)
    of_det::js::round((base_damage / 250.0) * damage_multiplier)
}

impl ExecBehavior for ShellExec {
    fn is_active(&self) -> bool {
        self.active
    }
    /// `activeDuringSpawnPhase()` returns `false` (`ShellExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    fn init(&mut self, cx: &mut Ctx, _ticks: i32) {
        // TS: :43-46 `PathFinding.Air(mg)` (seed = game.ticks()), then `new PseudoRandom(mg.ticks())`
        let ticks = cx.st.ticks();
        let path_finder = AirStepper::new(ticks);
        self.init = Some(ShellInit { path_finder, rng: Prng::new(f64::from(ticks)) });
    }
    fn tick(&mut self, cx: &mut Ctx, _ticks: i32) {
        // TS: :50 `this.shell ??= this._owner.buildUnit(UnitType.Shell, this.spawn, {})`
        let shell = match self.shell {
            Some(s) => s,
            None => {
                let s = cx.player_mut(self.owner).build_unit(UnitType::Shell, self.spawn, UnitParams::default());
                self.shell = Some(s);
                s
            }
        };
        // TS: :51-54
        if !cx.st.unit(shell).is_active() {
            self.active = false;
            return;
        }
        // TS: :55-63
        let destroy_now = !cx.st.unit(self.target).is_active()
            || cx.st.unit(self.target).owner() == cx.st.unit(shell).owner()
            || (self.destroy_at_tick != -1 && cx.st.ticks() >= self.destroy_at_tick);
        if destroy_now {
            cx.unit_mut(shell).delete(Some(false), None);
            self.active = false;
            return;
        }

        // TS: :65-67 `destroyAtTick = mg.ticks() + config.shellLifetime()`
        if self.destroy_at_tick == -1 && !cx.st.unit(self.owner_unit).is_active() {
            let at = f64::from(cx.st.ticks()) + cx.st.config().shell_lifetime();
            // int: `shellLifetime()` is an integer tick constant (`EngineConfig.shellLifetime`).
            debug_assert!(at.fract() == 0.0 && at <= f64::from(i32::MAX));
            self.destroy_at_tick = at as i32;
        }

        // TS: :69-95
        for _ in 0..3 {
            let (from, to) = (cx.st.unit(shell).tile(), cx.st.unit(self.target).tile());
            let init = self.init.as_mut().ts_expect("ShellExecution.ts:70 this.pathFinder");
            let result = init.path_finder.next(cx, from, to, None);
            match result {
                PathResult::Complete(_) => {
                    // TS: :75-78
                    self.active = false;
                    let target_type = cx.st.unit(self.target).unit_type();
                    let target_was_active = cx.st.unit(self.target).is_active();
                    let effect = self.effect_on_target(cx);
                    cx.unit_mut(self.target).modify_health(-effect, Some(self.owner));
                    // TS: :79-88 veterancy for the firing warship's killing blow
                    if target_was_active
                        && !cx.st.unit(self.target).is_active()
                        && cx.st.unit(self.owner_unit).is_active()
                        && cx.st.unit(self.owner_unit).unit_type() == UnitType::Warship
                    {
                        cx.unit_mut(self.owner_unit).record_kill(target_type);
                    }
                    // TS: :89-91
                    cx.unit_mut(shell).set_reached_target();
                    cx.unit_mut(shell).delete(Some(false), None);
                    return;
                }
                // TS: :92-94
                PathResult::Next(node) => {
                    cx.unit_mut(shell).move_to(node);
                }
                PathResult::NotFound => {}
            }
        }
    }
}

/// `Shell v1`: `{ active, init: {pathFinder: {seed, stepper}, random}|null, shell|null,
/// destroyAtTick, spawn, owner, ownerUnit, target }`.
impl Canon for ShellExec {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        let init = match &self.init {
            None => Cs::Null,
            Some(i) => Cs::Obj(vec![
                (
                    "pathFinder".into(),
                    Cs::Obj(vec![
                        ("seed".into(), Cs::Int(i64::from(i.path_finder.seed))),
                        ("stepper".into(), stepper_canon(&i.path_finder.stepper)),
                    ]),
                ),
                ("random".into(), prng_canon(&i.rng)),
            ]),
        };
        Cs::Obj(vec![
            ("active".into(), Cs::Bool(self.active)),
            ("init".into(), init),
            ("shell".into(), self.shell.canon(cx)),
            ("destroyAtTick".into(), Cs::Int(i64::from(self.destroy_at_tick))),
            ("spawn".into(), Cs::Int(i64::from(self.spawn))),
            ("owner".into(), self.owner.canon(cx)),
            ("ownerUnit".into(), self.owner_unit.canon(cx)),
            ("target".into(), self.target.canon(cx)),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ShellRandom.test.ts` "damage varies randomly between 200-300 base damage": every roll
    /// of `nextInt(1, 6)` gives `round((base/250) * m)` with `m ∈ {200, 225, 250, 275, 300}`.
    #[test]
    fn damage_range_per_roll_without_veterancy() {
        let got: Vec<f64> = (1..6).map(|r| shell_damage(None, f64::from(r), 0, 0.0)).collect();
        assert_eq!(got, [200.0, 225.0, 250.0, 275.0, 300.0]);
        // a non-default base damage scales linearly before the round
        assert_eq!(shell_damage(Some(100.0), 1.0, 0, 0.0), 80.0); // round(0.4 * 200)
        assert_eq!(shell_damage(Some(101.0), 2.0, 0, 0.0), 91.0); // round(90.9)
    }

    /// Veteran multiplier: `floor(m * (100 + v * bonus) / 100)` before the round
    /// (`ShellExecution.ts:107-112`).
    #[test]
    fn veteran_multiplier_floors_before_round() {
        // m = 225, v = 3, bonus = 7 → floor(225 * 121 / 100) = floor(272.25) = 272
        assert_eq!(shell_damage(None, 2.0, 3, 7.0), 272.0);
        // the bonus is ignored at veterancy 0, whatever its value
        assert_eq!(shell_damage(None, 5.0, 0, 1e9), 300.0);
    }

    /// `Math.round` at an exact .5 (`of_det::js::round`): base 125 gives `0.5 * m`; with
    /// m = 225 that is 112.5 → 113 (node: `Math.round((125/250)*225)` = 113).
    #[test]
    fn js_round_at_half() {
        assert_eq!(shell_damage(Some(125.0), 2.0, 0, 0.0), 113.0);
    }
}
