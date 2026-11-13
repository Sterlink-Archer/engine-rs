//! `BFSGrid` (`pathfinding/algorithms/BFS.Grid.ts`). Owner: **P4a**. Stamp-based visited
//! array (scratch: `sc.water_mgr.builder_bfs`, `sc.hpa.tile_bfs`).
//!
//! Signature note: the S0 stub's `Option<R>` visitor could not express TS's `null` (reject)
//! vs `undefined` (expand); it now returns [`Visit`] (callers are P4a-only).
//!
//! Scratch sharing: TS keeps one `BFSGrid` per WaterManager/HPA; Rust one per thread. Every
//! search takes a fresh stamp (`:276`, `:355-364`) and reads `visitedStamp`/`dist` only for
//! nodes stamped in this search, so a shared scratch is unobservable (single-pass safe; the
//! u32 wrap is TS's own: zero the stamps and restart at 1).

use crate::scratch::water::BfsGridScratch;
pub use crate::water::bfs::Visit;

impl BfsGridScratch {
    /// `nextStamp()` (`:355-364`): returns the current stamp, then advances; past
    /// `0xffffffff` the visited stamps are zeroed and the stamp restarts at 1.
    fn next_stamp(&mut self) -> u32 {
        let stamp = self.stamp;
        if self.stamp == u32::MAX {
            self.visited_stamp.fill(0);
            self.stamp = 1;
        } else {
            self.stamp += 1;
        }
        stamp
    }

    /// `new BFSGrid(numNodes)` sizing (`:252-256`): grow to at least `n` (zero-filled).
    fn ensure(&mut self, n: usize) {
        if self.visited_stamp.len() < n {
            self.visited_stamp.resize(n, 0);
            self.queue.resize(n, 0);
            self.dist.resize(n, 0);
        }
    }
}

/// `BFSGrid.search(width, height, start, maxDistance, isValidNode, visitor)` (`:268-353`) on a
/// `width`-wide grid. Neighbour order N, S, W, E. `start` is a TS number or array (all call
/// sites pass a single tile).
pub fn search<R>(
    sc: &mut BfsGridScratch,
    width: u32,
    height: u32,
    start: &[u32],
    max_distance: u32,
    mut is_valid: impl FnMut(u32) -> bool,
    mut visitor: impl FnMut(u32, u32) -> Visit<R>,
) -> Option<R> {
    let num_nodes = (width as usize) * (height as usize);
    sc.ensure(num_nodes);
    let stamp = sc.next_stamp();
    let last_row_start = (height - 1) * width; // TS: :277
    // TS: queue is an Int32Array(numNodes): writes past numNodes are dropped and reads give
    // undefined. With one start (every call site) and the stamp guard, tail <= numNodes.
    debug_assert!(start.len() <= 1, "BFSGrid: multi-start never used at the pin");
    let mut head = 0usize;
    let mut tail = 0usize;
    for &s in start {
        let si = s as usize; // in-range: start tiles are valid refs
        sc.visited_stamp[si] = stamp;
        sc.dist[si] = 0;
        sc.queue[tail] = s as i32;
        tail += 1;
    }
    while head < tail {
        let node = sc.queue[head] as u32; // in-range: head < tail <= numNodes
        head += 1;
        let dist = u32::from(sc.dist[node as usize]); // TS: Uint16Array read
        match visitor(node, dist) {
            Visit::Found(r) => return Some(r),
            Visit::Reject => continue,
            Visit::Expand => {}
        }
        let next_dist = dist + 1;
        if next_dist > max_distance {
            continue;
        }
        let x = node % width;
        // TS: :311-349 N, S, W, E; `dist[n] = nextDist` is a Uint16Array store (wraps).
        let mut visit = |n: u32, sc: &mut BfsGridScratch, tail: &mut usize| {
            let ni = n as usize;
            if sc.visited_stamp[ni] != stamp && is_valid(n) {
                sc.visited_stamp[ni] = stamp;
                sc.dist[ni] = next_dist as u16;
                sc.queue[*tail] = n as i32; // in-range: each node is enqueued once per stamp
                *tail += 1;
            }
        };
        if node >= width {
            visit(node - width, sc, &mut tail);
        }
        if node < last_row_start {
            visit(node + width, sc, &mut tail);
        }
        if x != 0 {
            visit(node - 1, sc, &mut tail);
        }
        if x != width - 1 {
            visit(node + 1, sc, &mut tail);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_bfs_order_distance_and_stamp_reuse() {
        // 4x3 grid, all valid except tile 5.
        let mut sc = BfsGridScratch::default();
        let mut order = Vec::new();
        let r: Option<()> = search(
            &mut sc,
            4,
            3,
            &[0],
            100,
            |n| n != 5,
            |n, d| {
                order.push((n, d));
                Visit::Expand
            },
        );
        assert!(r.is_none());
        // From 0: N none, S 4, W none, E 1; then 4: S 8 (N 0 seen, E 5 invalid); 1: S 5 invalid, E 2 ...
        assert_eq!(order[..4], [(0, 0), (4, 1), (1, 1), (8, 2)]);
        assert_eq!(order.len(), 11);
        // Second search on the same scratch: fresh stamp, identical result.
        let mut order2 = Vec::new();
        let _: Option<()> = search(
            &mut sc,
            4,
            3,
            &[0],
            100,
            |n| n != 5,
            |n, d| {
                order2.push((n, d));
                Visit::Expand
            },
        );
        assert_eq!(order, order2);
        // Distance cap and early found.
        let r = search(&mut sc, 4, 3, &[0], 1, |_| true, |n, d| if n == 2 { Visit::Found(d) } else { Visit::Expand });
        assert_eq!(r, None);
        let r = search(&mut sc, 4, 3, &[0], 2, |_| true, |n, d| if n == 2 { Visit::Found(d) } else { Visit::Expand });
        assert_eq!(r, Some(2));
    }

    #[test]
    fn grid_bfs_stamp_wrap_matches_ts() {
        let mut sc = BfsGridScratch { stamp: u32::MAX, ..Default::default() };
        let r = search(&mut sc, 2, 1, &[0], 5, |_| true, |n, _| if n == 1 { Visit::Found(n) } else { Visit::Expand });
        assert_eq!(r, Some(1));
        assert_eq!(sc.stamp, 1);
        let r = search(&mut sc, 2, 1, &[0], 5, |_| true, |n, _| if n == 1 { Visit::Found(n) } else { Visit::Expand });
        assert_eq!(r, Some(1));
    }
}
