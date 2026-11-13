//! `Game::from_canon`: load a CS v1 state (a *resumable* dump, SPEC §2.5, §2.9 R3) with
//! **TS-restore semantics** (ARCHITECTURE §9 "Loader"): a literal port of
//! `restoreGame` (`engine/src/snapshot/GameSnapshot.ts:249-408`) over CS instead of the
//! engine's snapshot bytes. Owner: **P13**.
//!
//! Order, as in TS:
//! 1. the maps (`GameMapImpl.restoreSnapshot`, `:260-265`);
//! 2. a game built with `restoring = true` from the stored humans/nations (`:267-284`);
//! 3. pass 1: a handle for every row of every table (`:286-358`): the [`LoadKeys`] (units,
//!    attacks, railroads and clusters get the arena slots a fresh arena hands out in table
//!    order; executions get `(kind, n-th of that kind)`);
//! 4. pass 2: players, units, attacks, alliances, requests, stations, railroads, clusters,
//!    executions (`:373-398`), each through its owner's `Uncanon`;
//! 5. `restoreState` (`:399`): G, the rail network, water, shared water cache, stats;
//! 6. tile ownership from the players' tile lists (`:402-407`).
//!
//! Non-snapshotted state takes its TS-restore value: memos empty (owners' `Uncanon`),
//! `_structureVersion` + 1, the water graph rebuilt from the components when stored (else the
//! constructor's version-0 graph), an empty segment cache and no water chain slot (TS: a new
//! game has an empty `_waterChainCache`), so steppers keep only their snapshotted fields
//! (`WaterPathFinder.fromState`, `PathFinder.ts:326-342`).

use super::exec_rows::exec_from_state;
use super::sections::{
    apply_owner_bits, load_g, load_humans_nations, load_m, load_player_tiles, load_shared_water, load_stats,
    load_water, rebuild_water_graph, t_section,
};
use super::{CanonError, CanonMode, Cs, LoadKeys, UncanonCx, arr, field, int, str_, tiles_of};
use crate::Game;
use crate::api::game_config::GameStartInfo;
use crate::config::EngineConfig;
use crate::exec::{ExecStore, WarheadRef};
use crate::ids::{AttackId, AttackRef, ClusterRef, ExecId, ExecKind, RailroadRef, ReqIdx, SmallId, UnitRef};
use crate::map::MapStatics;
use crate::rail::RailNetwork;
use crate::rail::railroad::Railroad;
use crate::rail::station::{Cluster, TrainStation};
use crate::state::State;
use crate::state::alliance::Alliance;
use crate::state::alliance_request::AllianceRequest;
use crate::state::attack::Attack;
use crate::state::player::Player;
use crate::state::unit::Unit;
use of_canon::digest::{CanonState, Section};
use std::sync::Arc;

impl Game {
    /// `createGameRunnerFromSnapshot(gameStart, snapshot, ..)` (`GameRunner.ts:122-148`) over a
    /// CS v1 state: the config is `gameStart.config` (`new EngineConfig(gc, false,
    /// gameStart.listed)`), the maps come from `statics`, and no `init()` runs.
    pub fn from_canon(start: &GameStartInfo, statics: Arc<MapStatics>, cs: &CanonState) -> Result<Game, CanonError> {
        let cfg = Arc::new(EngineConfig::new(start.config.clone(), false, start.listed.unwrap_or(false), false));
        // `new GameImpl(humans, nations, .., restoring = true)`: the constructor without
        // `populateTeams`/`addPlayers` (`GameImpl.ts:209-214`), P1's `create_state_restoring`
        // (request P13 #2, merge1). `playerTeams`/`botTeam` are then written from G.
        let st = crate::state::create_state_restoring(Vec::new(), Vec::new(), statics, cfg);
        load_into(st, &start.game_id, cs)
    }
}

