# Performance record (measured 2026-11-05, upstream pin 5b75bd3)

These numbers were measured on the engine at the earlier upstream pin 5b75bd3 (OpenFront.io before v0.36.0). They have NOT been re-measured on the v0.36.0 port in this repository; the v0.36.0 delta touches four engine files (water BFS cap, nation send accounting, island-nation boats, weighted trade-port pick), so large changes are not expected, but treat this as a record, not a current claim. The TypeScript reference runs and the sharded gate scripts are not part of this repository.

Workload: full World FFA Impossible game `long/S-FFA-WORLD-IMP-p1ffa0001` (72 nations, 400 tribes,
empty turns, 14,131 ticks to the engine's WinCheck), `engine-rs bench` (no digests, no canon,
default features), release (`lto=fat`, `codegen-units=1`), one NVIDIA GH200 (aarch64, Neoverse-V2),
(one pinned core, quiet: involuntary context switches 26-51 per run). TS reference: the same seed
with `p1/src/ffa.ts --slot none --no-instrument --hash-every 0 --canon-every 0` on
the same core: **90.55 s sim wall = 156 ticks/s** (finalStrong `f7843c377f9d5611`; an earlier run
on a second core: 91.75 s = 154 t/s).

| Commit (cumulative) | Change | loop s (2 reps) | ticks/s | vs TS |
|---|---|---|---|---|
| `e30e9d3` merge1 | baseline | 31.50 / 31.35 | 450 | 2.88x |
| `a67f641` | fill TS `unitCount` memos (game + player) at SAM/port/train-station reads | 28.03 / 28.14 | 503 | 3.23x |
| `28feb15` | `closestTwoTiles`: stable key sort == TimSort for its consistent comparator | 26.70 / 26.64 | 530 | 3.40x |
| `451d8c5` | ShoreCoercing `waterToOriginal` as an op log (was quadratic) | 25.35 / 25.26 | 558 | 3.58x |
| `5fa71a3` | MinHeap hole walks (same positions as TS swaps) | 24.98 / 24.83 | 567 | 3.64x |
| `5844f04` | fill TS `nearbyMemo` from nation/tribe AI | 23.65 / 23.65 | 598 | 3.83x |
| `aaa3e26` | fill TS `unitsByTypeMemo` for single-type `units(type)` | 22.86 / 22.82 | 619 | 3.97x |
| `422c13b` | `floodComponent` as a scanline fill (order-free outputs) | 21.98 / 21.97 | 643 | 4.12x |
| `673a162` | `calculateBoundingBox` integer fold | 21.66 / 21.69 | 652 | 4.18x |
| `b4ab358` | MinHeap (priority, node) slots (layout only) | 21.71 / 21.72 | 651 | 4.17x (neutral) |
| `3d439b2` | fill TS `myUnitsMemo` for single-type `player.units(type)` | 21.51 / 21.61 | 655 | 4.20x |
| `579443c` | no f64 `fmod` in `closestTwoTiles`; ToInt32 fast path [2^31, 2^32) | 21.23 / 21.31 | 664 | 4.26x |
| `396d8f0` | `checkAndAssignTerritory` flood as an 8-connected scanline | 20.42 / 20.53 | 690 | 4.42x |
| `6d587d1` | bounded water A*: coordinates from the box-local index | 20.51 / 20.76 | 685 | 4.39x (neutral) |
| `5b4a516` | exact `State::shore_memo` for `closestShoreByWater` | 18.87 / 18.96 | **747** | **4.79x** |

The series ran while 2 conformance shards used cores 40 and 42; quiet A/B runs of the final binary
gave 18.32-18.71 s (755-771 t/s, 4.84-4.94x). Gate (>= 3x TS) met; the 5x aim is within 2-4%
(mimalloc measured +2.5%).

Fork (`engine-rs fork-bench --at 2000,8000,14000 --reps 50`, one pinned core), pooled `clone_into` p50/p95:
base 0.91/0.96, 1.71/1.80, 1.80/1.92 ms; final 0.88/0.94, 1.63/1.77, 1.77/1.89 ms (gate <= 3 ms
met at every point; pooled allocations p95 160/936/580). State clone bytes 14.7 / 19.7 / 18.2 MB
(final; the shore memo adds <= 0.3 MB). Peak RSS of a full `bench` game: 106 MB (TS: 491 MB).

