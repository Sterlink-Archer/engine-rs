//! The CS v1 sections P13 owns (FILE_MAP §2 P13 row): **G** (`GameImpl.snapshotState`,
//! `engine/src/game/GameImpl.ts:1652-1700`, plus `execTypes` and `unitGrid`), **T** (raw tile
//! state), **M** (`GameMapImpl.snapshot`, `engine-lib/src/game/GameMapImpl.ts:584-611`),
//! **W** (`WaterManager.snapshot` `engine/src/game/WaterManager.ts:95-108` and
//! `SharedWaterCache.snapshot`, `execution/nation/SharedWaterCache.ts:50-61`) and **S**
//! (`StatsImpl.snapshot`), each with its import (TS `restoreSnapshot`). Owner: **P13**.
//!
//! The rows (P U A D R E) are the owners' `Canon` impls; `G.rail` (CS `R.network`) is the
//! rail owner's (P5) and `S`'s content is `Stats`'s owner's (P2), reached through
//! [`crate::canon_or_missing!`].

use super::{
    Canon, CanonCx, CanonError, CanonMode, Cs, LoadKeys, Typed, UncanonCx, arr, field, int, opt_field, str_, tiles,
    tiles_of,
};
use crate::api::types::{Nation, PlayerInfo, UnitType};
use crate::arena::Arena;
use crate::ids::{SmallId, TileRef, UnitRef};
use crate::jsarr::JsArr;
use crate::jsorder::{JsMap, JsSet};
use crate::map::GameMap;
use crate::map::game_map::{DEFENSE_BONUS_BIT, FALLOUT_BIT, TileStateBuf};
use crate::map::unit_grid::{CellHdr, NIL, UnitLink};
use crate::state::unit::Unit;
use crate::state::{State, Winner};
use crate::water::components::{ComponentIds, Components};
use std::sync::Arc;

// =====================================================================================
// Export
// =====================================================================================

/// `GameImpl.snapshotState(w)` split the CS way (`conformance/src/canon.ts:131`): the G
/// fields, and the four versioned sub-records that CS moves to R.network, W and S.
pub struct GOut {
    /// G fields in `snapshotState` order (without `execTypes`, added after the drain).
    pub fields: Vec<(String, Cs)>,
    /// `rail` → R.network (P5's `RailNetwork` `Canon`).
    pub rail: Result<Cs, CanonError>,
    /// `water` → W.water.
    pub water: Cs,
    /// `sharedWaterCache` → W.sharedWaterCache.
    pub shared_water: Cs,
    /// `stats` → S (`None` in the K projection, where S is `null`).
    pub stats: Option<Result<Cs, CanonError>>,
}

/// G, visiting references in `snapshotState` object-literal order (SPEC §1.2.1 step 2):
/// that order fixes the `e<n>` keys of empty clusters met in `rail`.
pub fn g_section(cx: &mut CanonCx<'_>) -> GOut {
    let st = cx.st;
    let ex = cx.execs;
    let mut f: Vec<(String, Cs)> = Vec::with_capacity(28);
    fn put(f: &mut Vec<(String, Cs)>, k: &str, v: Cs) {
        f.push((k.to_owned(), v));
    }
    // TS: GameImpl.ts:1655-1656 (already registered by CanonCx::new, so keys = positions)
    let execs: Vec<Cs> = ex.live.iter().map(|&id| Cs::Int(cx.exec_key(id))).collect();
    let un_init: Vec<Cs> = st.uninit.iter().map(|&id| Cs::Int(cx.exec_key(id))).collect();
    put(&mut f, "ticks", Cs::Int(i64::from(st.ticks)));
    put(&mut f, "startTick", st.start_tick.map_or(Cs::Null, |t| Cs::Int(i64::from(t))));
    put(&mut f, "humans", st.humans.canon(cx));
    put(&mut f, "nations", st.nations.canon(cx));
    // TS: `[...this._players.values()].map((p) => w.player(p))`: Map order == smallID order
    // (ARCHITECTURE §3.1, `add_player` asserts new ids).
    put(&mut f, "players", Cs::Arr(st.players.iter().map(|p| Cs::Int(i64::from(p.small_id.0))).collect()));
    put(&mut f, "execs", Cs::Arr(execs));
    put(&mut f, "unInitExecs", Cs::Arr(un_init));
    let reqs = st.alliance_requests.as_slice().canon(cx);
    put(&mut f, "allianceRequests", reqs);
    put(&mut f, "nextPlayerID", Cs::Int(i64::from(st.next_player_id)));
    put(&mut f, "nextUnitID", Cs::Int(i64::from(st.next_unit_id)));
    put(&mut f, "nextAllianceID", Cs::Int(i64::from(st.next_alliance_id)));
    // TS: `[...this._unitMap.values()]`: insertion order == ascending id (§3.2).
    let units: Vec<Cs> = unit_map_order(st).into_iter().map(|u| cx.unit_key(u)).collect();
    put(&mut f, "units", Cs::Arr(units));
    put(&mut f, "planDrivenUnitIds", Cs::Arr(st.plan_driven_unit_ids.iter().map(|&i| Cs::Int(i64::from(i))).collect()));
    let grid = unit_grid_section(cx);
    put(&mut f, "unitGrid", grid);
    put(&mut f, "playerTeams", st.player_teams.canon(cx));
    put(&mut f, "botTeam", Cs::Str(st.bot_team.clone()));
    put(&mut f, "isPaused", Cs::Bool(st.is_paused));
    // TS: :1675-1680
    let winner = match &st.winner {
        Winner::None => Cs::Null,
        Winner::Team(t) => Cs::Obj(vec![("team".into(), Cs::Str(t.clone()))]),
        Winner::Player(p) => Cs::Obj(vec![("player".into(), Cs::Int(i64::from(p.0)))]),
    };
    put(&mut f, "winner", winner);
    put(&mut f, "unitsVersion", Cs::Int(i64::from(st.units_version)));
    put(&mut f, "territoryVersion", Cs::Int(i64::from(st.territory_version)));
    put(
        &mut f,
        "shipStaggers",
        Cs::Obj(vec![
            ("tradeShip".into(), Cs::Int(i64::from(st.ship_staggers.trade_ship))),
            ("transportShip".into(), Cs::Int(i64::from(st.ship_staggers.transport_ship))),
        ]),
    );
    // TS: `[...this._nationMirvTargets]` = [[PlayerID, Tick]] in Map order.
    let mut mirv = Vec::with_capacity(st.nation_mirv_targets.len());
    for (p, t) in st.nation_mirv_targets.iter() {
        mirv.push(Cs::Arr(vec![cx.player_id_str(*p), Cs::Int(i64::from(*t))]));
    }
    put(&mut f, "nationMirvTargets", Cs::Arr(mirv));
    put(&mut f, "mirvsLaunched", Cs::Int(i64::from(st.mirvs_launched)));
    // TS: :1686-1697 rail, water, sharedWaterCache, stats (versioned records, unwrapped N4)
    let rail = crate::canon_or_missing!(st.rail, cx);
    let water = water_section(st);
    let shared_water = shared_water_section(cx);
    let stats = match cx.mode {
        CanonMode::Key => None,
        CanonMode::Full => Some(stats_section(cx)),
    };
    GOut { fields: f, rail, water, shared_water, stats }
}

