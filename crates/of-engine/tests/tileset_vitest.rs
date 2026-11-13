//! Port of upstream `tests/TileSet.test.ts` (FILE_MAP §4, P1 T0), run against both
//! layouts of the TileSet port: `OrderedTileSet` and `PlayerTileOrder` + `TileListHdr`.
//! Plus three cases pinned from the real `TileSet.ts` @5b75bd3 executed under node
//! v24.21.0 (`--experimental-strip-types`, import lines removed) on 2026-11-05, covering
//! behaviour the vitest file does not: clear during iteration and churn during iteration.

use of_det::{JsSet, Prng};
use of_engine::tileset::{NO_POS, OrderedTileSet, PlayerTileOrder, TileCursor, TileListHdr};

/// The operations the vitest file uses, over either layout.
trait Ts {
    fn add(&mut self, t: u32) -> bool;
    fn delete(&mut self, t: u32) -> bool;
    fn has(&self, t: u32) -> bool;
    fn size(&self) -> usize;
    fn clear(&mut self);
    fn next_live(&self, c: &mut TileCursor) -> Option<u32>;
    fn to_vec(&self) -> Vec<u32>;
}

impl Ts for OrderedTileSet {
    fn add(&mut self, t: u32) -> bool {
        OrderedTileSet::add(self, t)
    }
    fn delete(&mut self, t: u32) -> bool {
        OrderedTileSet::delete(self, t)
    }
    fn has(&self, t: u32) -> bool {
        OrderedTileSet::has(self, t)
    }
    fn size(&self) -> usize {
        OrderedTileSet::size(self)
    }
    fn clear(&mut self) {
        OrderedTileSet::clear(self)
    }
    fn next_live(&self, c: &mut TileCursor) -> Option<u32> {
        OrderedTileSet::next_live(self, c)
    }
    fn to_vec(&self) -> Vec<u32> {
        OrderedTileSet::to_vec(self)
    }
}

/// One player's `_tiles` in the State-level layout. A second, foreign list shares the
/// position array to prove the lists cannot see each other.
struct PSet {
    ord: PlayerTileOrder,
    mine: TileListHdr,
    other: TileListHdr,
    land: Vec<u32>,
}

const PSET_TILES: u32 = 4096;

impl PSet {
    fn new() -> Self {
        // Every 7th tile is water (not in the land index); land index != tile ref.
        let mut land = vec![NO_POS; PSET_TILES as usize];
        let mut n = 0;
        for t in 0..PSET_TILES {
            if t % 7 != 6 {
                land[t as usize] = n;
                n += 1;
            }
        }
        let mut s = PSet {
            ord: PlayerTileOrder::new(n as usize),
            mine: TileListHdr::default(),
            other: TileListHdr::default(),
            land,
        };
        // the foreign list owns some tiles no test uses (>= 3000)
        for t in [3000, 3001, 3003] {
            assert!(s.ord.add(&mut s.other, &s.land, t));
        }
        s
    }
    /// Map a test value onto a land tile (values in the vitest file are < 2200).
    fn tile(v: u32) -> u32 {
        assert!(v < 2500, "test value {v} out of PSet range");
        v + v / 6 // skip every 7th tile (water)
    }
    fn value(t: u32) -> u32 {
        t - t / 7
    }
}

impl Ts for PSet {
    fn add(&mut self, v: u32) -> bool {
        self.ord.add(&mut self.mine, &self.land, Self::tile(v))
    }
    fn delete(&mut self, v: u32) -> bool {
        self.ord.delete(&mut self.mine, &self.land, Self::tile(v))
    }
    fn has(&self, v: u32) -> bool {
        self.ord.has(&self.mine, &self.land, Self::tile(v))
    }
    fn size(&self) -> usize {
        self.mine.size()
    }
    fn clear(&mut self) {
        self.ord.clear(&mut self.mine, &self.land)
    }
    fn next_live(&self, c: &mut TileCursor) -> Option<u32> {
        self.mine.next_live(c).map(Self::value)
    }
    fn to_vec(&self) -> Vec<u32> {
        self.mine.iter().map(Self::value).collect()
    }
}

