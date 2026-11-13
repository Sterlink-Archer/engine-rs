//! `PlayerExecution` (`engine/src/execution/PlayerExecution.ts`). Owner: **P6**.
//!
//! Fields: the `Player v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`), then non-snapshot fields. `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1).
//!
//! - The per-call cluster arrays of `removeClusters` (`clusters`, `boxes`, `isPinched`,
//!   `clusterRemoved`, `clusterSizes`, `clusterTileTIds`, `territoryAreas`) live flattened in
//!   `sc.clusters` (ARCHITECTURE §11 item 3; layout in `scratch::clusters`). A cluster is named
//!   by its index; loops that call back into `cx` re-read the scratch by index each step.
//! - `tileTraversalScratch(mg)` is `sc.traversal` (one per thread, ARCHITECTURE §3.8); every
//!   stamp comparison is relative within a pass, so sharing it is unobservable.
//! - `nbuf`/`nbuf8` (`:39-40`) are stack arrays.
//! - The two `iter-snapshot` sites (`:60-79` units, `:107-111` alliances) iterate
//!   [`JsArr::iter_snapshot`](crate::jsarr::JsArr::iter_snapshot) through [`snapshot_loop`].

#[allow(unused_imports)]
use crate::exec::prelude::*;

use crate::api::types::{Cell, unit_groups};
use crate::jsarr::{JsArr, SnapElem};
use crate::map::GameMap;
use crate::plain::PlainState;
use crate::scratch::clusters::ClusterScratch;
use crate::scratch::traversal::TraversalScratch;
use crate::state::State;
use crate::state::player::BoundingBox;
use crate::state::stats::StatNum;

/// `TICKS_PER_CLUSTER_CALC = 20` (`:25`).
const TICKS_PER_CLUSTER_CALC: i32 = 20;
/// `CLUSTER_SURROUNDED = 1` (`:27`).
const CLUSTER_SURROUNDED: u32 = 1;
/// `1e9` / `-1e9` box sentinels (`:642-645, :705-708, :1143-1146`), exact in `i32`.
const BIG: i32 = 1_000_000_000;

/// `PlayerExecution` data.
#[derive(StateClone, Debug)]
pub struct PlayerExec {
    pub active: bool,
    pub initialized: bool,
    pub last_calc: i32,
    pub player: SmallId,
    /// non-snapshot: `clusterTerritoryIds` (`:41`)
    pub cluster_territory_ids: Option<Vec<i32>>,
    /// non-snapshot: `nextTerritoryId = 1` (`:42`)
    pub next_territory_id: i32,
}

/// `for (const x of <JsArr field>)` whose body may filter-reassign that field (ARCHITECTURE
/// §4.7): walks the array object the loop started on, picking up pushes to it while it has
/// not been reassigned. `field` re-selects the live field from the state on every step.
/// iter-snapshot: filter-reassign.
pub(crate) fn snapshot_loop<T: SnapElem + PartialEq + PlainState + Copy>(
    cx: &mut Ctx,
    field: impl Fn(&State) -> &JsArr<T>,
    mut body: impl FnMut(&mut Ctx, T),
) {
    let mut it = field(&*cx.st).iter_snapshot(&mut cx.sc.snap_pool);
    while let Some(x) = it.next(field(&*cx.st)) {
        body(cx, x);
    }
    it.finish(&mut cx.sc.snap_pool);
}

/// The `includeFn` of `floodFillWithGen` (`:1132`): the two closures TS passes.
#[derive(Clone, Copy)]
enum Include {
    /// `(tile) => visited[tile] === borderGen` (`:1068`).
    Stamp(u32),
    /// `(tile) => mg.ownerID(tile) === player.smallID()` (`:817, :867`).
    Owner(u16),
}

impl Include {
    #[inline]
    fn test(self, map: &GameMap, visited: &[u32], t: TileRef) -> bool {
        match self {
            Include::Stamp(g) => visited[t as usize] == g, // in-range: t is a valid tile ref
            Include::Owner(me) => map.owner_id(t) == me,
        }
    }
}

/// `floodFillWithGen(currentGen, visited, [start], diag, includeFn, outBox?)` (`:1127-1204`)
/// with one start tile. Appends the result (mark order) to `out` and returns the box
/// `[minX, minY, maxX, maxY]` (the TS writes it only when `outBox` is given).
fn flood_fill_with_gen(
    map: &GameMap,
    tr: &mut TraversalScratch,
    current_gen: u32,
    start: TileRef,
    diag: bool,
    include: Include,
    out: &mut Vec<TileRef>,
) -> [i32; 4] {
    tr.stack.clear();
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (BIG, BIG, -BIG, -BIG);
    let track = |t: TileRef, min_x: &mut i32, min_y: &mut i32, max_x: &mut i32, max_y: &mut i32| {
        let x = map.x(t) as i32; // int: map coordinates < 2^16
        let y = map.y(t) as i32;
        if x < *min_x {
            *min_x = x;
        }
        if y < *min_y {
            *min_y = y;
        }
        if x > *max_x {
            *max_x = x;
        }
        if y > *max_y {
            *max_y = y;
        }
    };
    // TS: :1149-1163 (one start tile)
    if tr.visited[start as usize] != current_gen && include.test(map, &tr.visited, start) {
        tr.visited[start as usize] = current_gen; // in-range: valid tile ref
        out.push(start);
        tr.stack.push(start);
        track(start, &mut min_x, &mut min_y, &mut max_x, &mut max_y);
    }
    let mut nbuf = [0u32; 8];
    while let Some(tile) = tr.stack.pop() {
        let num_neighbors = if diag {
            map.neighbors8(tile, &mut nbuf)
        } else {
            let mut b4 = [0u32; 4];
            let n = map.neighbors4(tile, &mut b4);
            nbuf[..4].copy_from_slice(&b4);
            n
        };
        for &neighbor in &nbuf[..num_neighbors] {
            if tr.visited[neighbor as usize] == current_gen {
                continue;
            }
            if !include.test(map, &tr.visited, neighbor) {
                continue;
            }
            tr.visited[neighbor as usize] = current_gen;
            out.push(neighbor);
            tr.stack.push(neighbor);
            track(neighbor, &mut min_x, &mut min_y, &mut max_x, &mut max_y);
        }
    }
    [min_x, min_y, max_x, max_y]
}

/// `floodComponent(start, tGen, areaGen)` (`PlayerExecution.ts:310-335`) as a scanline fill.
///
/// TS floods with a DFS stack; its only outputs are the returned size and the `t_gen` stamps,
/// i.e. the **set** of tiles 4-connected to `start` through tiles with
/// `visited < areaGen && owner == me` (`start` itself is stamped unconditionally). That set
/// does not depend on the visit order, so a row-span fill (sequential reads of the owner and
/// stamp rows instead of a random walk) stamps the same tiles and returns the same count
/// (P3 perf, t066; `tests::scanline_flood_equals_dfs_flood`). `mark(t)` sees every stamped tile
/// once (t409 H5: the component labels).
#[allow(clippy::too_many_arguments)]
fn flood_component_scan(
    w: u32,
    h: u32,
    owner: impl Fn(TileRef) -> u16,
    tr: &mut TraversalScratch,
    me: u16,
    start: TileRef,
    t_gen: u32,
    area_gen: u32,
    mut mark: impl FnMut(TileRef),
) -> i32 {
    let visited = &mut tr.visited;
    let stack = &mut tr.stack;
    visited[start as usize] = t_gen; // in-range: valid tile ref
    mark(start);
    stack.clear();
    stack.push(start);
    let mut size = 0i64;
    let last_row = (h - 1) * w;
    while let Some(seed) = stack.pop() {
        let row = seed - seed % w;
        let (mut l, mut r) = (seed, seed);
        while l > row && visited[(l - 1) as usize] < area_gen && owner(l - 1) == me {
            l -= 1;
            visited[l as usize] = t_gen;
            mark(l);
        }
        while r + 1 < row + w && visited[(r + 1) as usize] < area_gen && owner(r + 1) == me {
            r += 1;
            visited[r as usize] = t_gen;
            mark(r);
        }
        size += i64::from(r - l + 1);
        for (has, base) in [(row >= w, l.wrapping_sub(w)), (row < last_row, l + w)] {
            if !has {
                continue;
            }
            let mut in_run = false;
            for t in base..=base + (r - l) {
                if visited[t as usize] < area_gen && owner(t) == me {
                    if !in_run {
                        visited[t as usize] = t_gen;
                        mark(t);
                        stack.push(t);
                        in_run = true;
                    }
                } else {
                    in_run = false;
                }
            }
        }
    }
    size as i32 // int: a tile count, < 2^31
}

