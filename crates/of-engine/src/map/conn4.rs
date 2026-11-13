//! t409 H5: local rules that keep `Player::conn4` ("this player's territory is one 4-connected
//! component, or empty") true across single-tile ownership changes (ARCHITECTURE §11 item 3, the
//! proof-gated incremental `removeClusters` kernel; ledger t320).
//!
//! The territory `T` of a player is the set of tiles with `ownerID == smallID`. Both rules are
//! *sufficient* conditions checked on the owner map just before one tile changes owner; when a
//! rule fails the flag is cleared and the next `removeClusters` floods as TS does.
//!
//! - **Gain** ([`gain_keeps`]): `T` connected, `t ∉ T`. If `T` is empty, or a 4-neighbour of `t`
//!   is in `T`, then `T ∪ {t}` is connected (every tile reaches that neighbour, which reaches `t`).
//! - **Loss** ([`loss_keeps`]): `T` connected, `t ∈ T`. Let `R` be the 8 tiles around `t` in
//!   cyclic order N, NE, E, SE, S, SW, W, NW; consecutive ring tiles are 4-adjacent. If the
//!   4-neighbours of `t` that are in `T` all lie in ONE run of consecutive `T` tiles of `R`, then
//!   `T \ {t}` is connected: a 4-path through `t` enters and leaves via two such neighbours `u`,
//!   `v`, and the run gives a 4-path `u → v` inside `T \ {t}`. With no 4-neighbour in `T`, `t`
//!   was all of `T` and the result is empty. Off-map ring positions are never in `T`.
//!
//! Applied in sequence, each change preserves the invariant, so `conn4 == true` implies the
//! territory is connected at every point in time. `tests/conn4_fuzz.rs` checks the flag against a
//! flood after every change of 10^4+ random edits on random territories.
//!
//! **Component labels** ([`CompMemo`], [`CompLabels`]) extend this to a territory of several
//! components: while `Player::comps.valid`, every owned tile `t` has `labels[t]` = the index of
//! its 4-connected component and `comps.sizes[label]` = that component's tile count. Kept by:
//! - gain of `t`: no owned 4-neighbour → `t` is a new component (new label, size 1); all owned
//!   4-neighbours share label `c` → `t` joins `c` (it touches no other component); two labels →
//!   the components merge: invalid;
//! - loss of `t` (label `c`): the loss rule above (applied to `c`, the only component containing
//!   `t`'s owned 4-neighbours) keeps `c \ {t}` connected → `sizes[c] -= 1`; else invalid.
//!
//! - loss of `t` when the ring rule fails (R2): [`loss_search`] runs a bounded lock-step search over the territory minus
//!   `t`, from each owned 4-neighbour of `t`. Searches that meet are one component; a search whose frontier runs dry has seen a
//!   whole component of `T \ {t}` (a closed pocket). All meet: the component is still connected (`sizes[c] -= 1`, conn4 stays).
//!   All but at most one class closed: the closed pockets get fresh labels (`sizes[c]` shrinks by `t` and their sizes) and conn4
//!   is cleared; the one open class keeps `c`. Budget exceeded: invalid, as before.
//!
//! Labels are not cloned with the state: a fork gets disabled labels and invalid memos, so it
//! keeps TS's flood (and its pre-H5 clone and memory cost); only a state built by `Game::new` /
//! `create_state` (the RL envs, a search root) maintains labels. conn4 needs no labels and is
//! cloned.
//!
//! Owner: **P6** (serves `PlayerExecution.removeClusters`; t409 H5).

use super::GameMap;
use crate::ids::TileRef;

/// `LOSS_OK[mask]`: the ring rule of the module doc for an 8-bit ring mask (bit 0 = N, then
/// clockwise: NE, E, SE, S, SW, W, NW; the 4-neighbours are the even bits).
const LOSS_OK: [bool; 256] = {
    let mut t = [false; 256];
    let mut m = 0usize;
    while m < 256 {
        t[m] = ring_ok(m as u8);
        m += 1;
    }
    t
};

const fn ring_ok(m: u8) -> bool {
    if m == 0xff {
        return true; // one run around the whole ring
    }
    // Start the walk just after a position not in T, so every run closes inside the walk.
    let mut z = 0u32;
    while (m >> z) & 1 == 1 {
        z += 1;
    }
    let mut runs_with_edge = 0u32;
    let (mut in_run, mut run_has_edge) = (false, false);
    let mut k = 1u32;
    while k <= 8 {
        let i = (z + k) % 8;
        if (m >> i) & 1 == 1 {
            if !in_run {
                in_run = true;
                run_has_edge = false;
            }
            if i.is_multiple_of(2) {
                run_has_edge = true;
            }
        } else if in_run {
            if run_has_edge {
                runs_with_edge += 1;
            }
            in_run = false;
        }
        k += 1;
    }
    runs_with_edge <= 1
}