/// Pass 1 (`GameSnapshot.ts:286-358`): the key → handle tables. `st` must be fresh (empty
/// arenas), so the handles predicted here are the ones the arenas allocate in pass 2.
pub fn assign_keys(st: &State, cs: &CanonState) -> Result<LoadKeys, CanonError> {
    let mut k = LoadKeys::default();
    let players = arr(cs.get(Section::P)).map_err(|e| e.at("P"))?;
    for (i, p) in players.iter().enumerate() {
        let sid = int(field(p, "smallID")?)?;
        if sid != i as i64 + 1 {
            return Err(CanonError::new(format!("P[{i}].smallID = {sid}: rows must be smallIDs 1..n")));
        }
    }
    k.n_players = u16::try_from(players.len()).map_err(|_| CanonError::new("too many players"))?;
    if !st.units.is_empty() || st.units.slot_count() != 0 || st.attacks.slot_count() != 0 {
        return Err(CanonError::new("load onto a fresh state (unit/attack arenas must be empty)"));
    }
    if st.rail.railroads.slot_count() != 0 || st.rail.clusters.slot_count() != 0 {
        return Err(CanonError::new("load onto a fresh state (rail arenas must be empty)"));
    }
    for (i, u) in arr(cs.get(Section::U)).map_err(|e| e.at("U"))?.iter().enumerate() {
        let id = u32::try_from(int(field(u, "id")?)?).map_err(|_| CanonError::new("U: bad id"))?;
        if k.units.insert(id, UnitRef { slot: i as u32, generation: 0 }).is_some() {
            return Err(CanonError::new(format!("U: duplicate unit id {id}")));
        }
    }
    for (i, a) in arr(cs.get(Section::A)).map_err(|e| e.at("A"))?.iter().enumerate() {
        let s = str_(field(a, "id")?)?;
        let id = AttackId::parse(s).ok_or_else(|| CanonError::new(format!("A: bad attack id {s:?}")))?;
        if k.attacks.insert(id, AttackRef { slot: i as u32, generation: 0 }).is_some() {
            return Err(CanonError::new(format!("A: attack id {s} is not unique (SPEC §1.2)")));
        }
    }
    let d = cs.get(Section::D);
    for a in arr(field(d, "alliances")?)? {
        k.alliances.insert(u32::try_from(int(field(a, "id")?)?).map_err(|_| CanonError::new("D: bad alliance id"))?);
    }
    for (i, r) in arr(field(d, "allianceRequests")?)?.iter().enumerate() {
        k.requests.insert(str_(field(r, "key")?)?.to_owned(), ReqIdx(i as u32));
    }
    let r = cs.get(Section::R);
    for s in arr(field(r, "stations")?)? {
        k.stations.insert(u32::try_from(int(field(s, "id")?)?).map_err(|_| CanonError::new("R: bad station id"))?);
    }
    for (i, x) in arr(field(r, "railroads")?)?.iter().enumerate() {
        let id = u32::try_from(int(field(x, "id")?)?).map_err(|_| CanonError::new("R: bad railroad id"))?;
        k.railroads.insert(id, RailroadRef { slot: i as u32, generation: 0 });
    }
    for (i, c) in arr(field(r, "clusters")?)?.iter().enumerate() {
        k.clusters.insert(str_(field(c, "key")?)?.to_owned(), ClusterRef { slot: i as u32, generation: 0 });
    }
    let mut per_kind = [0u32; ExecKind::COUNT];
    for (pos, e) in arr(cs.get(Section::E)).map_err(|e| e.at("E"))?.iter().enumerate() {
        let t = str_(field(e, "t")?)?;
        let kind =
            ExecKind::from_name(t).ok_or_else(|| CanonError::new(format!("E[{pos}]: unknown execution type {t}")))?;
        let n = &mut per_kind[kind as usize]; // in-range: the 44 registered kinds
        k.execs.push(ExecId { kind, idx: *n });
        *n += 1;
    }
    Ok(k)
}

