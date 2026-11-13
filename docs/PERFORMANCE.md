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

Every change was gated on the sharded conformance corpus (see README, Conformance); memo changes were additionally run with the `verify-memos` feature (every memo hit is recomputed and asserted). Exactness arguments are in each commit message and doc comment; order-free rewrites have differential tests (`stable_key_sort_equals_timsort`, `hole_heap_equals_swap_heap`, `scanline_flood_equals_dfs_flood`, `flood8_scan_equals_dfs`).
