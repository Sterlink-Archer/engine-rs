//! `ProbeExec`: a unit-test-only scripted execution (`#[cfg(test)]`, not one of the 44 kinds).
//! Owner: **P1**. It drives the tick loop through the TS semantics the S0 tests check:
//! inbox order, init over a live-growing `uninit`, spawn-phase gating, post-throw state and
//! MIRV-style warhead pinning (ARCHITECTURE §4.1-4.6).
//!
//! Every call is logged as `"<call> <tag>@<tick>"` (`init`, `tick`, `cancel`) in a per-thread
//! log, then the per-thread hook (if any) runs with the execution and the context.
#![allow(clippy::disallowed_types)] // test-only thread-local hook/log (RefCell, Rc)

use crate::exec::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

/// Which call the hook is running in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Init,
    Tick,
}

type Hook = Rc<dyn Fn(&mut ProbeExec, &mut Ctx, Phase, i32)>;

thread_local! {
    static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) };
    static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Install this thread's hook (replacing any previous one) and clear the log.
pub fn set_hook(f: impl Fn(&mut ProbeExec, &mut Ctx, Phase, i32) + 'static) {
    HOOK.with(|h| *h.borrow_mut() = Some(Rc::new(f)));
    LOG.with(|l| l.borrow_mut().clear());
}
/// Remove this thread's hook and clear the log.
pub fn clear_hook() {
    HOOK.with(|h| *h.borrow_mut() = None);
    LOG.with(|l| l.borrow_mut().clear());
}
/// Append to this thread's log.
pub fn log(s: String) {
    LOG.with(|l| l.borrow_mut().push(s));
}
/// Take this thread's log.
pub fn take_log() -> Vec<String> {
    LOG.with(|l| std::mem::take(&mut *l.borrow_mut()))
}

/// A scripted execution.
#[derive(StateClone, Debug, Default)]
pub struct ProbeExec {
    pub tag: u32,
    pub active: bool,
    pub adsp: bool,
    /// MIRV-style warhead list (`ExecCmd::BindWarhead` target).
    pub warheads: Vec<WarheadRef>,
    pub cancelled: bool,
    pub inits: u32,
    pub ticks: u32,
}

impl ProbeExec {
    pub fn new(tag: u32, active: bool, adsp: bool) -> Self {
        ProbeExec { tag, active, adsp, ..Default::default() }
    }
    pub fn exec(tag: u32, active: bool, adsp: bool) -> Exec {
        Exec::Probe(Self::new(tag, active, adsp))
    }
    /// `NukeExecution.cancel()` stand-in.
    pub fn cancel(&mut self) {
        self.active = false;
        self.cancelled = true;
        log(format!("cancel {}", self.tag));
    }
    fn run_hook(&mut self, cx: &mut Ctx, phase: Phase, ticks: i32) {
        if let Some(h) = HOOK.with(|h| h.borrow().clone()) {
            h(self, cx, phase, ticks);
        }
    }
}

impl ExecBehavior for ProbeExec {
    fn is_active(&self) -> bool {
        self.active
    }
    fn active_during_spawn_phase(&self) -> bool {
        self.adsp
    }
    fn init(&mut self, cx: &mut Ctx, ticks: i32) {
        self.inits += 1;
        log(format!("init {}@{ticks}", self.tag));
        self.run_hook(cx, Phase::Init, ticks);
    }
    fn tick(&mut self, cx: &mut Ctx, ticks: i32) {
        self.ticks += 1;
        log(format!("tick {}@{ticks}", self.tag));
        self.run_hook(cx, Phase::Tick, ticks);
    }
}
