//! `ComponentCheckTransformer` (`:1-35`). Owner: **P4b**. Reads the **live** components.
//!
//! The HPA chain's `componentCheckFn = (t) => graph.getComponentId(t)` (`PathFinder.ts:60`)
//! reads the one live `ConnectedComponents` every graph shares (`AbstractGraph.ts:142-144`,
//! `this._waterComponents?.getComponentId(tile) ?? 0`), so an old chain sees the current
//! components (ARCHITECTURE §3.8): the Rust chain passes `state.water.components`.

use crate::ids::TileRef;
use crate::path::types::{Src, delegate_src};
use crate::water::components::Components;

/// `graph.getComponentId(t)` (`AbstractGraph.ts:142-144`): `?? 0` without components.
#[inline]
pub fn component_of(components: Option<&Components>, t: TileRef) -> u32 {
    components.map_or(0, |c| c.get_component_id(t))
}

/// The sources in `to`'s component (`validSources`, `:18-24`); `None` when there is none.
pub fn filter_sources(components: &Components, from: &[TileRef], to: TileRef) -> Option<Vec<TileRef>> {
    let to_component = components.get_component_id(to);
    let valid: Vec<TileRef> =
        from.iter().copied().filter(|&f| components.get_component_id(f) == to_component).collect();
    if valid.is_empty() { None } else { Some(valid) }
}

/// `findPath(from, to)` (`:17-34`): fail fast unless a source shares `to`'s component, then
/// delegate with the valid sources (a lone one unwrapped to a scalar).
pub fn find_path(
    get_component: impl Fn(TileRef) -> u32,
    from: Src<'_>,
    to: TileRef,
    inner: impl FnOnce(Src<'_>, TileRef) -> Option<Vec<TileRef>>,
) -> Option<Vec<TileRef>> {
    let to_component = get_component(to);
    let mut one = [0];
    let from_array = from.as_slice(&mut one);
    let valid_sources: Vec<TileRef> =
        from_array.iter().copied().filter(|&f| get_component(f) == to_component).collect();
    if valid_sources.is_empty() {
        return None;
    }
    inner(delegate_src(&valid_sources), to)
}
