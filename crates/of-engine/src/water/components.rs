//! `ConnectedComponents` (`pathfinding/algorithms/ConnectedComponents.ts`). Owner: **P4a**.
//!
//! The minimap water components: **state** (CS W.components), incrementally mutated by water
//! nukes (`addWaterTiles`, `:135`), u8 → u16 promotion at id 253 (`:96-116`; the typed kind
//! is state). Held as `Arc<Components>` with COW (`Arc::make_mut`) in `WaterManager` and is
//! **the only component source** (every graph reads it live, §3.8).
//!
//! Path compression (`find`, `:190-202`) is a pure optimisation: resolved roots are
//! identical with or without it, and the snapshot exports roots only (`:58-62`). So the
//! `&self` readers ([`Components::get_component_id`], [`Components::get_component_size`])
//! resolve without compressing, and only the `&mut` merge path compresses (as TS does).

use crate::StateClone;
use crate::ids::TileRef;
use crate::map::GameMap;
use crate::water::map_view::WaterMap;

/// `LAND_MARKER = 0xff` (`:6`), `0xffff` after promotion.
pub const LAND_MARKER: u32 = 0xff;
/// `LAND_MARKER_WIDE = 0xffff` (`:7`).
pub const LAND_MARKER_WIDE: u32 = 0xffff;

/// `componentIds: Uint8Array | Uint16Array` (`instanceof` sites `:98,116,209`).
#[derive(StateClone, Debug)]
pub enum ComponentIds {
    U8(Vec<u8>),
    U16(Vec<u16>),
}

impl Default for ComponentIds {
    fn default() -> Self {
        ComponentIds::U8(Vec::new())
    }
}

impl ComponentIds {
    /// `ids[i]` (`undefined` out of range).
    #[inline]
    pub fn get(&self, i: usize) -> Option<u32> {
        match self {
            ComponentIds::U8(v) => v.get(i).map(|&x| u32::from(x)),
            ComponentIds::U16(v) => v.get(i).map(|&x| u32::from(x)),
        }
    }
    /// `ids[i] = v`: a typed-array store (ToUint8/ToUint16 modular), ignored out of range.
    #[inline]
    fn set(&mut self, i: usize, v: u32) {
        match self {
            ComponentIds::U8(a) => {
                if let Some(x) = a.get_mut(i) {
                    *x = v as u8; // TS: Uint8Array store (mod 256)
                }
            }
            ComponentIds::U16(a) => {
                if let Some(x) = a.get_mut(i) {
                    *x = v as u16; // TS: Uint16Array store (mod 65536)
                }
            }
        }
    }
    pub fn len(&self) -> usize {
        match self {
            ComponentIds::U8(v) => v.len(),
            ComponentIds::U16(v) => v.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn is_u16(&self) -> bool {
        matches!(self, ComponentIds::U16(_))
    }
}

/// `ConnectedComponents` data, snapshot `components` record order.
#[derive(StateClone, Debug, Default)]
pub struct Components {
    /// `componentIds` (`null` before `initialize`).
    pub component_ids: Option<ComponentIds>,
    /// `_componentSizes: number[]` with holes (`?undef` entries).
    pub component_sizes: Vec<Option<u32>>,
    /// `parents: number[]` (union-find of merged components).
    pub parents: Vec<u32>,
    pub max_id: u32,
    pub land_marker: u32,
    /// non-snapshot: `width`, `height`, `numTiles`, `lastRowStart` (fixed per map).
    pub width: u32,
    /// non-snapshot: see `width`.
    pub height: u32,
}

/// `ConnectedComponentsState` (`:9-15`): the snapshot/restore record (CS W.components).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComponentsSnapshot {
    pub component_ids: ComponentIdsSnap,
    pub component_sizes: Vec<Option<u32>>,
    /// Roots only (`:58-62`).
    pub parents: Vec<u32>,
    pub max_id: u32,
    pub land_marker: u32,
}

/// `componentIds` of a snapshot: `zBytes()` or `zU16Array()` (`WaterManager.ts:900`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComponentIdsSnap {
    U8(Vec<u8>),
    U16(Vec<u16>),
}

impl Components {
    /// `new ConnectedComponents(map)` (`:40-49`).
    pub fn new(map: &GameMap) -> Self {
        Self::new_with(map)
    }

    pub(crate) fn new_with<M: WaterMap + ?Sized>(map: &M) -> Self {
        Components {
            component_ids: None,
            component_sizes: Vec::new(),
            parents: Vec::new(),
            max_id: 0,
            land_marker: LAND_MARKER,
            width: map.width(),
            height: map.height(),
        }
    }

    #[inline]
    fn num_tiles(&self) -> usize {
        (self.width as usize) * (self.height as usize)
    }
    #[inline]
    fn last_row_start(&self) -> usize {
        ((self.height as usize).saturating_sub(1)) * (self.width as usize) // TS: :47
    }

