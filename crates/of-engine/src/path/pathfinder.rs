//! `pathfinding/PathFinder.ts`: water chain slots, the LRU `WaterPathMemo`, `WaterPathFinder`,
//! and the `PathFinding.*` factories. Owner: **P4b**. ARCHITECTURE §3.8.
//!
//! - `_waterChainCache` (one `{version, chain, memo}` per game, `:42-45,145-162`) is **state**:
//!   [`ChainSlots`] in `state.water.chains`. [`ensure_current_chain`] runs at exactly the TS
//!   `sharedWaterChain` call sites; the chain kind is fixed at slot creation.
//! - [`WaterPathMemo`] is a literal port (LRU, 24 MB byte budget, cleared only when the main
//!   map's `waterVersion` changes, `:79-138`): observable state, always on.
//! - [`WaterPathFinder`] holds **no graph reference**: `chain_version` names its slot; its
//!   searches run on the current graph/components (HPA), with its slot's kind and memo.
//!
//! **What a TS stepper captured, and where it lives here.** `new PathFinderStepper(
//! sharedWaterChain(game, memoized), ..)` captures either the chain object or the memo
//! object of the slot current at that moment. The chain object's behaviour is its kind
//! (every transformer is stateless and the HPA it wraps is the game's one HPA, which always
//! routes on the current graph); the memo object is the slot's [`WaterPathMemo`]. So a
//! stepper is `(chain_version, memoized)`, and [`chain_find_path`] dispatches on the named
//! slot's kind and (if memoized) its memo.
//!
//! **Game reads.** `game.waterGraphVersion()` is `WaterManager._waterGraphVersion`
//! (`GameImpl.ts:1632-1634`), `game.map().waterVersion()` is `GameMapImpl.waterVersion_`
//! and `graph.nodeCount` is `_nodes.length` (`AbstractGraph.ts:60-62`): this file reads the
//! public fields that hold them. Tile predicates go through [`MapRead`].

use crate::StateClone;
use crate::ctx::Ctx;
use crate::ids::{StationId, TileRef};
use crate::jsorder::JsMap;
use crate::map::GameMap;
use crate::path::MapRead;
use crate::path::stepper::{StepFinder, StepperState, restore_on_unwind};
use crate::path::types::{PathResult, Src};
use std::sync::Arc;

/// Which chain `buildWaterChain` built (`:47-66`): `!hpa || !graph || nodeCount < 100` →
/// simple `AStarWater` chain, else the HPA chain.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChainKind {
    #[default]
    Simple,
    Hpa,
}
crate::plain_copy!(ChainKind);

/// `WaterPathMemo` (`:79-138`).
#[derive(StateClone, Debug, Default)]
pub struct WaterPathMemo {
    /// The main map's `waterVersion()` the entries belong to.
    pub water_version: u32,
    /// `paths: Map<number, Uint32Array | null>`, key `from * numTiles + to`; LRU order.
    pub paths: JsMap<u64, Option<Arc<[u32]>>>,
    /// `liveBytes` (`byteLength`, 16 for `null`).
    pub live_bytes: u64,
    /// 24_000_000.
    pub max_bytes: u64,
    /// `numTiles` of the main map.
    pub num_tiles: u64,
}

/// `WaterPathMemo` max bytes (`PathFinder.ts:79-100`).
pub const WATER_MEMO_MAX_BYTES: u64 = 24_000_000;

/// Byte size of a stored entry: `byteLength` of the `Uint32Array`, 16 for `null`
/// (`:127`, `:133-134`).
#[inline]
fn entry_bytes(v: &Option<Arc<[u32]>>) -> u64 {
    match v {
        None => 16,
        Some(a) => 4 * a.len() as u64,
    }
}

impl WaterPathMemo {
    /// `new WaterPathMemo(inner, numTiles, currentWaterVersion)` (`:84-93`): the version is
    /// read at construction.
    pub fn new(num_tiles: u64, water_version: u32) -> Self {
        Self::with_max_bytes(num_tiles, water_version, WATER_MEMO_MAX_BYTES)
    }
    /// With a test budget (`maxBytes`, "Tests shrink it to reach eviction", `:89-90`).
    pub fn with_max_bytes(num_tiles: u64, water_version: u32, max_bytes: u64) -> Self {
        WaterPathMemo { water_version, paths: JsMap::new(), live_bytes: 0, max_bytes, num_tiles }
    }
    /// `entryCount` (`:94-97`).
    pub fn entry_count(&self) -> usize {
        self.paths.len()
    }
    /// `byteCount` (`:98-101`).
    pub fn byte_count(&self) -> u64 {
        self.live_bytes
    }