/// Load `cs` into the freshly constructed `st` (a `restoring` `GameImpl`: maps pristine, no
/// players, no executions). See the module docs for the order.
pub fn load_into(mut st: State, game_id: &str, cs: &CanonState) -> Result<Game, CanonError> {
    let keys = assign_keys(&st, cs)?;
    let g = cs.get(Section::G);

    // (1) maps
    load_m(&mut st, cs.get(Section::M))?;

    let mut cx = UncanonCx { st: &mut st, keys: &keys };
    // (2) constructor arguments
    let (humans, nations) = load_humans_nations(g, &mut cx).map_err(|e| e.at("G"))?;
    cx.st.humans = Arc::from(humans);
    cx.st.nations = Arc::from(nations);

    // (4) pass 2, players first "so PlayerInfo identity and ownership are in place"
    for (i, row) in arr(cs.get(Section::P))?.iter().enumerate() {
        let sid = SmallId::from_player_index(i);
        let place = format!("P[smallID={}]", sid.0);
        let pl = crate::uncanon_or_missing!(Player, row, &mut cx).map_err(|e| e.at(&place))?;
        if pl.small_id != sid || !pl.tiles.is_empty() || !pl.border_tiles.is_empty() {
            return Err(CanonError::new(format!(
                "{place}: Player::uncanon must return smallID {} with empty tile lists (loader contract)",
                sid.0
            )));
        }
        cx.st.players.push(pl);
        // TS: `this._tiles = new TileSet(s.tiles); this._borderTiles = new TileSet(s.borderTiles)`
        load_player_tiles(cx.st, sid, tiles_of(field(row, "tiles")?)?, false).map_err(|e| e.at(&place))?;
        load_player_tiles(cx.st, sid, tiles_of(field(row, "borderTiles")?)?, true).map_err(|e| e.at(&place))?;
    }
    for (i, row) in arr(cs.get(Section::U))?.iter().enumerate() {
        let u = crate::uncanon_or_missing!(Unit, row, &mut cx).map_err(|e| e.at(format!("U[{i}]")))?;
        let h = cx.st.units.alloc(u);
        check_handle(h, UnitRef { slot: i as u32, generation: 0 }, "U")?;
    }
    for (i, row) in arr(cs.get(Section::A))?.iter().enumerate() {
        let a = crate::uncanon_or_missing!(Attack, row, &mut cx).map_err(|e| e.at(format!("A[{i}]")))?;
        let h = cx.st.attacks.alloc(a);
        check_handle(h, AttackRef { slot: i as u32, generation: 0 }, "A")?;
    }
    let d = cs.get(Section::D);
    for row in arr(field(d, "alliances")?)? {
        let id = int(field(row, "id")?)? as usize;
        let a =
            crate::uncanon_or_missing!(Alliance, row, &mut cx).map_err(|e| e.at(format!("D.alliances[id={id}]")))?;
        // `State.alliances` is indexed by id; ids without a row are unreferenced garbage in TS
        // and unobservable here, so they hold placeholders.
        let v = &mut cx.st.alliances;
        while v.len() < id {
            let ph = crate::ids::AllianceId(v.len() as u32);
            v.push(Alliance { id: ph, ..Alliance::default() });
        }
        if v.len() != id {
            return Err(CanonError::new(format!("D.alliances: id {id} out of order")));
        }
        v.push(a);
    }
    for row in arr(field(d, "allianceRequests")?)? {
        let rq = crate::uncanon_or_missing!(AllianceRequest, row, &mut cx)
            .map_err(|e| e.at(format!("D.allianceRequests[{}]", super::short(row))))?;
        cx.st.alliance_reqs.push(rq);
    }
    let r = cs.get(Section::R);
    let mut stations: Vec<TrainStation> = Vec::new();
    for row in arr(field(r, "stations")?)? {
        stations.push(crate::uncanon_or_missing!(TrainStation, row, &mut cx).map_err(|e| e.at("R.stations"))?);
    }
    let mut railroads: Vec<Railroad> = Vec::new();
    for row in arr(field(r, "railroads")?)? {
        railroads.push(crate::uncanon_or_missing!(Railroad, row, &mut cx).map_err(|e| e.at("R.railroads"))?);
    }
    let mut clusters: Vec<Cluster> = Vec::new();
    for row in arr(field(r, "clusters")?)? {
        clusters.push(crate::uncanon_or_missing!(Cluster, row, &mut cx).map_err(|e| e.at("R.clusters"))?);
    }
    // Executions, in key order.
    let mut ex = ExecStore::default();
    for (pos, row) in arr(cs.get(Section::E))?.iter().enumerate() {
        let want = keys.execs[pos]; // in-range: assign_keys pushed one per row
        let e = exec_from_state(want.kind, field(row, "d")?, &mut cx)
            .map_err(|e| e.at(format!("E[{pos}:{}]", want.kind.as_str())))?;
        let id = ex.ar.insert(e);
        if id != want {
            return Err(CanonError::new(format!("E[{pos}]: allocated {id:?}, predicted {want:?} (Rust bug)")));
        }
    }
    let (n_live, n_un) = exec_lists(g, keys.execs.len())?;
    ex.live = keys.execs[..n_live].to_vec();
    cx.st.uninit = keys.execs[n_live..n_live + n_un].to_vec();
    fix_holds(&mut ex, &keys.execs, n_live + n_un)?;

    // (5) restoreState
    load_g(g, &mut cx).map_err(|e| e.at("G"))?;
    // TS: `_railNetwork.restoreSnapshot(rail, r)`; the R tables' rows go into its storage in
    // table order (the handles pass 1 handed out).
    let mut net =
        crate::uncanon_or_missing!(RailNetwork, field(r, "network")?, &mut cx).map_err(|e| e.at("R.network"))?;
    place_rail_rows(&mut net, stations, railroads, clusters)?;
    cx.st.rail = net;
    let w = cs.get(Section::W);
    let rebuild = load_water(cx.st, field(w, "water")?).map_err(|e| e.at("W.water"))?;
    load_shared_water(field(w, "sharedWaterCache")?, &mut cx).map_err(|e| e.at("W.sharedWaterCache"))?;
    load_stats(cs.get(Section::S), &mut cx).map_err(|e| e.at("S"))?;

    // (6) tile ownership
    apply_owner_bits(&mut st)?;
    if rebuild {
        let mut sc = crate::scratch::Scratch::new();
        rebuild_water_graph(&mut st, &mut sc);
    }
    // T is derived (SPEC §1.3): when the CS carries it, the loaded state must reproduce it.
    let t = cs.get(Section::T);
    if !matches!(t, Cs::Absent) && t_section(&st, CanonMode::Full) != *t {
        return Err(CanonError::new("T: the tile state rebuilt from P.tiles + M differs from the CS (LOADER)"));
    }
    Ok(Game { state: st, execs: ex, game_id: Arc::from(game_id), stuck: None })
}

