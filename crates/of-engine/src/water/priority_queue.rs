//! `MinHeap` / `BucketQueue` (`pathfinding/algorithms/PriorityQueue.ts`). Owner: **P4a**.
//! Float32 priorities (`:11,16,29`); `priority | 0` bucket index (`:126`). Scratch objects.
//!
//! Capacity is not state (SPEC N3): TS's resize-with-console.error path (`:20-35`) and the
//! per-bucket doubling (`:135-139`) become `Vec` growth. Everything observable (pop order,
//! including f32 rounding and tie handling by heap position) is literal.

/// One heap slot: the `Float32Array` priority and the `Int32Array` node at the same index,
/// stored together so a level of the heap walk touches one cache line (P3 perf, t066).
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct HeapSlot {
    pub pri: f32,
    pub node: i32,
}

/// `MinHeap` (`:9-105`): binary heap over `Int32Array` nodes with `Float32Array` priorities
/// (kept as one array of [`HeapSlot`]s; positions are TS's).
#[derive(Debug, Default, Clone)]
pub struct MinHeap {
    pub slots: Vec<HeapSlot>,
    /// `size` (int: element count).
    pub size: usize,
}

impl MinHeap {
    /// `new MinHeap(capacity)` (`:14-17`).
    pub fn new(capacity: usize) -> Self {
        MinHeap { slots: vec![HeapSlot::default(); capacity], size: 0 }
    }

    /// The `heap` (node) array up to `size`, in heap order (tests, debugging).
    pub fn nodes(&self) -> Vec<i32> {
        self.slots[..self.size].iter().map(|s| s.node).collect()
    }
    /// The `priorities` array up to `size`, in heap order.
    pub fn priorities(&self) -> Vec<f32> {
        self.slots[..self.size].iter().map(|s| s.pri).collect()
    }

    /// `push(node, priority)` (`:19-54`). The f64 priority is stored as f32 (`Float32Array`
    /// store = round-to-nearest-even, PORTING_GUIDE §14.2); the bubble-up compares the
    /// **stored** f32 values (`:44`).
    #[inline]
    pub fn push(&mut self, node: i32, priority: f64) {
        // TS: :20-35 capacity doubling (not observable).
        if self.size >= self.slots.len() {
            let cap = (self.slots.len() * 2).max(16);
            self.slots.resize(cap, HeapSlot::default());
        }
        let pri = priority as f32; // TS: :39 Float32Array store
        let mut i = self.size;
        self.size += 1;
        // TS: :42-53 bubble up. A "hole" walk: parents move down until the stored priority's
        // place is found, then the node is written once. Same final positions as TS's
        // swap-per-level loop (P3 perf, t066; `tests::hole_heap_equals_swap_heap`).
        let slots = &mut self.slots[..self.size];
        while i > 0 {
            let parent = (i - 1) >> 1;
            let ps = slots[parent];
            if ps.pri <= pri {
                break;
            }
            slots[i] = ps;
            i = parent;
        }
        slots[i] = HeapSlot { pri, node };
    }

    /// `pop()` (`:56-95`). Callers always test `is_empty()` first (every A* loop,
    /// `AStar.*.ts`); TS's pop on an empty heap (size → -1) is unreachable.
    #[inline]
    pub fn pop(&mut self) -> i32 {
        assert!(self.size > 0, "MinHeap::pop on an empty heap (rust-invariant: callers check is_empty)");
        let result = self.slots[0].node;
        self.size -= 1;
        let size = self.size;
        if size > 0 {
            let slots = &mut self.slots[..=size];
            // TS: :60-61 the last element moves to the root; :64-92 bubble down (strict `<`,
            // left before right), as a hole walk (same positions as TS's swaps).
            let moving = slots[size];
            let mut i = 0usize;
            loop {
                let left = (i << 1) + 1;
                let right = left + 1;
                let mut smallest = i;
                let mut sp = moving.pri;
                if left < size && slots[left].pri < sp {
                    smallest = left;
                    sp = slots[left].pri;
                }
                if right < size && slots[right].pri < sp {
                    smallest = right;
                }
                if smallest == i {
                    break;
                }
                slots[i] = slots[smallest];
                i = smallest;
            }
            slots[i] = moving;
        }
        result
    }

    /// `isEmpty()` (`:97-99`).
    pub fn is_empty(&self) -> bool {
        self.size == 0
    }

    /// `clear()` (`:101-103`).
    pub fn clear(&mut self) {
        self.size = 0;
    }
}

/// `BucketQueue` (`:107-179`): O(1) integer-priority queue, LIFO within a bucket.
#[derive(Debug, Default, Clone)]
pub struct BucketQueue {
    pub buckets: Vec<Vec<i32>>,
    pub bucket_sizes: Vec<i32>,
    pub bucket_stamp: Vec<u32>,
    pub stamp: u32,
    /// `minBucket` (int; may go negative, see [`BucketQueue::push`]).
    pub min_bucket: i32,
    pub max_bucket: i32,
    /// `size` (int; counts pushes into negative buckets too, which TS loses).
    pub size: usize,
}

