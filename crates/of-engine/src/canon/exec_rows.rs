//! CS E rows: `{t: <registered type name>, d: <state>}` per execution key (SPEC §1.3 E,
//! `snapshot/ExecutionRegistry.ts:56-100`), dispatched over the 44 kinds to each execution
//! struct's own `Canon`/`Uncanon` impl (written by the struct's owner; reached through
//! [`crate::canon_or_missing!`] so a not-yet-written impl is reported, not a build error).
//! Owner: **P13**.

use super::{CanonCx, CanonError, Cs, UncanonCx};
use crate::exec::{Exec, ExecArenas};
use crate::ids::{ExecId, ExecKind};

macro_rules! exec_dispatch {
    ($($kind:ident => $field:ident : $ty:path),* $(,)?) => {
        /// `e.snapshot(w).d` for execution `id` (SPEC §1.6 N4 unwraps the versioned record).
        pub fn exec_state(ar: &ExecArenas, id: ExecId, cx: &mut CanonCx<'_>) -> Result<Cs, CanonError> {
            if !ar.is_allocated(id) {
                return Err(CanonError::new(format!("CS: execution {id:?} names a freed slot (Rust bug)")));
            }
            match id.kind {
                $(ExecKind::$kind => crate::canon_or_missing!(*ar.$field.get(id.idx), cx),)*
                #[cfg(test)]
                ExecKind::Probe => Err(CanonError::new("the test-only Probe execution has no CS row")),
            }
        }

        /// The execution of kind `kind` from its CS state `d` (TS: `Object.create(cls.prototype)`
        /// + `restoreSnapshot(data, r)`, `GameSnapshot.ts:349-397`).
        pub fn exec_from_state(kind: ExecKind, d: &Cs, cx: &mut UncanonCx<'_>) -> Result<Exec, CanonError> {
            match kind {
                $(ExecKind::$kind => crate::uncanon_or_missing!($ty, d, cx).map(Exec::$kind),)*
                #[cfg(test)]
                ExecKind::Probe => Err(CanonError::new("the test-only Probe execution has no CS row")),
            }
        }
    };
}

exec_dispatch! {
    AllianceExtension => alliance_extension: crate::exec::alliance::extension::AllianceExtensionExec,
    AllianceReject => alliance_reject: crate::exec::alliance::reject::AllianceRejectExec,
    AllianceRequest => alliance_request: crate::exec::alliance::request::AllianceRequestExec,
    Attack => attack: crate::exec::attack::AttackExec,
    BoatRetreat => boat_retreat: crate::exec::boat_retreat::BoatRetreatExec,
    BreakAlliance => break_alliance: crate::exec::alliance::break_::BreakAllianceExec,
    City => city: crate::exec::city::CityExec,
    Construction => construction: crate::exec::construction::ConstructionExec,
    DefensePost => defense_post: crate::exec::defense_post::DefensePostExec,
    DeleteUnit => delete_unit: crate::exec::delete_unit::DeleteUnitExec,
    DonateGold => donate_gold: crate::exec::donate_gold::DonateGoldExec,
    DonateTroops => donate_troops: crate::exec::donate_troops::DonateTroopsExec,
    DoomsdayClock => doomsday_clock: crate::exec::doomsday::DoomsdayClockExec,
    EmbargoAll => embargo_all: crate::exec::embargo_all::EmbargoAllExec,
    Embargo => embargo: crate::exec::embargo::EmbargoExec,
    Emoji => emoji: crate::exec::emoji::EmojiExec,
    Factory => factory: crate::exec::factory::FactoryExec,
    MarkDisconnected => mark_disconnected: crate::exec::mark_disconnected::MarkDisconnectedExec,
    Mirv => mirv: crate::exec::mirv::MirvExec,
    MissileSilo => missile_silo: crate::exec::missile_silo::MissileSiloExec,
    MoveWarship => move_warship: crate::exec::move_warship::MoveWarshipExec,
    Nation => nation: crate::exec::nation::NationExec,
    NoOp => noop: crate::exec::noop::NoOpExec,
    Nuke => nuke: crate::exec::nuke::NukeExec,
    Pause => pause: crate::exec::pause::PauseExec,
    Player => player: crate::exec::player::PlayerExec,
    Port => port: crate::exec::port::PortExec,
    QuickChat => quick_chat: crate::exec::quick_chat::QuickChatExec,
    RecomputeRailCluster => recompute_rail_cluster: crate::exec::rail_cluster::RecomputeRailClusterExec,
    Retreat => retreat: crate::exec::retreat::RetreatExec,
    SamLauncher => sam_launcher: crate::exec::sam_launcher::SamLauncherExec,
    SamMissile => sam_missile: crate::exec::sam_missile::SamMissileExec,
    Shell => shell: crate::exec::shell::ShellExec,
    Spawn => spawn: crate::exec::spawn::SpawnExec,
    SpawnTimer => spawn_timer: crate::exec::spawn_timer::SpawnTimerExec,
    TargetPlayer => target_player: crate::exec::target_player::TargetPlayerExec,
    TradeShip => trade_ship: crate::exec::trade_ship::TradeShipExec,
    Train => train: crate::exec::train::TrainExec,
    TrainStation => train_station: crate::exec::train_station::TrainStationExec,
    TransportShip => transport_ship: crate::exec::transport_ship::TransportShipExec,
    Tribe => tribe: crate::exec::tribe::TribeExec,
    UpgradeStructure => upgrade_structure: crate::exec::upgrade_structure::UpgradeStructureExec,
    Warship => warship: crate::exec::warship::WarshipExec,
    WinCheck => win_check: crate::exec::win_check::WinCheckExec,
}