fn from<S: Ts>(mut s: S, vals: &[u32]) -> S {
    for &v in vals {
        s.add(v);
    }
    s
}

/// `forEach` / `for..of` with a body that may mutate (re-reads the length every step).
fn each<S: Ts>(s: &mut S, mut body: impl FnMut(&mut S, u32)) -> Vec<u32> {
    let mut c = TileCursor::default();
    let mut seen = vec![];
    while let Some(t) = s.next_live(&mut c) {
        seen.push(t);
        body(s, t);
        assert!(seen.len() < 100_000);
    }
    seen
}

fn suite<S: Ts>(new: impl Fn() -> S) {
    // "adds, reports membership and size"
    let mut s = new();
    assert_eq!(s.size(), 0);
    assert!(!s.has(5));
    s.add(5);
    s.add(9);
    s.add(5); // duplicate
    assert_eq!(s.size(), 2);
    assert!(s.has(5) && s.has(9) && !s.has(6));

    // "deletes and reports whether the value was present"
    let mut s = from(new(), &[1, 2, 3]);
    assert!(s.delete(2));
    assert!(!s.delete(2));
    assert!(!s.delete(99));
    assert_eq!(s.size(), 2);
    assert!(!s.has(2));
    assert_eq!(s.to_vec(), [1, 3]);

    // "iterates in insertion order across all iteration surfaces"
    let values = [42, 7, 1000, 0, 13]; // vitest uses 100000; PSet maps values < 2500 (large refs: own test)
    let mut s = from(new(), &values);
    assert_eq!(s.to_vec(), values);
    assert_eq!(each(&mut s, |_, _| {}), values);

    // "moves a value to the end on delete + re-add, matching Set"
    let mut s = from(new(), &[1, 2, 3]);
    s.delete(1);
    s.add(1);
    assert_eq!(s.to_vec(), [2, 3, 1]);

    // "re-adding an existing value does not change its position"
    let mut s = from(new(), &[1, 2, 3]);
    s.add(1);
    assert_eq!(s.to_vec(), [1, 2, 3]);

    // "visits entries added during forEach, matching Set"
    let mut s = from(new(), &[1, 2]);
    let seen = each(&mut s, |s, t| {
        if t == 1 {
            s.add(3);
        }
    });
    assert_eq!(seen, [1, 2, 3]);

    // "skips entries deleted during forEach, matching Set"
    let mut s = from(new(), &[1, 2, 3]);
    let seen = each(&mut s, |s, t| {
        if t == 1 {
            s.delete(3);
        }
    });
    assert_eq!(seen, [1, 2]);

    // "supports deleting the current entry during iteration"
    let mut s = from(new(), &[1, 2, 3]);
    let seen = each(&mut s, |s, t| {
        s.delete(t);
    });
    assert_eq!(seen, [1, 2, 3]);
    assert_eq!(s.size(), 0);

    // "preserves order through tombstone compaction" (1000..; values shifted below 2200)
    let mut s = new();
    for i in 0..1000 {
        s.add(i);
    }
    for i in 0..1000 {
        if i % 3 != 0 {
            s.delete(i);
        }
    }
    for i in 2000..2100 {
        s.add(i);
    }
    let expected: Vec<u32> = (0..1000).filter(|i| i % 3 == 0).chain(2000..2100).collect();
    assert_eq!(s.to_vec(), expected);
    assert_eq!(s.size(), expected.len());
    for &v in &expected {
        assert!(s.has(v));
    }
    assert!(!s.has(1));

    // "clear empties the set"
    let mut s = from(new(), &[1, 2, 3]);
    s.clear();
    assert_eq!(s.size(), 0);
    assert!(!s.has(1));
    assert!(s.to_vec().is_empty());
    s.add(7);
    assert_eq!(s.to_vec(), [7]);

    // "matches native Set behavior on a randomized operation sequence"
    // (reference: of_det::JsSet, the vector-checked JS Set port)
    let mut random = Prng::new(12345.0);
    let mut ts = new();
    let mut reference: JsSet<u32> = JsSet::new();
    for op in 0..20000 {
        let value = random.next_int(0.0, 500.0) as u32;
        if random.chance(3.0) {
            assert_eq!(ts.delete(value), reference.remove(&value));
        } else {
            ts.add(value);
            reference.insert(value);
        }
        if op % 500 == 0 {
            assert_eq!(ts.size(), reference.len());
            assert_eq!(ts.to_vec(), reference.iter().copied().collect::<Vec<_>>());
        }
    }
    assert_eq!(ts.to_vec(), reference.iter().copied().collect::<Vec<_>>());
    for v in 0..500 {
        assert_eq!(ts.has(v), reference.contains(&v));
    }

    // --- pinned from TileSet.ts under node (see the file header) ---
    // clearDuringForEach: {"seen":[1,10],"final":[9,10]}  (differs from a JS Set!)
    let mut s = from(new(), &[1, 2, 3]);
    let mut first = true;
    let seen = each(&mut s, |s, _| {
        if first {
            first = false;
            s.clear();
            s.add(9);
            s.add(10);
        }
    });
    assert_eq!((seen, s.to_vec()), (vec![1, 10], vec![9, 10]));
    // clearDuringForOf: {"seen":[1,2,9,10],"final":[7,8,9,10]}
    let mut s = from(new(), &[1, 2, 3]);
    let mut first = true;
    let seen = each(&mut s, |s, t| {
        if t == 2 && first {
            first = false;
            s.clear();
            for v in [7, 8, 9, 10] {
                s.add(v);
            }
        }
    });
    assert_eq!((seen, s.to_vec()), (vec![1, 2, 9, 10], vec![7, 8, 9, 10]));
    // churn: seen [0..5, 300..309, 5], final [0..4, 300..309, 5]
    let mut s = new();
    for i in 0..200 {
        s.add(i);
    }
    let mut first = true;
    let seen = each(&mut s, |s, t| {
        if t == 5 && first {
            first = false;
            for i in 6..200 {
                s.delete(i);
            }
            for i in 300..310 {
                s.add(i);
            }
            s.delete(5);
            s.add(5);
        }
    });
    let want_seen: Vec<u32> = (0..=5).chain(300..310).chain([5]).collect();
    let want_final: Vec<u32> = (0..5).chain(300..310).chain([5]).collect();
    assert_eq!((seen, s.to_vec()), (want_seen, want_final));
}

