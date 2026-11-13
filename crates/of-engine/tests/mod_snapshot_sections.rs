//! P13 rung T2, MOD-SNAPSHOT (SPEC §5.2: "CS dump → native load → CS re-encode =
//! byte-identical") for the sections P13 owns, on **every dump of the golden CI corpus**:
//! G (all of `snapshotState` except the rail/water/stats sub-records, plus `execTypes` and
//! `unitGrid`), T (rebuilt from P.tiles + M), M (map and minimap) and W (water manager and
//! shared water cache).
//!
//! Each dump is loaded with the P13 section loaders (`load_m`, `load_g`, `load_water`,
//! `load_shared_water`, `load_player_tiles`, `apply_owner_bits`, `assign_keys`) into a state
//! whose *rows* (players, units, alliance requests) are placeholders carrying only what G, T
//! and W read (smallID + PlayerInfo, unit id, request key fields), then re-exported with the
//! P13 exporters (`g_section`, `t_section`, `m_section`, the W records) and compared with the
//! TS bytes by section digest. The full round trip of every section (owners' `Canon` /
//! `Uncanon` for P U A D R E S) is `of-cli`'s `mod_snapshot_full` test (S2).
//!
//! Corpus: `conformance/golden/traces/ci` (set `OF_MOD_SNAPSHOT_TIERS=ci,nightly,regress`
//! for more). Owner: **P13**.

use of_canon::Cs;
use of_canon::digest::{CanonState, Section};
use of_engine::api::types::PlayerInfo;
use of_engine::canon::load::assign_keys;
use of_engine::canon::sections::{
    apply_owner_bits, g_section, load_g, load_humans_nations, load_m, load_player_tiles, load_shared_water, load_water,
    m_section, t_section,
};
use of_engine::canon::{CanonCx, CanonMode, Uncanon, UncanonCx, arr, field, int, str_, tiles_of};
use of_engine::exec::ExecStore;
use of_engine::ids::{ExecId, SmallId};
use of_engine::map::game_map::TileStateBuf;
use of_engine::map::{GameMap, MapStatics};
use of_engine::state::State;
use of_engine::state::alliance_request::{AllianceRequest, RequestStatus};
use of_engine::state::player::Player;
use of_engine::state::unit::Unit;
use of_engine::tileset::PlayerTileOrder;
use of_engine::water::components::Components;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn golden() -> PathBuf {
    if let Some(d) = std::env::var_os("OF_CORPUS_DIR") {
        return PathBuf::from(d).join("traces");
    }
    let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        if d.join("conformance/golden/traces").is_dir() {
            return d.join("conformance/golden/traces");
        }
        assert!(d.pop(), "conformance/golden/traces not found");
    }
}

/// Every dump of the selected tiers, sorted.
fn dumps() -> Vec<PathBuf> {
    let tiers = std::env::var("OF_MOD_SNAPSHOT_TIERS").unwrap_or_else(|_| "ci".into());
    let mut out = Vec::new();
    for tier in tiers.split(',') {
        let Ok(rd) = std::fs::read_dir(golden().join(tier.trim())) else { continue };
        let mut scen: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
        scen.sort();
        for s in scen {
            if let Ok(rd) = std::fs::read_dir(s.join("dumps")) {
                let mut d: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
                d.sort();
                out.extend(d);
            }
        }
    }
    out
}

/// A map record's `GameMap` as `loadTerrainMap` would hand it over: pristine terrain (only the
/// edited tiles' original bytes matter to the restore check), zeroed state.
fn fresh_map(rec: &Cs, mini: bool) -> GameMap {
    let w = int(field(rec, "width").expect("width")).expect("int") as u32;
    let h = int(field(rec, "height").expect("height")).expect("int") as u32;
    let n = (w * h) as usize;
    let mut terrain = vec![0u8; n];
    let et = tiles_of(field(rec, "editTiles").expect("editTiles")).expect("tiles");
    if let Cs::Typed(of_canon::Typed::U8(orig)) = field(rec, "editOriginal").expect("editOriginal") {
        for (i, &t) in et.iter().enumerate() {
            terrain[t as usize] = orig[i];
        }
    }
    let mut m = GameMap::default();
    m.w = w;
    m.h = h;
    m.terrain = Arc::from(terrain);
    *m.state_buf_mut() = if mini { TileStateBuf::new_zero(n) } else { TileStateBuf::new_full(n) };
    m
}

