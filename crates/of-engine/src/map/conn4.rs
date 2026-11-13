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
}

impl Default for CompLabels {
    fn default() -> Self {
        CompLabels { enabled: true, v: Vec::new() }
    }
}

impl Clone for CompLabels {
    fn clone(&self) -> Self {
        CompLabels { enabled: false, v: Vec::new() }
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