/// The ring mask of `t` for owner `me` (module doc order; off-map positions are 0).
#[inline]
fn ring_mask(map: &GameMap, t: TileRef, me: u16) -> u8 {
    let (w, h) = (map.width(), map.height());
    let (x, y) = (t % w, t / w);
    let (lt, rt, up, dn) = (x > 0, x + 1 < w, y > 0, y + 1 < h);
    let own = |c: bool, n: TileRef| -> u8 { u8::from(c && map.owner_id(n) == me) };
    // wrapping_*: the tile index is only formed (and read) when its guard holds
    own(up, t.wrapping_sub(w))
        | own(up && rt, t.wrapping_sub(w).wrapping_add(1)) << 1
        | own(rt, t.wrapping_add(1)) << 2
        | own(dn && rt, t.wrapping_add(w).wrapping_add(1)) << 3
        | own(dn, t.wrapping_add(w)) << 4
        | own(dn && lt, t.wrapping_add(w).wrapping_sub(1)) << 5
        | own(lt, t.wrapping_sub(1)) << 6
        | own(up && lt, t.wrapping_sub(w).wrapping_sub(1)) << 7
}

/// Loss rule: `true` when `me` losing `t` (currently `me`'s) keeps a connected territory connected.
#[inline]
pub fn loss_keeps(map: &GameMap, t: TileRef, me: u16) -> bool {
    LOSS_OK[ring_mask(map, t, me) as usize] // in-range: a u8 index into 256 entries
}

/// Gain rule (non-empty territory): `true` when a 4-neighbour of `t` is `me`'s.
#[inline]
pub fn gain_keeps(map: &GameMap, t: TileRef, me: u16) -> bool {
    let mut nbuf = [0u32; 4];
    let n = map.neighbors4(t, &mut nbuf);
    nbuf[..n].iter().any(|&nb| map.owner_id(nb) == me)
}

/// Per-player component memo (non-snapshot; module doc). Not cloned: a clone is invalid.
#[derive(Debug, Default)]
pub struct CompMemo {
    pub valid: bool,
    /// Tile count per component label (labels index this; a label may reach size 0).
    pub sizes: Vec<i32>,
}

impl Clone for CompMemo {
    /// A fork does not carry the labels ([`CompLabels`]), so its memo starts invalid.
    fn clone(&self) -> Self {
        CompMemo::default()
    }
}
impl crate::plain::PlainState for CompMemo {}
crate::refs_leaf!(CompMemo);

/// Per-tile component labels of the tile's current owner (non-snapshot, `State::comp_labels`;
/// read only for a player whose [`CompMemo`] is valid). A fresh state has them enabled (allocated
/// on the first flood); a clone has them disabled and empty (module doc).
#[derive(Debug)]
pub struct CompLabels {
    /// `false` in a clone: no labels are kept, every `CompMemo` stays invalid.
    pub enabled: bool,
    /// Label per tile.
    pub v: Vec<u32>,
    /// Scratch of [`loss_search`] (R2); never state.
    pub search: SplitSearch,
}

impl Default for CompLabels {
    fn default() -> Self {
        CompLabels { enabled: true, v: Vec::new(), search: SplitSearch::default() }
    }
}

impl Clone for CompLabels {
    fn clone(&self) -> Self {
        CompLabels { enabled: false, v: Vec::new(), search: SplitSearch::default() }
    }
}
impl crate::plain::PlainState for CompLabels {}
crate::refs_leaf!(CompLabels);

impl CompLabels {
    /// Make room for `n` tiles (contents of new slots are irrelevant until written). A first
    /// allocation is zeroed lazily by the allocator (calloc), so untouched pages cost nothing.
    pub fn ensure(&mut self, n: usize) {
        if self.v.is_empty() {
            self.v = vec![0; n];
        } else if self.v.len() < n {
            self.v.resize(n, 0);
        }
    }
}

/// Tiles the lock-step search may visit before it gives up (the territory is then invalidated and flooded, as before R2).
pub const SEARCH_BUDGET: usize = 512;

/// Scratch of [`loss_search`]: a per-tile stamp array of its own (the shared traversal scratch cannot be used: `conquer` runs
/// inside passes that hold stamp windows of it), and one visit list per source.
#[derive(Debug, Default)]
pub struct SplitSearch {
    stamp: Vec<u32>,
    gen_no: u32,
    q: [Vec<TileRef>; 4],
}

