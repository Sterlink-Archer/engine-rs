# of-engine porting guide: mechanical rules for every porting agent

Engine pin: `upstream/` @ `5b75bd3`. Design v2 (2026-11-05).

Read [`ARCHITECTURE.md`](ARCHITECTURE.md) first. The sections you need are:
- §3: the state model and handles
- §3.8: water and scratch
- §4: the execution model, especially §4.4 (behaviours and `AiAttackHooks`), §4.6 (`TsThrow`) and §4.7 (filter-reassigned arrays)
- §5: dropped view work and the `unit_touched` shim.

This file states **how** to translate TS into Rust so that the result is bit-identical. Each rule names its TS evidence and its check (a vector suite, a module test or a conformance scenario). Paths are relative to `upstream/packages/`.

**The prime directive: transliterate, do not re-design.** Keep:
- statement order and evaluation order
- loop structure and early returns
- the exact floating-point expression shape. `(a*b)/c` is not `a*(b/c)`, and `x/4` is not `x*0.25` unless 1/4 is exact *and* TS wrote it that way. Keep TS's parenthesisation literally.
- which collection is iterated, and in what order. **That includes which array object**: see §5.3 on `filter` reassignment.

Optimise only after L3 is green, and only under ARCHITECTURE §11's rule.

---

## 0. Workflow per TS file

1. **Read the whole TS file.** Then read its record in `conformance/schema/snapshot_schemas_5b75bd3.txt`. That record is the list of state fields, and the skeleton struct already has exactly those fields in that order (FILE_MAP §3, stage S0).
2. **Port each method in place**, in the module FILE_MAP assigns. Keep TS method names in `snake_case` (`buildUnit` → `build_unit`), and keep TS-private helpers private.
3. **Implement `Canon` (export and import) and `RefVisitor`** for each type you own (ARCHITECTURE §9). `RefVisitor` must report every handle the type holds, because the v1 GC frees anything it does not see (ARCHITECTURE §3.11).
4. **Write unit tests.** Port the matching upstream vitest file under `upstream/tests/` when one exists; it is the cheapest oracle. Then run the vector suites that cover your functions (VECTORS §3) and the micro scenarios listed for your package (FILE_MAP §4).
5. **Never edit another package's files.** If you need a signature changed, file it on the ledger for the skeleton owner (P1). Do not work around it locally.

---

## 1. Ownership and borrowing patterns (the only allowed shapes)

`cx: &mut Ctx` is the `tick`/`init` argument. It is `{ st: &mut State, sc: &mut Scratch, jr: Option<&mut TileJournal> }` (ARCHITECTURE §3.8). `rcx: &mut Rcx` is the read-only-state variant, `{ st: &State, sc: &mut Scratch }`.

| TS shape | Rust shape |
|---|---|
| `this.mg.foo()` inside an execution | `cx.st.foo()` if `foo` needs no scratch, `cx.foo()` if it does (the frozen S0 signature tells you which). An execution never stores `&State`. |
| `player.method()` where `player: Player` | `cx.st.player(p).method()` (read) or `cx.player_mut(p).method()` (write), where `p: SmallId`. **Never hold a `&mut Player` across a call that takes `cx`/`st`.** Re-borrow per statement. |
| Two players mutated in one expression (`attacker.addTroops(x); defender.removeTroops(y)`) | Two statements, in TS order, each re-borrowing. Use `st.players.get_two_mut(a, b)` only when a single TS statement needs both. |
| `unit.method()` | `cx.st.unit(u)` / `cx.unit_mut(u)`, where `u: UnitRef`. Same rule. |
| Field initialiser `private random = new PseudoRandom(123)` | Initialise it in the Rust constructor (`AttackExec::new`), **not** in `init`. Construction time is when TS seeds it (ARCHITECTURE §4.5). |
| `mg.addExecution(new XExecution(..))` | `cx.st.add_execution(Exec::X(XExec::new(..)))`. It goes to `st.exec_inbox`; the driver moves it to an arena and appends to `st.uninit` right after your `tick`/`init` returns. The call returns `PendingExec` (only MIRV uses it, ARCHITECTURE §4.3). |
| A behaviour object calling a sibling behaviour (`this.allianceBehavior.f()`) | `self.alliance_f(cx)` on the owning `NationExec`/`TribeExec` (ARCHITECTURE §4.4). The shared `this.random` is `self.rng`. |
| `this.foo()` inside `AiAttackBehavior` where `foo` is one of the 7 hooks | `h.foo(cx, ..)` on `h: &mut H where H: AiAttackHooks` (ARCHITECTURE §4.4). **Never call the base free function directly from base code**, or the nation override is skipped. `super.foo()` in `NationAttackBehavior` → `base_foo(self, cx, ..)`. |
| Callback with a captured `this` (`map.forEachNeighbor(t, n => this.x(n))`) | Fill a neighbour buffer (`map.neighbors4(t, &mut buf)` returns its count), then loop over it. **Check that the TS callback cannot mutate the source the iterator walks.** If it can, apply §5.3. |
| Predicate parameter (`UnitPredicate`, filter callbacks) | `impl FnMut(&State, UnitRef) -> bool`; no boxing |
| Getter that is a computed property (`get size()`) | `fn size(&self)` |
| `x!` (non-null assertion) / any TS `TypeError` site | `.ts_expect("<TS file:line>")`: a `TsThrow` panic, matching TS (§11). **Not** `.expect()`/`.unwrap()`, which are Rust-bug panics. |
| Scratch use (`getTraversalScratch(game)`, `map.bfs…`, an A* instance) | the matching `cx.sc.<field>` (one field per TS scratch object, ARCHITECTURE §3.8). Never a thread-local. |

---

## 2. Number semantics

| TS | Rust | Notes |
|---|---|---|
| `number` (general) | `f64` | Default. Every config formula, troops on units and attacks, ratios, distances. |
| `number` that is provably an integer: `TileRef`, ids, ticks, counts, smallIDs, array indexes | `u32` / `i32` / `i64` / `usize` | Allowed only under ARCHITECTURE §7.1's four conditions. Write `// int: <reason>` next to the declaration the first time. |
| `a / b` | `a / b` in f64. For integer operands: `(a as f64) / (b as f64)`, unless the TS result is immediately floored or `|0`-ed. In that case integer division is allowed **only for non-negative operands**: Rust `/` truncates and `Math.floor` floors, so they differ for negatives. | |
| `a % b` | f64: Rust `%` (fmod, sign of the dividend; matches JS). Ints: Rust `%` (truncated; matches JS sign). | VECTORS `jssem/mod_div` |
| `x | 0`, `~~x` | `of_det::js::to_int32(x)` → `i32` | Wraps modulo 2^32 and truncates toward 0; NaN/±Inf → 0. **Never `x as i32`** (it saturates). |
| `x >>> 0` | `of_det::js::to_uint32(x)` → `u32` | |
| `a << n`, `a >> n`, `a >>> n` | `of_det::js::{shl, sar, shr}(a, n)` | The shift count is masked by 31 (VECTORS `jssem/shifts`) |
| `a & b`, `a ^ b`, `a | b` | on `to_int32` values | The result is int32 (signed) |
| `Math.imul(a, b)` | `of_det::js::imul(a, b)` / `imul_f64` | |
| integer `+`/`-` that TS wraps with `| 0` | `wrapping_add`/`wrapping_sub` on `i32` | e.g. `PseudoRandom`, `GameMapImpl.ts:571-577` |
| `Number.isInteger(x)` | `x.is_finite() && x.trunc() == x` | `GameMapImpl.isValidCoord` (`:102-111`) |
| `NaN` / `Infinity` | `f64::NAN` / `f64::INFINITY` | Comparisons with NaN are false in both languages. **But see §3 for `Math.max`/`min`.** |
| `-0` | Preserve it. Rust f64 keeps -0. | CS encodes -0 as F64 (SPEC §1.4); `Math.round` keeps -0 (§3) |
| `x ** 2` | `x * x` | All 22 uses are `x ** 2` (doc 08 §0) |
| `Number(bigint)` | `b as f64` | Exact below 2^53 |
| integer-valued `f64` used as an index | `x as usize` **after** `debug_assert!(x >= 0.0 && x.fract() == 0.0)` | JS would read `arr[1.5]` as `undefined`. If TS can produce a fraction there, port the `undefined` behaviour (§8). The `debug_assert!` is a Rust-bug check, never TS semantics. |

