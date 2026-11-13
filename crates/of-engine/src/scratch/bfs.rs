//! Per-map-instance BFS scratch of `GameMapImpl` (owner: **P3**; declared at S0 by P1).
//!
//! TS: `bfsVisited: Uint16Array`, `bfsQueue`, `bfsGen`, `inBfs` (`GameMapImpl.ts:9-12`),
//! one set **per map instance**. Rust: two independent fields, `sc.bfs_main` and
//! `sc.bfs_mini` (ARCHITECTURE §3.6.1), so a nested `bfsSearch` on the other map works
//! exactly as in TS, and a nested one on the same map raises the TS throw
//! ([`BfsScratch::enter`]).
//!
//! The u16 stamps wrap by TS's own rule (`bumpBfsGeneration`, `:433-445`), which is
//! single-pass safe (the fill happens before a pass starts), so sharing the scratch
//! across games and forks is unobservable; `prepare_step` still resets the stamps at a
//! step start when they are past half range, like every other stamp scratch (G14).

use super::StepReset;
use crate::ids::TileRef;

#[derive(Debug, Default)]
pub struct BfsScratch {
    pub visited: Vec<u16>,
    pub queue: Vec<TileRef>,
    pub generation: u32,
    /// TS `inBfs`: the re-entrancy guard of `bfsSearch` (`:477-479`).
    pub busy: bool,
}

impl BfsScratch {
    /// `bumpBfsGeneration()` (`GameMapImpl.ts:433-445`), literally. TS allocates a fresh
    /// array (and resets the generation) when the cached one is too small.
    pub fn bump_generation(&mut self, total_tiles: usize) -> u32 {
        if self.visited.len() < total_tiles {
            self.visited = vec![0; total_tiles];
            self.generation = 0;
        }
        self.generation += 1;
        if self.generation == 0xffff {
            self.visited.fill(0);
            self.generation = 1;
        }
        self.generation
    }

    /// Enter `bfsSearch`: TS throws `"Re-entrant bfsSearch is not supported"` when a
    /// search on this map instance is already running (`:477-479`).
    pub fn enter(&mut self) {
        if self.busy {
            of_det::ts_throw!("Re-entrant bfsSearch is not supported");
        }
        self.busy = true;
    }

    /// Leave `bfsSearch` (the TS `finally`, `:497-500`): clear the queue, drop the guard.
    pub fn exit(&mut self) {
        self.queue.clear();
        self.busy = false;
    }
}

impl StepReset for BfsScratch {
    fn prepare_step(&mut self) {
        if self.generation > 0x7fff {
            self.visited.fill(0);
            self.generation = 0;
        }
        // A throw that unwound out of a search skipped its `exit`; TS's `finally` had run.
        self.exit();
    }
    fn after_caught_throw(&mut self) {
        self.exit();
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // tests may catch panics to assert them
mod tests {
    use super::*;
    #[test]
    fn reentry_throws_and_wrap_follows_ts() {
        let mut b = BfsScratch::default();
        b.enter();
        let p = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| b.enter())).expect_err("re-entrant");
        let t = of_det::throw::as_ts_throw(&*p).expect("TsThrow");
        assert_eq!(t.msg, "Re-entrant bfsSearch is not supported");
        b.exit();
        b.enter(); // fine again
        b.exit();
        assert_eq!(b.bump_generation(4), 1);
        b.generation = 0xfffe;
        b.visited[0] = 3;
        assert_eq!(b.bump_generation(4), 1);
        assert_eq!(b.visited, [0, 0, 0, 0]);
    }
}
