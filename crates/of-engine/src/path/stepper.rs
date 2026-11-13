//! `PathFinderStepper` (`pathfinding/PathFinderStepper.ts`). Owner: **P4b**.
//!
//! The stepper's cached path is **state** (E rows: `stepper { path, pathIndex, lastTo }`).
//! `next` wraps `finder.findPath` in the engine's one sanctioned in-tick `catch_unwind`
//! (`:64-70`): a `TsThrow` payload → `NotFound` (+ `Scratch::after_caught_throw`), any other
//! panic is re-raised (ARCHITECTURE §4.6, PORTING_GUIDE §11).
//!
//! TS's `StepperConfig` closures and the wrapped finder become one [`StepFinder`] value
//! (PORTING_GUIDE §7: no boxed closures), so the finder and the config can share `&mut Ctx`.
//!
//! [`restore_on_unwind`] is the second use of `catch_unwind` in this file: it never catches
//! (it always re-raises); it only puts state that was lent out of `State` (the water
//! chain's `seg_vis` and memo, `path::pathfinder`) back before the unwind continues, so the
//! post-throw state equals TS's (PORTING_GUIDE §11, "no panic point between paired writes").
#![allow(clippy::collapsible_if)] // nested ifs kept as the TS writes them
#![allow(clippy::disallowed_methods)] // the sanctioned catch_unwind sites (clippy.toml)

use crate::StateClone;
use crate::path::types::{PathResult, Src};
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};

/// `PathFinderStepper` data (`:24-26`) over `u32` nodes (tile refs or station ids).
#[derive(StateClone, Debug, Default, PartialEq, Eq)]
pub struct StepperState {
    /// `path: T[] | Uint32Array | null`.
    pub path: Option<Vec<u32>>,
    pub path_index: u32,
    pub last_to: Option<u32>,
}

/// `StepperConfig<T>` (`:8-12`) without closures: `equals` is `==` for every port use
/// (`a === b`, station `a.id === b.id`); `distance`/`preCheck` live on [`StepFinder`].
#[derive(Clone, Copy, Debug, Default)]
pub struct StepperConfig {
    /// `tileStepperConfig(game)` sets a distance and a pre-check (`PathFinder.ts`).
    pub has_distance: bool,
}

/// The wrapped `PathFinder` plus its `StepperConfig` callbacks.
pub trait StepFinder {
    /// `config.preCheck?.(from, to)`: `None` both for "no preCheck" and for a `null` result.
    fn pre_check(&mut self, from: u32, to: u32) -> Option<PathResult<u32>>;
    /// Whether `config.preCheck` is set (`findPath`'s all-failed test, `:146-157`).
    fn has_pre_check(&self) -> bool;
    /// `config.distance?.(a, b)`: `None` when the config has no `distance`.
    fn distance(&mut self, a: u32, b: u32) -> Option<f64>;
    /// `finder.findPath(from, to)`; may raise a `TsThrow`.
    fn find_path(&mut self, from: Src<'_>, to: u32) -> Option<Vec<u32>>;
    /// After a caught `TsThrow`: `Scratch::after_caught_throw` (what TS `finally`s undid).
    fn after_caught_throw(&mut self);
}

impl StepperState {
    /// `next(from, to, dist?)` (`:37-103`).
    pub fn next(&mut self, f: &mut impl StepFinder, from: u32, to: u32, dist: Option<f64>) -> PathResult<u32> {
        // The TS tail call `return this.next(from, to, dist)` (`:87-91`) is this loop; it
        // runs at most twice (after the restart the path is fresh, so `pathIndex` is 0 or
        // `path[0] === from`).
        loop {
            // TS: :39-42
            if let Some(result) = f.pre_check(from, to) {
                return result;
            }
            // TS: :44-46
            if from == to {
                return PathResult::Complete(to);
            }
            // TS: :49-53 (`dist !== undefined && dist > 0 && this.config.distance`)
            if let Some(d) = dist {
                if d > 0.0 {
                    if let Some(dd) = f.distance(from, to) {
                        if dd <= d {
                            return PathResult::Complete(from);
                        }
                    }
                }
            }
            // TS: :56-60
            if self.last_to != Some(to) {
                self.path = None;
                self.path_index = 0;
                self.last_to = Some(to);
            }
            // TS: :63-84
            if self.path.is_none() {
                let caught = catch_unwind(AssertUnwindSafe(|| f.find_path(Src::One(from), to)));
                let path = match caught {
                    Ok(p) => p,
                    Err(payload) => {
                        if of_det::throw::is_ts_throw(&*payload) {
                            // `console.error(..)` dropped; `return { NOT_FOUND }`.
                            f.after_caught_throw();
                            return PathResult::NotFound;
                        }
                        // A Rust bug inside the chain is never a silent path change.
                        resume_unwind(payload)
                    }
                };
                let Some(path) = path else {
                    return PathResult::NotFound;
                };
                // `new Uint32Array(path)` for numeric paths: the nodes are u32 already.
                let starts_at_from = path.first() == Some(&from);
                self.path = Some(path);
                self.path_index = 0;
                if starts_at_from {
                    self.path_index = 1;
                }
            }
            let path = self.path.as_ref().expect("rust-invariant: set just above");
            // TS: :86-91 (`path[pathIndex - 1]` is only read when pathIndex > 0)
            if self.path_index > 0 && from != path[self.path_index as usize - 1] {
                self.invalidate();
                self.last_to = Some(to);
                continue;
            }
            // TS: :94-96
            if self.path_index as usize >= path.len() {
                return PathResult::Complete(to);
            }
            // TS: :99-102
            let next_node = path[self.path_index as usize];
            self.path_index += 1;
            return PathResult::Next(next_node);
        }
    }

