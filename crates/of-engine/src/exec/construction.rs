//! `ConstructionExecution` (`engine/src/execution/ConstructionExecution.ts`). Owner: **P7**.
//!
//! Fields: the `Construction v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1).

use crate::canon::{Canon, CanonCx, Cs};
use crate::exec::city::CityExec;
use crate::exec::defense_post::DefensePostExec;
use crate::exec::factory::FactoryExec;
use crate::exec::mirv::MirvExec;
use crate::exec::missile_silo::MissileSiloExec;
use crate::exec::nuke::NukeExec;
use crate::exec::port::PortExec;
use crate::exec::prelude::*;
use crate::exec::sam_launcher::SamLauncherExec;
use crate::exec::warship::{WarshipExec, WarshipInput};
use crate::state::unit::UnitParams;

/// `ConstructionExecution` data.
#[derive(StateClone, Debug)]
pub struct ConstructionExec {
    pub active: bool,
    /// `initialized: this.mg !== undefined` (set by `init`).
    pub initialized: bool,
    pub player: SmallId,
    pub construction_type: UnitType,
    /// Validated in `init` (`isValidRef`); an out-of-range intent value is `u32::MAX`
    /// (`runner/executor.rs`), which is never a valid ref either.
    pub tile: TileRef,
    pub rocket_direction_up: Option<bool>,
    pub amount: Option<u32>,
    pub structure: Option<UnitRef>,
    /// `ticksUntilComplete: Tick`, unset (`undefined`) until a structure with a build time is
    /// placed.
    pub ticks_until_complete: Option<i32>,
}

impl ConstructionExec {
    /// `new ConstructionExecution(player, constructionType, tile, rocketDirectionUp?, amount?)`.
    pub fn new(
        player: SmallId,
        construction_type: UnitType,
        tile: TileRef,
        rocket_direction_up: Option<bool>,
        amount: Option<u32>,
    ) -> Self {
        // TS: :28-30 `structure = null`, `active = true`
        ConstructionExec {
            active: true,
            initialized: false,
            player,
            construction_type,
            tile,
            rocket_direction_up,
            amount,
            structure: None,
            ticks_until_complete: None,
        }
    }

    /// `completeConstruction()` (`:114-173`).
    fn complete_construction(&mut self, cx: &mut Ctx) {
        // TS: :115-117
        if let Some(s) = self.structure {
            cx.unit_mut(s).set_under_construction(false);
        }
        // TS: :118
        let player = self.player;
        match self.construction_type {
            // TS: :120-137
            UnitType::AtomBomb | UnitType::HydrogenBomb => {
                // TS: :122 `const count = this.amount ?? 1`
                let count = self.amount.unwrap_or(1);
                for _ in 0..count {
                    // TS: :125-134 `new NukeExecution(type, player, tile, null, -1, 0,
                    // this.rocketDirectionUp)`: an undefined `rocketDirectionUp` takes the
                    // parameter default `true` (`NukeExecution.ts:54`).
                    cx.st.add_execution(Exec::Nuke(NukeExec::new(
                        self.construction_type,
                        player,
                        self.tile,
                        Tri::Null,
                        -1.0,
                        0,
                        self.rocket_direction_up.unwrap_or(true),
                    )));
                }
            }
            // TS: :139-141
            UnitType::Mirv => {
                cx.st.add_execution(Exec::Mirv(MirvExec::new(player, self.tile)));
            }
            // TS: :142-146 `new WarshipExecution({ owner: player, patrolTile: this.tile })`
            UnitType::Warship => {
                cx.st.add_execution(Exec::Warship(WarshipExec::new(WarshipInput::New {
                    owner: player,
                    patrol_tile: self.tile,
                })));
            }
            // TS: :147-149
            UnitType::Port => {
                let s = self.structure.ts_expect("ConstructionExecution.ts:148 this.structure!");
                cx.st.add_execution(Exec::Port(PortExec::new(s)));
            }
            // TS: :150-152
            UnitType::MissileSilo => {
                let s = self.structure.ts_expect("ConstructionExecution.ts:151 this.structure!");
                cx.st.add_execution(Exec::MissileSilo(MissileSiloExec::new(s)));
            }
            // TS: :153-155
            UnitType::DefensePost => {
                let s = self.structure.ts_expect("ConstructionExecution.ts:154 this.structure!");
                cx.st.add_execution(Exec::DefensePost(DefensePostExec::new(s)));
            }
            // TS: :156-160 `new SAMLauncherExecution(player, null, this.structure!)`. Its
            // constructor overwrites `tile` with `sam.tile()` when `sam !== null`
            // (`SAMLauncherExecution.ts:320-327`); the frozen `SamLauncherExec::new` has no
            // state access, so the caller passes that tile (`docs/requests/P7.md` R2).
            UnitType::SamLauncher => {
                let s = self.structure.ts_expect("ConstructionExecution.ts:158 this.structure!");
                let sam_tile = cx.st.unit(s).tile();
                cx.st.add_execution(Exec::SamLauncher(SamLauncherExec::new(player, Some(sam_tile), Some(s))));
            }
            // TS: :161-163
            UnitType::City => {
                let s = self.structure.ts_expect("ConstructionExecution.ts:162 this.structure!");
                cx.st.add_execution(Exec::City(CityExec::new(s)));
            }
            // TS: :164-166
            UnitType::Factory => {
                let s = self.structure.ts_expect("ConstructionExecution.ts:165 this.structure!");
                cx.st.add_execution(Exec::Factory(FactoryExec::new(s)));
            }
            // TS: :167-171 `console.warn(... cannot be constructed)`
            UnitType::TransportShip
            | UnitType::Shell
            | UnitType::SamMissile
            | UnitType::TradeShip
            | UnitType::MirvWarhead
            | UnitType::Train => {}
        }
    }
}

