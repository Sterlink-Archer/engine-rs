//! Opt-in wall-clock probes for profiling (cargo feature `prof`; compiled out of every default
//! build, conformance and throughput-oriented builds included). They only read the monotonic clock into
//! thread-local counters and never touch simulation state, so they cannot change results.
//!
//! What is measured, per game phase (`PHASES`, chosen from the tick being executed):
//! - `tick_ns[kind]` / `tick_calls[kind]`: `GameImpl.executeNextTick` tick pass, per execution kind
//!   (one clock read per executed `tick()`; the inactive/spawn-phase skip checks fall into the next
//!   executed one);
//! - `init_ns[kind]` / `init_calls[kind]`: the init pass, per kind;
//! - `sec_ns[SEC_*]`: the other parts of `Game::step` (intent→exec creation, compaction, per-player
//!   bookkeeping, water, maintenance, GC, and the whole step).
//!
//! The caller drains the current thread's counters with [`take`] (of-rl does it after every env step
//! and keys them by map / env).
//!
//! Owner: **P1** (runner probes; t409).
#![allow(clippy::disallowed_types)] // Cell: profiling counters, never simulation state (module doc)

use crate::ids::ExecKind;
use std::cell::RefCell;
use std::time::Instant;

pub const PHASES: [&str; 4] = ["spawn", "opening", "mid", "late"];
/// Tick bounds of the phases after spawn: opening < 1500 <= mid < 4000 <= late (1v1 SP1 episodes:
/// median ~5.4k ticks, cap 6.2k; results/perf/live).
pub const PHASE_TICKS: [i32; 2] = [1500, 4000];
/// Sections 8.. are nested probes inside the tick pass (t409 H5: `removeClusters` parts) and
/// `n.*` entries are event counts, not ns.
pub const SECTIONS: [&str; 20] = [
    "createExecs",
    "tickPass",
    "initPass",
    "removeInactive",
    "bookkeeping",
    "water",
    "maintenance",
    "gc",
    "rc.calc",
    "rc.pinchDetect",
    "rc.flood",
    "rc.passes",
    "n.rcCalls",
    "n.rcConn4",
    "n.rcClusterTiles",
    "n.rcFloodTiles",
    "n.rcFloodSingle",
    "n.rcFloodSingleTiles",
    "n.rcSmall",
    "n.rcComps",
];
pub const SEC_CREATE: usize = 0;
pub const SEC_TICK: usize = 1;
pub const SEC_INIT: usize = 2;
pub const SEC_REMOVE: usize = 3;
pub const SEC_BOOK: usize = 4;
pub const SEC_WATER: usize = 5;
pub const SEC_MAINT: usize = 6;
pub const SEC_GC: usize = 7;
pub const SEC_RC_CALC: usize = 8;
pub const SEC_RC_PINCH: usize = 9;
pub const SEC_RC_FLOOD: usize = 10;
pub const SEC_RC_PASSES: usize = 11;
pub const N_RC_CALLS: usize = 12;
pub const N_RC_CONN4: usize = 13;
pub const N_RC_CLUSTER_TILES: usize = 14;
pub const N_RC_FLOOD_TILES: usize = 15;
/// Full floods that found one component holding every owned tile (conn4 was lost to a rule
/// failure, not to a real split), and their tiles.
pub const N_RC_FLOOD_SINGLE: usize = 16;
pub const N_RC_FLOOD_SINGLE_TILES: usize = 17;
/// removeClusters calls of players with < 100 tiles (they run every tick).
pub const N_RC_SMALL: usize = 18;
/// removeClusters calls served by the component labels (conn4 false, labels valid).
pub const N_RC_COMPS: usize = 19;
const K: usize = ExecKind::COUNT;
const NP: usize = PHASES.len();
const NS: usize = SECTIONS.len();