/// The units of `_unitMap` in Map (= ascending id) order: the allocated units that
/// `unit_by_id` names.
pub fn unit_map_order(st: &State) -> Vec<UnitRef> {
    let mut v: Vec<(u32, UnitRef)> =
        st.units.iter_slots().filter(|(h, u)| st.unit_by_id.get(&u.id) == Some(h)).map(|(h, u)| (u.id, h)).collect();
    // Comparator `a - b` on distinct ids: any stable sort gives the same order.
    of_det::js_sort_by(&mut v, |a, b| f64::from(a.0) - f64::from(b.0));
    v.into_iter().map(|(_, h)| h).collect()
}

/// Handle of every allocated unit slot (`None` for a free slot), for grid list walks.
fn slot_handles(units: &Arena<Unit, UnitRef>) -> Vec<Option<UnitRef>> {
    let mut v = vec![None; units.slot_count()];
    for (h, _) in units.iter_slots() {
        v[h.slot as usize] = Some(h); // in-range: slot < slot_count
    }
    v
}

/// `UnitGrid.snapshot(unitRef)` (`engine-lib/src/game/UnitGrid.ts:23-35`): per cell
/// (row-major), each type's units in Set order, types in Map key order (emptied sets stay).
pub fn unit_grid_section(cx: &mut CanonCx<'_>) -> Cs {
    let st = cx.st;
    let g = &st.unit_grid;
    let slots = slot_handles(&st.units);
    let mut out = Vec::with_capacity(g.cells.len());
    for cell in &g.cells {
        let mut entries = Vec::with_capacity(cell.n_types as usize);
        for &ti in &cell.type_order[..cell.n_types as usize] {
            let Some(&t) = UnitType::ALL.get(ti as usize) else {
                entries.push(cx.fail(format!("CS: unit grid type index {ti}")));
                continue;
            };
            let mut ids = Vec::new();
            let mut s = cell.head[ti as usize]; // in-range: ti < 16 (checked above)
            let mut guard = 0usize;
            while s != NIL {
                guard += 1;
                if guard > slots.len() {
                    ids.push(cx.fail("CS: unit grid list cycle"));
                    break;
                }
                match slots.get(s as usize).copied().flatten() {
                    Some(u) => ids.push(cx.unit_key(u)),
                    None => {
                        ids.push(cx.fail(format!("CS: unit grid names free slot {s}")));
                        break;
                    }
                }
                s = g.links.get(s as usize).map_or(NIL, |l| l.next);
            }
            entries.push(Cs::Arr(vec![Cs::Str(t.as_str().to_owned()), Cs::Arr(ids)]));
        }
        out.push(Cs::Arr(entries));
    }
    Cs::Arr(out)
}

/// T: `{map: g.map().state, miniMap: g.miniMap().state}` (`conformance/src/canon.ts:145`).
/// The minimap state is never written on the tick path (ARCHITECTURE §3.6.1): zeros.
pub fn t_section(st: &State, mode: CanonMode) -> Cs {
    let map = state_words(st.map.state_buf());
    let mut f = vec![("map".to_owned(), Cs::Typed(Typed::U16(map)))];
    if mode == CanonMode::Full {
        f.push(("miniMap".to_owned(), Cs::Typed(Typed::U16(state_words(st.mini.state_buf())))));
    }
    Cs::Obj(f)
}