    /// `initialize()` (`:77-133`): scan-line flood fill over the minimap
    /// (`accessTerrainDirectly = true`, the default and the only production setting).
    #[allow(clippy::ptr_arg)] // the BFS queue grows (TS `Int32Array` sized on first use)
    pub fn initialize(&mut self, map: &GameMap, queue: &mut Vec<i32>) {
        self.initialize_with(map, true, queue)
    }

    /// `initialize()` with the constructor's `accessTerrainDirectly` flag (`:42`).
    pub(crate) fn initialize_with<M: WaterMap + ?Sized>(
        &mut self,
        map: &M,
        access_terrain_directly: bool,
        queue: &mut Vec<i32>,
    ) {
        let n = self.num_tiles();
        // TS: :79 `this.queue = new Int32Array(this.numTiles)` (reused buffer; contents are
        // only read below the current tail, see `flood_fill_component`).
        if queue.len() < n {
            queue.resize(n, 0);
        }
        let mut ids = ComponentIds::U8(self.create_prefilled_ids(map, access_terrain_directly));
        self.component_sizes = Vec::new();
        let mut next_id: u32 = 0;
        // TS: :86-109
        for start in 0..n {
            let value = ids.get(start).unwrap_or(0); // in-range: start < numTiles
            if value == LAND_MARKER || value > 0 {
                continue;
            }
            next_id += 1;
            // TS: :98-100 promote before assigning component 254 (0xff collides with land)
            if next_id == 253
                && let ComponentIds::U8(v) = &ids
            {
                ids = ComponentIds::U16(upgrade_to_u16(v));
            }
            // TS: :104-106
            if next_id == 0xffff {
                break;
            }
            self.flood_fill_component(&mut ids, start, next_id, queue);
        }
        // TS: :115-119
        self.land_marker = if ids.is_u16() { LAND_MARKER_WIDE } else { LAND_MARKER };
        self.component_ids = Some(ids);
        self.max_id = next_id;
        self.parents = (0..=next_id).collect();
    }

    /// `createPrefilledIds()` (`:221-231`).
    fn create_prefilled_ids<M: WaterMap + ?Sized>(&self, map: &M, direct: bool) -> Vec<u8> {
        let n = self.num_tiles();
        let mut ids = vec![0u8; n];
        if direct {
            // TS: :249-273 `premarkLandTilesDirect`: 4 bytes at a time through Uint32Array
            // views (little-endian on every supported target), then the tail bytes.
            // `-((chunk >> 7) & 1) & 0xff` is 0xff for a land byte (bit 7), else 0.
            let terrain = map.terrain();
            let num_chunks = n / 4; // TS: Math.floor(numTiles / 4)
            for i in 0..num_chunks {
                let o = i * 4;
                let chunk = u32::from_le_bytes([terrain[o], terrain[o + 1], terrain[o + 2], terrain[o + 3]]) as i32;
                let b0 = (-((chunk >> 7) & 1)) & 0xff;
                let b1 = (-((chunk >> 15) & 1)) & 0xff;
                let b2 = (-((chunk >> 23) & 1)) & 0xff;
                let b3 = -((chunk >> 31) & 1);
                let word = (b0 | (b1 << 8) | (b2 << 16) | (b3 << 24)) as u32; // TS: Uint32Array store
                ids[o..o + 4].copy_from_slice(&word.to_le_bytes());
            }
            for (i, id) in ids.iter_mut().enumerate().take(n).skip(num_chunks * 4) {
                *id = (-(i32::from(terrain[i]) >> 7)) as u8; // TS: :271 Uint8Array store of -1
            }
        } else {
            // TS: :238-242 `premarkLandTiles`
            for (i, id) in ids.iter_mut().enumerate() {
                *id = if map.is_water(i as TileRef) { 0 } else { 0xff };
            }
        }
        ids
    }

