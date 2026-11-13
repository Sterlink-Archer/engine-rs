//! `RefVisitor` / `Refs`: the handle walk shared by the v1 arena GC (ARCHITECTURE §3.11) and
//! the CS v1 drain loop (SPEC §1.2.1, ARCHITECTURE §9). Owner: **P13** (framework).
//!
//! - [`RefVisitor`] is the *visitor*: the GC marker (`state/gc.rs`) and, from S1, the CS
//!   exporter's key registrar. It receives every handle a state object holds.
//! - [`Refs`] is implemented by every *state type*: it reports each handle it holds, in field
//!   order (the snapshot dump order, then non-snapshot fields). `#[derive(StateClone)]`
//!   generates it, so a new field can never be forgotten (FILE_MAP §5 rule 7). Leaves are
//!   implemented here and by `plain_copy!`; handle types report themselves.
//! - Arena contents are **not** fields as far as `Refs` goes: an `Arena`'s `Refs` is a no-op,
//!   and the walkers visit arena objects by handle (GC worklist, CS drain queue). Execution
//!   slots ([`crate::exec::Slots`]) *are* visited: every allocated slot is listed or pinned
//!   (ARCHITECTURE §4.3), so all of them are GC roots.
//!
//! Visitation order matters only on the canon pass (`canon_pass() == true`), where the first
//! reference decides `e<n>` cluster keys and detached-execution keys.

use crate::ids::{
    AllianceId, AttackRef, ClusterRef, ExecId, RailroadId, RailroadRef, ReqIdx, SmallId, StationId, UnitRef,
};
use std::collections::VecDeque;
use std::marker::PhantomData;
use std::sync::{Arc, OnceLock};

/// Receives the handles a state object holds. Defaults are no-ops for handles whose objects
/// are never freed (players, alliances, requests, stations): only the CS pass needs them.
pub trait RefVisitor {
    /// `true` while extracting CS v1 (fields marked `#[refs(gc_only)]` are skipped), `false`
    /// for the GC marker.
    fn canon_pass(&self) -> bool;
    fn visit_unit(&mut self, u: UnitRef);
    fn visit_attack(&mut self, a: AttackRef);
    fn visit_cluster(&mut self, c: ClusterRef);
    fn visit_railroad(&mut self, r: RailroadRef);
    fn visit_exec(&mut self, e: ExecId);
    /// A `WaterPathFinder`/`WaterStepper` naming water chain slot `version` (§3.8): keeps a
    /// retired `ChainSlot` (and its memo) alive.
    fn visit_chain_version(&mut self, version: u32);
    fn visit_player(&mut self, _p: SmallId) {}
    fn visit_alliance(&mut self, _a: AllianceId) {}
    fn visit_alliance_request(&mut self, _r: ReqIdx) {}
    fn visit_station(&mut self, _s: StationId) {}
    fn visit_railroad_id(&mut self, _r: RailroadId) {}
}

/// Report every handle `self` holds to `v`, in field order (see the module docs).
pub trait Refs {
    fn visit_refs(&self, v: &mut dyn RefVisitor);
}

/// `impl Refs` with an empty body (leaf types that hold no handle). `plain_copy!` uses it.
#[macro_export]
macro_rules! refs_leaf {
    ($($t:ty),* $(,)?) => {$(
        impl $crate::canon::Refs for $t {
            #[inline]
            fn visit_refs(&self, _v: &mut dyn $crate::canon::RefVisitor) {}
        }
    )*};
}

// ---- handle types ----
impl Refs for UnitRef {
    #[inline]
    fn visit_refs(&self, v: &mut dyn RefVisitor) {
        v.visit_unit(*self)
    }
}
impl Refs for AttackRef {
    #[inline]
    fn visit_refs(&self, v: &mut dyn RefVisitor) {
        v.visit_attack(*self)
    }
}
impl Refs for ClusterRef {
    #[inline]
    fn visit_refs(&self, v: &mut dyn RefVisitor) {
        v.visit_cluster(*self)
    }
}
impl Refs for RailroadRef {
    #[inline]
    fn visit_refs(&self, v: &mut dyn RefVisitor) {
        v.visit_railroad(*self)
    }
}
impl Refs for ExecId {
    #[inline]
    fn visit_refs(&self, v: &mut dyn RefVisitor) {
        v.visit_exec(*self)
    }
}
impl Refs for SmallId {
    #[inline]
    fn visit_refs(&self, v: &mut dyn RefVisitor) {
        v.visit_player(*self)
    }
}
impl Refs for AllianceId {
    #[inline]
    fn visit_refs(&self, v: &mut dyn RefVisitor) {
        v.visit_alliance(*self)
    }
}
impl Refs for ReqIdx {
    #[inline]
    fn visit_refs(&self, v: &mut dyn RefVisitor) {
        v.visit_alliance_request(*self)
    }
}
impl Refs for StationId {
    #[inline]
    fn visit_refs(&self, v: &mut dyn RefVisitor) {
        v.visit_station(*self)
    }
}
impl Refs for RailroadId {
    #[inline]
    fn visit_refs(&self, v: &mut dyn RefVisitor) {
        v.visit_railroad_id(*self)
    }
}