**Float expression shape.** Rust evaluates `a + b + c` as `(a + b) + c`, as JS does. There is no reassociation and no FMA unless you write `mul_add`, which clippy bans. Keep every literal exactly as TS spells it: `0.1` is the same f64 in both languages, but `1/3` written as `0.3333` is not.

---

## 3. `Math.*` and DetMath → `of-det`

| TS | Rust | Why |
|---|---|---|
| `Math.floor/ceil/trunc/abs/sqrt` | `of_det::js::{floor, ceil, trunc, abs, sqrt}` (thin wrappers over `f64`) | IEEE-exact (VECTORS `jssem/Math.floor_ceil_trunc`, `sign_abs_sqrt`) |
| **`Math.round(x)`** | **`of_det::js::round(x)`** | JS rounds half toward +∞ and returns -0 for -0.5 ≤ x < 0; `f64::round` rounds half away from 0 (Q-ROUND, VECTORS `jssem/Math.round`, 8,664 cases) |
| **`Math.max(a, b, ..)` / `Math.min`** | **`of_det::js::max(a, b)` / `min`** (variadic: fold in TS argument order) | JS returns NaN if any argument is NaN, and `max(-0, +0) = +0`. Rust `f64::max` ignores NaN and is unspecified on ±0. **205 call sites; no exceptions.** |
| `Math.max(...arr)` / `Math.min(...arr)` | `of_det::js::max_n(arr)` / `min_n` | The empty case gives -∞/+∞. Sites: `nation/NationUtils.ts:123-124`, `nation/NationNukePlanning.ts:371` |
| `Math.sign(x)` | `of_det::js::sign(x)` | keeps ±0 and NaN |
| `Math.fround(x)`, a `Float32Array` store | `x as f32` (read back as `f as f64`) | round-to-nearest-even (VECTORS `jssem/Math.fround`) |
| `exp/log/pow/pow2/atan2` from `DetMath.ts` | `of_det::detmath::{exp, log, pow, pow2, atan2}` | VECTORS `detmath` (191,378 cases) |
| `sigmoid` (`engine-lib/src/Util.ts:254-260`) | `of_det::util::sigmoid` | VECTORS `util/sigmoid` |
| `Math.random`, `Date.now`, `performance.now` | **Forbidden.** `performance.now` appears only in timing logs; drop the log. | doc 01 §7 (0 `Math.random` hits [V]) |
| `std` `powf/powi/exp/ln/log2/sin/cos/atan2/hypot/cbrt/mul_add` | **Forbidden** (clippy `disallowed-methods`) | ARCHITECTURE §7.4 |

---

## 4. BigInt (troops, gold, health, stats counters) → `i64`

The storage type is `i64`. Conversions go through the strict `of-det` wrappers, which raise a `TsThrow` exactly where TS throws, **in every build** (ARCHITECTURE §7.2). `of-det`'s own BigInt core is checked `i128`; you call it only through these wrappers.

| TS | Rust | Notes |
|---|---|---|
| type `bigint` / `Gold` | `i64` | Bounds in ARCHITECTURE §7.2 |
| `123n` | `123i64` | |
| `toInt(x)`: `BigInt(Math.floor(x))`, with ±Inf → ±(2^53−1) | `of_det::util::to_int_strict(x) -> i64` | **Only** for calls to `toInt` itself (`engine-lib/src/Util.ts:232-240`). NaN → `TsThrow` (RangeError). VECTORS `util/toInt` |
| `BigInt(Math.floor(x))` written out (no `toInt`) | `of_det::bigint::from_floor_strict(x) -> i64` | **Does not clamp.** NaN/±Inf → `TsThrow`. Sites: `EngineConfig.ts:281, 464, 684`; `DonateGoldExecution.ts:123`. (`NationStructureBehavior.ts:881` is `BigInt(Math.ceil(..))`: `from_number_strict(js::ceil(x))`.) |
| `BigInt(n)` | `of_det::bigint::from_number_strict(n) -> i64` | NaN/±Inf/non-integer → `TsThrow`. **Never `n as i64`**, not even after a `debug_assert!`: TS throws in every build. |
| `StatsImpl._bigint(v)` (`StatsImpl.ts:58-65`) | `from_floor_strict` | Runs only for players with a clientID (`:116-130`). In the lite build (`stats-full` off), the conversion **still runs** at every stats entry point and its result is discarded (ARCHITECTURE §3.10). |
| `minInt/maxInt/withinInt` | `min`/`max` on i64 (or `of_det::bigint::{min_int, max_int, within_int}` narrowed) | `Util.ts:242-252` |
| `a / b` (bigint) | `if b == 0 { ts_throw!("Division by zero") }; a / b` (i64, truncates toward 0) | `DonateGoldExecution.ts:58`, `EngineConfig.ts:434`, `NationNukeBehavior.ts:915,1104,1109` |
| `a % b` (bigint) | same guard, then `a % b` | sign of the dividend, same as JS |
| `Number(b)` | `b as f64` | |
| bigint comparisons `<`, `===` | i64 comparisons | |
| `x + -y` with `-y` of a **number** fed to `_bigint` | `from_floor_strict(-y)`: keep it literal | `floor(-y) ≠ -floor(y)` for fractions. `attackCancel` relies on this (`StatsImpl.ts:279`); it is unit-tested (ARCHITECTURE §3.10). |
| mixed bigint/number arithmetic | impossible in TS (TypeError); you will not see it | |
| `${gold}` in messages | dropped (view) | |

A result outside `i64` is a plain panic (`Fatal::Bug`). It is never wrapped and never a `TsThrow`.

---

## 5. `Map` / `Set` / object-as-map / filter-reassigned arrays

### 5.1 Container choice

