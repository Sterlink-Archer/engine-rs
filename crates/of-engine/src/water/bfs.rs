//! Generic `BFS<T>` (`pathfinding/algorithms/BFS.ts`). Owner: **P4a**.
//!
//! No importer at the pin (`grep -rn "new BFS(" engine/src` is empty); ported for fidelity.
//! Signature note: the S0 stub took `max_distance: u32` and an `Option<R>` visitor, which
//! cannot express TS's `Infinity` or the `null` ("reject, don't expand") vs `undefined`
//! ("expand") distinction; no other package calls it, so P4a widened it (docs/requests/P4a.md).

use crate::jsorder::LookupSet;
use std::collections::VecDeque;

/// A visitor's answer (TS `R | null | undefined`, `BFS.ts:13-16`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visit<R> {
    /// `R`: found, return it.
    Found(R),
    /// `null`: reject this node, do not expand its neighbours.
    Reject,
    /// `undefined`: valid node, expand.
    Expand,
}

/// `BFS.search(start, maxDistance, visitor)` (`:19-64`) over `neighbors` (the adapter).
/// `max_distance` is an f64 so `Infinity` (unlimited) is representable.
pub fn search<T: Copy + Eq + std::hash::Hash, R>(
    start: &[T],
    max_distance: f64,
    mut neighbors: impl FnMut(T) -> Vec<T>,
    mut visitor: impl FnMut(T, u32) -> Visit<R>,
) -> Option<R> {
    let mut visited: LookupSet<T> = LookupSet::default(); // lookup-only: has/add (`:24,53-57`)
    let mut queue: VecDeque<(T, u32)> = VecDeque::new(); // TS: push/shift only (`:25,34`)
    for &s in start {
        visited.insert(s);
        queue.push_back((s, 0));
    }
    while let Some((node, dist)) = queue.pop_front() {
        match visitor(node, dist) {
            Visit::Found(r) => return Some(r),
            Visit::Reject => continue,
            Visit::Expand => {}
        }
        let next_dist = dist + 1;
        if f64::from(next_dist) > max_distance {
            continue;
        }
        for n in neighbors(node) {
            if visited.contains(&n) {
                continue;
            }
            visited.insert(n);
            queue.push_back((n, next_dist));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bfs_order_reject_and_distance_cap() {
        // Path graph 0-1-2-3-4 plus 1-5.
        let adj = |n: u32| -> Vec<u32> {
            match n {
                0 => vec![1],
                1 => vec![0, 2, 5],
                2 => vec![1, 3],
                3 => vec![2, 4],
                4 => vec![3],
                _ => vec![1],
            }
        };
        let mut seen = Vec::new();
        let r: Option<u32> = search(&[0], f64::INFINITY, adj, |n, d| {
            seen.push((n, d));
            if n == 2 { Visit::Reject } else { Visit::Expand }
        });
        assert_eq!(r, None);
        assert_eq!(seen, [(0, 0), (1, 1), (2, 2), (5, 2)]); // 2 rejected: 3, 4 never reached
        let mut seen = Vec::new();
        let r: Option<u32> = search(&[0], 1.0, adj, |n, d| {
            seen.push(n);
            if n == 5 { Visit::Found(d) } else { Visit::Expand }
        });
        assert_eq!((r, seen), (None, vec![0, 1]));
        let r = search(&[4, 0], f64::INFINITY, adj, |n, d| if n == 5 { Visit::Found(d) } else { Visit::Expand });
        assert_eq!(r, Some(2));
    }
}
