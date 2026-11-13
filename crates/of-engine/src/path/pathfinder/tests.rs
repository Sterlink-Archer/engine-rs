//! Tests of `path::pathfinder` (memo, chain slots, `WaterPathFinder`). Owner: **P4b**.
//!
//! Mandatory design tests (FILE_MAP §4, P4b row):
//! - stale memoized stepper hits the **old** memo after a rebuild and stores misses back into it;
//! - the chain kind is fixed at slot creation;
//! - (a non-`TsThrow` panic inside the chain propagates: `path::stepper` tests);
//! - memo LRU eviction order at a shrunk budget.
#![allow(clippy::disallowed_methods)] // tests assert panics with catch_unwind

use super::*;
use crate::path::testkit::{ISLAND, r, state_with};
use crate::scratch::Scratch;
use crate::state::State;
use crate::water::abstract_graph::{AbstractNode, WaterGraph};
use std::panic::{AssertUnwindSafe, catch_unwind};

fn hpa_graph(nodes: usize) -> Arc<WaterGraph> {
    let n = AbstractNode { id: 0, x: 0, y: 0, tile: 0, component_id: 0 };
    Arc::new(WaterGraph { nodes: vec![n; nodes], ..Default::default() })
}

fn path_of(v: &[u32]) -> Option<Vec<u32>> {
    Some(v.to_vec())
}

// ---------------------------------------------------------------- WaterPathMemo

#[test]
fn memo_hit_moves_to_end_and_returns_a_copy() {
    let mut m = WaterPathMemo::new(100, 0);
    let mut calls = 0;
    assert_eq!(
        m.find_path(0, Src::One(1), 2, |_, _| {
            calls += 1;
            path_of(&[1, 5, 2])
        }),
        path_of(&[1, 5, 2])
    );
    assert_eq!(
        m.find_path(0, Src::One(3), 4, |_, _| {
            calls += 1;
            None
        }),
        None
    );
    assert_eq!(m.entry_count(), 2);
    assert_eq!(m.byte_count(), 12 + 16); // 3 × 4 bytes + 16 for null
    // Hit: no inner call, re-inserted at the end (LRU order 3→4, 1→2).
    assert_eq!(
        m.find_path(0, Src::One(1), 2, |_, _| {
            calls += 1;
            None
        }),
        path_of(&[1, 5, 2])
    );
    assert_eq!(calls, 2);
    let keys: Vec<u64> = m.paths.keys().copied().collect();
    assert_eq!(keys, [3 * 100 + 4, 100 + 2]);
    // A null hit is a hit too.
    assert_eq!(
        m.find_path(0, Src::One(3), 4, |_, _| {
            calls += 1;
            path_of(&[9])
        }),
        None
    );
    assert_eq!(calls, 2);
}

#[test]
fn memo_array_source_bypasses_the_memo() {
    let mut m = WaterPathMemo::new(100, 0);
    let mut calls = 0;
    for _ in 0..2 {
        m.find_path(0, Src::Many(&[1]), 2, |_, _| {
            calls += 1;
            path_of(&[1, 2])
        });
    }
    assert_eq!((calls, m.entry_count()), (2, 0), "typeof from !== number → inner, no store");
}

#[test]
fn memo_clears_on_water_version_change_only() {
    let mut m = WaterPathMemo::new(100, 7);
    m.find_path(7, Src::One(1), 2, |_, _| path_of(&[1, 2]));
    assert_eq!(m.entry_count(), 1);
    m.find_path(7, Src::One(1), 3, |_, _| path_of(&[1, 3]));
    assert_eq!(m.entry_count(), 2);
    // A different main-map waterVersion drops everything before answering.
    let mut called = false;
    m.find_path(8, Src::One(1), 2, |_, _| {
        called = true;
        path_of(&[1, 9, 2])
    });
    assert!(called);
    assert_eq!((m.entry_count(), m.byte_count(), m.water_version), (1, 12, 8));
    // Even an array query (not memoised) applies the version check first.
    m.find_path(9, Src::Many(&[1, 2]), 2, |_, _| None);
    assert_eq!((m.entry_count(), m.water_version), (0, 9));
}