fn state_words(b: &TileStateBuf) -> Vec<u16> {
    match b.as_slice() {
        Some(s) => s.to_vec(),
        None => vec![0; b.len()],
    }
}

/// `GameMapImpl.snapshot()` minus `pristineHash` (SPEC §1.6 N2).
pub fn map_record(m: &GameMap) -> Cs {
    let mut edit_tiles = Vec::with_capacity(m.pristine_edits.len());
    let mut edit_orig = Vec::with_capacity(m.pristine_edits.len());
    let mut edit_cur = Vec::with_capacity(m.pristine_edits.len());
    for (&r, &orig) in m.pristine_edits.iter() {
        edit_tiles.push(r);
        edit_orig.push(orig);
        // TS: `Uint8Array.from(edits, ([ref]) => this.terrain[ref])`; an out-of-range ref
        // would read `undefined` → 0 in a Uint8Array.
        edit_cur.push(m.terrain.get(r as usize).copied().unwrap_or(0));
    }
    let (mut fallout, mut defense) = (Vec::new(), Vec::new());
    if let Some(s) = m.state_buf().as_slice() {
        for (i, &v) in s.iter().enumerate() {
            if v & (1 << FALLOUT_BIT) != 0 {
                fallout.push(i as u32);
            }
            if v & (1 << DEFENSE_BONUS_BIT) != 0 {
                defense.push(i as u32);
            }
        }
    }
    Cs::Obj(vec![
        ("width".into(), Cs::Int(i64::from(m.w))),
        ("height".into(), Cs::Int(i64::from(m.h))),
        ("editTiles".into(), Cs::Typed(Typed::U32(edit_tiles))),
        ("editOriginal".into(), Cs::Typed(Typed::U8(edit_orig))),
        ("editCurrent".into(), Cs::Typed(Typed::U8(edit_cur))),
        ("fallout".into(), Cs::Typed(Typed::U32(fallout))),
        ("defense".into(), Cs::Typed(Typed::U32(defense))),
        ("numLandTiles".into(), Cs::Int(i64::from(m.num_land_tiles))),
        ("waterVersion".into(), Cs::Int(i64::from(m.water_version))),
        ("numTilesWithFallout".into(), Cs::Int(i64::from(m.num_tiles_with_fallout))),
    ])
}

/// M: `{map, miniMap}`.
pub fn m_section(st: &State) -> Cs {
    Cs::Obj(vec![("map".into(), map_record(&st.map)), ("miniMap".into(), map_record(&st.mini))])
}

/// `WaterManager.snapshot()` (`WaterManager.ts:95-108`).
pub fn water_section(st: &State) -> Cs {
    let w = &st.water;
    // TS: `this.miniMap.waterVersion() > 0 ? (this._miniWaterCC?.snapshot() ?? null) : null`
    let components =
        if st.mini.water_version > 0 { w.components.as_deref().map_or(Cs::Null, components_record) } else { Cs::Null };
    Cs::Obj(vec![
        ("waterGraphVersion".into(), Cs::Int(i64::from(w.water_graph_version))),
        ("waterGraphDirty".into(), Cs::Bool(w.water_graph_dirty)),
        ("waterGraphLastRebuildTick".into(), Cs::Int(i64::from(w.water_graph_last_rebuild_tick))),
        ("pendingWaterTiles".into(), tiles(w.pending_water_tiles.iter().copied())),
        ("dirtyMiniTiles".into(), tiles(w.dirty_mini_tiles.iter().copied())),
        ("components".into(), components),
    ])
}

/// `ConnectedComponents.snapshot()` (`pathfinding/algorithms/ConnectedComponents.ts:51-66`):
/// `null` before `initialize`; `parents` reduced to roots (path compression is history).
pub fn components_record(c: &Components) -> Cs {
    let Some(ids) = &c.component_ids else { return Cs::Null };
    let ids = match ids {
        ComponentIds::U8(v) => Cs::Typed(Typed::U8(v.clone())),
        ComponentIds::U16(v) => Cs::Typed(Typed::U16(v.clone())),
    };
    // TS: `[...this._componentSizes]`: holes spread to `undefined` (ABSENT elements).
    let sizes = c.component_sizes.iter().map(|s| s.map_or(Cs::Absent, |x| Cs::Int(i64::from(x)))).collect();
    // TS: `parents.map((_, id) => { let root = id; while (parents[root] !== root) root = parents[root]; return root; })`
    let parents = &c.parents;
    let mut roots = Vec::with_capacity(parents.len());
    for id in 0..parents.len() {
        let mut root = id;
        let mut steps = 0usize;
        loop {
            match parents.get(root) {
                Some(&p) if p as usize != root => {
                    root = p as usize;
                    steps += 1;
                    if steps > parents.len() {
                        break; // a cycle: TS would loop forever; report the last node
                    }
                }
                // `parents[root]` undefined !== root → TS walks to `undefined`; impossible for a
                // well-formed union-find, so stop at the out-of-range node.
                _ => break,
            }
        }
        roots.push(Cs::Int(root as i64));
    }
    Cs::Obj(vec![
        ("componentIds".into(), ids),
        ("componentSizes".into(), Cs::Arr(sizes)),
        ("parents".into(), Cs::Arr(roots)),
        ("maxId".into(), Cs::Int(i64::from(c.max_id))),
        ("landMarker".into(), Cs::Int(i64::from(c.land_marker))),
    ])
}

