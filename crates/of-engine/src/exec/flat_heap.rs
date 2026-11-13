//! `FlatBinaryHeap` (`engine/src/execution/utils/FlatBinaryHeap.ts`). Owner: **P6** (wrapper);
//! the implementation is `of_det::heap::FlatBinaryHeap` (f32 store / f64 compare, VECTORS
//! `order/FlatBinaryHeap`).

pub use of_det::heap::FlatBinaryHeap;

/// Port of `tests/FlatBinaryHeap.test.ts` (FILE_MAP §4 P6, T0).
#[cfg(test)]
mod tests {
    use super::FlatBinaryHeap;

    #[test]
    fn dequeues_tiles_in_ascending_priority_order() {
        let mut heap = FlatBinaryHeap::new(1024);
        for (tile, pri) in [(100, 5.0), (200, 1.0), (300, 3.0), (400, 2.0), (500, 4.0)] {
            heap.enqueue(tile, pri);
        }
        assert_eq!(heap.size(), 5);
        for want in [200, 400, 300, 500, 100] {
            assert_eq!(heap.dequeue(), Ok(want));
        }
        assert_eq!(heap.size(), 0);
    }

    /// TS throws `heap empty`; the port returns `Err` and `AttackExec` raises the same
    /// message as a `TsThrow` (`exec/attack.rs`).
    #[test]
    fn errors_when_dequeuing_an_empty_heap() {
        let mut heap = FlatBinaryHeap::new(1024);
        assert!(heap.dequeue().is_err());
    }

    #[test]
    fn clear_empties_the_heap_without_breaking_subsequent_use() {
        let mut heap = FlatBinaryHeap::new(1024);
        heap.enqueue(1, 1.0);
        heap.enqueue(2, 2.0);
        heap.clear();
        assert_eq!(heap.size(), 0);
        heap.enqueue(3, 3.0);
        assert_eq!(heap.dequeue(), Ok(3));
    }

    #[test]
    fn grows_past_its_initial_capacity_and_stays_ordered() {
        let mut heap = FlatBinaryHeap::new(4);
        let n = 1000u32;
        for i in 0..n {
            heap.enqueue(i, f64::from(n - i));
        }
        assert_eq!(heap.size(), n as usize);
        for i in 0..n {
            assert_eq!(heap.dequeue(), Ok(n - 1 - i));
        }
    }
}
