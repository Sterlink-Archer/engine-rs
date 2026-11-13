//! `Ctx` / `Rcx`: what every execution and state method receives instead of a bare
//! `&mut State` (ARCHITECTURE §3.8, PORTING_GUIDE §1).
//!
//! - `cx: &mut Ctx` is the `tick`/`init` argument and the argument of every **mutating**
//!   cross-package method: state + scratch + the optional caller-owned tile journal.
//! - `rcx: &mut Rcx` is the read-only-state variant for queries that need scratch
//!   (legality oracles, path queries).
//! - Methods that need neither scratch nor mutation take `&State`.
//!
//! The frozen S0 signature of each method fixes which form it uses (FILE_MAP §2). Never
//! hold a `&mut Player`/`&mut Unit` across a call that takes `cx`; re-borrow per statement.
//!
//! The TS-throw helpers are re-exported here so ported code needs one import:
//! `use crate::ctx::*` brings `Ctx`, `Rcx`, `TsExpect`, `ts_throw!`.
//!
//! Owner: **P1** (FILE_MAP §2 foundations).

use crate::ids::{Owner, SmallId, TileRef};
use crate::scratch::Scratch;
use crate::state::State;

pub use of_det::throw::{TsErrorKind, TsExpect, TsThrow};
pub use of_det::ts_throw;

/// Optional changed-tile journal, owned by the caller and passed in through `Ctx`; never
/// part of `State`, so never cloned (ARCHITECTURE §3.1, §10, review 1 #10). It replaces
/// TS's `recordTileUpdate` output buffers (`GameImpl.ts:667-700`).
#[derive(Debug, Default, Clone)]
pub struct TileJournal {
    /// Tiles whose owner/fallout/defense state was written, in write order (may repeat).
    pub changed: Vec<TileRef>,
}

impl TileJournal {
    #[inline]
    pub fn record(&mut self, t: TileRef) {
        self.changed.push(t);
    }
    pub fn clear(&mut self) {
        self.changed.clear();
    }
}

/// Mutable context: state + scratch + optional journal.
pub struct Ctx<'a> {
    pub st: &'a mut State,
    pub sc: &'a mut Scratch,
    pub jr: Option<&'a mut TileJournal>,
}

/// Read-only state + scratch.
pub struct Rcx<'a> {
    pub st: &'a State,
    pub sc: &'a mut Scratch,
}

impl<'a> Ctx<'a> {
    pub fn new(st: &'a mut State, sc: &'a mut Scratch, jr: Option<&'a mut TileJournal>) -> Self {
        Ctx { st, sc, jr }
    }
    /// Shorter-lived copy of this context (to pass `&mut Ctx` down while keeping `self`).
    #[inline]
    pub fn reborrow(&mut self) -> Ctx<'_> {
        Ctx { st: &mut *self.st, sc: &mut *self.sc, jr: self.jr.as_deref_mut() }
    }
    /// D2: `st.player(me).sharesBorderWith(other)` through the witness hints of the scratch (the same answer as the scan).
    #[inline]
    pub fn shares_border(&mut self, me: SmallId, other: Owner) -> bool {
        self.st.player(me).shares_border_with_hinted(other, &mut self.sc.border_hints)
    }
    /// The read-only view with scratch, for queries.
    #[inline]
    pub fn rcx(&mut self) -> Rcx<'_> {
        Rcx { st: &*self.st, sc: &mut *self.sc }
    }
    /// Journal a tile-state write (no-op without a journal).
    #[inline]
    pub fn journal_tile(&mut self, t: TileRef) {
        if let Some(j) = self.jr.as_deref_mut() {
            j.record(t);
        }
    }
}

impl<'a> Rcx<'a> {
    pub fn new(st: &'a State, sc: &'a mut Scratch) -> Self {
        Rcx { st, sc }
    }
    #[inline]
    pub fn reborrow(&mut self) -> Rcx<'_> {
        Rcx { st: self.st, sc: &mut *self.sc }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reborrow_and_journal() {
        let mut st = State::default();
        let mut sc = Scratch::new();
        let mut jr = TileJournal::default();
        let mut cx = Ctx::new(&mut st, &mut sc, Some(&mut jr));
        {
            let mut inner = cx.reborrow();
            inner.st.ticks += 1;
            inner.journal_tile(7);
        }
        let r = cx.rcx();
        assert_eq!(r.st.ticks, 1);
        cx.journal_tile(8);
        assert_eq!(jr.changed, [7, 8]);
        let mut cx2 = Ctx::new(&mut st, &mut sc, None);
        cx2.journal_tile(9); // no journal: no-op
    }
}