/// `SharedWaterCache.snapshot(w)` (`SharedWaterCache.ts:50-61`).
pub fn shared_water_section(cx: &mut CanonCx<'_>) -> Cs {
    let s = &cx.st.shared_water;
    let by_player = match &s.by_player {
        None => Cs::Null,
        Some(m) => Cs::Arr(
            m.iter()
                .map(|(p, set)| {
                    let set =
                        set.as_ref().map_or(Cs::Null, |s| Cs::Arr(s.iter().map(|&x| Cs::Int(i64::from(x))).collect()));
                    Cs::Arr(vec![Cs::Int(i64::from(p.0)), set])
                })
                .collect(),
        ),
    };
    Cs::Obj(vec![("tick".into(), Cs::Num(s.tick)), ("byPlayer".into(), by_player)])
}

/// S: `StatsImpl.snapshot()` = `{data}` (`StatsImpl.ts:87-89`), the content being `Stats`'s
/// owner's (P2). Without `stats-full` the build keeps only `attacks_sent` (ARCHITECTURE
/// §3.10), so S — and with it D — cannot be produced (K has `S = null`).
pub fn stats_section(cx: &mut CanonCx<'_>) -> Result<Cs, CanonError> {
    #[cfg(feature = "stats-full")]
    {
        let st = cx.st;
        crate::canon_or_missing!(st.stats, cx)
    }
    #[cfg(not(feature = "stats-full"))]
    {
        let _ = cx;
        Err(CanonError::missing("the S section (build with feature `stats-full`, ARCHITECTURE §3.10)", "Stats"))
    }
}

// =====================================================================================
// Import (TS `restoreSnapshot` semantics)
// =====================================================================================

/// `GameMapImpl.restoreSnapshot(s)` (`GameMapImpl.ts:614-640`) onto a freshly loaded map.
/// The `pristineHash` check is replaced by the trace header's map SHA-256s (SPEC §1.6 N2).
pub fn load_map_record(m: &mut GameMap, v: &Cs) -> Result<(), CanonError> {
    let w = int(field(v, "width")?)?;
    let h = int(field(v, "height")?)?;
    if w != i64::from(m.w) || h != i64::from(m.h) {
        // TS: :615-619
        return Err(CanonError::new(format!("snapshot map is {w}x{h}, loaded map is {}x{}", m.w, m.h)));
    }
    // TS: :620-622 "snapshot must be restored onto a freshly loaded map"
    let state_dirty = m.state_buf().as_slice().is_some_and(|s| s.iter().any(|&x| x != 0));
    if !m.pristine_edits.is_empty() || state_dirty {
        return Err(CanonError::new("snapshot must be restored onto a freshly loaded map"));
    }
    let et = tiles_of(field(v, "editTiles")?)?;
    let eo = bytes_of(field(v, "editOriginal")?)?;
    let ec = bytes_of(field(v, "editCurrent")?)?;
    if eo.len() != et.len() || ec.len() != et.len() {
        return Err(CanonError::new("editTiles/editOriginal/editCurrent lengths differ"));
    }
    if !et.is_empty() {
        let mut terrain = m.terrain.to_vec(); // COW: one copy for the whole restore
        for (i, &r) in et.iter().enumerate() {
            let Some(cur) = terrain.get_mut(r as usize) else {
                return Err(CanonError::new(format!("edit tile {r} outside the map")));
            };
            // TS: :627-630
            if *cur != eo[i] {
                return Err(CanonError::new(format!("terrain of tile {r} differs from the snapshot")));
            }
            // TS: :631 `noteTerrainEdit(ref)` keeps the first original byte (first-edit order).
            if !m.pristine_edits.contains_key(&r) {
                m.pristine_edits.insert(r, eo[i]);
            }
            *cur = ec[i]; // TS: :632
            m.terrain_gen = m.terrain_gen.wrapping_add(1); // every terrain write bumps the gen (§3.6.1)
        }
        m.terrain = Arc::from(terrain);
    }
    let fallout = tiles_of(field(v, "fallout")?)?;
    let defense = tiles_of(field(v, "defense")?)?;
    if !fallout.is_empty() || !defense.is_empty() {
        let n = m.state_buf().len();
        let buf = m.state_buf_mut();
        for (&r, bit) in fallout.iter().map(|r| (r, FALLOUT_BIT)).chain(defense.iter().map(|r| (r, DEFENSE_BONUS_BIT)))
        {
            if r as usize >= n {
                return Err(CanonError::new(format!("fallout/defense tile {r} outside the map")));
            }
            match buf.as_slice() {
                // TS: `this.state[ref] |= 1 << BIT` (`:634-637`)
                Some(s) => {
                    let cur = s[r as usize]; // in-range: checked above
                    buf.set_raw(r, cur | (1 << bit));
                }
                // The minimap's state is the all-zero buffer (ARCHITECTURE §3.6.1): TS writes
                // these bits only here, and no live game ever sets them on the minimap.
                None => return Err(CanonError::new("minimap fallout/defense bits are not supported (§3.6.1)")),
            }
        }
    }
    m.num_land_tiles = u32_of(field(v, "numLandTiles")?)?;
    m.water_version = u32_of(field(v, "waterVersion")?)?;
    m.num_tiles_with_fallout = u32_of(field(v, "numTilesWithFallout")?)?;
    Ok(())
}

