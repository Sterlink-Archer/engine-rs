//! `Attack` (`engine/src/game/AttackImpl.ts`). Owner: **P6**.
//!
//! Fields: `Attack v1` snapshot order. The border is a JS `Set<number>` passed in by
//! `AttackExecution.init` (`AttackExecution.ts:139-144`); its order is state (CS A.border).
//! Methods touching only the attack are inherent; `delete()` (filter-reassigns both
//! players' lists, `AttackImpl.ts:61-71`) and `clusteredPositions()` (reads the map) are on
//! `Ctx`/`State`.

use crate::StateClone;
use crate::ctx::Ctx;
use crate::ids::{AttackId, AttackRef, Owner, SmallId, TileRef};
use crate::state::State;
use crate::tileset::OrderedTileSet;

/// `AttackImpl` data.
#[derive(StateClone, Debug, Default)]
pub struct Attack {
    /// `_id` (8-char base-36 → number, ARCHITECTURE §3.2).
    pub id: AttackId,
    pub target: Owner,
    pub attacker: SmallId,
    /// `_troops: number`.
    pub troops: f64,
    pub source_tile: Option<TileRef>,
    /// `_border: Set<number>`.
    pub border: OrderedTileSet,
    /// `_borderSize`. // int: a count.
    pub border_size: i32,
    pub is_active: bool,
    pub retreating: bool,
    pub retreated: bool,
}

impl Attack {
    /// `new AttackImpl(id, target, attacker, troops, sourceTile, border, mg)` (`:26-34`).
    /// Field initialisers (`:21-24`): `_isActive = true`, `_borderSize = 0`,
    /// `_retreating = _retreated = false`. The constructor stores `troops` as given (no
    /// clamp: only `setTroops` clamps, `:49-51`).
    pub fn new(
        id: AttackId,
        target: Owner,
        attacker: SmallId,
        troops: f64,
        source_tile: Option<TileRef>,
        border: OrderedTileSet,
    ) -> Self {
        Attack {
            id,
            target,
            attacker,
            troops,
            source_tile,
            border,
            border_size: 0,
            is_active: true,
            retreating: false,
            retreated: false,
        }
    }
    #[inline]
    pub fn source_tile(&self) -> Option<TileRef> {
        self.source_tile
    }
    #[inline]
    pub fn target(&self) -> Owner {
        self.target
    }
    #[inline]
    pub fn attacker(&self) -> SmallId {
        self.attacker
    }
    #[inline]
    pub fn troops(&self) -> f64 {
        self.troops
    }
    /// `setTroops(troops)`: `this._troops = Math.max(0, troops)` (`:49-51`; JS `max`, so NaN
    /// stays NaN).
    #[inline]
    pub fn set_troops(&mut self, troops: f64) {
        self.troops = of_det::js::max(0.0, troops);
    }
    #[inline]
    pub fn is_active(&self) -> bool {
        self.is_active
    }
    #[inline]
    pub fn id(&self) -> AttackId {
        self.id
    }
    /// `orderRetreat()` (`:75-77`).
    #[inline]
    pub fn order_retreat(&mut self) {
        self.retreating = true;
    }
    /// `executeRetreat()` (`:79-81`).
    #[inline]
    pub fn execute_retreat(&mut self) {
        self.retreated = true;
    }
    #[inline]
    pub fn retreating(&self) -> bool {
        self.retreating
    }
    #[inline]
    pub fn retreated(&self) -> bool {
        self.retreated
    }
    #[inline]
    pub fn border_size(&self) -> i32 {
        self.border_size
    }
    /// `borderTiles()` (the live set).
    #[inline]
    pub fn border_tiles(&self) -> &OrderedTileSet {
        &self.border
    }
    /// `clearBorder()` (`:99-102`).
    pub fn clear_border(&mut self) {
        self.border_size = 0;
        self.border.clear();
    }
    /// `addBorderTile(tile)` (`:104-109`).
    pub fn add_border_tile(&mut self, tile: TileRef) {
        // `add` inserts iff `!has(tile)` (one hash probe instead of three, t409 H5)
        if self.border.add(tile) {
            self.border_size += 1;
        }
    }
    /// `removeBorderTile(tile)` (`:111-116`).
    pub fn remove_border_tile(&mut self, tile: TileRef) {
        // `delete` removes iff `has(tile)` (one probe instead of two, t409 H5)
        if self.border.delete(tile) {
            self.border_size -= 1;
        }
    }
}