/// The flood of `checkAndAssignTerritory` (`PlayerExecution.ts:609-628`) as a scanline fill.
///
/// TS floods 8-connected `owner == me` tiles from `start` with a DFS, stamping `flood_gen` and,
/// for every stamped tile except `start`, setting `clusterTerritoryIds[clusterIndexMap[n] - 1]
/// = territoryId` (one value for the whole flood). Its outputs are the stamped set and the
/// set of assigned clusters, so visit order is unobservable and a row-span fill (adjacent rows
/// scanned over `[l - 1, r + 1]` for 8-connectivity) gives the same state (P3 perf, t066;
/// `tests::flood8_scan_equals_dfs`). `assign(c)` receives each cluster index found.
#[allow(clippy::too_many_arguments)]
fn flood8_assign(
    w: u32,
    h: u32,
    owner: impl Fn(TileRef) -> u16,
    tr: &mut TraversalScratch,
    me: u16,
    start: TileRef,
    flood_gen: u32,
    mut assign: impl FnMut(usize),
) {
    let TraversalScratch { visited, stack, cluster_index_map, .. } = tr;
    visited[start as usize] = flood_gen; // in-range: valid tile ref
    stack.clear();
    stack.push(start);
    let last_row = (h - 1) * w;
    let mut mark = |visited: &mut Vec<u32>, t: TileRef| {
        visited[t as usize] = flood_gen;
        let c = cluster_index_map[t as usize];
        if c > 0 {
            assign((c - 1) as usize);
        }
    };
    while let Some(seed) = stack.pop() {
        let row = seed - seed % w;
        let (mut l, mut r) = (seed, seed);
        while l > row && visited[(l - 1) as usize] != flood_gen && owner(l - 1) == me {
            l -= 1;
            mark(visited, l);
        }
        while r + 1 < row + w && visited[(r + 1) as usize] != flood_gen && owner(r + 1) == me {
            r += 1;
            mark(visited, r);
        }
        // 8-connectivity: the next rows over [l-1, r+1], clamped to the row.
        let (lx, rx) = (if l > row { l - 1 - row } else { 0 }, if r + 1 < row + w { r + 1 - row } else { w - 1 });
        for (has, nrow) in [(row >= w, row.wrapping_sub(w)), (row < last_row, row + w)] {
            if !has {
                continue;
            }
            let mut in_run = false;
            for t in nrow + lx..=nrow + rx {
                if visited[t as usize] != flood_gen && owner(t) == me {
                    if !in_run {
                        mark(visited, t);
                        stack.push(t);
                        in_run = true;
                    }
                } else {
                    in_run = false;
                }
            }
        }
    }
}

/// `bumpGenerations(count)` (`:1118-1123`): `count` consecutive stamps, retried after a wrap.
fn bump_generations(tr: &mut TraversalScratch, count: u32) -> u32 {
    let first = tr.bump_generation();
    let mut last = first;
    for _ in 1..count {
        last = tr.bump_generation();
    }
    if i64::from(last) - i64::from(first) == i64::from(count) - 1 { first } else { bump_generations(tr, count) }
}

/// `isEnclosed(start, minX, minY, maxX, maxY)` (`:939-997`).
fn is_enclosed(map: &GameMap, tr: &mut TraversalScratch, me: u16, start: TileRef, bx: [i32; 4]) -> bool {
    let stamp = tr.bump_generation();
    tr.stack.clear();
    tr.visited[start as usize] = stamp; // in-range: valid tile ref
    tr.stack.push(start);
    let [min_x, min_y, max_x, max_y] = bx.map(i64::from);
    let (fence_min_x, fence_max_x, fence_min_y, fence_max_y) = (min_x - 1, max_x + 1, min_y - 1, max_y + 1);
    let map_width = map.width();
    let mut nbuf = [0u32; 4];
    while let Some(tile) = tr.stack.pop() {
        // TS: :964-965 `x = tile % mapWidth; y = (tile / mapWidth) | 0` (non-negative ints)
        let x = i64::from(tile % map_width);
        let y = i64::from(tile / map_width);
        if x < fence_min_x || x > fence_max_x || y < fence_min_y || y > fence_max_y {
            return false;
        }
        if map.is_on_edge_of_map(tile) {
            return false;
        }
        let n = map.neighbors4(tile, &mut nbuf);
        for &nb in &nbuf[..n] {
            if tr.visited[nb as usize] == stamp {
                continue;
            }
            let owner_id = map.owner_id(nb);
            if owner_id == me {
                tr.visited[nb as usize] = stamp;
                tr.stack.push(nb);
                continue;
            }
            if owner_id == 0 {
                if !map.is_land(nb) || !map.has_fallout(nb) {
                    // Open water or clean, non-irradiated unclaimed land is an escape path.
                    return false;
                }
                // Irradiated unclaimed land (fallout crater) is a boundary wall.
                continue;
            }
            // Enemy tiles are boundary walls.
        }
    }
    true
}

/// `markRejectedComponent(start, floodGen, fence…)` (`:888-928`).
#[allow(clippy::too_many_arguments)]
fn mark_rejected_component(
    map: &GameMap,
    tr: &mut TraversalScratch,
    me: u16,
    start: TileRef,
    flood_gen: u32,
    fence_min_x: i64,
    fence_min_y: i64,
    fence_max_x: i64,
    fence_max_y: i64,
) {
    let map_width = map.width();
    tr.stack.clear();
    tr.visited[start as usize] = flood_gen; // in-range: valid tile ref
    tr.stack.push(start);
    let mut nbuf = [0u32; 4];
    while let Some(cur) = tr.stack.pop() {
        let num_n = map.neighbors4(cur, &mut nbuf);
        for &n in &nbuf[..num_n] {
            if tr.visited[n as usize] == flood_gen || map.owner_id(n) != me {
                continue;
            }
            let x = i64::from(n % map_width);
            let y = i64::from(n / map_width); // TS: `(n / mapWidth) | 0`
            if x >= fence_min_x && x <= fence_max_x && y >= fence_min_y && y <= fence_max_y {
                tr.visited[n as usize] = flood_gen;
                tr.stack.push(n);
            }
        }
    }
}

/// `hasDiagonalPinch(cluster)` (`:416-440`).
fn has_diagonal_pinch(map: &GameMap, me: u16, cluster: &[TileRef]) -> bool {
    let w = map.width();
    let h = map.height();
    for &t in cluster {
        if map.owner_id(t) != me {
            continue;
        }
        // TS: :424 `if (t >= (h - 1) * w) continue;` (h >= 1, so no underflow)
        if t >= (h - 1) * w {
            continue;
        }
        let below = t + w;
        if map.owner_id(below) == me {
            continue;
        }
        let x = map.x(t);
        if (x < w - 1 && map.owner_id(below + 1) == me && map.owner_id(t + 1) != me)
            || (x > 0 && map.owner_id(below - 1) == me && map.owner_id(t - 1) != me)
        {
            return true;
        }
    }
    false
}

/// `getCapturingPlayer(cluster)` (`:999-1042`).
fn get_capturing_player(st: &State, player: SmallId, cluster: &[TileRef]) -> Option<SmallId> {
    // `neighbors = new Map<Player, number>()`: local, insert/update only, iterated in
    // insertion order below and by getMode.
    let mut neighbors: Vec<(SmallId, f64)> = Vec::new();
    let map = &st.map;
    let my_small_id = player.0;
    let mut nbuf = [0u32; 4];
    for &t in cluster {
        let n = map.neighbors4(t, &mut nbuf);
        for &nb in &nbuf[..n] {
            let owner_id = map.owner_id(nb);
            if owner_id == 0 || owner_id == my_small_id {
                continue;
            }
            let owner = SmallId(owner_id); // `mg.playerBySmallID(ownerId) as Player`
            if !st.player(owner).is_friendly(player, false) {
                match neighbors.iter_mut().find(|(p, _)| *p == owner) {
                    Some(e) => e.1 += 1.0, // `(neighbors.get(owner) ?? 0) + 1`
                    None => neighbors.push((owner, 0.0 + 1.0)),
                }
            }
        }
    }

    // TS: :1018-1020
    if neighbors.is_empty() {
        return None;
    }

    // TS: :1023-1034 the largest attack from the neighbours (live arrays, read only)
    let mut largest_neighbor_attack: Option<SmallId> = None;
    let mut largest_troop_count = 0.0f64;
    for &(neighbor, _) in &neighbors {
        // iter-live: read-only scan of `neighbor.outgoingAttacks()`.
        for &a in st.player_data(neighbor).outgoing_attacks.as_slice() {
            let att = st.attack(a);
            if att.target() == player && att.troops() > largest_troop_count {
                largest_troop_count = att.troops();
                largest_neighbor_attack = Some(neighbor);
            }
        }
    }
    if largest_neighbor_attack.is_some() {
        return largest_neighbor_attack;
    }

    // TS: :1041 no ongoing attacks: the enemy with the largest border (`getMode`).
    of_det::util::get_mode(neighbors)
}

/// Which tile list a cluster helper reads (all live in `sc.clusters`).
#[derive(Clone, Copy)]
enum Src {
    /// `clusters[i]`.
    Cluster(usize),
    /// The last `floodFillWithGen` result of `removeCluster` (`tiles`).
    Fill,
    /// `componentTiles` of `evaluatePinchCandidates`.
    Component,
}

