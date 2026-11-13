//! The embedding API entry points: `Game::new` (= `createGameRunner` + `GameRunner.init`,
//! `engine/src/GameRunner.ts:48-100,183-207`) and `Game::step` (= `GameRunner.executeNextTick`
//! minus view work, `:213-246`), plus the tick loop (`GameImpl.executeNextTick`,
//! `engine/src/game/GameImpl.ts:599-665`), forks and the GC cadence. Owner: **P1**.
//! ARCHITECTURE §4.1-4.6, §8, §10.

pub mod executor;

use crate::StateClone;
use crate::api::game_config::GameStartInfo;
use crate::api::intents::Turn;
use crate::api::map_files::MapFiles;
use crate::api::types::{GameType, PlayerInfo, UnitType};
use crate::canon::CanonState;
use crate::config::EngineConfig;
use crate::ctx::{Ctx, TileJournal, TsThrow};
use crate::exec::{Exec, ExecStore};
use crate::ids::{ExecKind, SmallId};
use crate::map::MapStatics;
use crate::scratch::{Scratch, with_thread_scratch};
use crate::state::gc::GC_EVERY_TICKS;
use crate::state::{State, Winner, create_state, human_player_info};
use of_det::Prng;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;

/// A whole game: state + executions (§3.1).
#[derive(StateClone, Debug, Default)]
pub struct Game {
    pub state: State,
    pub execs: ExecStore,
    /// `Executor.gameID_` (`ExecutionManager.ts:44`): the intent → execution mapping passes it
    /// to `SpawnExecution` (`:84-90`).
    pub game_id: Arc<str>,
    /// Set by `Fatal::Intent`, `Fatal::Setup` and `Fatal::Bug` (§4.6): later steps return it
    /// again. A TS tick throw (`Fatal::Tick`) does **not** set it.
    pub stuck: Option<Fatal>,
}

/// Why `Game::new` or `step` failed (§4.6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fatal {
    /// A TS throw inside the tick (`GameRunner.ts:231-246`): the game stays steppable with TS's
    /// post-throw state. `tick` is the tick that threw (`ticks()` was not incremented).
    Tick { tick: i32, msg: String },
    /// An intent with no execution (`ExecutionManager.ts:147-148`, Q-INTENT-THROW), or a throw
    /// while mapping intents (outside the tick try/catch): stuck, no state change.
    Intent { msg: String },
    /// A TS throw while building the game (`createGameRunner` / `GameRunner.init`).
    Setup { msg: String },
    /// A Rust-only panic (no TS counterpart; includes an S0 `todo!()`): stuck.
    Bug { msg: String },
}
impl crate::plain::PlainState for Fatal {}
crate::refs_leaf!(Fatal);

/// What a step produced (bot early-stop signals, §10).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StepEvents {
    /// `game.ticks()` after the step (the `tick` of TS's `GameUpdateViewData`,
    /// `GameRunner.ts:310`).
    pub tick: i32,
    pub winner_set: Option<Winner>,
    pub killed: Vec<SmallId>,
}

/// The text of a panic payload.
fn panic_msg(p: &(dyn std::any::Any + Send)) -> String {
    if let Some(t) = p.downcast_ref::<TsThrow>() {
        t.msg.to_string()
    } else if let Some(s) = p.downcast_ref::<&'static str>() {
        (*s).to_owned()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_owned()
    }
}

/// Run `f`; a `TsThrow` becomes `ts(msg)`, any other panic `Fatal::Bug`.
fn guarded<R>(f: impl FnOnce() -> R, ts: impl FnOnce(String) -> Fatal) -> Result<R, Fatal> {
    // The two sanctioned catch sites are path/stepper.rs and this runner (PORTING_GUIDE §11).
    #[allow(clippy::disallowed_methods)]
    let r = std::panic::catch_unwind(AssertUnwindSafe(f));
    r.map_err(|p| if p.is::<TsThrow>() { ts(panic_msg(&*p)) } else { Fatal::Bug { msg: panic_msg(&*p) } })
}