#[derive(Clone, Debug)]
pub struct Acc {
    pub tick_ns: [[u64; K]; NP],
    pub tick_calls: [[u64; K]; NP],
    pub init_ns: [[u64; K]; NP],
    pub init_calls: [[u64; K]; NP],
    pub sec_ns: [[u64; NS]; NP],
    /// `Game::step` calls (ticks) and their total wall time, per phase.
    pub steps: [u64; NP],
    pub step_ns: [u64; NP],
}

impl Default for Acc {
    fn default() -> Self {
        Acc {
            tick_ns: [[0; K]; NP],
            tick_calls: [[0; K]; NP],
            init_ns: [[0; K]; NP],
            init_calls: [[0; K]; NP],
            sec_ns: [[0; NS]; NP],
            steps: [0; NP],
            step_ns: [0; NP],
        }
    }
}

impl Acc {
    pub fn add(&mut self, o: &Acc) {
        for p in 0..NP {
            for k in 0..K {
                self.tick_ns[p][k] += o.tick_ns[p][k];
                self.tick_calls[p][k] += o.tick_calls[p][k];
                self.init_ns[p][k] += o.init_ns[p][k];
                self.init_calls[p][k] += o.init_calls[p][k];
            }
            for s in 0..NS {
                self.sec_ns[p][s] += o.sec_ns[p][s];
            }
            self.steps[p] += o.steps[p];
            self.step_ns[p] += o.step_ns[p];
        }
    }
}

thread_local! {
    static ACC: RefCell<Acc> = RefCell::new(Acc::default());
    /// The phase of the tick being executed (for nested probes that do not know it).
    static CUR_PHASE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static EPOCH: Instant = Instant::now();
}

/// Monotonic ns since this thread's first probe (differences only).
#[inline(always)]
pub fn now() -> u64 {
    EPOCH.with(|e| e.elapsed().as_nanos() as u64)
}

#[inline(always)]
pub fn phase(in_spawn: bool, tick: i32) -> usize {
    if in_spawn {
        0
    } else if tick < PHASE_TICKS[0] {
        1
    } else if tick < PHASE_TICKS[1] {
        2
    } else {
        3
    }
}

#[inline(always)]
pub fn tick(p: usize, k: ExecKind, ns: u64) {
    ACC.with(|a| {
        let k = k as usize;
        if k < K {
            let mut a = a.borrow_mut();
            a.tick_ns[p][k] += ns;
            a.tick_calls[p][k] += 1;
        }
    });
}

#[inline(always)]
pub fn init(p: usize, k: ExecKind, ns: u64) {
    ACC.with(|a| {
        let k = k as usize;
        if k < K {
            let mut a = a.borrow_mut();
            a.init_ns[p][k] += ns;
            a.init_calls[p][k] += 1;
        }
    });
}

#[inline(always)]
pub fn sec(p: usize, s: usize, ns: u64) {
    ACC.with(|a| a.borrow_mut().sec_ns[p][s] += ns);
}

/// Set the phase nested probes ([`nested`]) are filed under.
#[inline(always)]
pub fn set_phase(p: usize) {
    CUR_PHASE.with(|c| c.set(p));
}

/// A nested section time or an `n.*` count, filed under the current tick's phase.
#[inline(always)]
pub fn nested(s: usize, v: u64) {
    let p = CUR_PHASE.with(|c| c.get());
    ACC.with(|a| a.borrow_mut().sec_ns[p][s] += v);
}

#[inline(always)]
pub fn step(p: usize, ns: u64) {
    ACC.with(|a| {
        let mut a = a.borrow_mut();
        a.steps[p] += 1;
        a.step_ns[p] += ns;
    });
}

/// Drain this thread's counters.
pub fn take() -> Acc {
    ACC.with(|a| std::mem::take(&mut *a.borrow_mut()))
}

/// Exec kind names in `ExecKind as usize` order.
pub fn kind_names() -> Vec<&'static str> {
    ExecKind::ALL.iter().map(|k| k.as_str()).collect()
}