/// M import: main map, then minimap (`GameSnapshot.ts:260-265`).
pub fn load_m(st: &mut State, m: &Cs) -> Result<(), CanonError> {
    load_map_record(&mut st.map, field(m, "map")?).map_err(|e| e.at("M.map"))?;
    load_map_record(&mut st.mini, field(m, "miniMap")?).map_err(|e| e.at("M.miniMap"))
}

fn bytes_of(v: &Cs) -> Result<&[u8], CanonError> {
    match v {
        Cs::Typed(Typed::U8(b)) => Ok(b),
        _ => Err(CanonError::new(format!("expected bytes, got {}", super::short(v)))),
    }
}
fn u32_of(v: &Cs) -> Result<u32, CanonError> {
    u32::try_from(int(v)?).map_err(|_| CanonError::new(format!("expected uint, got {}", super::short(v))))
}
fn i32_of(v: &Cs) -> Result<i32, CanonError> {
    i32::try_from(int(v)?).map_err(|_| CanonError::new(format!("expected int32, got {}", super::short(v))))
}

/// The constructor arguments of `new GameImpl(humans, nations, ..)` read from G
/// (`GameSnapshot.ts:267-275`).
pub fn load_humans_nations(g: &Cs, cx: &mut UncanonCx<'_>) -> Result<(Vec<PlayerInfo>, Vec<Nation>), CanonError> {
    Ok((super::load_field(g, "humans", cx)?, super::load_field(g, "nations", cx)?))
}

/// `GameImpl.restoreState(s, r)` (`GameImpl.ts:1709-1758`) minus rail, water, shared water
/// and stats (their own loaders) and the exec lists (the loader's, it owns the arenas):
/// scalars, alliance requests, `_unitMap`, `planDrivenUnitIds`, the unit grid, teams,
/// winner, versions, staggers, nation MIRV targets.
pub fn load_g(g: &Cs, cx: &mut UncanonCx<'_>) -> Result<(), CanonError> {
    // TS: G.players is `_players` Map order, which the port requires to be smallID order
    // (ARCHITECTURE §3.1).
    let players = arr(field(g, "players")?)?;
    for (i, p) in players.iter().enumerate() {
        if int(p)? != i as i64 + 1 {
            return Err(CanonError::new(format!(
                "G.players[{i}] = {} breaks smallID order (ARCHITECTURE §3.1: unsupported)",
                int(p)?
            )));
        }
    }
    if players.len() != usize::from(cx.keys.n_players) {
        return Err(CanonError::new("G.players and P disagree"));
    }
    let st = &mut *cx.st;
    st.ticks = i32_of(field(g, "ticks")?)?;
    st.start_tick = match field(g, "startTick")? {
        Cs::Null => None,
        v => Some(i32_of(v)?),
    };
    let mut reqs = JsArr::new();
    for k in arr(field(g, "allianceRequests")?)? {
        reqs.push(cx.request(str_(k)?)?);
    }
    let st = &mut *cx.st;
    st.alliance_requests = reqs;
    st.next_player_id = u32_of(field(g, "nextPlayerID")?)?;
    st.next_unit_id = u32_of(field(g, "nextUnitID")?)?;
    st.next_alliance_id = u32_of(field(g, "nextAllianceID")?)?;
    // TS: `this._unitMap = new Map(s.units.map(i => [u.id(), r.unit(i)]))`
    let mut unit_by_id = crate::jsorder::LookupMap::new();
    for k in arr(field(g, "units")?)? {
        let id = int(k)?;
        let u = cx.unit(id)?;
        unit_by_id.insert(id as u32, u);
    }
    let st = &mut *cx.st;
    st.unit_by_id = unit_by_id;
    let mut plan = JsSet::default();
    for x in arr(field(g, "planDrivenUnitIds")?)? {
        plan.insert(u32_of(x)?);
    }
    st.plan_driven_unit_ids = plan;
    load_unit_grid(field(g, "unitGrid")?, cx)?;
    let st = &mut *cx.st;
    st.player_teams =
        arr(field(g, "playerTeams")?)?.iter().map(|t| str_(t).map(str::to_owned)).collect::<Result<_, _>>()?;
    st.bot_team = str_(field(g, "botTeam")?)?.to_owned();
    st.is_paused = field(g, "isPaused")?.as_bool().ok_or_else(|| CanonError::new("G.isPaused"))?;
    let winner = match field(g, "winner")? {
        Cs::Null => Winner::None,
        w => match (opt_field(w, "team"), opt_field(w, "player")) {
            (Some(t), _) => Winner::Team(str_(t)?.to_owned()),
            (None, Some(p)) => Winner::Player(cx.player(int(p)?)?),
            _ => return Err(CanonError::new("G.winner is neither {team} nor {player}")),
        },
    };
    let st = &mut *cx.st;
    st.winner = winner;
    st.units_version = u32_of(field(g, "unitsVersion")?)?;
    st.territory_version = u32_of(field(g, "territoryVersion")?)?;
    // TS: :1737 "Not snapshotted: it only keys memos, which must not outlive the restore."
    st.structure_version = st.structure_version.wrapping_add(1);
    let ss = field(g, "shipStaggers")?;
    st.ship_staggers.trade_ship = i32_of(field(ss, "tradeShip")?)?;
    st.ship_staggers.transport_ship = i32_of(field(ss, "transportShip")?)?;
    // TS: `new Map(s.nationMirvTargets)` keyed by PlayerID; the port keys by smallID.
    let mut mt = JsMap::default();
    for e in arr(field(g, "nationMirvTargets")?)? {
        let [pid, t] = arr(e)? else { return Err(CanonError::new("G.nationMirvTargets entry is not a pair")) };
        let pid = str_(pid)?;
        let p = player_by_id_str(st, pid)
            .ok_or_else(|| CanonError::new(format!("G.nationMirvTargets names unknown player {pid:?}")))?;
        mt.insert(p, i32_of(t)?);
    }
    st.nation_mirv_targets = mt;
    st.mirvs_launched = u32_of(field(g, "mirvsLaunched")?)?;
    Ok(())
}