#[inline]
fn src_slice(cl: &ClusterScratch, s: Src) -> &[TileRef] {
    match s {
        Src::Cluster(i) => &cl.tiles[cl.range(i)],
        Src::Fill => &cl.fill,
        Src::Component => &cl.component,
    }
}

#[inline]
fn src_get(cl: &ClusterScratch, s: Src, k: usize) -> TileRef {
    src_slice(cl, s)[k] // in-range: callers loop over 0..len
}

impl PlayerExec {
    /// `new PlayerExecution(player)`.
    pub fn new(player: SmallId) -> Self {
        // TS field initialisers (`:30-42`): `lastCalc = 0`, `active = true`,
        // `clusterTerritoryIds = null`, `nextTerritoryId = 1`.
        PlayerExec {
            active: true,
            initialized: false,
            last_calc: 0,
            player,
            cluster_territory_ids: None,
            next_territory_id: 1,
        }
    }

    /// `owner()` (`:1093-1098`).
    pub fn owner(&self) -> SmallId {
        self.player
    }

    #[inline]
    fn me(&self) -> u16 {
        self.player.0
    }

    #[inline]
    fn alive(&self, cx: &Ctx) -> bool {
        cx.st.player(self.player).is_alive()
    }

    /// `removeOnDeath()` (`:1206-1224`).
    fn remove_on_death(&mut self, cx: &mut Ctx) {
        let p = self.player;
        let gold = cx.st.player(p).gold();
        cx.player_mut(p).remove_gold(gold);
        // TS: `this.player.units().forEach(..)`: `forEach` captures the length of the array
        // object it was called on; `u.delete()` reassigns the field to a new array, so the
        // walk sees exactly the elements present at the start (a copy is the same sequence).
        // iter-mut: forEach over the start-of-loop array object.
        let units: Vec<UnitRef> = cx.st.player_data(p).units.as_slice().to_vec();
        for u in units {
            let t = cx.st.unit(u).unit_type();
            if t != UnitType::AtomBomb
                && t != UnitType::HydrogenBomb
                && t != UnitType::MirvWarhead
                && t != UnitType::Mirv
            {
                cx.unit_mut(u).delete(None, None);
            }
        }
        cx.player_mut(p).remove_all_alliances();
    }

    // ---------------------------------------------------------------- removeClusters

    /// The deferred `finally` of a pinch pass that a TS throw unwound (scratch module doc).
    fn cleanup_stale_index_map(cx: &mut Ctx) {
        let (tr, cl) = (&mut cx.sc.traversal, &mut cx.sc.clusters);
        if cl.index_map_dirty {
            for &t in &cl.tiles {
                tr.cluster_index_map[t as usize] = 0; // in-range: tiles are valid refs
            }
            cl.index_map_dirty = false;
        }
    }

    /// `removeClusters()` (`:139-226`).
    fn remove_clusters(&mut self, cx: &mut Ctx) {
        let total_tiles = (cx.st.map.width() as usize) * (cx.st.map.height() as usize);
        cx.sc.traversal.ensure(total_tiles); // `tileTraversalScratch(this.mg)`
        Self::cleanup_stale_index_map(cx);

        #[cfg(feature = "prof")]
        let pf0 = crate::prof::now();
        // TS: :142 `const { clusters, boxes } = this.calculateClusters()`
        self.calculate_clusters(cx);
        let n = cx.sc.clusters.len();
        #[cfg(feature = "prof")]
        let pf1 = {
            use crate::prof::*;
            let x = now();
            nested(SEC_RC_CALC, x - pf0);
            nested(N_RC_CALLS, 1);
            nested(N_RC_CLUSTER_TILES, cx.sc.clusters.tiles.len() as u64);
            if cx.st.player_data(self.player).conn4 {
                nested(N_RC_CONN4, 1);
            }
            if cx.st.player(self.player).num_tiles_owned() < 100 {
                nested(N_RC_SMALL, 1);
            }
            x
        };

        if n == 0 {
            // TS: :144-147
            cx.player_mut(self.player).set_largest_cluster_bounding_box(None);
            return;
        }

        // TS: :149-152 `isPinched` is computed on first use (`ensure_pinched`, t409 H5).
        #[cfg(feature = "verify-memos")]
        {
            let me = self.me();
            let map = &cx.st.map;
            let cl = &mut cx.sc.clusters;
            cl.pinched_ts = (0..n).map(|i| has_diagonal_pinch(map, me, &cl.tiles[cl.range(i)])).collect();
        }

        #[cfg(feature = "prof")]
        let pf2 = {
            let x = crate::prof::now();
            crate::prof::nested(crate::prof::SEC_RC_PINCH, x - pf1);
            x
        };
        // TS: :164-170
        let (max_territory_area, by_conn4) = self.flood_territory_areas(cx);
        #[cfg(feature = "prof")]
        let pf3 = {
            let x = crate::prof::now();
            crate::prof::nested(crate::prof::SEC_RC_FLOOD, x - pf2);
            x
        };
        let largest_index = Self::find_largest_cluster_index(&cx.sc.clusters);
        // TS: :172 `if (largestCluster === undefined) throw new Error("No clusters")` —
        // unreachable: n > 0 and largest_index < n.
        if largest_index >= n {
            of_det::ts_throw!("No clusters");
        }

        let l = largest_index * 4;
        let lbox: [i32; 4] = {
            let b = &cx.sc.clusters.boxes;
            [b[l], b[l + 1], b[l + 2], b[l + 3]] // in-range: 4 entries per cluster
        };
        cx.player_mut(self.player).set_largest_cluster_bounding_box(Some(BoundingBox {
            min: Cell::new(f64::from(lbox[0]), f64::from(lbox[1])),
            max: Cell::new(f64::from(lbox[2]), f64::from(lbox[3])),
        }));

        // TS: :184
        cx.sc.clusters.removed.clear();
        cx.sc.clusters.removed.resize(n, false);

        // TS: :186-205 Pass 0: sovereign main body removal if fully enclosed by a single enemy
        let surrounded_by = self.surrounded_by_same_player(cx, Src::Cluster(largest_index), lbox);
        if let Some(s) = surrounded_by
            && !cx.st.player(s).is_friendly(self.player, false)
        {
            self.ensure_pinched(cx);
            let pinched = cx.sc.clusters.pinched[largest_index];
            let removed = self.remove_cluster(cx, largest_index, Some(lbox), Some(s), Some(pinched));
            if removed {
                cx.sc.clusters.removed[largest_index] = true;
            }
        }

        // TS: :208-214 Pass 1: whole-cluster removal for secondary clusters
        self.execute_secondary_pass(cx, largest_index);

        // TS: :217-225 Pass 2: pinch pass on unremoved pinched clusters
        if by_conn4 {
            // t409 H5: the areas came from conn4 (one component), so every owned cluster tile has
            // territory id 1 with area `maxTerritoryArea` and every other one is unowned (passes 0-1
            // only take tiles away): `evaluatePinchCandidates` skips every tile, and what
            // `isHoleCluster` writes (`clusterTerritoryIds`, `clusterIndexMap`, stamps) is reset by
            // the pass's `finally` or is scratch. The pass changes no state; its end state is:
            #[cfg(not(feature = "verify-memos"))]
            {
                self.cluster_territory_ids = None;
                self.next_territory_id = 1;
            }
            #[cfg(feature = "verify-memos")]
            {
                let v = cx.st.territory_version;
                self.ensure_pinched(cx);
                self.execute_pinch_pass(cx, max_territory_area);
                assert_eq!(v, cx.st.territory_version, "verify-memos: the conn4-skipped pinch pass changed ownership");
            }
        } else {
            self.ensure_pinched(cx);
            self.execute_pinch_pass(cx, max_territory_area);
        }
        #[cfg(feature = "prof")]
        crate::prof::nested(crate::prof::SEC_RC_PASSES, crate::prof::now() - pf3);
    }

    /// TS's `isPinched` (`:149-152`), filled on first use (t409 H5). TS computes it right after
    /// `calculateClusters`; `hasDiagonalPinch` reads only tile owners and the cluster lists, and
    /// nothing between `calculateClusters` and the first `removeCluster` (pass 0 or 1) changes an
    /// owner (`surroundedBySamePlayer` and `classifyCluster` only read). Every reader calls this
    /// before that first `removeCluster` or after it ran, so the values equal TS's. A call whose
    /// clusters are never removed or pinch-checked (the conn4 path) never computes them.
    fn ensure_pinched(&self, cx: &mut Ctx) {
        let me = self.me();
        let map = &cx.st.map;
        let cl = &mut cx.sc.clusters;
        let n = cl.len();
        if cl.pinched.len() == n {
            return;
        }
        cl.pinched.clear();
        for i in 0..n {
            let p = has_diagonal_pinch(map, me, &cl.tiles[cl.range(i)]);
            cl.pinched.push(p);
        }
        #[cfg(feature = "verify-memos")]
        assert_eq!(cl.pinched, cl.pinched_ts, "verify-memos: lazy isPinched differs from TS's eager one");
    }

