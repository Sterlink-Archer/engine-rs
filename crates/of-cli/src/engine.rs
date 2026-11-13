//! [`OfEngine`]: of-engine behind the of-trace [`Engine`] trait, so the SPEC §3 conformance
//! loop (`of_trace::conform::run`) drives the native port. Owner: **P13**.
//!
//! - `start`: `scenario.json` → `GameStartInfo` (serde edge) + the map files (by path, SHA-256
//!   checked, SPEC §2.2) → `check_map_files` → `MapStatics::load` → `Game::new`. A dump start
//!   (`start.kind == "dump"`, SPEC §3.4 step 2) loads `start.cs` with `Game::from_canon`.
//! - `step`: the CSE1 turn → JSON → typed `Turn` → `Game::step`. `Fatal::Tick` /
//!   `Fatal::Intent` / `Fatal::Setup` are TS throws → `StepError::Fatal` (compared with
//!   `scenario.fatal`); `Fatal::Bug` (a Rust-only panic, including every S0 `todo!()`) →
//!   `StepError::Unimplemented`.
//! - digests: `Game::canon_k` (K never computes S) / `Game::canon_d`, the P13 exporter
//!   (`of_engine::canon`); an owner impl that has not landed yet, or any exporter panic, is
//!   *unimplemented*, never a divergence. `canonical_state` (for field diffs) is the full CS.
//! - `gc_every_tick` (SPEC-L3 GC guard, ARCHITECTURE §3.11): run the v1 GC after every step, so
//!   a conformance run proves the collector unobservable.
//!
//! Map files are resolved under the upstream checkout: `$OF_UPSTREAM_DIR`, else
//! the bundled `<repo>/assets` (Iceland and Australia, CC BY-SA 4.0). `scenario.map.dir` (relative to it) wins; otherwise
//! `resources/maps/<name lowercased, non-alphanumerics removed>` (the fixture writer's rule,
//! `conformance/tools/rs_fixture.ts:101`) or `tests/testdata/maps/<name>` for `source:
//! "testdata"`.

use of_canon::Cs;
use of_canon::digest::{CanonState, Digests};
use of_engine::api::intents::Turn;
use of_engine::api::map_files::MapFiles;
use of_engine::api::types::GameMapSize;
use of_engine::api::{GameStartInfo, game_config::GameConfig};
use of_engine::canon::CanonError;
use of_engine::map::MapStatics;
use of_engine::map::manifest::MapManifest;
use of_engine::runner::{Fatal, check_map_files};
use of_engine::{Game, ctx::TsThrow};
use of_trace::engine::{Engine, StepError};
use of_trace::trace::Trace;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// of-engine as an of-trace engine.
pub struct OfEngine {
    upstream: PathBuf,
    game: Option<Game>,
    steps: u32,
    /// Run `State::collect_garbage` after every step (the GC-every-tick guard).
    pub gc_every_tick: bool,
    /// PRNG draw recorder (`--prng-trace`): told the tick before each step.
    pub prng: Option<crate::prngtrace::Recorder>,
}

impl Default for OfEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// The upstream checkout holding the map files.
pub fn default_upstream_dir() -> PathBuf {
    match std::env::var_os("OF_UPSTREAM_DIR") {
        Some(d) => PathBuf::from(d),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets"),
    }
}

impl OfEngine {
    pub fn new() -> Self {
        OfEngine { upstream: default_upstream_dir(), game: None, steps: 0, gc_every_tick: false, prng: None }
    }
    pub fn with_upstream(upstream: PathBuf) -> Self {
        OfEngine { upstream, game: None, steps: 0, gc_every_tick: false, prng: None }
    }
    /// The game (after a successful `start`).
    pub fn game(&self) -> Option<&Game> {
        self.game.as_ref()
    }
    /// The game, mutably (bench / fork-bench drive it directly).
    pub fn game_mut(&mut self) -> Option<&mut Game> {
        self.game.as_mut()
    }
    /// Replace the game (e.g. a `from_canon` resume), resetting the step counter to `steps`.
    pub fn set_game(&mut self, g: Game, steps: u32) {
        self.game = Some(g);
        self.steps = steps;
    }
}

/// A CS export/load failure as a step error: a missing owner impl or an exporter bug is never
/// a TS behaviour, so both are *unimplemented* (the message says which).
pub fn canon_to_step(what: &str, e: CanonError) -> StepError {
    if e.is_missing() {
        StepError::Unimplemented(format!("{what}: {e}"))
    } else {
        StepError::Unimplemented(format!("{what}: CS v1 exporter/loader error (port bug): {e}"))
    }
}