/// Mandatory: memo LRU eviction order at a shrunk budget.
#[test]
fn memo_lru_eviction_order_at_shrunk_budget() {
    // Budget 40 bytes: entries of 8 (2 tiles), 16 (null), 12 (3 tiles), 8 (2 tiles).
    let mut m = WaterPathMemo::with_max_bytes(1000, 0, 40);
    m.find_path(0, Src::One(1), 10, |_, _| path_of(&[1, 10])); // 8
    m.find_path(0, Src::One(2), 20, |_, _| None); // 16 → 24
    m.find_path(0, Src::One(3), 30, |_, _| path_of(&[3, 4, 30])); // 12 → 36
    // Touch 1→10 so 2→20 becomes the oldest.
    m.find_path(0, Src::One(1), 10, |_, _| unreachable!("hit"));
    // +8 → 44 > 40: evict the first live key (2→20, 16 bytes) → 28.
    m.find_path(0, Src::One(5), 50, |_, _| path_of(&[5, 50]));
    let keys: Vec<u64> = m.paths.keys().copied().collect();
    assert_eq!(keys, [3 * 1000 + 30, 1000 + 10, 5 * 1000 + 50]);
    assert_eq!(m.byte_count(), 28);
    // A single entry larger than the budget evicts everything older, then itself.
    m.find_path(0, Src::One(6), 60, |_, _| Some((0..20).collect()));
    assert_eq!((m.entry_count(), m.byte_count()), (0, 0));
    // The miss result is still returned to the caller.
    assert_eq!(m.find_path(0, Src::One(7), 70, |_, _| Some((0..20).collect())).map(|p| p.len()), Some(20));
}

// ---------------------------------------------------------------- chain slots

fn cx_run<R>(st: &mut State, f: impl FnOnce(&mut Ctx) -> R) -> R {
    let mut sc = Scratch::new();
    let mut cx = Ctx::new(st, &mut sc, None);
    f(&mut cx)
}

/// Mandatory: the chain kind is fixed at slot creation.
#[test]
fn chain_kind_is_fixed_at_slot_creation() {
    let mut st = state_with(5, 5, ISLAND);
    // No nav mesh → simple chain at version 0.
    let v0 = cx_run(&mut st, ensure_current_chain);
    assert_eq!(v0, 0);
    assert_eq!(st.water.chains.slot(0).map(|s| s.kind), Some(ChainKind::Simple));
    // A graph with < 100 nodes is still simple; the slot is not rebuilt at the same version.
    st.water.graph = Some(hpa_graph(99));
    cx_run(&mut st, ensure_current_chain);
    assert_eq!(st.water.chains.retired.len(), 0);
    // Rebuild (version 1) with a 100-node graph → HPA; the old slot keeps Simple.
    st.water.graph = Some(hpa_graph(100));
    st.water.water_graph_version = 1;
    let v1 = cx_run(&mut st, ensure_current_chain);
    assert_eq!(v1, 1);
    assert_eq!(st.water.chains.slot(1).map(|s| s.kind), Some(ChainKind::Hpa));
    assert_eq!(st.water.chains.slot(0).map(|s| s.kind), Some(ChainKind::Simple));
    // A stepper on slot 0 dispatches on slot 0's kind although the graph is now HPA-sized.
    let mut seen = Vec::new();
    cx_run(&mut st, |cx| {
        chain_find_path_with(cx, 0, false, Src::One(1), 2, &mut |_: &mut Ctx, k: ChainKind, _: Src<'_>, _: TileRef| {
            seen.push(k);
            None
        });
        chain_find_path_with(cx, 1, false, Src::One(1), 2, &mut |_: &mut Ctx, k: ChainKind, _: Src<'_>, _: TileRef| {
            seen.push(k);
            None
        });
    });
    assert_eq!(seen, [ChainKind::Simple, ChainKind::Hpa]);
    // The kind of a slot built while the nav mesh is disabled is Simple however many nodes.
    st.water.graph = None;
    st.water.water_graph_version = 2;
    cx_run(&mut st, ensure_current_chain);
    assert_eq!(st.water.chains.slot(2).map(|s| s.kind), Some(ChainKind::Simple));
    assert_eq!(st.water.chains.retired.iter().map(|s| s.version).collect::<Vec<_>>(), [0, 1]);
}