    /// `invalidate()` (`:105-109`).
    pub fn invalidate(&mut self) {
        self.path = None;
        self.path_index = 0;
        self.last_to = None;
    }

    /// `pathAfterNext()` (`:139-142`): the remainder starting at the node `next` returned
    /// last (`path.slice(pathIndex - 1)`), `None` without an active traversal.
    pub fn path_after_next(&self) -> Option<&[u32]> {
        let path = self.path.as_ref()?;
        if self.path_index == 0 {
            return None;
        }
        Some(&path[self.path_index as usize - 1..])
    }

    /// `findPath(from, to)` (`:145-160`): a one-shot route; the cached route is untouched.
    /// No try/catch here: a throw propagates as in TS.
    pub fn find_path(&mut self, f: &mut impl StepFinder, from: Src<'_>, to: u32) -> Option<Vec<u32>> {
        if f.has_pre_check() {
            let mut one = [0];
            let from_array = from.as_slice(&mut one);
            // `fromArray.every(..)` (true for an empty array), short-circuiting on the first
            // source whose preCheck is not NOT_FOUND.
            let all_failed = from_array.iter().all(|&s| matches!(f.pre_check(s, to), Some(PathResult::NotFound)));
            if all_failed {
                return None;
            }
        }
        f.find_path(from, to)
    }
}

/// Run `f` with `T` lent out of its home, and put it back on **every** exit, an unwind
/// included (the unwind then continues unchanged). `take` moves the value out, `put` moves
/// it back; `f` receives the context and the value.
pub(crate) fn restore_on_unwind<C, T, R>(
    cx: &mut C,
    take: impl FnOnce(&mut C) -> T,
    put: impl FnOnce(&mut C, T),
    f: impl FnOnce(&mut C, &mut T) -> R,
) -> R {
    let mut v = take(cx);
    let r = catch_unwind(AssertUnwindSafe(|| f(cx, &mut v)));
    put(cx, v);
    match r {
        Ok(r) => r,
        Err(payload) => resume_unwind(payload),
    }
}

/// CS: `TileStepperSchema` (`snapshot/PathfinderSnapshots.ts:29-33`, `tileStepperState`):
/// `{path: tiles|null, pathIndex, lastTo: uint|null}`; `path` is a `Uint32Array` (TYPED U32).
/// Written at merge1 (requests P7 R3, P8 #2) by the integrator on behalf of P4b.
impl crate::canon::Canon for StepperState {
    fn canon(&self, _cx: &mut crate::canon::CanonCx<'_>) -> crate::canon::Cs {
        use crate::canon::Cs;
        Cs::Obj(vec![
            ("path".into(), self.path.as_ref().map_or(Cs::Null, |p| crate::canon::tiles(p.iter().copied()))),
            ("pathIndex".into(), Cs::Int(i64::from(self.path_index))),
            ("lastTo".into(), self.last_to.map_or(Cs::Null, |t| Cs::Int(i64::from(t)))),
        ])
    }
}

#[cfg(test)]
mod tests {
    //! Port of `tests/core/pathfinding/PathFinderStepper.test.ts` (T0) plus the mandatory
    //! design test "a non-`TsThrow` panic inside the chain propagates" (FILE_MAP §4).
    use super::*;
    use crate::path::PathStatus;
    use std::collections::BTreeMap;