    /// `executeSecondaryPass(..)` (`:228-262`).
    fn execute_secondary_pass(&mut self, cx: &mut Ctx, largest_index: usize) {
        let n = cx.sc.clusters.len();
        for i in 0..n {
            if !self.alive(cx) {
                break;
            }
            if i == largest_index {
                continue;
            }
            let bx = Self::cluster_box(&cx.sc.clusters, i);
            if self.classify_cluster(cx, i, bx) & CLUSTER_SURROUNDED != 0 {
                self.ensure_pinched(cx);
                let pinched = cx.sc.clusters.pinched[i];
                if self.remove_cluster(cx, i, Some(bx), None, Some(pinched)) {
                    cx.sc.clusters.removed[i] = true;
                }
            }
        }
    }

    /// `executePinchPass(..)` (`:264-308`), including its `finally`.
    fn execute_pinch_pass(&mut self, cx: &mut Ctx, max_territory_area: i32) {
        self.cluster_territory_ids = None;
        self.next_territory_id = 1;
        // try {
        let n = cx.sc.clusters.len();
        for i in 0..n {
            if !self.alive(cx) {
                break;
            }
            if cx.sc.clusters.removed[i] || !cx.sc.clusters.pinched[i] {
                continue;
            }
            // Guard interior hole clusters so hole crenellations are not falsely annexed.
            if self.is_hole_cluster(cx, i) {
                continue;
            }
            self.evaluate_pinch_candidates(cx, i, max_territory_area);
        }
        // } finally { (a throw skips this; `cleanup_stale_index_map` runs it later)
        if self.cluster_territory_ids.is_some() {
            let (tr, cl) = (&mut cx.sc.traversal, &mut cx.sc.clusters);
            for &t in &cl.tiles {
                tr.cluster_index_map[t as usize] = 0; // in-range: tiles are valid refs
            }
            cl.index_map_dirty = false;
            self.cluster_territory_ids = None;
        }
        self.next_territory_id = 1;
        // }
    }

    /// `floodComponent(start, tGen, areaGen)` (`:310-335`): see [`flood_component_scan`].
    /// (t409 H5) also writes `label` into `labels` for every tile of the component.
    #[allow(clippy::too_many_arguments)]
    fn flood_component(
        map: &GameMap,
        tr: &mut TraversalScratch,
        me: u16,
        start: TileRef,
        t_gen: u32,
        area_gen: u32,
        labels: &mut [u32],
        label: u32,
    ) -> i32 {
        let (w, h) = (map.width(), map.height());
        if labels.is_empty() {
            // a clone: no labels kept (`map::conn4::CompLabels`)
            return flood_component_scan(w, h, |t| map.owner_id(t), tr, me, start, t_gen, area_gen, |_| {});
        }
        let write = |t: TileRef| labels[t as usize] = label; // in-range: labels cover the map (ensured by the caller)
        flood_component_scan(w, h, |t| map.owner_id(t), tr, me, start, t_gen, area_gen, write)
    }

    /// `floodTerritoryAreas(clusters)` (`:337-380`): fills `sizes`, `tids`, `territory_areas`
    /// in `sc.clusters` and returns `maxTerritoryArea` (and whether conn4 supplied it).
    ///
    /// t409 H5: while `Player::conn4` holds (the territory is one 4-connected component, kept by
    /// `map::conn4`), the result is derived without flooding ([`Self::territory_areas_conn4`]);
    /// otherwise TS's flood runs and re-establishes the flag when it finds a single component
    /// covering every owned tile.
    fn flood_territory_areas(&mut self, cx: &mut Ctx) -> (i32, bool) {
        if !cx.st.player_data(self.player).conn4 && cx.st.player_data(self.player).comps.valid {
            #[cfg(feature = "prof")]
            crate::prof::nested(crate::prof::N_RC_COMPS, 1);
            #[cfg(not(feature = "verify-memos"))]
            return self.territory_areas_comps(cx);
            #[cfg(feature = "verify-memos")]
            {
                let (fast, single) = self.territory_areas_comps(cx);
                let cl = &cx.sc.clusters;
                let got = (cl.sizes.clone(), cl.tids.clone(), cl.territory_areas.clone());
                let slow = self.flood_territory_areas_ts(cx);
                let cl = &cx.sc.clusters;
                assert_eq!(
                    (fast, got),
                    (slow, (cl.sizes.clone(), cl.tids.clone(), cl.territory_areas.clone())),
                    "verify-memos: component-label territory areas differ from the flood (player {:?}, tick {})",
                    self.player,
                    cx.st.ticks()
                );
                return (slow, single);
            }
        }
        if cx.st.player_data(self.player).conn4 {
            #[cfg(not(feature = "verify-memos"))]
            return (self.territory_areas_conn4(cx), true);
            #[cfg(feature = "verify-memos")]
            {
                let fast = self.territory_areas_conn4(cx);
                let cl = &cx.sc.clusters;
                let got = (cl.sizes.clone(), cl.tids.clone(), cl.territory_areas.clone());
                let slow = self.flood_territory_areas_ts(cx);
                let cl = &cx.sc.clusters;
                assert_eq!(
                    (fast, got),
                    (slow, (cl.sizes.clone(), cl.tids.clone(), cl.territory_areas.clone())),
                    "verify-memos: conn4 territory areas differ from the flood (player {:?}, tick {})",
                    self.player,
                    cx.st.ticks()
                );
                return (slow, true);
            }
        }
        (self.flood_territory_areas_ts(cx), false)
    }

    /// [`Self::flood_territory_areas`] under `conn4`. TS's flood from the first owned cluster tile
    /// (territory id 1) then reaches every owned tile (all stamps are `< areaGen` before the pass),
    /// so its size is `numTilesOwned`, every later owned cluster tile reads id 1, an unowned one
    /// keeps 0, and no other component is flooded. The flood's `visited` stamps are not written:
    /// every later reader bumps a newer generation first, so stamps older than it are all alike.
    fn territory_areas_conn4(&mut self, cx: &mut Ctx) -> i32 {
        let me = self.me();
        let n_tiles = cx.st.player(self.player).num_tiles_owned() as i32; // int: tile count < 2^31
        let map = &cx.st.map;
        let (tr, cl) = (&mut cx.sc.traversal, &mut cx.sc.clusters);
        let n = cl.len();
        // the same generation stamps TS takes (keeps the scratch's generation sequence identical)
        let _area_gen = bump_generations(tr, (n as u32) * 4 + 16);
        cl.sizes.clear();
        cl.sizes.resize(n, 0);
        cl.tids.clear();
        cl.tids.resize(cl.tiles.len(), 0);
        cl.territory_areas.clear();
        cl.territory_areas.push(0);
        let mut any = false;
        for i in 0..n {
            let mut owned = false;
            for k in cl.range(i) {
                if map.owner_id(cl.tiles[k]) == me {
                    cl.tids[k] = 1; // in-range: k in the cluster's range
                    owned = true;
                }
            }
            if owned {
                cl.sizes[i] = n_tiles;
                any = true;
            }
        }
        if any {
            cl.territory_areas.push(n_tiles);
            n_tiles
        } else {
            0
        }
    }

