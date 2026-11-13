//! Generic `AStar` over an adapter (`pathfinding/algorithms/AStar.ts`). Owner: **P4b**.
//!
//! Also the [`PQueue`] view of P4a's `MinHeap`/`BucketQueue` (`water::priority_queue`), which
//! every A* in `path/` uses through UFCS (`PQueue::push(q, ..)`). Production delegates to
//! P4a's inherent methods (the S1 test-only `PriorityQueue.ts` shim was deleted at merge1,
//! `docs/requests/P4b.md` R1).
//!
//! **Typed-array semantics kept** (PORTING_GUIDE §6, §8):
//! - `gScore` is a `Uint32Array`: the f64 `tentativeG` is compared as a number and stored
//!   with `ToUint32` (`AStar.ts:99-106`).
//! - `neighborBuffer` is an `Int32Array(maxNeighbors)`: an adapter that reports more
//!   neighbours than fit (`StationGraphAdapter`, 32 slots, `PathFinder.Station.ts:57-75`)
//!   loses the writes past the end, and the loop then reads `undefined` for them. TS then
//!   treats `undefined` as a node: `closedStamp[undefined]` is never set, the
//!   `gScore`/`gScoreStamp`/`cameFrom` writes become ordinary properties named
//!   `"undefined"` on the typed arrays, and `queue.push(undefined, ..)` stores `0` into the
//!   queue's `Int32Array`. All of that is ported ([`AStarBuffers::undef_g`]).

use crate::water::priority_queue::{BucketQueue, MinHeap};

/// `PriorityQueue` (`PriorityQueue.ts:1-6`).
pub trait PQueue {
    fn push(&mut self, node: i32, priority: f64);
    fn pop(&mut self) -> i32;
    fn is_empty(&self) -> bool;
    fn clear(&mut self);
}

/// `new MinHeap(capacity)` (`PriorityQueue.ts:14-17`).
pub fn new_min_heap(capacity: usize) -> MinHeap {
    MinHeap::new(capacity)
}

/// `new BucketQueue(maxPriority)` (`PriorityQueue.ts:116-123`).
pub fn new_bucket_queue(max_priority: u32) -> BucketQueue {
    BucketQueue::new(max_priority)
}

impl PQueue for MinHeap {
    #[inline]
    fn push(&mut self, node: i32, priority: f64) {
        MinHeap::push(self, node, priority)
    }
    #[inline]
    fn pop(&mut self) -> i32 {
        MinHeap::pop(self)
    }
    #[inline]
    fn is_empty(&self) -> bool {
        MinHeap::is_empty(self)
    }
    #[inline]
    fn clear(&mut self) {
        MinHeap::clear(self)
    }
}

impl PQueue for BucketQueue {
    #[inline]
    fn push(&mut self, node: i32, priority: f64) {
        BucketQueue::push(self, node, priority)
    }
    #[inline]
    fn pop(&mut self) -> i32 {
        BucketQueue::pop(self)
    }
    #[inline]
    fn is_empty(&self) -> bool {
        BucketQueue::is_empty(self)
    }
    #[inline]
    fn clear(&mut self) {
        BucketQueue::clear(self)
    }
}

/// `AStarAdapter` (`:8-18`).
pub trait AStarAdapter {
    /// Writes neighbours into `buffer` (an `Int32Array(maxNeighbors)`: writes past the end
    /// are dropped, as TS drops them) and returns the TS count, which can exceed
    /// `buffer.len()`.
    fn neighbors(&self, node: u32, buffer: &mut [i32]) -> usize;
    fn cost(&self, from: u32, to: u32, prev: Option<u32>) -> f64;
    fn heuristic(&self, node: u32, goal: u32) -> f64;
    fn num_nodes(&self) -> u32;
    fn max_priority(&self) -> u32;
    fn max_neighbors(&self) -> u32;
    /// `cost(from, undefined, prev)`: only for an adapter whose `neighbors` can overflow.
    fn cost_to_undefined(&self, from: u32, prev: Option<u32>) -> f64 {
        let _ = (from, prev);
        unreachable!("adapter cannot report more neighbours than maxNeighbors")
    }
    /// `heuristic(undefined, goal)`: see [`AStarAdapter::cost_to_undefined`].
    fn heuristic_undefined(&self, goal: u32) -> f64 {
        let _ = goal;
        unreachable!("adapter cannot report more neighbours than maxNeighbors")
    }
}

/// `AStarConfig` (`:20-23`).
#[derive(Clone, Copy, Debug, Default)]
pub struct AStarConfig {
    pub max_iterations: Option<u32>,
}