/// `loadGameMap`'s check (`GameRunner.ts:103-114`): the files must be the configured map at
/// the configured size. The caller that loads `MapFiles` for a `GameStartInfo` runs it before
/// `MapStatics::load`.
pub fn check_map_files(start: &GameStartInfo, files: &MapFiles) -> Result<(), Fatal> {
    let c = &start.config;
    if files.map != c.game_map || files.map_size != c.game_map_size {
        return Err(Fatal::Setup {
            msg: format!(
                "the game is on {} ({}), the files passed are {} ({})",
                c.game_map,
                c.game_map_size.as_str(),
                files.map,
                files.map_size.as_str()
            ),
        });
    }
    Ok(())
}

/// The SPEC §3.2.6 exec-boundary probe of [`Game::step_with_probe`]: `(index in live, kind,
/// state)` after each executed `tick()` (named for clippy `type_complexity`, merge1).
pub type ExecProbe<'p> = dyn FnMut(usize, ExecKind, &State) + 'p;

impl Game {
    /// `createGameRunner(gameStart, clientID, mapLoader)` + `GameRunner.init()`
    /// (`GameRunner.ts:48-100, 183-207`), with the map already loaded (`statics`, checked by
    /// [`check_map_files`]). Setup PRNG, humans, nations, `createGame`, then the init
    /// executions in TS order, drained into `uninit`.
    pub fn new(start: &GameStartInfo, statics: Arc<MapStatics>) -> Result<Game, Fatal> {
        guarded(|| Self::new_unguarded(start, statics), |msg| Fatal::Setup { msg })
    }

    fn new_unguarded(start: &GameStartInfo, statics: Arc<MapStatics>) -> Game {
        // TS: GameRunner.ts:54 `new EngineConfig(gameStart.config, false, gameStart.listed)`;
        // `listed` undefined → the default parameter `false` (Config.ts:89-94).
        let cfg = Arc::new(EngineConfig::new(start.config.clone(), false, start.listed.unwrap_or(false), false));
        // TS: GameRunner.ts:56
        let mut random = Prng::new(of_det::util::simple_hash(&start.game_id));
        // TS: GameRunner.ts:58-69 — `random.nextID()` is the 4th constructor argument; the
        // earlier ones have no side effects, so one draw per lobby player, in roster order.
        let humans: Vec<PlayerInfo> = start.players.iter().map(|p| human_player_info(p, &random.next_id())).collect();
        // TS: GameRunner.ts:71-77
        let nations = crate::setup::nation_creation::create_nations_for_game(
            start,
            &statics.nations,
            &statics.additional_nations,
            u32::try_from(humans.len()).unwrap_or(u32::MAX),
            &mut random,
        );
        // TS: GameRunner.ts:79-86 `createGame(...)`
        let state = create_state(humans, nations, statics, cfg);
        let mut g =
            Game { state, execs: ExecStore::default(), game_id: Arc::from(start.game_id.as_str()), stuck: None };
        let tribe_names: Vec<String> =
            start.tribes.as_ref().map(|t| t.iter().map(|x| x.name.clone()).collect()).unwrap_or_default();
        g.init(&tribe_names);
        g
    }

    /// `createGame` without `GameRunner.init()` (upstream `tests/util/Setup.ts`): the test
    /// fixtures' entry point. `state` must be at a tick boundary.
    pub fn from_state(state: State, game_id: &str) -> Game {
        Game { state, execs: ExecStore::default(), game_id: Arc::from(game_id), stuck: None }
    }