    /// TS's `floodTerritoryAreas` flood (see [`Self::flood_territory_areas`]).
    fn flood_territory_areas_ts(&mut self, cx: &mut Ctx) -> i32 {
        let me = self.me();
        let total_tiles = (cx.st.map.width() as usize) * (cx.st.map.height() as usize);
        let with_labels = cx.st.comp_labels.enabled;
        if with_labels {
            cx.st.comp_labels.ensure(total_tiles);
        }
        let map = &cx.st.map;
        let labels = &mut cx.st.comp_labels.v;
        let (tr, cl) = (&mut cx.sc.traversal, &mut cx.sc.clusters);
        let n = cl.len();
        cl.sizes.clear();
        cl.sizes.resize(n, 0);
        cl.tids.clear();
        cl.tids.resize(cl.tiles.len(), 0);
        cl.territory_areas.clear();
        cl.territory_areas.push(0); // `territoryAreas = [0]`
        let mut max_territory_area = 0i32;
        // TS: :350 (clusters.length * 4 + 16 consecutive stamps)
        let area_gen = bump_generations(tr, (n as u32) * 4 + 16);
        let mut next_tid: u32 = 1;

        for i in 0..n {
            let mut max_cluster = 0i32;
            for k in cl.range(i) {
                let t = cl.tiles[k]; // in-range: k in the cluster's range
                let v = tr.visited[t as usize];
                if v >= area_gen {
                    let tid = v - area_gen + 1;
                    cl.tids[k] = tid as i32; // int: tid <= next_tid, far below 2^31
                    // `territoryAreas[tId] > maxCluster` (an unset entry is undefined: false)
                    if let Some(&area) = cl.territory_areas.get(tid as usize)
                        && area > max_cluster
                    {
                        max_cluster = area;
                    }
                    continue;
                }
                if map.owner_id(t) != me {
                    continue;
                }
                let tid = next_tid;
                next_tid += 1;
                cl.tids[k] = tid as i32;
                let t_gen = area_gen + tid - 1;
                // TS: :371
                while tr.generation < t_gen && tr.generation >= area_gen {
                    tr.bump_generation();
                }
                let size = Self::flood_component(map, tr, me, t, t_gen, area_gen, labels, tid - 1);
                // `territoryAreas[tId] = size` (tId == length: an append)
                let ti = tid as usize;
                if cl.territory_areas.len() <= ti {
                    cl.territory_areas.resize(ti + 1, 0);
                }
                cl.territory_areas[ti] = size;
                if size > max_cluster {
                    max_cluster = size;
                }
            }
            cl.sizes[i] = max_cluster;
            if max_cluster > max_territory_area {
                max_territory_area = max_cluster;
            }
        }
        #[cfg(feature = "prof")]
        crate::prof::nested(crate::prof::N_RC_FLOOD_TILES, cl.territory_areas.iter().map(|&a| a as u64).sum());
        // t409 H5: one component (id 1) holding every owned tile => the territory is connected.
        let n_tiles = cx.st.player(self.player).num_tiles_owned();
        let single = next_tid == 2 && cl.territory_areas.get(1).is_some_and(|&a| a as u32 == n_tiles);
        #[cfg(feature = "prof")]
        if single {
            crate::prof::nested(crate::prof::N_RC_FLOOD_SINGLE, 1);
            crate::prof::nested(crate::prof::N_RC_FLOOD_SINGLE_TILES, u64::from(n_tiles));
        }
        // t409 H5: every owned tile was flooded (the areas add up to numTilesOwned) => the labels
        // written above (territory id - 1) name every owned tile's component.
        let total: i64 = cl.territory_areas.iter().map(|&a| i64::from(a)).sum();
        let d = cx.st.player_data_mut(self.player);
        d.conn4 = single;
        d.comps.valid = with_labels && total == i64::from(n_tiles);
        d.comps.sizes.clear();
        if d.comps.valid {
            d.comps.sizes.extend_from_slice(&cl.territory_areas[1..]);
        }
        max_territory_area
    }

    /// [`Self::flood_territory_areas`] from the component labels (`Player::comps` valid). TS's
    /// flood from an owned cluster tile not yet stamped floods exactly that tile's component, so
    /// territory ids go to components in first-seen order and each area is the component's size;
    /// a tile of an already flooded component reads that component's id; unowned tiles keep 0.
    /// Returns `maxTerritoryArea` and whether one component is left (then the result is the conn4
    /// one and conn4 is set).
    fn territory_areas_comps(&mut self, cx: &mut Ctx) -> (i32, bool) {
        let me = self.me();
        let st = &mut *cx.st;
        let map = &st.map;
        let labels = &st.comp_labels.v;
        let comps = &st.players[self.player.player_index()].comps; // in-range: a live player
        let (tr, cl) = (&mut cx.sc.traversal, &mut cx.sc.clusters);
        let n = cl.len();
        // the same generation stamps TS takes (keeps the scratch's generation sequence identical)
        let _area_gen = bump_generations(tr, (n as u32) * 4 + 16);
        cl.sizes.clear();
        cl.sizes.resize(n, 0);
        cl.tids.clear();
        cl.tids.resize(cl.tiles.len(), 0);
        cl.territory_areas.clear();
        cl.territory_areas.push(0);
        cl.tid_of_label.clear();
        cl.tid_of_label.resize(comps.sizes.len(), 0);
        let mut max_territory_area = 0i32;
        for i in 0..n {
            let mut max_cluster = 0i32;
            for k in cl.range(i) {
                let t = cl.tiles[k]; // in-range: k in the cluster's range
                if map.owner_id(t) != me {
                    continue;
                }
                let c = labels[t as usize] as usize; // in-range: labels cover the map while valid
                let mut tid = cl.tid_of_label[c]; // in-range: owned tiles' labels index sizes
                if tid == 0 {
                    tid = cl.territory_areas.len() as u32; // int: <= number of components
                    cl.tid_of_label[c] = tid;
                    cl.territory_areas.push(comps.sizes[c]);
                }
                cl.tids[k] = tid as i32; // int: small
                let area = cl.territory_areas[tid as usize]; // in-range: pushed above
                if area > max_cluster {
                    max_cluster = area;
                }
            }
            cl.sizes[i] = max_cluster;
            if max_cluster > max_territory_area {
                max_territory_area = max_cluster;
            }
        }
        // one live component left => connected (labels are exact while valid)
        let single = comps.sizes.iter().filter(|&&s| s > 0).count() <= 1;
        if single {
            st.players[self.player.player_index()].conn4 = true; // in-range: a live player
        }
        (max_territory_area, single)
    }