| TS | Rust | Rule |
|---|---|---|
| `Map<K, V>` or `Set<K>` that is **ever iterated** (`for..of`, `.forEach`, `.keys()`, `.values()`, `.entries()`, spread `[...m]`, `Array.from(m)`, `randFromSet`, **or by the snapshot**) | `of_det::JsMap<K, V>` / `of_det::JsSet<K>` | Insertion order. `set` on an existing key keeps its position. `delete` + `set` moves it to the end. Iteration skips deleted entries, visits entries appended during iteration, and **keeps going after `clear()`** (§5.2). |
| …and it is per-entity and usually tiny (per-player relations/embargoes/quick-chats, station railroads, cluster sets, nation trackers) | `of_det::SmallJsMap<K, V, N>` / `SmallJsSet<K, N>` | Same semantics; inline up to `N`, spills to `JsMap` above it (ARCHITECTURE §8.2). The skeleton picks `N`. |
| `Map`/`Set` used **only** for `get/has/set/delete` (never iterated, never spread, never counted by order, not in the snapshot) | `FxHashMap`/`FxHashSet` through `jsorder::LookupMap` (a wrapper with no iteration API) | Leave a `// lookup-only: <grep evidence>` comment. If unsure, use `JsMap`. |
| keys are object references (`Map<Player, number>`, `Set<Unit>`, `Map<Unit, TrainStation>`) | key by handle: `SmallIdKey`, `UnitKey` (the TS `unit.id()`, **not** the slot), `StationId` | Object identity ↔ TS id is a bijection for live objects. For units, use the TS id, because GC slot reuse must not change map identity. **Exception (S0):** a `Set<Unit>` whose members are read back as units (`DefensePost/Warship.alreadySentShell`, `NationWarshipBehavior.tracked*`) holds `UnitRef`: TS keeps deleted units in it, and deleted units are not in `_unitMap`, so a `UnitKey` could not be resolved. A `UnitRef` carries its generation (slot reuse cannot alias) and the owner's `RefVisitor` marks it, so the GC never frees a member. |
| string keys that are a bijection with small ints (`` `${cx}:${cy}` `` in `RailroadSpatialGrid.ts:113`) | pack into `u32`; keep `JsMap` if iterated; render the TS string in `Canon` | |
| `Map<UnitType, ..>` / `Record<UnitType, ..>` over the 16 unit types | `[Option<V>; 16]` **plus** an insertion-order list when iterated | `numUnitsConstructed` is iterated with `Object.entries` (insertion order of non-numeric keys, Q-OBJKEYS; `PlayerImpl.ts:2034`) |
| small-key maps keyed by `SmallId` and iterated (relations, embargoes) | `SmallJsMap<SmallIdKey, V, N>` | Do not use a dense `Vec` indexed by smallID: it would iterate in smallID order, not insertion order |
| `WeakMap<Game|Player, ..>` (module caches) | see ARCHITECTURE §3.11 | Never a global map |
| plain object used as a dict (`{}` with dynamic keys) iterated by `Object.keys/entries` | `JsMap<String, V>` with **integer-like keys ordered first, ascending**, then the rest in insertion order (ES spec; `of_det::ordmap::object_keys_order`). Only one sim-relevant site (`numUnitsConstructed`, non-numeric keys), plus `teamGameSpawnAreas` at load (`TerrainMapLoader.ts:79`). | VECTORS `order/object_key_order` |
| a JS **array field** that TS deletes from by `this.x = this.x.filter(..)` | `JsArr<T>` (ARCHITECTURE §4.7) | Removal is `filter_reassign(pred)`, **never** a bare `retain` or `remove`. Known fields: `Player.units`, `incoming_attacks`, `outgoing_attacks`, `alliances`, `State.alliance_requests`. `MIRVExecution.stagedTargets` (`:237`) and `NationAttackBehavior.followedLandings` (`:130`) are filter-reassigned but never iterated across the reassignment: plain `Vec` + `retain`, annotated `// filter-reassign: not iterated across`. |

### 5.2 Required `JsMap`/`JsSet` API in `of-det` (ARCHITECTURE §12 G7, ledger t088)

`of-det` already has `ordmap::{OrdMap, OrdSet, Cursor}`, aliased as `JsMap`/`JsSet`: `new`, `len`, `get`, `get_mut`, `contains_key`/`contains`, `insert`, `remove`, `clear`, `cursor()` + `next_live(&mut Cursor)`, `iter`, `from_iter`.

Compaction happens inside `insert` when tombstones outnumber live entries. Cursors survive it through an epoch plus a `remap` table, so a cursor stays valid across any mutation.

**Changes P1 contributes to `of-det`, never as a local copy:**
1. **`clear()` must not end live cursors.** ECMAScript and V8 keep a live iterator running after `clear()`, and it visits entries added afterwards. Verified with node: a `Set` iterated while the body runs `clear(); add(9); add(10)` yields `1,9,10`. The current `ordmap.rs:92-98` ends cursors, which is a bug. Implement `clear` by tombstoning every entry, or by remapping old cursors to the new start; never by truncating while a cursor may be live. Add a vector case.
2. **A hasher type parameter**, defaulting to an inline Fx hasher (of-det has no dependencies). Today it is `std` `RandomState` SipHash (`ordmap.rs:17,23,44`). Lookups only, so determinism is unaffected.
3. **A hand-written `clone_from`** that reuses `entries`/`index` capacity. `remap` is not carried by clones, and an `end_tick()` clears it (cursors never outlive a tick).
4. **`SmallJsMap<K, V, N>` / `SmallJsSet<K, N>`** with identical semantics (including items 1 and 3).
5. `first_live()` for the `WaterPathMemo` LRU eviction (`paths.keys().next()`).

### 5.3 Iterating while the loop body mutates

TS often mutates a Map/Set/TileSet/array inside its own `for..of`. JS semantics:
- **Map/Set:** deleted entries not yet visited are skipped; entries added are visited; after `clear()` the iterator continues with whatever is added next.
- **Arrays with `for..of`:** a live index iterator over **that array object**. Appended elements are visited and splices shift. **If the field is reassigned (`this.x = this.x.filter(..)`), the running loop keeps walking the old object, unchanged.**
- **`forEach` on arrays:** the length is captured at the start, but holes created by later deletes are skipped.

Rust rules:
- **Map/Set/TileSet:** iterate with a cursor that re-reads the length every step.

  ```rust
  let mut c = set.cursor();
  while let Some(t) = set_ref(cx).next_live(&mut c) { /* may mutate cx */ }
  ```

  Never `for x in set.iter()` when the body takes `&mut State`; the borrow checker forbids it anyway.
- **A `JsArr` whose loop body can (transitively) delete from it** (ARCHITECTURE §4.7):

  ```rust
  let mut it = cx.st.player(p).units.iter_snapshot(&mut cx.sc.snap_pool);   // iter-snapshot: filter-reassign
  while let Some(u) = it.next(&cx.st.player(p).units) { /* may delete/capture units */ }
  it.finish(&mut cx.sc.snap_pool);
  ```

  `next` extends the snapshot with pushes for as long as no reassignment has happened (`gen == gen0`), because those pushes went to the same JS array object. A site where a push and then a reassignment can happen *within one step* needs a written proof or custom code: `// iter-snapshot: push-then-reassign <argument>`.
- **A `JsArr` whose loop body provably cannot delete from it:** iterate by index with a re-read length, annotated `// iter-live: <reason>`.
- **Never** collect to a `Vec` first **unless** TS also snapshots (`[...set]`, `Array.from`, `.slice()`, a `filter`/`map` result, `incomingAttacks()`, `units(type)`).

For each loop that mutates its own source, write `// iter-mut: <semantics>` (or `iter-snapshot`/`iter-live`), and add a unit test with an add and a delete during iteration.

**Known `iter-snapshot` sites with mandatory tests:**
- `PlayerExecution.ts:60-79` (P6): units of the player, which delete and capture structures
- `PlayerExecution.ts:107-111` (P6): alliances, which expire
- `AttackExecution.ts:169-178` (P6): outgoing attacks, which merge and delete.

