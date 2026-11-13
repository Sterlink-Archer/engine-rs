//! The v1 arena GC (ARCHITECTURE §3.11). Owner: **P1** (driver); marking goes through the
//! derived `Refs` impls (`crate::canon::refs`, P13 framework), so a type's owner cannot forget
//! a handle field.
//!
//! Frees objects that are both inactive and unmarked: units (`!active`), attacks
//! (`!isActive`), rail clusters and railroads (no TS liveness flag: unmarked = garbage), and
//! retired water `ChainSlot`s no stepper names. Roots: every `State` field (players' lists,
//! game lists, the rail network's stations and id map, the unit grid, memos; the arenas
//! themselves report nothing) and every allocated execution slot (each is listed in `live` /
//! `uninit` or pinned by a MIRV, §4.3). Marked arena objects are walked in turn (a live unit's
//! `target_unit`, a detached warhead's nuke, a railroad's stations).
//!
//! Unobservable by construction: CS keys and map keys use TS ids, never slots (PORTING_GUIDE
//! §5.1). Runs every [`GC_EVERY_TICKS`] ticks at a tick boundary and at the start of
//! `Game::fork_into`.

use crate::arena::Marks;
use crate::canon::{RefVisitor, Refs};
use crate::exec::ExecStore;
use crate::ids::{ArenaHandle, AttackRef, ClusterRef, ExecId, RailroadRef, UnitRef};
use crate::jsorder::LookupSet;
use crate::state::State;

/// GC cadence (ticks).
pub const GC_EVERY_TICKS: i32 = 32;

/// What one collection freed (tests, fork-bench).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GcStats {
    pub units: usize,
    pub attacks: usize,
    pub clusters: usize,
    pub railroads: usize,
    pub chain_slots: usize,
}

enum Work {
    Unit(UnitRef),
    Attack(AttackRef),
    Cluster(ClusterRef),
    Railroad(RailroadRef),
}

struct Marker {
    units: Marks,
    attacks: Marks,
    clusters: Marks,
    railroads: Marks,
    chain_versions: LookupSet<u32>,
    work: Vec<Work>,
}

/// `true` for the placeholder handle of `Default` state (slot `u32::MAX`, `ids.rs`): nothing to
/// mark.
#[inline]
fn is_null<H: ArenaHandle>(h: H) -> bool {
    h.slot() == u32::MAX
}

impl RefVisitor for Marker {
    fn canon_pass(&self) -> bool {
        false
    }
    fn visit_unit(&mut self, u: UnitRef) {
        if !is_null(u) && !self.units.get(u.slot as usize) {
            self.units.mark(u);
            self.work.push(Work::Unit(u));
        }
    }
    fn visit_attack(&mut self, a: AttackRef) {
        if !is_null(a) && !self.attacks.get(a.slot as usize) {
            self.attacks.mark(a);
            self.work.push(Work::Attack(a));
        }
    }
    fn visit_cluster(&mut self, c: ClusterRef) {
        if !is_null(c) && !self.clusters.get(c.slot as usize) {
            self.clusters.mark(c);
            self.work.push(Work::Cluster(c));
        }
    }
    fn visit_railroad(&mut self, r: RailroadRef) {
        if !is_null(r) && !self.railroads.get(r.slot as usize) {
            self.railroads.mark(r);
            self.work.push(Work::Railroad(r));
        }
    }
    /// Every allocated execution slot is already a root (module docs).
    fn visit_exec(&mut self, _e: ExecId) {}
    fn visit_chain_version(&mut self, version: u32) {
        self.chain_versions.insert(version);
    }
}

impl State {
    /// `collect_garbage(&ExecStore)`: mark from the roots, sweep the arenas (§3.11). Must run
    /// at a tick boundary (empty inbox and command queue). A handle that names a freed slot is
    /// a Rust bug (use after free) and panics.
    pub fn collect_garbage(&mut self, execs: &ExecStore) -> GcStats {
        debug_assert!(self.exec_inbox.is_empty() && self.exec_cmds.is_empty(), "GC inside a tick");
        let mut m = Marker {
            units: Marks::default(),
            attacks: Marks::default(),
            clusters: Marks::default(),
            railroads: Marks::default(),
            chain_versions: LookupSet::new(),
            work: Vec::new(),
        };
        m.units.reset(self.units.slot_count());
        m.attacks.reset(self.attacks.slot_count());
        m.clusters.reset(self.rail.clusters.slot_count());
        m.railroads.reset(self.rail.railroads.slot_count());

        // Roots.
        self.visit_refs(&mut m);
        execs.visit_refs(&mut m);
        // Walk marked arena objects (the get()s panic on a stale handle: a Rust bug).
        while let Some(w) = m.work.pop() {
            match w {
                Work::Unit(u) => self.units.get(u).visit_refs(&mut m),
                Work::Attack(a) => self.attacks.get(a).visit_refs(&mut m),
                Work::Cluster(c) => self.rail.clusters.get(c).visit_refs(&mut m),
                Work::Railroad(r) => self.rail.railroads.get(r).visit_refs(&mut m),
            }
        }

        // Sweep.
        let units = self.units.sweep(&m.units, |u| !u.active);
        let attacks = self.attacks.sweep(&m.attacks, |a| !a.is_active);
        let clusters = self.rail.clusters.sweep(&m.clusters, |_| true);
        let railroads = self.rail.railroads.sweep(&m.railroads, |_| true);
        let retired = &mut self.water.chains.retired;
        let before = retired.len();
        retired.retain(|s| m.chain_versions.contains(&s.version));
        GcStats { units, attacks, clusters, railroads, chain_slots: before - retired.len() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::pathfinder::ChainSlot;
    use crate::state::attack::Attack;
    use crate::state::unit::Unit;

    #[test]
    fn frees_unreferenced_inactive_keeps_referenced() {
        let mut st = State::default();
        let ex = ExecStore::default();
        // u0: inactive, unreferenced → freed. u1: inactive, referenced by u2.target_unit → kept.
        // u2: active, in nothing → kept (only inactive objects are freed).
        let u0 = st.units.alloc(Unit { id: 1, active: false, ..Default::default() });
        let u1 = st.units.alloc(Unit { id: 2, active: false, ..Default::default() });
        let u2 = st.units.alloc(Unit { id: 3, active: true, target_unit: Some(u1), ..Default::default() });
        let a0 = st.attacks.alloc(Attack { is_active: false, ..Default::default() });
        let a1 = st.attacks.alloc(Attack { is_active: false, ..Default::default() });
        // Neither attack is referenced. u2 is reachable through `_unitMap` (a GC-only root).
        st.unit_by_id.insert(3, u2);
        st.water.chains.retired.push(ChainSlot { version: 4, ..Default::default() });
        st.water.chains.retired.push(ChainSlot { version: 5, ..Default::default() });
        let s = st.collect_garbage(&ex);
        assert_eq!(s.units, 1);
        assert!(!st.units.contains(u0) && st.units.contains(u1) && st.units.contains(u2));
        // u2 is active, so it was never a candidate; u1 survives only through u2's target_unit.
        assert_eq!(s.attacks, 2);
        assert!(!st.attacks.contains(a0) && !st.attacks.contains(a1));
        assert_eq!(s.chain_slots, 2, "no stepper names a retired slot");
        // Idempotent.
        assert_eq!(st.collect_garbage(&ex), GcStats::default());
    }
}