    /// `[minX, minY, maxX, maxY]` of cluster `i`.
    #[inline]
    fn cluster_box(cl: &ClusterScratch, i: usize) -> [i32; 4] {
        let b = &cl.boxes;
        [b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]] // in-range: 4 entries per cluster
    }

    /// `findLargestClusterIndex(clusters, boxes, clusterSizes)` (`:382-408`).
    #[allow(clippy::if_same_then_else)] // branch structure mirrors PlayerExecution.ts:388-405
    fn find_largest_cluster_index(cl: &ClusterScratch) -> usize {
        let area = |i: usize| {
            let [a, b, c, d] = Self::cluster_box(cl, i).map(i64::from);
            (c - a + 1) * (d - b + 1)
        };
        let mut largest_index = 0usize;
        for i in 1..cl.len() {
            if cl.sizes[i] > cl.sizes[largest_index] {
                largest_index = i;
            } else if cl.sizes[i] == cl.sizes[largest_index] {
                let cur_box_area = area(i);
                let best_box_area = area(largest_index);
                if cur_box_area > best_box_area {
                    largest_index = i;
                } else if cur_box_area == best_box_area && cl.range(i).len() > cl.range(largest_index).len() {
                    largest_index = i;
                }
            }
        }
        largest_index
    }

    /// `evaluatePinchCandidates(cluster, maxTerritoryArea, tileTIds, territoryAreas)`
    /// (`:442-526`). Both optional arguments are always passed (`:288-293`).
    fn evaluate_pinch_candidates(&mut self, cx: &mut Ctx, ci: usize, max_territory_area: i32) {
        if !self.alive(cx) {
            return;
        }
        let me = self.me();
        let mut pinch_gen = cx.sc.traversal.bump_generation();
        let range = cx.sc.clusters.range(ci);
        for j in 0..range.len() {
            if !self.alive(cx) {
                break;
            }
            let k = range.start + j;
            {
                let cl = &cx.sc.clusters;
                let tid = cl.tids[k]; // in-range: k in the cluster's range
                // `tId !== 0 && territoryAreas[tId] >= maxTerritoryArea` (undefined >= x: false)
                if tid != 0 && cl.territory_areas.get(tid as usize).is_some_and(|&a| a >= max_territory_area) {
                    continue;
                }
            }
            let start = cx.sc.clusters.tiles[k];
            if cx.sc.traversal.generation < pinch_gen {
                pinch_gen = cx.sc.traversal.bump_generation();
            }
            if cx.st.map.owner_id(start) != me || cx.sc.traversal.visited[start as usize] >= pinch_gen {
                continue;
            }
            // TS: :470-500 4-way flood to measure and bound this component
            let (cmin_x, cmin_y, cmax_x, cmax_y, component_has_fallout) = {
                let map = &cx.st.map;
                let (tr, cl) = (&mut cx.sc.traversal, &mut cx.sc.clusters);
                let mut has_fallout = false;
                tr.visited[start as usize] = pinch_gen;
                tr.stack.clear();
                tr.stack.push(start);
                cl.component.clear();
                cl.component.push(start);
                let (mut a, mut b) = (map.x(start), map.y(start));
                let (mut c, mut d) = (a, b);
                let mut nbuf = [0u32; 4];
                while let Some(cur) = tr.stack.pop() {
                    let (x, y) = (map.x(cur), map.y(cur));
                    if x < a {
                        a = x;
                    }
                    if y < b {
                        b = y;
                    }
                    if x > c {
                        c = x;
                    }
                    if y > d {
                        d = y;
                    }
                    let num_neighbors = map.neighbors4(cur, &mut nbuf);
                    for &nb in &nbuf[..num_neighbors] {
                        if !has_fallout && map.has_fallout(nb) {
                            has_fallout = true;
                        }
                        if tr.visited[nb as usize] < pinch_gen && map.owner_id(nb) == me {
                            tr.visited[nb as usize] = pinch_gen;
                            cl.component.push(nb);
                            tr.stack.push(nb);
                        }
                    }
                }
                (a, b, c, d, has_fallout)
            };
            // TS: :502-504 the component cannot be the main body
            let comp_len = cx.sc.clusters.component.len();
            if comp_len as i64 >= i64::from(max_territory_area) {
                continue;
            }
            // TS: :509-511 nuclear severance requirement
            if !component_has_fallout {
                continue;
            }
            // TS: :514-524
            let bx = [cmin_x as i32, cmin_y as i32, cmax_x as i32, cmax_y as i32]; // int: map coordinates < 2^16
            if is_enclosed(&cx.st.map, &mut cx.sc.traversal, me, start, bx) {
                let capturing = get_capturing_player(cx.st, self.player, &cx.sc.clusters.component);
                if let Some(capturing) = capturing {
                    if cx.st.player(self.player).num_tiles_owned() as usize == comp_len {
                        cx.conquer_player(capturing, self.player);
                    }
                    for idx in 0..comp_len {
                        let t = src_get(&cx.sc.clusters, Src::Component, idx);
                        cx.player_mut(capturing).conquer(t);
                    }
                }
            }
        }
    }

    /// `initClusterTerritories(clusters)` (`:528-538`).
    fn init_cluster_territories(&mut self, cx: &mut Ctx) {
        let (tr, cl) = (&mut cx.sc.traversal, &mut cx.sc.clusters);
        let n = cl.len();
        self.cluster_territory_ids = Some(vec![-1; n]);
        for c in 0..n {
            for k in cl.range(c) {
                tr.cluster_index_map[cl.tiles[k] as usize] = (c + 1) as i32; // in-range: valid refs
            }
        }
        cl.index_map_dirty = true;
    }

    /// `isHoleCluster(i, clusters, boxes)` (`:540-571`).
    fn is_hole_cluster(&mut self, cx: &mut Ctx, i: usize) -> bool {
        let n = cx.sc.clusters.len();
        let [i_min_x, i_min_y, i_max_x, i_max_y] = Self::cluster_box(&cx.sc.clusters, i);
        for j in 0..n {
            if j == i {
                continue;
            }
            let [a, b, c, d] = Self::cluster_box(&cx.sc.clusters, j);
            if a <= i_min_x && b <= i_min_y && c >= i_max_x && d >= i_max_y {
                if self.cluster_territory_ids.is_none() {
                    self.init_cluster_territories(cx);
                }
                self.check_and_assign_territory(cx, i, j);
                if let Some(t_ids) = &self.cluster_territory_ids
                    && t_ids[i] != -1
                    && t_ids[i] == t_ids[j]
                {
                    return true;
                }
            }
        }
        false
    }

    /// `checkAndAssignTerritory(clusters, cIdxA, cIdxB)` (`:573-630`).
    fn check_and_assign_territory(&mut self, cx: &mut Ctx, c_idx_a: usize, c_idx_b: usize) {
        let Some(ids) = self.cluster_territory_ids.as_mut() else {
            return;
        };
        if ids[c_idx_a] != -1 && ids[c_idx_a] == ids[c_idx_b] {
            return;
        }
        if ids[c_idx_a] != -1 && ids[c_idx_b] != -1 {
            return;
        }
        let start_idx = if ids[c_idx_b] == -1 { c_idx_b } else { c_idx_a };
        let territory_id = self.next_territory_id;
        self.next_territory_id += 1;

        let map = &cx.st.map;
        let my_owner = self.player.0;
        let (tr, cl) = (&mut cx.sc.traversal, &cx.sc.clusters);
        let flood_gen = tr.bump_generation();
        tr.stack.clear();

        // TS: :604 `clusters[startIdx]?.[0]`
        let start = cl.tiles[cl.range(start_idx)].first().copied();
        let Some(start) = start else { return };
        if map.owner_id(start) != my_owner {
            return;
        }
        ids[start_idx] = territory_id;
        flood8_assign(
            map.width(),
            map.height(),
            |t| map.owner_id(t),
            tr,
            my_owner,
            start,
            flood_gen,
            |c| {
                ids[c] = territory_id; // in-range: set by init
            },
        );
    }

    /// `surroundedBySamePlayer(cluster, box)` (`:633-690`).
    fn surrounded_by_same_player(&self, cx: &Ctx, src: Src, bx: [i32; 4]) -> Option<SmallId> {
        let mut single_enemy_small_id: u16 = 0;
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (BIG, BIG, -BIG, -BIG);
        let map = &cx.st.map;
        let me = self.me();
        let mut nbuf = [0u32; 4];
        for &tile in src_slice(&cx.sc.clusters, src) {
            if map.is_ocean_shore(tile) || map.is_on_edge_of_map(tile) {
                return None;
            }
            let n = map.neighbors4(tile, &mut nbuf);
            for &nb in &nbuf[..n] {
                let owner_id = map.owner_id(nb);
                if owner_id == me {
                    continue;
                }
                if owner_id == 0 {
                    // Unowned neighbour / wilderness / water / fallout: escape route
                    return None;
                }
                if single_enemy_small_id == 0 {
                    single_enemy_small_id = owner_id;
                } else if single_enemy_small_id != owner_id {
                    // Multi-enemy border: immediate exit
                    return None;
                }
                let px = map.x(nb) as i32; // int: map coordinates < 2^16
                let py = map.y(nb) as i32;
                if px < min_x {
                    min_x = px;
                }
                if py < min_y {
                    min_y = py;
                }
                if px > max_x {
                    max_x = px;
                }
                if py > max_y {
                    max_y = py;
                }
            }
        }
        if single_enemy_small_id != 0 && min_x <= bx[0] && min_y <= bx[1] && max_x >= bx[2] && max_y >= bx[3] {
            return Some(SmallId(single_enemy_small_id)); // `mg.playerBySmallID(id) as Player`
        }
        None
    }

    /// `classifyCluster(cluster, box)` (`:697-754`).
    fn classify_cluster(&self, cx: &Ctx, ci: usize, bx: [i32; 4]) -> u32 {
        let mut has_enemy = false;
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (BIG, BIG, -BIG, -BIG);
        let map = &cx.st.map;
        let me = self.me();
        let mut nbuf = [0u32; 4];
        let mut grow = |n: TileRef| {
            let x = map.x(n) as i32; // int: map coordinates < 2^16
            let y = map.y(n) as i32;
            if x < min_x {
                min_x = x;
            }
            if y < min_y {
                min_y = y;
            }
            if x > max_x {
                max_x = x;
            }
            if y > max_y {
                max_y = y;
            }
        };
        for &tr in src_slice(&cx.sc.clusters, Src::Cluster(ci)) {
            if map.is_shore(tr) || map.is_on_edge_of_map(tr) {
                return 0;
            }
            let num_neighbors = map.neighbors4(tr, &mut nbuf);
            for &n in &nbuf[..num_neighbors] {
                let owner_id = map.owner_id(n);
                if owner_id != 0 && owner_id != me {
                    has_enemy = true;
                    grow(n);
                } else if owner_id == 0 {
                    if map.has_fallout(n) {
                        grow(n);
                    } else {
                        return 0;
                    }
                }
            }
        }
        if !has_enemy {
            return 0;
        }
        if min_x <= bx[0] && min_y <= bx[1] && max_x >= bx[2] && max_y >= bx[3] {
            return CLUSTER_SURROUNDED;
        }
        0
    }

    /// `removeCluster(cluster, minX?, minY?, maxX?, maxY?, knownCaptor?, isPinched?)` (`:756-886`).
    fn remove_cluster(
        &mut self,
        cx: &mut Ctx,
        ci: usize,
        bounds: Option<[i32; 4]>,
        known_captor: Option<SmallId>,
        is_pinched: Option<bool>,
    ) -> bool {
        let me = self.me();
        let src = Src::Cluster(ci);
        // TS: :765-771 other removeCluster operations could change tile owners: double check
        for &t in src_slice(&cx.sc.clusters, src) {
            if cx.st.map.owner_id(t) != me {
                return false;
            }
        }
        // TS: :773-776
        let Some(&first_tile) = src_slice(&cx.sc.clusters, src).first() else {
            return false;
        };

        // TS: :778-798 (every caller passes the bounds; the fallback is ported for fidelity)
        let [min_x, min_y, max_x, max_y] = match bounds {
            Some(b) => b,
            None => {
                let map = &cx.st.map;
                let cl = src_slice(&cx.sc.clusters, src);
                let (mut a, mut b) = (map.x(first_tile) as i32, map.y(first_tile) as i32);
                let (mut c, mut d) = (a, b);
                for &t in &cl[1..] {
                    let (x, y) = (map.x(t) as i32, map.y(t) as i32); // int: map coordinates < 2^16
                    if x < a {
                        a = x;
                    }
                    if y < b {
                        b = y;
                    }
                    if x > c {
                        c = x;
                    }
                    if y > d {
                        d = y;
                    }
                }
                [a, b, c, d]
            }
        };
        let bx = [min_x, min_y, max_x, max_y];

        // TS: :800
        let pinched = match is_pinched {
            Some(p) => p,
            None => has_diagonal_pinch(&cx.st.map, me, src_slice(&cx.sc.clusters, src)),
        };

        // TS: :804-826 fast path: an unpinched cluster is a single 4-way component
        if !pinched {
            let capturing = match known_captor {
                Some(c) => Some(c),
                None => get_capturing_player(cx.st, self.player, src_slice(&cx.sc.clusters, src)),
            };
            let Some(capturing) = capturing else {
                return false;
            };
            if !is_enclosed(&cx.st.map, &mut cx.sc.traversal, me, first_tile, bx) {
                return false;
            }
            let stamp = cx.sc.traversal.bump_generation();
            {
                let map = &cx.st.map;
                let (tr, cl) = (&mut cx.sc.traversal, &mut cx.sc.clusters);
                cl.fill.clear();
                flood_fill_with_gen(map, tr, stamp, first_tile, false, Include::Owner(me), &mut cl.fill);
            }
            let fill_len = cx.sc.clusters.fill.len();
            if cx.st.player(self.player).num_tiles_owned() as usize == fill_len {
                cx.conquer_player(capturing, self.player);
            }
            for idx in 0..fill_len {
                let tile = src_get(&cx.sc.clusters, Src::Fill, idx);
                cx.player_mut(capturing).conquer(tile);
            }
            return true;
        }

        // TS: :829-885 pinched clusters: handle each 4-way component independently
        let flood_gen = cx.sc.traversal.bump_generation();
        let mut any_removed = false;
        let len = src_slice(&cx.sc.clusters, src).len();
        for i in 0..len {
            if !self.alive(cx) {
                break;
            }
            let t = src_get(&cx.sc.clusters, src, i);
            if cx.sc.traversal.visited[t as usize] == flood_gen || cx.st.map.owner_id(t) != me {
                continue;
            }
            // TS: :847-860 verify this 4-way component is sealed in before conquering it
            if !is_enclosed(&cx.st.map, &mut cx.sc.traversal, me, t, bx) {
                mark_rejected_component(
                    &cx.st.map,
                    &mut cx.sc.traversal,
                    me,
                    t,
                    flood_gen,
                    i64::from(min_x) - 1,
                    i64::from(min_y) - 1,
                    i64::from(max_x) + 1,
                    i64::from(max_y) + 1,
                );
                continue;
            }
            {
                let map = &cx.st.map;
                let (tr, cl) = (&mut cx.sc.traversal, &mut cx.sc.clusters);
                cl.fill.clear();
                flood_fill_with_gen(map, tr, flood_gen, t, false, Include::Owner(me), &mut cl.fill);
            }
            let fill_len = cx.sc.clusters.fill.len();
            if fill_len > 0 {
                let component_captor = match known_captor {
                    Some(c) => Some(c),
                    None => get_capturing_player(cx.st, self.player, &cx.sc.clusters.fill),
                };
                let Some(component_captor) = component_captor else {
                    continue;
                };
                any_removed = true;
                if cx.st.player(self.player).num_tiles_owned() as usize == fill_len {
                    cx.conquer_player(component_captor, self.player);
                }
                for idx in 0..fill_len {
                    let tile = src_get(&cx.sc.clusters, Src::Fill, idx);
                    cx.player_mut(component_captor).conquer(tile);
                }
            }
        }
        any_removed
    }

    /// `calculateClusters()` (`:1044-1091`): fills `sc.clusters` (tiles, offsets, boxes).
    fn calculate_clusters(&mut self, cx: &mut Ctx) {
        let st = &*cx.st;
        let map = &st.map;
        let border_tiles = &st.player_data(self.player).border_tiles;
        let (tr, cl) = (&mut cx.sc.traversal, &mut cx.sc.clusters);
        cl.reset();
        if border_tiles.size() == 0 {
            return;
        }
        // TS: :1056-1060 two stamps: every border tile gets `borderGen`, floods `currentGen`
        let border_gen = bump_generations(tr, 2);
        let current_gen = border_gen + 1;
        for tile in border_tiles.iter() {
            tr.visited[tile as usize] = border_gen; // in-range: valid tile ref
        }
        // TS: :1069-1089 `borderTiles.forEach(startTile => ..)`; iter-live: the flood only
        // writes stamps, never the border set.
        let mut c = border_tiles.cursor();
        while let Some(start_tile) = border_tiles.next_live(&mut c) {
            if tr.visited[start_tile as usize] == current_gen {
                continue;
            }
            let bx =
                flood_fill_with_gen(map, tr, current_gen, start_tile, true, Include::Stamp(border_gen), &mut cl.tiles);
            cl.boxes.extend_from_slice(&bx);
            cl.offsets.push(cl.tiles.len());
        }
    }
}