    /// `findPath(from, to)` (`:102-137`). `current_water_version` is
    /// `currentWaterVersion()` (the main map's `waterVersion()`); `inner` is the chain.
    pub fn find_path(
        &mut self,
        current_water_version: u32,
        from: Src<'_>,
        to: TileRef,
        inner: impl FnOnce(Src<'_>, TileRef) -> Option<Vec<TileRef>>,
    ) -> Option<Vec<TileRef>> {
        // TS: :110-115
        let wv = current_water_version;
        if wv != self.water_version {
            self.water_version = wv;
            self.paths.clear();
            self.live_bytes = 0;
        }
        // TS: :116 `if (typeof from !== "number") return this.inner.findPath(from, to)`
        let f = match from {
            Src::One(f) => f,
            Src::Many(_) => return inner(from, to),
        };
        // TS: :117 (JS number, exact: from * numTiles + to < 2^53)
        let key = u64::from(f) * self.num_tiles + u64::from(to);
        // TS: :118-124
        if let Some(hit) = self.paths.get(&key).cloned() {
            self.paths.remove(&key);
            self.paths.insert(key, hit.clone());
            return hit.map(|a| a.to_vec()); // `Array.from(hit)`: callers get their own copy
        }
        // TS: :125-128
        let path = inner(from, to);
        let stored: Option<Arc<[u32]>> = path.as_deref().map(Arc::from);
        self.live_bytes += entry_bytes(&stored);
        self.paths.insert(key, stored);
        // TS: :129-135
        while self.live_bytes > self.max_bytes {
            // `paths.keys().next().value!`: liveBytes > maxBytes >= 0 implies an entry.
            let oldest_key = *self.paths.first_live().expect("rust-invariant: liveBytes counts the live entries");
            let oldest = self.paths.remove(&oldest_key).expect("rust-invariant: first_live is live");
            self.live_bytes -= entry_bytes(&oldest);
        }
        path
    }
}

/// One `{ version, chain, memo }`.
#[derive(StateClone, Debug, Default)]
pub struct ChainSlot {
    pub version: u32,
    pub kind: ChainKind,
    pub memo: WaterPathMemo,
}

/// The current chain plus retired ones still named by a stale `WaterPathFinder`.
#[derive(StateClone, Debug, Default)]
pub struct ChainSlots {
    pub current: Option<ChainSlot>,
    pub retired: Vec<ChainSlot>,
}

impl ChainSlots {
    /// The `if (!cached || cached.version !== version)` rebuild of `sharedWaterChain`
    /// (`:146-160`): build a slot for `version` (kind and memo made only then), retiring the
    /// previous current slot. Returns `version`.
    pub fn ensure(&mut self, version: u32, kind: ChainKind, memo: impl FnOnce() -> WaterPathMemo) -> u32 {
        let fresh = match &self.current {
            Some(c) => c.version != version,
            None => true,
        };
        if fresh {
            let slot = ChainSlot { version, kind, memo: memo() };
            if let Some(old) = self.current.replace(slot) {
                self.retired.push(old);
            }
        }
        version
    }
    /// The slot named `version` (current or retired).
    pub fn slot(&self, version: u32) -> Option<&ChainSlot> {
        match &self.current {
            Some(c) if c.version == version => Some(c),
            _ => self.retired.iter().find(|s| s.version == version),
        }
    }
    pub fn slot_mut(&mut self, version: u32) -> Option<&mut ChainSlot> {
        match &mut self.current {
            Some(c) if c.version == version => Some(c),
            _ => self.retired.iter_mut().find(|s| s.version == version),
        }
    }
}

/// `buildWaterChain`'s kind decision (`:48-52`): `hpa` and `graph` are both null exactly
/// when the nav mesh is disabled (`WaterManager.ts:69-93`), i.e. `water.graph` is `None`.
pub fn chain_kind_for(st: &crate::state::State) -> ChainKind {
    match &st.water.graph {
        None => ChainKind::Simple,
        Some(g) if g.nodes.len() < 100 => ChainKind::Simple,
        Some(_) => ChainKind::Hpa,
    }
}

/// `sharedWaterChain(game, memoized)` (`:145-162`): rebuild the current slot if the water graph
/// version changed; returns the current slot's version.
pub fn ensure_current_chain(cx: &mut Ctx) -> u32 {
    let version = cx.st.water.water_graph_version;
    let kind = chain_kind_for(cx.st);
    // `game.map().width() * game.map().height()` and `currentWaterVersion()` at construction.
    let num_tiles = u64::from(cx.st.map.w) * u64::from(cx.st.map.h);
    let wv = cx.st.map.water_version;
    cx.st.water.chains.ensure(version, kind, || WaterPathMemo::new(num_tiles, wv))
}

/// Run the chain (or its memo) of slot `chain_version` for `from → to` (`findPath` of the
/// chain / `WaterPathMemo.findPath`, `:102-138`). `from` per [`Src::from_slice`].
pub fn chain_find_path(
    cx: &mut Ctx,
    chain_version: u32,
    memoized: bool,
    from: &[TileRef],
    to: TileRef,
) -> Option<Vec<TileRef>> {
    chain_find_path_src(cx, chain_version, memoized, Src::from_slice(from), to)
}

/// [`chain_find_path`] with an explicit scalar/array source.
pub fn chain_find_path_src(
    cx: &mut Ctx,
    chain_version: u32,
    memoized: bool,
    from: Src<'_>,
    to: TileRef,
) -> Option<Vec<TileRef>> {
    chain_find_path_with(cx, chain_version, memoized, from, to, &mut run_water_chain)
}

/// The chain itself on the current game: `seg_vis` is lent to the chain and restored on
/// every exit (a TS throw inside the HPA keeps the segments it filled, as TS does).
fn run_water_chain(cx: &mut Ctx, kind: ChainKind, from: Src<'_>, to: TileRef) -> Option<Vec<TileRef>> {
    restore_on_unwind(
        cx,
        |cx| std::mem::take(&mut cx.st.water.seg_vis),
        |cx, seg| cx.st.water.seg_vis = seg,
        |cx, seg| {
            let mut rcx = cx.rcx();
            crate::path::builder::water_chain_find_path_src(&mut rcx, seg, kind, from, to)
        },
    )
}

type ChainRunner<'r> = dyn FnMut(&mut Ctx, ChainKind, Src<'_>, TileRef) -> Option<Vec<TileRef>> + 'r;