/// The CS E row `{t, d}` of execution `id`; `toConquer.capacity` is stripped from Attack
/// rows (SPEC §1.6 N3, `conformance/src/canon.ts:76-83`) in case the owner emits it.
pub fn exec_row(ar: &ExecArenas, id: ExecId, cx: &mut CanonCx<'_>) -> Result<Cs, CanonError> {
    let mut d = exec_state(ar, id, cx)?;
    if id.kind == ExecKind::Attack
        && let Some(Cs::Obj(f)) = d.get_mut("toConquer")
    {
        f.retain(|(k, _)| k != "capacity");
    }
    Ok(Cs::Obj(vec![("t".into(), Cs::Str(id.kind.as_str().to_owned())), ("d".into(), d)]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canon::LoadKeys;
    use crate::exec::{ExecStore, noop::NoOpExec};
    use crate::state::State;

    /// Every one of the 44 kinds dispatches (either to its owner's impl or to a reported
    /// "missing"), and the row wrapper carries the registered type name.
    #[test]
    fn all_kinds_dispatch() {
        let st = State::default();
        let mut ex = ExecStore::default();
        let id = ex.ar.insert(Exec::NoOp(NoOpExec::new()));
        let mut cx = CanonCx::new(&st, &ex);
        match exec_row(&ex.ar, id, &mut cx) {
            Ok(row) => assert_eq!(row.get("t"), Some(&Cs::Str("NoOp".into()))),
            Err(e) => assert!(e.is_missing(), "{e}"),
        }
        let keys = LoadKeys::default();
        let mut st2 = State::default();
        let mut ucx = UncanonCx { st: &mut st2, keys: &keys };
        for k in ExecKind::ALL {
            match exec_from_state(k, &Cs::Obj(vec![]), &mut ucx) {
                Ok(e) => assert_eq!(e.kind(), k),
                Err(e) => assert!(!e.0.is_empty()),
            }
        }
        // A freed slot is a Rust bug, reported as an error (never a panic in the exporter).
        let ghost = ExecId { kind: ExecKind::Nuke, idx: 3 };
        assert!(exec_row(&ex.ar, ghost, &mut cx).is_err());
    }
}