/// The game a scenario starts from, built from `scenario.json` (fresh or dump start).
pub fn start_game(upstream: &Path, trace: &Trace) -> Result<Game, StepError> {
    let sc = &trace.scenario;
    let gs: GameStartInfo = serde_json::from_value(sc["gameStart"].clone())
        .map_err(|e| StepError::Fatal(format!("gameStart does not parse: {e}")))?;
    let files = load_map_files(upstream, &sc["map"], &gs.config)?;
    check_map_files(&gs, &files).map_err(fatal_to_step)?;
    let statics: Arc<MapStatics> = adapter_guard("MapStatics::load", || MapStatics::load(&files))?
        .map_err(|e| StepError::Fatal(format!("loadTerrainMap: {e}")))?;
    match sc.pointer("/start/kind").and_then(Value::as_str) {
        Some("fresh") | None => Game::new(&gs, statics).map_err(fatal_to_step),
        Some("dump") => {
            let rel = sc
                .pointer("/start/cs")
                .and_then(Value::as_str)
                .ok_or_else(|| StepError::Fatal("scenario.start.cs is missing for a dump start".into()))?;
            let cs = read_dump_state(&trace.dir.join(rel))?;
            from_canon_guarded(&gs, statics, &cs)
        }
        Some(k) => Err(StepError::Unimplemented(format!("start kind {k:?}"))),
    }
}

/// A dump file's full CS (T rebuilt, SPEC §2.5).
pub fn read_dump_state(p: &Path) -> Result<CanonState, StepError> {
    let d = of_trace::trace::read_dump(p).map_err(|e| StepError::Fatal(format!("dump {}: {e:#}", p.display())))?;
    of_trace::trace::dump_state(&d).map_err(|e| StepError::Fatal(format!("dump {}: {e:#}", p.display())))
}

/// `Game::from_canon` behind the adapter's panic guard (an owner's `todo!()` on the setup path
/// is *unimplemented*).
pub fn from_canon_guarded(gs: &GameStartInfo, statics: Arc<MapStatics>, cs: &CanonState) -> Result<Game, StepError> {
    adapter_guard("Game::from_canon", || Game::from_canon(gs, statics, cs))?.map_err(|e| canon_to_step("from_canon", e))
}

fn fatal_to_step(f: Fatal) -> StepError {
    match f {
        Fatal::Tick { msg, .. } | Fatal::Intent { msg } | Fatal::Setup { msg } => StepError::Fatal(msg),
        Fatal::Bug { msg } => StepError::Unimplemented(format!("Rust panic (no TS counterpart): {msg}")),
    }
}

/// Run `f`, turning any panic into `Unimplemented` (an S0/S1 `todo!()` on the setup path, e.g.
/// `MapStatics::load`), a `TsThrow` into `Fatal`.
pub fn adapter_guard<R>(what: &str, f: impl FnOnce() -> R) -> Result<R, StepError> {
    // Conformance-adapter boundary (outside the sim): report, never continue the game.
    #[allow(clippy::disallowed_methods)]
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    r.map_err(|p| {
        if let Some(t) = p.downcast_ref::<TsThrow>() {
            StepError::Fatal(t.msg.to_string())
        } else if let Some(s) = p.downcast_ref::<&'static str>() {
            StepError::Unimplemented(format!("{what}: {s}"))
        } else if let Some(s) = p.downcast_ref::<String>() {
            StepError::Unimplemented(format!("{what}: {s}"))
        } else {
            StepError::Unimplemented(format!("{what}: panic"))
        }
    })
}

/// CSE1 value → the JSON the TS side serialized (`Turn` objects hold no BigInt or typed array).
pub fn cs_to_turn_json(v: &Cs) -> Result<Value, String> {
    Ok(match v {
        Cs::Null => Value::Null,
        Cs::Bool(b) => Value::Bool(*b),
        Cs::Int(i) => Value::from(*i),
        Cs::Num(x) => Value::from(serde_json::Number::from_f64(*x).ok_or_else(|| format!("non-finite number {x}"))?),
        Cs::Str(s) => Value::String(s.clone()),
        Cs::Arr(a) => Value::Array(a.iter().map(cs_to_turn_json).collect::<Result<_, _>>()?),
        Cs::Obj(f) => {
            let mut m = serde_json::Map::new();
            for (k, x) in f {
                if !matches!(x, Cs::Absent) {
                    m.insert(k.clone(), cs_to_turn_json(x)?);
                }
            }
            Value::Object(m)
        }
        Cs::Absent | Cs::Big(_) | Cs::Typed(_) => return Err(format!("unexpected CS value in a turn: {v:?}")),
    })
}