/// [`chain_find_path_src`] with the chain runner supplied (tests stub the chain).
fn chain_find_path_with(
    cx: &mut Ctx,
    chain_version: u32,
    memoized: bool,
    from: Src<'_>,
    to: TileRef,
    run: &mut ChainRunner<'_>,
) -> Option<Vec<TileRef>> {
    let kind = cx
        .st
        .water
        .chains
        .slot(chain_version)
        .expect("rust-invariant: a live stepper's chain slot is kept by the GC")
        .kind;
    if !memoized {
        return run(cx, kind, from, to);
    }
    // `currentWaterVersion` = `() => game.map().waterVersion()` (`:156`).
    let wv = cx.st.map.water_version;
    restore_on_unwind(
        cx,
        |cx| std::mem::take(&mut cx.st.water.chains.slot_mut(chain_version).expect("rust-invariant: checked").memo),
        |cx, memo| cx.st.water.chains.slot_mut(chain_version).expect("rust-invariant: checked").memo = memo,
        |cx, memo| memo.find_path(wv, from, to, |f, t| run(cx, kind, f, t)),
    )
}

/// `tileStepperConfig(game)` (`:361-373`) plus the water chain of one slot.
struct WaterFinder<'c, 'a> {
    cx: &'c mut Ctx<'a>,
    chain_version: u32,
    memoized: bool,
}

/// `tileStepperConfig.preCheck` (`:365-371`): refs are numbers here, so only `isValidRef`.
#[inline]
fn tile_pre_check(map: &GameMap, from: TileRef, to: TileRef) -> Option<PathResult<u32>> {
    if !MapRead::is_valid_ref(map, f64::from(from)) || !MapRead::is_valid_ref(map, f64::from(to)) {
        Some(PathResult::NotFound)
    } else {
        None
    }
}