Profile (the aarch64 host `perf` is blocked by `perf_event_paranoid=4`; `engine-rs bench` with feature
`sampler` and `OF_SAMPLE_OUT=<file>` runs an unprivileged SIGPROF frame-pointer sampler,
symbolised by a symbolizer script). Shares
at the start: nation `maybeAttack` 32% (boat-route planning 26%: `closestShoreByWater` 16%,
`closestTwoTiles` sort 6%), `removeClusters` 12%, `unitCount` walks 10%, AttackExecution 8%,
TrainStation 6%, TradeShip 5%, Warship 5%. At the end: `maybeAttack` 34% (water A* heap and
bounded search ~16% self), AttackExecution 11%, `removeClusters` ~11%, nation structures 13%.

Every change was gated on the sharded conformance corpus (see [CONFORMANCE.md](CONFORMANCE.md)); memo changes were additionally run with the `verify-memos` feature (every memo hit is recomputed and asserted). Exactness arguments are in each commit message and doc comment; order-free rewrites have differential tests (`stable_key_sort_equals_timsort`, `hole_heap_equals_swap_heap`, `scanline_flood_equals_dfs_flood`, `flood8_scan_equals_dfs`).


# v0.36.0 speed-up (measured 2026-11-13)

Five engine-only optimisations on the v0.36.0 port, each bit-exact: the same tests on the same operands, or a result that is proved
equal before it is used. They do not change any observable (the 222-trace corpus, which compares a digest of the key state every tick, passes
222/222 before and after).

| id | change | where |
|---|---|---|
| D1+A1 | `compute_nearby` / `shares_border_with` scan loops tell the player's own tiles apart first; the two tile arrays are hoisted into a read-only `GameMap::view()` (no per-tile enum dispatch) | `state/player.rs`, `map/game_map.rs` |
| D2 | `shares_border_with` remembers a witness border tile per pair of players in `Scratch`; a hint is re-validated on every use and only ever answers `true`, `false` always comes from the scan | `scratch/hints.rs`, `Ctx::shares_border` |
| C4 | the end-of-tick tile-list compaction runs every 8th tick (positions, tombstones and capacities are not part of any state) | `state/game.rs` |
| R1 | `removeClusters` pinch pass: the `isHoleCluster` test is deferred until a read-only dry run says the cluster would be captured, then replayed in index order on the unchanged map | `exec/player.rs` |
| R2 | component labels (and the 4-connectivity flag) survive the loss of a bridge tile: a bounded two-sided search proves reconnection or relabels the closed pocket, falling back to the flood when the budget is hit | `map/conn4.rs`, `state/game.rs` |

Feature `verify-opt` (test builds only) re-runs the original sequence beside R1's and asserts equal; `mut-r1` makes the dry run deny every
capture, to prove the corpus can fail (it does: 2 of 19 sampled traces diverge with it, 19 of 19 pass with `verify-opt`). Two more tries (a tribe early-exit scan and an integer remainder in the attack-phase
test) measured no gain and were not kept.

Measurement: 2026-11-13, NVIDIA GH200 (aarch64, Neoverse-V2), one pinned core (`taskset -c 47`), release (`lto=fat`, `codegen-units=1`), default
features, `engine-rs bench`, base (the first commit) and new binaries alternating. The box was shared with other jobs on other cores, so absolute
ticks/s are lower than the quiet 2026-11-05 record above (the World game ran at 492 to 551 ticks/s here against 747 then, on an earlier pin);
only the paired comparison is meaningful.

| scenario | runs each | base median | new median | change | ticks/s base to new |
|---|---|---|---|---|---|
| `RS-SMOKE-01` Iceland Compact, 600 ticks | 9 | 0.1078 s | 0.1057 s | -2.0% | 5,566 to 5,676 |
| `RS-SMOKE-02` Australia Compact, 100 bots, 2,500 ticks | 9 | 0.8275 s | 0.7930 s | -4.2% | 3,021 to 3,153 |
| `S-FFA-WORLD-IMP-p1ffa0001` World FFA, 14,131 ticks | 5 | 27.21 s | 25.65 s | -5.7% | 519 to 551 |

Ranges (min to max) did not overlap on the two smoke scenarios (RS-SMOKE-02 base 0.823 to 0.837 s, new 0.778 to 0.803 s); on World the
ranges overlap slightly (base 26.6 to 28.4 s, new 25.2 to 28.7 s, one noisy new run), so treat -5.7% as indicative.
In our own nation-heavy self-play games (not part of this repository) the same changes measured about x1.3 on Normal-size 1v1 games and x1.07 on Compact ones; those were not re-measured here.

# Speed summary and cross-machine figures