/// What [`loss_search`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Loss {
    /// `T \ {t}` has the same number of components as `T` around `t`: everything that touched `t` is still connected.
    Connected,
    /// `t` was a cut tile: the closed pockets were relabelled (when sizes were given) and `T \ {t}` has several components.
    Split,
    /// The budget ran out before the question was settled: the caller invalidates.
    Budget,
}

/// `me` is about to lose `t` (still `me`'s in `map`), `T` was connected per component, and the ring rule failed. Decide, with a
/// bounded search, whether the pieces around `t` stay connected. With `comps = Some((c, sizes))` (the label `c` of `t`, the
/// component sizes) a split also relabels every closed pocket with a fresh label and updates `sizes`; `labels` are the per-tile
/// labels. `Connected`/`Split` are proofs: a path found between two sources is a path; a frontier that ran dry has enumerated a
/// whole 4-connected component of `T \ {t}`; the one class left open is, by elimination, the rest of the component.
#[inline(never)]
pub fn loss_search(
    map: &GameMap,
    me: u16,
    t: TileRef,
    labels: &mut [u32],
    ss: &mut SplitSearch,
    comps: Option<(u32, &mut Vec<i32>)>,
) -> Loss {
    let out = loss_search_inner(map, me, t, labels, ss, comps);
    #[cfg(feature = "verify-opt")]
    verify_loss(map, me, t, out);
    out
}

/// `verify-opt`: an unbounded flood of `T \ {t}` from the first owned 4-neighbour of `t` must agree with the search's claim.
#[cfg(feature = "verify-opt")]
fn verify_loss(map: &GameMap, me: u16, t: TileRef, out: Loss) {
    let mut nbuf = [0u32; 4];
    let n = map.neighbors4(t, &mut nbuf);
    let srcs: Vec<TileRef> = nbuf[..n].iter().copied().filter(|&u| map.owner_id(u) == me).collect();
    if srcs.len() < 2 || out == Loss::Budget {
        return;
    }
    let total = (map.width() as usize) * (map.height() as usize);
    let mut seen = vec![false; total];
    seen[srcs[0] as usize] = true;
    let mut stack = vec![srcs[0]];
    while let Some(x) = stack.pop() {
        let m = map.neighbors4(x, &mut nbuf);
        for &y in &nbuf[..m] {
            if y != t && map.owner_id(y) == me && !seen[y as usize] {
                seen[y as usize] = true;
                stack.push(y);
            }
        }
    }
    let all = srcs.iter().all(|&u| seen[u as usize]);
    assert_eq!(
        out == Loss::Connected,
        all,
        "verify-opt: loss_search said {out:?} for tile {t} of player {me}, flood says connected={all}"
    );
}