/// The smallID of the player whose `PlayerInfo.id` is `id`.
pub fn player_by_id_str(st: &State, id: &str) -> Option<SmallId> {
    st.players.iter().find(|p| st.infos.get(p.info.0 as usize).is_some_and(|i| i.id == id)).map(|p| p.small_id)
}

/// `UnitGrid.restoreSnapshot(cells, unit)` (`UnitGrid.ts:37-48`): for cell `i`
/// (`grid[floor(i / width)][i % width]`), `cell.set(type, new Set(refs.map(unit)))` in entry
/// order. The grid dimensions are `UnitGrid::new`'s (`ceil(H/100) × ceil(W/100)`); a grid
/// with no cells (an un-constructed `Default`) is sized from the map here.
pub fn load_unit_grid(v: &Cs, cx: &mut UncanonCx<'_>) -> Result<(), CanonError> {
    let cells = arr(v)?;
    let (w, h) = (cx.st.map.w, cx.st.map.h);
    let n_slots = cx.st.units.slot_count();
    let g = &mut cx.st.unit_grid;
    if g.cells.is_empty() && !cells.is_empty() {
        // TS: UnitGrid ctor `cellSize = 100`, `Math.ceil(height / cellSize)` rows of
        // `Math.ceil(width / cellSize)` cells.
        g.cell_size = 100;
        g.cols = w.div_ceil(100);
        g.rows = h.div_ceil(100);
        g.cells = vec![CellHdr::default(); (g.cols * g.rows) as usize];
    }
    if cells.len() != g.cells.len() {
        return Err(CanonError::new(format!("G.unitGrid has {} cells, the grid {}", cells.len(), g.cells.len())));
    }
    g.links.clear();
    g.links.resize(n_slots, UnitLink::default());
    for (ci, entries) in cells.iter().enumerate() {
        for e in arr(entries)? {
            let [t, ids] = arr(e)? else { return Err(CanonError::new("G.unitGrid entry is not a pair")) };
            let ts = str_(t)?;
            let ty = UnitType::from_ts(ts).ok_or_else(|| CanonError::new(format!("G.unitGrid type {ts:?}")))?;
            let ti = ty.index();
            let mut list = Vec::new();
            for k in arr(ids)? {
                list.push(cx.unit(int(k)?)?);
            }
            let g = &mut cx.st.unit_grid;
            let cell = &mut g.cells[ci]; // in-range: lengths checked above
            // `Map.set` on a fresh cell appends the key; on an existing key it keeps the
            // position (a snapshot never repeats a key).
            if !cell.type_order[..cell.n_types as usize].contains(&(ti as u8)) {
                cell.type_order[cell.n_types as usize] = ti as u8; // in-range: ≤ 16 distinct types
                cell.n_types += 1;
            }
            // `new Set(refs)`: drops duplicates, keeps first-insertion order.
            let mut prev = NIL;
            let mut head = NIL;
            let mut count = 0u16;
            for u in list {
                let s = u.slot;
                let link = &mut g.links[s as usize]; // in-range: links sized to slot_count
                if link.cell != NIL {
                    continue; // already in a set (duplicate ref)
                }
                // `..default()`: P3 adds non-snapshot link fields (e.g. the slot generation,
                // which is 0 for every unit of a fresh load).
                #[allow(clippy::needless_update)] // forward-compatible with P3's added fields
                {
                    *link = UnitLink { prev, next: NIL, cell: ci as u32, ..UnitLink::default() };
                }
                if prev == NIL {
                    head = s;
                } else {
                    g.links[prev as usize].next = s; // in-range: prev was a valid slot
                }
                prev = s;
                count += 1;
            }
            let cell = &mut g.cells[ci]; // in-range: as above
            cell.head[ti] = head;
            cell.tail[ti] = prev;
            if ty == UnitType::DefensePost {
                cell.n_defense_posts = count;
            }
        }
    }
    // t409 H5: the non-snapshot total, from the loaded cells
    let g = &mut cx.st.unit_grid;
    g.n_defense_posts_total = g.cells.iter().map(|c| u32::from(c.n_defense_posts)).sum();
    Ok(())
}