impl StepFinder for WaterFinder<'_, '_> {
    fn pre_check(&mut self, from: u32, to: u32) -> Option<PathResult<u32>> {
        tile_pre_check(&self.cx.st.map, from, to)
    }
    fn has_pre_check(&self) -> bool {
        true
    }
    fn distance(&mut self, a: u32, b: u32) -> Option<f64> {
        Some(f64::from(MapRead::manhattan_dist(&self.cx.st.map, a, b)))
    }
    fn find_path(&mut self, from: Src<'_>, to: u32) -> Option<Vec<u32>> {
        chain_find_path_src(self.cx, self.chain_version, self.memoized, from, to)
    }
    fn after_caught_throw(&mut self) {
        self.cx.sc.after_caught_throw();
    }
}

/// `PathFinding.Water(game)` / `WaterSimple` (`:189-200`): a stepper over the shared chain.
#[derive(StateClone, Debug, Default, PartialEq, Eq)]
pub struct WaterStepper {
    #[refs(chain_version)]
    pub chain_version: u32,
    pub stepper: StepperState,
}

impl WaterStepper {
    pub fn new(cx: &mut Ctx) -> Self {
        WaterStepper { chain_version: ensure_current_chain(cx), stepper: StepperState::default() }
    }
    pub fn next(&mut self, cx: &mut Ctx, from: TileRef, to: TileRef, dist: Option<f64>) -> PathResult<TileRef> {
        let mut f = WaterFinder { cx, chain_version: self.chain_version, memoized: false };
        self.stepper.next(&mut f, from, to, dist)
    }
    /// `findPath(from, to)`; `from` per [`Src::from_slice`].
    pub fn find_path(&mut self, cx: &mut Ctx, from: &[TileRef], to: TileRef) -> Option<Vec<TileRef>> {
        self.find_path_src(cx, Src::from_slice(from), to)
    }
    pub fn find_path_src(&mut self, cx: &mut Ctx, from: Src<'_>, to: TileRef) -> Option<Vec<TileRef>> {
        let mut f = WaterFinder { cx, chain_version: self.chain_version, memoized: false };
        self.stepper.find_path(&mut f, from, to)
    }
    pub fn invalidate(&mut self) {
        self.stepper.invalidate();
    }
}

/// `tileStepperConfig(game)` plus the rail chain (`sharedRailChain`).
struct RailFinder<'c, 'a> {
    cx: &'c mut Ctx<'a>,
}

impl StepFinder for RailFinder<'_, '_> {
    fn pre_check(&mut self, from: u32, to: u32) -> Option<PathResult<u32>> {
        tile_pre_check(&self.cx.st.map, from, to)
    }
    fn has_pre_check(&self) -> bool {
        true
    }
    fn distance(&mut self, a: u32, b: u32) -> Option<f64> {
        Some(f64::from(MapRead::manhattan_dist(&self.cx.st.map, a, b)))
    }
    fn find_path(&mut self, from: Src<'_>, to: u32) -> Option<Vec<u32>> {
        let st: &crate::state::State = self.cx.st;
        crate::path::builder::rail_chain(&st.map, &st.mini, &mut self.cx.sc.astar_rail, from, to)
    }
    fn after_caught_throw(&mut self) {
        self.cx.sc.after_caught_throw();
    }
}

/// `PathFinding.Rail(game)` (`:202-207`): stepper over the rail chain (`sc.astar_rail`).
#[derive(StateClone, Debug, Default, PartialEq, Eq)]
pub struct RailStepper {
    pub stepper: StepperState,
}

impl RailStepper {
    pub fn new() -> Self {
        RailStepper::default()
    }
    pub fn next(&mut self, cx: &mut Ctx, from: TileRef, to: TileRef, dist: Option<f64>) -> PathResult<TileRef> {
        self.stepper.next(&mut RailFinder { cx }, from, to, dist)
    }
    /// `findPath(from, to)` (`RailNetworkImpl.ts:284` calls it with a scalar).
    pub fn find_path(&mut self, cx: &mut Ctx, from: &[TileRef], to: TileRef) -> Option<Vec<TileRef>> {
        self.stepper.find_path(&mut RailFinder { cx }, Src::from_slice(from), to)
    }
}

/// `PathFinding.Stations` config (`:212-215`: `equals` by id, `distance` = manhattan of the
/// station tiles, no preCheck) plus `StationPathFinder`.
struct StationFinder<'c, 'a> {
    cx: &'c mut Ctx<'a>,
}