fn loss_search_inner(
    map: &GameMap,
    me: u16,
    t: TileRef,
    labels: &mut [u32],
    ss: &mut SplitSearch,
    comps: Option<(u32, &mut Vec<i32>)>,
) -> Loss {
    let mut src = [0u32; 4];
    let mut nbuf = [0u32; 4];
    let n = map.neighbors4(t, &mut nbuf);
    let mut k = 0usize;
    for &u in &nbuf[..n] {
        if map.owner_id(u) == me {
            src[k] = u;
            k += 1;
        }
    }
    if k < 2 {
        return Loss::Connected; // the ring rule cannot fail with fewer than two owned 4-neighbours (module doc)
    }
    let total_tiles = (map.width() as usize) * (map.height() as usize);
    if ss.stamp.len() < total_tiles {
        ss.stamp.resize(total_tiles, 0);
    }
    if ss.gen_no > (1 << 31) {
        ss.stamp.fill(0);
        ss.gen_no = 0;
    }
    let base = ss.gen_no + 1;
    ss.gen_no += k as u32; // int: k <= 4
    let mut head = [0usize; 4];
    #[allow(clippy::needless_range_loop)]
    for i in 0..k {
        ss.q[i].clear();
        ss.q[i].push(src[i]);
        ss.stamp[src[i] as usize] = base + i as u32; // in-range: valid tile ref
    }
    let mut total = k;
    let mut parent = [0usize, 1, 2, 3]; // DSU over the sources
    fn find(p: &mut [usize; 4], mut x: usize) -> usize {
        while p[x] != x {
            p[x] = p[p[x]];
            x = p[x];
        }
        x
    }
    loop {
        let mut progressed = false;
        #[allow(clippy::needless_range_loop)]
        for i in 0..k {
            if head[i] >= ss.q[i].len() {
                continue;
            }
            progressed = true;
            let x = ss.q[i][head[i]];
            head[i] += 1;
            let m = map.neighbors4(x, &mut nbuf);
            for &y in &nbuf[..m] {
                if y == t || map.owner_id(y) != me {
                    continue;
                }
                let s = ss.stamp[y as usize]; // in-range: valid tile ref
                if s >= base {
                    let j = (s - base) as usize; // stamps of this search only: base.. base + k
                    let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                    if a != b {
                        parent[a] = b;
                    }
                } else {
                    ss.stamp[y as usize] = base + i as u32;
                    ss.q[i].push(y);
                    total += 1;
                    if total > SEARCH_BUDGET {
                        return Loss::Budget;
                    }
                }
            }
        }
        // classes: roots of the sources; a class is open while any member still has frontier
        let mut root = [0usize; 4];
        let mut classes = 0usize;
        let mut open = [false; 4];
        let mut size = [0usize; 4];
        for i in 0..k {
            let r = find(&mut parent, i);
            root[i] = r;
            if size[r] == 0 {
                classes += 1;
            }
            size[r] += ss.q[i].len();
            if head[i] < ss.q[i].len() {
                open[r] = true;
            }
        }
        if classes == 1 {
            return Loss::Connected;
        }
        let open_classes = (0..k).filter(|&i| root[i] == i && open[i]).count();
        if open_classes <= 1 || !progressed {
            // every class but at most one is a closed pocket: that is the whole answer
            let keep = if open_classes == 1 {
                (0..k).find(|&i| root[i] == i && open[i]).unwrap_or(0)
            } else {
                // all closed: keep the biggest class's label (relabel the smaller ones)
                (0..k)
                    .filter(|&i| root[i] == i)
                    .fold(None, |best: Option<usize>, i| match best {
                        Some(b) if size[b] >= size[i] => Some(b),
                        _ => Some(i),
                    })
                    .unwrap_or(0)
            };
            if let Some((c, sizes)) = comps {
                let mut removed = 1i32; // `t` itself
                for r in (0..k).filter(|&i| root[i] == i && i != keep) {
                    let label = sizes.len() as u32; // int: < number of tiles
                    let mut cnt = 0i32;
                    #[allow(clippy::needless_range_loop)]
                    for i in 0..k {
                        if root[i] == r {
                            for &x in &ss.q[i] {
                                labels[x as usize] = label; // in-range: valid tile ref, labels cover the map while valid
                            }
                            cnt += ss.q[i].len() as i32; // int: <= SEARCH_BUDGET
                        }
                    }
                    sizes.push(cnt);
                    removed += cnt;
                }
                sizes[c as usize] -= removed; // in-range: `c` labels an owned tile
            }
            return Loss::Split;
        }
    }
}

/// Gain rule for the labels: the label `t` takes when `me` gains it, or `None` when `t` would
/// merge two components. `Some(None)`: no owned 4-neighbour (a new component).
#[inline]
pub fn gain_label(map: &GameMap, labels: &[u32], t: TileRef, me: u16) -> Option<Option<u32>> {
    let mut nbuf = [0u32; 4];
    let n = map.neighbors4(t, &mut nbuf);
    let mut found: Option<u32> = None;
    for &nb in &nbuf[..n] {
        if map.owner_id(nb) == me {
            let c = labels[nb as usize]; // in-range: labels cover the map while a memo is valid
            match found {
                None => found = Some(c),
                Some(f) if f != c => return None,
                Some(_) => {}
            }
        }
    }
    Some(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_table_spot_checks() {
        // only N: a leaf; N and S without a connecting side: a bridge; N, NE, E: one run
        assert!(ring_ok(0b0000_0001));
        assert!(!ring_ok(0b0001_0001));
        assert!(ring_ok(0b0000_0111));
        assert!(!ring_ok(0b0000_0101)); // N and E only via t (NE missing)
        assert!(ring_ok(0b0000_0000));
        assert!(ring_ok(0xff));
        assert!(ring_ok(0b0001_1111)); // N..S along the east side
        assert!(!ring_ok(0b0101_0101)); // a plus sign: all four arms meet only at t
        assert!(ring_ok(0b1111_1110)); // everything but N, whose run is the rest
        assert!(ring_ok(0b1010_1010)); // diagonals only: no 4-neighbour in T
    }

    /// A fork never carries labels: its `CompLabels` is disabled and empty, its memos invalid
    /// (so it floods like TS and pays no label clone or allocation).
    #[test]
    fn clones_drop_labels_and_memos() {
        let mut l = CompLabels::default();
        assert!(l.enabled);
        l.ensure(16);
        l.v[3] = 7;
        let c = l.clone();
        assert!(!c.enabled && c.v.is_empty());
        let m = CompMemo { valid: true, sizes: vec![3, 4] };
        let mc = m.clone();
        assert!(!mc.valid && mc.sizes.is_empty());
    }
}