fn check_handle<H: PartialEq + std::fmt::Debug>(got: H, want: H, what: &str) -> Result<(), CanonError> {
    if got == want {
        Ok(())
    } else {
        Err(CanonError::new(format!("{what}: allocated {got:?}, predicted {want:?} (load onto a fresh arena)")))
    }
}

/// `G.execs` / `G.unInitExecs` are the positions `0..n` and `n..n+m` (SPEC §1.2 "Execution");
/// the rest of E are detached (reachable only through a MIRV).
fn exec_lists(g: &Cs, n_rows: usize) -> Result<(usize, usize), CanonError> {
    let live = arr(field(g, "execs")?)?;
    let un = arr(field(g, "unInitExecs")?)?;
    for (i, k) in live.iter().chain(un.iter()).enumerate() {
        if int(k)? != i as i64 {
            return Err(CanonError::new(format!("G.execs/unInitExecs[{i}] is {}, expected position {i}", int(k)?)));
        }
    }
    if live.len() + un.len() > n_rows {
        return Err(CanonError::new("G lists more executions than E has rows"));
    }
    if let Some(Cs::Arr(types)) = super::opt_field(g, "execTypes")
        && types.len() != n_rows
    {
        return Err(CanonError::new("G.execTypes and E disagree"));
    }
    Ok((live.len(), un.len()))
}

/// Hold counts (ARCHITECTURE §4.3): 1 per listing (`live`/`uninit`) + 1 per MIRV that lists
/// the slot. `insert` gave every slot one hold; a detached slot (position ≥ `n_listed`) has no
/// listing, so after the MIRV holds are added its insert hold is dropped.
fn fix_holds(ex: &mut ExecStore, ids: &[ExecId], n_listed: usize) -> Result<(), CanonError> {
    let mut warheads: Vec<ExecId> = Vec::new();
    for m in ex.ar.mirv.items.iter().flatten() {
        for w in &m.warhead_executions {
            match w {
                WarheadRef::Id(id) => warheads.push(*id),
                WarheadRef::Pending(p) => {
                    return Err(CanonError::new(format!("MIRV warhead {p:?} unbound after load")));
                }
            }
        }
    }
    for &w in &warheads {
        ex.ar.hold(w);
    }
    for &id in &ids[n_listed..] {
        if !warheads.contains(&id) {
            return Err(CanonError::new(format!("E: detached execution {id:?} is listed by no MIRV")));
        }
        ex.ar.release(id);
    }
    Ok(())
}