    /// `GameRunner.init()` (`GameRunner.ts:183-207`): every `addExecution` in TS order, then the
    /// drain (the execs land in `uninit`; none is inited or ticked here).
    fn init(&mut self, purchased_tribe_names: &[String]) {
        let cfg = self.state.cfg.clone();
        let gid: &str = &self.game_id;
        // TS: GameRunner.ts:184-186
        if cfg.game_config().game_type != GameType::Singleplayer {
            self.state.add_execution(Exec::SpawnTimer(crate::exec::spawn_timer::SpawnTimerExec::new()));
        }
        // TS: :187-189 → Executor.nationExecutions() (ExecutionManager.ts:166-172)
        if cfg.spawn_nations() {
            let nations = self.state.nations.clone();
            for nation in nations.iter() {
                self.state.add_execution(Exec::Nation(crate::exec::nation::NationExec::new(gid, nation.clone())));
            }
        }
        // TS: :190-192 → new PlayerSpawner(mg, gameID).spawnPlayers()
        if cfg.is_random_spawn() {
            for e in crate::setup::player_spawner::spawn_players(&self.state, gid) {
                self.state.add_execution(Exec::Spawn(e));
            }
        }
        // TS: :193-197 → Executor.spawnTribes (ExecutionManager.ts:152-161)
        if cfg.bots() > 0.0 {
            let nation_cells: Vec<_> = self.state.nations.iter().filter_map(|n| n.spawn_cell).collect();
            let mut sp = crate::setup::tribe_spawner::TribeSpawner::new(&self.state, gid, &nation_cells);
            // `numTribes` is a zod uint (`GameConfigSchema.bots`).
            let n = cfg.bots() as u32;
            for e in sp.spawn_tribes(&self.state, n, purchased_tribe_names) {
                self.state.add_execution(Exec::Spawn(e));
            }
        }
        // TS: :198
        self.state.add_execution(Exec::WinCheck(crate::exec::win_check::WinCheckExec::new()));
        // TS: :199-201
        if cfg.doomsday_clock_config().enabled {
            self.state.add_execution(Exec::DoomsdayClock(crate::exec::doomsday::DoomsdayClockExec::new()));
        }
        // TS: :202-206 (the rail network is `mg.railNetwork()`, reached through `cx` at run time)
        if !cfg.is_unit_disabled(UnitType::Factory) {
            self.state
                .add_execution(Exec::RecomputeRailCluster(crate::exec::rail_cluster::RecomputeRailClusterExec::new()));
        }
        self.execs.drain_inbox(&mut self.state);
    }

    /// `GameRunner.executeNextTick()` with this thread's `Scratch` and no journal.
    pub fn step(&mut self, turn: &Turn) -> Result<StepEvents, Fatal> {
        with_thread_scratch(|sc| self.step_impl(turn, std::iter::empty(), sc, None))
    }

    /// `step` with a caller-owned `Scratch` and optional tile journal (§10).
    pub fn step_with(
        &mut self,
        turn: &Turn,
        sc: &mut Scratch,
        jr: Option<&mut TileJournal>,
    ) -> Result<StepEvents, Fatal> {
        self.step_impl(turn, std::iter::empty(), sc, jr)
    }

    /// Debug/test injection: the turn's intent executions, then `extra`, then the tick (§10).
    pub fn step_with_execs(&mut self, turn: &Turn, extra: impl IntoIterator<Item = Exec>) -> Result<StepEvents, Fatal> {
        with_thread_scratch(|sc| self.step_impl(turn, extra, sc, None))
    }

    /// `step` with the SPEC §3.2.6 exec-boundary hook: `probe(i, kind, &state)` runs right
    /// after `live[i].tick()` returns (and its `addExecution`s are drained) for every
    /// execution the tick pass runs (`GameImpl.ts:606-613`). For `engine-rs conform --probe`
    /// (request P13 #1); the probe must not need anything but `&State`.
    pub fn step_with_probe(
        &mut self,
        turn: &Turn,
        mut probe: impl FnMut(usize, ExecKind, &State),
    ) -> Result<StepEvents, Fatal> {
        with_thread_scratch(|sc| {
            let p: &mut ExecProbe<'_> = &mut probe;
            self.step_impl_probed(turn, std::iter::empty(), sc, None, Some(p))
        })
    }

    fn step_impl(
        &mut self,
        turn: &Turn,
        extra: impl IntoIterator<Item = Exec>,
        sc: &mut Scratch,
        jr: Option<&mut TileJournal>,
    ) -> Result<StepEvents, Fatal> {
        self.step_impl_probed(turn, extra, sc, jr, None)
    }