/// A player shell carrying only smallID + PlayerInfo (what G/T/W read).
fn shell_player(small_id: SmallId, info: of_engine::ids::PlayerInfoIdx) -> Player {
    Player {
        small_id,
        info,
        team: None,
        last_tile_change: 0,
        tile_change_version: 0,
        rng: of_engine::of_det::Prng::from_state([0; 4]),
        gold: 0,
        troops: 0,
        trade_gold: 0,
        train_gold: 0,
        piracy_gold: 0,
        gold_earned: 0,
        marked_traitor_tick: 0,
        marked_doomsday_clock_tick: 0,
        rotted_at_tick: 0,
        betrayal_count: 0,
        embargoes: Default::default(),
        tiles: Default::default(),
        border_tiles: Default::default(),
        units: Default::default(),
        my_units_version: 0,
        past_outgoing_alliance_requests: Vec::new(),
        expired_alliances: Vec::new(),
        targets: Vec::new(),
        outgoing_emojis: Vec::new(),
        outgoing_quick_chats: Default::default(),
        sent_donations: Vec::new(),
        relations: Default::default(),
        last_delete_unit_tick: 0,
        last_embargo_all_tick: 0,
        incoming_attacks: Default::default(),
        outgoing_attacks: Default::default(),
        outgoing_land_attacks: Vec::new(),
        alliances: Default::default(),
        spawn_tile: None,
        is_disconnected: false,
        disconnect_snapshot: None,
        largest_cluster_bbox: Default::default(),
        num_units_constructed: Vec::new(),
        my_units_memo: Default::default(),
        my_unit_count_memo: Default::default(),
        my_units_owned_memo: Default::default(),
        nearby_memo: None,
        reachable_components_memo: None,
        conn4: false,
        comps: Default::default(),
    }
}

/// The state `create_state` would hand the loader for this dump's map (maps pristine and
/// sized from M, statics with an identity land index, position indexes sized, the water
/// components present when W stores them), built without the map files.
fn fresh_base(cs: &CanonState) -> Result<State, String> {
    let m = cs.get(Section::M);
    let (mrec, mini_rec) = (field(m, "map").map_err(|e| e.0)?, field(m, "miniMap").map_err(|e| e.0)?);
    let mut st = State::default();
    st.map = fresh_map(mrec, false);
    st.mini = fresh_map(mini_rec, true);
    let n = st.map.state_buf().len();
    // Identity land index: every tile may hold a player tile here (the real index maps land
    // tiles only; the order of each player's list is what is under test).
    st.statics = Arc::new(MapStatics {
        width: st.map.w,
        height: st.map.h,
        terrain: st.map.terrain.clone(),
        num_land_tiles: n as u32,
        mini_width: st.mini.w,
        mini_height: st.mini.h,
        mini_terrain: st.mini.terrain.clone(),
        mini_num_land_tiles: 0,
        land_index: (0..n as u32).collect(),
        nations: Arc::from(Vec::new()),
        additional_nations: Arc::from(Vec::new()),
        team_game_spawn_areas: None,
        manifest: None,
        graph0: None,
        components0: None,
    });
    // The default state's position indexes are empty and private: size them through the
    // split-borrow accessors of a temporary player shell.
    st.players.push(shell_player(SmallId(1), of_engine::ids::PlayerInfoIdx(0)));
    let (pos, _, _) = st.tiles_parts_mut(SmallId(1));
    *pos = PlayerTileOrder::new(n);
    let (pos, _, _) = st.border_parts_mut(SmallId(1));
    *pos = PlayerTileOrder::new(n);
    st.players.clear();
    let w = cs.get(Section::W);
    if !field(field(w, "water").map_err(|e| e.0)?, "components").map_err(|e| e.0)?.is_null() {
        st.water.components = Some(Arc::new(Components::default()));
    }
    Ok(st)
}