/// The R tables' rows into the network's storage (the loader contract for
/// `RailNetwork::uncanon`, `canon/mod.rs`).
fn place_rail_rows(
    net: &mut RailNetwork,
    stations: Vec<TrainStation>,
    railroads: Vec<Railroad>,
    clusters: Vec<Cluster>,
) -> Result<(), CanonError> {
    if net.railroads.slot_count() != 0 || net.clusters.slot_count() != 0 {
        return Err(CanonError::new(
            "R.network: RailNetwork::uncanon must leave railroads/clusters empty (loader contract)",
        ));
    }
    let by_id = &mut net.station_manager.by_id;
    for s in stations {
        let i = s.id.0 as usize;
        if by_id.len() <= i {
            by_id.resize_with(i + 1, || None);
        }
        by_id[i] = Some(s); // in-range: resized above
    }
    for (i, rr) in railroads.into_iter().enumerate() {
        let id = rr.id;
        let h = net.railroads.alloc(rr);
        check_handle(h, RailroadRef { slot: i as u32, generation: 0 }, "R.railroads")?;
        net.railroad_by_id.insert(id, h);
    }
    for (i, c) in clusters.into_iter().enumerate() {
        let h = net.clusters.alloc(c);
        check_handle(h, ClusterRef { slot: i as u32, generation: 0 }, "R.clusters")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use of_canon::Cs;

    fn empty_cs() -> CanonState {
        let obj = |f: Vec<(&str, Cs)>| Cs::Obj(f.into_iter().map(|(k, v)| (k.to_owned(), v)).collect());
        let arr0 = || Cs::Arr(vec![]);
        CanonState {
            tick: 0,
            sec: [
                obj(vec![("execs", arr0()), ("unInitExecs", arr0())]),
                Cs::Absent,
                Cs::Null,
                arr0(),
                arr0(),
                arr0(),
                obj(vec![("alliances", arr0()), ("allianceRequests", arr0())]),
                obj(vec![("stations", arr0()), ("railroads", arr0()), ("clusters", arr0())]),
                Cs::Null,
                arr0(),
                Cs::Null,
            ],
        }
    }

    #[test]
    fn keys_of_an_empty_cs() {
        let st = State::default();
        let k = assign_keys(&st, &empty_cs()).expect("keys");
        assert_eq!(k.n_players, 0);
        assert!(k.execs.is_empty());
    }

    #[test]
    fn exec_keys_count_per_kind() {
        let st = State::default();
        let mut cs = empty_cs();
        let row = |t: &str| Cs::Obj(vec![("t".into(), Cs::Str(t.into())), ("d".into(), Cs::Obj(vec![]))]);
        cs.sec[Section::E as usize] = Cs::Arr(vec![row("Nation"), row("Tribe"), row("Nation"), row("MIRV")]);
        let k = assign_keys(&st, &cs).expect("keys");
        assert_eq!(
            k.execs,
            vec![
                ExecId { kind: ExecKind::Nation, idx: 0 },
                ExecId { kind: ExecKind::Tribe, idx: 0 },
                ExecId { kind: ExecKind::Nation, idx: 1 },
                ExecId { kind: ExecKind::Mirv, idx: 0 },
            ]
        );
        cs.sec[Section::E as usize] = Cs::Arr(vec![row("Bogus")]);
        assert!(assign_keys(&st, &cs).is_err());
        // Positions other than 0..n in G.execs are rejected.
        let g = Cs::Obj(vec![("execs".into(), Cs::Arr(vec![Cs::Int(1)])), ("unInitExecs".into(), Cs::Arr(vec![]))]);
        assert!(exec_lists(&g, 2).is_err());
    }
}