    fn step_impl_probed(
        &mut self,
        turn: &Turn,
        extra: impl IntoIterator<Item = Exec>,
        sc: &mut Scratch,
        jr: Option<&mut TileJournal>,
        probe: Option<&mut ExecProbe<'_>>,
    ) -> Result<StepEvents, Fatal> {
        if let Some(f) = &self.stuck {
            return Err(f.clone());
        }
        #[cfg(feature = "prof")]
        let (pf_ph, pf_t0) = (crate::prof::phase(self.state.in_spawn_phase(), self.state.ticks), crate::prof::now());
        // TS: GameRunner.ts:222-224 `addExecution(...createExecs(turn))`: the spread evaluates
        // every createExec first, so a throw leaves the game untouched (and, being outside the
        // try, leaves the runner `isExecuting` forever: stuck).
        let execs =
            match guarded(|| executor::create_execs(&self.state, &self.game_id, turn), |msg| Fatal::Intent { msg }) {
                Ok(Ok(v)) => v,
                Ok(Err(f)) | Err(f) => {
                    self.stuck = Some(f.clone());
                    return Err(f);
                }
            };
        for e in execs {
            self.state.add_execution(e);
        }
        for e in extra {
            self.state.add_execution(e);
        }
        self.execs.drain_inbox(&mut self.state);
        // TS: :225 `currTurn++` (the caller owns the turn sequence).
        #[cfg(feature = "prof")]
        crate::prof::sec(pf_ph, crate::prof::SEC_CREATE, crate::prof::now() - pf_t0);

        sc.prepare();
        // TS: GameImpl.ts:600 `this.updates = createGameUpdatesMap()`.
        self.state.events = Default::default();
        let tick = self.state.ticks;
        // TS: GameRunner.ts:229-246 try { game.executeNextTick() } catch → return false
        match guarded(|| self.execute_next_tick_probed(sc, jr, probe), |msg| Fatal::Tick { tick, msg }) {
            Ok(()) => {}
            Err(f @ Fatal::Tick { .. }) => {
                self.after_tick_throw(sc);
                return Err(f);
            }
            Err(f) => {
                self.stuck = Some(f.clone());
                return Err(f);
            }
        }
        // GC at a tick boundary every GC_EVERY_TICKS ticks (§3.11; unobservable).
        #[cfg(feature = "prof")]
        let pf_g = crate::prof::now();
        let gc = if self.state.ticks % GC_EVERY_TICKS == 0 {
            guarded(|| self.state.collect_garbage(&self.execs), |msg| Fatal::Bug { msg }).map(|_| ())
        } else {
            Ok(())
        };
        #[cfg(feature = "prof")]
        {
            let t = crate::prof::now();
            crate::prof::sec(pf_ph, crate::prof::SEC_GC, t - pf_g);
            crate::prof::step(pf_ph, t - pf_t0);
        }
        if let Err(f) = gc {
            self.stuck = Some(f.clone());
            return Err(f);
        }
        let ev = std::mem::take(&mut self.state.events);
        Ok(StepEvents { tick: self.state.ticks, winner_set: ev.winner_set, killed: ev.killed })
    }

    /// The TS-visible effects of a tick throw that Rust has not yet applied (§4.6): the
    /// throwing execution's `addExecution` pushes and MIRV warhead-list pushes had already
    /// happened in TS, so drain them (into `uninit`) and apply its queued commands; then clear
    /// the per-step outputs. Everything else is already TS-exact: the init loop never vacates
    /// `uninit`, `inited` is appended only after the loop, `ticks` was not incremented.
    fn after_tick_throw(&mut self, sc: &mut Scratch) {
        let issuer = self.execs.running.take();
        let mut cx = Ctx::new(&mut self.state, sc, None);
        self.execs.drain_inbox(cx.st);
        if !cx.st.exec_cmds.is_empty() {
            match issuer {
                Some(id) => self.execs.apply_exec_cmds(&mut cx, id),
                None => cx.st.exec_cmds.clear(),
            }
        }
        self.state.events = Default::default();
        self.execs.buf_inited.clear();
        self.execs.buf_uninited.clear();
    }

    /// `GameImpl.executeNextTick` (`GameImpl.ts:599-665`), §4.2. A `TsThrow` unwinds out of it
    /// with TS's partial state (see `after_tick_throw`).
    pub fn execute_next_tick(&mut self, sc: &mut Scratch, jr: Option<&mut TileJournal>) {
        self.execute_next_tick_probed(sc, jr, None)
    }