Headline, measured 2026-11-05 on the earlier pin 5b75bd3 with this engine (the exported code is a
descendant of it), World map, free-for-all with 72 Impossible nations and 400 tribes, 14,131 ticks,
**one aarch64 core (NVIDIA GH200, Neoverse-V2), TS and Rust on the same core**, no digests:
**747 ticks/s for the Rust engine against 156 ticks/s for the TS engine (4.79x)**; peak RSS 106 MB
against 491 MB. Method, the step-by-step optimisation table (every step bit-exact, gated on the
corpus) and the profile are in this file. That number was **not
re-measured on v0.36.0**, and the TS reference harness is not part of this repository. Re-run the
Rust side with `engine-rs bench`.

**Speed-up on v0.36.0 (2026-11-13).** The second commit of this repository adds five bit-exact engine optimisations
(listed in this file, section "v0.36.0 speed-up"). Same machine, same pinned core (NVIDIA GH200,
Neoverse-V2, one core), default features, base and new binaries run alternately, median wall time per run:
Australia smoke `RS-SMOKE-02` (100 bots, 2,500 ticks) 0.828 s to 0.793 s (**-4.2%**, 3,020 to 3,153 ticks/s, 9 runs each);
`RS-SMOKE-01` (Iceland, 600 ticks) 0.108 s to 0.106 s (-2%); World FFA to the win (72 nations, 400 tribes, 14,131 ticks,
`S-FFA-WORLD-IMP-p1ffa0001`, not shipped here) 27.2 s to 25.6 s (**-5.7%**, 519 to 551 ticks/s, 5 runs each). The games are
identical: the 222-trace corpus still passes 222/222. The gain is modest on a pure upstream game; in our own
nation-heavy self-play it measured larger (about x1.3), because those games spend more of their time in the scans that the
optimisations shortcut. The figures in the table below were taken before this commit.

Other figures we have, with their sources. **They come from different machines, maps and
configurations, so cross-row comparisons are indicative only**; only the rows marked "this repo"
were measured on the code exported here.

| what | figure | hardware | source and caveats |
|---|---|---|---|
| Rust, World FFA (72 nations, 400 tribes) | 747 ticks/s | GH200 aarch64, 1 core | this file; earlier pin 5b75bd3; same-core TS run: 156 ticks/s |
| TS, ranked-1v1 config (Australia, 400 tribes, no nations, 3k ticks) | ~195 ticks/s (5.2 ms/tick) | Intel Core Ultra 7 255H laptop, one P-core, solo | our recon of the upstream TS engine (Node 24, headless); laptop numbers drop 2 to 3.5x when all cores are busy |
| TS, World FFA to the win, 472 players | ~302 ticks/s | same laptop P-core, solo | same recon; a different config from the 156 ticks/s row (that one is nation-heavy and ran on the GH200) |
| Rust, Australia, 100 tribes/bots, 2,500 ticks | ~1,235 ticks/s, p95 step 1.9 ms | Intel Core Ultra 7 255H, one core, release | this repo, v0.36.0: `engine-rs bench --scenario tests/fixtures/RS-SMOKE-02`; no TS number for this scenario on this machine |
| Forking a game: TS snapshot write + restore, World, mid-game | ~0.43 s (about 160 ms write + 270 ms restore, warm) | same laptop P-core | TS `snapshotGame`/`restoreGame`, measured by us (World tick 18,000) |
| Forking a game: Rust pooled `clone_into`, World FFA | about 1 to 2 ms (p50 0.9 to 1.8 ms at ticks 2k to 14k) | GH200 aarch64, 1 core | this file; a fork is a state clone, not a serialise/parse round trip |
| Forking a game: Rust pooled state fork, 1v1 Australia (Compact) | ~0.2 ms | GH200 aarch64, 1 core | an internal research harness around this engine, not reproducible from this repo |
| Rust, 1v1 mix (Australia/Normal and Compact, opening to late game) | ~2,590 ticks/s per core (386 us/tick) | GH200 aarch64, 1 core | the same internal harness, with extra per-tick work around the engine; an indicative figure only |

Reading the fork rows: the TS figure is the cost of serialising and restoring a World game, the Rust
figures are in-memory state clones, and the machines differ, so treat the gap as indicative. The only
like-for-like same-map pair is World FFA: about 0.43 s (TS, laptop) against 1 to 2 ms (Rust, GH200), roughly
200 to 400x. The 1v1 Rust row has no TS counterpart measured on the same map.

## Build features and build notes

Maps are read from `$OF_UPSTREAM_DIR/resources/maps/<name>/` and default to the bundled `assets/`.
Built and tested with Rust stable 1.96.0 (`rust-toolchain.toml` pins 1.96.0; the code uses edition 2024 let-chains, so a recent compiler is needed). Cargo features: `stats-full` (full
`StatsImpl`, needed for the S/D digests; off for throughput builds), `verify-memos` (recompute and
assert every exact memo), `prng-trace`, `prof`, `sampler`.