/// The directory of the scenario's map (module docs).
fn map_dir(upstream: &Path, map: &Value) -> Result<PathBuf, StepError> {
    if let Some(d) = map.get("dir").and_then(Value::as_str) {
        return Ok(upstream.join(d));
    }
    let name = map.get("name").and_then(Value::as_str).ok_or_else(|| StepError::Fatal("scenario.map.name".into()))?;
    Ok(match map.get("source").and_then(Value::as_str) {
        Some("testdata") => upstream.join("tests/testdata/maps").join(name),
        _ => {
            let key: String = name.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
            upstream.join("resources/maps").join(key)
        }
    })
}

/// Read the map files for `cfg`'s size, checking each against `scenario.map.files` SHA-256s.
pub fn load_map_files(upstream: &Path, map: &Value, cfg: &GameConfig) -> Result<MapFiles, StepError> {
    let dir = map_dir(upstream, map)?;
    let sums = map.get("files").and_then(Value::as_object);
    let read = |f: &str| -> Result<Vec<u8>, StepError> {
        let p = dir.join(f);
        let b = std::fs::read(&p).map_err(|e| StepError::Unimplemented(format!("map file {}: {e}", p.display())))?;
        if let Some(want) = sums.and_then(|s| s.get(f)).and_then(Value::as_str).filter(|w| !w.is_empty()) {
            let got = hex::encode(Sha256::digest(&b));
            if got != want {
                return Err(StepError::Fatal(format!("map file {} sha256 {got} != scenario {want}", p.display())));
            }
        }
        Ok(b)
    };
    let mut mj: Value =
        serde_json::from_slice(&read("manifest.json")?).map_err(|e| StepError::Fatal(format!("manifest.json: {e}")))?;
    // Test maps have no `nations` key (upstream tests pass nations separately).
    if let Some(o) = mj.as_object_mut() {
        o.entry("nations").or_insert_with(|| Value::Array(Vec::new()));
    }
    let manifest: MapManifest = serde_json::from_value(mj).map_err(|e| StepError::Fatal(format!("manifest: {e}")))?;
    // MapFiles.ts: Normal games use map.bin + map4x.bin, compact ones map4x.bin + map16x.bin.
    let (map_bin, map16x_bin) = match cfg.game_map_size {
        GameMapSize::Normal => (Some(read("map.bin")?), None),
        GameMapSize::Compact => (None, Some(read("map16x.bin")?)),
    };
    Ok(MapFiles {
        map: cfg.game_map.clone(),
        map_size: cfg.game_map_size,
        manifest,
        map_bin,
        map4x_bin: Some(read("map4x.bin")?),
        map16x_bin,
    })
}

impl Engine for OfEngine {
    fn name(&self) -> String {
        "of-engine".into()
    }

    fn start(&mut self, trace: &Trace) -> Result<(), StepError> {
        self.game = None;
        self.steps = 0;
        self.game = Some(start_game(&self.upstream, trace)?);
        Ok(())
    }

    fn step(&mut self, turn: &Cs) -> Result<(), StepError> {
        let g = self.game.as_mut().ok_or_else(|| StepError::Unimplemented("step before start".into()))?;
        let tj = cs_to_turn_json(turn).map_err(StepError::Unimplemented)?;
        let turn: Turn =
            serde_json::from_value(tj).map_err(|e| StepError::Unimplemented(format!("turn does not parse: {e}")))?;
        if let Some(rec) = &self.prng {
            rec.set_tick(u32::try_from(g.state.ticks).unwrap_or(0));
        }
        let r = g.step(&turn);
        self.steps += 1;
        if self.gc_every_tick && r.is_ok() {
            adapter_guard("collect_garbage", || {
                g.state.collect_garbage(&g.execs);
            })?;
        }
        r.map(|_| ()).map_err(fatal_to_step)
    }

    fn tick(&self) -> u32 {
        self.steps
    }

    fn canonical_state(&mut self) -> Option<CanonState> {
        let g = self.game.as_ref()?;
        adapter_guard("canon_full", || g.canon_full()).ok()?.ok()
    }

    fn key_digest(&mut self) -> Result<u64, StepError> {
        let g = self.game.as_ref().ok_or_else(|| StepError::Unimplemented("key_digest before start".into()))?;
        adapter_guard("canon_k", || g.canon_k())?.map_err(|e| canon_to_step("K", e))
    }

    fn full_digests(&mut self) -> Result<Digests, StepError> {
        let g = self.game.as_ref().ok_or_else(|| StepError::Unimplemented("full_digests before start".into()))?;
        adapter_guard("canon_d", || g.canon_d())?.map_err(|e| canon_to_step("D", e))
    }
}