impl ExecBehavior for PlayerExec {
    fn is_active(&self) -> bool {
        self.active
    }
    /// `activeDuringSpawnPhase()` returns `false` (`PlayerExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    /// `init(mg, ticks)` (`:50-56`).
    fn init(&mut self, cx: &mut Ctx, ticks: i32) {
        self.initialized = true;
        // TS: :54-55 `ticks + (simpleHash(player.id()) % ticksPerClusterCalc)`; simpleHash is
        // a non-negative integer (f64), so the remainder is an integer in [0, 20).
        let h = of_det::util::simple_hash(cx.st.player(self.player).player_id());
        self.last_calc = ticks + of_det::js::to_int32(h % f64::from(TICKS_PER_CLUSTER_CALC));
    }
    /// `tick(ticks)` (`:58-137`).
    fn tick(&mut self, cx: &mut Ctx, ticks: i32) {
        let p = self.player;
        cx.player_mut(p).decay_relations();

        // TS: :60-80 `for (const u of this.player.units())`: the live array; `u.delete()` and
        // `captor.captureUnit(u)` (setOwner) filter-reassign it, so the running loop walks the
        // old array object (ARCHITECTURE §4.7). `captureUnit` pushes to the *captor's* list.
        // iter-snapshot: filter-reassign.
        snapshot_loop(
            cx,
            |st| &st.player_data(p).units,
            |cx, u| {
                let ut = cx.st.unit(u).unit_type();
                if !unit_groups::has(unit_groups::STRUCTURES, ut) {
                    return;
                }
                // TS: :65 `this.mg!.owner(u.tile())`
                let owner = cx.st.owner(cx.st.unit(u).tile());
                if !owner.is_player() {
                    cx.unit_mut(u).delete(None, None);
                    return;
                }
                if owner == p {
                    return;
                }
                let captor = owner; // `this.mg!.player(owner.id())`
                if ut == UnitType::DefensePost {
                    cx.unit_mut(u).delete(Some(true), Some(captor));
                } else {
                    cx.player_mut(captor).capture_unit(u);
                }
            },
        );

        // TS: :82-97
        if !cx.st.player(p).is_alive() {
            self.remove_on_death(cx);
            self.active = false;
            let still_standing =
                cx.st.players().into_iter().filter(|&q| cx.st.player(q).player_type() != PlayerType::Bot).count()
                    as i32;
            cx.st.stats_cx().record_death_position(p, still_standing + 1);
            cx.st.stats_cx().player_killed(p, ticks);
            return;
        }

        // TS: :99-105
        let troop_inc = cx.st.config().troop_increase_rate(&cx.st.player(p));
        cx.player_mut(p).add_troops(troop_inc);
        let gold_from_workers = cx.st.config().gold_addition_rate(&cx.st.player(p));
        cx.player_mut(p).add_gold(gold_from_workers, None);
        cx.st.stats_cx().gold_work(p, StatNum::Big(gold_from_workers));

        // TS: :107-111 `for (const alliance of this.player.alliances())`: `expire()` detaches
        // the alliance, filter-reassigning `_alliances` (GameImpl.ts:1143-1144).
        // iter-snapshot: filter-reassign.
        snapshot_loop(
            cx,
            |st| &st.player_data(p).alliances,
            |cx, alliance| {
                if cx.st.alliance(alliance).expires_at <= cx.st.ticks() {
                    cx.alliance_expire(alliance);
                }
            },
        );

        // TS: :113-121 (`getEmbargoes()` is a copy)
        let embargoes = cx.st.player(p).get_embargoes();
        for embargo in embargoes {
            if embargo.is_temporary && cx.st.ticks() - embargo.created_at > cx.st.config().temporary_embargo_duration()
            {
                cx.player_mut(p).stop_embargo(embargo.target);
            }
        }

        // TS: :123-136 (the timing log is dropped)
        // (two nested ifs in TS; collapsed, same evaluation order)
        if (ticks - self.last_calc > TICKS_PER_CLUSTER_CALC || cx.st.player(p).num_tiles_owned() < 100)
            && cx.st.player(p).last_tile_change() >= self.last_calc
        {
            self.last_calc = ticks;
            self.remove_clusters(cx);
        }
    }
}

