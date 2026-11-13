//! Executions: the `Execution` interface, the 44 kinds, per-kind arenas, the exec store and
//! the MIRV command channel (ARCHITECTURE §4.2-4.4). Owner of this file: **P1**; each
//! execution file belongs to the package FILE_MAP §1 names in its header.
//!
//! - [`ExecBehavior`] is the TS `Execution` interface (`engine/src/game/Game.ts:125-136`);
//!   `snapshot()` is replaced by `Canon` (P13).
//! - [`Exec`] is the transport type `add_execution` receives (one by-value variant per kind,
//!   no `Box`); it lives in `State.exec_inbox` until the driver drains it into the
//!   kind's [`Slots`] and appends its [`ExecId`] to `State.uninit`.
//! - [`ExecStore`] = TS `GameImpl.execs` (`live`, tick order) + the arenas.
//! - The only exec→exec mutation is MIRV cancelling its warheads (`MIRVExecution.ts:179-187`):
//!   [`ExecCmd`] queued by MIRV, applied right after its `tick` returns (§4.3).
//!
//! TS `TypeError` sites use `.ts_expect("<file:line>")`, never `unwrap`/`expect` (PORTING_GUIDE
//! §12); the lints below make a bare one a compile error in every execution file. A true Rust
//! invariant uses `#[allow(..)]` with a `// rust-invariant:` comment.
#![deny(clippy::unwrap_used, clippy::expect_used)]

pub mod ai_attack;
pub mod alliance;
pub mod attack;
pub mod boat_retreat;
pub mod city;
pub mod construction;
pub mod defense_post;
pub mod delete_unit;
pub mod donate_gold;
pub mod donate_troops;
pub mod doomsday;
pub mod embargo;
pub mod embargo_all;
pub mod emoji;
pub mod factory;
pub mod flat_heap;
pub mod mark_disconnected;
pub mod mirv;
pub mod missile_silo;
pub mod move_warship;
pub mod nation;
pub mod noop;
pub mod nuke;
pub mod nuke_alliance;
pub mod pause;
pub mod player;
pub mod port;
#[cfg(test)]
pub mod probe;
pub mod quick_chat;
pub mod rail_cluster;
pub mod retreat;
pub mod sam_launcher;
pub mod sam_missile;
pub mod shell;
pub mod spawn;
pub mod spawn_timer;
pub mod target_player;
pub mod trade_ship;
pub mod train;
pub mod train_station;
pub mod transport_ship;
pub mod tribe;
pub mod upgrade_structure;
pub mod util;
pub mod warship;
pub mod win_check;

use crate::StateClone;
use crate::ctx::Ctx;
use crate::ids::{ExecId, ExecKind, PendingExec};
use crate::plain::PlainState;
use crate::state::State;

/// What every execution file imports (`use crate::exec::prelude::*`).
pub mod prelude {
    pub use super::{Exec, ExecBehavior, ExecCmd, WarheadRef};
    pub use crate::StateClone;
    pub use crate::api::intents::EmbargoAction;
    pub use crate::api::types::{EmojiRecipient, Gold, Nation, PlayerInfo, PlayerType, Tick, UnitType};
    pub use crate::ctx::{Ctx, Rcx, TsExpect};
    pub use crate::ids::{
        AllianceId, AttackRef, ExecId, Owner, PendingExec, ReqIdx, SmallId, SmallIdKey, StationId, TileRef, UnitKey,
        UnitRef,
    };
    pub use crate::jsorder::{JsMap, JsSet, SmallJsMap, SmallJsSet};
    pub use crate::jsval::Tri;
    pub use crate::path::parabola::ParabolaPathFinder;
    pub use crate::path::pathfinder::{AirStepper, WaterPathFinder};
    pub use crate::rail::railroad::OrientedRailroad;
    pub use crate::tileset::OrderedTileSet;
    pub use of_det::Prng;
    pub use of_det::heap::FlatBinaryHeap;
    pub use std::collections::VecDeque;
}

