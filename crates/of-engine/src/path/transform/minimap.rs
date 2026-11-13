//! `MiniMapTransformer` (`:1-124`). Owner: **P4b**. Main ↔ minimap tile mapping; the path
//! upscale uses `Math.round` ×2 (`:85-86`, `of_det::js::round`, PORTING_GUIDE §14.1).

use crate::ids::TileRef;
use crate::map::GameMap;
use crate::path::MapRead;
use crate::path::types::{Src, delegate_src};
use of_det::js;

/// Main-map tile → minimap tile (`miniMap.ref(floor(x/2), floor(y/2))`, `:12-16`).
pub fn to_mini(map: &GameMap, mini: &GameMap, t: TileRef) -> TileRef {
    to_mini_m(map, mini, t)
}

/// [`to_mini`] over any [`MapRead`].
pub fn to_mini_m<M: MapRead>(map: &M, mini: &M, t: TileRef) -> TileRef {
    mini.ref_(js::floor(f64::from(map.x(t)) / 2.0), js::floor(f64::from(map.y(t)) / 2.0))
}

/// Minimap path → main-map path: `upscalePath` + `fixExtremes` with the scalar/array source
/// rule of `findPath` (`:34-56`).
pub fn upscale_path(map: &GameMap, mini: &GameMap, path: &[TileRef], from: &[TileRef], to: TileRef) -> Vec<TileRef> {
    let up = upscale_path_m(map, mini, path, 2.0);
    let src = src_ref(map, Src::from_slice(from), &up);
    fix_extremes(up, to, src)
}

/// `findPath(from, to)` (`:8-56`) of the transformer wrapping `inner` (which runs on the
/// minimap).
pub fn find_path<M: MapRead>(
    map: &M,
    mini: &M,
    from: Src<'_>,
    to: TileRef,
    inner: impl FnOnce(Src<'_>, TileRef) -> Option<Vec<TileRef>>,
) -> Option<Vec<TileRef>> {
    // TS: :10-25 (`fromArray.map(..)` keeps the order; one source → scalar)
    let mut one = [0];
    let from_array = from.as_slice(&mut one);
    let mini_from_array: Vec<TileRef> = from_array.iter().map(|&f| to_mini_m(map, mini, f)).collect();
    let mini_to = to_mini_m(map, mini, to);

    // TS: :28-32
    let path = inner(delegate_src(&mini_from_array), mini_to)?;
    if path.is_empty() {
        return None;
    }
    let upscaled_path = upscale_path_m(map, mini, &path, 2.0);
    let src = src_ref(map, from, &upscaled_path);
    Some(fix_extremes(upscaled_path, to, src))
}

/// `srcRef` (`:36-54`): for an array, the source closest (Manhattan, first wins) to the
/// path start; for a scalar, the scalar.
fn src_ref<M: MapRead>(map: &M, from: Src<'_>, upscaled_path: &[TileRef]) -> Option<TileRef> {
    match from {
        Src::Many(arr) => {
            let mut src: Option<TileRef> = None;
            if let Some(&p0) = upscaled_path.first() {
                let start_x = f64::from(map.x(p0));
                let start_y = f64::from(map.y(p0));
                let mut min_dist = f64::INFINITY;
                for &f in arr {
                    let dist = js::abs(f64::from(map.x(f)) - start_x) + js::abs(f64::from(map.y(f)) - start_y);
                    if dist < min_dist {
                        min_dist = dist;
                        src = Some(f);
                    }
                }
            }
            src
        }
        Src::One(f) => Some(f),
    }
}

/// `upscalePath(path, scaleFactor = 2)` (`:62-97`).
pub fn upscale_path_m<M: MapRead>(map: &M, mini: &M, path: &[TileRef], scale_factor: f64) -> Vec<TileRef> {
    let mut smooth_path: Vec<TileRef> = Vec::new();
    for i in 0..path.len().saturating_sub(1) {
        let cur_x = f64::from(mini.x(path[i])) * scale_factor;
        let cur_y = f64::from(mini.y(path[i])) * scale_factor;
        let next_x = f64::from(mini.x(path[i + 1])) * scale_factor;
        let next_y = f64::from(mini.y(path[i + 1])) * scale_factor;

        smooth_path.push(map.ref_(cur_x, cur_y));

        let dx = next_x - cur_x;
        let dy = next_y - cur_y;
        let steps = js::max(js::abs(dx), js::abs(dy));

        let mut step = 1.0;
        while step < steps {
            smooth_path.push(map.ref_(js::round(cur_x + (dx * step) / steps), js::round(cur_y + (dy * step) / steps)));
            step += 1.0;
        }
    }
    if let Some(&last) = path.last() {
        smooth_path.push(map.ref_(f64::from(mini.x(last)) * scale_factor, f64::from(mini.y(last)) * scale_factor));
    }
    smooth_path
}

/// `fixExtremes(upscaled, dst, src?)` (`:99-123`).
pub fn fix_extremes(mut upscaled: Vec<TileRef>, dst: TileRef, src: Option<TileRef>) -> Vec<TileRef> {
    if let Some(src) = src {
        match upscaled.iter().position(|&t| t == src) {
            None => upscaled.insert(0, src),
            Some(0) => {}
            Some(src_index) => {
                upscaled.drain(..src_index);
            }
        }
    }
    match upscaled.iter().position(|&t| t == dst) {
        None => upscaled.push(dst),
        Some(dst_index) if dst_index != upscaled.len() - 1 => upscaled.truncate(dst_index + 1),
        Some(_) => {}
    }
    upscaled
}