/// Load the P13 sections of `cs` into a fresh state (module docs).
fn load_sections(cs: &CanonState) -> Result<(State, ExecStore, Vec<ExecId>), String> {
    let m = cs.get(Section::M);
    let w = cs.get(Section::W);
    let mut st = fresh_base(cs)?;
    let keys = assign_keys(&st, cs).map_err(|e| e.0)?;
    load_m(&mut st, m).map_err(|e| e.0)?;
    let g = cs.get(Section::G);
    {
        let mut cx = UncanonCx { st: &mut st, keys: &keys };
        let (h, na) = load_humans_nations(g, &mut cx).map_err(|e| e.0)?;
        cx.st.humans = Arc::from(h);
        cx.st.nations = Arc::from(na);
        for (i, row) in arr(cs.get(Section::P)).map_err(|e| e.0)?.iter().enumerate() {
            let sid = SmallId::from_player_index(i);
            let info = PlayerInfo::uncanon(field(row, "info").map_err(|e| e.0)?, &mut cx).map_err(|e| e.0)?;
            let idx = cx.intern_player_info(info);
            cx.st.players.push(shell_player(sid, idx));
            load_player_tiles(cx.st, sid, tiles_of(field(row, "tiles").map_err(|e| e.0)?).map_err(|e| e.0)?, false)
                .map_err(|e| e.0)?;
            load_player_tiles(
                cx.st,
                sid,
                tiles_of(field(row, "borderTiles").map_err(|e| e.0)?).map_err(|e| e.0)?,
                true,
            )
            .map_err(|e| e.0)?;
        }
        for row in arr(cs.get(Section::U)).map_err(|e| e.0)? {
            let u = Unit { id: int(field(row, "id").map_err(|e| e.0)?).map_err(|e| e.0)? as u32, ..Unit::default() };
            cx.st.units.alloc(u);
        }
        for row in arr(field(cs.get(Section::D), "allianceRequests").map_err(|e| e.0)?).map_err(|e| e.0)? {
            let f = |k: &str| int(field(row, k).map_err(|e| e.0)?).map_err(|e| e.0);
            let status = match str_(field(row, "status").map_err(|e| e.0)?).map_err(|e| e.0)? {
                "pending" => RequestStatus::Pending,
                "accepted" => RequestStatus::Accepted,
                _ => RequestStatus::Rejected,
            };
            cx.st.alliance_reqs.push(AllianceRequest {
                requestor: SmallId(f("requestor")? as u16),
                recipient: SmallId(f("recipient")? as u16),
                created_at: f("createdAt")? as i32,
                status,
            });
        }
        load_g(g, &mut cx).map_err(|e| format!("G: {}", e.0))?;
        load_shared_water(field(w, "sharedWaterCache").map_err(|e| e.0)?, &mut cx).map_err(|e| e.0)?;
    }
    load_water(&mut st, field(w, "water").map_err(|e| e.0)?).map_err(|e| e.0)?;
    apply_owner_bits(&mut st).map_err(|e| e.0)?;
    let mut ex = ExecStore::default();
    let n_live = arr(field(g, "execs").map_err(|e| e.0)?).map_err(|e| e.0)?.len();
    let n_un = arr(field(g, "unInitExecs").map_err(|e| e.0)?).map_err(|e| e.0)?.len();
    ex.live = keys.execs[..n_live].to_vec();
    st.uninit = keys.execs[n_live..n_live + n_un].to_vec();
    Ok((st, ex, keys.execs))
}

/// Re-export G, T, M and W.
fn export_sections(st: &State, ex: &ExecStore, all_execs: &[ExecId]) -> Result<[Cs; 4], String> {
    let mut cx = CanonCx::with_mode(st, ex, CanonMode::Full);
    // The drain registers detached executions (MIRV warheads) after the listed ones, in
    // E-table order: register every E key so `execTypes` covers them.
    for &id in all_execs {
        cx.exec_key(id);
    }
    let g = g_section(&mut cx);
    if let Some(e) = cx.errors.first() {
        return Err(e.0.clone());
    }
    let mut gf = g.fields;
    gf.push((
        "execTypes".into(),
        Cs::Arr(cx.exec_order.iter().map(|id| Cs::Str(id.kind.as_str().to_owned())).collect()),
    ));
    let w = Cs::Obj(vec![("water".into(), g.water), ("sharedWaterCache".into(), g.shared_water)]);
    Ok([Cs::Obj(gf), t_section(st, CanonMode::Full), m_section(st), w])
}

fn digest(v: &Cs) -> u64 {
    of_canon::digest::row_digest(v).expect("CSE1")
}

fn check(p: &Path) -> Result<(), String> {
    let d = of_trace::trace::read_dump(p).map_err(|e| format!("{e:#}"))?;
    let cs = of_trace::trace::dump_state(&d).map_err(|e| format!("{e:#}"))?;
    let (st, ex, execs) = load_sections(&cs)?;
    let got = export_sections(&st, &ex, &execs)?;
    let mut bad = Vec::new();
    let mut actual = cs.clone();
    for (i, s) in [Section::G, Section::T, Section::M, Section::W].into_iter().enumerate() {
        if digest(&got[i]) != digest(cs.get(s)) {
            bad.push(s.name());
        }
        *actual.get_mut(s) = got[i].clone();
    }
    let bad: Vec<String> = if bad.is_empty() {
        Vec::new()
    } else {
        let diffs = of_trace::localise::diff_states(&cs, &actual, 8);
        vec![format!("sections {bad:?}: {}", diffs.iter().map(|f| f.render()).collect::<Vec<_>>().join(" | "))]
    };
    if bad.is_empty() { Ok(()) } else { Err(bad.join("; ")) }
}