/// Mandatory: a stale memoized stepper hits the OLD memo after a rebuild and stores misses
/// back into it (end to end through the simple chain on the island map).
#[test]
fn stale_memoized_stepper_uses_old_memo_and_stores_misses_there() {
    // 10×10: open water with an island in the middle.
    let grid = "
        WWWWWWWWWW
        WWWWWWWWWW
        WWWWWWWWWW
        WWWLLLLWWW
        WWWLLLLWWW
        WWWLLLLWWW
        WWWLLLLWWW
        WWWWWWWWWW
        WWWWWWWWWW
        WWWWWWWWWW";
    let mut st = state_with(10, 10, grid);
    let (a, b, c) = (r(&st.map, 0, 0), r(&st.map, 9, 9), r(&st.map, 9, 0));
    let mut pf = cx_run(&mut st, |cx| WaterPathFinder::new(cx, 2, true));
    assert_eq!((pf.chain_version, pf.water_graph_version, pf.pending_version), (0, 0, -1));

    let p_ab = cx_run(&mut st, |cx| pf.find_path(cx, &[a], b)).expect("water path");
    assert_eq!((p_ab[0], *p_ab.last().unwrap()), (a, b));
    assert_eq!(st.water.chains.slot(0).unwrap().memo.entry_count(), 1);

    // Simulate a graph rebuild: the version moves on, the stagger keeps the old slot.
    st.water.water_graph_version = 1;
    // (1) stale hit: same key, answered from slot 0's memo, nothing created.
    let p_ab2 = cx_run(&mut st, |cx| pf.find_path(cx, &[a], b));
    assert_eq!(p_ab2.as_ref(), Some(&p_ab));
    assert_eq!((pf.chain_version, pf.stagger_countdown, pf.pending_version), (0, 1, 1));
    assert!(st.water.chains.slot(1).is_none(), "no new slot while staggering");
    // (2) stale miss: computed (on the current map) and stored back into slot 0's memo.
    cx_run(&mut st, |cx| pf.find_path(cx, &[a], c)).expect("water path");
    assert_eq!(st.water.chains.slot(0).unwrap().memo.entry_count(), 2);
    assert_eq!(pf.stagger_countdown, 0);
    // (3) countdown over: swap to the current slot; its fresh memo takes the miss.
    cx_run(&mut st, |cx| pf.find_path(cx, &[a], b)).expect("water path");
    assert_eq!((pf.chain_version, pf.water_graph_version), (1, 1));
    assert_eq!(st.water.chains.slot(1).unwrap().memo.entry_count(), 1);
    assert_eq!(st.water.chains.slot(0).unwrap().memo.entry_count(), 2, "old memo untouched");
    assert!(cx_run(&mut st, |cx| pf.take_rebuilt(cx)));
    assert!(!cx_run(&mut st, |cx| pf.take_rebuilt(cx)));
}

/// `ensureFresh` restarts the countdown only for a new pending version, and a stagger of 0
/// swaps immediately (`PathFinder.ts:271-295`).
#[test]
fn stagger_countdown_rules() {
    let mut st = state_with(5, 5, ISLAND);
    let mut pf = cx_run(&mut st, |cx| WaterPathFinder::new(cx, 3, false));
    st.water.water_graph_version = 1;
    cx_run(&mut st, |cx| pf.ensure_fresh(cx));
    assert_eq!((pf.pending_version, pf.stagger_countdown, pf.chain_version), (1, 2, 0));
    // A second rebuild while counting down restarts the countdown for the new version.
    st.water.water_graph_version = 2;
    cx_run(&mut st, |cx| pf.ensure_fresh(cx));
    assert_eq!((pf.pending_version, pf.stagger_countdown), (2, 2));
    cx_run(&mut st, |cx| pf.ensure_fresh(cx));
    cx_run(&mut st, |cx| pf.ensure_fresh(cx));
    assert_eq!((pf.stagger_countdown, pf.chain_version, pf.rebuilt), (0, 0, false));
    cx_run(&mut st, |cx| pf.ensure_fresh(cx));
    assert_eq!((pf.chain_version, pf.water_graph_version, pf.rebuilt), (2, 2, true));
    // Stagger 0: immediate.
    let mut q = cx_run(&mut st, |cx| WaterPathFinder::new(cx, 0, false));
    st.water.water_graph_version = 3;
    cx_run(&mut st, |cx| q.ensure_fresh(cx));
    assert_eq!((q.chain_version, q.rebuilt), (3, true));
}

/// The stepper's cached path survives the stagger window (the old stepper keeps it) and is
/// dropped on the swap.
#[test]
fn stale_stepper_keeps_cached_path_until_swap() {
    let mut st = state_with(5, 5, ISLAND);
    let (a, b) = (r(&st.map, 0, 0), r(&st.map, 4, 4));
    let mut pf = cx_run(&mut st, |cx| WaterPathFinder::new(cx, 1, false));
    let n1 = cx_run(&mut st, |cx| pf.next(cx, a, b, None));
    let PathResult::Next(n1) = n1 else { panic!("expected NEXT, got {n1:?}") };
    let cached = pf.stepper.path.clone();
    assert!(cached.is_some());
    st.water.water_graph_version = 1;
    let n2 = cx_run(&mut st, |cx| pf.next(cx, n1, b, None));
    assert!(matches!(n2, PathResult::Next(_)));
    assert_eq!(pf.stepper.path, cached, "same cached path during the stagger");
    let PathResult::Next(n2) = n2 else { unreachable!() };
    cx_run(&mut st, |cx| pf.next(cx, n2, b, None));
    assert_eq!(pf.chain_version, 1);
    assert_ne!(pf.stepper.path, cached, "fresh stepper after the swap");
}

