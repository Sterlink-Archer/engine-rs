//! D2: witness hints for `PlayerRef::shares_border_with`.
//!
//! `sharesBorderWith(other)` is a boolean: any border tile of `me` with a 4-neighbour owned by `other` proves `true`. The table
//! remembers, per `(me, other)` pair (direct-mapped, colliding pairs evict each other), the last few border tiles that proved it.
//! A hint is only ever *re-validated against the live state before it is believed* (still in `me`'s border list, still next to a
//! tile of `other`); a stale or foreign hint (the scratch is shared by every game on the thread) fails the validation and the full
//! scan runs, so no invalidation rule exists to get wrong, and a `false` answer always comes from the scan. Not game state, never
//! cloned, never part of CS.
//!
//! Owner: **P1** (scratch foundations; t949 addition).

use super::StepReset;
use crate::ids::TileRef;

const SLOTS: usize = 256;
const WITNESSES: usize = 4;
const NONE: TileRef = TileRef::MAX;

#[derive(Debug)]
pub struct BorderHints {
    /// `(me << 16) | other`, or `u32::MAX` for an empty slot.
    keys: Vec<u32>,
    /// `WITNESSES` tiles per slot, most recent first.
    wit: Vec<[TileRef; WITNESSES]>,
}

impl Default for BorderHints {
    fn default() -> Self {
        BorderHints { keys: vec![u32::MAX; SLOTS], wit: vec![[NONE; WITNESSES]; SLOTS] }
    }
}

impl BorderHints {
    #[inline]
    fn slot(key: u32) -> usize {
        (key.wrapping_mul(0x9E37_79B1) >> 24) as usize // top 8 bits: in 0..SLOTS
    }
    #[inline]
    pub fn key(me: u16, other: u16) -> u32 {
        (u32::from(me) << 16) | u32::from(other)
    }
    /// The remembered witnesses of `key` (empty when the slot belongs to another pair).
    #[inline]
    pub fn witnesses(&self, key: u32) -> [TileRef; WITNESSES] {
        let s = Self::slot(key);
        if self.keys[s] == key { self.wit[s] } else { [NONE; WITNESSES] } // in-range: s < SLOTS
    }
    /// Remember `tile` as the most recent witness of `key`.
    #[inline]
    pub fn record(&mut self, key: u32, tile: TileRef) {
        let s = Self::slot(key);
        if self.keys[s] != key {
            self.keys[s] = key; // in-range: s < SLOTS
            self.wit[s] = [NONE; WITNESSES];
        }
        let w = &mut self.wit[s];
        w.rotate_right(1);
        w[0] = tile;
    }
    #[inline]
    pub fn is_none(t: TileRef) -> bool {
        t == NONE
    }
}

impl StepReset for BorderHints {}