/// `WaterManager.restoreSnapshot(s)` (`WaterManager.ts:119-137`): scalars and the two Sets,
/// and the components when stored. Returns whether the graph must be rebuilt from them
/// ([`rebuild_water_graph`]); with `components: null` the constructor's graph (the version-0
/// graph of the pristine minimap) stays.
pub fn load_water(st: &mut State, v: &Cs) -> Result<bool, CanonError> {
    let w = &mut st.water;
    w.water_graph_version = u32_of(field(v, "waterGraphVersion")?)?;
    w.water_graph_dirty = field(v, "waterGraphDirty")?.as_bool().ok_or_else(|| CanonError::new("waterGraphDirty"))?;
    w.water_graph_last_rebuild_tick = i32_of(field(v, "waterGraphLastRebuildTick")?)?;
    w.pending_water_tiles = tiles_of(field(v, "pendingWaterTiles")?)?.iter().copied().collect();
    w.dirty_mini_tiles = tiles_of(field(v, "dirtyMiniTiles")?)?.iter().copied().collect();
    let c = field(v, "components")?;
    if c.is_null() {
        return Ok(false);
    }
    // TS: `if (s.components !== null && this._miniWaterCC !== null)`
    let Some(cur) = w.components.as_ref() else { return Ok(false) };
    let mut comp: Components = (**cur).clone();
    load_components(&mut comp, c)?;
    w.components = Some(Arc::new(comp));
    Ok(true)
}

/// `ConnectedComponents.restoreSnapshot(s)` (`ConnectedComponents.ts:69-75`).
pub fn load_components(c: &mut Components, v: &Cs) -> Result<(), CanonError> {
    c.component_ids = Some(match field(v, "componentIds")? {
        Cs::Typed(Typed::U8(b)) => ComponentIds::U8(b.clone()),
        Cs::Typed(Typed::U16(b)) => ComponentIds::U16(b.clone()),
        x => return Err(CanonError::new(format!("componentIds: expected bytes or u16s, got {}", super::short(x)))),
    });
    c.component_sizes = arr(field(v, "componentSizes")?)?
        .iter()
        .map(|x| match x {
            Cs::Absent => Ok(None),
            x => u32_of(x).map(Some),
        })
        .collect::<Result<_, _>>()?;
    c.parents = arr(field(v, "parents")?)?.iter().map(u32_of).collect::<Result<_, _>>()?;
    c.max_id = u32_of(field(v, "maxId")?)?;
    c.land_marker = u32_of(field(v, "landMarker")?)?;
    Ok(())
}

/// The graph rebuild of `WaterManager.restoreSnapshot` (`WaterManager.ts:126-135`): a new
/// `AbstractGraphBuilder(miniMap, .., this._miniWaterCC, ..).build()` set on the HPA, with an
/// empty `_pathCache` (TS-restore semantics, ARCHITECTURE §9 "Loader").
pub fn rebuild_water_graph(st: &mut State, sc: &mut crate::scratch::Scratch) {
    let Some(comp) = st.water.components.clone() else { return };
    let g = Arc::new(crate::water::abstract_graph::WaterGraph::build(&st.mini, &comp, &mut sc.water_mgr.builder_bfs));
    st.water.seg_vis.reset(&g);
    st.water.graph = Some(g);
}

/// `SharedWaterCache.restoreSnapshot(s, r)` (`SharedWaterCache.ts:63-74`).
pub fn load_shared_water(v: &Cs, cx: &mut UncanonCx<'_>) -> Result<(), CanonError> {
    let tick = field(v, "tick")?.as_f64().ok_or_else(|| CanonError::new("sharedWaterCache.tick"))?;
    let by_player = match field(v, "byPlayer")? {
        Cs::Null => None,
        bp => {
            let mut m = JsMap::default();
            for e in arr(bp)? {
                let [p, set] = arr(e)? else { return Err(CanonError::new("byPlayer entry is not a pair")) };
                let p = cx.player(int(p)?)?;
                let set = match set {
                    Cs::Null => None,
                    s => Some(arr(s)?.iter().map(i32_of).collect::<Result<JsSet<i32>, _>>()?),
                };
                m.insert(p, set);
            }
            Some(m)
        }
    };
    let s = &mut cx.st.shared_water;
    s.tick = tick;
    s.by_player = by_player;
    Ok(())
}

/// S import: `StatsImpl.restoreSnapshot(s)` = `this.data = s.data`, through `Stats`'s owner
/// (P2) under `stats-full`. Without the feature the lite `Stats` keeps only `attacks_sent`:
/// the loader reads it from S (`S.data[clientID].attacks[0]`, `ATTACK_INDEX_SENT`), the one
/// stats value the simulation reads (`GameImpl.ts:1777-1781`).
pub fn load_stats(v: &Cs, cx: &mut UncanonCx<'_>) -> Result<(), CanonError> {
    #[cfg(feature = "stats-full")]
    {
        cx.st.stats = crate::uncanon_or_missing!(crate::state::stats::Stats, v, cx)?;
        Ok(())
    }
    #[cfg(not(feature = "stats-full"))]
    {
        let mut lite = JsMap::default();
        if let Some(Cs::Obj(clients)) = opt_field(v, "data") {
            for (client, rec) in clients {
                // TS: `stats.getPlayerStats(p)?.attacks?.[ATTACK_INDEX_SENT] ?? 0n`
                let sent = match opt_field(rec, "attacks").and_then(Cs::as_arr).and_then(|a| a.first()) {
                    Some(b @ Cs::Big(_)) => super::big_of(b)?,
                    _ => 0,
                };
                lite.insert(client.clone(), crate::state::stats::PlayerStatsLite { attacks_sent: sent });
            }
        }
        cx.st.stats.lite = lite;
        Ok(())
    }
}