    /// `createMockFinder(pathMap)`: key `${fromTile}->${to}`, `fromTile` = first source.
    struct Mock {
        paths: BTreeMap<(u32, u32), Vec<u32>>,
        calls: u32,
        echo: bool,
        throw_ts: bool,
        throw_bug: bool,
        caught: u32,
    }
    impl Mock {
        fn new(paths: &[((u32, u32), &[u32])]) -> Self {
            Mock {
                paths: paths.iter().map(|(k, v)| (*k, v.to_vec())).collect(),
                calls: 0,
                echo: false,
                throw_ts: false,
                throw_bug: false,
                caught: 0,
            }
        }
    }
    impl StepFinder for Mock {
        fn pre_check(&mut self, _: u32, _: u32) -> Option<PathResult<u32>> {
            None
        }
        fn has_pre_check(&self) -> bool {
            false
        }
        fn distance(&mut self, _: u32, _: u32) -> Option<f64> {
            None
        }
        fn find_path(&mut self, from: Src<'_>, to: u32) -> Option<Vec<u32>> {
            self.calls += 1;
            if self.throw_ts {
                of_det::ts_throw!("boom");
            }
            if self.throw_bug {
                let v: Vec<u32> = Vec::new();
                let _ = v[3]; // a Rust bug (index out of bounds)
            }
            let mut one = [0];
            let first = from.as_slice(&mut one)[0];
            if self.echo {
                return Some(vec![first, to]);
            }
            self.paths.get(&(first, to)).cloned()
        }
        fn after_caught_throw(&mut self) {
            self.caught += 1;
        }
    }

    #[test]
    fn complete_when_at_destination() {
        let mut s = StepperState::default();
        let r = s.next(&mut Mock::new(&[]), 5, 5, None);
        assert_eq!(r, PathResult::Complete(5));
    }

    #[test]
    fn next_walks_the_path() {
        let mut m = Mock::new(&[((1, 4), &[1, 2, 3, 4])]);
        let mut s = StepperState::default();
        assert_eq!(s.next(&mut m, 1, 4, None), PathResult::Next(2));
        assert_eq!(s.next(&mut m, 2, 4, None), PathResult::Next(3));
        assert_eq!(s.next(&mut m, 3, 4, None), PathResult::Next(4));
        assert_eq!(s.next(&mut m, 4, 4, None).status(), PathStatus::Complete);
    }

    #[test]
    fn not_found_without_path() {
        let mut s = StepperState::default();
        assert_eq!(s.next(&mut Mock::new(&[]), 1, 99, None).status(), PathStatus::NotFound);
    }

    #[test]
    fn recomputes_when_moved_off_path() {
        let mut m = Mock::new(&[((1, 5), &[1, 2, 3, 4, 5]), ((10, 5), &[10, 9, 8, 7, 6, 5])]);
        let mut s = StepperState::default();
        assert_eq!(s.next(&mut m, 1, 5, None), PathResult::Next(2));
        assert_eq!(s.next(&mut m, 10, 5, None), PathResult::Next(9));
    }

    #[test]
    fn recomputes_when_destination_changes() {
        let mut m = Mock::new(&[((1, 5), &[1, 2, 3, 4, 5]), ((2, 9), &[2, 6, 7, 8, 9])]);
        let mut s = StepperState::default();
        assert_eq!(s.next(&mut m, 1, 5, None), PathResult::Next(2));
        assert_eq!(s.next(&mut m, 2, 9, None), PathResult::Next(6));
    }

