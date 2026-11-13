//! `SAMMissileExecution` (`engine/src/execution/SAMMissileExecution.ts`). Owner: **P8**.
//!
//! Fields: the `SAMMissile v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1).
//!
//! The interception `displayMessage` (`:88-98`) is view work (dropped); its arguments
//! (`INTERCEPTED_UNIT_TRANSLATION_KEYS[type] ?? type`, `_owner.id()`) are pure.

#[allow(unused_imports)]
use crate::exec::prelude::*;
use crate::path::types::PathResult;
use crate::state::stats::StatNum;
use crate::state::unit::UnitParams;

/// `nukesWhitelist` (`:63-67`): the MIRV carrier itself can't be intercepted, only its warheads.
const NUKES_WHITELIST: [UnitType; 3] = [UnitType::AtomBomb, UnitType::HydrogenBomb, UnitType::MirvWarhead];

/// `SAMMissileExecution` data.
#[derive(StateClone, Debug)]
pub struct SamMissileExec {
    pub active: bool,
    /// `PathFinding.Air(mg)` (set in `init`)
    pub path_finder: Option<AirStepper>,
    pub missile: Option<UnitRef>,
    pub speed: f64,
    pub spawn: TileRef,
    pub owner: SmallId,
    pub owner_unit: UnitRef,
    pub target: UnitRef,
    pub target_tile: TileRef,
}

impl SamMissileExec {
    /// `new SAMMissileExecution(spawn, owner, ownerUnit, target, targetTile)`.
    pub fn new(spawn: TileRef, owner: SmallId, owner_unit: UnitRef, target: UnitRef, target_tile: TileRef) -> Self {
        // TS: SAMMissileExecution.ts:33-45 (`speed = 0` until init).
        SamMissileExec {
            active: true,
            path_finder: None,
            missile: None,
            speed: 0.0,
            spawn,
            owner,
            owner_unit,
            target,
            target_tile,
        }
    }
}

impl ExecBehavior for SamMissileExec {
    fn is_active(&self) -> bool {
        self.active
    }
    /// `activeDuringSpawnPhase()` returns `false` (`SAMMissileExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    /// `init(mg, ticks)` (`:47-52`): **ticks once from init** (`this.tick(ticks)`).
    fn init(&mut self, cx: &mut Ctx, ticks: i32) {
        self.path_finder = Some(AirStepper::new(cx.st.ticks())); // TS: PathFinding.Air(mg)
        self.speed = cx.st.config().default_sam_missile_speed();
        self.tick(cx, ticks);
    }
    /// `tick(ticks)` (`:54-112`).
    fn tick(&mut self, cx: &mut Ctx, _ticks: i32) {
        // TS: `this.SAMMissile ??= this._owner.buildUnit(UnitType.SAMMissile, this.spawn, { targetUnit })`
        let missile = match self.missile {
            Some(m) => m,
            None => {
                let m = cx.player_mut(self.owner).build_unit(
                    UnitType::SamMissile,
                    self.spawn,
                    UnitParams { target_unit: Some(Some(self.target)), ..UnitParams::default() },
                );
                self.missile = Some(m);
                m
            }
        };
        if !cx.st.unit(missile).is_active() {
            self.active = false;
            return;
        }
        if !cx.st.unit(self.target).is_active()
            || !cx.st.unit(self.owner_unit).is_active()
            || cx.st.unit(self.target).owner() == cx.st.unit(missile).owner()
            || !NUKES_WHITELIST.contains(&cx.st.unit(self.target).unit_type())
        {
            // Clear the flag so other SAMs can re-target this nuke
            if cx.st.unit(self.target).is_active() {
                cx.unit_mut(self.target).set_targeted_by_sam(false);
            }
            cx.unit_mut(missile).delete(Some(false), None);
            self.active = false;
            return;
        }
        // TS: `for (let i = 0; i < this.speed; i++)` with a number bound.
        let mut i = 0.0;
        while i < self.speed {
            let from = cx.st.unit(missile).tile();
            let pf = self.path_finder.as_mut().ts_expect("SAMMissileExecution.ts:83 this.pathFinder");
            let result = pf.next(cx, from, self.target_tile, None);
            match result {
                PathResult::Complete(_) => {
                    // TS: :88-98 displayMessage (view, dropped).
                    self.active = false;
                    cx.unit_mut(self.target).delete(Some(true), Some(self.owner));
                    cx.unit_mut(missile).delete(Some(false), None);

                    // Record stats
                    let target_type = cx.st.unit(self.target).unit_type();
                    cx.st.stats_cx().bomb_intercept(self.owner, target_type, StatNum::from(1.0));
                    return;
                }
                PathResult::Next(node) => {
                    cx.unit_mut(missile).move_to(node);
                }
                PathResult::NotFound => {}
            }
            i += 1.0;
        }
    }
}

// CS v1 `SAMMissile v1 (execution)` record (`SAMMissileExecution.ts:121-135`), dump field
// order. Written at merge1 by the integrator on behalf of P8.
crate::impl_canon_record!(SamMissileExec {
    "active" => active,
    "pathFinder" => path_finder,
    "missile" => missile,
    "speed" => speed,
    "spawn" => spawn,
    "owner" => owner,
    "ownerUnit" => owner_unit,
    "target" => target,
    "targetTile" => target_tile,
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_matches_ts_field_initialisers() {
        let (o, t) = (UnitRef { slot: 1, generation: 0 }, UnitRef { slot: 2, generation: 0 });
        let e = SamMissileExec::new(5, SmallId(3), o, t, 9);
        assert!(e.active && e.is_active() && !e.active_during_spawn_phase());
        assert!(e.path_finder.is_none() && e.missile.is_none());
        assert_eq!((e.speed, e.spawn, e.owner, e.owner_unit, e.target, e.target_tile), (0.0, 5, SmallId(3), o, t, 9));
    }

    #[test]
    fn whitelist_excludes_mirv_carrier() {
        assert!(!NUKES_WHITELIST.contains(&UnitType::Mirv));
        assert!(NUKES_WHITELIST.contains(&UnitType::MirvWarhead));
    }
}