Grep for more: `for (const .* of .*\.\(units()\|outgoingAttacks()\|alliances()\|_incomingAttacks\|allianceRequests\))`.

---

## 6. Arrays

| TS | Rust | Trap |
|---|---|---|
| **`arr.sort(cmp)`** | **`of_det::js_sort_by(&mut arr, \|a, b\| -> f64 { cmp })`** (the V8 TimSort port) | Use it at **all 50 live sites** (§14.5), consistent comparators included, so the comparator call order and inconsistent-comparator results (NaN keys, `Infinity - Infinity`) match V8 (Q-SORT, VECTORS `order/array_sort` with call traces). The comparator returns `f64`. Do not use Rust `sort_by`. |
| `arr.sort()` with no comparator | **None at the pin** [V grep]. It would be a string sort. | |
| `filter`, `map`, `flatMap`, `slice`, `concat`, `[...a, ...b]` producing a **new** value | a new `Vec`, same order | |
| **`this.field = this.field.filter(..)`** (field reassignment) | **`field.filter_reassign(pred)` on a `JsArr`** | **Not** "retain is fine". A `for..of` already running over the old array keeps visiting every old element; see §5.3 and ARCHITECTURE §4.7. |
| `forEach` | `for i in 0..len_at_start` | The length is captured once |
| `for (const x of arr)` | index loop re-reading `len()`, or `iter_snapshot` if the field can be reassigned during the loop | Visits elements pushed to the same array object during the loop |
| `find`, `findIndex`, `some`, `every`, `includes`, `indexOf` | Iterator equivalents. Comparisons use `===`. For f64, `NaN !== NaN`, so `includes(NaN)` is **true** (SameValueZero) but `indexOf(NaN) === -1`. | Handles compare by value |
| `reduce((a, b) => a + b, 0)` over floats | Left fold, **same order, same initial value** | Never `iter().sum()` on floats from a reordered source. A missing initial value starts with element 0. |
| `push(...xs)`, `unshift`, `splice(i, n, ..)`, `pop`, `shift` | `extend`, `insert(0, ..)`, `splice`, `pop`, `remove(0)`, or `VecDeque::pop_front` where the array is a queue (`TrainExecution.usedTiles`) | 5 `splice` sites [V] |
| `arr.length = 0` | `clear()` | |
| `arr[i]` out of range | **`undefined`, not a throw** | §8 |
| `new Array(n).fill(x)`, `Array.from({length: n}, f)` | `vec![x; n]`, `(0..n).map(f).collect()` | |
| `Array.from(set)` / `[...map.keys()]` | a snapshot `Vec` in container order | |
| typed arrays (`Uint8Array`, `Uint16Array`, `Int32Array`, `Uint32Array`, `Float32Array`) | `Vec<u8/u16/i32/u32/f32>` | **Stores wrap or round**: `u8arr[i] = 300` stores 44; `u16arr[i] = -1` stores 65535 (ToUint16, modular); `f32arr[i] = x` rounds; `i32arr[i] = 2**31` stores -2^31. Port with `to_int32(x) as u8/u16` (modular), never `as` on an f64. |

---

## 7. Closures and callbacks

- TS closures that capture mutable locals (e.g. `visit` in `PlayerImpl.computeNearby`, `:627-655`) → local `FnMut` closures, or inline loops when the closure needs `&mut State`. Keep the call order.
- **Callback-driven traversals** (`GameMap.bfsSearch(tile, filter, onVisit)`, `forEachNeighbor`, `forEachTile`) keep the same visit order, from the same queue discipline (FIFO with a `head` index, `GameMapImpl.ts:477-500`). `onVisit` returning `false` stops early. `bfsSearch` sets and clears its scratch's `busy` flag with a drop guard, mirroring TS's re-entrancy guard (ARCHITECTURE §3.6.1).
- **Comparator closures** must be pure. If TS's comparator reads mutable state, read it the same way.
- **Closure parameters across a frozen API** (`calculateAttackTroops(.., nonBotTroops: (reserve) => number, ..)`, `NationAttackBehavior.ts:1149-1152`) become a small `enum`/`struct` carrying the captured data plus an `eval(&self, cx, reserve) -> f64` method, so the hook trait stays object-safe and allocation-free.
- Do not box closures in state. Executions store data, not functions. A TS field holding a closure becomes an enum or a per-type `match`, plus the closure's captured data as fields. Example: `TrainStation.stopHandlers` (`TrainStation.ts:73`). Its factory ignores its PRNG argument (`:59-68`), so the handler carries no data at all.

---

## 8. `undefined`, `null`, optional chaining, truthiness

| TS | Rust |
|---|---|
| `T \| undefined`, `T \| null`, `x?: T` | `Option<T>` |
| a field where **absent ≠ null** matters for CS (the snapshot schema shows `?` vs `nullable`): `largestClusterBoundingBox`, `WarshipState.isInCombat` | `Tri<T> { Absent, Null, Some(T) }` or `Option<Option<T>>`. The schema dump is the authority (SPEC N7). |
| `a?.b` | `a.map(\|a\| a.b)` / `a.and_then(..)` |
| `a ?? b` | `a.unwrap_or(b)`. **Only `null`/`undefined` fall through.** |
| **`a \|\| b` on numbers or strings** | **Falsy = `0`, `-0`, `NaN`, `""`, `null`, `undefined`, `false`.** `troops \|\| 0` maps NaN to 0. Port it as `of_det::js::or_num(a, b)`. |
| `if (x)` where `x: number` | `x != 0.0 && !x.is_nan()`. For example `if (this.player.unitsConstructed(UnitType.Port))` (`NationExecution.ts:133`) means "≠ 0". |
| `if (arr)` / `if (obj)` | always true when non-null: `is_some()` |
| `arr[i]` with `i` out of range, or `map.get(k)` missing | `None`. Then follow the TS consequences: `undefined + 1 = NaN`, `undefined < 3 = false`, `undefined === x` is false. **Typed-array OOB reads give `undefined`; typed-array OOB writes are silently ignored.** Rust indexing would panic, and that panic is a `Fatal::Bug`, never a quiet `NOT_FOUND` (ARCHITECTURE §4.6). So wherever an index can be out of range in TS, use `.get(i)` and port the `undefined` semantics. Typical cases: neighbour arithmetic at map edges, `-1` sentinels, `clusters[startIdx]?.[0]`, `PlayerExecution.ts:604`. If you can prove the index is in range, write `// in-range: <reason>` and index directly. |
| `undefined.foo` / `null.foo` that TS *would* reach (a TypeError) | `.ts_expect("<file:line>")` → `TsThrow` |
| `delete obj.k` | `remove` |

---

## 9. Strings and ids