impl BucketQueue {
    /// `new BucketQueue(maxPriority)` (`:116-123`).
    pub fn new(max_priority: u32) -> Self {
        let max_bucket = max_priority as i32 + 1; // int: TS maxPriority + 1
        let n = max_bucket.max(0) as usize;
        BucketQueue {
            buckets: vec![Vec::new(); n],
            bucket_sizes: vec![0; n],
            bucket_stamp: vec![0; n],
            stamp: 0,
            min_bucket: max_bucket,
            max_bucket,
            size: 0,
        }
    }

    /// `push(node, priority)` (`:125-149`). `bucket = Math.min(priority | 0, maxBucket - 1)`.
    ///
    /// JS semantics kept for a negative bucket (`priority < 0`): `this.buckets[-k]` is a plain
    /// array property and typed-array OOB reads/writes are `undefined`/ignored, so the node is
    /// stored nowhere a pop can reach, `size` still increments and `minBucket` drops to the
    /// negative value (`pop` then walks up past it). No caller pushes a negative priority
    /// (A* f = g + h ≥ 0); this only keeps the port total.
    pub fn push(&mut self, node: i32, priority: f64) {
        let bucket = of_det::js::to_int32(priority).min(self.max_bucket - 1); // TS: :126 (ints)
        if bucket >= 0 {
            let b = bucket as usize; // in-range: 0 <= bucket <= maxBucket - 1
            let size = if self.bucket_stamp[b] == self.stamp { self.bucket_sizes[b] } else { 0 };
            let su = size as usize; // int: bucket fill, >= 0
            let bv = &mut self.buckets[b];
            if su >= bv.len() {
                // TS: :128-139 lazily created Int32Array(64), doubled when full
                let cap = (bv.len() * 2).max(64);
                bv.resize(cap, 0);
            }
            bv[su] = node;
            self.bucket_sizes[b] = size + 1;
            self.bucket_stamp[b] = self.stamp;
        }
        self.size += 1;
        if bucket < self.min_bucket {
            self.min_bucket = bucket;
        }
    }

    /// `pop()` (`:151-164`): `-1` when nothing is reachable.
    pub fn pop(&mut self) -> i32 {
        while self.min_bucket < self.max_bucket {
            // TS: :153 `bucketStamp[minBucket]` is `undefined` for a negative index
            if self.min_bucket >= 0 {
                let b = self.min_bucket as usize; // in-range: 0 <= min_bucket < max_bucket
                if self.bucket_stamp[b] == self.stamp {
                    let size = self.bucket_sizes[b];
                    if size > 0 {
                        self.bucket_sizes[b] -= 1;
                        self.size -= 1;
                        return self.buckets[b][(size - 1) as usize]; // in-range: size-1 < fill
                    }
                }
            }
            self.min_bucket += 1;
        }
        -1
    }

    /// `isEmpty()` (`:166-168`).
    pub fn is_empty(&self) -> bool {
        self.size == 0
    }

