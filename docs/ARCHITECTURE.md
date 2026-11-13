# of-engine: architecture of the bit-exact Rust port

Status: **design v2, 2026-11-05, design only (no Rust source yet).** v2 folds in two reviews: fidelity (review 0) and performance/clone (review 1). Each point and what was done with it is in the [Review log](#review-log) at the end. Engine pin: `upstream/` @ `5b75bd3ad8f66325a727f7bcec47680a0cc07e63`.

Companion documents:
- [`PORTING_GUIDE.md`](PORTING_GUIDE.md): translation rules and the trap checklist.
- [`FILE_MAP.md`](FILE_MAP.md): every TS file mapped to a Rust module, plus the 13-package work split.

Normative inputs:
- `conformance/docs/SPEC.md`: CS v1, the trace format and conformance levels L0–L4.
- `conformance/docs/VECTORS.md`: tier-(a) golden vectors.
- `conformance/schema/snapshot_schemas_5b75bd3.txt`: the authoritative list of state fields (65 record types, 44 of them executions).

Paths without a prefix are relative to `upstream/packages/`. Labels:
- **[V]** verified in the TS source at the pin. The citation is `file:line`. v2 re-verified every claim a review relied on.
- **[D]** a design decision made by this document.
- **[E]** an estimate. Each [E] has a measurement gate in §12.
- **UNVERIFIED** means not checked.

---

## 0. TL;DR

1. **One crate, `of-engine`**, sits on top of the conformance workflow's crates and never re-implements anything they own:
   - `of-det`: DetMath, sfc32, JS semantics, `JsMap`/`JsSet`, TimSort
   - `of-canon`: CS v1 tree, CSE1, FNV64W
   - `of-trace`: scenario I/O.

   A small proc-macro crate, `of-engine-derive`, provides `#[derive(StateClone)]` (§8). Nation AI is a module, not a separate crate (§2).
2. **State is plain data.** `Game = { state: State, execs: ExecStore }`. Every part is `StateClone`, a `Clone` whose `clone_from` forwards field by field so pooled forks reuse memory.
   - Objects that TS identifies by pointer become integer handles:
     - players: `SmallId(u16)`
     - units, attacks and rail clusters: generational arena slots
     - alliances, stations and railroads: their TS ids
     - alliance requests: append-only indexes
     - executions: `ExecId { kind, idx }` into per-kind arenas (§4.2).
   - There is no `Rc`, no `RefCell` and no `Box<dyn>` in `State` or `ExecStore`.
   - Immutable map data sits behind one `Arc` per game, and the water graph and the map-sized connected components sit behind copy-on-write `Arc`s.
   - Map-sized scratch (BFS visited arrays, A* buffers, traversal stamps) lives in a `Scratch` struct that is passed down through a `Ctx` (§3.8). It is never cloned and never reached through TLS on the hot path.
   - A fork is `src.clone_into(&mut pooled_dst)`. Revised estimate: **≈17–22 MB copied, ≈1.5–3 ms pooled, 3–6 ms unpooled on World mid-game [E]**, against a 5 ms target. Pooling is mandatory, not optional (§8).
3. **The execution loop reproduces `GameImpl.executeNextTick` exactly** (`engine/src/game/GameImpl.ts:599-665`) [V]. That covers order, the per-execution spawn-phase re-read, init over a live-growing `uninit` list, and compaction. It also covers **TS's post-throw state**: after a tick throws, `uninit` is unchanged, no execution was moved to the live list, and the game keeps stepping (§4.6).
   - Executions live in per-kind arenas, outside `State`. While one executes, it gets `&mut Ctx` (state plus scratch).
   - New executions go to an inbox in `State`, which the driver drains after every `tick`/`init` call.
   - The only execution-to-execution mutation is MIRV cancelling its warheads (`engine/src/execution/MIRVExecution.ts:179-187`). It goes through a command applied right after the MIRV tick returns. That is equivalent, because the cancels are MIRV's last statements (§4.3).
4. **Lists that TS deletes from with `filter()` are iterated as a snapshot** when the loop body can delete from them (§4.7).
   - TS replaces the array, so a `for..of` already running keeps walking the old array.
   - Three hot sites are affected: `PlayerExecution.ts:60-79` and `:107-111`, and `AttackExecution.ts:169-178`.
5. **`AiAttackBehavior` keeps its virtual dispatch.**
   - The base logic is generic over a 7-method `AiAttackHooks` trait. `TribeExec` uses the defaults; `NationExec` overrides all seven (`execution/nation/NationAttackBehavior.ts:1032-1272`) [V].
   - The trait is frozen at S0 (§4.4).
6. **Water pathfinding state is per game and literal (§3.8):**
   - There is one HPA, and it always reads the *current* graph.
   - Stale ships differ only in their cached path, their chain kind and their chain version's `WaterPathMemo`.
   - The memo is ported literally as per-game state. It is LRU-observable, so it is not a pure cache.
   - The per-edge segment cache is shared across forks only for fills made on unedited terrain. Each game keeps its own "filled" bitset and post-edit overlay.
7. **View work is dropped** except one state write. `UnitImpl.warshipState()` **writes** `_warshipState.isInCombat` (`engine/src/game/UnitImpl.ts:437-442`) [V], and every `toUpdate()` calls it, including through `touch()` (`:145-147`), which executions call. The port keeps the `st.unit_touched(u)` shim at every site (§5).
8. **Numbers:**
   - JS `number` → `f64`, except provably-integral values (tile refs, ids, ticks, counts), which keep JS `/`, `%` and overflow semantics (PORTING_GUIDE §2).
   - BigInt → **`i64`** with JS semantics. Conversions are strict: `BigInt(x)` throws on non-integer, NaN or ±Inf in TS, and the port raises a `TsThrow` panic in every build (§7.2).
9. **TS throws → `TsThrow` panics; everything else is a fatal Rust bug.** The stepper's try/catch maps only `TsThrow` to `NOT_FOUND` and re-raises any other panic (§4.6). That way a Rust-only panic such as an OOB index, an overflow or an assert can never turn into a silent path change.
10. **Conformance is built in.**
    - Every state struct implements `Canon` (CS v1 export and import) and `RefVisitor`.
    - The mark-sweep arena GC is **in v1**: it runs every 32 ticks and before every fork. It uses the same visitor that drives CS's dead-but-referenced rows.
    - `of-cli conform` implements SPEC §3, including the exec-boundary probe and a per-nation PRNG draw trace (§9).
11. **Embedding API** (§10):
    - `Game::new`
    - `step(&Turn) -> Result<StepEvents, Fatal>` with typed intents
    - `clone_into(&self, &mut Game)` (the primary fork entry)
    - a zero-copy read API with the TS legality oracles
    - an optional changed-tile journal, owned by the caller and never cloned.

---

## 1. Requirements and the decisions they force

| # | Requirement (priority order) | Forced decision [D] |
|---|---|---|
| R1 | Bit-for-bit identical to TS @5b75bd3. Checked at L2 (K every tick) and L3 (D plus dumps) per SPEC §0 | Port the *algorithms and their iteration orders* literally, including caches whose contents are observable. TTL caches, the LRU water memo and the per-game edge-segment cache are state (§3.8). Memos keyed on versions are ported as they are. Every container whose iteration order can reach the sim is JS-ordered (`JsMap`/`JsSet`/`OrderedTileSet`). Array-replacing `filter` keeps its old-array iteration semantics (§4.7). |
| R2 | Fork ≤ 5 ms on World, many forks | Flat data with handles, not pointers. Per-kind execution arenas with a `Copy` live list. `StateClone` everywhere, with field-wise `clone_from`. Small inline JS containers for the many tiny maps. Arena GC in v1. O(1) `Arc` refcount updates per fork. Pooled `clone_into` as the primary API. No per-fork rebuild of anything: the TS restore spends ~59 ms rebuilding the WaterManager (doc 08 §7.3). |
| R3 | ≥ 5× TS single-core ticks/s on World FFA, many games per core, no GC | No view work. Allocation-free tick: reused buffers in `Scratch` and double-buffered exec lists. i64 instead of BigInt. Fx-hashed lookups. Struct-of-arrays where hot (§11). Algorithmic speedups only behind a conformance gate. |
| R4 | Portable to aarch64 (the Grace-Hopper box) | Plain IEEE-754 `f64`/`f32` with no FMA contraction (Rust never contracts without explicit `mul_add`). No `std` transcendental math: all of it goes through `of-det` DetMath. No hash-order dependence: `HashMap` iteration is banned by the clippy config. No `target-cpu`-dependent results. |

What the engine is not:
- no client, no renderer and no server
- no snapshot byte codec (CS v1 via `of-canon` replaces it)
- no takeover/singleplayer-conversion hooks: `GameImpl.takeoverPlayer`/`convertHumanToNation`/`applySingleplayerConfig`, `engine/src/game/GameImpl.ts:798-900` [V]. They are not reachable from a `Turn`.

---

## 2. Crate layout

```
engine-rs/
  Cargo.toml                 workspace; rust-toolchain.toml pins 1.96.0
  clippy.toml                disallowed-types/methods (PORTING_GUIDE §12)
  crates/
    of-det/                  [conformance workflow] DetMath, Prng (sfc32), js:: (round, int32, imul, fround,
                             min/max, toString(36)), bigint (checked i128), util (simpleHash, toInt, sigmoid…),
                             timsort (js_sort_by = V8 TimSort), heap (FlatBinaryHeap), ordmap (JsMap/JsSet).
                             of-engine's required additions are contributed HERE (ledger t088, §12 G7).
    of-canon/                [conformance workflow] CsValue tree, CSE1 encode/decode, FNV64W, section/K/D digests
    of-trace/                [conformance workflow] scenario.json, turns.cse.zst, digests.*.bin, dumps
    of-engine-derive/        [this design] proc-macro: #[derive(StateClone)] (Clone + field-wise clone_from)
    of-engine/               [this design] the simulation (no I/O, no threads; one thread_local Scratch fetched
                             once per step, §3.8)
      src/
        lib.rs               pub use of the embedding API (§10); static_assertions::assert_impl_all!(Game: Send)
        api/                 engine-api: types.rs (enums, PlayerInfo), intents.rs (typed Intent + serde edge),
                             game_config.rs (GameConfig + zod defaults), map_files.rs, views.rs (PlayerLike/UnitLike)
        ids.rs               SmallId, UnitRef, AttackRef, AllianceId, ReqIdx, StationId, RailroadId, ClusterRef,
                             ExecId/ExecKind, PlayerInfoIdx
        arena.rs             Arena<T>: generational slots, free list, sweep(marks)
        ctx.rs               Ctx / Rcx (state + scratch + optional journal), TsThrow helpers re-exported
        jsarr.rs             JsArr<T> (TS array field replaced by filter) + snapshot iteration (§4.7)
        tileset.rs           OrderedTileSet (generic, TS TileSet semantics) + PlayerTileOrder (global position index)
        jsorder.rs           re-exports of of-det JsMap/JsSet/SmallJsMap/SmallJsSet + LookupMap wrapper
        map/                 game_map.rs (TileStateBuf with write accessors, §8.5), loader.rs, manifest.rs,
                             maps_gen.rs (generated), unit_grid.rs
        config/              config.rs + engine_config.rs (one struct), unit_info.rs, doomsday.rs, veterancy.rs
        setup/               nation_creation.rs, nation_count.rs, team_assignment.rs, tribe_spawner.rs,
                             tribe_names.rs, player_spawner.rs
        state/               game.rs (State = GameImpl data), player.rs, unit.rs, attack.rs, alliance.rs,
                             alliance_request.rs, stats.rs, shared_water_cache.rs, transport_ship_utils.rs, gc.rs
        water/               manager.rs, abstract_graph.rs (+ SegVisibility), components.rs, hpa.rs,
                             astar_abstract.rs, priority_queue.rs, bfs.rs, bfs_grid.rs
        path/                pathfinder.rs (WaterPathFinder, ChainSlots, WaterPathMemo), stepper.rs,
                             transform/{component_check,smoothing,shore,minimap}.rs, astar*.rs, air.rs, parabola.rs,
                             station.rs, line.rs, spatial_query.rs, types.rs
        rail/                network.rs, station.rs, railroad.rs, grid.rs
        exec/                mod.rs (Exec transport enum, ExecKind, ExecArenas, ExecBehavior), one file per
                             execution; nation/ (mod.rs NationExec + one file per behaviour), tribe.rs,
                             ai_attack.rs (AiAttackBase + AiAttackHooks + generic base methods)
        runner.rs            Game::new (= createGameRunner + init), step (= GameRunner.executeNextTick minus view),
                             executor.rs (= ExecutionManager)
        scratch.rs           Scratch: one field per TS scratch object (§3.8)
        canon/               CS v1 export/import (§9), RefVisitor, key assignment
        view.rs              read-only embedding API (§10)
    of-cli/                  bin: `engine-rs conform --scenario <dir> --json` (SPEC §3.4), `bench`, `fork-bench`
```

**Why not an `of-nations` crate** [D]:
- A nation is an `ExecKind::Nation` slot in `ExecArenas`, so `ExecStore` has to know every execution type.
- Splitting nations out would need `Box<dyn>` executions (no plain clone, no exhaustive CS export) or a generic `Game<E>`. Neither is worth it at 45 k lines: one crate builds in about a minute.
- Parallel porting is isolated by **frozen skeleton signatures plus one git worktree per package**, not by crates (FILE_MAP §3).

**External crates** (all pure Rust, aarch64-clean):
- `serde` and `serde_json` (GameStartInfo, manifest and intents, at the edge only)
- `rustc-hash` (`FxHashMap` for lookups only, never iteration)
- `smallvec`
- `static_assertions`
- `syn`/`quote`/`proc-macro2` (in `of-engine-derive` only)
- `mimalloc` (in `of-cli` only)
- `zstd`, via `of-trace` only.

`of-det` stays dependency-free, so its Fx hasher is a 20-line inline copy (§12 G7).

No `indexmap`: its `shift_remove` keeps order, but it cannot be mutated during iteration with JS semantics (PORTING_GUIDE §5).

---

## 3. State model

### 3.1 Top level: `Game`, `State`, `ExecStore`

```rust
#[derive(StateClone)]                    // Clone + field-wise clone_from (§8)
pub struct Game {
    pub state: State,
    pub execs: ExecStore,                // arenas + live list (§4.2); disjoint from State while an exec runs
    stuck: Option<Fatal>,                // set by Q-INTENT-THROW, Fatal::Setup and Fatal::Bug (§4.6), returned by
                                         // every later step; a TS tick throw does NOT set it (S0: was `bool`)
}

#[derive(StateClone)]
pub struct State {
    // --- clock / phase ---
    pub ticks: i32,                      // GameImpl._ticks (:133) [V]
    pub start_tick: Option<i32>,         // GameImpl.startTick: None = spawn phase (:134, :536-552) [V]
    // --- statics (shared, immutable; one Arc each per game, §8.1) ---
    pub cfg: Arc<EngineConfig>,          // §6
    pub statics: Arc<MapStatics>,        // dims, pristine terrain, land index, nations, manifest, spawn areas,
                                         // version-0 WaterGraph + components (§3.8)
    pub infos: Arc<Vec<PlayerInfo>>,     // indexed by PlayerInfoIdx; COW-appended by add_player (spawn time only)
    pub humans: Arc<[PlayerInfo]>, pub nations: Arc<[Nation]>,   // GameImpl._humans/_nations (ctor :182-190)
    // --- tiles ---
    pub map: GameMap, pub mini: GameMap, // §3.6
    // --- entities ---
    pub players: Vec<Player>,            // index = smallID-1; == _playersBySmallID; == _players Map order (§3.2)
    pub units: Arena<Unit>,              // §3.2
    pub unit_by_id: LookupMap<u32, UnitRef>,   // GameImpl._unitMap (:159); iteration order == ascending id (§3.2)
    pub attacks: Arena<Attack>,
    pub alliances: Vec<Alliance>,        // index == alliance id (nextAllianceID, :489) [V]
    pub alliance_reqs: Vec<AllianceRequest>,  // append-only; index = creation order
    pub alliance_requests: JsArr<ReqIdx>,     // GameImpl.allianceRequests (:146): filter-reassigned (:469,:507), §4.7
    pub unit_grid: UnitGrid,             // §3.7
    pub rail: RailNetwork,               // §3.9
    pub water: WaterManager,             // §3.8 (incl. chain slots, memo, segment visibility)
    pub shared_water: SharedWaterCache,  // TTL cache = state (W section)
    pub stats: Stats,                    // §3.10
    // --- counters / misc (all in CS G) ---
    pub next_player_id: u32, pub next_unit_id: u32, pub next_alliance_id: u32,   // :148-149, :167
    pub player_teams: Vec<Team>, pub bot_team: Team,                            // :163-164
    pub is_paused: bool, pub winner: Winner,                                    // :174-175
    pub units_version: u32, pub territory_version: u32, pub structure_version: u32,  // :1345, :1378, :1358
    pub ship_staggers: ShipStaggers, pub nation_mirv_targets: JsMap<SmallIdKey, i32>, pub mirvs_launched: u32,
    pub plan_driven_unit_ids: JsSet<u32>,  // §5: kept, gates the isInCombat shim, in CS G
    // --- execution plumbing (§4.2) ---
    pub uninit: Vec<ExecId>,             // GameImpl.unInitExecs (:136), order is state
    pub exec_inbox: Vec<Exec>,           // add_execution target; drained after every tick/init call; empty at boundaries
    pub exec_cmds: Vec<ExecCmd>,         // MIRV bind/cancel/unpin (§4.3); empty at boundaries
    // --- derived memos kept for exactness (cloned; one backing Vec each, §3.3) ---
    pub units_by_type_memo: TypeMemo, pub unit_count_memo: [Memo<i32>; 16],  // :349-428
    // --- per-step outputs (cleared at step start; never read by the sim) ---
    pub events: StepEventsBuf,           // §10
}
```

The optional changed-tile journal is **not** in `State`. It is owned by the caller and passed in through `Ctx` (§10), so it is never cloned.

Mapping of every `GameImpl` field (`engine/src/game/GameImpl.ts:133-180`) [V]:

| TS field | Rust | Class | Why |
|---|---|---|---|
| `_ticks`, `startTick` | `ticks`, `start_tick` | state | CS G |
| `unInitExecs`, `execs` | `state.uninit`, `Game.execs.live` | state | Order is state (CS G.execs, G.unInitExecs) |
| `_players` Map, `_playersBySmallID` | `players: Vec<Player>` | state | `addPlayer` pushes to both in the same call with `nextPlayerID++` (`:906-918`) [V], so Map order == smallID order. **This holds only when every id is new**: a duplicate id would overwrite the Map entry in place and still append to `_playersBySmallID`. `add_player` asserts that `id` is new (a plain panic, not `TsThrow`: TS would silently split the two orders, which the port does not model). `units()`, `unitCount()` and `hash()` iterate the Map (`:362-405`). |
| `_width`, `_height`, `_terraNullius` | `statics`; owner `0` | static | `TerraNulliusImpl.smallID()==0`, `id()==null` (`engine-lib/src/game/TerraNulliusImpl.ts`) |
| `allianceRequests` | `alliance_requests: JsArr` | state | CS G. Removal is by `filter` reassignment (`:469`, `:507`) [V] |
| `nextPlayerID`, `_nextUnitID`, `nextAllianceID` | counters | state | CS G |
| `updates`, `tileUpdatePairs`, `viewResyncQueued`, `playerStatsQuads`, `attackTroopsQuads`, `motionPlanRecords`, `_nukeImpactQueue` | **dropped** (journal optional) | view | Upstream `DERIVED_FIELDS` (`tests/util/Snapshot.ts:133-145`) [V], §5 |
| `planDrivenUnitIds` | `plan_driven_unit_ids` | state | CS G. Read by `maybeAddUnitUpdate` (`:717-725`), which gates the shim (§5) |
| `unitGrid` | `unit_grid` | state | CS G.unitGrid (bucket order and in-bucket order are state) |
| `_unitMap` | `unit_by_id` | state | Lookup only. Insertion order == ascending id, because `nextUnitID()` and `addUnit` are called only from `PlayerImpl.buildUnit` (`engine/src/game/PlayerImpl.ts:1413-1428`) [V] |
| `playerTeams`, `botTeam` | same | state | |
| `_railNetwork` | `rail` | state | §3.9 |
| `_mirvsLaunched`, `shipStaggers`, `_nationMirvTargets` | same | state | |
| `_isPaused`, `_winner` | same | state | |
| `_waterManager`, `_sharedWaterCache` | `water`, `shared_water` | state (+ shared `Arc`s) | §3.8 |
| `_teamGameSpawnAreas` | `statics` | static | |
| `_unitsVersion`, `_territoryVersion` | same | state | CS G (TTL caches and memos read them) |
| `_structureVersion` | `structure_version` | derived but cloned | Keys the rail preview memo only (`DERIVED_FIELDS`). Kept because it is one u32. |
| `unitsByTypeMemo`, `unitCountMemo` | memo structs | derived, cloned | Exact memos (`:383-392`, `:418-427`). Layout in §3.3. |
| `borderNbuf` | stack buffer | scratch | |

### 3.2 Identity: how TS object references become handles

TS executions and entities hold direct object references (doc 01 §8.3). The rule [D]:
- **Every reference becomes a `Copy` handle, and every TS-visible identifier is kept as a field.**
- A handle is never assumed to equal the TS id.
- Deletion follows TS exactly: an object that a TS execution still references keeps its data readable after it is "deleted".

| TS object | TS identity source | Rust handle | Storage | Deletion / reuse in TS | Rust deletion | CS key (SPEC §1.2) |
|---|---|---|---|---|---|---|
| Player | `smallID` from `nextPlayerID++` (`GameImpl.ts:906-918`) [V] | `SmallId(u16)`, where 0 = TerraNullius. `Owner = SmallId` covers `Player \| TerraNullius`. | `Vec<Player>` | Never removed. Dead players stay. | Never | smallID |
| Unit | `_id` from `nextUnitID()` (`GameImpl.ts:289-293`), monotonic | `UnitRef { slot: u32, gen: u32 }` | `Arena<Unit>`, `unit_by_id` | `delete()` marks it inactive and removes it from the owner list (by `filter` reassignment, `UnitImpl.ts:353`), the grid and `_unitMap` (`UnitImpl.ts:344-357`, `GameImpl.ts:1388-1397`) [V]. Executions keep reading it (`NukeExecution.nuke`, …). The id is never reused. | Slot stays allocated while reachable. **v1 GC** frees unreachable inactive units (§3.11). `gen` detects stale use in debug builds. | `unit.id` |
| Attack | 8-char base-36 `nextID()` from the attacker's PRNG (`PlayerImpl.ts:1879-1898`, `PseudoRandom.ts:73-78`) [V] | `AttackRef { slot, gen }`; the id is kept as `u64` (value < 36^8) | `Arena<Attack>` | Created only by `AttackExecution.init` (`AttackExecution.ts:139`) [V]. `delete()` filter-reassigns both players' lists (`AttackImpl.ts:61-71`) [V]. The exec keeps the attack. | v1 GC | (attacker, id string) |
| Alliance | `id` from `nextAllianceID++` (`GameImpl.ts:483-490`) [V] | `AllianceId(u32)` == TS id | `Vec<Alliance>` | `detachAlliance` filter-reassigns both players' lists (`:1140-1145`) [V]. `_expiredAlliances` and executions keep it. | Never freed (≈40 B each) | id |
| AllianceRequest | none (object identity) | `ReqIdx(u32)` = creation index | `Vec<AllianceRequest>` | Filtered out of `allianceRequests` on accept or reject (`:468-518`). It stays in `pastOutgoingAllianceRequests` forever. | Never freed (12 B) | `"req:rec:createdAt"` |
| TrainStation | `id` from `StationManager.nextId++`, starting at 1 (`RailNetworkImpl.ts:42-48`) [V] | `StationId(u32)` == TS id | `Vec<Option<TrainStation>>` by id | `removeStation` clears `stationsById[id]` and pops trailing holes (`:51-66`). `nextId` never decreases, so ids are never reused. | Slot = id. Removed stations stay readable while referenced. | id |
| Railroad | `id` from `RailNetworkImpl.nextId` (`:303`) | `RailroadId(u32)` | `Arena<Railroad>` + id → slot `LookupMap` | | v1 GC (with its `tiles`) | id |
| Cluster | none (`new Cluster()`, `:348`) | `ClusterRef { slot, gen }` | `Arena<Cluster>` | Unreferenced clusters become garbage | v1 GC | `c<min station id>` / `e<n>` |
| Execution | object identity | `ExecId { kind: ExecKind(u8), idx: u32 }` (8 B, `Copy`) | `ExecArenas` (one `Slots<T>` per kind, §4.2) | Compacted out when inactive (`:757-773`). MIRV keeps warhead refs (`MIRVExecution.ts:85,298`). | Slot freed on compaction unless pinned (MIRV warheads, §4.3) | list position (SPEC §1.2.1) |
| PseudoRandom | object identity, **shared**: one NationExecution PRNG is passed to 7 behaviours (`NationExecution.ts:270-321`) [V] | none: a plain `Prng` value owned by the exec; behaviours reach it through `&mut self` | inline | | | (inside E rows) |

**Strings.**
- PlayerIDs are kept as `Arc<str>` inside `PlayerInfo`. They are used for `simpleHash` seeds, intents and CS.
- Internal maps keyed by PlayerID are keyed by `SmallId`, because id→smallID is a bijection (`GameImpl.player()`, `:931-937`).
- Attack ids are `u64` and are rendered to base-36 only at the edges (intents, CS).
- A string that can hold arbitrary UTF-16 (names that reach `simpleHash`) must be hashed over UTF-16 code units: see PORTING_GUIDE §9.

### 3.3 Player (`engine/src/game/PlayerImpl.ts:135-208, 1968-2040`) [V]

```rust
#[derive(StateClone)]
pub struct Player {
    pub small_id: SmallId, pub info: PlayerInfoIdx, pub team: Option<Team>,   // index into State.infos (no Arc per player)
    pub last_tile_change: i32, pub tile_change_version: u32,
    pub rng: Prng,                                   // PseudoRandom(simpleHash(id)) (:219)
    pub gold: i64, pub troops: i64,                  // bigint (:141-142), §7.2
    pub trade_gold: i64, pub train_gold: i64, pub piracy_gold: i64, pub gold_earned: i64,
    pub marked_traitor_tick: i32, pub marked_doomsday_clock_tick: i32, pub rotted_at_tick: i32, pub betrayal_count: i32,
    pub embargoes: SmallJsMap<SmallIdKey, Embargo, 4>,      // Map<PlayerID, Embargo> (:159)
    // tiles/borderTiles: order lives in State-level PlayerTileOrder (§3.6.2); per-player header only
    pub tiles: TileListHdr, pub border_tiles: TileListHdr,
    pub units: JsArr<UnitRef>,                       // JS array, filter-REASSIGNED on delete/setOwner (§4.7)
    pub my_units_version: u32,                       // _myUnitsVersion
    pub my_units_memo: TypeMemo, pub my_unit_count_memo: [Memo<i32>;16], pub my_units_owned_memo: [Memo<i32>;16],
    pub past_outgoing_alliance_requests: Vec<ReqIdx>, pub expired_alliances: Vec<AllianceId>,
    pub targets: Vec<Target>, pub outgoing_emojis: Vec<EmojiMessage>,
    pub outgoing_quick_chats: SmallJsMap<i32, i32, 4>, pub sent_donations: Vec<Donation>,
    pub relations: SmallJsMap<SmallIdKey, f64, 8>,   // Map<Player, number> (:194): key by smallID, JS order
    pub last_delete_unit_tick: i32, pub last_embargo_all_tick: i32,
    pub incoming_attacks: JsArr<AttackRef>, pub outgoing_attacks: JsArr<AttackRef>,   // filter-reassigned (AttackImpl.ts:61-71)
    pub outgoing_land_attacks: Vec<AttackRef>,
    pub alliances: JsArr<AllianceId>,                // filter-reassigned (GameImpl.ts:1143-1144)
    pub spawn_tile: Option<TileRef>, pub is_disconnected: bool, pub disconnect_snapshot: Option<DisconnectSnapshot>,
    pub largest_cluster_bbox: Tri<BBox>,             // Absent | Null | Some: "null ≠ absent" (:2024-2033)
    pub num_units_constructed: SmallVec<[(UnitType, i32); 16]>,  // Object.entries insertion order (:2034, Q-OBJKEYS)
    pub nearby_memo: Option<NearbyMemo>,             // exact memo (:600-626)
    pub reachable_components_memo: Option<ReachableMemo>, // SpatialQuery WeakMap<Player> (§3.11)
}
```

**Methods.** TS `PlayerImpl` methods that touch other objects (`buildUnit`, `conquer`, `createAttack`, …) become methods on a borrowed view, `PlayerMut<'a, 'c> { cx: &'a mut Ctx<'c>, id: SmallId }`.
- TS `player.buildUnit(..)` ports to `cx.player_mut(p).build_unit(..)`.
- Pure readers go on `PlayerRef<'a> { st: &'a State, id }`.
- Method names stay 1:1 with TS, which keeps the port mechanical. The same holds for `UnitMut`/`UnitRef`.

**Memo layout** [D] (review 1 #9). `TypeMemo` keeps one backing `Vec<UnitRef>` and 16 headers `(off: u32, len: u32, version: u32, valid: bool)`.
- A miss for any type at version `v` rebuilds **all 16** lists in one pass over `units`, in `units` order.
- Hits hand out a copy, like TS's `memo.list.slice()` (`PlayerImpl.ts:486-497`). They become a borrowed slice only where §11's rule allows it.
- This is equivalent to TS's per-type lazy fill whenever every mutation of `units` bumps `my_units_version`, because the filter result at version `v` does not depend on when it is computed.
- The `verify-memos` cargo feature recomputes on every hit and asserts equality. It runs on the full CI corpus (§12 G11).

**`units()` with no argument and `units([])` return the live array** (`PlayerImpl.ts:458-459, 470-472`) [V]. Loops over them follow §4.7.

### 3.4 Unit (`engine/src/game/UnitImpl.ts:39-64, 798-910`) [V]

`Unit` is a flat struct with per-type `Option` sub-states:
- `transport_ship_state`
- `warship_state`
- `nuke_state` (with `trajectory: Vec<TileRef>`)
- `sam_launcher_state`.

It also holds `missile_timer_queue: SmallVec<[i32; 4]>`, `health: i64` and `troops: f64`. `target_unit`, `target_player` and `destroyer` are `Option<handle>`.

`WarshipState.is_in_combat` is **`Option<bool>`**:
- The constructor (`:104-110`) and `updateWarshipState` (`:459-466`) build the object without that field, so it is briefly `undefined`.
- In both cases a `toUpdate()` follows at once (`PlayerImpl.ts:1427` in `buildUnit`; `UnitImpl.ts:467`), and that call writes the field back through `warshipState()`.
- The snapshot reads the raw field (`:821-830`), and CS distinguishes absent from false (SPEC §1.1).

`mg.stats()` calls in the constructor, `setOwner` and `delete` (`:121-127`, `:250-282`, `:344-400`) go to `Stats` (§3.10).

### 3.5 Attack, Alliance, AllianceRequest

- **Attack** (`engine/src/game/AttackImpl.ts:20-120`) [V]:
  - fields: `id: u64`, `target: Owner`, `attacker: SmallId`, `troops: f64`, `source_tile: Option<TileRef>`, `border: OrderedTileSet`, `border_size: i32`, `is_active`, `retreating`, `retreated`.
  - The border is a JS `Set<number>` passed in by `AttackExecution.init` (`AttackExecution.ts:139-144`). Its iteration order is state (CS A.border).
  - `delete()` filter-reassigns `target._incomingAttacks` (only when the target is a player) and `attacker._outgoingAttacks` (`:61-71`). The port uses `JsArr::filter_reassign` (§4.7).
- **Alliance** (`AllianceImpl.ts:15-30`): `id`, `requestor`, `recipient`, `created_at`, `expires_at`, `extension_requested_requestor`, `extension_requested_recipient`.
- **AllianceRequest** (`AllianceRequestImpl.ts:19-27`): `requestor`, `recipient`, `created_at`, `status`. `accept()`/`reject()` call back into the game (`:45-52`). In Rust they are `State::accept_alliance_request(ReqIdx)` and `State::reject_alliance_request(ReqIdx)`.

### 3.6 Tile layer

#### 3.6.1 `GameMap` (`engine-lib/src/game/GameMapImpl.ts`) [V]

```rust
#[derive(StateClone)]
pub struct GameMap {
    w: u32, h: u32,
    terrain: Arc<[u8]>,                 // bit7 land, bit6 shoreline, bit5 ocean, 0-4 magnitude (:26-35); COW on water edits
    terrain_gen: u32,                   // per-lineage counter, +1 on EVERY terrain write (setWater/setOcean/setMagnitude/…)
    state: TileStateBuf,                // u16: bits 0-11 owner, 13 fallout, 14 defense (:37-41); private, accessor-only (§8.5)
    num_land_tiles: u32, num_tiles_with_fallout: u32, water_version: u32,
    pristine_edits: JsMap<TileRef, u8>, // pristineTerrain Map (first-edit order) (:161-175): CS M.editTiles/…
}
```

- `ref(x, y)` throws on invalid coordinates (`:67-72`). Rust: `ref_(x, y) -> TileRef` raises `ts_throw!` with the TS message (§4.6). A non-throwing `try_ref` is used where TS checks `isValidCoord` first.
- `x(ref) = ref % w` and `y(ref) = (ref / w) | 0` (`:80-86`) map to integer `%` and `/` on `u32`. They are exact because refs are non-negative integers below 2^32.
- `terrain_gen` gates cross-fork sharing of water segment paths (§3.8).
  - It is not a global id. It is compared only within the lineage of one water graph, and there "same gen" implies "same terrain bytes", because every write bumps it.
  - Every game starts at gen 0 with pristine terrain, so gen 0 means pristine in every game.
- **Minimap *state* (owner, fallout and defense bits) is never written on the tick path; minimap *terrain* is.** On a water nuke, `WaterManager` edits minimap terrain three ways [V]:
  - `miniMap.setWater` (`engine/src/game/WaterManager.ts:493`)
  - `miniMap.setOcean` (`:541,552`)
  - **`setMagnitude`**, through `recomputeMagnitudesInBox(this.miniMap, …)` (`:582-590` → `:776-881`, write at `:881`).
- Magnitude is not cosmetic. `GameMap.cost()` is `magnitude < 10 ? 2 : 1` (`engine-lib/src/game/GameMapImpl.ts:296-297`), and water pathfinding reads it.
  - A port that ports only `setWater`/`setOcean` on the minimap **changes ship paths** after a water nuke.
  - Rule: the minimap goes through exactly the same `WaterManager` edit sequence as TS, including the magnitude BFS with its own stamp arrays (`_miniStampArr`/`_miniDistArr`, `:64-67`).
  - Every edit goes through `noteTerrainEdit`, so CS M.miniMap.edit* matches.
- The only writer of minimap fallout and defense bits is `GameMapImpl.restoreSnapshot` (`:634-637`), which the port replaces with `from_canon`.
  - `TileStateBuf` for the minimap is therefore the `Zero` variant: the minimap state stays zero, which saves 1 MB per clone, and the CS exporter emits zeros. A debug assertion fires on any minimap state write.
  - Minimap **terrain** is an `Arc<[u8]>` with COW, like the main map.
- Defense-bonus bit: `setDefenseBonus` has no callers at the pin (grep) [V]. It is ported for fidelity.
- `bfsSearch`/`bfs` (`:433-500`) use a **per-map-instance** generation-stamped `bfsVisited` and a re-entrancy guard (`:9-12, :477-479`) [V].
  - In Rust there are two independent scratch fields, `sc.bfs_main` and `sc.bfs_mini` (§3.8). Each has an explicit `busy: bool` that mirrors the TS guard.
  - A nested call on the same map raises `ts_throw!("Re-entrant bfsSearch is not supported")` exactly where TS throws. A nested call on the other map works, as in TS.

#### 3.6.2 TileSet port: `OrderedTileSet` and `PlayerTileOrder`

The TS `TileSet` (`engine-lib/src/game/TileSet.ts:27-242`) [V] is an insertion-ordered set. It stores a dense `Uint32Array` with `TOMBSTONE` holes plus an open-addressing index. Its **observable** semantics are exactly those of a JS `Set<number>` (doc comment `:13-26`):
- iteration is in insertion order
- entries added during iteration are visited (the length is re-read each step, `:175-189`)
- deleted entries are skipped
- delete followed by re-add moves the value to the end.

Compaction (`:208-218`) preserves live order and is deferred while an iterator is live (`iterDepth`). Dense positions, tombstones and capacity are therefore **not observable** (review 0 confirmed this, `TileSet.ts:150-217`). CS P.tiles is the live sequence.

So the port is free to choose its layout, as long as the live order and the iteration-under-mutation semantics match [D]:

- **`PlayerTileOrder` (players' `_tiles` and `_borderTiles`).** A tile is in at most one player's `_tiles`: `conquer` deletes it from the previous owner before adding (`GameImpl.ts:987-1009`). The same holds for `_borderTiles`, through `updateBorderStatus`, which adds to the current owner only, and through the deletes in `conquer`/`relinquish` (`:1042-1052`). Layout:
  - one **State-level position index per set kind**: `pos_tiles: Vec<u32>` and `pos_border: Vec<u32>`, indexed by the *land index* of a tile
  - `statics.land_index: Arc<[u32]>` maps tile → land index. It is shared, 8 MB, and never cloned.
  - The position arrays hold 651,569 × 4 B = 2.6 MB each on World. They are written only through `pos_tiles_set`/`pos_border_set` (§8.5).
  - per player, a dense `Vec<u32>` with tombstones (`TileListHdr`)
  - Membership is O(1) without hashing, and a clone is a memcpy.
- **`OrderedTileSet` (generic).** Used for attack borders, the `NukeExecution` blast set (`:108`) and the `GameMapImpl.bfs` result (`:503`). It reproduces the TS layout: a dense `Vec<u32>` plus a `LookupMap<u32, u32>` index (an Fx-hashed wrapper with no iteration API). Sizes are small.
- **Iteration under mutation.** Callers iterate with an index cursor that re-reads the length on every step (`while i < set.dense_len(st)`), never with a borrowed Rust iterator, because the loop body mutates `State`.
- **Compaction never runs inside a tick.**
  - It runs only in `State::end_of_tick_maintenance()`, when tombstones exceed **25 % of the live count** (review 1 #6). Order is unobservable, so the ratio is free. 25 % bounds the dense array at 1.25 × live and cuts tombstone skips in the `calculateClusters`/`floodTerritoryAreas` border scans.
  - This removes the need for `iterDepth`, since no cursor ever survives a tick boundary, and the observable order is unchanged.

### 3.7 UnitGrid (`engine-lib/src/game/UnitGrid.ts`) [V]

TS uses a `ceil(H/100) × ceil(W/100)` grid of `Map<UnitType, Set<Unit>>` (`:5-17`). For World that is 10 × 20 = 200 cells. Two orders are state and appear in CS G.unitGrid (`:19-35`):
- **per-cell type order**: the first insertion of that type into the cell. Emptied sets stay as keys, because `Set.delete` never removes the Map entry (`:78-85`).
- **in-set order**: insertion order. A move to another cell is `removeUnitByTile` followed by `addUnit`, which appends at the end (`:90-105`). Every tile change goes through `move()` → `onUnitMoved` (`UnitImpl.ts:213-220`) [V, review 0], so remove and add stay consistent.

Rust layout (flat and cheap to clone) [D]:
- `cells: Vec<CellHdr>` with `CellHdr { type_order: [u8; 16], n_types: u8, head: [u32; 16], tail: [u32; 16], n_defense_posts: u16 }`
- an intrusive doubly-linked list per (cell, type), with `prev`/`next` per unit slot stored in arrays parallel to `Arena<Unit>`.

Removal is O(1) and keeps order, and re-insert appends. That reproduces JS Set order exactly with no per-cell heap allocations. `nearbyUnits` and `hasUnitNearby` (`:117-221`) iterate cells in TS order, then types in `type_order`, then the list.

### 3.8 Water graph, pathfinding and caches: what is state, what is shared, what is scratch

**The TS object graph** [V, re-verified for v2]:
- **There is one `AStarWaterHierarchical` (HPA) per game.** A rebuild builds a **new** `AbstractGraph` object (`AbstractGraph.ts:268`) and calls `hpa.setGraph(newGraph)` (`WaterManager.ts:199-201`; `AStar.WaterHierarchical.ts:60-64`).
- Every water chain, old or new, wraps that same HPA object (`PathFinder.ts:47-66`). So **an old chain's HPA search runs on the current graph**, using the current graph's `_pathCache`.
- The only graph-bound piece of a chain is `componentCheckFn = graph.getComponentId` (`PathFinder.ts:60`). Every graph's `_waterComponents` is the one shared, live `ConnectedComponents` object:
  - the builder stores `sharedWaterComponents` (`AbstractGraph.ts:250-262, 323`)
  - `WaterManager` passes `_miniWaterCC` (`WaterManager.ts:190`).

  So an old chain's component check reads the **current** components.
- **The chain kind is fixed when the chain is built.** `!hpa || !graph || graph.nodeCount < 100` gives the plain `AStarWater` chain (`PathFinder.ts:52-58`); otherwise the chain is HPA.
- `_waterChainCache` holds one `{version, chain, memo}` per game. It is replaced lazily at the first `sharedWaterChain` call that sees a new `waterGraphVersion` (`PathFinder.ts:148-162`).
- A `WaterPathFinder` keeps its old stepper, and with it the old chain *and the old memo*, until its stagger countdown ends (`PathFinder.ts:271-285`).
- **`WaterPathMemo` is observable state.**
  - It is an LRU keyed `from * numTiles + to` with a 24 MB byte budget. Each entry counts `byteLength`, or 16 for `null` (`PathFinder.ts:79-138`).
  - It is cleared only when the **main map's** `waterVersion()` changes, not on a graph rebuild (`:103-110`).
  - After a rebuild, stale memoized steppers (trade ships, `TradeShipExecution.ts:46`) still use the old memo. Hits return old paths; misses are computed by the HPA on the *new* graph and stored back into the old memo.
  - So which keys are present and the LRU order change results. Disabling the memo, or keying it on (terrain, graph, from, to), would diverge.
- **`AbstractGraph._pathCache` depends on query history.**
  - Segments are computed by `findLocalPath` over the *live* minimap terrain (`AStar.WaterHierarchical.ts:262-273, 354-393`; `AStar.WaterBounded.ts:47`).
  - The cache is reset only when a new graph is built (`AbstractGraph.ts:133-136, 320`; `_initPathCache` has no other caller).
  - Water conversions change minimap terrain (`WaterManager.ts:493, 541, 552, 881`), but the rebuild comes 1–20 ticks later (`:174-210`).
  - So within one graph version, an edge filled before a nuke returns the old-terrain segment, and an edge first filled after it returns a new-terrain segment.

**Rust model** [D]:

| Object | TS location | Class | Rust treatment |
|---|---|---|---|
| `WaterManager` scalars: `waterGraphVersion`, `waterGraphDirty`, `waterGraphLastRebuildTick`, `pendingWaterTiles` (Set), `dirtyMiniTiles` (Set) | `WaterManager.ts:52-57,95-108` | **state** (CS W) | Plain fields; the two Sets become `JsSet<TileRef>` |
| Minimap `ConnectedComponents` | `:47`, mutated incrementally on water nukes; u8 → u16 promotion at id 253 (`ConnectedComponents.ts:96-116`) | **state** (CS W.components; the typed kind is state) | `state.water.components: Arc<Components>`, COW via `Arc::make_mut` on edit. **The only component source**: `get_component_id(t)` always reads `state.water.components`. No graph captures a copy. Version 0 starts as `statics.components0.clone()`. |
| Current `AbstractGraph` + HPA (`_miniWaterGraph`, `_miniWaterHPA`) | ctor (`:69-93`); rebuilt ≤ every 20 ticks after water nukes (`:174-210`) | **state** (which graph is current) | `state.water.graph: Option<Arc<WaterGraph>>` (None when `disableNavMesh`). It starts as `statics.graph0.clone()` and a rebuild replaces it with a new `Arc`. **The HPA always reads this field.** Old graphs are dropped when the last `Arc` goes; nothing routes on them. `WaterGraph` records `built_at_mini_gen: u32`. Forks share the `Arc` and never rebuild on fork. |
| `AbstractGraph._pathCache` (per-edge, per-direction segment) | `AbstractGraph.ts:105-136`; filled by `AStar.WaterHierarchical.ts:242-273` | **state: per-game visibility over shared values** | Three parts. (1) Shared values: `WaterGraph.seg_shared: Box<[OnceLock<Arc<[u32]>>]>` (2 × edges). It is filled only while `mini.terrain_gen == graph.built_at_mini_gen`, which makes the value a pure function of (graph, build-time terrain) and safe to share across forks and, for graph 0, across games. (2) Per-game `state.water.seg_vis.filled: BitVec` (2 × edges bits, a few KB, cloned). (3) Per-game `seg_vis.overlay: FxHashMap<u32, Arc<[u32]>>` for fills made after a terrain edit under the same graph (cloned; rare). Read rule: **a hit only if this game's `filled` bit is set**; the value comes from `overlay`, else from `seg_shared`. On a miss, compute; if the gens match, write it through `seg_shared.get_or_init`, else into `overlay`; set the bit. TS stores only non-null segments and treats an empty cached path as a miss (`:242-273`), so the port does the same. A rebuild resets `seg_vis` (TS `_initPathCache`). |
| `_waterChainCache` (chain + memo per game, keyed on `waterGraphVersion`) | `PathFinder.ts:42-45,145-162` | **state** (the memo is state; the chain kind is state) | `state.water.chains: ChainSlots { current: Option<ChainSlot>, retired: SmallVec<[ChainSlot; 2]> }`, where `ChainSlot { version: u32, kind: ChainKind /*Simple \| Hpa*/, memo: WaterPathMemo }`. `ensure_current_chain(cx)` is called at exactly the TS `sharedWaterChain` call sites (`PathFinding.Water`, the `WaterPathFinder` constructor and the `ensureFresh` swap). It retires the old slot, builds `kind` from the current graph's node count and creates an empty memo with `water_version = map.water_version`. Retired slots are dropped by the GC when no stepper names their version (§3.11). |
| `WaterPathMemo` | `PathFinder.ts:79-138` | **state** (LRU-observable, see above) | A literal port inside its `ChainSlot`: `water_version: u32`, `paths: JsMap<u64, Option<Arc<[u32]>>>`, `live_bytes: u64`, `max_bytes: u64 = 24_000_000`. Hit: `remove` + `insert` to move the key to the end. Evict: the first live key. Clear when `map.water_version` changes. Byte accounting is `4 × len`, or 16 for `None`. Cloned with the game. Its clone cost is measured in t091. **No feature flag**: it is always on, as in TS. |
| Chain transformers (ComponentCheck, Smoothing, ShoreCoercing, MiniMap) | `pathfinding/transformers/*` | stateless (review 0: no per-instance caches) | Free functions composed by `ChainKind`, called with `&mut Rcx` |
| `WaterPathFinder` (per ship: stepper, `_waterGraphVersion`, `_staggerCountdown`, `_pendingVersion`, `_rebuilt`, `_memoized`) | `PathFinder.ts:226-342` | **state** (in E rows, `snapshot/PathfinderSnapshots.ts`) | A plain struct inside the owning exec, plus `chain_version: u32`, which names its `ChainSlot` (current or retired). It holds **no graph reference**. While the stagger countdown runs, the old stepper keeps its cached path, its slot's `kind` and its slot's memo. Any search runs on the current graph and components, exactly as in TS. |
| `PathFinderStepper` cached path | `PathFinderStepper.ts:55-90` | **state** (in E rows) | Inside the exec |
| `_railChainCache` (AStarRail scratch) | `PathFinder.ts:171-183` | scratch | `sc.astar_rail` |
| `TileTraversalScratch` (`visited` u32 stamps, `stack`, `clusterIndexMap`) | `TileTraversalScratch.ts:17-50`; `clusterIndexMap` is zeroed after every use (`PlayerExecution.ts:295-305`) | scratch, **with a stamp-wrap hazard** | `sc.traversal`. TS keeps one per game, and a World game makes about 5 k stamp bumps per tick, so TS never reaches the wrap at 0xffffffff (`:43-50`). A Rust `Scratch` shared by every game and fork on a thread could wrap after about 10^6 fork-ticks. A wrap inside the `floodTerritoryAreas` extension loop (`PlayerExecution.ts:371`) would change territory areas. **Rule:** `Scratch::prepare()` at the start of every `step` zeroes the stamps and resets `gen` to 0 when `gen > 2^31`. TS never wraps and every comparison is relative within a pass, so this cannot be observed. A unit test forces `gen` near the wrap. |
| `GameMapImpl.bfsVisited/bfsQueue` (per map instance) | `GameMapImpl.ts:9-12,433-500` | scratch | `sc.bfs_main`, `sc.bfs_mini` (separate), each with a `busy` flag (§3.6.1) and the same step-start wrap reset |
| WaterManager stamp arrays (`_waterDist/_waterStamp`, `_miniDist/_miniStamp`, u16) and `_builderBFS` | `WaterManager.ts:57-67,190` | scratch | `sc.water_mgr`. The porter documents TS's own u16 wrap handling. If it is single-pass-safe, sharing cannot be observed. Otherwise the step-start reset applies. |
| HPA internals: `AbstractGraphAStar`, the three `AStarWaterBounded` instances (local, multi-cluster, short-path), `SourceResolver` | `AStar.WaterHierarchical.ts:32-48,60-64` | scratch (stamps reset per query, `PriorityQueue.ts:169-177`) | `sc.hpa: HpaScratch { abstract_astar, local, local_multi, local_short, source_resolver }`: one field per TS instance, so nesting stays legal exactly where TS nests |
| Simple-chain `AStarWater`, `BFSGrid`, `MinHeap`/`BucketQueue` | OPAQUE classes | scratch | `sc.astar_water`, `sc.bfs_grid`, … |
| `SpatialQuery.reachableComponents` (WeakMap<Player>, keyed `(tileVersion, waterVersion)`) | `pathfinding/spatial/SpatialQuery.ts:16-25,160-175` | exact memo | Field on `Player` (cloned, small) |
| `SharedWaterCache.byPlayer` (TTL 30 ticks) | `execution/nation/SharedWaterCache.ts:25,37-44` | **state**: stale reads are observable (its own doc says so, `:46-49`) | `state.shared_water` with `JsMap<SmallIdKey, Option<JsSet<i32>>>`; `playerWater` is derived and cloned |

The general rule [D] has four parts:
1. **TTL caches, LRU caches, history-dependent caches and anything in the snapshot are state.** They are cloned.
2. **Version-keyed exact memos** are ported with the same keys and kept in `State` when small. They are cloned.
3. **Values** may be shared across forks (through `Arc`/`OnceLock`) only when they are a pure function of data that every sharer provably has. **Visibility** (whether this game would hit) is always per game.
4. **Map-sized scratch** lives in `Scratch`. It is never cloned. One field per TS scratch object.

**How scratch is reached** [D] (review 1 #16, review 0 #7). Executions and state methods receive a context instead of `&mut State`:

```rust
pub struct Ctx<'a> { pub st: &'a mut State, pub sc: &'a mut Scratch, pub jr: Option<&'a mut TileJournal> }
pub struct Rcx<'a> { pub st: &'a State, pub sc: &'a mut Scratch }   // read-only state + scratch (oracles, queries)
```

- `Scratch` is a plain struct of independent fields with no `RefCell`, so the compiler checks the borrows and there is no TLS on the hot path.
- `Game::step` fetches the thread's `Scratch` from one `thread_local!` **once per step** and passes `&mut` down. `Game::step_with(&turn, &mut Scratch)` lets a caller own it.
- A TS "Re-entrant …" throw becomes an explicit `busy` flag that raises `ts_throw!` at the same place.

**Fork exactness.** Every input to a water query is in the cloned `State`: the current graph `Arc`, the components, `seg_vis`, the chain slots with their memos, and the steppers. So a Rust fork reproduces the stale-graph window, the memo contents and the segment cache exactly. The TS restore (`createGameRunnerFromSnapshot`) cannot do that: it rebuilds the WaterManager, and `WaterPathFinder.fromState` routes new queries on the current graph with a fresh chain cache (`PathFinder.ts:326-342`). `Game::from_canon` reproduces the **TS restore** semantics instead: fresh graph, empty `seg_vis`, a single fresh chain slot and empty memos (§9).

### 3.9 RailNetwork (`engine/src/game/RailNetworkImpl.ts`, `TrainStation.ts`, `Railroad.ts`, `RailroadSpatialGrid.ts`) [V]

| TS | Rust |
|---|---|
| `StationManagerImpl.stations: Set<TrainStation>`, `stationsById[]`, `stationsByUnit: Map<Unit, TrainStation>` (rebuilt in `stations` order on removal, `:36-122`) | `stations: JsSet<StationId>`, `by_id: Vec<Option<TrainStation>>`, `by_unit: JsMap<UnitKey, StationId>`, with the same rebuild rule |
| `TrainStation { id, stopHandlers, cluster, railroads: Set, railroadByNeighbor: Map }` (`:71-80`) | `TrainStation { id, unit: UnitRef, cluster: Option<ClusterRef>, railroads: SmallJsSet<RailroadId, 4>, by_neighbor: SmallJsMap<StationId, RailroadId, 4> }`. **No PRNG field:** `createTrainStopHandlers(random)` ignores its argument and builds stateless handlers (`TrainStation.ts:59-68`) [V], so the handler is a `match` on the unit type. |
| `Cluster { stations: Set, tradeStations: Set }` (`TrainStation.ts:212-215`) | `Arena<Cluster>` with two `SmallJsSet<StationId, 8>` |
| `Railroad { id, from, to, tiles }` | `Arena<Railroad>`, with `tiles: Vec<TileRef>` |
| `RailroadSpatialGrid { cells: Map<string, Set<Railroad>>, railToCells: Map<Railroad, Set<string>> }` (`:5,7`) | The string key `${cx}:${cy}` (`:113`) is a bijection with `(cx, cy)`, so keys are packed into `u32`. Both maps are **iterated by the snapshot** (`:86-98`, whose own comment says "Cell and rail entry order both decide query result order"), so CS needs their insertion order and they stay `JsMap`. The CS exporter renders a packed key back to the TS string `${cx}:${cy}`. The per-cell rail set is iterated by `query` (`:78`), so it keeps insertion order through intrusive links per cell, as in `UnitGrid`. |
| `TrainExecution.usedTiles` (`shift()`, `:230-232`), `pathTiles` (a full concatenated copy, `:83-101`) | `usedTiles` → `VecDeque<TileRef>` (same order, O(1) pop). `pathTiles` stays a `Vec`; its bytes are measured in t091. |
| `RailNetworkImpl { railGrid, nextId, dirtyClusters: Set<Cluster>, networkVersion, … }` (`:298-309`) | Same fields. `networkVersion` and `ghostPathsCache` are derived (`DERIVED_FIELDS`). |
| Ghost-rail preview (`computeGhostRailPaths`, `GhostRailNetworkState`, `:150-296`) | **Skipped.** It is reached only from `PlayerImpl.buildableUnits`, gated by `buildNew = canBuild !== false && (canUpgrade === false \|\| includeBuildPreview)` (`PlayerImpl.ts:1553-1554,1578-1579`). That is called from `GameRunner` player-actions queries (`GameRunner.ts:347,361`) [V]. It is a dry run whose only write is `ghostPathsCache` (derived, not in the snapshot). No execution calls it. **Embedder note:** if an embedder's legality oracle uses `buildableUnits`, the Rust version returns an empty `ghostRailPaths`. That cannot change state. |
| `RailNetworkImpl.hash()` (`:930-946`) | Only for the legacy hash (§5), on demand |

### 3.10 Stats: only what the simulation reads, plus the full version behind a feature

The sim reads stats in exactly one place. `GameImpl.conquerPlayer` reads `stats.getPlayerStats(conquered)?.attacks?.[ATTACK_INDEX_SENT] ?? 0n` to decide whether gold transfers (`engine/src/game/GameImpl.ts:1777-1781`) [V].
- `getPlayerStats` returns `undefined` for players without a clientID (`StatsImpl.ts:96-100`), so this matters only for **Humans**, which is what an embedder's player is.
- **Correction to SPEC §1.3**, which says the sim reads stats only for the PlayerUpdate and Win updates: `conquerPlayer` is a third read, and it affects the sim (ledger t089).
- The other reads are view or output: `PlayerImpl.toUpdate` (`PlayerImpl.ts:377`) and `WinCheck.setWinner(.., stats())` (`WinCheckExecution.ts:75-207`).

Rust [D]:
- **Always on.** `Stats` holds, per clientID, `attacks_sent: i64`.
  - `attack()` adds `troops` and `attackCancel()` adds `-troops`, each through `_bigint` = `BigInt(Math.floor(x))` (`StatsImpl.ts:58-65, 254-281`) [V].
  - **Floor asymmetry:** `floor(x) + floor(-x) = -1` for non-integral `x`. A send followed by a cancel can therefore leave `attacks_sent = -1`, which is non-zero, and `conquerPlayer` checks `=== 0n`. This is ported literally and has a unit test.
- **Strict conversions at every stats call site, even when `stats-full` is off.**
  - `_bigint` runs only after `_makePlayerStats` returned a record, i.e. for players with a clientID (`StatsImpl.ts:116-130`) [V].
  - `BigInt(NaN or ±Inf or fraction)` throws there, and that throw is fatal for the tick.
  - The lite build therefore keeps every stats entry point with the same clientID gate. It calls `of_det::bigint::from_floor_strict(value)` on each numeric argument and discards the result, except for `attacks_sent`. That way fatal errors happen at the same tick.
- Under `feature = "stats-full"` (on for conformance L3, off for throughput-oriented builds), all of `StatsImpl` is ported, including the `_sampleCache` peaks (`:74-82`, moved into the per-client record), so CS S matches.
- K digests have `S = null` (SPEC §1.5), so a build with `stats-full` off can still claim L2.
- `per_player_bookkeeping` (TS `recordTickSample`, `GameImpl.ts:631-648`) compiles to nothing without `stats-full`.

### 3.11 Module-level caches and arena garbage collection

| TS module state | File:line [V] | Rust |
|---|---|---|
| `_waterChainCache` WeakMap<Game> | `pathfinding/PathFinder.ts:42` | `state.water.chains` (**state**, §3.8) |
| `_railChainCache` WeakMap<Game> | `pathfinding/PathFinder.ts:171` | `sc.astar_rail` |
| `scratches` WeakMap<Game, TileTraversalScratch> | `game/TileTraversalScratch.ts:25` | `sc.traversal` (+ step-start wrap reset) |
| `reachableComponents` WeakMap<Player> | `pathfinding/spatial/SpatialQuery.ts:25` | `Player.reachable_components_memo` |
| `_sampleCache` WeakMap<PlayerStats> | `game/StatsImpl.ts:82` | inside `Stats` per client (`stats-full`) |
| `NEIGHBOR_SCRATCH`, `UNITS_SCRATCH`, `TYPE_SET_SCRATCH`, `EMPTY_*` | `game/PlayerImpl.ts:114-128` | stack buffers or `SmallVec`; `EMPTY_*` are view-only |
| DetMath `f64`/`u32` views | `engine-lib/src/DetMath.ts:15-16` | `f64::to_bits` (inside `of-det`) |
| `NameBoxCalculator` histograms | `game/NameBoxCalculator.ts:23-24` | dropped (view) |
| `NO_BOTS`, `O_TO_OES`, `SPECIAL_PLURALS`, `UNIT_TYPE_TO_INT` | `nation/BotAnnexation.ts:95`, `game/NationCreation.ts:618-621`, `engine-lib/src/game/PackedUnitUpdates.ts:51` | `const`/`static` immutables |

**Arena GC: in v1, not phase 2** [D] (review 1 #5). Dead objects would otherwise roughly double the clone by mid-game:
- Each train builds 2 + `numCars` units (`TrainExecution.ts:164-180`), and the prototype saw 1,708 trains in 1,000 World ticks (`docs/recon/12_clone_prototype.md:92`).
- There were 4,948 AttackExecutions per 3,000 ticks, each keeping its border set.

How it works:
- **When:** `State::collect_garbage(&ExecStore)` runs at a tick boundary every 32 ticks, and at the start of `Game::fork_into` (§10).
- **What it frees:** objects that are both inactive and unmarked:
  - units
  - attacks
  - clusters
  - railroads (with their `tiles`)
  - retired water `ChainSlot`s.
- **Roots:**
  - every exec reachable from `execs.live`, `state.uninit` and pinned slots, through their `RefVisitor` impls. These include `NationAttackBehavior.followedLandings` (attacks) and stepper `chain_version`s.
  - players' lists
  - game lists
  - rail (`by_unit`, stations, clusters, railroads)
  - live units' `target_unit`.
- It uses the same `RefVisitor` trait that the CS exporter needs for SPEC §1.2 ("dead-but-referenced objects are rows").
- **Why it is unobservable:** CS keys and map keys use TS ids, never slots (PORTING_GUIDE §5.1).
- **Guards:**
  - a conformance run with GC forced every tick must still pass L3
  - `fork_identity` (§10) runs with GC at random points.

---

## 4. Execution model

### 4.1 TS semantics to reproduce (`engine/src/game/GameImpl.ts:599-665`, `GameRunner.ts:213-246`) [V]

1. **Intents first.** `GameRunner.executeNextTick` maps the turn's intents to executions in array order and pushes them to `unInitExecs` (`GameRunner.ts:222-224`, `GameImpl.ts:787-789`). Then `currTurn++`, then `game.executeNextTick()` runs inside a `try`. The new executions are inited at the end of this tick and first ticked on the next one.
2. **Tick pass.** `this.execs.forEach(e => if ((!inSpawnPhase() || e.activeDuringSpawnPhase()) && e.isActive()) e.tick(ticks))` (`:606-613`):
   - `forEach` captures the array length once. Nothing appends to `execs` during the pass, because `addExecution` targets `unInitExecs`.
   - `inSpawnPhase()` is **re-read per execution**. `SpawnTimerExecution` can end the phase mid-pass, and later executions then tick (Q-SPAWNFLIP, `:536-552`, `SpawnTimerExecution.ts:15`).
   - `isActive()` is re-read per execution, so one that was deactivated earlier in the pass is skipped.
3. **Init pass.** `for (const e of this.unInitExecs)` (`:614-627`) runs a live array iterator, so executions **added during `init` are visited in the same pass**.
   - If `!inSpawnPhase() || e.activeDuringSpawnPhase()`, the exec gets `e.init(this, ticks)` and goes to the local `inited`, else to the local `unInited`.
   - **`init` runs even on inactive executions** (there is no `isActive` check).
   - Then `execs.push(...inited)` and `unInitExecs = unInited`.
4. **Compaction** (`:757-773`):
   - in place, preserving order
   - in the spawn phase it keeps `!activeDuringSpawnPhase() || isActive()`
   - otherwise it keeps `isActive()`.
5. **Per-player bookkeeping** (`:631-648`): `recordTickSample` (stats, §3.10) and `toUpdate` (view, §5).
6. **Hash update** every 10 ticks (`:649-655`): view, plus the legacy hash on demand (§5).
7. **Water:** `_waterManager.tick(ticks)` flushes pending conversions and the throttled graph rebuild. `_structureVersion++` runs if tiles changed (`:657-662`).
8. `_ticks++` is the last state-changing statement (`:663`).

`removeExecution` (`:791-796`) is reachable only from the takeover hooks (`:807-812`, `:888-890`) [V, grep], which are out of scope.

### 4.2 Rust shape

```rust
#[derive(Clone, Copy, PartialEq, Eq)] #[repr(u8)]
pub enum ExecKind { AllianceExtension, AllianceReject, AllianceRequest, Attack, BoatRetreat, BreakAlliance, City,
    Construction, DefensePost, DeleteUnit, DonateGold, DonateTroops, DoomsdayClock, EmbargoAll, Embargo, Emoji,
    Factory, MarkDisconnected, Mirv, MissileSilo, MoveWarship, Nation, NoOp, Nuke, Pause, Player, Port, QuickChat,
    RecomputeRailCluster, Retreat, SamLauncher, SamMissile, Shell, Spawn, SpawnTimer, TargetPlayer, TradeShip,
    Train, TrainStation, TransportShip, Tribe, UpgradeStructure, Warship, WinCheck }   // 44 = ExecutionRegistry.ts:56-100 [V]

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ExecId { pub kind: ExecKind, pub idx: u32 }

pub enum Exec { AllianceExtension(AllianceExtensionExec), /* … one by-value variant per kind … */ WinCheck(WinCheckExec) }
// Transport type only: what add_execution receives. It lives in state.exec_inbox for microseconds. No size limit, no Box.

#[derive(StateClone)]
pub struct Slots<T> { items: Vec<T>, alive: Vec<bool>, pins: Vec<u16>, free: Vec<u32> }   // slot reuse: LIFO free list

#[derive(StateClone)]
pub struct ExecArenas { attack: Slots<AttackExec>, nation: Slots<NationExec>, tribe: Slots<TribeExec>, /* … 44 … */ }

#[derive(StateClone)]
pub struct ExecStore {
    pub live: Vec<ExecId>,               // TS GameImpl.execs, in tick order. Copy elements: cloning is a memcpy.
    pub ar: ExecArenas,
    buf_inited: Vec<ExecId>, buf_uninited: Vec<ExecId>,   // double buffers for the init pass (no per-tick alloc)
}

pub trait ExecBehavior {               // TS Execution interface (engine/src/game/Game.ts:125-136) [V]
    fn is_active(&self) -> bool;
    fn active_during_spawn_phase(&self) -> bool;
    fn init(&mut self, cx: &mut Ctx, ticks: i32);
    fn tick(&mut self, cx: &mut Ctx, ticks: i32);
}   // ExecArenas::{tick, init, is_active, …}(id, ..) dispatch by `match id.kind`; snapshot() is replaced by Canon (§9)
```

Why per-kind arenas (review 1 #3):
- The live list is `Copy` and cloned with a memcpy.
- Slot positions are stable between a root and its forks until they diverge, so element-wise `clone_from` reuses each exec's inner buffers.
- No variant needs a `Box`. That removes about 500 allocations per fork (72 nations, 400 tribes, warships, SAMs).
- Order and CS keys come from `live`/`uninit` positions, never from slots, so the layout cannot be observed.

`add_execution(cx, Exec)` pushes to `st.exec_inbox`. The driver's `drain_inbox()` moves each `Exec` into its arena, in push order, and appends the `ExecId` to `st.uninit`. It runs right after every `tick`/`init` call, after the intents are mapped, and inside `Game::new`. Nothing reads `uninit` between a push and its drain, so the resulting order equals TS's direct `unInitExecs.push`.

The tick (a literal transcription of §4.1):

```rust
fn execute_next_tick(&mut self, sc: &mut Scratch, jr: Option<&mut TileJournal>) {
    let (st, ex) = (&mut self.state, &mut self.execs);
    let mut cx = Ctx { st, sc, jr };
    let t = cx.st.ticks;
    // (2) tick pass: forEach semantics (length captured once)
    let n = ex.live.len();
    for i in 0..n {
        let id = ex.live[i];
        if (!cx.st.in_spawn_phase() || ex.ar.active_during_spawn_phase(id)) && ex.ar.is_active(id) {
            ex.ar.tick(id, &mut cx, t);
            ex.drain_inbox(cx.st);                       // new execs → arenas → st.uninit (call order)
            if !cx.st.exec_cmds.is_empty() { ex.apply_exec_cmds(&mut cx); }   // §4.3 (MIRV only)
        }
    }
    // (3) init pass over a live-growing uninit (TS for-of semantics), TS-exact under panics (§4.6)
    if !cx.st.uninit.is_empty() {
        let mut inited = std::mem::take(&mut ex.buf_inited);
        let mut un = std::mem::take(&mut ex.buf_uninited);
        let mut i = 0;
        while i < cx.st.uninit.len() {                   // re-read len: inits append via drain_inbox
            let id = cx.st.uninit[i];
            if !cx.st.in_spawn_phase() || ex.ar.active_during_spawn_phase(id) {
                ex.ar.init(id, &mut cx, t);              // a panic unwinds here: st.uninit and ex.live untouched
                ex.drain_inbox(cx.st);
                if !cx.st.exec_cmds.is_empty() { ex.apply_exec_cmds(&mut cx); }
                inited.push(id);
            } else { un.push(id); }
            i += 1;
        }
        ex.live.extend_from_slice(&inited);              // == execs.push(...inited)
        std::mem::swap(&mut cx.st.uninit, &mut un);      // == unInitExecs = unInited
        inited.clear(); un.clear();
        ex.buf_inited = inited; ex.buf_uninited = un;    // keep capacity
    }
    // (4) compaction (in place, order-preserving); frees slots unless pinned (§4.3)
    ex.remove_inactive_executions(cx.st);
    // (5)-(7)
    cx.st.per_player_bookkeeping();      // no-op without stats-full
    cx.water_tick(t);                    // WaterManager.tick + structure_version bump
    cx.st.end_of_tick_maintenance();     // TileSet compaction, OrdMap remap clear (never observable)
    cx.st.ticks += 1;
}
```

Subtle equivalences, each one argued:
- **Executions cannot see the exec list.** Executions reach other executions only through MIRV's warhead list (SPEC §1.2.1 [V grep `w.exec(`]) and through `GameImpl.executions()`, which only the takeover hooks call (`GameImpl.ts:807,888`) [V]. Review 0 independently confirmed MIRV as the only execution-to-execution reference. `TrainStation.onTrainStop` only reads the train execution it is given (`TrainStation.ts:161-167`).
- **`inited` is appended after the loop**, exactly like TS. Nothing reads `live` during init.
- **Slot reuse.** A freed slot may be reused by a later `add_execution`. Nothing outside the arenas holds an `ExecId` except `live`, `uninit` and MIRV's pinned warheads, so reuse cannot be observed.

### 4.3 The single exec→exec interaction: MIRV warheads

Independently confirmed twice (Grok citation check, `engine-rs/docs/CITATION_CHECK_GROK.md` #9; review 0):
- `MirvExecution.warheadExecutions` (`MIRVExecution.ts:85`) is the **only** Execution field that holds another Execution.
- `:184` is the only cross-exec `.cancel(`.

`MirvExecution` pushes each warhead `NukeExecution` both to `this.warheadExecutions` and to `mg.addExecution` (`MIRVExecution.ts:280-300`) [V]. When the MIRV's own nuke dies, it runs `for (w of warheadExecutions) w.cancel(); warheadExecutions = []; return;` (`:179-187`). `NukeExecution.cancel()` sets `active = false` and deletes its nuke unit if that unit is active (`NukeExecution.ts:510-515`).

**Rust.**
- **Warhead handles.** `add_execution` returns `PendingExec(u32)`, the index in `exec_inbox`. MIRV stores it in `warhead_executions: Vec<WarheadRef>` as `WarheadRef::Pending(p)` and queues `ExecCmd::BindWarhead { k, pending: p }` (`k` = list index) together with the `add_execution` call. Right after the MIRV's call returns, the driver drains the inbox and applies the commands, rewriting entry `k` to `WarheadRef::Id(ExecId)`. The issuer is implicit (the driver knows which execution ran), so no execution needs its own `ExecId`. MIRV only ever sees resolved ids on its later ticks. The S0 skeleton owns this mechanism (`exec/mod.rs`, P1), and P8 only calls it.
- On cancel, MIRV pushes `ExecCmd::CancelNuke(w)` for each warhead, then `ExecCmd::Unpin(w)` for each, then clears its list and returns. Both take a `WarheadRef`, so a warhead created and cancelled in the same call (still `Pending`) resolves through that call's drain.
- The driver applies the commands **immediately after the MIRV's `tick` returns**. That is equivalent because:
  - the cancels are MIRV's final side effects in that tick (only `warheadExecutions = []` and `return` follow)
  - `cancel()` touches only the warhead execution and its unit.

**Pinning.** A warhead execution compacted out of `live` while the MIRV still lists it must stay readable. TS keeps the object, and CS E rows include "detached" executions reachable only through MIRV (SPEC §1.2.1).
- S0 implements pins as **hold counts** (`Slots.holds`): 1 while the slot is listed (`uninit`/`live`) plus 1 per MIRV that lists it. `BindWarhead` adds a hold.
- Compaction drops the listing hold; the slot is freed only when no hold is left.
- `Unpin` drops the MIRV's hold, freeing a slot already compacted out. When a MIRV slot itself is freed (compacted out with warheads still listed), its holds on them are dropped too: TS's MIRV object becomes garbage, and its warheads are then reachable only through `execs`/`unInitExecs`.
- CS export finds detached warheads through the MIRV's `warheads` (SPEC §1.2.1 drain order).

A cancel on a detached (already inactive) warhead is a TS no-op on everything outside that execution: `active` was already false, and `nuke.isActive()` is false or the nuke is `null`.

### 4.4 Nested behaviours, shared PRNGs and `AiAttackBehavior` inheritance

`NationExecution` owns one PRNG and passes the *same object* to 7 behaviours, which also hold references to each other (`NationExecution.ts:270-321`) [V]:
- attack → alliance, emoji, warship
- nuke → attack, emoji
- mirv, alliance, warship → emoji.

**Nation layout** [D]: `NationExec { rng: Prng, player: Option<SmallId>, emoji: EmojiB, mirv: MirvB, alliance: AllianceB, warship: WarshipB, attack: NationAttackB, nuke: NukeB, structure: StructureB, ..scalar fields.. }`, where `NationAttackB { base: AiAttackBase, bot_attack_troops_sent, followed_landings: Vec<AttackRef>, land_neighbors: Option<SmallJsSet<SmallIdKey, 8>>, .. }`.

Every behaviour method is an `impl NationExec` method in its own file, for example `exec/nation/alliance.rs`: `impl NationExec { fn alliance_maybe_accept(&mut self, cx: &mut Ctx) … }`.
- A cross-behaviour call `this.allianceBehavior.foo()` becomes `self.alliance_foo(cx)`.
- Every `this.random` is `self.rng`.
- One `&mut self` covers all siblings, so the PRNG stream is shared exactly as in TS, with no aliasing problem.

**`AiAttackBehavior` keeps its virtual dispatch** [D] (review 0 #5). `abstract class AiAttackBehavior` (`execution/utils/AiAttackBehavior.ts:41`) calls hookable methods through `this.` (`:94, 100, 133, 200, 219, 308-309, 317, 379`) [V].
- `TribeAttackBehavior extends` it with no overrides.
- `NationAttackBehavior extends` it and overrides seven methods (`NationAttackBehavior.ts:1032, 1056, 1140, 1149, 1201, 1214, 1272`) [V]. It calls `super.` at `:1202` and `:1218`.

Port it like this:

```rust
// exec/ai_attack.rs (P9). FROZEN AT S0 (signatures).
#[derive(StateClone)]
pub struct AiAttackBase {                       // AiAttackBehavior fields (:43-56)
    pub trigger_ratio: f64, pub reserve_ratio: f64, pub expand_ratio: f64,
    pub boat_routes: Option<JsMap<SmallIdKey, Option<BoatRoute>>>,
}
pub trait AiAttackHooks {
    // access to the shared pieces (implemented by TribeExec and NationExec)
    fn ai(&mut self) -> &mut AiAttackBase;
    fn rng(&mut self) -> &mut Prng;             // the SAME Prng as the owner's other behaviours
    fn player(&self) -> SmallId;
    // the 7 virtual methods; defaults = base (tribe) behaviour, implemented as free fns in ai_attack.rs
    fn should_attack(&mut self, cx: &mut Ctx, other: Owner) -> bool { base_should_attack(self, cx, other) }
    fn ignores_attacks_from(&mut self, cx: &mut Ctx, attacker: SmallId) -> bool { base_ignores_attacks_from(self, cx, attacker) }
    fn expansion_troop_cap(&mut self, cx: &mut Ctx) -> f64 { base_expansion_troop_cap(self, cx) }
    fn calculate_attack_troops(&mut self, cx: &mut Ctx, target: Owner, non_bot: NonBotTroops, opponent: Option<SmallId>)
        -> Option<f64> { base_calculate_attack_troops(self, cx, target, non_bot, opponent) }
    fn send_boat_attack(&mut self, cx: &mut Ctx, target: SmallId) -> bool { base_send_boat_attack(self, cx, target) }
    fn send_expansion_boat(&mut self, cx: &mut Ctx, tile: TileRef) -> bool { base_send_expansion_boat(self, cx, tile) }
    fn sail(&mut self, cx: &mut Ctx, launch: TileRef, target: TileRef, warships: Option<&[UnitRef]>) -> SailResult
        { base_sail(self, cx, launch, target, warships) }
}
// Every other AiAttackBehavior method is a generic free fn: pub fn send_attack<H: AiAttackHooks + ?Sized>(h: &mut H, cx, ..)
// and calls hooks through `h.`, exactly where TS writes `this.`.
impl AiAttackHooks for TribeExec { fn ai(..), fn rng(..), fn player(..) }        // no overrides (P9)
impl AiAttackHooks for NationExec { /* all 7 overrides */ }                       // P10, exec/nation/attack.rs
```

- `super.sendBoatAttack(t)` → `base_send_boat_attack(self, cx, t)`. `super.sendExpansionBoat(t)` → `base_send_expansion_boat(self, cx, t)`.
- `NonBotTroops` is the TS closure parameter `(reserveTroops) => number` (`:1149-1152`), ported as a small enum or struct carrying its captured data (PORTING_GUIDE §7).
- `sail`'s default argument `warships = this.hostileWarships()` is evaluated inside the override when `None`, at the same point where TS evaluates it.
- The context passed to hooks is the whole `&mut NationExec`/`&mut TribeExec`, so overrides can reach sibling behaviours (`self.alliance_*`, `self.warship_*`) and the shared PRNG.
- Dependency: P10 (nation overrides) needs P9's base. That is FILE_MAP §2.1 and the critical path.

### 4.5 PRNG instances (all `of_det::Prng`, 16 B, inline)

| Owner | Seed (TS) | File:line [V] |
|---|---|---|
| Game setup | `simpleHash(gameID)`: human ids, then nations, in that order | `GameRunner.ts:56-79` |
| Executor | `simpleHash(gameID)+1` (constructed, **never drawn**) | `execution/ExecutionManager.ts:49`; not ported (no observable effect) |
| TribeSpawner | `simpleHash(gameID)+2` | `TribeSpawner.ts:32` |
| Player | `simpleHash(player.id)` (`nextID()` for attack ids) | `PlayerImpl.ts:219` |
| AttackExecution | `123` per execution (constructed at `new`) | `AttackExecution.ts:40` (the post-warmup state can be a `const`, PORTING_GUIDE §13) |
| NationExecution | `simpleHash(playerID)+simpleHash(gameID)`; draws 3 ratios in the **constructor** | `NationExecution.ts:73-78` |
| TribeExecution | `simpleHash(tribe.id())` | `TribeExecution.ts:36` |
| SpawnExecution | (see file) | `SpawnExecution.ts:48` |
| Emoji / Port / Donate{Gold,Troops} / Shell / Warship / Nuke(rand) / TrainStationExec | `mg.ticks()` at init | `EmojiExecution.ts:35`, `PortExecution.ts:33`, `DonateGoldExecution.ts:47`, `DonateTroopExecution.ts:43`, `ShellExecution.ts:45`, `WarshipExecution.ts:56`, `NukeExecution.ts:81`, `TrainStationExecution.ts:41` |
| MIRV | `ticks + simpleHash(player.id)` | `MIRVExecution.ts:93` |
| SAMLauncher | `sam.id()`, lazily (`??=`) | `SAMLauncherExecution.ts:385` |
| TrainStation stop handlers | constructed with `ticks`/`0` but **never drawn**: `createTrainStopHandlers` ignores `random` (`game/TrainStation.ts:59-68`) | not ported |
| NationMIRVBehavior (local) | `ticks*1024 + target.smallID()` | `nation/NationMIRVBehavior.ts:63` |
| TeamAssignment (local) | `simpleHash(nationPlayers[0].id)` | `engine-lib/src/game/TeamAssignment.ts:158` |
| AirPathFinder (local) | `this.seed` | `pathfinding/PathFinder.Air.ts:27` |

All seeds go through `PseudoRandom(seed)` → `seed | 0` (`PseudoRandom.ts:11-34`). Rust: `Prng::new(seed: f64)` in `of-det` applies `to_int32` (VECTORS `prng` suite).

**Construction time matters.** A PRNG built in a TS constructor is seeded when the execution object is created, not at `init`. Example: `AttackExecution`'s field initialiser. The Rust constructor must do the same.

**Shared-stream debugging** (review 0 #13). One nation PRNG feeds all seven behaviours, so one extra draw in any package (P10–P12) shifts every later draw, and no nation scenario can pass until all three packages are correct. `of-det`'s `Prng` gets a `prng-trace` feature: a draw counter, plus a hook receiving `(owner tag, draw index, value, #[track_caller] location)`. The TS side records the same tuple, with a stack line, through the probe patch (SPEC §3.2.6, ledger t073 + t108). `conform --prng-trace` reports the first divergent draw per nation, which points at one behaviour file.

### 4.6 Errors: TS `throw` → Rust

**Panic payload classes** [D] (review 0 #6). Exactly two kinds of panic exist:
1. **`TsThrow`**, a TS-equivalent throw. It is raised only by:
   - `ts_throw!(msg)` at every explicit TS `throw new Error(..)` / RangeError site
   - `.ts_expect(site)` on `Option`/`Result` at every TS `TypeError` site (`x!`, `undefined.foo`, a non-null assertion that can fail)
   - the strict BigInt conversions (§7.2) and BigInt `/ 0n`.

   The payload is `of_det::throw::TsThrow { msg: Cow<'static, str> }`, raised with `std::panic::panic_any`.
2. **Anything else** is a Rust-only failure with no TS counterpart: an overflow check (the conformance profile has `overflow-checks = true`), `debug_assert!`, a slice index out of bounds where TS would read `undefined`, a broken arena-generation check, or `unreachable!`. **These are always fatal and are never mapped to a TS behaviour.**

**Where each is caught:**
- **Inside the stepper.** `PathFinderStepper` wraps `finder.findPath` in try/catch → `NOT_FOUND` (`pathfinding/PathFinderStepper.ts:64-70`) [V]. Rust: `catch_unwind(AssertUnwindSafe(|| chain.find_path(rcx, ..)))`. On `Err(p)`, if `p.is::<TsThrow>()`, the result is `NotFound`; **otherwise `resume_unwind(p)`**, which makes it a tick fatal.
  - Scratch consistency after a caught `TsThrow`: every scratch user re-stamps per query (`PriorityQueue.ts:169-177`). Every `busy` flag is reset by a drop guard, matching TS, whose guard is a `try/finally` (`GameMapImpl.ts:477-500`).
  - `DebugSpan.ts:117` is the only other `catch` in the engine [V grep], and it is dropped.
- **Inside a tick.** `GameRunner` catches any throw from `game.executeNextTick()`, sets `isExecuting = false` and returns `false` (`GameRunner.ts:231-246`) [V]. `currTurn` has already advanced and `_ticks` is not incremented. **The runner can keep stepping**, so production replays that hit a tick error carry on. Rust [D] (review 0 #11):
  - `Game::step` wraps the tick in `catch_unwind`.
  - On a `TsThrow`, it finishes the TS-visible effects of the throw:
    - it moves any `exec_inbox` leftovers into `st.uninit`, because TS's `addExecution` had already pushed them to `unInitExecs`
    - it applies the throwing execution's queued `exec_cmds` (S0: TS had already pushed the warhead into `warheadExecutions` before the throw, so its `BindWarhead` must land; `apply_exec_cmds` takes its queue before applying, so a throw *inside* the cancel loop drops the remaining commands, as in TS)
    - it clears `events`.
  - It then returns `Err(Fatal::Tick { tick, msg })` **without** poisoning the game.
  - The init loop never vacates `uninit` entries and appends `inited` only after the loop (§4.2), so the post-throw state equals TS's: `uninit` is unchanged, plus anything added before the throw; `live` is unchanged; executions that were already inited stay mutated in place; `ticks` is not incremented.
  - On a non-`TsThrow` panic, it returns `Err(Fatal::Bug { .. })` and sets `stuck`. There is no TS equivalent, so the game cannot continue faithfully.
  - Rust compound operations that TS performs in one statement (`TileSet.add` → dense push + `pos_*_set`) contain no panic point between their paired writes, so post-throw state matches at TS-statement granularity.
- **Before a tick.** The 3 lobby intents throw in `createExec` (`ExecutionManager.ts:147-148`) *outside* the try (Q-INTENT-THROW). The runner stays `isExecuting = true` forever. Rust: `step` first converts all intents; on an unknown type it returns `Err(Fatal::Intent)` with no state change and sets `stuck` (later `step`s return the same error).
- **Search/rollout users** should discard any fork whose `step` returns `Err`.
- `of-cli` installs a quiet panic hook that ignores `TsThrow` payloads inside the stepper boundary, so expected caught panics do not spam stderr.
- `panic = "unwind"` is required in every profile.

### 4.7 Iterating a TS array that is replaced by `filter` (review 0 #1)

TS deletes from several per-player and per-game lists by **replacing** the field with a filtered copy [V]:

| TS list | Accessor returning the live array | Reassigned by |
|---|---|---|
| `PlayerImpl._units` | `units()`, `units([])` (`PlayerImpl.ts:458-459,470-472`) | `UnitImpl.delete` (`:353`), `UnitImpl.setOwner` (`:275`) |
| `PlayerImpl._outgoingAttacks` | `outgoingAttacks()` (`:1900-1901`) | `AttackImpl.delete` (`AttackImpl.ts:68-70`) |
| `PlayerImpl._incomingAttacks` | the raw field only (`incomingAttacks()` returns a fresh `filter`, `:1903-1904`) | `AttackImpl.delete` (`:62-66`) |
| `PlayerImpl._alliances` | `alliances()` (`:764-765`) | `GameImpl.detachAlliance` (`GameImpl.ts:1143-1144`) |
| `GameImpl.allianceRequests` | the field (`PlayerImpl.ts:293`, view only) | `:469`, `:507` |

Two more fields are filter-reassigned but private and never iterated across their own reassignment, so they stay plain `Vec` with `retain`, annotated `// filter-reassign: not iterated across` [V]: `MIRVExecution.stagedTargets` (`:237`) and `NationAttackBehavior.followedLandings` (`:130`). The `GameImpl.ts:792-869` reassignments belong to `removeExecution` and the takeover hooks, which are out of scope. PORTING_GUIDE §14 has the grep. It misses the multi-line form in `AttackImpl.ts:61-71`, which is why the table lists sites by hand.

A `for..of` that is already running keeps walking the **old** array object, so it visits every element that was there at loop start. A Rust `retain`, combined with a cursor that re-reads the length, would skip the element after each removal.

**Rule** [D]:
- These lists are `JsArr<T> { v: Vec<T>, gen: u32 }`. `filter_reassign(pred)` does `retain` and then `gen += 1`; `push` appends.
- Any loop over the live array whose body can (transitively) remove from that list iterates `JsArr::iter_snapshot(sc)`. That takes a snapshot `Vec` from the scratch pool at loop start and returns it at loop end.
- **Pushes during the loop:**
  - While `gen` is unchanged, the old array object *is* the current one, so JS visits pushes made to it. `next()` extends the snapshot with `v[snap.len()..]` as long as `gen == gen0`.
  - After a reassignment, pushes go to the new array and are not visited.
  - The one corner this cannot see is a push followed by a reassignment inside the same loop step. A site where that is possible needs a site-specific proof or custom handling, annotated `// iter-snapshot: push-then-reassign …`.
- Every such loop is annotated `// iter-snapshot: filter-reassign` and has a unit test.
- The three known hot sites:
  - `PlayerExecution.ts:60-79`: every player, every tick; it deletes or captures structures while iterating `units()`. `captureUnit` pushes to the *captor's* list, not this one.
  - `PlayerExecution.ts:107-111`: alliances expire during the loop.
  - `AttackExecution.ts:169-178`: merges and deletes all outgoing attacks on the same target, inside the loop.
- A loop whose body provably cannot remove from its list may iterate the live `Vec` by index, annotated `// iter-live: <reason>`.
- Copies (`incomingAttacks()`, `units(type)`, `filter(..)` results) are already snapshots: iterate the copy.

---

## 5. View and update work that is dropped, and why each drop is safe

| Dropped | TS | Feeds back into state? |
|---|---|---|
| `GameUpdates` map + `addUpdate` | `GameImpl.ts:285-287,1888-1894` | No. `updates` is only returned to `GameRunner` (`:664`), which packs it for the client (`GameRunner.ts:289-328`). No engine file reads `updates[` [V grep]. |
| `PlayerImpl.toUpdate` / `toFullUpdate` / `lastSentUpdate` / stats and attack quads | `PlayerImpl.ts:241-421`; `GameImpl.ts:631-648` | No. `toFullUpdate` calls only pure getters: `alliances`, `targets` (filter), `outgoingEmojis` (filter + sort of a **copy**), `incomingAttacks` (filter), `isTraitor`, `inDoomsdayClock`, `isDecaying`, and the read-only `stats().getPlayerStats` (`:286-421`, `:756-1060`) [V; review 0 re-checked]. `lastSentUpdate` is read only by `toUpdate` itself. |
| `recordTileUpdate` / `tileUpdatePairs` / `drainPacked*` | `GameImpl.ts:667-700` | No (output buffers). Re-implemented as the optional caller-owned `TileJournal` (§10). |
| `recordMotionPlan` / `motionPlanRecords` / `packMotionPlans` | `GameImpl.ts:702-740`, `engine-lib/src/game/MotionPlans.ts` | **Partly.** `planDrivenUnitIds` (Set) is CS state and gates `maybeAddUnitUpdate` (`:717-725`), so it is kept. The records array is dropped. |
| **`Unit.toUpdate()`**, including through **`Unit.touch()`** | `UnitImpl.ts:145-147,159-203` | **YES, one field.** `toUpdate` evaluates `{ ...this.warshipState() }` for warships (`:169-172`). `warshipState()` writes `this._warshipState.isInCombat = this.isInCombat()` (`:437-442`) [V]. That field is snapshotted (`:820-830`) and therefore in CS U, and `updateWarshipState` resets it to `undefined` (`:459-466`). The sim never reads the stored field except through `warshipState()`, which refreshes it first (`WarshipExecution.ts:84`), so **sim behaviour** is unaffected. **CS** would differ. |
| `GameImpl.hash()` (Hash update every 10 ticks) | `GameImpl.ts:649-655,742-751` | No; read-only sums. Ported as `State::legacy_hash()` **on demand** for record replays (SPEC Q-UPHASH). |
| `placeName` / `placeSpawnName` / `playerViewData` / `lastSpawnTiles` | `GameRunner.ts:255-287`; `game/NameBoxCalculator.ts` | No (runner-local view data, read-only on game state; review 0 re-checked). |
| Unit update packing, nuke impact queue | `GameRunner.ts:289-328`; `GameImpl.ts:335-343` | No. |
| `displayMessage` / `displayChat` / `displayIncomingUnit` / `sendEmojiUpdate` / `renderNumber` | `GameImpl.ts:1173-1350`, `engine-lib/src/Format.ts` | No; `addUpdate` only. Emoji *state* (`outgoingEmojis_`) lives in `PlayerImpl.sendEmoji` (`PlayerImpl.ts:1030-1043`) and is kept. |
| `DebugSpan`, `performance.now`, `console.*` | | No (doc 01 §7). |

**The view side-effect shim** [D].
- Each TS expression `X(unit.toUpdate())` and each **`unit.touch()`** becomes `cx.st.unit_touched(u)`.
- `maybeAddUnitUpdate(u)` becomes `if !st.plan_driven_unit_ids.contains(&id) { st.unit_touched(u) }`.
- `unit_touched` performs **only** the state writes of `toUpdate`: today, `if let Some(ws) = &mut unit.warship_state { ws.is_in_combat = Some(st.ticks - ws.last_combat_tick <= 3) }`.
- **The shim is never dropped**, even when the surrounding TS line looks like pure view code.

The complete list of call sites at the pin [V: `grep -rn '\.toUpdate()\|\.touch()' engine/src`, excluding alliance-request and player updates]:
- `toUpdate()` inside the unit and game classes (owners P1/P2):
  - `UnitImpl.ts:133,146 (touch body),228,281,314,334,356,467,478,510,538,552,566,587,674,737,762,780,794`
  - `PlayerImpl.ts:1427` (`buildUnit`, before `addUnit`)
  - `GameImpl.ts:723` (`maybeAddUnitUpdate`, gated by `planDrivenUnitIds`)
  - `GameImpl.ts:574` (`addViewResync`; host-triggered after a restore and unreachable from a `Turn`, so not ported).
- **`touch()` callers in executions** (review 0 #9). These are the ones a P7 or P8 porter is likely to delete as "view only":
  - `WarshipExecution.ts:86, 687, 707` (P7). Losing these loses the `isInCombat` write on warships.
  - `TradeShipExecution.ts:134` (P7)
  - `NukeExecution.ts:504` (P8).

`UnitImpl.ts:467` is the important one inside `UnitImpl`. `updateWarshipState` rebuilds `_warshipState` without `isInCombat` (`:459-466`) and then immediately calls `toUpdate()`, which writes it back. If the shim is skipped there, CS U shows `isInCombat` absent where TS has a boolean.

Separately, the **sim-side getter `warshipState()` keeps its write**. `WarshipExecution.ts:84` calls it directly. The Rust `UnitMut::warship_state()` must perform the same `is_in_combat` write before returning, because it is a mutating getter in TS, not a pure read.

**Guards:**
- A unit test hashes the bodies of `UnitImpl.toUpdate`/`touch`/`warshipState`/`transportShipState` at the pin, and the list of `touch()`/`toUpdate()` call sites from the grep above. It fails if either changes on an upstream bump.
- A second test asserts that `PlayerImpl.toFullUpdate` calls only the getter set listed above.
- So an upstream bump that adds a side-effecting getter, or a new `touch()` caller, fails CI instead of silently desyncing.

---

## 6. Config / EngineConfig port

- TS: `class Config` (`engine-lib/src/configuration/Config.ts:87-460`) and `class EngineConfig extends Config` (`engine/src/configuration/EngineConfig.ts:125-830`).
  - They read `_gameConfig`, with memo maps `unitInfoCache`/`engineUnitInfoCache` (derived).
  - `unitInfo(t).cost` is a closure `(game, player) => Gold` built by `unitCost(type)` (`EngineConfig.ts:338-460`) [V].
- Mutators exist: `setDifficulty`/`setGameType` (`EngineConfig.ts:134-140`), used only by `applySingleplayerConfig` (a takeover hook, out of scope).
- Rust [D]: one `EngineConfig { gc: GameConfig, listed: bool, unit_info: [UnitInfo; 16] }`, built once and shared as `Arc`. Costs become `fn unit_cost(&self, t: UnitType, st: &State, p: SmallId) -> i64`.
- **BigInt conversions in config are strict** (§7.2): `EngineConfig.ts:281, 464, 684` use `from_floor_strict`/`from_number_strict` as TS writes them, never the clamping `to_int`.
- Formula functions take `&impl PlayerLike` (the `engine-api/src/game/ReadViews.ts` port), so they can be **checked against the `config` vector suite without building a `State`**. VECTORS.md §3 "config" uses duck-typed stubs, so 79,584 rows test `of-engine::config` at L0, before any state exists.
- `GameConfig` is a serde struct reproducing zod defaults and optionality (`engine-api/src/Schemas.ts`). The scenario's `gameStart.config` is already `GameConfigSchema`-parsed (SPEC §2.2), so the Rust side only needs faithful `Option` handling and must not re-apply defaults that zod already applied.

---

## 7. Determinism rules (summary; the mechanics are in PORTING_GUIDE)

### 7.1 Numbers

- **JS `number` → `f64` by default.** A variable may be an integer type (`u32`/`i32`/`i64`) only if all of the following hold:
  - every assignment is integral
  - every `+ - *` result stays below 2^53 (or below 2^31 when TS coerces with `|0`)
  - every `/` is immediately floored or `|0`-ed in TS
  - it is never compared with a fractional value.

  Tile refs, ids, ticks, counts and smallIDs qualify. Troops on units (`UnitImpl._troops: number`), attack troops (`AttackImpl._troops`) and every config formula stay `f64`.
- **int32 ops** (`|0`, `>>>`, `<<`, `>>`, `Math.imul`) use `of_det::js::{to_int32, to_uint32, imul, shl, sar, shr}` on the f64 → int32 conversion. `as i32` is never used on an f64 (Rust saturates; JS wraps).
- **f32:** `Float32Array` stores (`FlatBinaryHeap`, `PriorityQueue`, `AStar.AbstractGraph.gScore`) become `x as f32`, round-to-nearest-even, identical to `Math.fround` (VECTORS `jssem`/`Math.fround`). Comparisons widen the stored f32 to f64 and compare it against the **un-rounded f64** operand exactly where TS does (`FlatBinaryHeap.ts:36-38`, Q-F32HEAP; vector gap ledger t093).

### 7.2 BigInt → `i64` (troops, gold, health, stats counters)

**Storage is `i64`; `of-det`'s BigInt helpers are checked `i128`** (`crates/of-det/src/bigint.rs`).
- The engine does its hot arithmetic natively on `i64`, with `overflow-checks = true` in the conformance profile.
- It calls `of-det` for conversions, through the strict wrappers below.
- A value outside `i64` is a non-`TsThrow` fatal: TS BigInt is unbounded, so this is a port limitation and must be loud. Ledger t096 tracks the domain question.

**Bounds** [V]:
- **Troops:** `maxTroops ≤ 2·(numTiles^0.6·1000 + 50000) + cityTroops`, × 1.25 for Impossible nations (`Config.ts:344-383`). On World (651,569 land tiles) that is about 6.3e6 + cities. The infinite-troops cheat gives 1e9 (`:353-354`).
- **Clamp:** `toInt(±Infinity)` clamps to ±(2^53−1) (`engine-lib/src/Util.ts:232-240`). The sum of two clamped values is 2^54, still about 2^9 below i64::MAX.
- **Gold:** grows by per-tick income over at most ~10^5 ticks. Every other gold formula is linear (MIRV `25_000_000n + mirvsLaunched * 15_000_000n`, `EngineConfig.ts:373`; `cost * 5n`/`* 20n` save-up targets, `NationStructureBehavior.ts:913-917`; `atomCost * BigInt(bombsToFire)`, `NationNukePlanning.ts:392`).
- **Exception, the one compounding bigint: NationNukeBehavior perceived nuke prices** (corrected 2026-11-05). Each send stores `inflated * 150n / 100n` (atom; hydrogen `* 125n`) of the *inflated* price (`NationNukeBehavior.ts:1104,1109`), and Impossible SAM salvos send several bombs per tick, so within one 3000-tick memory window the price compounds geometrically (Team games never forget, `:907`). The old "prices of about 10^6–10^9" bound was wrong: World FFA rollouts reached 3.8e15, where the decay product `(perceived − real) * BigInt(left)` (`:915`) leaves i64 (1.7% of rollouts stopped with a Rust-only `Fatal::Bug`). These two fields are therefore **`i128`** (`exec::nation::nuke::PerceivedCost`), with the decay and send products computed by an exact split (`mul_div_nonneg`: `floor(a·m/d) = (a/d)·m + floor((a%d)·m/d)`) so no intermediate leaves i128. Exact for every price below ≈1.1e38 (`i128::MAX / 1.5`); beyond that it stays a loud fatal. CS exports them as BIGINT over the full i128, byte-identical to TS.
- **Conclusion:** every other reachable value is below about 2^50, and i64 holds 2^63. i128 storage for troops/gold buys nothing and costs two-register arithmetic on the hottest path (troops per conquered tile, `AttackExecution.ts:354-356`).

**Conversions** [D] (review 0 #8). These are the `of-det` additions (t088); each raises a `TsThrow` in **every** build where TS throws:

| TS | Rust | Throws (TS) |
|---|---|---|
| `toInt(x)` (`Util.ts:232-240`) | `of_det::util::to_int_strict(x) -> i64` | NaN → RangeError. ±Inf **clamps** to ±(2^53−1). |
| `BigInt(Math.floor(x))` | `of_det::bigint::from_floor_strict(x) -> i64` | NaN, ±Inf → RangeError (**no clamp**) |
| `BigInt(n)` | `of_det::bigint::from_number_strict(n) -> i64` | NaN, ±Inf, non-integer → RangeError |
| `StatsImpl._bigint(v)` for a number | `from_floor_strict` | as above, but only for players with a clientID (§3.10) |
| `a / b`, `a % b` | `i64` ops after `if b == 0 { ts_throw!("Division by zero") }` | `b == 0n` → RangeError |

Sites that look like `toInt` but are plain `BigInt(…)`, which **must not clamp** [V, review 0]: `configuration/EngineConfig.ts:281, 464, 684`; `nation/NationStructureBehavior.ts:881`; `DonateGoldExecution.ts:123`; every `StatsImpl._bigint` call.

Other semantics, exact with i64 in range:
- `/` truncates toward 0, which is the same in Rust and JS.
- `Number(b)` → `b as f64`, exact below 2^53; above that, both languages round to nearest-even.

**Guard:** `overflow-checks = true` in the conformance profile, plus `debug_assert!(x.unsigned_abs() < 1 << 62)` in `addGold`/`addTroops`. Both are non-`TsThrow`, so they surface as `Fatal::Bug`.

### 7.3 Order

- Every TS `Map`/`Set` whose iteration can reach the sim → `of_det::JsMap`/`JsSet`, or `SmallJsMap`/`SmallJsSet` for the many tiny ones (§8.2). These are tombstoned dense storage with JS insertion-order and mutation-during-iteration semantics, **including `clear()` during iteration**: a live iterator continues and visits entries added after the clear (§12 G7).
- `std` `HashMap`/`HashSet` iteration is banned by clippy (`disallowed-methods` on `iter`/`keys`/`values`/`into_iter` for `std::collections::HashMap`/`HashSet`). `FxHashMap` is allowed only through the lookup-only wrapper.
- `.sort(cmp)` → `of_det::js_sort_by` (the V8 TimSort port) at **all 50 live call sites**, even the consistent ones, so the comparator call sequence matches too. It is cheap and mechanical (PORTING_GUIDE §6, §14.5).
- Arrays replaced by `filter`: §4.7.

### 7.4 Floating point across architectures

- Rust/LLVM never fuses `a*b+c` without `mul_add`, which clippy bans.
- `sqrt` is correctly rounded on both architectures.
- No `powf`/`exp`/`ln`/`sin`/`atan2` from std: all of them go through `of_det::detmath` (banned in clippy). VECTORS shows that DetMath ports exactly with plain f64 (332,723 rows, 0 failures in an independent Python port).
- NaN payloads matter only where DetMath reads raw bits of a NaN. V8 quietens signalling NaNs (VECTORS §5), so `of-det` must do the same at those entry points (`js::quiet_nan_bits`).
- **Cross-arch check:** SPEC R5 (t041). The Rust port runs the same CI corpus on aarch64 host.

---

## 8. Clone strategy

### 8.1 Mechanics

1. **`#[derive(StateClone)]` on every state type** (review 1 #3).
   - `#[derive(Clone)]` never generates `clone_from`, so the default `clone_from` reallocates.
   - `StateClone` (from `of-engine-derive`) generates `Clone` with a `clone_from` that forwards to every field's `clone_from`.
   - It is required on `Game`, `State`, `ExecStore`, `Slots<T>`, every exec struct and behaviour, `Player`, `Unit`, `Attack`, `Arena<T>`, `GameMap`, `UnitGrid`, `RailNetwork`, the water types and `Stats`.
   - `of-det`'s containers (`OrdMap`/`OrdSet`, `SmallJsMap`/`SmallJsSet`, `FlatBinaryHeap`) implement `clone_from` by hand (t088). `Vec::clone_from` reuses capacity and calls `clone_from` element-wise on the common prefix.
2. **No `Rc`/`RefCell`/`Box<dyn>`/`Box<T>` in `State` or `ExecStore`.** A marker trait `PlainState` is required by `StateClone`. A compile-fail test covers it.
3. **Per-kind execution arenas** (§4.2). `live` and `uninit` are `Vec<ExecId>` (memcpy). The arenas clone element-wise into reused buffers.
4. **`Arc` for immutable or rarely-mutated bulk data, with O(1) refcount updates per fork** (review 1 #7):
   - `cfg`, `statics`, `infos`, `humans`/`nations`
   - main and minimap terrain (2)
   - `water.components`, `water.graph`
   - one `Arc` per stored path in the memo and in `seg_vis.overlay`. These are the only per-entry refcounts; their count is measured in t091, and the fallback is a lineage-shared, append-only path store addressed by `u32`.
   - Nothing per player or per stepper.
5. **Pooling is mandatory** (review 1 #8). An unpooled 20 MB clone touches fresh pages: about 5 k first-touch faults on 4 KB pages, which can cost 2–5 ms on its own. The primary fork API is `src.clone_into(&mut dst)`, i.e. `dst.clone_from(src)` on a pooled `Game`. `Game::clone()` exists for convenience.
6. **GC before forking** (§3.11). `fork_into(&mut self, dst)` runs `collect_garbage` on the source first.

### 8.2 Small containers (review 1 #4)

The TS census at World tick 5000 counted 11.6 k arrays, 5.4 k Sets and 3.3 k Maps (`docs/recon/12_clone_prototype.md:154`). Each non-empty `OrdMap` is at least 2 heap blocks (entries + index).

`of-det` adds `SmallJsSet<K, N>` and `SmallJsMap<K, V, N>`:
- inline arrays with tombstones and linear search
- the same JS semantics (insertion order, mutation during iteration, `clear` during iteration)
- spilling to an `OrdMap` only above `N`.

Use them for:
- per player: `relations`, `embargoes`, `outgoing_quick_chats` (PORTING_GUIDE §5.1)
- rail: `TrainStation.railroads`/`railroadByNeighbor` (`TrainStation.ts:75,77`) and `Cluster` sets (`:213-214`)
- nation behaviour trackers (`NationWarshipBehavior.ts:31-37`, `NationEmojiBehavior.ts:62`)
- `land_neighbors`.

Per-player arrays (`units`, the attack lists, `alliances`) stay `JsArr`/`Vec` (one block each, often empty, so no allocation).

### 8.3 Estimated clone bytes, World mid-game [E]

Basis: 2000×1000 = 2.0 M tiles, 651,569 land tiles, 472 players at start; doc 01 §10, `docs/recon/01_map_sizes.tsv`. The revised table follows review 1 and assumes the fixes above.

| Component | Formula | Bytes |
|---|---|---|
| `map.state` u16 | 2 B × 2,000,000 | 4.0 MB |
| minimap state | never written (§3.6.1) | 0 |
| terrain map + minimap | `Arc`, COW | 0 (2.5 MB once, on a water edit) |
| `pos_tiles` + `pos_border` | 2 × 2.6 MB | 5.2 MB |
| player tiles, dense, ≤ 25 % tombstones | ≤ 1.25 × 651,569 × 4 B | ≤ 3.3 MB |
| border tiles, dense | 100–200 k × 4 B × 1.25 | 0.5–1 MB |
| attack borders + heaps | Σ fronts × (12 B set + 8 B heap) | 1–3 MB |
| units (GC on) | ≤ 5 k live + recent dead × ~160 B | ≤ 1 MB |
| players | 472 × ~0.7 KB inline + small containers + memos | ≈ 1 MB |
| executions (per-kind arenas) | ~800–1,050 live (doc 01 §2); CS E was 240 KB on Australia (SPEC §6.2) | 0.5–1 MB |
| rail (measure at t4000/t8000) | stations, railroads + tiles, grid, train `pathTiles` | 0.5–2 MB |
| water: `seg_vis` bits + memo map (+ overlay) | memo ≤ ~10 k entries × ~24 B + index | 0.2–1 MB |
| unit grid, water scalars, stats | | < 0.3 MB |
| **Total** | | **≈ 17–22 MB** |

Estimates:
- **Pooled:** 1.5–3 ms with fewer than ~2 k allocations (mostly memo paths and changed-shape containers).
- **Unpooled:** 3–6 ms.
- **Without** per-kind arenas, `StateClone` and v1 GC, expect about 25–35 MB, 15–25 k allocations, and an unpooled clone that misses 5 ms by tick 8000. That is why those are v1, not options.

**Gate (t091):** `of-cli fork-bench --map World` at ticks 1000, 4000 and 8000, on the x86 laptop and on the aarch64 box. It reports:
- pooled and unpooled p50 and p95
- allocations per fork (counting allocator) and page faults (`getrusage`)
- bytes per component (the table above), including rail and the water memo.

Pass criteria:
- pooled p95 ≤ 5 ms (target ≤ 3 ms)
- pooled allocations p95 ≤ 2,000.

### 8.4 Lookahead with thousands of retained nodes (review 1 #18)

About 9 MB of each fork is three arrays: `map.state` and `pos_tiles`/`pos_border`. A rollout of 10–50 ticks touches a small fraction of them, so retaining 1,000 tree nodes costs 15–20 GB.

### 8.5 Write accessors, frozen at S0

Every write to `map.state` goes through `TileStateBuf::{set_owner_id, set_fallout, set_defense_bonus}`, and every write to the position arrays goes through `pos_tiles_set`/`pos_border_set`. Reads go through `get`. The buffers are private fields.

Chunked copy-on-write (`Vec<Arc<[u16; 32768]>>`, copied on first write) can then be added later behind the `fork-bench` gate, without touching ported code. It extends to the two position arrays.

---

## 9. Canonical-state (CS v1) exporter and loader

- `trait Canon { fn canon(&self, cx: &mut CanonCx) -> of_canon::CsValue; fn uncanon(v: &CsValue, cx: &mut UncanonCx) -> Self; }` is implemented next to every state struct and every execution struct by the package that owns the type.
  - Its fields must mirror the snapshot schema dump (`conformance/schema/snapshot_schemas_5b75bd3.txt`) **in field order**, which is the order in which references are visited (SPEC §1.2.1 step 4).
  - Fields that are not in the snapshot (`seg_vis`, chain slots, memos) are not exported.
- `trait RefVisitor` (`visit_unit`, `visit_attack`, `visit_station`, `visit_cluster`, `visit_railroad`, `visit_exec`, `visit_chain_version`, …) is implemented together with `Canon`.
  - The exporter (P13) uses it for the SPEC §1.2.1 drain loop: dead-but-referenced units and attacks become rows, detached warhead execution keys come out in (MIRV position, warhead index) order, and empty clusters get `e<n>` keys.
  - The v1 GC uses it to mark (§3.11).
- Keys are the SPEC §1.2 keys, computed from TS-visible ids, never from Rust slots: smallID, unit id, (attacker, attack id), alliance id, `"req:rec:createdAt"`, station id, railroad id, cluster key, and execution position (live list, then `uninit`, then detached).
- **Typed-array kinds must match:**
  - `P.tiles` is u32 (live order)
  - heap `pri` is f32 and `tiles` is u32, both in heap array order
  - `W.components` is **u8 until promotion, u16 after** (`ConnectedComponents.ts:96-116`)
  - `T.map`/`T.miniMap` are u16.
- `num` fields encode as INT when they are safe integers (SPEC §1.1). That is handled by `of-canon`; `of-engine` hands it `f64`.
- **Loader:** `Game::from_canon(start, maps, cs)` loads any *resumable* dump (SPEC §2.5, R3, §3.2.2), with **TS-restore semantics** for non-snapshotted state:
  - the water graph is rebuilt from the components (resumable dumps have `waterGraphDirty == false` by definition)
  - `seg_vis` is empty
  - there is one fresh chain slot
  - memos are empty
  - steppers keep their snapshotted path and stagger fields (`WaterPathFinder.fromState`, `PathFinder.ts:326-342`).
- **Conformance CLI** (`of-cli`):
  - `engine-rs conform --scenario <dir> --json` implements SPEC §3.1–§3.3: per-tick K, section D at checkpoints, first bad tick, section/row/field diff and the JSON report.
  - `--probe` enables the **exec-boundary hook** (SPEC §3.2.6). `Game::step_with_probe(&turn, |pos, kind, st| ..)` is called after each `live[i]` tick and computes the K-restricted probe digest {T.map, P scalars, U, A}. The hook costs nothing when off (a generic closure parameter monomorphised to a no-op).
  - `--prng-trace` (§4.5) compares per-nation PRNG draw sequences against the TS probe's trace.
- **Fast K.** A per-tick K digest is needed in CI. The exporter streams CSE1 straight into FNV64W without building a `CsValue` tree for typed arrays (it writes `INT(len)` for the K projection). Target: ≤ 2 ms/tick on World [E]. That is too slow for use per search node; see `fingerprint64` in §10.

---

## 10. Embedding API

```rust
// Construction (== createGameRunner + GameRunner.init, GameRunner.ts:48-100,183-207)
let statics = MapStatics::load(&map_files)?;          // once per process: terrain, land index, graph0, components0
let mut g = Game::new(&game_start_info, statics.clone())?;

// Stepping (== GameRunner.executeNextTick minus view)
let ev: StepEvents = g.step(&Turn { turn_number, intents })?;   // typed Intent enum; serde only at the edge
// ev = { tick, winner_set: Option<Winner>, killed: SmallVec<[SmallId; 4]>, .. }: lets rollout users stop early without reads
// Err(Fatal::Tick{..}) = TS tick throw (game stays steppable, TS-exact); Err(Fatal::Intent|Bug) = stuck (§4.6)
g.step_with(&turn, &mut my_scratch, Some(&mut journal))?;      // caller-owned Scratch / optional TileJournal

// Debug/test injection: exactly GameRunner's addExecution(...createExecs(turn)) followed by `extra`,
// i.e. appended after this turn's intent executions, before the tick.
g.step_with_execs(&turn, [Exec::Attack(AttackExec::new(..))])?;

// Fork (primary entry; pooled)
src.clone_into(&mut pooled);                          // == pooled.clone_from(&src): reuses every buffer
src.fork_into(&mut pooled);                           // GC on src first (§3.11), then clone_into
pooled.step(&my_candidate_turn)?;                     // exact-model lookahead: opponents are deterministic (doc 08 §7.8)

// Read API (zero-copy borrows, no allocation)
let v = g.view();
v.ticks(); v.in_spawn_phase(); v.owner(tile); v.has_fallout(tile); v.map_state_raw() /* &[u16] */;
let p = v.player(sid); p.troops(); p.gold(); p.num_tiles_owned(); p.tiles() /* iterator, TS order */;
p.border_tiles(); p.units(); p.outgoing_attacks(); p.alliances(); p.is_alive(); p.relation(other);
v.units_of_type(UnitType::Warship); v.attack(aref).troops();
// Legality oracles (same code paths as GameRunner player_actions, GameRunner.ts:337-439); they may need scratch:
let o = g.oracles(&mut sc);  o.can_attack(sid, tile); o.can_build(sid, UnitType::City, tile); o.best_transport_ship_spawn(sid, tile);
// Debug
g.fingerprint64();                                    // cheap fork-identity check (scalars, counters, sampled tiles), not CS
g.canon_k(); g.canon_d(); g.legacy_hash();
```

- **An embedder's player** is a `Human` with a clientID in `GameStartInfo.players`, exactly like the harness's scripted players (SPEC §4.5 `scripted2`). Its intents go through `step`, so legality, latency-free application and stats (§3.10) are faithful.
- **No globals.** There are many independent `Game`s per thread or core. Process-wide sharing consists only of immutable `Arc`s and the deterministic `OnceLock` segment values (§3.8). `Scratch` is per thread. `assert_impl_all!(Game: Send)` and `'static`. Rayon or threads parallelise across games outside the crate.
- **Rollout determinism.** `step` is a pure function of `(Game, Turn)`. A clone followed by the same turn sequence is bit-identical. The `fork_identity` property test checks this: random fork points, GC at random points, 200 ticks, K equal every tick, plus a water-nuke variant (§12 G12).

---

## 11. Performance design (after conformance)

Ordered by expected value. Hot-path shares:
- **World** (`docs/recon/06_benchmark.md:78`; doc 01 §2; doc 08 §0/§7.1): Attack 31–38 %, PlayerExecution/removeClusters 20–36 %, Nation 18–19 % (NationStructureBehavior 7.0 %), view work 8–10 %, TradeShip 5.3 %, Warship 4.5 %.
- **Big FFA** (`docs/recon/11_true_workload_bench.md:22,172-174`): pathfinding 14 %, trains/rail ~14 %, trade ships 6 %, warships 6 %, removeClusters 8 %.

1. **Free wins from the port itself:**
   - no view work (−8–10 %)
   - i64 instead of BigInt on every conquered tile
   - no GC
   - inline `Prng`
   - `FlatBinaryHeap` with `Vec<f32>` + `Vec<u32>` and lazy capacity (TS allocates 1024 slots per `AttackExecution`, `FlatBinaryHeap.ts:14`)
   - Fx-hashed `OrdMap` index (§12 G7)
   - double-buffered exec lists
   - `per_player_bookkeeping` compiled out.
2. **AttackExecution conquest loop:**
   - struct-of-arrays heap
   - `hasUnitNearby(DefensePost)` per tile (`AttackExecution.ts:372-379`) answered from the per-cell DefensePost count in `UnitGrid`, with the result unchanged
   - a `largestTerritoryBonus` memo keyed on the exact `numTiles` argument (pure DetMath of an integer; doc 08 §3.1)
   - the attack border stays an Fx-indexed `OrderedTileSet`.
3. **`removeClusters`/`floodComponent`:**
   - a fixed `Vec<u32>` stack instead of a JS array (doc 08 §7.4: 1.6–2.3×)
   - `neighbors8` inlined
   - **flat cluster scratch:** `clusters: TileRef[][]`, `clusterTileTIds: Int32Array[]`, `boxes` and `isPinched` are built per call (`PlayerExecution.ts:142-151,342-357,1063-1090`). They become `sc.clusters { tiles: Vec<u32>, offsets: Vec<u32>, tids: Vec<i32>, boxes, pinched }`. This is a mechanical change, and the result order is unchanged.
   - Incremental or dirty-region recomputation **only** with a proof that the outputs match: the same clusters in the same order, the same bounding box, the same annexation decisions. It is gated by L3 on the nightly corpus plus a differential fuzz test that runs the old and new kernels on 10^4 random territories (ledger t075 context).
4. **Water pathfinding (14 % in big FFA):**
   - water A* and `PriorityQueue`/`BucketQueue` with typed buffers in `Scratch`; the f32 priorities stay
   - the minimap path transformer (`MiniMapTransformer.ts`) writes into a reused buffer
   - the literal `WaterPathMemo` (on, as in TS) recovers TS's ~64 % hit rate.
5. **Rail (14 % in big FFA):**
   - `usedTiles` → `VecDeque`
   - u32 rail-grid keys plus intrusive per-cell lists (§3.9)
   - train `pathTiles` as a borrowed concatenation where the train never mutates it (measure first).
6. **Trade-ship and warship scans:** `nearbyUnits` writes into a reused `Scratch` buffer instead of allocating per call.
7. **Nation AI (18–19 %):**
   - NationStructureBehavior tile scans use reused buffers
   - candidate lists (`units(type)` copies) become borrowed slices where §13 of PORTING_GUIDE allows
   - profile per behaviour.
8. **Allocation-free tick.** Every per-tick `Vec` becomes a reused buffer in `Scratch` or a `SmallVec`. The gate is a counting allocator in `bench`: allocations per tick are reported.
9. **Struct-of-arrays where hot:** per-player `troops`/`gold`/`num_tiles` in parallel arrays if profiling shows player-struct cache misses; unit `tile`/`type`/`owner` columns for `UnitGrid` scans.
10. **Precomputation (pure):** the `PseudoRandom(123)` post-warmup state as a `const`, and the `land_index`.
11. **Throughput across games:** one game per core, plus `clone_into` pools. `mimalloc` is in the binary. The NUMA/aarch64 check is on the aarch64 box (t041/t036).

**Rule for every optimisation:** it lands only with L3 green on the CI corpus, L4 green on the nightly corpus, and a micro-benchmark delta recorded in WORKLOG.

**Target accounting** [E]:
- The 5× target is aggressive: doc 08 §3.1 estimates 2–3× for the same algorithms.
- Items 1 and 2 are expected to reach about 3×. Items 3–7 cover the remaining 40–50 % of time.
- If 5× is missed after items 1–8, the next levers are item 3's proof-gated incremental kernel and the per-behaviour nation profile.

---

**Status after P3 (2026-11-05, branch `perf`, tag `perf1`, t066; measured, README §9):** World FFA
Impossible p1ffa0001 450 -> 747 ticks/s on one aarch64 core (TS 156: 2.88x -> 4.79x; quiet runs up
to 4.94x); pooled fork p95 <= 1.89 ms at ticks 2000/8000/14000. What landed vs the list above: the
exact TS memos the state already declared (`unitCount`, `unitsByType`, `myUnits`, `nearby`) are now
filled where the readers hold `&mut State`; a new exact `State::shore_memo` (`closestShoreByWater`,
keyed on the shores list + terrain gens + graph version, bypassed while HPA segments may be
history-dependent); order-free floods as scanline fills (item 3, without the incremental kernel);
`closestTwoTiles`' stable key sort; MinHeap hole walks. Not done: item 3 incremental kernel (t320),
mimalloc (t318, +2.5% measured), rail/trade/warship scratch (items 5-6), nation sail() memo (t319),
water A* kernels (t322). The `Cell`/`RefCell` ban means a memo can only be filled from a `&mut`
reader; `&State` readers still recompute (same value).

**Status after t409 H5 (2026-11-07, branch `opt-H5`; README §12):** item 3's incremental kernel landed in proof-gated
form: `Player::conn4` (one 4-connected component) and per-tile component labels kept by local gain/ring-loss rules on
every ownership change (`map/conn4.rs`), so `floodTerritoryAreas` floods only after a rule failure or a merge (18-22%
of calls in 1v1 games); lazy `isPinched`; the pinch pass skipped under conn4. Item 2: `hasUnitNearby(DefensePost)`
by handle + a global DefensePost count. Gates: full corpus 182/182, verify-memos corpus, a 96k-edit fuzz of the rules,
trajectory hashes 6/6. Env speedup 1.09-1.14x in 1v1, 1.06x World FFA. Not done: `calculateClusters` (the
8-connected border flood, now the largest removeClusters part).

## 12. Open questions, risks, and the gates that check this design

| # | Item | Status / owner |
|---|---|---|
| G1 | Clone ≤ 5 ms p95 pooled on World, with allocation, page-fault and per-component byte reports (§8.3) | [E]: `of-cli fork-bench`, P13 (ledger t091) |
| G2 | ≥ 5× TS ticks/s single core, World FFA, 72 Impossible nations + 400 tribes; allocations per tick reported | [E]: `of-cli bench` vs `docs/recon/06_benchmark.md` numbers (194–302 t/s) (ledger t091) |
| G3 | `WaterPathMemo` and the edge-segment cache are **state** (§3.8); ported literally, no feature flag. SPEC §1.7 does not list the segment cache. Rust conformance no longer depends on t074's memo-neutrality question; that question remains for the TS restore. | P4a/P4b; conformance scenarios in ledger t107; SPEC note in t107 |
| G4 | SPEC §1.3 says stats are output-only; `conquerPlayer` reads `attacks[SENT]` (`GameImpl.ts:1777-1781`) | Report to the conformance owner; CS needs no change (S is in D) (ledger t089) |
| G5 | `isInCombat` view side effect (§5), now including the `touch()` callers | Shim plus upstream-bump guard test (P2, P7, P8) (ledger t090) |
| G6 | JS typed-array OOB reads return `undefined` (no throw) and writes are ignored; Rust panics | PORTING_GUIDE §8: porters mark every possibly-OOB index. A missed one is a non-`TsThrow` panic → `Fatal::Bug`, never a silent path change (§4.6). |
| G7 | **`of-det` contributions** (t088): (a) `JsMap`/`JsSet` **`clear()` must keep live cursors running and visit later inserts**. The current `ordmap.rs:92-98` ends them, which is **wrong**: `node -e` with a Set iterated while `clear()`+`add(9)`+`add(10)` runs prints `1,9,10`, verified 2026-11-05. (b) a hasher parameter defaulting to an inline Fx hasher (today it is `std` SipHash with `RandomState`, `ordmap.rs:17,23,44`). (c) a hand-written `clone_from`, plus a `remap` buffer cleared at tick boundaries (`end_tick()`) and not carried by clones. (d) `SmallJsMap`/`SmallJsSet`. (e) `throw::{TsThrow, ts_throw!}` + `bigint::{from_floor_strict, from_number_strict}` + `util::to_int_strict` (i64, `TsThrow`). (f) `Prng` `prng-trace` feature. (g) `FlatBinaryHeap::clone_from`. | P1 contributes, conformance workflow reviews (ledger t088) |
| G8 | The engine-api `GameConfig` zod defaults must round-trip (SPEC §2.2 says configs are pre-parsed) | P1, verified by loading every corpus `scenario.json` |
| G9 | Upstream drift: the design is pinned to 5b75bd3. A bump follows SPEC §5.6, plus a re-run of the §5 guard tests and the FILE_MAP regeneration script. | — |
| G10 | Minimap terrain magnitude is edited by water nukes (`WaterManager.ts:582-590,881`) and read by `cost()` (§3.6.1). A shortcut port silently changes ship paths. | P4a; M-NUKE-WATER-01..03 at L3 (CS M.miniMap edit lists) (ledger t098) |
| G11 | Memo layout change (§3.3) relies on every `_units` mutation bumping `_myUnitsVersion` | `verify-memos` feature on the CI corpus (P2) |
| G12 | Water fork exactness (§3.8): fork, water-nuke in one child, query the same edge in both, compare segment results; a stale trade ship hits the old memo after a rebuild | Unit tests (P4a/P4b) + conformance scenarios: water nuke, a boat routed inside the stale-graph window, and a trade route repeated across a rebuild (ledger t107) |
| G13 | Shared nation PRNG makes P10–P12 failures hard to localise | `prng-trace` (§4.5) (ledger t108 for the TS probe side) |
| G14 | Stamp-wrap safety of every stamp scratch that moved from per-game (TS) to per-thread (Rust) | Step-start reset + forced-wrap unit test per scratch (P3 traversal, P4a water/HPA, P3 BFS) |

**Self-check for this document.**
- v1: 30 load-bearing claims were re-checked independently by a Grok reviewer (`CITATION_CHECK_GROK.md`): 24 true, 4 partly true, 1 false-by-design.
- v2: every TS claim the two reviews raised was re-read at the pin before it was adopted. The citations are in the text and in the review log. One claim was checked by execution (G7a, JS `clear()` semantics, run with node).
- **External baseline:** upstream's own state classification, the snapshot README rules and `DERIVED_FIELDS`/`OPAQUE_CLASSES` (`tests/util/Snapshot.ts:130-196`). This design agrees with them except in four places, each justified:
  - it adds the `isInCombat` shim
  - it keeps memos (cloned) instead of dropping them
  - it treats `WaterPathMemo` and the edge-segment cache as state, where upstream calls them OPAQUE: they are not in the snapshot, so a TS restore loses them, but a live run's results depend on them (§3.8)
  - Rust forks carry the full water-cache state that TS restores cannot.

---

## Review log

Each review point, with the decision and where it landed. "Accepted" means adopted as proposed; "Adapted" means adopted with a change, with the reason given; "Rejected" gives a one-line reason.

### Review 0 (fidelity)

| # | Point | Decision | Where |
|---|---|---|---|
| 0.1 | Arrays replaced by `filter` are still iterated in their old form; `retain` + live cursor skips elements | Accepted. Verified `PlayerImpl.ts:458-459,764-765,1900-1901`, `UnitImpl.ts:275,353`, `AttackImpl.ts:61-71`, `GameImpl.ts:469,507,1143-1144`, and the loops `PlayerExecution.ts:60-79,107-111`, `AttackExecution.ts:169-178`. Added `JsArr` + snapshot iteration, push-while-same-array handling, annotations and tests. Also found `units([])` returning the live array (`:470-472`). | §4.7, §3.3, PG §5.3/§6 |
| 0.2 | Stale ships route on the **new** graph; the old chain differs only in cached path, memo and kind | Accepted. Verified `WaterManager.ts:199-201`, `AStar.WaterHierarchical.ts:60-64`, `PathFinder.ts:47-66`, `AbstractGraph.ts:250-262,323`. Removed the `Arc<WaterGraph>` per stepper; added `chain_version` + `ChainSlots`; components are always read live. | §3.8 |
| 0.3 | `_pathCache` depends on query history; a write-once shared cache is wrong | Accepted (bitset + shared values + overlay). Verified `AbstractGraph.ts:105-136,320`, `AStar.WaterHierarchical.ts:242-273`. Added fork test G12. | §3.8, G12 |
| 0.4 | `WaterPathMemo` is state; "feature off" does not conform either | Accepted. Verified `PathFinder.ts:79-162,271-285`. Ported literally per chain slot, cloned; feature flag removed; t074 re-scoped (it is no longer a Rust conformance dependency). | §3.8, G3 |
| 0.5 | `NationAttackBehavior` overrides 7 virtual methods; the split ignored it | Accepted. Verified the 7 overrides and `super.` at `:1202,1218`; Tribe has none. Added `AiAttackHooks` frozen at S0 and the P9 → P10 dependency. | §4.4, FILE_MAP §2.1/§3 |
| 0.6 | `catch_unwind` in the stepper masks Rust-only panics | Accepted. `TsThrow` payload class; the stepper re-raises everything else. | §4.6, PG §11 |
| 0.7 | One thread-local `RefCell` for all scratch → re-entrancy panics TS lacks | Adapted: merged with review 1 #16. One plain field per TS scratch object, passed through `Ctx` (no `RefCell` at all). The TS re-entrancy guard becomes an explicit `busy` flag raising `ts_throw!` where TS throws. | §3.8, §3.6.1 |
| 0.8 | BigInt conversions mapped wrongly (clamp vs throw; `debug_assert!` vanishes in release) | Accepted. Verified `StatsImpl.ts:58-65,116-130,254-281`. Strict conversions in every build; strict stats checks in the lite build; floor-asymmetry test. | §7.2, §3.10, PG §4 |
| 0.9 | Shim list misses executions' `touch()` callers | Accepted. Verified `UnitImpl.ts:145-147` and the 5 call sites. | §5, PG §14.9 |
| 0.10 | `JsMap`/`JsSet` `clear()` must keep cursors running | Accepted. Verified by execution with node (prints `1,9,10`). The current of-det `ordmap.rs:92-98` contradicts this; filed under t088. | §7.3, G7a, PG §5.2 |
| 0.11 | Post-panic state differs from TS; GameRunner does not poison | Accepted with the TS-exact option: no vacating, `inited` applied after the loop, inbox leftovers moved to `uninit`, no poisoning on `TsThrow`. Verified `GameImpl.ts:614-627`, `GameRunner.ts:213-246`. | §4.2, §4.6 |
| 0.12 | Player Map order vs smallID order with a duplicate id | Accepted: `add_player` asserts the id is new; assumption documented. | §3.1 |
| 0.13 | Shared PRNG couples P10–P12 | Accepted: `prng-trace` on both sides (TS probe in t108). | §4.5, G13 |
| 0.14a | 50 live `.sort(` sites, not 51 | Accepted. Verified: the grep finds 51 lines, and `DefensePostExecution.ts:80` is commented out. | §7.3, PG §6/§14.5 |
| 0.14b | `TrainStation.stop_rng` is not state | Accepted. Verified `TrainStation.ts:59-68`; field dropped. | §3.9, §4.5 |
| 0.CF | "Checked and fine" items | Noted. Citations added where the text relies on them (§3.6.2, §3.7, §4.2, §5). | — |

### Review 1 (performance and clone)

| # | Point | Decision | Where |
|---|---|---|---|
| 1.1 | Shared segment cache breaks bit-exactness; proposed a per-fork `Vec<u32>` fill table | Adapted: same diagnosis as 0.3. Took review 0's bitset (2 × edges bits) plus overlay instead of a 40–400 KB per-fork table. Sharing is gated on `terrain_gen == built_at_mini_gen`, which also covers the memo-key flaw (the memo is now per-game state). | §3.8 |
| 1.2 | Thread-shared traversal stamp counter can wrap mid-pass | Accepted, generalised to every stamp scratch (G14). | §3.8 |
| 1.3 | `derive(Clone)` has no `clone_from`; `Exec` enum cannot forward; boxed variants allocate | Accepted: `StateClone` derive, per-kind exec arenas with a `Copy` live list, no boxes. | §8.1, §4.2 |
| 1.4 | Allocation count about 5× higher; add `SmallJsSet`/`SmallJsMap`; gate on allocations | Accepted. | §8.2, §8.3, G1 |
| 1.5 | Dead units, attacks and railroads double the clone; GC in v1 | Accepted: every 32 ticks + before `fork_into`, railroads and retired chain slots included. | §3.11 |
| 1.6 | Tombstone policy contradicts the size table | Accepted: compact at end of tick above 25 %. | §3.6.2 |
| 1.7 | Too many `Arc` refcount updates per fork | Accepted: `PlayerInfoIdx` into one `Arc<Vec<PlayerInfo>>`. Steppers hold a `chain_version` and no graph at all (0.2 made per-stepper graph refs unnecessary). | §3.3, §3.8, §8.1 |
| 1.8 | Pooling mandatory; benchmark pooled vs unpooled with page faults | Accepted: `clone_into` is the primary API. | §8.1, §10, G1 |
| 1.9 | Memo arrays add cloned allocations | Accepted with the `verify-memos` guard (G11). | §3.3 |
| 1.10 | Observation journal cloned into every fork | Accepted: caller-owned, passed through `Ctx`. | §3.1, §10 |
| 1.11 | Rail memory unaccounted for and allocation-heavy | Accepted, with one correction: both grid maps are iterated by the snapshot (`RailroadSpatialGrid.ts:86-98`), so they keep `JsMap` order with packed `u32` keys. They are not lookup-only. | §3.9, §8.3 |
| 1.12 | §11 omits the big-FFA hot paths | Accepted: levers 4–7 added. The "water memo turned off" concern is moot because the memo is now ported literally (on). | §11 |
| 1.13 | `OrdMap` uses SipHash with `RandomState`; `remap` should not be cloned | Accepted (G7b, G7c). An inline Fx hasher keeps of-det dependency-free. | G7 |
| 1.14 | The tick loop allocates; bookkeeping loops over 472 players | Accepted: double buffers, and bookkeeping compiled out without `stats-full`. | §4.2, §3.10 |
| 1.15 | `removeClusters` allocates per call | Accepted: flat cluster scratch. | §11 item 3 |
| 1.16 | Scratch through a thread-local `RefCell`; pass `Ctx` instead | Accepted (merged with 0.7). Decided now, before the skeleton. | §3.8, PG §1 |
| 1.17 | The attack hot path is already well served | Noted; no change. | §11 item 2 |
| 1.18 | Retaining many fork nodes needs COW chunks; hook all writes now | Accepted: write accessors frozen at S0 (t092), COW deferred behind the gate. | §8.4/§8.5 |
| 1.19 | Share pristine-map data per process | Accepted: `MapStatics` holds `graph0` and `components0`; the gen-0 segment values are shared across games. | §3.1, §3.8 |
| 1.20 | API gaps: `push_exec` semantics, typed intents, `Send`, `StepEvents`, fingerprint | Accepted: `push_exec` replaced by `step_with_execs` with defined semantics; typed `Intent`; `assert_impl_all!`; `StepEvents`; `fingerprint64`. | §10 |
| 1.T | Revised clone estimate table | Accepted with additions: the water memo and `seg_vis` rows (from 0.3/0.4). | §8.3 |

### S0 implementation notes (2026-11-05, tag `s0`)

| What | Decision | Where |
|---|---|---|
| `ExecCmd` shape | `BindWarhead { k, pending }`, `CancelNuke(WarheadRef)`, `Unpin(WarheadRef)`; the issuer is the execution whose call just returned (`ExecStore.running`). | §4.3, `exec/mod.rs` |
| Pins | Hold counts (listing + 1 per listing MIRV); freeing a MIRV releases its warheads. | §4.3 |
| Post-throw | Inbox leftovers drained into `uninit`, then the thrower's queued commands applied (not cleared). | §4.6, `runner/mod.rs` |
| `Fatal` | Adds `Setup` (TS throw in `createGameRunner`/`init`); `Stuck` removed (a stuck game returns its original error). | §4.6 |
| `RefVisitor` | `#[derive(StateClone)]` also derives `Refs` (field-order handle walk; `#[refs(skip/gc_only/chain_version/manual)]`), so no type can miss a handle; arenas report nothing, exec slots are all GC roots. | §3.11, §9, `canon/refs.rs` |
| `add_player` | Takes `team: Option<Team>` (TS `addPlayer(playerInfo, team = null)`). | `state/game.rs` |

### S0 gate review (2026-11-05, tag `s0` moved)

An adversarial pass over the skeleton before S1. The checks were mechanical scripts plus a line-by-line read of the loop.

| Check | Result | Change |
|---|---|---|
| Struct fields vs `snapshot_schemas_5b75bd3.txt`, **recursively** (nested objects such as `Unit.nukeState`, `MIRV.launch.pathFinder.curve`, `Player.targets`) | 0 missing, 0 misordered. Representation differences are documented at the type: map-keyed lists (`SAMLauncher.targeting`, `DoomsdayClock.rotState`), `BoundingBox{min,max}` and `trajectory`. | Four fields were not marked `non-snapshot` and now are: `RailSpatialGrid.cell_size`, `Components.width`/`height`, `DistanceBasedBezierCurve.increment`. |
| TS interface members (`Game.ts`, `GameMap.ts`, `RailNetwork.ts`, `Stats.ts`, `pathfinding/types.ts`, `Config`/`EngineConfig`) | 541 members. Every one has a `fn` or a documented drop (view/takeover), except `GameMap.forEachNeighbor`. | Added `GameMap::for_each_neighbor` (N, S, W, E, the `neighbors4` order). |
| Every `XExec::new` in FILE_MAP §2.1 and the TS constructors | All 44 present, with TS parameter order. | none |
| Tick loop vs `GameImpl.ts:599-665` and `GameRunner.ts:213-246` | Matches. The `toUpdate`, `hash()` and `placeName` view paths were read and have no state writes. | none |
| FILE_MAP §5 (one owner per file) | 14 files had no owner marker. Two §1 modules named files that do not exist (`state::ids`, `runner::tick`). | Markers added. §1 rows 5 and 13 corrected. New guard `tests/file_owners.rs`. |

No review point was rejected outright. Two were adapted (1.1 to review 0's smaller structure; 0.7 merged into review 1 #16's `Ctx` design), and one gained a correction (1.11, the snapshot iterates both grid maps).