- **`simpleHash(str)`** (`engine-lib/src/Util.ts:105-113`) runs over **UTF-16 code units**: `of_det::util::simple_hash_u16(&[u16])`, with `simple_hash(&str)` as a convenience that encodes UTF-16. It returns `f64`, not `i32`: `Math.abs(-2^31) = 2^31` (VECTORS `util/simpleHash`, 3 such strings).
- **Strings that can carry lone surrogates** cannot live in a Rust `String`. Player names from usernames are the case here. Store them as `Box<[u16]>` (`JsString`) wherever they reach hashing, sorting or CS. PlayerIDs are base-36 ASCII (`random.nextID()`), so `String` is fine for them.
- **`nextID()`** gives `floor(next()·36^8).toString(36).padStart(8, "0")` (`PseudoRandom.ts:73-78`). Keep the numeric `u64` and render it with `of_det::js::{to_string_radix, pad_start_zero}` only where a string is needed. Equality on the number is equivalent, because `padStart` makes the representation unique.
- **String `<` / `>` and `localeCompare`:** no sim comparator uses `localeCompare` [V, 0 hits]. If a comparator compares strings with `<`, compare `encode_utf16()` sequences. UTF-8 byte order differs for astral characters versus U+E000–U+FFFF.
- **Template literals** feed only messages and logs, which are dropped. **Exception:** a template literal that builds a *key*, such as a `Map` key like `` `${a}:${b}` ``. Port it exactly, or replace it with a packed or tuple key that is a bijection, and render the string wherever CS needs it (§5.1).
- `PlayerID` strings in intents (`targetID`, `recipient`) resolve to `SmallId` via `st.player_by_id(&str)`. It raises `TsThrow` like TS `player(id)` (`GameImpl.ts:931-937`), or use `has_player` first where TS checks.

---

## 10. Classes, interfaces, inheritance

| TS | Rust |
|---|---|
| `interface Execution` + 44 classes | `ExecKind` + per-kind arenas + `trait ExecBehavior` (ARCHITECTURE §4.2). Each class becomes a `struct XExec` with the snapshot-schema fields. |
| `interface Player`/`Unit`/`Game` (`game/Game.ts`) | Concrete structs with no traits. The TS interfaces exist for the client view. Exception: `PlayerLike`/`UnitLike` (`engine-api/src/game/ReadViews.ts`) become traits so config formulas can be vector-tested (ARCHITECTURE §6). |
| `class EngineConfig extends Config` | one struct `EngineConfig`. `override` methods replace base methods; `super.unitInfo` → call the base logic inline (`EngineConfig.ts:338-346`). No base code calls an overridden method virtually; check this per method, and if one does, use the hooks pattern below. |
| **`abstract class AiAttackBehavior` + `TribeAttackBehavior`/`NationAttackBehavior`** | **`trait AiAttackHooks`** with the 7 overridable methods as defaulted trait methods. All other base methods are generic free functions over `H: AiAttackHooks + ?Sized`, which call hooks through `h.` exactly where TS writes `this.`. `super.x()` → `base_x(self, cx, ..)`. Implemented by `TribeExec` (no overrides) and `NationExec` (all 7). Frozen at S0 (ARCHITECTURE §4.4). |
| `Player \| TerraNullius` | `Owner` (= `SmallId`, 0 = TN). `x.isPlayer()` → `x != 0`; `x.smallID()` → `x`; `tn.id()` → `None` |
| `Player \| Team \| null` (winner) | `enum Winner { None, Player(SmallId), Team(Team) }` |
| `x instanceof Y` | `matches!`. Sim-relevant sites: `ConnectedComponents.ts:98,116,209` (u8 vs u16 typed array → `enum Ids { U8(..), U16(..) }`) and `engine-lib/src/Util.ts:138` (Set vs TileSet parameter → two functions). The `GameImpl.ts:809,889,893` sites are takeover-only (skipped). |
| string enums (`UnitType.City = "City"`) | `#[repr(u8)] enum` with `as_str()` returning the TS string, used for CS and for `Object.entries` ordering |
| `readonly`, `private` | Rust visibility, no semantics |
| `static` class fields that are constants | `const` / `static` |
| static class methods (`PathFinding.Water(game)`) | free functions |

---

## 11. Exceptions → `TsThrow` panics or `Result`

There are two panic classes (ARCHITECTURE §4.6). **Only `TsThrow` stands for a TS throw.**

| TS | Rust |
|---|---|
| `throw new Error(msg)` anywhere in tick-reachable code | `ts_throw!("{msg}")` with the same message text (it helps the fatal-message diff, SPEC §2.2). **Not** `panic!`. |
| implicit `TypeError`s (`undefined.foo`, `null!`, `x!` that can fail) | `.ts_expect("<file:line>")` |
| RangeError from BigInt (`BigInt(NaN)`, `/ 0n`) | the strict `of-det` wrappers (§4) |
| `assertNever(x)` | `unreachable!()` (a Rust bug if reached; TS types make it unreachable) |
| Rust-only checks: `debug_assert!`, arena generation checks, slice indexing you proved in range, overflow | ordinary panics → `Fatal::Bug`. **Never** convert them to a TS behaviour. |
| `PathFinderStepper.next`'s `try { finder.findPath } catch → NOT_FOUND` (`pathfinding/PathFinderStepper.ts:64-70`) | `catch_unwind(AssertUnwindSafe(..))`. If the payload `is::<TsThrow>()`, return `NotFound`; **otherwise `resume_unwind`**. This is the **only** `catch_unwind` inside the sim (clippy-checked, §12). Code inside the chain keeps throwing as TS does. |
| `GameRunner` try/catch around the tick (`GameRunner.ts:231-246`) | `Game::step` uses `catch_unwind`: a `TsThrow` → `Err(Fatal::Tick)`, and the game stays steppable with TS's post-throw state; a non-`TsThrow` → `Err(Fatal::Bug)`, and the game is stuck (ARCHITECTURE §4.6) |
| `createExec` throwing for unknown intents (`ExecutionManager.ts:147-148`) | `Err(Fatal::Intent)` before any mutation; the game is stuck |
| `console.warn` + `return` (soft failure, e.g. `AttackExecution.init` target not found, `:80-84`) | Drop the log. Keep the `return` and the state changes before it. |
| A re-entrancy guard (`GameMapImpl.ts:477-479`) | an explicit `busy` flag on that scratch field + `ts_throw!` with the TS message; the flag is reset by a drop guard (TS `finally`) |

**No panic point between paired writes.** When TS does something in one statement that Rust does in two writes (e.g. a TileSet add = dense push + `pos_*_set`), put nothing that can panic between them. That keeps post-throw state equal to TS at statement granularity.

`panic = "unwind"` in all profiles. Never `abort`.

---

## 12. Logging, debug and lint configuration

- `console.log/warn/error`, `DebugSpan.*` and `performance.now()` are **dropped**. **But evaluate any argument expression that has side effects.** None was found at the pin, and most are string templates over pure getters; check anyway.
- `clippy.toml` (P1, stage S0):
  - `disallowed-methods`:
    - `f64::{powf, powi, exp, exp2, ln, log, log2, log10, sin, cos, tan, atan, atan2, hypot, cbrt, mul_add, max, min, round}` (use `of_det::js::*`)
    - `std::collections::HashMap::{iter, keys, values, into_iter, drain}`, and the same for `HashSet`
    - `slice::sort_by`/`sort_unstable*`/`sort_by_key` on sim data (use `js_sort_by`)
    - `std::panic::catch_unwind` outside `path/stepper.rs` and `runner.rs`
    - `Option::expect`/`unwrap` in `exec/` and `state/` (use `ts_expect`, or `// rust-invariant:` + `expect` for true Rust invariants).
  - `disallowed-types`:
    - `std::rc::Rc`, `std::cell::RefCell`, `std::cell::Cell` in sim code (no exception; scratch goes through `Ctx`)
    - `std::collections::HashMap`/`HashSet` in `State`
    - `Box<dyn ..>` in `State`/`ExecStore`.
  - `#![deny(clippy::float_cmp_const)]` is **not** used: JS compares floats exactly, and so does the port.