#[test]
fn tileset_vitest_ordered_tile_set() {
    suite(OrderedTileSet::new);
}

#[test]
fn tileset_vitest_player_tile_order() {
    suite(PSet::new);
}

/// "handles large tile refs (up to the 65535x65535 map bound)": OrderedTileSet only
/// (a PlayerTileOrder is sized by the map's land tiles).
#[test]
fn tileset_vitest_large_refs() {
    let big = 65535u32 * 65535 - 1;
    let s: OrderedTileSet = [big, 0, big - 1].into_iter().collect();
    assert!(s.has(big));
    assert_eq!(s.to_vec(), [big, 0, big - 1]);
}

/// Compaction (end-of-tick maintenance) keeps order and membership in both layouts.
#[test]
fn maintenance_compaction_preserves_order() {
    let mut a: OrderedTileSet = (0..300).collect();
    let mut p = from(PSet::new(), &(0..300).collect::<Vec<_>>());
    for i in (0..300).filter(|i| i % 4 != 0) {
        a.delete(i);
        Ts::delete(&mut p, i);
    }
    let before = a.to_vec();
    assert!(a.maybe_compact());
    assert!(p.ord.maybe_compact(&mut p.mine, &p.land));
    assert_eq!(a.dense_len(), before.len());
    assert_eq!(p.mine.dense_len(), before.len());
    assert_eq!(a.to_vec(), before);
    assert_eq!(Ts::to_vec(&p), before);
    for v in 0..300 {
        assert_eq!(a.has(v), v % 4 == 0);
        assert_eq!(Ts::has(&p, v), v % 4 == 0);
    }
    // the foreign list was untouched
    assert_eq!(p.other.to_vec(), [3000, 3001, 3003]);
    assert!(p.ord.has(&p.other, &p.land, 3003));
}