#[test]
#[ignore = "needs the full conformance corpus (not shipped): set OF_CORPUS_DIR"]
fn g_t_m_w_round_trip_every_ci_dump() {
    let all = dumps();
    assert!(all.len() >= 100, "expected the golden CI corpus (≥ 100 dumps), found {}", all.len());
    let mut fails = Vec::new();
    for p in &all {
        if let Err(e) = check(p) {
            fails.push(format!("{}: {e}", p.display()));
        }
    }
    eprintln!("MOD-SNAPSHOT(G,T,M,W): {} dumps, {} failed", all.len(), fails.len());
    assert!(fails.is_empty(), "{} of {} dumps differ:\n{}", fails.len(), all.len(), fails.join("\n"));
}

/// The round trip is not vacuous: corrupting one loaded value changes the re-export.
#[test]
#[ignore = "needs the full conformance corpus (not shipped): set OF_CORPUS_DIR"]
fn round_trip_detects_a_corruption() {
    let p = dumps().into_iter().find(|p| p.ends_with("t700.cs.zst")).expect("a t700 dump");
    let d = of_trace::trace::read_dump(&p).expect("dump");
    let cs = of_trace::trace::dump_state(&d).expect("cs");
    let (mut st, ex, execs) = load_sections(&cs).expect("load");
    st.territory_version += 1;
    let got = export_sections(&st, &ex, &execs).expect("export");
    assert_ne!(digest(&got[0]), digest(cs.get(Section::G)), "G must see territoryVersion");
    st.territory_version -= 1;
    if let Some(s) = st.map.state_buf().as_slice() {
        let t = s.iter().position(|&x| x != 0).unwrap_or(0) as u32;
        let cur = st.map.state_buf().get(t);
        st.map.state_buf_mut().set_raw(t, cur ^ 1);
    }
    let got = export_sections(&st, &ex, &execs).expect("export");
    assert_ne!(digest(&got[1]), digest(cs.get(Section::T)), "T must see one owner bit");
}

/// The full loader (`canon::load::load_into`, TS-restore semantics) over every CI dump, and
/// the full re-export (`Game::canon_full`) compared section by section with the dump. Until
/// the row owners' `Canon`/`Uncanon` impls land, a dump may stop at a *missing* impl (counted
/// as pending, with the impl named); any other error — a loader or exporter bug — fails.
#[test]
#[ignore = "needs the full conformance corpus (not shipped): set OF_CORPUS_DIR"]
fn full_loader_and_exporter_on_every_ci_dump() {
    let all = dumps();
    let (mut ok, mut pending, mut fails) = (0usize, std::collections::BTreeMap::new(), Vec::new());
    for p in &all {
        let d = of_trace::trace::read_dump(p).expect("dump");
        let cs = of_trace::trace::dump_state(&d).expect("cs");
        let base = fresh_base(&cs).expect("base");
        let r = of_engine::canon::load::load_into(base, "g", &cs).and_then(|g| g.canon_full());
        match r {
            Ok(got) => {
                let bad: Vec<&str> = of_canon::digest::SECTIONS
                    .iter()
                    .filter(|s| digest(got.get(**s)) != digest(cs.get(**s)))
                    .map(|s| s.name())
                    .collect();
                if bad.is_empty() {
                    ok += 1;
                } else {
                    fails.push(format!("{}: sections {bad:?} differ", p.display()));
                }
            }
            Err(e) if e.is_missing() => {
                *pending.entry(e.0.split(": ").last().unwrap_or("").to_owned()).or_insert(0) += 1
            }
            Err(e) => fails.push(format!("{}: {e}", p.display())),
        }
    }
    eprintln!(
        "MOD-SNAPSHOT(full): {} dumps: {ok} byte-identical, pending (missing owner impl): {pending:?}",
        all.len()
    );
    assert!(fails.is_empty(), "{} loader/exporter failures:\n{}", fails.len(), fails.join("\n"));
}