/// `tileStepperConfig`: invalid refs are NOT_FOUND (preCheck), `dist` uses manhattan.
#[test]
fn tile_stepper_config_precheck_and_distance() {
    let mut st = state_with(5, 5, ISLAND);
    let (a, b) = (r(&st.map, 0, 0), r(&st.map, 4, 0));
    let mut ws = cx_run(&mut st, WaterStepper::new);
    assert_eq!(cx_run(&mut st, |cx| ws.next(cx, a, 25, None)), PathResult::NotFound);
    assert_eq!(cx_run(&mut st, |cx| ws.next(cx, 99, b, None)), PathResult::NotFound);
    // dist >= manhattan → COMPLETE at `from`.
    assert_eq!(cx_run(&mut st, |cx| ws.next(cx, a, b, Some(4.0))), PathResult::Complete(a));
    assert!(matches!(cx_run(&mut st, |cx| ws.next(cx, a, b, Some(3.0))), PathResult::Next(_)));
    // findPath with every source invalid → null without searching.
    assert_eq!(cx_run(&mut st, |cx| ws.find_path(cx, &[99, 100], b)), None);
}

/// `pathForTraversal`: the remainder after `next`, else a one-shot query, prefixed with
/// `from` when needed; `[from]` when there is no route.
#[test]
fn path_for_traversal_rules() {
    let mut st = state_with(5, 5, ISLAND);
    let (a, b) = (r(&st.map, 0, 0), r(&st.map, 4, 4));
    let mut pf = cx_run(&mut st, |cx| WaterPathFinder::new(cx, 0, true));
    let one_shot = cx_run(&mut st, |cx| pf.path_for_traversal(cx, a, b));
    assert_eq!(one_shot[0], a);
    assert_eq!(*one_shot.last().unwrap(), b);
    let PathResult::Next(n1) = cx_run(&mut st, |cx| pf.next(cx, a, b, None)) else { panic!() };
    let rest = cx_run(&mut st, |cx| pf.path_for_traversal(cx, n1, b));
    assert_eq!(rest[0], n1);
    assert_eq!(rest, pf.stepper.path_after_next().unwrap());
    // No route (an invalid destination fails the preCheck) → [from].
    let mut q = cx_run(&mut st, |cx| WaterPathFinder::new(cx, 0, false));
    assert_eq!(cx_run(&mut st, |cx| q.path_for_traversal(cx, a, 25)), vec![a]);
}

/// A TS throw inside the chain: `next` answers NOT_FOUND (and resets scratch guards);
/// `findPath` lets it propagate. Either way the lent-out `seg_vis` and memo are back in
/// `State` (post-throw state as in TS).
#[test]
fn ts_throw_inside_chain_restores_lent_state() {
    // An inconsistent minimap (1×1 for a 5×5 map) makes `miniMap.ref` throw
    // "Invalid coordinates" inside the MiniMapTransformer.
    let mut st = state_with(5, 5, ISLAND);
    let (a, b) = (r(&st.map, 0, 0), r(&st.map, 4, 4));
    st.mini = crate::path::testkit::game_map(1, 1, "W");
    st.water.seg_vis.filled = vec![42];
    let mut pf = cx_run(&mut st, |cx| WaterPathFinder::new(cx, 0, true));
    st.water.chains.current.as_mut().unwrap().memo.paths.insert(7, None);
    st.water.chains.current.as_mut().unwrap().memo.live_bytes = 16;

    let mut sc = Scratch::new();
    sc.bfs_mini.busy = true; // a guard a TS `finally` would have released
    let res = {
        let mut cx = Ctx::new(&mut st, &mut sc, None);
        pf.next(&mut cx, a, b, None)
    };
    assert_eq!(res, PathResult::NotFound);
    assert!(!sc.bfs_mini.busy, "after_caught_throw ran");
    assert_eq!(st.water.seg_vis.filled, vec![42]);
    assert_eq!(st.water.chains.slot(0).unwrap().memo.entry_count(), 1);

    let thrown = catch_unwind(AssertUnwindSafe(|| cx_run(&mut st, |cx| pf.find_path(cx, &[a], b))));
    let payload = thrown.expect_err("findPath has no try/catch");
    let t = of_det::throw::as_ts_throw(&*payload).expect("TsThrow");
    assert!(t.msg.starts_with("Invalid coordinates"), "{}", t.msg);
    assert_eq!(st.water.seg_vis.filled, vec![42]);
    assert_eq!(st.water.chains.slot(0).unwrap().memo.entry_count(), 1);
}

/// The GC keeps a retired slot exactly while a stepper names it (`#[refs(chain_version)]`).
#[test]
fn steppers_report_their_chain_version() {
    let pf = WaterPathFinder { chain_version: 4, ..Default::default() };
    let ws = WaterStepper { chain_version: 5, ..Default::default() };
    let mut v = crate::canon::refs::RefLog::default();
    crate::canon::Refs::visit_refs(&pf, &mut v);
    crate::canon::Refs::visit_refs(&ws, &mut v);
    assert_eq!(v.chain_versions, [4, 5]);
}
