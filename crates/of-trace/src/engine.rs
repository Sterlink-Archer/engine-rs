//! The pluggable engine interface.
//!
//! A native port implements [`Engine`]. The conformance loop ([`crate::conform`]) calls
//! `start` once (state S_0), then for t = 0..ticks: `step(turn_t)` (S_t -> S_{t+1}) and
//! asks for the K digest every kStride ticks and the full section digests at every
//! section checkpoint. Digests are separate calls rather than the return value of `step`
//! so a fast engine computes the expensive full D only where the trace has one
//! (SPEC §2.4: K every tick, D every secEvery ticks).
//!
//! The default digest methods derive everything from [`Engine::canonical_state`] through
//! `of-canon`, so an engine only has to produce the CS v1 tree; it can override them with
//! a streaming implementation later without changing the loop.

use crate::trace::Trace;
use anyhow::Result;
use of_canon::Cs;
use of_canon::digest::{self, CanonState, Digests, Section};

/// Why a step failed.
#[derive(Debug, Clone, PartialEq)]
pub enum StepError {
    /// The engine itself "threw" (the TS engine records these as `scenario.fatal`); the
    /// port must fail at the same step (SPEC §3.1.4).
    Fatal(String),
    /// The port does not implement something yet (counts as a divergence).
    Unimplemented(String),
}

pub trait Engine {
    fn name(&self) -> String;
    /// Engine commit this port targets (checked against `scenario.engine.commit`).
    fn target_commit(&self) -> &str {
        // rules-v36 (t713): v0.36.0 (0211f17b), pin 5b75bd3 + 48 commits.
        "0211f17b71efa762242b620b91ffa5d8439522e6"
    }
    /// Build S_0 from `scenario.json` (`gameStart`, `map`, `start`).
    fn start(&mut self, trace: &Trace) -> Result<(), StepError>;
    /// Consume `turn_t` (the `Turn` object, SPEC §2.3): S_t -> S_{t+1}.
    fn step(&mut self, turn: &Cs) -> Result<(), StepError>;
    /// Current tick t (number of steps taken since S_0).
    fn tick(&self) -> u32;
    /// CS v1 of the current state (full projection). `None` if this engine cannot
    /// produce one (then the digest methods must be overridden).
    fn canonical_state(&mut self) -> Option<CanonState>;
    /// K_t.
    fn key_digest(&mut self) -> Result<u64, StepError> {
        let cs = self.canonical_state().ok_or_else(|| StepError::Unimplemented("canonical_state".into()))?;
        digest::digest_key(&cs).map(|d| d.all).map_err(|e| StepError::Unimplemented(format!("CSE1: {e}")))
    }
    /// D_t and the 11 section digests.
    fn full_digests(&mut self) -> Result<Digests, StepError> {
        let cs = self.canonical_state().ok_or_else(|| StepError::Unimplemented("canonical_state".into()))?;
        digest::digest_full(&cs).map_err(|e| StepError::Unimplemented(format!("CSE1: {e}")))
    }
}

/// Replays the TS-recorded digests as if it were an engine: the smoke test for the whole
/// pipeline before any native engine exists. With `inject` it corrupts one recorded
/// digest, to prove the loop detects and reports a divergence at exactly that tick.
pub struct ReplayEngine {
    k: std::collections::BTreeMap<u32, u64>,
    sec: std::collections::BTreeMap<u32, crate::trace::SecRecord>,
    t: u32,
    pub inject: Option<Inject>,
    turns_seen: usize,
}

/// A deliberate corruption for the replay engine.
#[derive(Clone, Copy, Debug)]
pub enum Inject {
    /// Flip the low bit of K at this tick.
    K(u32),
    /// Flip one section digest (and therefore D) at this checkpoint tick, leaving K intact
    /// — reported as K-blind.
    Section(u32, Section),
    /// Fail with `Fatal` at this step.
    Fatal(u32),
}

impl ReplayEngine {
    pub fn new(trace: &Trace) -> Self {
        ReplayEngine { k: trace.k.clone(), sec: trace.sec.clone(), t: 0, inject: None, turns_seen: 0 }
    }
    pub fn with_inject(mut self, i: Inject) -> Self {
        self.inject = Some(i);
        self
    }
}

impl Engine for ReplayEngine {
    fn name(&self) -> String {
        match self.inject {
            None => "replay".into(),
            Some(i) => format!("replay+inject({i:?})"),
        }
    }
    fn start(&mut self, _trace: &Trace) -> Result<(), StepError> {
        self.t = 0;
        Ok(())
    }
    fn step(&mut self, turn: &Cs) -> Result<(), StepError> {
        if let Some(Inject::Fatal(t)) = self.inject {
            if t == self.t {
                return Err(StepError::Fatal("injected".into()));
            }
        }
        // The turn must be the one for this tick (exercises the reader's ordering).
        if turn.get("turnNumber").and_then(Cs::as_i64) != Some(self.t as i64) {
            return Err(StepError::Fatal(format!("turn out of order at {}", self.t)));
        }
        self.turns_seen += 1;
        self.t += 1;
        Ok(())
    }
    fn tick(&self) -> u32 {
        self.t
    }
    fn canonical_state(&mut self) -> Option<CanonState> {
        None
    }
    fn key_digest(&mut self) -> Result<u64, StepError> {
        let k = *self.k.get(&self.t).ok_or_else(|| StepError::Unimplemented(format!("no recorded K at {}", self.t)))?;
        Ok(match self.inject {
            Some(Inject::K(t)) if t == self.t => k ^ 1,
            _ => k,
        })
    }
    fn full_digests(&mut self) -> Result<Digests, StepError> {
        let r =
            *self.sec.get(&self.t).ok_or_else(|| StepError::Unimplemented(format!("no recorded D at {}", self.t)))?;
        let mut sec = r.sec;
        let mut all = r.d;
        if let Some(Inject::Section(t, s)) = self.inject {
            if t == self.t {
                sec[s as usize] ^= 1;
                all = digest::combine(digest::MAGIC_FULL, self.t, &sec);
            }
        }
        Ok(Digests { tick: self.t, all, sec })
    }
}