/// The TS `Execution` interface (`engine/src/game/Game.ts:125-136`).
pub trait ExecBehavior {
    /// `isActive()`, for executions whose answer is their own field.
    fn is_active(&self) -> bool;
    /// `isActive()` as the driver calls it: with the state, for executions whose TS
    /// `isActive()` reads the game (`WarshipExecution.ts:762-764` reads the unit,
    /// `SpawnTimerExecution.ts:19-21` reads `inSpawnPhase()`). Those override this; every
    /// other execution keeps the default. The tick-pass gate and compaction call this one
    /// (requests P6 #1, P7 R1).
    fn is_active_in(&self, st: &State) -> bool {
        let _ = st;
        self.is_active()
    }
    /// `activeDuringSpawnPhase()`.
    fn active_during_spawn_phase(&self) -> bool;
    /// `init(mg, ticks)`. Runs even on inactive executions (`GameImpl.ts:614-627`).
    fn init(&mut self, cx: &mut Ctx, ticks: i32);
    /// `tick(ticks)`.
    fn tick(&mut self, cx: &mut Ctx, ticks: i32);
}

/// A MIRV warhead handle (`MIRVExecution.warheadExecutions`, `:85`): pending (an index into
/// `exec_inbox`) until the driver binds it to its `ExecId` right after the MIRV's call
/// returns (§4.3). Only `Id` is ever seen at a tick boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WarheadRef {
    Pending(PendingExec),
    Id(ExecId),
}
impl crate::plain::PlainState for WarheadRef {}
/// A bound warhead is an execution reference (a detached warhead is a CS E row reachable only
/// through its MIRV, SPEC §1.2.1).
impl crate::canon::Refs for WarheadRef {
    #[inline]
    fn visit_refs(&self, v: &mut dyn crate::canon::RefVisitor) {
        if let WarheadRef::Id(id) = self {
            v.visit_exec(*id);
        }
    }
}

/// Commands from MIRV to the driver (§4.3), queued in `State.exec_cmds` and applied by
/// [`ExecStore::apply_exec_cmds`] immediately after the queuing execution's `tick`/`init`
/// returns, in queue order. Empty at tick boundaries. The issuing execution is implicit (the
/// driver knows which one ran), so an execution never needs its own `ExecId`.
///
/// MIRV's use (P8):
/// - spawning a warhead (`MIRVExecution.ts:296-297`): `let p = cx.st.add_execution(Exec::Nuke(..));`
///   then `self.warhead_executions.push(WarheadRef::Pending(p));` and
///   `cx.st.exec_cmds.push(ExecCmd::BindWarhead { k, pending: p })` (`k` = its list index);
/// - the cancel loop (`:181-186`): for each `w` in the list, `CancelNuke(w)`, then for each
///   `w`, `Unpin(w)`; then `self.warhead_executions.clear()` and return.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecCmd {
    /// Rewrite the issuer's `warhead_executions[k]` from `Pending(pending)` to the drained
    /// `ExecId`, and pin that slot (one hold per listing MIRV).
    BindWarhead { k: u32, pending: PendingExec },
    /// `warhead.cancel()` (`NukeExecution.ts:510-515`).
    CancelNuke(WarheadRef),
    /// Drop the issuer's hold on a warhead slot (frees it if it was already compacted out).
    Unpin(WarheadRef),
}
crate::plain_copy!(ExecCmd);

/// One per-kind arena (§4.2). Slot reuse is a LIFO free list. `holds[i]` counts what keeps
/// slot `i` allocated: 1 while it is listed (`uninit` or `live`), plus 1 per MIRV that lists it
/// as a warhead (§4.3 "pinning"). A slot is freed when its count reaches 0, so a warhead
/// compacted out of `live` stays readable while its MIRV still lists it.
#[derive(StateClone, Debug)]
pub struct Slots<T> {
    pub items: Vec<Option<T>>,
    pub holds: Vec<u16>,
    pub free: Vec<u32>,
}

impl<T> Default for Slots<T> {
    fn default() -> Self {
        Slots { items: Vec::new(), holds: Vec::new(), free: Vec::new() }
    }
}