/// `isStructure(type)` (`:175-187`).
pub(crate) fn is_structure(t: UnitType) -> bool {
    matches!(
        t,
        UnitType::Port
            | UnitType::MissileSilo
            | UnitType::DefensePost
            | UnitType::SamLauncher
            | UnitType::City
            | UnitType::Factory
    )
}

impl ExecBehavior for ConstructionExec {
    fn is_active(&self) -> bool {
        self.active
    }
    /// `activeDuringSpawnPhase()` returns `false` (`ConstructionExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    fn init(&mut self, cx: &mut Ctx, _ticks: i32) {
        // TS: :43 `this.mg = mg`
        self.initialized = true;

        // TS: :45-51 (console.warn dropped)
        if cx.st.config().is_unit_disabled(self.construction_type) {
            self.active = false;
            return;
        }

        // TS: :53-57
        if !cx.st.map.is_valid_ref(f64::from(self.tile)) {
            self.active = false;
        }
    }
    fn tick(&mut self, cx: &mut Ctx, _ticks: i32) {
        let Some(structure) = self.structure else {
            // TS: :61-94 (`this.structure === null`)
            // TS: :62 `const info = this.mg.unitInfo(this.constructionType)` (GameImpl.unitInfo
            // → config().unitInfo; read before the structure check, as TS does).
            let info = cx.st.config().unit_info(self.construction_type);
            // TS: :64-70: non-structure units (nukes/warship) delegate to their executions.
            if !is_structure(self.construction_type) {
                self.complete_construction(cx);
                self.active = false;
                return;
            }

            // TS: :73-78 `player.canBuild(constructionType, tile)` (validTiles = null)
            let spawn_tile = cx.st.player(self.player).can_build(cx.sc, self.construction_type, self.tile, None);
            let Some(spawn_tile) = spawn_tile else {
                self.active = false;
                return;
            };
            // TS: :79-83 `buildUnit(type, spawnTile, {})`
            let s = cx.player_mut(self.player).build_unit(self.construction_type, spawn_tile, UnitParams::default());
            self.structure = Some(s);
            // TS: :84 `info.constructionDuration ?? 0`
            let duration = info.construction_duration.unwrap_or(0.0);
            if duration > 0.0 {
                cx.unit_mut(s).set_under_construction(true);
                // int: `constructionDuration` is an integer tick count in the unit table
                // (`Config.unitInfo`, `EngineConfig.ts:338-346`).
                debug_assert!(duration.fract() == 0.0 && duration <= f64::from(i32::MAX));
                self.ticks_until_complete = Some(duration as i32);
                return;
            }
            // TS: :90-93 (no construction time)
            self.complete_construction(cx);
            self.active = false;
            return;
        };

        // TS: :96-99
        if !cx.st.unit(structure).is_active() {
            self.active = false;
            return;
        }

        // TS: :101-103
        let owner = cx.st.unit(structure).owner();
        if self.player != owner {
            self.player = owner;
        }

        // TS: :105-110 `if (this.ticksUntilComplete === 0)`. The `duration > 0` branch above is
        // the only live path that keeps the execution active, and it sets the count. Only a
        // restored record can lack it: then `undefined === 0` is false and `undefined--` is
        // NaN, which never reaches 0 (kept as `None`: the execution idles, as in TS).
        let Some(left) = self.ticks_until_complete else { return };
        if left == 0 {
            self.player = cx.st.unit(structure).owner();
            self.complete_construction(cx);
            self.active = false;
            return;
        }
        // TS: :111
        self.ticks_until_complete = Some(left - 1);
    }
}

/// `Construction v1`: `{ active, initialized, player, constructionType, tile,
/// rocketDirectionUp?, amount?, structure|null, ticksUntilComplete? }`.
impl Canon for ConstructionExec {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        // Fields in dump order; `?undef` fields are `Cs::Absent` when undefined.
        Cs::Obj(vec![
            ("active".into(), Cs::Bool(self.active)),
            ("initialized".into(), Cs::Bool(self.initialized)),
            ("player".into(), self.player.canon(cx)),
            ("constructionType".into(), Cs::Str(self.construction_type.as_str().to_owned())),
            ("tile".into(), Cs::Int(i64::from(self.tile))),
            ("rocketDirectionUp".into(), self.rocket_direction_up.map_or(Cs::Absent, Cs::Bool)),
            ("amount".into(), self.amount.map_or(Cs::Absent, |a| Cs::Int(i64::from(a)))),
            ("structure".into(), self.structure.canon(cx)),
            ("ticksUntilComplete".into(), self.ticks_until_complete.map_or(Cs::Absent, |t| Cs::Int(i64::from(t)))),
        ])
    }
}