    #[test]
    fn invalidate_forces_recompute() {
        let mut m = Mock::new(&[]);
        m.echo = true;
        let mut s = StepperState::default();
        s.next(&mut m, 1, 5, None);
        s.next(&mut m, 5, 5, None);
        assert_eq!(m.calls, 1);
        s.invalidate();
        s.next(&mut m, 1, 5, None);
        assert_eq!(m.calls, 2);
    }

    #[test]
    fn path_after_next_is_a_copy_of_the_remainder() {
        let mut m = Mock::new(&[((1, 5), &[1, 2, 3, 4, 5])]);
        let mut s = StepperState::default();
        assert_eq!(s.path_after_next(), None);
        assert_eq!(s.next(&mut m, 1, 5, None), PathResult::Next(2));
        let mut copy = s.path_after_next().expect("active").to_vec();
        assert_eq!(copy, [2, 3, 4, 5]);
        copy[0] = 99;
        assert_eq!(s.next(&mut m, 2, 5, None), PathResult::Next(3));
    }

    #[test]
    fn find_path_delegates_and_supports_multi_source() {
        let mut m = Mock::new(&[((1, 5), &[1, 2, 3, 4, 5])]);
        let mut s = StepperState::default();
        assert_eq!(s.find_path(&mut m, Src::One(1), 5), Some(vec![1, 2, 3, 4, 5]));
        let mut e = Mock::new(&[]);
        e.echo = true;
        assert_eq!(s.find_path(&mut e, Src::Many(&[1, 2, 3]), 5), Some(vec![1, 5]));
    }

    #[test]
    fn ts_throw_in_finder_is_not_found_and_resets_scratch() {
        let mut m = Mock::new(&[]);
        m.throw_ts = true;
        let mut s = StepperState::default();
        assert_eq!(s.next(&mut m, 1, 5, None), PathResult::NotFound);
        assert_eq!(m.caught, 1);
        // TS state after the catch: lastTo = to, path = null.
        assert_eq!(s, StepperState { path: None, path_index: 0, last_to: Some(5) });
    }

    #[test]
    fn rust_bug_in_finder_propagates() {
        let mut m = Mock::new(&[]);
        m.throw_bug = true;
        let mut s = StepperState::default();
        let r = catch_unwind(AssertUnwindSafe(|| s.next(&mut m, 1, 5, None)));
        let payload = r.expect_err("a non-TsThrow panic must propagate");
        assert!(!of_det::throw::is_ts_throw(&*payload));
        assert_eq!(m.caught, 0);
    }

    #[test]
    fn restore_on_unwind_puts_back_before_reraising() {
        let mut home: (u32, Vec<u32>) = (0, vec![1, 2]);
        let r = catch_unwind(AssertUnwindSafe(|| {
            restore_on_unwind(
                &mut home,
                |h| std::mem::take(&mut h.1),
                |h, v| h.1 = v,
                |h, v| {
                    h.0 = 7;
                    v.push(3);
                    of_det::ts_throw!("inside");
                },
            )
        }));
        assert!(of_det::throw::is_ts_throw(&*r.expect_err("re-raised")));
        assert_eq!(home, (7, vec![1, 2, 3]));
    }

    /// `findPath` with a preCheck that fails every source returns null without calling the
    /// finder; an empty source array counts as "all failed" (`[].every(..)` is true).
    #[test]
    fn find_path_all_failed_precheck() {
        struct P(u32);
        impl StepFinder for P {
            fn pre_check(&mut self, f: u32, _: u32) -> Option<PathResult<u32>> {
                if f >= 100 { Some(PathResult::NotFound) } else { None }
            }
            fn has_pre_check(&self) -> bool {
                true
            }
            fn distance(&mut self, _: u32, _: u32) -> Option<f64> {
                None
            }
            fn find_path(&mut self, _: Src<'_>, to: u32) -> Option<Vec<u32>> {
                self.0 += 1;
                Some(vec![to])
            }
            fn after_caught_throw(&mut self) {}
        }
        let mut p = P(0);
        let mut s = StepperState::default();
        assert_eq!(s.find_path(&mut p, Src::Many(&[100, 101]), 5), None);
        assert_eq!(s.find_path(&mut p, Src::Many(&[]), 5), None);
        assert_eq!(p.0, 0);
        assert_eq!(s.find_path(&mut p, Src::Many(&[100, 1]), 5), Some(vec![5]));
    }
}