/// One `{ tile, size }` record of `clusterBorderTiles` (`AttackImpl.ts:140`).
#[derive(Clone, Copy, Debug)]
struct BorderCluster {
    tile: TileRef,
    /// int: a tile count.
    size: u32,
}

impl State {
    #[inline]
    pub fn attack(&self, a: AttackRef) -> &Attack {
        self.attacks.get(a)
    }
    #[inline]
    pub fn attack_mut(&mut self, a: AttackRef) -> &mut Attack {
        self.attacks.get_mut(a)
    }
    /// `attack.clusteredPositions()` (`AttackImpl.ts:120-126`): the top 2 clustered positions
    /// of the attack's border (`clusterBorderTiles(30, 2)`), or `[sourceTile]` / `[]` when
    /// the border is empty.
    pub fn attack_clustered_positions(&self, a: AttackRef) -> Vec<TileRef> {
        let att = self.attack(a);
        if att.border_size == 0 {
            // TS: :122-123
            return match att.source_tile() {
                Some(tile) => vec![tile],
                None => Vec::new(),
            };
        }
        self.attack_cluster_border_tiles(a, 30, 2)
    }

    /// `clusterBorderTiles(minSize, maxClusters)` (`AttackImpl.ts:137-202`).
    fn attack_cluster_border_tiles(&self, a: AttackRef, min_size: u32, max_clusters: usize) -> Vec<TileRef> {
        let map = &self.map;
        let border = &self.attack(a).border;
        // lookup-only: `visited` is only `.has`/`.add`ed (`:139, :143, :159-160`).
        let mut visited: crate::jsorder::LookupSet<TileRef> = crate::jsorder::LookupSet::new();
        let mut clusters: Vec<BorderCluster> = Vec::new();
        let mut queue: Vec<TileRef> = Vec::new();
        let mut nb = [0u32; 8];

        // iter-live: the border is only read here (`for (const startTile of this._border)`).
        let mut c = border.cursor();
        while let Some(start_tile) = border.next_live(&mut c) {
            if visited.contains(&start_tile) {
                continue;
            }
            queue.clear();
            queue.push(start_tile);
            visited.insert(start_tile);
            let mut qi = 0usize;
            let mut sum_x = 0.0f64;
            let mut sum_y = 0.0f64;
            let mut count = 0u32;
            while qi < queue.len() {
                let t = queue[qi]; // in-range: loop condition
                qi += 1;
                sum_x += f64::from(map.x(t));
                sum_y += f64::from(map.y(t));
                count += 1;
                // TS: `this._mg.forEachNeighborWithDiag(t, ...)` (:158-163); the callback only
                // reads the border and grows the local queue.
                let n = map.neighbors_with_diag(t, &mut nb);
                for &neighbor in &nb[..n] {
                    if border.has(neighbor) && !visited.contains(&neighbor) {
                        visited.insert(neighbor);
                        queue.push(neighbor);
                    }
                }
            }
            // TS: :169-181 (centroid, then the closest real tile; strict `<` keeps the first)
            let cx = sum_x / f64::from(count);
            let cy = sum_y / f64::from(count);
            let mut best = queue[0]; // in-range: the queue holds start_tile
            let mut best_dist = f64::INFINITY;
            for &t in &queue {
                let dx = f64::from(map.x(t)) - cx;
                let dy = f64::from(map.y(t)) - cy;
                let dist = dx * dx + dy * dy;
                if dist < best_dist {
                    best_dist = dist;
                    best = t;
                }
            }
            clusters.push(BorderCluster { tile: best, size: count });
        }

        // TS: :185 `clusters.sort((a, b) => b.size - a.size)` (V8 TimSort)
        of_det::js_sort_by(&mut clusters, |a, b| f64::from(b.size) - f64::from(a.size));

        match clusters.len() {
            0 => Vec::new(),
            1 => vec![clusters[0].tile],
            _ => {
                let significant: Vec<BorderCluster> = clusters.iter().copied().filter(|c| c.size >= min_size).collect();
                if significant.is_empty() {
                    return vec![clusters[0].tile];
                }
                significant.iter().take(max_clusters).map(|c| c.tile).collect()
            }
        }
    }
}