// CS v1 export (`Player v1 (execution)` record, dump field order).
crate::impl_canon_record!(PlayerExec { "active" => active, "initialized" => initialized, "lastCalc" => last_calc, "player" => player });

#[cfg(test)]
mod tests {

    /// TS's DFS `floodComponent` (`PlayerExecution.ts:310-335`), the reference for the scanline.
    #[allow(clippy::too_many_arguments)]
    fn dfs_flood(w: u32, h: u32, own: &[u16], vis: &mut [u32], me: u16, start: u32, t_gen: u32, area_gen: u32) -> i32 {
        vis[start as usize] = t_gen;
        let mut stack = vec![start];
        let mut size = 0;
        while let Some(cur) = stack.pop() {
            size += 1;
            let x = cur % w;
            let mut nb = Vec::new();
            if cur >= w {
                nb.push(cur - w);
            }
            if cur < (h - 1) * w {
                nb.push(cur + w);
            }
            if x != 0 {
                nb.push(cur - 1);
            }
            if x != w - 1 {
                nb.push(cur + 1);
            }
            for n in nb {
                if vis[n as usize] < area_gen && own[n as usize] == me {
                    vis[n as usize] = t_gen;
                    stack.push(n);
                }
            }
        }
        size
    }

    /// P3 (t066) guard: `flood8_assign` stamps the same tiles and assigns the same clusters as
    /// TS's 8-connected DFS (`checkAndAssignTerritory`), on random territories and index maps.
    #[test]
    fn flood8_scan_equals_dfs() {
        let mut seed: u64 = 0x0bad_cafe_dead_beef;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for case in 0..300 {
            let (w, h) = (1 + (next() % 50) as u32, 1 + (next() % 40) as u32);
            let n = (w * h) as usize;
            let dens = next() % 100;
            let own: Vec<u16> = (0..n).map(|_| if next() % 100 < dens { 3 } else { 1 }).collect();
            let cim: Vec<i32> = (0..n).map(|_| if next() % 5 == 0 { 1 + (next() % 9) as i32 } else { 0 }).collect();
            let vis0: Vec<u32> = (0..n).map(|_| if next() % 7 == 0 { 9 } else { 0 }).collect();
            let start = (next() % n as u64) as u32;
            // reference DFS (TS :609-628), start's own index-map entry not consulted
            let mut va = vis0.clone();
            let mut ida = vec![false; 10];
            va[start as usize] = 9;
            let mut st = vec![start];
            while let Some(t) = st.pop() {
                let (x, y) = ((t % w) as i64, (t / w) as i64);
                for dx in -1i64..=1 {
                    for dy in -1i64..=1 {
                        let (nx, ny) = (x + dx, y + dy);
                        if (dx == 0 && dy == 0) || nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                            continue;
                        }
                        let nb = (ny * w as i64 + nx) as usize;
                        if va[nb] == 9 || own[nb] != 3 {
                            continue;
                        }
                        va[nb] = 9;
                        st.push(nb as u32);
                        if cim[nb] > 0 {
                            ida[(cim[nb] - 1) as usize] = true;
                        }
                    }
                }
            }
            let mut tr = TraversalScratch { visited: vis0, cluster_index_map: cim, ..Default::default() };
            let mut idb = vec![false; 10];
            flood8_assign(w, h, |t| own[t as usize], &mut tr, 3, start, 9, |c| idb[c] = true);
            // Note: the reference treats pre-stamped 9s as visited, exactly like TS's `visited[n] === floodGen`.
            assert_eq!(tr.visited, va, "case {case} stamps");
            assert_eq!(idb, ida, "case {case} clusters");
        }
    }

    /// P3 (t066) guard: the scanline flood stamps exactly the DFS flood's tiles and returns
    /// its count, on random blobby territories with pre-stamped tiles of both kinds.
    #[test]
    fn scanline_flood_equals_dfs_flood() {
        let mut seed: u64 = 0x1234_5678_9abc_def1;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for case in 0..300 {
            let (w, h) = (1 + (next() % 60) as u32, 1 + (next() % 40) as u32);
            let n = (w * h) as usize;
            let dens = next() % 100;
            let own: Vec<u16> = (0..n).map(|_| if next() % 100 < dens { 7 } else { (next() % 3) as u16 }).collect();
            let area_gen = 50;
            let vis0: Vec<u32> = (0..n)
                .map(|_| match next() % 10 {
                    0 => 60,
                    1 => 10,
                    _ => 0,
                })
                .collect();
            let start = (next() % n as u64) as u32;
            let mut a = vis0.clone();
            let want = dfs_flood(w, h, &own, &mut a, 7, start, 55, area_gen);
            let mut tr = TraversalScratch { visited: vis0.clone(), ..Default::default() };
            let mut marked = vec![false; n];
            let got = flood_component_scan(
                w,
                h,
                |t| own[t as usize],
                &mut tr,
                7,
                start,
                55,
                area_gen,
                |t| {
                    assert!(!marked[t as usize], "case {case}: tile {t} marked twice");
                    marked[t as usize] = true;
                },
            );
            // mark() saw exactly the stamped tiles
            let stamped: Vec<bool> = (0..n).map(|t| a[t] == 55 && vis0[t] != 55).collect();
            assert_eq!(marked, stamped, "case {case} marks");
            assert_eq!(got, want, "case {case} size");
            assert_eq!(tr.visited, a, "case {case} stamps");
        }
    }
    use super::*;
    use crate::ids::ReqIdx;

    /// Mandatory design test (FILE_MAP §4 P6) for the `iter-snapshot` mechanism the two
    /// `PlayerExecution` loops use: a body that deletes the *next* elements (filter
    /// reassignment) still visits every element of the start-of-loop array, and a push made
    /// before any reassignment is visited too. Exercised on `State.alliance_requests`, a
    /// `JsArr` with the same semantics as `Player.units`/`Player.alliances`.
    #[test]
    fn snapshot_loop_visits_every_old_element() {
        let mut st = State::default();
        let mut sc = crate::scratch::Scratch::new();
        for i in 0..5 {
            st.alliance_requests.push(ReqIdx(i));
        }
        let mut cx = Ctx::new(&mut st, &mut sc, None);
        let mut seen = vec![];
        snapshot_loop(
            &mut cx,
            |st| &st.alliance_requests,
            |cx, r| {
                seen.push(r.0);
                // "delete every structure": each step removes itself and its successor.
                cx.st.alliance_requests.filter_reassign(|x| x.0 != r.0 && x.0 != r.0 + 1);
            },
        );
        assert_eq!(seen, [0, 1, 2, 3, 4]); // a retain + live cursor would give [0, 2, 4] or less
        assert!(cx.st.alliance_requests.is_empty());

        // Pushes before any reassignment are visited (same JS array object)…
        for i in 0..2 {
            cx.st.alliance_requests.push(ReqIdx(i));
        }
        let mut seen = vec![];
        snapshot_loop(
            &mut cx,
            |st| &st.alliance_requests,
            |cx, r| {
                seen.push(r.0);
                if r.0 == 0 {
                    cx.st.alliance_requests.push(ReqIdx(9));
                }
            },
        );
        assert_eq!(seen, [0, 1, 9]);
        // …but not after one (they go to the new object).
        let mut seen = vec![];
        snapshot_loop(
            &mut cx,
            |st| &st.alliance_requests,
            |cx, r| {
                seen.push(r.0);
                if r.0 == 0 {
                    cx.st.alliance_requests.filter_reassign(|_| true);
                    cx.st.alliance_requests.push(ReqIdx(7));
                }
            },
        );
        assert_eq!(seen, [0, 1, 9]);
        assert_eq!(cx.st.alliance_requests.as_slice(), &[ReqIdx(0), ReqIdx(1), ReqIdx(9), ReqIdx(7)]);
    }

    #[test]
    fn constructor_defaults() {
        let e = PlayerExec::new(SmallId(7));
        assert!(e.active && !e.initialized && e.cluster_territory_ids.is_none());
        assert_eq!((e.last_calc, e.next_territory_id, e.owner()), (0, 1, SmallId(7)));
        assert!(!e.active_during_spawn_phase());
    }

    /// `bumpGenerations(count)` returns the first of `count` consecutive stamps and retries
    /// after TS's own wrap (`:1118-1123`).
    #[test]
    fn bump_generations_consecutive_and_wrap_retry() {
        let mut tr = TraversalScratch::default();
        tr.ensure(4);
        tr.generation = 10;
        assert_eq!(bump_generations(&mut tr, 3), 11);
        assert_eq!(tr.generation, 13);
        // A run that hits 0xffffffff wraps to 1 mid-run: retried from the fresh counter.
        tr.generation = 0xffff_fffc;
        // 0xfffffffd, 0xfffffffe, (0xffffffff → fill, 1), 2: not consecutive → retry 3..=6.
        let g = bump_generations(&mut tr, 4);
        assert_eq!((g, tr.generation), (3, 6));
    }
}
