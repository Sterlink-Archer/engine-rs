//! Port of `engine/src/game/TileTraversalScratch.ts` (owner: **P3**; skeleton
//! implementation written at S0 by P1 because `Scratch::prepare` depends on it).
//!
//! TS keeps one scratch per game (`WeakMap<Game, …>`, `:25-41`): a generation-stamped
//! `visited` array, a reusable `stack`, and `clusterIndexMap` (zeroed by its users after
//! every use, `PlayerExecution.ts:295-305`). Rust keeps one per thread, shared by every
//! game and fork on it (`sc.traversal`, ARCHITECTURE §3.8).
//!
//! **Stamp-wrap hazard (G14).** TS never reaches its wrap at `0xffffffff` (about 5 k
//! bumps per World tick). A per-thread scratch could, after ~10^6 fork-ticks, and users
//! compare stamps *relatively* within a pass (`visited[t] >= areaGen`,
//! `PlayerExecution.ts:360-372`), so a wrap mid-pass would change territory areas.
//! Rule: [`TraversalScratch::prepare_step`] (called by `Scratch::prepare` at the start of
//! every step) zeroes the stamps and resets `generation` to 0 once `generation > 2^31`. TS never wraps
//! and every comparison is relative within a pass, so this cannot be observed; the
//! forced-wrap unit test below checks it.

use super::StepReset;
use crate::ids::TileRef;

/// `TileTraversalScratch` (`:17-23`).
#[derive(Debug, Default)]
pub struct TraversalScratch {
    pub visited: Vec<u32>,
    pub stack: Vec<TileRef>,
    pub cluster_index_map: Vec<i32>,
    /// Current generation; advance via [`TraversalScratch::bump_generation`].
    pub generation: u32,
}

/// Reset threshold of the step-start hook.
pub const GEN_RESET_ABOVE: u32 = 1 << 31;

impl TraversalScratch {
    /// `tileTraversalScratch(game)` (`:27-41`): make sure the arrays cover `total_tiles`.
    /// TS allocates a fresh scratch (generation 0) when the cached one is too small; growing in
    /// place keeps old stamps, all `< generation`, which no pass can confuse with its own.
    pub fn ensure(&mut self, total_tiles: usize) -> &mut Self {
        if self.visited.len() < total_tiles {
            self.visited.resize(total_tiles, 0);
            self.cluster_index_map.resize(total_tiles, 0);
        }
        self
    }

    /// `bumpTraversalGeneration(scratch)` (`:43-50`), literally (TS's own wrap included).
    pub fn bump_generation(&mut self) -> u32 {
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0xffff_ffff {
            self.visited.fill(0);
            self.generation = 1;
        }
        self.generation
    }
}

impl StepReset for TraversalScratch {
    fn prepare_step(&mut self) {
        if self.generation > GEN_RESET_ABOVE {
            self.visited.fill(0);
            self.generation = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-generation pass (`visited[t] >= areaGen` style) gives identical results
    /// whether `generation` starts at 0 or just below the wrap, thanks to the step-start reset.
    #[test]
    fn forced_wrap_is_unobservable() {
        fn pass(sc: &mut TraversalScratch) -> Vec<u32> {
            sc.ensure(16);
            let area_gen = sc.bump_generation();
            for t in 0..8 {
                sc.visited[t] = area_gen + (t as u32 % 3);
            }
            for _ in 0..3 {
                sc.bump_generation();
            }
            (0..16).map(|t| u32::from(sc.visited[t] >= area_gen) + (sc.visited[t].saturating_sub(area_gen))).collect()
        }
        let mut fresh = TraversalScratch::default();
        fresh.prepare_step();
        let want = pass(&mut fresh);

        let mut near = TraversalScratch::default();
        near.ensure(16);
        near.generation = 0xffff_ffff - 2; // a pass starting here would wrap mid-pass
        near.visited.fill(0xffff_fff0); // stale stamps from earlier passes
        near.prepare_step();
        assert_eq!(near.generation, 0);
        assert_eq!(pass(&mut near), want);
    }

    #[test]
    fn ts_wrap_is_ported() {
        let mut sc = TraversalScratch::default();
        sc.ensure(4);
        sc.visited[2] = 7;
        sc.generation = 0xffff_fffe;
        assert_eq!(sc.bump_generation(), 1); // TS: generation === 0xffffffff -> fill(0), generation = 1
        assert_eq!(sc.visited, [0, 0, 0, 0]);
    }
}