/// Tile ownership from the players' tile lists (`GameSnapshot.ts:402-407`): for each player
/// in `_players` order, `map.setOwnerID(tile, smallID)` for every tile of `_tiles`.
pub fn apply_owner_bits(st: &mut State) -> Result<(), CanonError> {
    let n = st.map.state_buf().len();
    let mut writes: Vec<(TileRef, u16)> = Vec::new();
    for p in &st.players {
        for t in p.tiles.iter() {
            if t as usize >= n {
                return Err(CanonError::new(format!("P[smallID={}].tiles holds {t}, outside the map", p.small_id.0)));
            }
            writes.push((t, p.small_id.0));
        }
    }
    let buf = st.map.state_buf_mut();
    for (t, id) in writes {
        buf.set_owner_id(t, id);
    }
    Ok(())
}

/// Add `tiles` to player `p`'s `_tiles` (or `_borderTiles` when `border`) in order: TS
/// `new TileSet(s.tiles)` (`PlayerImpl.ts:2081-2082`).
pub fn load_player_tiles(st: &mut State, p: SmallId, tiles: &[u32], border: bool) -> Result<(), CanonError> {
    let n_land = st.statics.land_index.len();
    let (pos, hdr, li) = if border { st.border_parts_mut(p) } else { st.tiles_parts_mut(p) };
    for &t in tiles {
        if (t as usize) >= n_land {
            return Err(CanonError::new(format!("P[smallID={}] tile {t} outside the map", p.0)));
        }
        if !pos.add(hdr, li, t) {
            return Err(CanonError::new(format!("P[smallID={}] lists tile {t} twice", p.0)));
        }
    }
    Ok(())
}

/// The keys one load resolves (exported for tests that drive the section loaders alone).
pub fn keys_with_players(n_players: u16) -> LoadKeys {
    LoadKeys { n_players, ..LoadKeys::default() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::ExecStore;

    #[test]
    fn map_record_round_trip() {
        let mut m = GameMap::default();
        m.w = 4;
        m.h = 2;
        m.terrain = Arc::from(vec![0u8; 8]);
        *m.state_buf_mut() = TileStateBuf::new_full(8);
        let v = Cs::Obj(vec![
            ("width".into(), Cs::Int(4)),
            ("height".into(), Cs::Int(2)),
            ("editTiles".into(), Cs::Typed(Typed::U32(vec![5, 1]))),
            ("editOriginal".into(), Cs::Typed(Typed::U8(vec![0, 0]))),
            ("editCurrent".into(), Cs::Typed(Typed::U8(vec![32, 64]))),
            ("fallout".into(), Cs::Typed(Typed::U32(vec![2, 7]))),
            ("defense".into(), Cs::Typed(Typed::U32(vec![7]))),
            ("numLandTiles".into(), Cs::Int(3)),
            ("waterVersion".into(), Cs::Int(2)),
            ("numTilesWithFallout".into(), Cs::Int(2)),
        ]);
        load_map_record(&mut m, &v).expect("load");
        assert_eq!(map_record(&m), v, "export(import(M)) == M (edit order kept)");
        assert_eq!(m.terrain_gen, 2);
        // A second restore onto a non-fresh map throws, as TS does.
        assert!(load_map_record(&mut m, &v).is_err());
    }

    #[test]
    fn components_roots_and_holes() {
        let c = Components {
            component_ids: Some(ComponentIds::U8(vec![1, 2, 255])),
            component_sizes: vec![None, Some(4), Some(0)],
            parents: vec![0, 2, 2, 1],
            max_id: 3,
            land_marker: 255,
            width: 3,
            height: 1,
        };
        let v = components_record(&c);
        assert_eq!(
            v.get("parents"),
            Some(&Cs::Arr(vec![Cs::Int(0), Cs::Int(2), Cs::Int(2), Cs::Int(2)])),
            "parents are reduced to roots"
        );
        assert_eq!(v.get("componentSizes").and_then(Cs::as_arr).map(|a| a[0].clone()), Some(Cs::Absent));
        let mut c2 = Components::default();
        load_components(&mut c2, &v).expect("load");
        assert_eq!(components_record(&c2), v);
    }

    #[test]
    fn empty_state_sections() {
        let st = State::default();
        let ex = ExecStore::default();
        let mut cx = CanonCx::with_mode(&st, &ex, CanonMode::Key);
        let g = g_section(&mut cx);
        assert!(g.stats.is_none(), "K projection: S = null, never computed");
        assert_eq!(g.shared_water.get("tick"), Some(&Cs::Num(f64::NEG_INFINITY)));
        assert_eq!(g.water.get("components"), Some(&Cs::Null));
        assert_eq!(t_section(&st, CanonMode::Key).get("miniMap"), None);
        assert!(cx.errors.is_empty(), "{:?}", cx.errors);
    }
}