- Every `// int:`, `// in-range:`, `// lookup-only:`, `// iter-mut:`, `// iter-snapshot:`, `// iter-live:` and `// rust-invariant:` annotation is a reviewable claim. The P13 reviewer greps them.

---

## 13. Allowed bit-exact optimisations during the port (and forbidden ones)

**Allowed** (they cannot change results):
- Replace a TS allocation with a reused buffer (a `Scratch` field, `SmallVec`).
- Replace `memo.list.slice()` copies with borrowed slices when the caller does not mutate the source during use.
- Rebuild all 16 per-type unit memo lists at once on a miss (ARCHITECTURE §3.3; guarded by `verify-memos`).
- Precompute `Prng::new(123.0)`'s post-warmup state as a `const` (`AttackExecution.ts:40`) and assert it in a test.
- Use `Vec::with_capacity`, and ignore TS capacities (`FlatBinaryHeap` default 1024, `:14`; capacity is not state, SPEC N3).
- Use `u32` tile refs and integer `%`/`/` for `x()`/`y()`.
- Use `PlayerTileOrder` instead of a per-player hash `TileSet` (ARCHITECTURE §3.6.2).
- Use a `VecDeque` for an array used only with `push`/`shift`.
- Pack bijective string keys into integers (§5.1).

**Forbidden until L3 is green and the ARCHITECTURE §11 gate passes:**
- reordering loops
- replacing a sort with a selection or partial sort
- caching a DetMath result across calls with a different key
- early exits TS does not have
- changing f64 to f32 or the reverse
- "simplifying" `a * (b / c)` and similar
- removing a memo, an LRU, a TTL cache or a version bump (the `WaterPathMemo` and the edge-segment cache are **state**, ARCHITECTURE §3.8)
- iterating a Map in a different order "because it doesn't matter"
- `retain` on a filter-reassigned array (§5.3).

---

## 14. Trap checklist: the exact TS call sites

Regenerate this list on every upstream bump. Run the commands from `upstream/packages`:

```sh
EX='snapshot/\|worker/\|WinnerReplay\|NameBoxCalculator\|DebugSpan\|Maps.gen\|PackedUnitUpdates\|MotionPlans\|GameUpdateUtils\|HumanStats\|Format.ts'
grep -rn 'Math.round(' --include=*.ts engine/src engine-lib/src engine-api/src | grep -v "$EX"
grep -rn 'Math.fround\|Float32Array' --include=*.ts engine/src engine-lib/src | grep -v "$EX"
grep -rn 'Math.imul' --include=*.ts engine/src engine-lib/src | grep -v "$EX"
grep -rn '| 0\b\||0)\||0;\|>>> \|>>>0\| >> \| << ' --include=*.ts engine/src engine-lib/src | grep -v "$EX"
grep -rn '\.sort(' --include=*.ts engine/src engine-lib/src | grep -v "$EX" | grep -v '^[^:]*:[0-9]*:\s*//'
grep -rn 'new Map\b\|new Map<\|new Map(\|new Set\b\|new Set<\|new Set(' --include=*.ts engine/src engine-lib/src | grep -v "$EX"
grep -rn '[0-9]n\b\|BigInt(\|: bigint\|toInt(\|Number(' --include=*.ts engine/src engine-lib/src engine-api/src | grep -v "$EX"
grep -rn 'Math\.\(max\|min\)(' --include=*.ts engine/src engine-lib/src | grep -v "$EX" | wc -l
grep -rn '\._[a-zA-Z]* = .*\._[a-zA-Z]*\.filter(\|this\.[a-zA-Z]* = this\.[a-zA-Z]*\.filter(' --include=*.ts engine/src engine-lib/src | grep -v "$EX"   # filter-reassign (misses multi-line forms such as AttackImpl.ts:61-71)
grep -rn '\.touch()\|\.toUpdate()' --include=*.ts engine/src | grep -v "$EX"                                                       # §5 shim sites
grep -rn 'override ' --include=*.ts engine/src | grep -v "$EX"                                                                      # virtual dispatch
```

### 14.1 `Math.round` → `of_det::js::round` (23 sites [V])

| Site | Owner pkg |
|---|---|
| `engine/src/execution/MIRVExecution.ts:510` (`actualMirvLength * 256`) | P8 |
| `engine/src/execution/DonateGoldExecution.ts:124` (chunk size) | P9 |
| `engine/src/execution/ShellExecution.ts:114` (damage) | P7 |
| `engine/src/pathfinding/transformers/MiniMapTransformer.ts:85,86` (minimap → map interpolation) | P4b |
| `engine/src/utilities/Line.ts:29-32, 39-42, 67, 117, 123-126, 144-147` (18 sites: Bezier control points and spacing) | P4b |

### 14.2 `Float32Array` / `Math.fround` (Q-F32HEAP)

| Site | Rule | Owner |
|---|---|---|
| `engine/src/execution/utils/FlatBinaryHeap.ts:10,15,101` (`pri`). Enqueue compares the **f64** `priority >= this.pri[parent]` (`:36-38`) against stored f32 values, then stores f32 (`:42`). Dequeue compares stored f32 to stored f32 (`:61-63`), with ties by position. | Use `of_det::heap::FlatBinaryHeap`, which already exists. VECTORS `order/FlatBinaryHeap` (120 scripts incl. NaN/±Inf; tie gap ledger t093) | P6 |
| `engine/src/pathfinding/algorithms/PriorityQueue.ts:11,16,29` (MinHeap/BucketQueue priorities); `priority \| 0` bucket index (`:126`) | same pattern; `to_int32` for the bucket. **No vector coverage yet** (VECTORS §5.1 gap → add to MOD-PATH-*) | P4a |
| `engine/src/pathfinding/algorithms/AStar.AbstractGraph.ts:15,32` (`gScore`) | f32 store / f64 compare exactly as written | P4a |

### 14.3 `Math.imul` and int32 hashing (8 sites [V])

| Site | Owner | Note |
|---|---|---|
| `engine-lib/src/PseudoRandom.ts:19,21` | of-det | splitmix32 |
| `engine-lib/src/game/TileSet.ts:50` | — | Internal hash only; not observable (ARCHITECTURE §3.6.2). Do not port. |
| `engine-lib/src/game/GameMapImpl.ts:568` (pristine terrain hash) | — | Used only for snapshot verification; `pristineHash` is dropped by SPEC N2. Do not port. |
| `engine-lib/src/game/DoomsdayClock.ts:257,269,271` (`rotSpeckleNoise`/`rotFrontNoise`) | of-det (`util::rot_*_noise`) | VECTORS `util/rotSpeckleNoise`, `rotFrontNoise` |
| `engine/src/execution/nation/TutorialSilo.ts:139` (`imul(ticks+i, 2654435761) >>> 0`) | P12 | |

### 14.4 `|0`, `>>>`, `<<`, `>>` (beyond the PRNG and TileSet)

