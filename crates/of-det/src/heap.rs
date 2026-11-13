//! Exact port of `packages/engine/src/execution/utils/FlatBinaryHeap.ts`.
//!
//! Traps reproduced (checked by `of-vectors`, order suite `FlatBinaryHeap`):
//! - priorities are STORED as f32 (`Float32Array`) but the incoming priority is compared
//!   as f64 against the f32-stored parents during sift-up;
//! - `dequeue` re-reads the last element's priority from the f32 store;
//! - NaN priorities: `priority >= parent` is false, so NaN sifts to the root;
//! - ties dequeue by array position (the exact layout is snapshot state, SPEC §1.3 E).

#[derive(Debug)]
pub struct FlatBinaryHeap {
    pri: Vec<f32>,
    tiles: Vec<u32>,
    len: usize,
}

impl Clone for FlatBinaryHeap {
    fn clone(&self) -> Self {
        FlatBinaryHeap { pri: self.pri.clone(), tiles: self.tiles.clone(), len: self.len }
    }
    /// Reuses both buffers (pooled forks, ARCHITECTURE §8.1; ledger t088 item (g)).
    fn clone_from(&mut self, src: &Self) {
        self.pri.clone_from(&src.pri);
        self.tiles.clone_from(&src.tiles);
        self.len = src.len;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeapEmpty;

/// `getState()` layout: live entries in array order + allocated capacity.
#[derive(Clone, Debug, PartialEq)]
pub struct HeapState {
    pub pri: Vec<f32>,
    pub tiles: Vec<u32>,
    pub capacity: usize,
}

impl Default for FlatBinaryHeap {
    fn default() -> Self {
        Self::new(1024)
    }
}

impl FlatBinaryHeap {
    pub fn new(capacity: usize) -> Self {
        FlatBinaryHeap { pri: vec![0.0; capacity], tiles: vec![0; capacity], len: 0 }
    }
    #[inline]
    pub fn clear(&mut self) {
        self.len = 0;
    }
    #[inline]
    pub fn size(&self) -> usize {
        self.len
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    /// Allocated capacity (`pri.length`); not state (SPEC §1.6 N3) but reported by getState.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.pri.len()
    }

    pub fn enqueue(&mut self, tile: u32, priority: f64) {
        if self.len == self.pri.len() {
            self.grow();
        }
        let mut i = self.len;
        self.len += 1;
        while i > 0 {
            let parent = (i - 1) >> 1;
            if priority >= self.pri[parent] as f64 {
                break;
            }
            self.pri[i] = self.pri[parent];
            self.tiles[i] = self.tiles[parent];
            i = parent;
        }
        self.pri[i] = priority as f32;
        self.tiles[i] = tile;
    }

    pub fn dequeue(&mut self) -> Result<u32, HeapEmpty> {
        if self.len == 0 {
            return Err(HeapEmpty);
        }
        let top = self.tiles[0];
        self.len -= 1;
        let last_pri = self.pri[self.len];
        let last_tile = self.tiles[self.len];
        let mut i = 0;
        loop {
            let left = (i << 1) + 1;
            if left >= self.len {
                break;
            }
            let right = left + 1;
            let child = if right < self.len && self.pri[right] < self.pri[left] { right } else { left };
            if last_pri <= self.pri[child] {
                break;
            }
            self.pri[i] = self.pri[child];
            self.tiles[i] = self.tiles[child];
            i = child;
        }
        self.pri[i] = last_pri;
        self.tiles[i] = last_tile;
        Ok(top)
    }

    pub fn get_state(&self) -> HeapState {
        HeapState {
            pri: self.pri[..self.len].to_vec(),
            tiles: self.tiles[..self.len].to_vec(),
            capacity: self.pri.len(),
        }
    }

    pub fn from_state(s: &HeapState) -> Self {
        let mut h = Self::new(s.capacity);
        h.pri[..s.pri.len()].copy_from_slice(&s.pri);
        h.tiles[..s.tiles.len()].copy_from_slice(&s.tiles);
        h.len = s.tiles.len();
        h
    }

    fn grow(&mut self) {
        // JS: `this.pri.length << 1` (a capacity-0 heap would stay 0 and then write out of
        // bounds into a holey array; the engine never constructs one).
        let new_cap = (self.pri.len() << 1).max(1);
        self.pri.resize(new_cap, 0.0);
        self.tiles.resize(new_cap, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clone_from_reuses_buffers() {
        let mut src = FlatBinaryHeap::new(16);
        for (i, p) in [5.0, 1.0, 3.0].into_iter().enumerate() {
            src.enqueue(i as u32, p);
        }
        let mut dst = FlatBinaryHeap::new(16);
        let ptr = dst.pri.as_ptr();
        dst.clone_from(&src);
        assert_eq!(dst.pri.as_ptr(), ptr);
        assert_eq!(dst.get_state(), src.get_state());
        assert_eq!((dst.dequeue(), dst.dequeue(), dst.dequeue()), (Ok(1), Ok(2), Ok(0)));
    }
    /// Guard for a case the golden vectors do not cover (mutation M3 in README survived
    /// them): the incoming f64 priority is compared UNROUNDED against f32-stored parents.
    /// Expected values from the real TS FlatBinaryHeap:
    /// `harness/ofh conformance/tools/rs_heap_probe.ts` ->
    /// `{"fround":1,"tiles":[11,10],"pri":[1,1],"drain":[11,10]}`.
    #[test]
    fn f64_incoming_vs_f32_parent() {
        let mut h = FlatBinaryHeap::new(4);
        h.enqueue(10, 1.0);
        h.enqueue(11, 1.0 - 1e-12);
        let st = h.get_state();
        assert_eq!(st.tiles, vec![11, 10]);
        assert_eq!(st.pri, vec![1.0, 1.0]);
        assert_eq!((h.dequeue(), h.dequeue()), (Ok(11), Ok(10)));
    }
}