/// Per-instance buffers (`:26-35`), kept in `Scratch`. One TS `AStar` instance.
#[derive(Debug, Default)]
pub struct AStarBuffers {
    pub stamp: u32,
    pub closed_stamp: Vec<u32>,
    pub g_score_stamp: Vec<u32>,
    pub g_score: Vec<u32>,
    pub came_from: Vec<i32>,
    pub neighbor_buffer: Vec<i32>,
    /// `new BucketQueue(adapter.maxPriority())` (`:45`).
    pub queue: Option<BucketQueue>,
    /// `gScoreStamp["undefined"]` / `gScore["undefined"]`: the ordinary properties TS
    /// creates on the typed arrays for an overflowed neighbour (module doc). `None` reads
    /// `undefined`.
    pub undef_g_stamp: Option<u32>,
    pub undef_g: f64,
    /// `adapter.maxPriority()` the queue was built for.
    max_priority: u32,
}

impl AStarBuffers {
    /// `new AStar({adapter})` (`:37-46`): the arrays are sized by the adapter. Reused while
    /// the sizes match; stamps are per query, so reuse is unobservable (a fresh TS instance
    /// starts at `stamp = 1` with zeroed arrays, which is what a resize reproduces).
    fn ensure(&mut self, adapter: &impl AStarAdapter) {
        let n = adapter.num_nodes() as usize;
        if self.closed_stamp.len() != n {
            self.closed_stamp = vec![0; n];
            self.g_score_stamp = vec![0; n];
            self.g_score = vec![0; n];
            self.came_from = vec![0; n];
            self.stamp = 1;
            self.undef_g_stamp = None;
            self.undef_g = 0.0;
        }
        let m = adapter.max_neighbors() as usize;
        if self.neighbor_buffer.len() != m {
            self.neighbor_buffer = vec![0; m];
        }
        let mp = adapter.max_priority();
        if self.queue.is_none() || self.max_priority != mp {
            self.queue = Some(new_bucket_queue(mp));
            self.max_priority = mp;
        }
    }
}

/// `findPath(start, goal)` (`:48-113`). `start`: `number | number[]` (the generic A* treats
/// both alike).
pub fn find_path(
    buf: &mut AStarBuffers,
    adapter: &impl AStarAdapter,
    cfg: AStarConfig,
    start: &[u32],
    goal: u32,
) -> Option<Vec<u32>> {
    buf.ensure(adapter);
    // TS: AStar.ts:39 `config.maxIterations ?? 500_000`
    let max_iterations = cfg.max_iterations.unwrap_or(500_000);
    // TS: :49-54
    if buf.stamp == u32::MAX {
        buf.closed_stamp.fill(0);
        buf.g_score_stamp.fill(0);
        buf.stamp = 1;
    } else {
        buf.stamp += 1;
    }
    let stamp = buf.stamp;
    let AStarBuffers {
        closed_stamp,
        g_score_stamp,
        g_score,
        came_from,
        neighbor_buffer,
        queue,
        undef_g_stamp,
        undef_g,
        ..
    } = buf;
    let queue = queue.as_mut().expect("rust-invariant: ensure() built the queue");

    PQueue::clear(queue);
    for &s in start {
        let si = s as usize; // in-range: start nodes are adapter nodes (< numNodes)
        g_score[si] = 0;
        g_score_stamp[si] = stamp;
        came_from[si] = -1;
        PQueue::push(queue, s as i32, adapter.heuristic(s, goal));
    }

    let mut iterations = i64::from(max_iterations);
    while !PQueue::is_empty(queue) {
        // TS: :77 `if (--iterations <= 0) return null`
        iterations -= 1;
        if iterations <= 0 {
            return None;
        }
        let current = PQueue::pop(queue) as u32;
        let ci = current as usize; // in-range: queued nodes are adapter nodes (or the Int32Array 0)
        if closed_stamp[ci] == stamp {
            continue;
        }
        closed_stamp[ci] = stamp;
        if current == goal {
            return Some(build_path(came_from, goal));
        }
        let current_g = f64::from(g_score[ci]);
        let prev = came_from[ci];
        let prev = if prev == -1 { None } else { Some(prev as u32) };
        let count = adapter.neighbors(current, neighbor_buffer);
        for i in 0..count {
            match neighbor_buffer.get(i) {
                Some(&n) => {
                    let neighbor = n as u32;
                    let ni = neighbor as usize;
                    if closed_stamp[ni] == stamp {
                        continue;
                    }
                    // TS: :99-101 (number arithmetic; the store below is ToUint32)
                    let tentative_g = current_g + adapter.cost(current, neighbor, prev);
                    if g_score_stamp[ni] != stamp || tentative_g < f64::from(g_score[ni]) {
                        came_from[ni] = current as i32;
                        g_score[ni] = of_det::js::to_uint32(tentative_g);
                        g_score_stamp[ni] = stamp;
                        PQueue::push(queue, neighbor as i32, tentative_g + adapter.heuristic(neighbor, goal));
                    }
                }
                None => {
                    // `buffer[i]` past the Int32Array end reads `undefined` (module doc).
                    // closedStamp["undefined"] is never set, so the `continue` never fires.
                    let tentative_g = current_g + adapter.cost_to_undefined(current, prev);
                    let lt = match *undef_g_stamp {
                        Some(s) if s == stamp => tentative_g < *undef_g,
                        _ => true, // `gScoreStamp[undefined] !== stamp`
                    };
                    if lt {
                        // cameFrom["undefined"] = current: never read back.
                        *undef_g = tentative_g;
                        *undef_g_stamp = Some(stamp);
                        // queue.push(undefined, ..): the Int32Array store makes it node 0.
                        PQueue::push(queue, 0, tentative_g + adapter.heuristic_undefined(goal));
                    }
                }
            }
        }
    }
    None
}