impl Ctx<'_> {
    /// `attack.delete()` (`AttackImpl.ts:61-71`): filter-reassigns `target._incomingAttacks`
    /// (target is a player) and `attacker._outgoingAttacks`, then marks the attack inactive.
    pub fn attack_delete(&mut self, a: AttackRef) {
        let (target, attacker) = {
            let att = self.st.attack(a);
            (att.target, att.attacker)
        };
        if target.is_player() {
            // TS: :62-66 (`a !== this`: handle identity)
            self.st.player_data_mut(target).incoming_attacks.filter_reassign(|x| *x != a);
        }
        // TS: :68-70
        self.st.player_data_mut(attacker).outgoing_attacks.filter_reassign(|x| *x != a);
        self.st.attack_mut(a).is_active = false;
    }
}

// CS v1 A row (`AttackImpl.snapshot`, `AttackImpl.ts:204-217`; dump record `Attack v1`), in dump
// field order: `target` is `w.owner(..)` (smallID, 0 = TerraNullius), `border` is `w.tiles(..)`
// (Set iteration order). Written at merge1 by the integrator on behalf of P6 (the row was
// missing, so every scenario with an attack stopped as "Unimplemented").
crate::impl_canon_record!(Attack {
    "id" => id,
    "target" => target,
    "attacker" => attacker,
    "troops" => troops,
    "sourceTile" => source_tile,
    "border" => border,
    "borderSize" => border_size,
    "isActive" => is_active,
    "retreating" => retreating,
    "retreated" => retreated,
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn border_bookkeeping_matches_attack_impl() {
        let mut a = Attack::new(AttackId(5), SmallId(2), SmallId(1), 10.0, None, OrderedTileSet::new());
        assert!(a.is_active() && !a.retreating() && !a.retreated());
        assert_eq!(a.border_size(), 0);
        a.add_border_tile(3);
        a.add_border_tile(3); // already present: no double count (:105)
        a.add_border_tile(9);
        assert_eq!(a.border_size(), 2);
        a.remove_border_tile(4); // absent: no change (:112)
        a.remove_border_tile(3);
        assert_eq!(a.border_size(), 1);
        assert_eq!(a.border_tiles().to_vec(), [9]);
        a.add_border_tile(3); // re-add goes to the end (JS Set order)
        assert_eq!(a.border_tiles().to_vec(), [9, 3]);
        a.clear_border();
        assert_eq!((a.border_size(), a.border_tiles().size()), (0, 0));
    }

    #[test]
    fn set_troops_is_js_max_with_zero() {
        let mut a = Attack::default();
        a.set_troops(-3.5);
        assert_eq!(a.troops(), 0.0);
        a.set_troops(7.25);
        assert_eq!(a.troops(), 7.25);
        a.set_troops(f64::NAN); // Math.max(0, NaN) = NaN
        assert!(a.troops().is_nan());
        a.set_troops(-0.0); // Math.max(0, -0) = +0
        assert!(a.troops() == 0.0 && a.troops().is_sign_positive());
        // The constructor does not clamp.
        let b = Attack::new(AttackId(1), SmallId(0), SmallId(1), -2.0, Some(7), OrderedTileSet::new());
        assert_eq!((b.troops(), b.source_tile()), (-2.0, Some(7)));
    }

    #[test]
    fn retreat_flags() {
        let mut a = Attack::default();
        a.order_retreat();
        assert!(a.retreating() && !a.retreated());
        a.execute_retreat();
        assert!(a.retreated());
    }
}