    /// `floodFillComponent(ids, start, componentId)` (`:293-348`), scan-line fill.
    ///
    /// The TS queue is an `Int32Array(numTiles)`: a push past `numTiles` is a dropped typed
    /// array write (but `tail` still advances) and a pop past it reads `undefined`, so
    /// `ids[undefined] !== 0` skips it. Ported literally.
    fn flood_fill_component(&mut self, ids: &mut ComponentIds, start: usize, component_id: u32, queue: &mut [i32]) {
        let n = self.num_tiles();
        let w = self.width as usize;
        let last_row_start = self.last_row_start();
        let mut head = 0usize;
        let mut tail = 0usize;
        let push = |queue: &mut [i32], tail: &mut usize, v: usize| {
            if *tail < n {
                queue[*tail] = v as i32; // in-range: tail < numTiles <= queue.len()
            }
            *tail += 1;
        };
        push(queue, &mut tail, start);
        while head < tail {
            let slot = head;
            head += 1;
            if slot >= n {
                continue; // TS: `queue[head]` is undefined → `ids[undefined] !== 0`
            }
            let seed = queue[slot] as usize; // in-range: written this fill (slot < tail, slot < n)
            if ids.get(seed) != Some(0) {
                continue;
            }
            // TS: :311-322
            let mut left = seed;
            let row_start = seed - (seed % w);
            while left > row_start && ids.get(left - 1) == Some(0) {
                left -= 1;
            }
            let mut right = seed;
            let row_end = row_start + w - 1;
            while right < row_end && ids.get(right + 1) == Some(0) {
                right += 1;
            }
            // TS: :325-346
            let span_size = (right - left + 1) as u32; // int: span length
            self.add_size(component_id, span_size);
            for x in left..=right {
                ids.set(x, component_id);
                if x >= w {
                    let above = x - w;
                    if ids.get(above) == Some(0) {
                        push(queue, &mut tail, above);
                    }
                }
                if x < last_row_start {
                    let below = x + w;
                    if ids.get(below) == Some(0) {
                        push(queue, &mut tail, below);
                    }
                }
            }
        }
    }

    /// `_componentSizes[id] = (_componentSizes[id] ?? 0) + d`.
    fn add_size(&mut self, id: u32, d: u32) {
        let i = id as usize;
        if self.component_sizes.len() <= i {
            self.component_sizes.resize(i + 1, None);
        }
        let cur = self.component_sizes[i].unwrap_or(0); // in-range: resized above
        self.component_sizes[i] = Some(cur + d);
    }

    /// `_componentSizes[id] = v`.
    fn set_size(&mut self, id: u32, v: u32) {
        let i = id as usize;
        if self.component_sizes.len() <= i {
            self.component_sizes.resize(i + 1, None);
        }
        self.component_sizes[i] = Some(v); // in-range: resized above
    }

    #[inline]
    fn size_of(&self, id: u32) -> u32 {
        self.component_sizes.get(id as usize).copied().flatten().unwrap_or(0) // `?? 0`
    }

    /// `addWaterTiles(tiles)` (`:135-181`): incremental merge after water conversions.
    /// No-op before `initialize()`. Tiles must already be water in the map.
    #[allow(clippy::ptr_arg)] // the BFS queue grows
    pub fn add_water_tiles(&mut self, map: &GameMap, tiles: &[TileRef], queue: &mut Vec<i32>) {
        let _ = (map, queue); // TS addWaterTiles reads neither the map nor the queue
        self.add_water_tiles_impl(tiles)
    }

    pub(crate) fn add_water_tiles_impl(&mut self, tiles: &[TileRef]) {
        if self.component_ids.is_none() {
            return; // TS: :136
        }
        let w = self.width as usize;
        let last_row_start = self.last_row_start();
        for &tile in tiles {
            let t = tile as usize;
            let ids = self.component_ids.as_ref().ts_ids(); // rust-invariant: checked above
            if ids.get(t) != Some(self.land_marker) {
                continue; // TS: :140 already labeled water (an OOB tile reads undefined)
            }
            // TS: :143-151 distinct roots of the cardinal water neighbours
            let x = t % w;
            let mut r0 = 0;
            let mut r1 = 0;
            let mut r2 = 0;
            let mut r3 = 0;
            if t >= w {
                r0 = self.root_at(t - w);
            }
            if t < last_row_start {
                r1 = self.root_at(t + w);
            }
            if x > 0 {
                r2 = self.root_at(t - 1);
            }
            if x < w - 1 {
                r3 = self.root_at(t + 1);
            }
            // TS: :154-157 canonical id: smallest non-zero root
            let mut canon = 0;
            for r in [r0, r1, r2, r3] {
                if r != 0 && (canon == 0 || r < canon) {
                    canon = r;
                }
            }
            if canon == 0 {
                // TS: :159-166 isolated new water
                let id = self.alloc_component_id();
                if let Some(ids) = self.component_ids.as_mut() {
                    ids.set(t, id); // may have been upgraded to u16
                }
                self.set_size(id, 1);
                continue;
            }
            if let Some(ids) = self.component_ids.as_mut() {
                ids.set(t, canon);
            }
            let s = self.size_of(canon) + 1;
            self.set_size(canon, s);
            // TS: :172-179 union the other adjacent roots into canon
            for r in [r0, r1, r2, r3] {
                if r != 0 && r != canon && self.parents.get(r as usize).copied() != Some(canon) {
                    self.set_parent(r, canon);
                    let merged = self.size_of(canon) + self.size_of(r);
                    self.set_size(canon, merged);
                    self.set_size(r, 0);
                }
            }
        }
    }