    /// `clear()` (`:170-178`): new stamp; at the u32 wrap (`stamp > 0xffffffff`) the stamps are
    /// zeroed and the stamp restarts at 1.
    pub fn clear(&mut self) {
        if self.stamp == u32::MAX {
            self.bucket_stamp.fill(0);
            self.stamp = 1;
        } else {
            self.stamp += 1;
        }
        self.min_bucket = self.max_bucket;
        self.size = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn min_heap_pops_in_f32_order_with_position_ties() {
        let mut h = MinHeap::new(2);
        // 1.00000001 and 1.00000002 both round to f32 1.0 (ties broken by heap position).
        for (n, p) in [(1, 5.0), (2, 1.000_000_01), (3, 1.000_000_02), (4, 0.5), (5, 7.0), (6, 1.0)] {
            h.push(n, p);
        }
        let mut out = Vec::new();
        while !h.is_empty() {
            out.push(h.pop());
        }
        assert_eq!(out[0], 4);
        assert_eq!(*out.last().expect("non-empty"), 5);
        assert_eq!(out.len(), 6);
        // f32 store: 1.00000001 rounds to 1.0 (f32 spacing at 1 is 1.19e-7); 1.0000001 does not.
        assert_eq!(1.000_000_01_f64 as f32, 1.0_f32);
        assert_ne!(1.000_000_1_f64 as f32, 1.0_f32);
        // equal stored priorities 1.0 for nodes 2, 3, 6: popped by heap position after 4.
        assert_eq!(
            out[1..4].iter().copied().collect::<std::collections::BTreeSet<_>>(),
            [2, 3, 6].into_iter().collect()
        );
    }

    #[test]
    fn min_heap_matches_reference_sequence() {
        // Hand-traced against PriorityQueue.ts:19-95 (bubble-up `<=` break, bubble-down strict `<`).
        let mut h = MinHeap::new(8);
        for (n, p) in [(10, 3.0), (11, 3.0), (12, 1.0), (13, 3.0), (14, 2.0)] {
            h.push(n, p);
        }
        // heap after pushes: [12(1), 14(2), 10(3), 13(3), 11(3)]
        assert_eq!(h.nodes()[..5], [12, 14, 10, 13, 11]);
        assert_eq!(h.pop(), 12); // [11,14,10,13] -> bubble 11 below 14: [14,11,10,13]
        assert_eq!(h.nodes()[..4], [14, 11, 10, 13]);
        assert_eq!(h.pop(), 14); // [13,11,10] -> 13 vs 11(3)/10(3): no strict improvement
        assert_eq!(h.nodes()[..3], [13, 11, 10]);
        assert_eq!((h.pop(), h.pop(), h.pop()), (13, 10, 11));
        assert!(h.is_empty());
    }

    /// TS's literal swap-per-level heap (`PriorityQueue.ts:19-95`), the reference for the
    /// hole-walk port above.
    struct SwapHeap {
        heap: Vec<i32>,
        pri: Vec<f32>,
    }
    impl SwapHeap {
        fn push(&mut self, n: i32, p: f64) {
            self.heap.push(n);
            self.pri.push(p as f32);
            let mut i = self.heap.len() - 1;
            while i > 0 {
                let parent = (i - 1) >> 1;
                if self.pri[parent] <= self.pri[i] {
                    break;
                }
                self.heap.swap(parent, i);
                self.pri.swap(parent, i);
                i = parent;
            }
        }
        fn pop(&mut self) -> i32 {
            let r = self.heap[0];
            let last = self.heap.len() - 1;
            self.heap[0] = self.heap[last];
            self.pri[0] = self.pri[last];
            self.heap.pop();
            self.pri.pop();
            let size = self.heap.len();
            let mut i = 0;
            loop {
                let (l, rr) = (2 * i + 1, 2 * i + 2);
                let mut s = i;
                if l < size && self.pri[l] < self.pri[s] {
                    s = l;
                }
                if rr < size && self.pri[rr] < self.pri[s] {
                    s = rr;
                }
                if s == i {
                    break;
                }
                self.heap.swap(s, i);
                self.pri.swap(s, i);
                i = s;
            }
            r
        }
    }

    /// P3 (t066) guard: the hole-walk heap equals the swap heap, array for array, on random
    /// push/pop mixes with many f32 ties (which is where pop order depends on positions).
    #[test]
    fn hole_heap_equals_swap_heap() {
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for case in 0..400 {
            let mut a = MinHeap::new((case % 5) as usize);
            let mut b = SwapHeap { heap: Vec::new(), pri: Vec::new() };
            let distinct = 1 + next() % if case % 2 == 0 { 8 } else { 100_000 };
            for step in 0..2000 {
                if b.heap.is_empty() || next() % 3 != 0 {
                    let n = step;
                    let p = (next() % distinct) as f64 * 0.37 + 1e-9 * (next() % 3) as f64;
                    a.push(n, p);
                    b.push(n, p);
                } else {
                    assert_eq!(a.pop(), b.pop(), "case {case} step {step}");
                }
                assert_eq!(a.nodes(), b.heap, "case {case} step {step}");
                assert_eq!(a.priorities(), b.pri, "case {case} step {step}");
            }
        }
    }

    #[test]
    fn bucket_queue_lifo_within_bucket_and_clear() {
        let mut q = BucketQueue::new(10);
        q.push(1, 3.7); // |0 -> 3
        q.push(2, 3.2);
        q.push(3, 1.0);
        q.push(4, 99.0); // clamped to bucket 10
        assert_eq!((q.pop(), q.pop(), q.pop(), q.pop(), q.pop()), (3, 2, 1, 4, -1));
        q.push(5, 2.0);
        q.clear();
        assert!(q.is_empty());
        assert_eq!(q.pop(), -1); // stale bucket from before clear is invisible
        q.push(6, f64::NAN); // NaN | 0 == 0
        assert_eq!(q.pop(), 6);
    }

    #[test]
    fn bucket_queue_negative_priority_is_lost_like_js() {
        let mut q = BucketQueue::new(4);
        q.push(7, -2.0);
        assert!(!q.is_empty());
        q.push(8, 1.0);
        assert_eq!(q.pop(), 8);
        assert_eq!(q.pop(), -1);
        assert!(!q.is_empty()); // size still counts the lost node, as in TS
    }

    #[test]
    fn bucket_queue_stamp_wrap() {
        let mut q = BucketQueue::new(3);
        q.stamp = u32::MAX;
        q.push(1, 1.0);
        q.clear();
        assert_eq!(q.stamp, 1);
        assert!(q.bucket_stamp.iter().all(|&s| s == 0));
        q.push(2, 2.0);
        assert_eq!(q.pop(), 2);
    }
}