    fn execute_next_tick_probed(
        &mut self,
        sc: &mut Scratch,
        jr: Option<&mut TileJournal>,
        mut probe: Option<&mut ExecProbe<'_>>,
    ) {
        let Game { state, execs: ex, .. } = self;
        let mut cx = Ctx::new(state, sc, jr);
        let t = cx.st.ticks;

        // (2) tick pass (GameImpl.ts:606-613): `forEach` captures the length once; nothing
        // appends to `execs` during the pass. `inSpawnPhase()` and `isActive()` are re-read per
        // execution (Q-SPAWNFLIP).
        #[cfg(feature = "prof")]
        let pf_ph = crate::prof::phase(cx.st.in_spawn_phase(), t);
        #[cfg(feature = "prof")]
        crate::prof::set_phase(pf_ph);
        #[cfg(feature = "prof")]
        let (pf_s0, mut pf_t) = {
            let x = crate::prof::now();
            (x, x)
        };
        let n = ex.live.len();
        for i in 0..n {
            let id = ex.live[i]; // in-range: live does not change during the pass
            if (!cx.st.in_spawn_phase() || ex.ar.active_during_spawn_phase(id)) && ex.ar.is_active_in(id, cx.st) {
                ex.running = Some(id);
                ex.ar.tick(id, &mut cx, t);
                ex.after_exec_call(&mut cx, id);
                ex.running = None;
                #[cfg(feature = "prof")]
                {
                    let x = crate::prof::now();
                    crate::prof::tick(pf_ph, id.kind, x - pf_t);
                    pf_t = x;
                }
                if let Some(p) = probe.as_deref_mut() {
                    p(i, id.kind, cx.st); // SPEC §3.2.6 exec-boundary hook
                }
            }
        }

        #[cfg(feature = "prof")]
        let (pf_i0, mut pf_t) = {
            let x = crate::prof::now();
            crate::prof::sec(pf_ph, crate::prof::SEC_TICK, x - pf_s0);
            (x, x)
        };
        // (3) init pass (:614-627): a live array iterator over `unInitExecs`, so executions
        // added during `init` are visited in the same pass. `init` runs even on inactive
        // executions. A throw leaves `uninit` and `live` untouched (§4.6).
        if !cx.st.uninit.is_empty() {
            let mut inited = std::mem::take(&mut ex.buf_inited);
            let mut un_inited = std::mem::take(&mut ex.buf_uninited);
            inited.clear();
            un_inited.clear();
            let mut i = 0;
            while i < cx.st.uninit.len() {
                let id = cx.st.uninit[i]; // in-range: i < len (re-read: inits append)
                if !cx.st.in_spawn_phase() || ex.ar.active_during_spawn_phase(id) {
                    ex.running = Some(id);
                    ex.ar.init(id, &mut cx, t);
                    ex.after_exec_call(&mut cx, id);
                    ex.running = None;
                    #[cfg(feature = "prof")]
                    {
                        let x = crate::prof::now();
                        crate::prof::init(pf_ph, id.kind, x - pf_t);
                        pf_t = x;
                    }
                    inited.push(id);
                } else {
                    un_inited.push(id);
                }
                i += 1;
            }
            ex.live.extend_from_slice(&inited); // TS: this.execs.push(...inited)
            std::mem::swap(&mut cx.st.uninit, &mut un_inited); // TS: this.unInitExecs = unInited
            inited.clear();
            un_inited.clear();
            ex.buf_inited = inited;
            ex.buf_uninited = un_inited;
        }

        #[cfg(feature = "prof")]
        let pf_m = {
            let x = crate::prof::now();
            crate::prof::sec(pf_ph, crate::prof::SEC_INIT, x - pf_i0);
            let _ = pf_t;
            x
        };
        // (4) compaction (:629, :757-773).
        ex.remove_inactive_executions(cx.st);
        #[cfg(feature = "prof")]
        let pf_m = {
            let x = crate::prof::now();
            crate::prof::sec(pf_ph, crate::prof::SEC_REMOVE, x - pf_m);
            x
        };

        // (5) per-player bookkeeping (:631-648): stats samples under `stats-full`; the
        // `toUpdate` view work is dropped (ARCHITECTURE §5).
        cx.per_player_bookkeeping();
        #[cfg(feature = "prof")]
        let pf_m = {
            let x = crate::prof::now();
            crate::prof::sec(pf_ph, crate::prof::SEC_BOOK, x - pf_m);
            x
        };
        // (6) the Hash update every 10 ticks (:649-655) is view-only (§5): `legacy_hash` is on
        // demand.

        // (7) water (:656-662): flush pending conversions + throttled graph rebuild.
        let water_changed = cx.water_tick(t);
        for &tile in &water_changed {
            cx.journal_tile(tile); // TS: recordTileUpdate(tile)
        }
        if !water_changed.is_empty() {
            cx.st.structure_version += 1; // TS: :662
        }
        #[cfg(feature = "prof")]
        let pf_m = {
            let x = crate::prof::now();
            crate::prof::sec(pf_ph, crate::prof::SEC_WATER, x - pf_m);
            x
        };
        // Rust-only, unobservable (TileSet compaction etc., §3.6.2).
        cx.end_of_tick_maintenance();
        #[cfg(feature = "prof")]
        crate::prof::sec(pf_ph, crate::prof::SEC_MAINT, crate::prof::now() - pf_m);
        // (8) TS: :663
        cx.st.ticks += 1;
    }