impl StepFinder for StationFinder<'_, '_> {
    fn pre_check(&mut self, _: u32, _: u32) -> Option<PathResult<u32>> {
        None
    }
    fn has_pre_check(&self) -> bool {
        false
    }
    fn distance(&mut self, a: u32, b: u32) -> Option<f64> {
        let st = &*self.cx.st;
        let (ta, tb) = (st.station_tile(StationId(a)), st.station_tile(StationId(b)));
        Some(f64::from(MapRead::manhattan_dist(&st.map, ta, tb)))
    }
    fn find_path(&mut self, from: Src<'_>, to: u32) -> Option<Vec<u32>> {
        let mut one = [0];
        let from: Vec<StationId> = from.as_slice(&mut one).iter().map(|&s| StationId(s)).collect();
        let mut rcx = self.cx.rcx();
        crate::path::station::find_path(&mut rcx, &from, StationId(to)).map(|p| p.into_iter().map(|s| s.0).collect())
    }
    fn after_caught_throw(&mut self) {
        self.cx.sc.after_caught_throw();
    }
}

/// `PathFinding.Stations(game)` (`:209-216`): stepper over `StationPathFinder`.
#[derive(StateClone, Debug, Default, PartialEq, Eq)]
pub struct StationStepper {
    pub stepper: StepperState,
}

impl StationStepper {
    pub fn new() -> Self {
        StationStepper::default()
    }
    pub fn next(&mut self, cx: &mut Ctx, from: StationId, to: StationId, dist: Option<f64>) -> PathResult<StationId> {
        self.stepper.next(&mut StationFinder { cx }, from.0, to.0, dist).map(StationId)
    }
    /// `findPath(from, to)` (`RailNetworkImpl.ts:288`).
    pub fn find_path(&mut self, cx: &mut Ctx, from: StationId, to: StationId) -> Option<Vec<StationId>> {
        self.stepper
            .find_path(&mut StationFinder { cx }, Src::One(from.0), to.0)
            .map(|p| p.into_iter().map(StationId).collect())
    }
}

/// `{ equals: (a, b) => a === b }` plus `AirPathFinder` (`:218-224`).
struct AirFinder<'c, 'a> {
    cx: &'c mut Ctx<'a>,
    seed: i32,
}

impl StepFinder for AirFinder<'_, '_> {
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
        match from {
            Src::One(f) => Some(crate::path::air::find_path_m(&self.cx.st.map, self.seed, f, to)),
            Src::Many(_) => of_det::ts_throw!("AirPathFinder does not support multiple start points"),
        }
    }
    fn after_caught_throw(&mut self) {
        self.cx.sc.after_caught_throw();
    }
}

/// `PathFinding.Air(game)` (`:218-225`): `AirPathFinder` (seed = `game.ticks()` at creation)
/// wrapped in a stepper. Snapshot: `pathFinder { seed, stepper }`.
#[derive(StateClone, Debug, Default, PartialEq, Eq)]
pub struct AirStepper {
    pub seed: i32,
    pub stepper: StepperState,
}

impl AirStepper {
    pub fn new(ticks: i32) -> Self {
        AirStepper { seed: ticks, stepper: StepperState::default() }
    }
    pub fn next(&mut self, cx: &mut Ctx, from: TileRef, to: TileRef, dist: Option<f64>) -> PathResult<TileRef> {
        let seed = self.seed;
        self.stepper.next(&mut AirFinder { cx, seed }, from, to, dist)
    }
    /// `findPath(from, to)` through the stepper (no preCheck).
    pub fn find_path(&mut self, cx: &mut Ctx, from: TileRef, to: TileRef) -> Option<Vec<TileRef>> {
        let seed = self.seed;
        self.stepper.find_path(&mut AirFinder { cx, seed }, Src::One(from), to)
    }
    pub fn invalidate(&mut self) {
        self.stepper.invalidate();
    }
}

/// `WaterPathFinder` (`:232-360`); snapshot `pathFinder { stagger, memoized,
/// waterGraphVersion, rebuilt, staggerCountdown, pendingVersion, stepper }`.
#[derive(StateClone, Debug, Default, PartialEq, Eq)]
pub struct WaterPathFinder {
    pub stagger: i32,
    pub memoized: bool,
    pub water_graph_version: u32,
    pub rebuilt: bool,
    pub stagger_countdown: i32,
    /// `_pendingVersion = -1`.
    pub pending_version: i64,
    pub stepper: StepperState,
    /// non-snapshot: the `ChainSlot` version the stepper's chain belongs to.
    #[refs(chain_version)]
    pub chain_version: u32,
}

impl WaterPathFinder {
    /// `STAGGER_SPREAD = 50` (`:261`).
    pub const STAGGER_SPREAD: i32 = 50;

