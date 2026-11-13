//! `PathFinderBuilder` (`pathfinding/PathFinderBuilder.ts`). Owner: **P4b**.
//!
//! TS composes transformer objects at runtime (`create(core).wrap(..).build()`, each
//! `wrap` wrapping the previous finder, `:21-35`; the `DebugSpan` wrapper is view-only).
//! The port composes them statically per [`crate::path::pathfinder::ChainKind`]
//! (`buildWaterChain`, `PathFinder.ts:47-66`). A call enters at the outermost transformer:
//! - `Simple`: MiniMap → ShoreCoercing → AStarWater (core, `sc.astar_water`)
//! - `Hpa`: MiniMap → ShoreCoercing → Smoothing (`sc.smoothing_local`) → ComponentCheck
//!   (live components) → HPA (P4a, current graph)
//! - rail: MiniMap → AStarRail (`sc.astar_rail`, `PathFinder.ts:173-183`).

use crate::ctx::Rcx;
use crate::ids::TileRef;
use crate::path::MapRead;
use crate::path::astar_water::{self, AStarWaterConfig};
use crate::path::pathfinder::ChainKind;
use crate::path::transform::{component_check, minimap, shore, smoothing};
use crate::path::types::Src;
use crate::scratch::path::{AStarRailScratch, AStarWaterBoundedScratch, AStarWaterScratch};
use crate::state::State;
use crate::water::abstract_graph::SegVisibility;

/// Run a water chain of `kind` (`chain.findPath(from, to)`); `from` per [`Src::from_slice`].
pub fn water_chain_find_path(
    rcx: &mut Rcx,
    seg_vis: &mut SegVisibility,
    kind: ChainKind,
    from: &[TileRef],
    to: TileRef,
) -> Option<Vec<TileRef>> {
    water_chain_find_path_src(rcx, seg_vis, kind, Src::from_slice(from), to)
}

/// [`water_chain_find_path`] with an explicit scalar/array source.
pub fn water_chain_find_path_src(
    rcx: &mut Rcx,
    seg_vis: &mut SegVisibility,
    kind: ChainKind,
    from: Src<'_>,
    to: TileRef,
) -> Option<Vec<TileRef>> {
    let st: &State = rcx.st;
    match kind {
        ChainKind::Simple => simple_water_chain(&st.map, &st.mini, &mut rcx.sc.astar_water, from, to),
        ChainKind::Hpa => {
            // The smoother's scratch is lent out while the HPA (which takes the whole
            // `Rcx`) runs underneath it; scratch is not state, so losing it to an unwind
            // would only cost a re-allocation.
            let mut smoothing_sc = std::mem::take(&mut rcx.sc.smoothing_local);
            let components = st.water.components.as_deref();
            let r = hpa_water_chain(
                &st.map,
                &st.mini,
                &mut smoothing_sc,
                |t| component_check::component_of(components, t),
                from,
                to,
                |f, t| {
                    let mut one = [0];
                    // P4a's HPA takes a slice: a 1-element slice is the TS scalar
                    // (`AStar.WaterHierarchical.ts:71-75`; the component check above has
                    // already unwrapped a lone source, so a 1-element array cannot reach it).
                    let fs = f.as_slice(&mut one);
                    crate::water::hpa::find_path(rcx, seg_vis, fs, t)
                },
            );
            rcx.sc.smoothing_local = smoothing_sc;
            r
        }
    }
}

/// The simple chain (`PathFinder.ts:52-58`) over any map: MiniMap → ShoreCoercing → AStarWater.
pub fn simple_water_chain<M: MapRead>(
    map: &M,
    mini: &M,
    sc: &mut AStarWaterScratch,
    from: Src<'_>,
    to: TileRef,
) -> Option<Vec<TileRef>> {
    minimap::find_path(map, mini, from, to, |f, t| {
        shore::find_path(mini, f, t, |f, t| {
            let mut one = [0];
            astar_water::find_path_m(sc, mini, AStarWaterConfig::default(), f.as_slice(&mut one), t)
        })
    })
}

/// The HPA chain (`PathFinder.ts:60-66`) over any map, with the component lookup and the
/// HPA core supplied: MiniMap → ShoreCoercing → Smoothing → ComponentCheck → `hpa`.
pub fn hpa_water_chain<M: MapRead>(
    map: &M,
    mini: &M,
    smoothing_sc: &mut AStarWaterBoundedScratch,
    get_component: impl Fn(TileRef) -> u32,
    from: Src<'_>,
    to: TileRef,
    hpa: impl FnOnce(Src<'_>, TileRef) -> Option<Vec<TileRef>>,
) -> Option<Vec<TileRef>> {
    minimap::find_path(map, mini, from, to, |f, t| {
        shore::find_path(mini, f, t, |f, t| {
            smoothing::find_path(smoothing_sc, mini, f, t, |f, t| component_check::find_path(get_component, f, t, hpa))
        })
    })
}

/// Run the rail chain (`sharedRailChain`, `PathFinder.ts:173-183`); `from` per
/// [`Src::from_slice`] (the rail chain is scalar/array-insensitive).
pub fn rail_chain_find_path(rcx: &mut Rcx, from: &[TileRef], to: TileRef) -> Option<Vec<TileRef>> {
    let st: &State = rcx.st;
    rail_chain(&st.map, &st.mini, &mut rcx.sc.astar_rail, Src::from_slice(from), to)
}

/// The rail chain over any map: MiniMap → AStarRail.
pub fn rail_chain<M: MapRead>(
    map: &M,
    mini: &M,
    sc: &mut AStarRailScratch,
    from: Src<'_>,
    to: TileRef,
) -> Option<Vec<TileRef>> {
    minimap::find_path(map, mini, from, to, |f, t| {
        let mut one = [0];
        crate::path::astar_rail::find_path_m(sc, mini, f.as_slice(&mut one), t)
    })
}