| Sites | Rule |
|---|---|
| `y = (ref / width) \| 0`: `GameMapImpl.ts:85`; `AStar.Water.ts:72,80,99,123`; `AStar.WaterBounded.ts:63,72,113,123,133,148,171,198`; `AStar.Rail.ts:109,111`; `SmoothingWaterTransformer.ts:199,201,271,273,342,344`; `PlayerExecution.ts:916,965`; `PlayerImpl.ts:1756,1760` | Non-negative integer refs → `u32 /`. Exact. |
| `engine/src/execution/Util.ts:148`: `Math.abs(((currentX / w) \| 0) - ((currentY / w) \| 0))` (**looks like an upstream bug**: x/w; port it literally) | literal |
| `(i - 1) >> 1`, `(i << 1) + 1`, `len << 1`: `FlatBinaryHeap.ts:36,58,99`; `PriorityQueue.ts:43,66` | `usize` arithmetic (non-negative, small) |
| `(lo + hi) >> 1`: `SpatialQuery.ts:270`; `(lo + hi) >>> 1`: `SmoothingWaterTransformer.ts:80` | `usize` |
| `utilities/Line.ts:136-137,156-157,206-207` (`(p + 128) >> 8`), `:216-229` (midpoints `>> 1`) | **Signed** values: `of_det::js::sar(p + 128, 8)` (arithmetic shift on i32) |
| `RailNetworkImpl.ts:938,944` (`>>> 0` hash) | legacy hash only (on demand) |
| `DoomsdayClock.ts:240,259,269-273` | of-det, vectors |
| `ConnectedComponents.ts:263-271` (byte-parallel land marking) | Port literally with `i32` ops; output is checked by MOD-CC |
| `engine-lib/src/Util.ts:109-110` (`simpleHash`) | of-det |
| `GameImpl.ts:672` (`terrainByte << 16`, tile update packing) | view; only the journal uses it |

### 14.5 `.sort(` → `of_det::js_sort_by` (50 live sites [V]; `DefensePostExecution.ts:80` is commented out)

`MIRVExecution.ts:267`; `nation/NationWarshipBehavior.ts:345,411,487`; `nation/NationAttackBehavior.ts:189,416,424,726,730,981,1564`; `utils/AiAttackBehavior.ts:427`; `game/PlayerImpl.ts:854,966,1051,1684,1799`; `execution/Util.ts:128,129`; `SAMLauncherExecution.ts:262`; `nation/NationMIRVBehavior.ts:309,369`; `engine-lib/src/game/TeamAssignment.ts:53,224`; `nation/NationNukeBehavior.ts:337,408,668,782,819`; `nation/NationEmojiBehavior.ts:180,193`; `game/RailNetworkImpl.ts:460,565,577,631,781`; `WinCheckExecution.ts:84,198`; `pathfinding/algorithms/AbstractGraph.ts:195`; `nation/BotAnnexation.ts:62,87,90`; `nation/NationStructureBehavior.ts:1113`; `game/AttackImpl.ts:185`; `PortExecution.ts:134`; `nation/NationNukePlanning.ts:217,280,330,362`; `nation/TutorialSilo.ts:194`.

That is 50 entries. The grep finds 51 lines, the extra one being the commented `DefensePostExecution.ts:80`. The `NationAttackBehavior.ts:981` line carries a trailing comment, so a naive comment filter wrongly drops it.

Comparators that can be **inconsistent**, because NaN or ±∞ differences are possible:
- `b.score - a.score` (`SAMLauncherExecution.ts:262`, `NationNukeBehavior.ts:337`, `NationStructureBehavior.ts:1113`)
- `density(a) - density(b)` (`NationAttackBehavior.ts:726`)
- `a.distance - b.distance` (`:981`)
- `a.troopPercentage - b.troopPercentage` (`:1564`)
- `a.relation - b.relation` (`PlayerImpl.ts:966`).

These are exactly why the TimSort port is mandatory. **Comparator results are f64, and TimSort branches on `< 0`.** Do not convert them to `Ordering`.

### 14.6 `Map`/`Set` construction hot spots (files with ≥ 5; the full list comes from the grep)

`game/RailNetworkImpl.ts` 19, `game/PlayerImpl.ts` 18, `game/GameImpl.ts` 13, `game/WaterManager.ts` 11, `nation/NationStructurePlacement.ts` 11, `pathfinding/algorithms/AbstractGraph.ts` 10, `engine-lib/src/game/TeamAssignment.ts` 10, `game/TrainStation.ts` 9, `game/RailroadSpatialGrid.ts` 9, `nation/BotAnnexation.ts` 9, `nation/SharedWaterCache.ts` 8, `nation/NationWarshipBehavior.ts` 6, `DoomsdayClockExecution.ts` 6, `nation/NationAttackBehavior.ts` 5. Total: 95 `new Map`, 109 `new Set` (engine + engine-lib, excluding snapshot) [V].

For every one, decide `JsMap`/`JsSet` vs `SmallJsMap`/`SmallJsSet` vs lookup-only (§5.1), and annotate. Remember that a map iterated **only by its snapshot** is still iterated (CS order), for example `RailroadSpatialGrid.cells`.

### 14.7 BigInt sites (by file, [V])

`game/StatsImpl.ts` 48, `game/PlayerImpl.ts` 21, `configuration/EngineConfig.ts` 16, `game/UnitImpl.ts` 8, `engine-lib/src/Util.ts` 7, `engine-api/src/StatsSchemas.ts` 7, `game/GameImpl.ts` 6, `nation/NationStructureBehavior.ts` 6, `nation/NationAttackBehavior.ts` 4, `DonateGoldExecution.ts` 4, `nation/NationUtils.ts` 3, `nation/NationStructurePlacement.ts` 3, `nation/NationNukeBehavior.ts` 3, then 1–2 each in `AiAttackBehavior`, `TransportShipExecution`, `NationWarshipBehavior`, `TransportShipUtils`, `UpgradeStructureExecution`, `TradeShipExecution`, `NationNukePlanning`, `Game.ts`, `GameTypes.ts`.

**BigInt division sites** (each needs the `/ 0n` guard):
- `DonateGoldExecution.ts:58` (`gold / 3n`)
- `EngineConfig.ts:434` (`gold / 2n`)
- `NationNukeBehavior.ts:915` (`… * BigInt(left)) / BigInt(NUKE_PRICE_MEMORY_TICKS)`)
- `NationNukeBehavior.ts:1104,1109` (`* 150n / 100n`, `* 125n / 100n`).

**Plain `BigInt(…)` conversions that must stay strict** (no clamp): `EngineConfig.ts:281, 464, 684`; `NationStructureBehavior.ts:881` (`BigInt(Math.ceil(..))`); `DonateGoldExecution.ts:123`; `StatsImpl._bigint` (`:58-65`).

### 14.8 Order-sensitive object and `instanceof` sites

- `Object.entries(this.numUnitsConstructed)`: `PlayerImpl.ts:2034` (CS order). `Object.fromEntries`: `PlayerImpl.ts:1868` (`playerProfile`, view) and `:2109` (restore).
- `Object.entries(teamGameSpawnAreas)`: `engine-lib/src/game/TerrainMapLoader.ts:79`.
- `instanceof`: §10.

### 14.9 Other JS-semantics traps found while designing

