//! `#[derive(StateClone)]`: `clone_from` forwards field by field, so a pooled destination
//! keeps every buffer (ARCHITECTURE §8.1; FILE_MAP §4 P1 mandatory design test
//! "`StateClone::clone_from` reuses capacity").

use of_engine::StateClone;
use of_engine::jsarr::JsArr;
use of_engine::jsorder::{JsMap, SmallJsSet};
use of_engine::tileset::OrderedTileSet;
use std::sync::Arc;

#[derive(StateClone, Debug, PartialEq)]
struct Inner {
    tiles: Vec<u32>,
    name: String,
}

#[derive(StateClone, Debug, PartialEq)]
struct Pair(Vec<u64>, u8);

#[derive(StateClone, Debug, PartialEq)]
enum Mode {
    Idle,
    Walking { path: Vec<u32>, step: u32 },
    Holding(Vec<u16>),
}

#[derive(StateClone, Debug)]
struct Outer<T> {
    inner: Inner,
    many: Vec<Inner>,
    pair: Pair,
    mode: Mode,
    generic: Vec<T>,
    map: JsMap<u32, f64>,
    small: SmallJsSet<u32, 4>,
    arr: JsArr<u32>,
    border: OrderedTileSet,
    shared: Arc<[u8]>,
    opt: Option<Inner>,
}

fn make(n: u32) -> Outer<u64> {
    let mut map = JsMap::new();
    let mut small = SmallJsSet::new();
    let mut arr = JsArr::new();
    for i in 0..n {
        map.insert(i, i as f64 * 0.5);
        small.insert(i % 3);
        arr.push(i);
    }
    Outer {
        inner: Inner { tiles: (0..n).collect(), name: format!("p{n}") },
        many: (0..n / 10).map(|i| Inner { tiles: vec![i; 8], name: "x".into() }).collect(),
        pair: Pair((0..n as u64).collect(), 3),
        mode: Mode::Walking { path: (0..n).collect(), step: 1 },
        generic: (0..n as u64).collect(),
        map,
        small,
        arr,
        border: (0..n).collect(),
        shared: Arc::from(vec![1u8, 2, 3]),
        opt: Some(Inner { tiles: vec![9; n as usize], name: "o".into() }),
    }
}

#[test]
fn clone_from_reuses_capacity() {
    let src = make(100);
    let mut dst = make(1000); // a pooled destination with larger buffers
    let p_inner = dst.inner.tiles.as_ptr();
    let p_many = dst.many.as_ptr();
    let p_many0 = dst.many[0].tiles.as_ptr();
    let p_pair = dst.pair.0.as_ptr();
    let p_generic = dst.generic.as_ptr();
    let p_opt = dst.opt.as_ref().map(|o| o.tiles.as_ptr());
    let p_path = match &dst.mode {
        Mode::Walking { path, .. } => path.as_ptr(),
        _ => unreachable!(),
    };
    let cap_inner = dst.inner.tiles.capacity();

    dst.clone_from(&src);

    // every buffer kept its allocation
    assert_eq!(dst.inner.tiles.as_ptr(), p_inner);
    assert_eq!(dst.inner.tiles.capacity(), cap_inner);
    assert_eq!(dst.many.as_ptr(), p_many);
    assert_eq!(dst.many[0].tiles.as_ptr(), p_many0, "element-wise clone_from on the common prefix");
    assert_eq!(dst.pair.0.as_ptr(), p_pair);
    assert_eq!(dst.generic.as_ptr(), p_generic);
    assert_eq!(dst.opt.as_ref().map(|o| o.tiles.as_ptr()), p_opt);
    match &dst.mode {
        Mode::Walking { path, .. } => assert_eq!(path.as_ptr(), p_path, "same enum variant: field-wise"),
        _ => panic!("variant"),
    }
    // and the content equals the source
    assert_eq!(dst.inner, src.inner);
    assert_eq!(dst.many, src.many);
    assert_eq!(dst.pair, src.pair);
    assert_eq!(dst.mode, src.mode);
    assert_eq!(dst.generic, src.generic);
    assert_eq!(dst.map.iter().collect::<Vec<_>>(), src.map.iter().collect::<Vec<_>>());
    assert_eq!(dst.small.iter().collect::<Vec<_>>(), src.small.iter().collect::<Vec<_>>());
    assert_eq!(dst.arr, src.arr);
    assert_eq!(dst.border.to_vec(), src.border.to_vec());
    assert!(Arc::ptr_eq(&dst.shared, &src.shared), "Arc data is shared, not copied");
    assert_eq!(dst.opt, src.opt);
}

#[test]
fn clone_equals_source_and_variant_change_falls_back() {
    let src = make(50);
    let c = src.clone();
    assert_eq!(c.inner, src.inner);
    assert_eq!(c.mode, src.mode);
    let mut m = Mode::Holding(vec![1, 2]);
    m.clone_from(&Mode::Idle);
    assert_eq!(m, Mode::Idle);
    m.clone_from(&Mode::Walking { path: vec![4], step: 2 });
    assert_eq!(m, Mode::Walking { path: vec![4], step: 2 });
}
