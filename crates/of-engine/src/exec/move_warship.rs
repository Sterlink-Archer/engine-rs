//! `MoveWarshipExecution` (`engine/src/execution/MoveWarshipExecution.ts`). Owner: **P7**.
//!
//! Fields: the `MoveWarship v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1).

use crate::canon::{Canon, CanonCx, Cs};
use crate::exec::prelude::*;
use crate::state::unit::WarshipStateUpdate;

/// `MoveWarshipExecution` data.
#[derive(StateClone, Debug)]
pub struct MoveWarshipExec {
    pub owner: SmallId,
    /// `unitIds: number[]` (unvalidated intent values; `None` = `undefined`, see
    /// `Intent::MoveWarship`)
    pub unit_ids: Option<Vec<f64>>,
    /// `position: TileRef` (unvalidated intent value)
    pub position: f64,
}

impl MoveWarshipExec {
    /// `new MoveWarshipExecution(owner, unitIds, position)`.
    pub fn new(owner: SmallId, unit_ids: Option<Vec<f64>>, position: f64) -> Self {
        MoveWarshipExec { owner, unit_ids, position }
    }
}

/// `new Set(ids)` iteration order over numbers: first occurrence wins, SameValueZero
/// equality (`NaN` equals `NaN`, `-0` equals `+0`), and `Set.prototype.add` stores `-0` as
/// `+0` (ECMA-262 §24.2.3.1 step 3; node prints `[...new Set([-0])]` as `[0]`).
/// `new Set(undefined)` is empty.
fn dedup_same_value_zero(ids: Option<&[f64]>) -> Vec<f64> {
    let mut out: Vec<f64> = Vec::new();
    for &x in ids.unwrap_or(&[]) {
        // SameValueZero: `==` except that NaN equals NaN (`-0 == 0` already holds).
        if !out.iter().any(|&y| y == x || (y.is_nan() && x.is_nan())) {
            out.push(if x == 0.0 { 0.0 } else { x });
        }
    }
    out
}

impl ExecBehavior for MoveWarshipExec {
    /// TS: :55-57 `return false`.
    fn is_active(&self) -> bool {
        false
    }
    /// `activeDuringSpawnPhase()` returns `false` (`MoveWarshipExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    fn init(&mut self, cx: &mut Ctx, _ticks: i32) {
        // TS: :20-23 (warn dropped)
        if !cx.st.map.is_valid_ref(self.position) {
            return;
        }
        // in-range: `isValidRef` holds, so `position` is an integer tile ref < width*height.
        let position = self.position as TileRef;
        // TS: :25 `mg.getWaterComponent(this.position)`
        let new_patrol_tile_water_component = cx.rcx().get_water_component(position);
        // TS: :27-29 `new Map(owner.units(Warship).map(u => [u.id(), u]))`: a lookup-only map
        // (never iterated); ids are unique, so a scan of the copy finds the same unit.
        let warships = cx.st.player(self.owner).units_of(&[UnitType::Warship]);
        // TS: :31 `for (const unitId of new Set(this.unitIds))`
        for unit_id in dedup_same_value_zero(self.unit_ids.as_deref()) {
            // TS: :32 `warshipMap.get(unitId)`: Map keys compare by SameValueZero, so a number
            // key matches iff it equals the integer id (`-0` matches 0, NaN/fractions none).
            let warship = warships.iter().copied().find(|&u| f64::from(cx.st.unit(u).id()) == unit_id);
            // TS: :33-36
            let Some(warship) = warship else { continue };
            // TS: :37-40
            if !cx.st.unit(warship).is_active() {
                continue;
            }
            // TS: :41-44 `!mg.hasWaterComponent(warship.tile(), newPatrolTileWaterComponent!)`.
            // `!` is a type assertion only: a `null` component reaches hasWaterComponent.
            // `getWaterComponent` returns null only when the water graph exists
            // (`WaterManager.ts:229-257`), and then hasWaterComponent compares numeric
            // component ids (`AbstractGraph.getComponentId` returns a number) against null,
            // which is false for every tile (`WaterManager.ts:259-299`).
            let wtile = cx.st.unit(warship).tile();
            let same_component = match new_patrol_tile_water_component {
                Some(c) => cx.rcx().has_water_component(wtile, c),
                None => false,
            };
            if !same_component {
                continue;
            }
            // TS: :45-48
            cx.unit_mut(warship).update_warship_state(WarshipStateUpdate {
                patrol_tile: Some(Some(position)),
                ..WarshipStateUpdate::default()
            });
            // TS: :48
            cx.unit_mut(warship).set_target_tile(None);
        }
    }
    /// TS: :52 `tick(_ticks): void {}`.
    fn tick(&mut self, _cx: &mut Ctx, _ticks: i32) {}
}

/// `MoveWarship v1`: `{ owner, unitIds[num], position: num }`. (TS `[...this.unitIds]` would
/// throw on `undefined`; the port exports an empty array.)
impl Canon for MoveWarshipExec {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        let ids: &[f64] = self.unit_ids.as_deref().unwrap_or(&[]);
        Cs::Obj(vec![
            ("owner".into(), self.owner.canon(cx)),
            ("unitIds".into(), ids.canon(cx)),
            ("position".into(), Cs::Num(self.position)),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `[...new Set([3, 1, 3, -0, 0, NaN, NaN, 1.5, 1])]` is `[3, 1, 0, NaN, 1.5]` in node
    /// (and `Object.is(that[2], -0)` is false).
    #[test]
    fn set_dedup_is_same_value_zero_first_occurrence() {
        let ids = [3.0, 1.0, 3.0, -0.0, 0.0, f64::NAN, f64::NAN, 1.5, 1.0];
        let d = dedup_same_value_zero(Some(&ids));
        assert_eq!(d.len(), 5);
        assert_eq!((d[0], d[1], d[4]), (3.0, 1.0, 1.5));
        assert!(d[2] == 0.0 && d[2].is_sign_positive(), "Set.add stores -0 as +0");
        assert!(d[3].is_nan());
        assert!(dedup_same_value_zero(None).is_empty());
    }
}