impl<T: Clone + PlainState> Slots<T> {
    /// Store `x` with one hold (its listing), returning its slot index.
    pub fn alloc(&mut self, x: T) -> u32 {
        if let Some(i) = self.free.pop() {
            let slot = &mut self.items[i as usize]; // in-range: free list holds valid slots
            debug_assert!(slot.is_none());
            *slot = Some(x);
            self.holds[i as usize] = 1; // in-range: holds parallel to items
            i
        } else {
            #[allow(clippy::expect_used)] // rust-invariant: < 2^32 executions alive at once
            let i = u32::try_from(self.items.len()).expect("exec slot fits u32");
            self.items.push(Some(x));
            self.holds.push(1);
            i
        }
    }
    #[cold]
    #[inline(never)]
    fn dead(i: u32) -> ! {
        panic!("of-engine exec arena: freed slot {i} (Rust bug, not a TS behaviour)")
    }
    #[inline]
    pub fn get(&self, i: u32) -> &T {
        match self.items.get(i as usize) {
            Some(Some(x)) => x,
            _ => Self::dead(i),
        }
    }
    #[inline]
    pub fn get_mut(&mut self, i: u32) -> &mut T {
        match self.items.get_mut(i as usize) {
            Some(Some(x)) => x,
            _ => Self::dead(i),
        }
    }
    #[inline]
    pub fn is_allocated(&self, i: u32) -> bool {
        matches!(self.items.get(i as usize), Some(Some(_)))
    }
    /// Number of allocated slots.
    pub fn len(&self) -> usize {
        self.items.len() - self.free.len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Add one hold to slot `i`.
    pub fn hold(&mut self, i: u32) {
        if !self.is_allocated(i) {
            Self::dead(i)
        }
        let h = &mut self.holds[i as usize]; // in-range: allocated
        *h = h.checked_add(1).unwrap_or_else(|| Self::dead(i));
    }
    /// Drop one hold from slot `i`; frees it (returning the value) when none is left.
    pub fn release(&mut self, i: u32) -> Option<T> {
        if !self.is_allocated(i) {
            Self::dead(i)
        }
        let h = &mut self.holds[i as usize]; // in-range: allocated
        *h = h.checked_sub(1).unwrap_or_else(|| Self::dead(i));
        if *h > 0 {
            return None;
        }
        self.free.push(i);
        self.items[i as usize].take() // in-range: allocated
    }
}

macro_rules! exec_table {
    ($($(#[$m:meta])* $kind:ident => $field:ident : $ty:path),* $(,)?) => {
        /// The transport type of `add_execution` (one by-value variant per kind).
        #[derive(Clone, Debug)]
        #[allow(clippy::large_enum_variant)] // transport only: lives in the inbox for microseconds
        pub enum Exec { $($(#[$m])* $kind($ty)),* }

        impl PlainState for Exec {}

        impl Exec {
            pub fn kind(&self) -> ExecKind {
                match self { $($(#[$m])* Exec::$kind(_) => ExecKind::$kind),* }
            }
        }

        /// One [`Slots`] per kind (§4.2). As a `Refs` field it visits every allocated slot:
        /// each one is listed or pinned, i.e. reachable (GC roots, §3.11).
        #[derive(StateClone, Debug, Default)]
        pub struct ExecArenas { $($(#[$m])* pub $field: Slots<$ty>),* }

        impl ExecArenas {
            /// Move `e` into its arena (one hold); returns its id.
            pub fn insert(&mut self, e: Exec) -> ExecId {
                match e { $($(#[$m])* Exec::$kind(x) => ExecId { kind: ExecKind::$kind, idx: self.$field.alloc(x) }),* }
            }
            /// `e.isActive()` without the state ([`ExecBehavior::is_active`]).
            pub fn is_active(&self, id: ExecId) -> bool {
                match id.kind { $($(#[$m])* ExecKind::$kind => self.$field.get(id.idx).is_active()),* }
            }
            /// `e.isActive()` as the driver evaluates it ([`ExecBehavior::is_active_in`]).
            pub fn is_active_in(&self, id: ExecId, st: &State) -> bool {
                match id.kind { $($(#[$m])* ExecKind::$kind => self.$field.get(id.idx).is_active_in(st)),* }
            }
            /// `e.activeDuringSpawnPhase()`.
            pub fn active_during_spawn_phase(&self, id: ExecId) -> bool {
                match id.kind { $($(#[$m])* ExecKind::$kind => self.$field.get(id.idx).active_during_spawn_phase()),* }
            }
            /// `e.init(mg, ticks)`.
            pub fn init(&mut self, id: ExecId, cx: &mut Ctx, ticks: i32) {
                match id.kind { $($(#[$m])* ExecKind::$kind => self.$field.get_mut(id.idx).init(cx, ticks)),* }
            }
            /// `e.tick(ticks)`.
            pub fn tick(&mut self, id: ExecId, cx: &mut Ctx, ticks: i32) {
                match id.kind { $($(#[$m])* ExecKind::$kind => self.$field.get_mut(id.idx).tick(cx, ticks)),* }
            }
            /// Whether `id` names an allocated slot.
            pub fn is_allocated(&self, id: ExecId) -> bool {
                match id.kind { $($(#[$m])* ExecKind::$kind => self.$field.is_allocated(id.idx)),* }
            }
            /// Hold count of a slot (1 per listing + 1 per listing MIRV).
            pub fn holds(&self, id: ExecId) -> u16 {
                match id.kind { $($(#[$m])* ExecKind::$kind => self.$field.holds.get(id.idx as usize).copied().unwrap_or(0)),* }
            }
            /// Add a hold (MIRV pin).
            pub fn hold(&mut self, id: ExecId) {
                match id.kind { $($(#[$m])* ExecKind::$kind => self.$field.hold(id.idx)),* }
            }
            /// Number of allocated executions over all kinds.
            pub fn len(&self) -> usize {
                let n = 0;
                $($(#[$m])* let n = n + self.$field.len();)*
                n
            }
            pub fn is_empty(&self) -> bool {
                self.len() == 0
            }
            /// Report the handles execution `id` holds (CS drain loop, SPEC §1.2.1; P13 S1).
            pub fn visit_refs_of(&self, id: ExecId, v: &mut dyn crate::canon::RefVisitor) {
                match id.kind { $($(#[$m])* ExecKind::$kind => crate::canon::Refs::visit_refs(self.$field.get(id.idx), v)),* }
            }
            /// Drop one hold of `id`. When the slot is freed and it was a MIRV, the MIRV's holds on
            /// its listed warheads are dropped too (TS: the MIRV object becomes garbage, so the
            /// warheads stay reachable only through `execs`/`unInitExecs`).
            #[allow(unreachable_patterns)] // the Mirv/Probe arms precede the generated ones
            pub fn release(&mut self, id: ExecId) {
                let warheads: Vec<ExecId> = match id.kind {
                    ExecKind::Mirv => match self.mirv.release(id.idx) {
                        Some(m) => bound_warheads(&m.warhead_executions),
                        None => return,
                    },
                    #[cfg(test)]
                    ExecKind::Probe => match self.probe.release(id.idx) {
                        Some(p) => bound_warheads(&p.warheads),
                        None => return,
                    },
                    $($(#[$m])* ExecKind::$kind => { let _ = self.$field.release(id.idx); return; }),*
                };
                for w in warheads {
                    self.release(w);
                }
            }
        }
    };
}

/// The bound warhead ids of a MIRV's list (only `Id` entries hold a slot).
fn bound_warheads(list: &[WarheadRef]) -> Vec<ExecId> {
    list.iter().filter_map(|w| if let WarheadRef::Id(id) = w { Some(*id) } else { None }).collect()
}

exec_table! {
    AllianceExtension => alliance_extension: alliance::extension::AllianceExtensionExec,
    AllianceReject => alliance_reject: alliance::reject::AllianceRejectExec,
    AllianceRequest => alliance_request: alliance::request::AllianceRequestExec,
    Attack => attack: attack::AttackExec,
    BoatRetreat => boat_retreat: boat_retreat::BoatRetreatExec,
    BreakAlliance => break_alliance: alliance::break_::BreakAllianceExec,
    City => city: city::CityExec,
    Construction => construction: construction::ConstructionExec,
    DefensePost => defense_post: defense_post::DefensePostExec,
    DeleteUnit => delete_unit: delete_unit::DeleteUnitExec,
    DonateGold => donate_gold: donate_gold::DonateGoldExec,
    DonateTroops => donate_troops: donate_troops::DonateTroopsExec,
    DoomsdayClock => doomsday_clock: doomsday::DoomsdayClockExec,
    EmbargoAll => embargo_all: embargo_all::EmbargoAllExec,
    Embargo => embargo: embargo::EmbargoExec,
    Emoji => emoji: emoji::EmojiExec,
    Factory => factory: factory::FactoryExec,
    MarkDisconnected => mark_disconnected: mark_disconnected::MarkDisconnectedExec,
    Mirv => mirv: mirv::MirvExec,
    MissileSilo => missile_silo: missile_silo::MissileSiloExec,
    MoveWarship => move_warship: move_warship::MoveWarshipExec,
    Nation => nation: nation::NationExec,
    NoOp => noop: noop::NoOpExec,
    Nuke => nuke: nuke::NukeExec,
    Pause => pause: pause::PauseExec,
    Player => player: player::PlayerExec,
    Port => port: port::PortExec,
    QuickChat => quick_chat: quick_chat::QuickChatExec,
    RecomputeRailCluster => recompute_rail_cluster: rail_cluster::RecomputeRailClusterExec,
    Retreat => retreat: retreat::RetreatExec,
    SamLauncher => sam_launcher: sam_launcher::SamLauncherExec,
    SamMissile => sam_missile: sam_missile::SamMissileExec,
    Shell => shell: shell::ShellExec,
    Spawn => spawn: spawn::SpawnExec,
    SpawnTimer => spawn_timer: spawn_timer::SpawnTimerExec,
    TargetPlayer => target_player: target_player::TargetPlayerExec,
    TradeShip => trade_ship: trade_ship::TradeShipExec,
    Train => train: train::TrainExec,
    TrainStation => train_station: train_station::TrainStationExec,
    TransportShip => transport_ship: transport_ship::TransportShipExec,
    Tribe => tribe: tribe::TribeExec,
    UpgradeStructure => upgrade_structure: upgrade_structure::UpgradeStructureExec,
    Warship => warship: warship::WarshipExec,
    WinCheck => win_check: win_check::WinCheckExec,
    #[cfg(test)]
    Probe => probe: probe::ProbeExec,
}

impl ExecArenas {
    /// The warhead list of the MIRV `mirv` (`warheadExecutions`). Any other kind is a Rust bug.
    fn warheads_mut(&mut self, mirv: ExecId) -> &mut Vec<WarheadRef> {
        match mirv.kind {
            ExecKind::Mirv => &mut self.mirv.get_mut(mirv.idx).warhead_executions,
            #[cfg(test)]
            ExecKind::Probe => &mut self.probe.get_mut(mirv.idx).warheads,
            k => panic!("of-engine: BindWarhead from a {k:?} execution (Rust bug: only MIRV binds warheads)"),
        }
    }
    /// `warhead.cancel()` (`NukeExecution.ts:510-515`).
    fn cancel_warhead(&mut self, w: ExecId, cx: &mut Ctx) {
        match w.kind {
            ExecKind::Nuke => self.nuke.get_mut(w.idx).cancel(cx),
            #[cfg(test)]
            ExecKind::Probe => self.probe.get_mut(w.idx).cancel(),
            k => panic!("of-engine: CancelNuke on a {k:?} execution (Rust bug: warheads are NukeExecutions)"),
        }
    }
}

/// TS `GameImpl.execs` + the arenas (§4.2). Disjoint from `State` while an execution runs.
#[derive(StateClone, Debug, Default)]
pub struct ExecStore {
    /// `GameImpl.execs` (`:141`), in tick order. `Copy` elements: cloning is a memcpy.
    pub live: Vec<ExecId>,
    pub ar: ExecArenas,
    /// Double buffers of the init pass (no per-tick allocation); empty at boundaries.
    #[refs(skip)]
    pub buf_inited: Vec<ExecId>,
    #[refs(skip)]
    pub buf_uninited: Vec<ExecId>,
    /// The ids the last [`ExecStore::drain_inbox`] assigned, by inbox index (`PendingExec`).
    #[refs(skip)]
    drained: Vec<ExecId>,
    /// Commands being applied (swapped out of `State.exec_cmds`).
    #[refs(skip)]
    cmd_buf: Vec<ExecCmd>,
    /// The execution whose `tick`/`init` is running (the issuer of `State.exec_cmds`); `None`
    /// between execution calls.
    #[refs(skip)]
    pub running: Option<ExecId>,
}

impl ExecStore {
    /// Move every `State.exec_inbox` entry into its arena, in push order, appending its id to
    /// `State.uninit` (§4.2). Nothing reads `uninit` between a push and this drain, so the
    /// resulting order equals TS's direct `unInitExecs.push` (`GameImpl.ts:787-789`).
    pub fn drain_inbox(&mut self, st: &mut State) {
        self.drained.clear();
        for e in st.exec_inbox.drain(..) {
            let id = self.ar.insert(e);
            st.uninit.push(id);
            self.drained.push(id);
        }
    }

    /// Apply `State.exec_cmds`, queued by the execution `issuer` whose call just returned and
    /// whose inbox was just drained (§4.3). The queue is taken first, so a throw while applying
    /// drops the remaining commands, as a TS throw inside MIRV's cancel loop skips the rest.
    pub fn apply_exec_cmds(&mut self, cx: &mut Ctx, issuer: ExecId) {
        self.cmd_buf.clear();
        std::mem::swap(&mut self.cmd_buf, &mut cx.st.exec_cmds);
        for i in 0..self.cmd_buf.len() {
            let cmd = self.cmd_buf[i]; // in-range: i < len
            match cmd {
                ExecCmd::BindWarhead { k, pending } => {
                    let id = self.resolve(WarheadRef::Pending(pending));
                    let list = self.ar.warheads_mut(issuer);
                    match list.get_mut(k as usize) {
                        Some(w) if *w == WarheadRef::Pending(pending) => *w = WarheadRef::Id(id),
                        other => panic!("of-engine: BindWarhead k={k} {pending:?} found {other:?} (Rust bug)"),
                    }
                    self.ar.hold(id);
                }
                ExecCmd::CancelNuke(w) => {
                    let id = self.resolve(w);
                    self.ar.cancel_warhead(id, cx);
                }
                ExecCmd::Unpin(w) => {
                    let id = self.resolve(w);
                    self.ar.release(id);
                }
            }
        }
        self.cmd_buf.clear();
    }

    /// A warhead reference as an id (`Pending` resolves through the last drain).
    fn resolve(&self, w: WarheadRef) -> ExecId {
        match w {
            WarheadRef::Id(id) => id,
            WarheadRef::Pending(p) => match self.drained.get(p.0 as usize) {
                Some(&id) => id,
                None => panic!("of-engine: {p:?} not in the last inbox drain (Rust bug)"),
            },
        }
    }

    /// After an execution call returned: drain its inbox pushes, then apply its commands.
    #[inline]
    pub fn after_exec_call(&mut self, cx: &mut Ctx, issuer: ExecId) {
        self.drain_inbox(cx.st);
        if !cx.st.exec_cmds.is_empty() {
            self.apply_exec_cmds(cx, issuer);
        }
    }

    /// `GameImpl.removeInactiveExecutions` (`:757-773`): compact `live` in place, order kept.
    /// In the spawn phase it keeps `!activeDuringSpawnPhase() || isActive()`, otherwise
    /// `isActive()` (`inSpawnPhase()` read once, `:761`). A removed execution drops its listing
    /// hold (the slot is freed unless a MIRV still lists it, §4.3).
    pub fn remove_inactive_executions(&mut self, st: &State) {
        let in_spawn_phase = st.in_spawn_phase(); // TS: GameImpl.ts:761
        let mut w = 0;
        for i in 0..self.live.len() {
            let id = self.live[i]; // in-range: i < len
            let keep = if in_spawn_phase {
                !self.ar.active_during_spawn_phase(id) || self.ar.is_active_in(id, st)
            } else {
                self.ar.is_active_in(id, st)
            };
            if keep {
                self.live[w] = id; // in-range: w <= i
                w += 1;
            } else {
                self.ar.release(id);
            }
        }
        self.live.truncate(w); // TS: execs.length = w
    }
}

impl State {
    /// `mg.addExecution(exec)` (`GameImpl.ts:787-789`): pushes to the inbox (drained right after
    /// the current `tick`/`init` returns, so the resulting `uninit` order equals TS's direct
    /// push). Returns the inbox index (MIRV binds warheads with it, §4.3).
    pub fn add_execution(&mut self, e: Exec) -> PendingExec {
        #[allow(clippy::expect_used)] // rust-invariant: the inbox is drained after every exec call
        let k = u32::try_from(self.exec_inbox.len()).expect("exec inbox fits u32");
        self.exec_inbox.push(e);
        PendingExec(k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::Scratch;

    #[test]
    fn noop_dispatch_and_inbox_order() {
        let mut st = State::default();
        let mut sc = Scratch::new();
        assert_eq!(st.add_execution(Exec::NoOp(noop::NoOpExec::new())), PendingExec(0));
        assert_eq!(st.add_execution(Exec::NoOp(noop::NoOpExec::new())), PendingExec(1));
        assert_eq!(st.exec_inbox[0].kind(), ExecKind::NoOp);
        let mut ex = ExecStore::default();
        let id = ex.ar.insert(st.exec_inbox.remove(0));
        assert_eq!(id, ExecId { kind: ExecKind::NoOp, idx: 0 });
        assert!(!ex.ar.is_active(id) && !ex.ar.active_during_spawn_phase(id));
        let mut cx = Ctx::new(&mut st, &mut sc, None);
        ex.ar.init(id, &mut cx, 0);
        ex.ar.tick(id, &mut cx, 0);
        // LIFO slot reuse after the last hold is released; a fresh slot has one hold.
        assert_eq!(ex.ar.holds(id), 1);
        ex.ar.release(id);
        assert!(!ex.ar.is_allocated(id));
        let id2 = ex.ar.insert(Exec::NoOp(noop::NoOpExec::new()));
        assert_eq!(id2, id);
        assert_eq!(ex.ar.holds(id2), 1);
    }

    #[test]
    fn spawn_phase_flags_match_ts() {
        // `activeDuringSpawnPhase()` is `true` only for Nation, Pause, Spawn, SpawnTimer (grep of
        // every execution file at the pin); spot-check through the arena dispatch.
        let mut ar = ExecArenas::default();
        let st = ar.insert(Exec::SpawnTimer(spawn_timer_value()));
        assert!(ar.active_during_spawn_phase(st));
    }

    // A SpawnTimerExec value without its (S1) constructor.
    fn spawn_timer_value() -> spawn_timer::SpawnTimerExec {
        spawn_timer::SpawnTimerExec { initialized: false }
    }

    #[test]
    fn game_default_clones_with_reuse() {
        let mut a = crate::Game::default();
        a.state.ticks = 7;
        a.state.uninit.push(ExecId { kind: ExecKind::NoOp, idx: 3 });
        let mut b = crate::Game::default();
        b.state.uninit.reserve(64);
        let p = b.state.uninit.as_ptr();
        a.clone_into(&mut b);
        assert_eq!(b.state.ticks, 7);
        assert_eq!(b.state.uninit, a.state.uninit);
        assert_eq!(b.state.uninit.as_ptr(), p, "clone_from reuses the Vec buffer");
    }
}