    /// `parents[i] = v` on a JS array (extends with holes; never needed for valid ids).
    fn set_parent(&mut self, i: u32, v: u32) {
        let iu = i as usize;
        if self.parents.len() <= iu {
            self.parents.resize(iu + 1, 0);
        }
        self.parents[iu] = v; // in-range: resized above
    }

    /// `rootAt(index)` (`:184-188`).
    fn root_at(&mut self, index: usize) -> u32 {
        let id = self.component_ids.as_ref().ts_ids().get(index).unwrap_or(0); // in-range: neighbour of a valid tile
        if id == 0 || id == self.land_marker {
            return 0;
        }
        self.find_mut(id)
    }

    /// `find(id)` with path compression (`:190-202`).
    fn find_mut(&mut self, id: u32) -> u32 {
        let root = self.find(id);
        let mut cur = id;
        while self.parents[cur as usize] != root {
            // in-range: every id on the chain is <= maxId < parents.len()
            let next = self.parents[cur as usize];
            self.parents[cur as usize] = root;
            cur = next;
        }
        root
    }

    /// `find(id)` without compression (same root, see the module doc).
    #[inline]
    fn find(&self, id: u32) -> u32 {
        let mut root = id;
        while self.parents[root as usize] != root {
            // in-range: union-find ids are <= maxId (parents has maxId + 1 entries)
            root = self.parents[root as usize];
        }
        root
    }

    /// `allocComponentId()` (`:204-215`).
    fn alloc_component_id(&mut self) -> u32 {
        if self.max_id >= 0xfffe {
            return self.max_id; // saturate (unreachable in practice)
        }
        self.max_id += 1;
        let id = self.max_id;
        if id >= 253
            && let Some(ComponentIds::U8(v)) = &self.component_ids
        {
            let up = upgrade_to_u16(v);
            self.component_ids = Some(ComponentIds::U16(up));
            self.land_marker = LAND_MARKER_WIDE;
        }
        self.set_parent(id, id);
        id
    }

    /// `getComponentId(tile)` (`:350-356`): 0 before initialize or out of range; the land
    /// marker for land; else the union-find root.
    pub fn get_component_id(&self, tile: TileRef) -> u32 {
        let Some(ids) = self.component_ids.as_ref() else {
            return 0;
        };
        let id = ids.get(tile as usize).unwrap_or(0); // `?? 0`
        if id == 0 || id == self.land_marker {
            return id;
        }
        self.find(id)
    }

    /// `getComponentSize(componentId)` (`:358-362`).
    pub fn get_component_size(&self, component_id: u32) -> u32 {
        if component_id == 0 || component_id > self.max_id {
            return 0;
        }
        self.size_of(self.find(component_id))
    }

    /// `snapshot()` (`:51-66`): `None` before initialize; parents as roots.
    pub fn snapshot(&self) -> Option<ComponentsSnapshot> {
        let ids = self.component_ids.as_ref()?;
        Some(ComponentsSnapshot {
            component_ids: match ids {
                ComponentIds::U8(v) => ComponentIdsSnap::U8(v.clone()),
                ComponentIds::U16(v) => ComponentIdsSnap::U16(v.clone()),
            },
            component_sizes: self.component_sizes.clone(),
            parents: (0..self.parents.len() as u32).map(|id| self.find(id)).collect(),
            max_id: self.max_id,
            land_marker: self.land_marker,
        })
    }

    /// `restoreSnapshot(s)` (`:69-75`): replaces `initialize()`.
    pub fn restore_snapshot(&mut self, s: &ComponentsSnapshot) {
        self.component_ids = Some(match &s.component_ids {
            ComponentIdsSnap::U8(v) => ComponentIds::U8(v.clone()),
            ComponentIdsSnap::U16(v) => ComponentIds::U16(v.clone()),
        });
        self.component_sizes = s.component_sizes.clone();
        self.parents = s.parents.clone();
        self.max_id = s.max_id;
        self.land_marker = s.land_marker;
    }
}

/// `upgradeToUint16Array(ids)` (`:279-285`): 0xff land markers become 0xffff.
fn upgrade_to_u16(ids: &[u8]) -> Vec<u16> {
    ids.iter().map(|&x| if u32::from(x) == LAND_MARKER { LAND_MARKER_WIDE as u16 } else { u16::from(x) }).collect()
}

/// `this.componentIds!` on a path where TS already checked it (rust-invariant).
trait TsIds<'a> {
    fn ts_ids(self) -> &'a ComponentIds;
}
impl<'a> TsIds<'a> for Option<&'a ComponentIds> {
    #[inline]
    fn ts_ids(self) -> &'a ComponentIds {
        match self {
            Some(x) => x,
            None => unreachable!("componentIds checked non-null by the caller (ConnectedComponents.ts:136)"),
        }
    }
}