    /// `new WaterPathFinder(game, stagger = 0, memoized = false)` (`:247-259`).
    pub fn new(cx: &mut Ctx, stagger: i32, memoized: bool) -> Self {
        // TS: :252-255 `sharedWaterChain(game, _memoized)` first, then the version read.
        let chain_version = ensure_current_chain(cx);
        WaterPathFinder {
            stagger,
            memoized,
            water_graph_version: cx.st.water.water_graph_version,
            rebuilt: false,
            stagger_countdown: 0,
            pending_version: -1,
            stepper: StepperState::default(),
            chain_version,
        }
    }

    /// `ensureFresh()` (`:271-295`).
    fn ensure_fresh(&mut self, cx: &mut Ctx) {
        let v = cx.st.water.water_graph_version;
        if v == self.water_graph_version {
            return;
        }
        // New graph version detected — start or continue the stagger countdown.
        if self.pending_version != i64::from(v) {
            self.pending_version = i64::from(v);
            self.stagger_countdown = self.stagger;
        }
        if self.stagger_countdown > 0 {
            self.stagger_countdown -= 1;
            return; // Keep using old stepper (and its cached path) for now
        }
        // Countdown complete — swap to a fresh stepper around the (now-current) shared chain.
        self.water_graph_version = v;
        self.chain_version = ensure_current_chain(cx);
        self.stepper = StepperState::default();
        self.rebuilt = true;
    }

    /// `get rebuilt` (`:263-269`): ensureFresh, then read-and-reset.
    pub fn take_rebuilt(&mut self, cx: &mut Ctx) -> bool {
        self.ensure_fresh(cx);
        let v = self.rebuilt;
        self.rebuilt = false;
        v
    }
    /// `next(from, to, dist?)` (`:297-301`).
    pub fn next(&mut self, cx: &mut Ctx, from: TileRef, to: TileRef, dist: Option<f64>) -> PathResult<TileRef> {
        self.ensure_fresh(cx);
        let mut f = WaterFinder { cx, chain_version: self.chain_version, memoized: self.memoized };
        self.stepper.next(&mut f, from, to, dist)
    }
    /// `findPath(from, to)` (`:303-311`); `from` per [`Src::from_slice`].
    pub fn find_path(&mut self, cx: &mut Ctx, from: &[TileRef], to: TileRef) -> Option<Vec<TileRef>> {
        self.find_path_src(cx, Src::from_slice(from), to)
    }
    pub fn find_path_src(&mut self, cx: &mut Ctx, from: Src<'_>, to: TileRef) -> Option<Vec<TileRef>> {
        self.ensure_fresh(cx);
        let mut f = WaterFinder { cx, chain_version: self.chain_version, memoized: self.memoized };
        self.stepper.find_path(&mut f, from, to)
    }
    /// `pathForTraversal(from, to)` (`:313-319`).
    pub fn path_for_traversal(&mut self, cx: &mut Ctx, from: TileRef, to: TileRef) -> Vec<TileRef> {
        self.ensure_fresh(cx);
        let path = match self.stepper.path_after_next() {
            Some(p) => Some(p.to_vec()),
            None => {
                let mut f = WaterFinder { cx, chain_version: self.chain_version, memoized: self.memoized };
                self.stepper.find_path(&mut f, Src::One(from), to)
            }
        };
        let path = match path {
            Some(p) if !p.is_empty() => p,
            _ => return vec![from],
        };
        if path[0] == from {
            path
        } else {
            let mut v = Vec::with_capacity(path.len() + 1);
            v.push(from);
            v.extend_from_slice(&path);
            v
        }
    }
    /// `invalidate()` (`:321-323`).
    pub fn invalidate(&mut self) {
        self.stepper.invalidate();
    }
}

/// CS: `AirPathFinderSchema` (`snapshot/PathfinderSnapshots.ts:66-77`): `{seed, stepper}`.
/// Written at merge1 (request P8 #2) by the integrator on behalf of P4b.
impl crate::canon::Canon for AirStepper {
    fn canon(&self, cx: &mut crate::canon::CanonCx<'_>) -> crate::canon::Cs {
        use crate::canon::Cs;
        Cs::Obj(vec![("seed".into(), Cs::Int(i64::from(self.seed))), ("stepper".into(), self.stepper.canon(cx))])
    }
}

#[cfg(test)]
mod tests;