/// `buildPath(goal)` (`:115-126`).
fn build_path(came_from: &[i32], goal: u32) -> Vec<u32> {
    let mut path = Vec::new();
    let mut current = goal as i32;
    while current != -1 {
        path.push(current as u32);
        current = came_from[current as usize];
    }
    path.reverse();
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1-D line 0..n with unit costs.
    struct Line(u32);
    impl AStarAdapter for Line {
        fn neighbors(&self, node: u32, b: &mut [i32]) -> usize {
            let mut c = 0;
            if node > 0 {
                b[c] = node as i32 - 1;
                c += 1;
            }
            if node + 1 < self.0 {
                b[c] = node as i32 + 1;
                c += 1;
            }
            c
        }
        fn cost(&self, _: u32, _: u32, _: Option<u32>) -> f64 {
            1.0
        }
        fn heuristic(&self, n: u32, g: u32) -> f64 {
            f64::from(n.abs_diff(g))
        }
        fn num_nodes(&self) -> u32 {
            self.0
        }
        fn max_priority(&self) -> u32 {
            2 * self.0
        }
        fn max_neighbors(&self) -> u32 {
            2
        }
    }

    #[test]
    fn line_paths_and_iteration_cap() {
        let mut b = AStarBuffers::default();
        let l = Line(10);
        assert_eq!(find_path(&mut b, &l, AStarConfig::default(), &[2], 6), Some(vec![2, 3, 4, 5, 6]));
        assert_eq!(find_path(&mut b, &l, AStarConfig::default(), &[9, 0], 1), Some(vec![0, 1]));
        // `--iterations <= 0` fires before the 3rd pop with maxIterations = 3.
        assert_eq!(find_path(&mut b, &l, AStarConfig { max_iterations: Some(3) }, &[0], 5), None);
        assert_eq!(find_path(&mut b, &l, AStarConfig { max_iterations: Some(3) }, &[0], 1), Some(vec![0, 1]));
    }

    /// Star graph: hub 1 has more neighbours than its 2-slot buffer; overflowed ones are
    /// unreachable through the hub (TS drops the writes), node 0 is the `undefined` sink.
    struct Star;
    impl AStarAdapter for Star {
        fn neighbors(&self, node: u32, b: &mut [i32]) -> usize {
            if node != 1 {
                return 0;
            }
            let ns = [2, 3, 4, 5];
            for (i, n) in ns.iter().enumerate() {
                if i < b.len() {
                    b[i] = *n;
                }
            }
            ns.len()
        }
        fn cost(&self, _: u32, _: u32, _: Option<u32>) -> f64 {
            1.0
        }
        fn heuristic(&self, _: u32, _: u32) -> f64 {
            0.0
        }
        fn num_nodes(&self) -> u32 {
            6
        }
        fn max_priority(&self) -> u32 {
            20
        }
        fn max_neighbors(&self) -> u32 {
            2
        }
        fn cost_to_undefined(&self, _: u32, _: Option<u32>) -> f64 {
            1.0
        }
        fn heuristic_undefined(&self, _: u32) -> f64 {
            0.0
        }
    }

    #[test]
    fn overflowed_neighbours_are_dropped_like_typed_array_writes() {
        let mut b = AStarBuffers::default();
        assert_eq!(find_path(&mut b, &Star, AStarConfig::default(), &[1], 3), Some(vec![1, 3]));
        assert_eq!(find_path(&mut b, &Star, AStarConfig::default(), &[1], 4), None);
        assert_eq!(b.undef_g_stamp, Some(b.stamp));
        assert_eq!(b.undef_g, 1.0);
    }
}