// ---- leaves and generic containers ----
refs_leaf!(String, str, Box<str>, of_det::Prng, of_det::heap::FlatBinaryHeap, of_det::ordmap::Cursor);

impl<T> Refs for PhantomData<T> {
    #[inline]
    fn visit_refs(&self, _v: &mut dyn RefVisitor) {}
}
impl<T: Refs> Refs for [T] {
    fn visit_refs(&self, v: &mut dyn RefVisitor) {
        for x in self {
            x.visit_refs(v);
        }
    }
}
impl<T: Refs, const N: usize> Refs for [T; N] {
    fn visit_refs(&self, v: &mut dyn RefVisitor) {
        self.as_slice().visit_refs(v)
    }
}
impl<T: Refs> Refs for Vec<T> {
    fn visit_refs(&self, v: &mut dyn RefVisitor) {
        self.as_slice().visit_refs(v)
    }
}
impl<T: Refs> Refs for VecDeque<T> {
    fn visit_refs(&self, v: &mut dyn RefVisitor) {
        for x in self {
            x.visit_refs(v);
        }
    }
}
impl<T: Refs> Refs for Box<[T]> {
    fn visit_refs(&self, v: &mut dyn RefVisitor) {
        (**self).visit_refs(v)
    }
}
impl<T: Refs> Refs for Option<T> {
    #[inline]
    fn visit_refs(&self, v: &mut dyn RefVisitor) {
        if let Some(x) = self {
            x.visit_refs(v);
        }
    }
}
impl<T: ?Sized + Refs> Refs for Arc<T> {
    #[inline]
    fn visit_refs(&self, v: &mut dyn RefVisitor) {
        (**self).visit_refs(v)
    }
}
impl<T: Refs> Refs for OnceLock<T> {
    fn visit_refs(&self, v: &mut dyn RefVisitor) {
        if let Some(x) = self.get() {
            x.visit_refs(v);
        }
    }
}
impl<A: Refs, B: Refs> Refs for (A, B) {
    fn visit_refs(&self, v: &mut dyn RefVisitor) {
        self.0.visit_refs(v);
        self.1.visit_refs(v);
    }
}
impl<A: Refs, B: Refs, C: Refs> Refs for (A, B, C) {
    fn visit_refs(&self, v: &mut dyn RefVisitor) {
        self.0.visit_refs(v);
        self.1.visit_refs(v);
        self.2.visit_refs(v);
    }
}
impl<A: Refs, B: Refs, C: Refs, D: Refs> Refs for (A, B, C, D) {
    fn visit_refs(&self, v: &mut dyn RefVisitor) {
        self.0.visit_refs(v);
        self.1.visit_refs(v);
        self.2.visit_refs(v);
        self.3.visit_refs(v);
    }
}
/// JS insertion order: keys then value per entry (a `Map<Unit, ..>` key is visited first).
impl<K: Refs + std::hash::Hash + Eq + Clone, V: Refs, S: std::hash::BuildHasher + Default, const N: usize> Refs
    for of_det::ordmap::OrdMap<K, V, S, N>
{
    fn visit_refs(&self, v: &mut dyn RefVisitor) {
        for (k, x) in self.iter() {
            k.visit_refs(v);
            x.visit_refs(v);
        }
    }
}
impl<K: Refs + std::hash::Hash + Eq + Clone, S: std::hash::BuildHasher + Default, const N: usize> Refs
    for of_det::ordmap::OrdSet<K, S, N>
{
    fn visit_refs(&self, v: &mut dyn RefVisitor) {
        for k in self.iter() {
            k.visit_refs(v);
        }
    }
}

/// A [`RefVisitor`] that records every handle in visit order (tests and debugging).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RefLog {
    pub canon: bool,
    pub units: Vec<UnitRef>,
    pub attacks: Vec<AttackRef>,
    pub clusters: Vec<ClusterRef>,
    pub railroads: Vec<RailroadRef>,
    pub execs: Vec<ExecId>,
    pub chain_versions: Vec<u32>,
    pub players: Vec<SmallId>,
}

impl RefVisitor for RefLog {
    fn canon_pass(&self) -> bool {
        self.canon
    }
    fn visit_unit(&mut self, u: UnitRef) {
        self.units.push(u)
    }
    fn visit_attack(&mut self, a: AttackRef) {
        self.attacks.push(a)
    }
    fn visit_cluster(&mut self, c: ClusterRef) {
        self.clusters.push(c)
    }
    fn visit_railroad(&mut self, r: RailroadRef) {
        self.railroads.push(r)
    }
    fn visit_exec(&mut self, e: ExecId) {
        self.execs.push(e)
    }
    fn visit_chain_version(&mut self, version: u32) {
        self.chain_versions.push(version)
    }
    fn visit_player(&mut self, p: SmallId) {
        self.players.push(p)
    }
}