| Trap | Site | Rule |
|---|---|---|
| **A TS array field replaced by `filter` while a loop walks it** | `UnitImpl.ts:275,353`; `AttackImpl.ts:61-71`; `GameImpl.ts:469,507,1143-1144`; loops `PlayerExecution.ts:60-79,107-111`, `AttackExecution.ts:169-178` | `JsArr::filter_reassign` + `iter_snapshot` (§5.3, ARCHITECTURE §4.7). Unit test per site. |
| `units()` / `units([])` return the **live** array; `units(type)` returns a copy | `PlayerImpl.ts:458-459,470-472,486-497` | Treat the no-arg form as the field itself (§5.3) |
| A getter with a side effect, reached from view code | `UnitImpl.warshipState()` writes `isInCombat` (`UnitImpl.ts:437-442`). It is called by `toUpdate()` at the tick-reachable sites in ARCHITECTURE §5 and directly by `WarshipExecution.ts:84`. The snapshot reads the raw field (`UnitImpl.ts:826`). | `cx.st.unit_touched(u)` shim at **every** `toUpdate()` site, **and** the Rust `warship_state()` getter keeps the write |
| **`unit.touch()` in executions is not view-only** | `UnitImpl.ts:145-147` (`touch()` = `addUpdate(toUpdate())`); callers `WarshipExecution.ts:86,687,707` (P7), `TradeShipExecution.ts:134` (P7), `NukeExecution.ts:504` (P8) | `touch()` → `cx.st.unit_touched(u)`. **Never drop it.** |
| Minimap terrain is edited by water nukes, magnitude included | `WaterManager.ts:493` (setWater), `:541,552` (setOcean), `:582-590` → `recomputeMagnitudesInBox(this.miniMap)` → `setMagnitude` `:881`; `GameMap.cost()` reads magnitude (`GameMapImpl.ts:296-297`) | Port the whole `WaterManager` edit sequence for **both** maps, and bump `terrain_gen` on every terrain write. Never shortcut minimap edits to setWater/setOcean. Covered by M-NUKE-WATER-01..03 and MOD-PATH-WATER after a nuke. |
| The constructor and `updateWarshipState` drop `isInCombat`; the `toUpdate()` that follows immediately writes it back | `UnitImpl.ts:104-110,459-467`; `PlayerImpl.ts:1427` | `Option<bool>` set to `None`, then the shim at the same statement position restores it. Never skip the shim at these two sites. |
| `init` runs even on inactive executions | `GameImpl.ts:614-627` | Do not add an `is_active` guard to the init loop. Each exec's own `init` checks `if (!this.active) return` where TS does (e.g. `AttackExecution.ts:71-73`). |
| The spawn phase flips mid-tick | `GameImpl.ts:536-552,606-613` | Re-read `in_spawn_phase()` per exec |
| `forEach` vs `for..of` length semantics | `GameImpl.ts:606` (forEach) vs `:617` (for..of) | §6 |
| A PRNG drawn in the **constructor** | `NationExecution.ts:76-78` (3 draws before `init`) | §1 |
| A PRNG object that is constructed but **never drawn** | `ExecutionManager.ts:49`; `TrainStation.ts:59-68,83,187` (`createTrainStopHandlers` ignores `random`) | Do not port it; no field |
| One PRNG shared by 7 nation behaviours | `NationExecution.ts:270-321` | `self.rng`. Debug with `prng-trace` (ARCHITECTURE §4.5). |
| **Virtual dispatch through `this.` in a base class** | `AiAttackBehavior.ts:94,100,133,200,219,308-309,317,379` → overrides `NationAttackBehavior.ts:1032-1272` | `AiAttackHooks` (§10) |
| A stats read that affects the sim | `GameImpl.ts:1777-1781` | ARCHITECTURE §3.10 |
| Stats BigInt conversion throws for clientID players, and `floor(x) + floor(-x) = -1` | `StatsImpl.ts:58-65,116-130,254-281` | Strict conversion at every stats call site, even in the lite build; port send/cancel literally |
| `numUnitsConstructed[type]++` on `Partial<Record>` | `PlayerImpl.ts:513-520` | The first construction appends the key (order list) |
| `Math.abs(simpleHash)` can be 2^31 | `engine-lib/src/Util.ts:112` | f64 result |
| `seed \| 0` truncation of PRNG seeds (sums of two `simpleHash` values exceed 2^31) | `PseudoRandom.ts:14`, `NationExecution.ts:73-75` | `Prng::new(f64)` → `to_int32` |
| TTL caches are state | `SharedWaterCache.ts:25,46-49` | clone; CS W |
| **Stale water steppers route on the CURRENT graph** with their old cached path, old chain kind and old memo | `WaterManager.ts:199-201`; `AStar.WaterHierarchical.ts:60-64`; `PathFinder.ts:47-66,145-162,271-285` | `WaterPathFinder.chain_version` → `ChainSlot`; the HPA reads `state.water.graph`; components are read live (ARCHITECTURE §3.8). **No graph `Arc` in steppers.** |
| `WaterPathMemo` LRU contents are observable | `PathFinder.ts:79-138` | A literal port, per chain slot, cloned. No feature flag. |
| The edge-segment cache depends on query history | `AbstractGraph.ts:105-136`; `AStar.WaterHierarchical.ts:242-273` | Per-game `filled` bitset + overlay; shared values only while `terrain_gen == built_at_mini_gen` (ARCHITECTURE §3.8) |
| Stamp counters move from per-game (TS) to per-thread (Rust) | `TileTraversalScratch.ts:43-50`; `PlayerExecution.ts:371` (multi-pass gen range); WaterManager u16 stamps | Step-start reset when past half range; forced-wrap unit test (ARCHITECTURE §3.8, G14) |
| Separate BFS scratch per map instance | `GameMapImpl.ts:9-12,477-479` | `sc.bfs_main` ≠ `sc.bfs_mini`; `busy` flag → `ts_throw!` |
| `JsMap`/`JsSet` `clear()` during iteration | ECMAScript; node prints `1,9,10` for the test in §5.2 | The iterator continues (§5.2 item 1) |
| Duplicate player id would split Map order from smallID order | `GameImpl.ts:906-918` | `add_player` asserts the id is new |

---

## 15. Review checklist (each PR)

1. Clippy is clean: no `f64::max/min/round/powf…`, no `as i32/u32` on f64, no std HashMap iteration, no `sort_by`, no `RefCell`/`Rc`, no `catch_unwind` outside the two allowed files, no bare `expect`/`unwrap` in `exec/`/`state/`.
2. Every Map/Set has a `JsMap`/`SmallJsMap`/lookup-only decision with an annotation.
3. Every loop that mutates its source has `// iter-mut:`/`iter-snapshot:`/`iter-live:` and a test. Every filter-reassigned field is a `JsArr` removed only via `filter_reassign`.
4. Every possibly-OOB index uses `.get()` with `undefined` semantics, or has `// in-range:`.
5. Every TS `throw` is `ts_throw!` with the same message; every TS `TypeError` site is `.ts_expect`; every BigInt conversion is strict.
6. PRNG seeding happens at the same moment (constructor vs init) and with the same seed expression.
7. Every `touch()`/`toUpdate()` site in your files calls `unit_touched` (ARCHITECTURE §5 list).
8. Base-class methods that TS calls through `this.` and that a subclass overrides go through the hook trait.
9. `Canon` field order equals the schema dump order; absent vs null is preserved. `RefVisitor` reports every handle (the GC depends on it).
10. The upstream vitest for the file is ported (if one exists), the relevant VECTORS suite passes, and the package's micro scenarios pass at L2 (L3 once `stats-full` is wired).