    /// The primary fork entry: `dst.clone_from(self)` (pooled; §8.1).
    pub fn clone_into(&self, dst: &mut Game) {
        dst.clone_from(self);
    }

    /// GC on the source first (§3.11, §8.1 item 6), then `clone_into`.
    pub fn fork_into(&mut self, dst: &mut Game) {
        self.state.collect_garbage(&self.execs);
        self.clone_into(dst);
    }

    /// CS v1 of the current state (SPEC §1): P13's full export (`canon_full`), `None` when an
    /// owner's `Canon` impl is still missing (request P13 #3, merge1).
    pub fn canonical_state(&self) -> Option<CanonState> {
        self.canon_full().ok()
    }

    /// Cheap fork-identity check (scalars, counters, exec lists, players' scalars, sampled
    /// tiles; ARCHITECTURE §10). **Not** CS: equal fingerprints do not prove equal state, but
    /// a clone and its source must agree every tick.
    pub fn fingerprint64(&self) -> u64 {
        let st = &self.state;
        let mut h = Fnv64::new();
        h.i64(i64::from(st.ticks));
        h.i64(st.start_tick.map_or(-1, i64::from));
        h.u64(st.players.len() as u64);
        h.u64(u64::from(st.next_player_id));
        h.u64(u64::from(st.next_unit_id));
        h.u64(u64::from(st.next_alliance_id));
        h.u64(u64::from(st.units_version));
        h.u64(u64::from(st.territory_version));
        h.u64(u64::from(st.structure_version));
        h.u64(u64::from(st.mirvs_launched));
        h.u64(u64::from(st.is_paused));
        h.i64(i64::from(st.ship_staggers.trade_ship));
        h.i64(i64::from(st.ship_staggers.transport_ship));
        match &st.winner {
            Winner::None => h.u64(0),
            Winner::Player(p) => h.u64(1 << 32 | u64::from(p.0)),
            Winner::Team(t) => h.bytes(t.as_bytes()),
        }
        h.u64(st.units.len() as u64);
        h.u64(st.attacks.len() as u64);
        h.u64(st.alliances.len() as u64);
        h.u64(st.alliance_requests.len() as u64);
        for p in &st.players {
            h.u64(u64::from(p.small_id.0));
            h.i64(i64::from(p.last_tile_change));
            h.u64(u64::from(p.tile_change_version));
            h.i64(p.gold);
            h.i64(p.troops);
            h.u64(p.tiles.size() as u64);
            h.u64(p.border_tiles.size() as u64);
            h.u64(p.units.len() as u64);
        }
        // Exec lists by kind (slots are not state; kinds and order are).
        h.u64(self.execs.live.len() as u64);
        for id in &self.execs.live {
            h.u64(id.kind as u64);
        }
        h.u64(st.uninit.len() as u64);
        for id in &st.uninit {
            h.u64(id.kind as u64);
        }
        // Every 61st tile word of the main map (owner, fallout, defense bonus).
        if let Some(words) = st.map.state_buf().as_slice() {
            h.u64(words.len() as u64);
            for w in words.iter().step_by(61) {
                h.u64(u64::from(*w));
            }
        }
        h.u64(u64::from(st.map.water_version));
        h.u64(u64::from(st.water.water_graph_version));
        h.finish()
    }
}

/// FNV-1a 64 over little-endian words (the fingerprint only; CS digests are FNV64W in
/// of-canon).
struct Fnv64(u64);

impl Fnv64 {
    fn new() -> Self {
        Fnv64(0xcbf2_9ce4_8422_2325)
    }
    fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 ^= u64::from(x);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    fn u64(&mut self, x: u64) {
        self.bytes(&x.to_le_bytes());
    }
    fn i64(&mut self, x: i64) {
        self.bytes(&x.to_le_bytes());
    }
    fn finish(&self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests;
