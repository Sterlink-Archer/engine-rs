//! `MIRVExecution` (`engine/src/execution/MIRVExecution.ts`). Owner: **P8**.
//!
//! Fields: the `MIRV v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`), then non-snapshot fields. `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1).
//!
//! The only exec→exec reference (ARCHITECTURE §4.3): warheads are added with
//! `add_execution` + `ExecCmd::BindWarhead`, cancelled with `ExecCmd::CancelNuke` + `Unpin`.
//! `displayIncomingUnit` (`:162-169`) is view work (dropped; its arguments are pure getters).

use crate::ctx::ts_throw;
use crate::exec::nuke::NukeExec;
#[allow(unused_imports)]
use crate::exec::prelude::*;
use crate::map::GameMap;
use crate::path::line::DistanceBasedBezierCurve;
use crate::path::parabola::{ParabolaOptions, get_parabola_control_points};
use crate::path::types::PathResult;
use crate::state::State;
use crate::state::unit::UnitParams;
use of_det::js;

/// `MATH_SCALE = 100` (`:52`).
const MATH_SCALE: f64 = 100.0;
/// `longFlightMult = 14` (`:53`).
const LONG_FLIGHT_MULT: f64 = 14.0;
/// `longFlightLinearPercent = 10` (`:54`).
const LONG_FLIGHT_LINEAR_PERCENT: f64 = 10.0;
/// `shortFlightMult = 10` (`:55`).
const SHORT_FLIGHT_MULT: f64 = 10.0;
/// `SATURATION_THRESHOLD = 1500` (`:77`).
const SATURATION_THRESHOLD: i32 = 1500;

/// The `init` block of the MIRV snapshot (`random`, `targetPlayer`, `baseX`, `baseY`).
#[derive(StateClone, Debug)]
pub struct MirvInit {
    /// `PseudoRandom(ticks + simpleHash(player.id))` (`:93`).
    pub rng: Prng,
    pub target_player: Owner,
    pub base_x: i32,
    pub base_y: i32,
}

/// The `launch` block (`spawnTile`, `separateDst`, `pathFinder`).
#[derive(StateClone, Debug, Default)]
pub struct MirvLaunch {
    pub spawn_tile: TileRef,
    pub separate_dst: TileRef,
    pub path_finder: ParabolaPathFinder,
}

/// `MIRVExecution` data.
#[derive(StateClone, Debug)]
pub struct MirvExec {
    pub active: bool,
    pub player: SmallId,
    pub dst: TileRef,
    /// `range = 1500`
    pub range: i32,
    pub range_squared: i32,
    /// `minimumSpread = 55`
    pub minimum_spread: i32,
    /// `warheadCount = 350`
    pub warhead_count: i32,
    /// set by `init`
    pub init: Option<MirvInit>,
    pub nuke: Option<UnitRef>,
    pub launch: Option<MirvLaunch>,
    /// `speed = -1`
    pub speed: f64,
    pub full_path: Vec<TileRef>,
    pub path_index: u32,
    /// filter-reassign: not iterated across (`:237`, ARCHITECTURE §4.7)
    pub staged_targets: Vec<TileRef>,
    pub warheads_spawned: bool,
    /// the only exec→exec reference (§4.3)
    pub warhead_executions: Vec<WarheadRef>,
    pub consecutive_overlap_failures: i32,
    /// non-snapshot: `minX, maxX, minY, maxY` (derived in `init`)
    pub bounds: [i32; 4],
    /// non-snapshot: `spatialGrid: Int32Array` (`:79`)
    pub spatial_grid: Vec<i32>,
}

/// The grid geometry of `MirvExecution` (`baseX/baseY/range/minimumSpread/gridWidth`), so the
/// overlap-grid methods (`:311-434`) can run on any tile → (x, y) function.
#[derive(Clone, Copy, Debug)]
struct Grid {
    base_x: f64,
    base_y: f64,
    range: f64,
    minimum_spread: f64,
    grid_width: f64,
}

impl Grid {
    /// The cell `(cx, cy)` of `(x, y)`, or `None` when outside the grid (`:312-318`).
    #[inline]
    fn cell(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let rel_x = x - self.base_x + self.range;
        let rel_y = y - self.base_y + self.range;
        let cx = js::floor(rel_x / self.minimum_spread);
        let cy = js::floor(rel_y / self.minimum_spread);
        if cx < 0.0 || cx >= self.grid_width || cy < 0.0 || cy >= self.grid_width {
            return None;
        }
        Some((cx, cy))
    }

    /// `(cy * gridWidth + cx) * 2` for an in-grid cell.
    #[inline]
    fn base_offset(&self, cx: f64, cy: f64) -> usize {
        let o = (cy * self.grid_width + cx) * 2.0;
        debug_assert!(o >= 0.0 && o.fract() == 0.0);
        o as usize
    }

    /// `addToGrid(grid, tile)` (`:311-327`). Typed-array semantics: an out-of-range read is
    /// `undefined` (never `=== -1`), so nothing is written into an emptied grid.
    fn add_to_grid(&self, grid: &mut [i32], tile: TileRef, xy: impl Fn(TileRef) -> (f64, f64)) {
        let (x, y) = xy(tile);
        let Some((cx, cy)) = self.cell(x, y) else { return };
        let base_offset = self.base_offset(cx, cy);
        for i in 0..2 {
            if let Some(slot) = grid.get_mut(base_offset + i)
                && *slot == -1
            {
                // TS: Int32Array store of a TileRef (< 2^31 on every map: exact).
                *slot = js::to_int32(f64::from(tile));
                return;
            }
        }
    }

    /// `removeFromGrid(tile)` (`:329-346`).
    fn remove_from_grid(&self, grid: &mut [i32], tile: TileRef, xy: impl Fn(TileRef) -> (f64, f64)) {
        if grid.is_empty() {
            return;
        }
        let (x, y) = xy(tile);
        let Some((cx, cy)) = self.cell(x, y) else { return };
        let base_offset = self.base_offset(cx, cy);
        let t = js::to_int32(f64::from(tile));
        // in-range: an in-grid cell's two slots are inside the gridWidth² × 2 array.
        if grid.get(base_offset) == Some(&t) {
            grid[base_offset] = grid[base_offset + 1];
            grid[base_offset + 1] = -1;
        } else if grid.get(base_offset + 1) == Some(&t) {
            grid[base_offset + 1] = -1;
        }
    }

    /// `buildOverlapGrid(taken)` (`:348-355`).
    fn build_overlap_grid(&self, taken: &[TileRef], xy: impl Fn(TileRef) -> (f64, f64)) -> Vec<i32> {
        let gw = self.grid_width as usize; // int: gridWidth = ceil(3000 / 55) + 2 = 57
        let mut grid = vec![-1i32; gw * gw * 2]; // TS: new Int32Array(n).fill(-1)
        for &t in taken {
            self.add_to_grid(&mut grid, t, &xy);
        }
        grid
    }

    /// `isOverlapping(x, y)` (`:393-434`).
    fn is_overlapping(&self, grid: &[i32], x: f64, y: f64, xy: impl Fn(TileRef) -> (f64, f64)) -> bool {
        let Some((cx, cy)) = self.cell(x, y) else { return true };

        // The grid strictly allocates 2 slots per cell. A 55x55 cell can mathematically hold 3 targets
        // without violating the minimumSpread, but we restrict it to 2 to optimize memory and CPU.
        // If the target cell is already full, we must reject the target to prevent it from being
        // silently dropped by addToGrid, which would allow subsequent targets to overlap it.
        let target_cell_offset = self.base_offset(cx, cy);
        // TS: `spatialGrid[off + 1] !== -1` (an OOB read is `undefined`, which is `!== -1`).
        if grid.get(target_cell_offset + 1) != Some(&-1) {
            return true;
        }

        let mut gy = cy - 1.0;
        while gy <= cy + 1.0 {
            if gy < 0.0 || gy >= self.grid_width {
                gy += 1.0;
                continue;
            }
            let mut gx = cx - 1.0;
            while gx <= cx + 1.0 {
                if gx < 0.0 || gx >= self.grid_width {
                    gx += 1.0;
                    continue;
                }
                let base_offset = self.base_offset(gx, gy);
                for i in 0..2 {
                    // TS: `const t = spatialGrid[i]; if (t === -1) break;` (OOB → undefined:
                    // not -1, and `mg.x(undefined)` would be NaN, failing both `<` tests).
                    let Some(&t) = grid.get(base_offset + i) else { continue };
                    if t == -1 {
                        break;
                    }
                    #[allow(clippy::cast_sign_loss)] // t >= 0: a stored TileRef
                    let (tx, ty) = xy(t as u32);
                    let dx = js::abs(x - tx);
                    if dx < self.minimum_spread {
                        let dy = js::abs(y - ty);
                        if dx + dy < self.minimum_spread {
                            return true;
                        }
                    }
                }
                gx += 1.0;
            }
            gy += 1.0;
        }

        false
    }
}

/// `calculateDeterministicSpeed` (`:448-521`) after the two curve lengths: the integer
/// normalisation. `ideal_len`/`actual_len` are `DistanceBasedBezierCurve.getLength` of the
/// uncapped and the actual control points; `base_speed = nukeSpeed(MIRV)`,
/// `base_ticks = mirvNormalizeTargetTicks()`.
fn mirv_speed_from_lengths(ideal_len: f64, base_speed: f64, base_ticks: f64, actual_len: f64) -> f64 {
    // Math.sqrt and basic fractional division are IEEE-754 deterministic across all compliant JS engines.
    let ideal_mirv_ticks = ideal_len / base_speed; // TS: :472

    let base_ticks_scaled = base_ticks * MATH_SCALE; // TS: :475

    let ideal_mirv_ticks_int = js::floor(ideal_mirv_ticks * MATH_SCALE); // TS: :477-479
    let mut target_mirv_ticks_int = base_ticks_scaled;

    // normalize towards baseTicks using sqrt heuristic to stay deterministic
    if ideal_mirv_ticks_int > base_ticks_scaled {
        let diff = ideal_mirv_ticks_int - base_ticks_scaled;
        // TS: :486-489 (left-assoc sum)
        target_mirv_ticks_int = base_ticks_scaled
            + js::floor(js::sqrt(diff)) * LONG_FLIGHT_MULT
            + js::floor((diff * LONG_FLIGHT_LINEAR_PERCENT) / 100.0);
    } else if ideal_mirv_ticks_int < base_ticks_scaled {
        let diff = base_ticks_scaled - ideal_mirv_ticks_int;
        target_mirv_ticks_int = base_ticks_scaled - js::floor(js::sqrt(diff)) * SHORT_FLIGHT_MULT;
    }
    target_mirv_ticks_int = js::max(MATH_SCALE, target_mirv_ticks_int); // TS: :496

    let actual_mirv_length_int = js::round(actual_len * 256.0); // TS: :510 (Math.round, PG §14.1)

    // Pure integer division: Math.floor((num + Math.floor(den / 2)) / den)
    let num = actual_mirv_length_int * MATH_SCALE;
    let den = target_mirv_ticks_int;
    let pure_integer_speed_scaled = js::floor((num + js::floor(den / 2.0)) / den);
    // safe to use in Parabola since Parabola uses x256 scaled integers.
    js::max(1.0, pure_integer_speed_scaled / 256.0)
}

impl MirvExec {
    /// `new MIRVExecution(player, dst)`.
    pub fn new(player: SmallId, dst: TileRef) -> Self {
        // TS: MIRVExecution.ts:41-90 field initialisers (`rangeSquared = range * range`,
        // `spatialGrid = new Int32Array(0)`); `mg`, `random`, `pathFinder` unset until init/launch.
        let range = 1500;
        MirvExec {
            active: true,
            player,
            dst,
            range,
            range_squared: range * range,
            minimum_spread: 55,
            warhead_count: 350,
            init: None,
            nuke: None,
            launch: None,
            speed: -1.0,
            full_path: Vec::new(),
            path_index: 0,
            staged_targets: Vec::new(),
            warheads_spawned: false,
            warhead_executions: Vec::new(),
            consecutive_overlap_failures: 0,
            bounds: [0; 4],
            spatial_grid: Vec::new(),
        }
    }

    /// `owner()` (`:436-438`).
    pub fn owner(&self) -> SmallId {
        self.player
    }

    /// `gridWidth = Math.ceil((range * 2) / minimumSpread) + 2` (`:80-81, :98, :569`).
    fn grid_width(&self) -> f64 {
        js::ceil(f64::from(self.range * 2) / f64::from(self.minimum_spread)) + 2.0
    }

    /// The overlap-grid geometry (needs `init`'s `baseX/baseY`).
    fn grid(&self) -> Grid {
        let init = self.init.as_ref().ts_expect("MIRVExecution: baseX before init");
        Grid {
            base_x: f64::from(init.base_x),
            base_y: f64::from(init.base_y),
            range: f64::from(self.range),
            minimum_spread: f64::from(self.minimum_spread),
            grid_width: self.grid_width(),
        }
    }

    /// The MIRV's own warhead cancel loop (`:181-187`): `warhead.cancel()` for each listed
    /// warhead, then `warheadExecutions = []`. Ported as `ExecCmd::CancelNuke` for each, then
    /// `ExecCmd::Unpin` for each (ARCHITECTURE §4.3); the driver applies them right after this
    /// `tick` returns, which is equivalent because they are MIRV's last side effects.
    fn cancel_warheads(&mut self, cx: &mut Ctx) {
        for &w in &self.warhead_executions {
            cx.st.exec_cmds.push(ExecCmd::CancelNuke(w));
        }
        for &w in &self.warhead_executions {
            cx.st.exec_cmds.push(ExecCmd::Unpin(w));
        }
        self.warhead_executions.clear();
    }

    /// `finalizeDestinations(additionalAttempts = 500)` (`:234-276`).
    fn finalize_destinations(&mut self, cx: &mut Ctx, additional_attempts: i32) {
        let count_before = self.staged_targets.len();
        // Re-check target tile ownership at tick 10
        // TS: `this.stagedTargets = this.stagedTargets.filter(..)` with a side-effecting
        // predicate (removeFromGrid), called once per element in order — `Vec::retain` does
        // the same. // filter-reassign: not iterated across
        let grid = self.grid();
        let target_player = self.init.as_ref().ts_expect("MIRVExecution.ts:238 targetPlayer").target_player;
        let dst = self.dst;
        let map = &cx.st.map;
        let st = &*cx.st;
        let spatial_grid = &mut self.spatial_grid;
        self.staged_targets.retain(|&tile| {
            if tile == dst || st.owner(tile) == target_player {
                return true;
            }
            grid.remove_from_grid(spatial_grid, tile, |t| xy(map, t));
            false
        });

        if self.staged_targets.len() < count_before {
            self.consecutive_overlap_failures = 0;
        }

        if self.consecutive_overlap_failures < SATURATION_THRESHOLD {
            // Top-up loop using specified attempt budget if targets were lost or not yet filled
            for _attempt in 0..additional_attempts {
                if self.staged_targets.len() >= self.warhead_count_usize() {
                    break;
                }
                let target = self.try_generate_target(cx);
                self.push_target(cx, target);
                if self.consecutive_overlap_failures >= SATURATION_THRESHOLD {
                    break;
                }
            }
        }

        // Sort in place
        // TS: :267-270 `manhattanDist(b, dst) - manhattanDist(a, dst)` (V8 TimSort).
        let map = &cx.st.map;
        of_det::js_sort_by(&mut self.staged_targets, |&a, &b| {
            f64::from(map.manhattan_dist(b, dst)) - f64::from(map.manhattan_dist(a, dst))
        });

        // We no longer need the spatial grid after targets are finalized,
        // and clearing it ensures the snapshot comparison tool doesn't complain about map insertion order
        // while also freeing the memory.
        self.spatial_grid = Vec::new(); // TS: new Int32Array(0)
    }

    /// `if (target) { stagedTargets.push(target); addToGrid(spatialGrid, target) }`
    /// (`:201-205`, `:253-257`). **`if (target)` is falsy for tile 0**: TileRef 0 is never
    /// staged (literal port).
    fn push_target(&mut self, cx: &Ctx, target: Option<TileRef>) {
        if let Some(t) = target
            && t != 0
        {
            self.staged_targets.push(t);
            let grid = self.grid();
            let map = &cx.st.map;
            grid.add_to_grid(&mut self.spatial_grid, t, |t| xy(map, t));
        }
    }

    fn warhead_count_usize(&self) -> usize {
        usize::try_from(self.warhead_count).unwrap_or(0) // int: 350
    }

    /// `spawnWarheadsWithWait(remainingTicks)` (`:278-301`).
    fn spawn_warheads_with_wait(&mut self, cx: &mut Ctx, remaining_ticks: i64) {
        if self.nuke.is_none() {
            return;
        }

        let wait_base = std::cmp::max(0, remaining_ticks); // int: Math.max on integers
        let warhead_speed = cx.st.config().nuke_speed(UnitType::MirvWarhead);
        self.queue_warheads(cx, wait_base, warhead_speed);
    }

    /// The `for (const [i, dst] of this.stagedTargets.entries())` loop of
    /// `spawnWarheadsWithWait` (`:283-300`).
    fn queue_warheads(&mut self, cx: &mut Ctx, wait_base: i64, warhead_speed: f64) {
        // TS: `this.separateDst` (set on the launch tick, before any warhead spawns).
        let separate_dst = self.launch.as_ref().map(|l| l.separate_dst).ts_expect("MIRVExecution.ts:293 separateDst");
        // iter-live: the loop body never touches `stagedTargets`.
        for i in 0..self.staged_targets.len() {
            let dst = self.staged_targets[i]; // in-range: i < len
            let speed_offset = if i < 70 {
                0.0
            } else if i < 140 {
                1.0
            } else if i < 210 {
                2.0
            } else if i < 280 {
                3.0
            } else {
                4.0
            };
            // order of extra speed assign does not matter, they all spawn at once.
            let rng = &mut self.init.as_mut().ts_expect("MIRVExecution.ts:296 this.random").rng;
            let wait = wait_base as f64 + rng.next_int(0.0, 15.0); // TS: waitBase + nextInt(0, 15)
            let execution = NukeExec::new(
                UnitType::MirvWarhead,
                self.player,
                dst,
                Tri::Some(separate_dst),
                warhead_speed + speed_offset,
                js::to_int32(wait), // int: small non-negative integer
                true,
            );
            // TS: `warheadExecutions.push(execution); mg.addExecution(execution);` — the bind
            // command rewrites entry `k` to the drained ExecId (ARCHITECTURE §4.3).
            let pending = cx.st.add_execution(Exec::Nuke(execution));
            #[allow(clippy::expect_used)] // rust-invariant: < 2^32 warheads
            let k = u32::try_from(self.warhead_executions.len()).expect("warhead index fits u32");
            self.warhead_executions.push(WarheadRef::Pending(pending));
            cx.st.exec_cmds.push(ExecCmd::BindWarhead { k, pending });
        }
    }

    /// `separate()` (`:303-309`).
    fn separate(&mut self, cx: &mut Ctx) {
        let Some(nuke) = self.nuke else {
            ts_throw!("uninitialized");
        };
        cx.unit_mut(nuke).delete(Some(false), None);
    }

    /// `tryGenerateTarget()` (`:357-391`).
    fn try_generate_target(&mut self, cx: &mut Ctx) -> Option<TileRef> {
        if self.consecutive_overlap_failures >= SATURATION_THRESHOLD {
            return None;
        }
        let grid = self.grid();
        let [min_x, max_x, min_y, max_y] = self.bounds.map(f64::from);
        let range_squared = f64::from(self.range_squared);

        for _attempt in 0..100 {
            let init = self.init.as_mut().ts_expect("MIRVExecution.ts:363 this.random");
            let r1 = init.rng.next();
            let r2 = (r1 * 15485863.0) % 1.0; // TS: f64 `%` (fmod)

            let x = min_x + js::floor(r1 * (max_x - min_x + 1.0));
            let y = min_y + js::floor(r2 * (max_y - min_y + 1.0));

            let dx = x - grid.base_x;
            let dy = y - grid.base_y;
            if dx * dx + dy * dy > range_squared {
                continue;
            }

            let tile = cx.st.map.ref_(x, y);

            if cx.st.owner(tile) != init.target_player {
                continue;
            }
            if !cx.st.map.is_land(tile) || cx.st.map.is_impassable(tile) {
                continue;
            }

            let map = &cx.st.map;
            if grid.is_overlapping(&self.spatial_grid, x, y, |t| xy(map, t)) {
                self.consecutive_overlap_failures += 1;
                if self.consecutive_overlap_failures >= SATURATION_THRESHOLD {
                    return None;
                }
                continue;
            }

            self.consecutive_overlap_failures = 0;
            return Some(tile);
        }
        None
    }

    /// `calculateDeterministicSpeed(spawnTile, separateDst)` (`:448-521`).
    fn calculate_deterministic_speed(&self, st: &State, spawn_tile: TileRef, separate_dst: TileRef) -> f64 {
        // using base speed and uncapped curve, calculate ideal mirv flight ticks
        // prevents top of map shenanigans
        // Intentionally pass the clamped separateDst so timing matches the drawn physical arc exactly.
        let [ip0, ip1, ip2, ip3] = get_parabola_control_points(
            &st.map,
            spawn_tile,
            separate_dst,
            Some(&ParabolaOptions {
                increment: None,
                distance_based_height: Some(true),
                direction_up: Some(true),
                ignore_map_bounds: Some(true),
            }),
        );
        let ideal_mirv_length = DistanceBasedBezierCurve::get_length(ip0, ip1, ip2, ip3);
        let base_speed = st.config().nuke_speed(UnitType::Mirv);
        let base_ticks = st.config().mirv_normalize_target_ticks();

        let [ap0, ap1, ap2, ap3] = get_parabola_control_points(&st.map, spawn_tile, separate_dst, None);
        let actual_mirv_length = DistanceBasedBezierCurve::get_length(ap0, ap1, ap2, ap3);

        // TS evaluates the two config reads and both curves in this order too; all four are
        // pure, so computing the integer part after them is the same expression sequence.
        mirv_speed_from_lengths(ideal_mirv_length, base_speed, base_ticks, actual_mirv_length)
    }
}

/// `(mg.x(t), mg.y(t))` as JS numbers.
#[inline]
fn xy(map: &GameMap, t: TileRef) -> (f64, f64) {
    (f64::from(map.x(t)), f64::from(map.y(t)))
}

impl ExecBehavior for MirvExec {
    fn is_active(&self) -> bool {
        self.active
    }
    /// `activeDuringSpawnPhase()` returns `false` (`MIRVExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    /// `init(mg, ticks)` (`:92-105`).
    fn init(&mut self, cx: &mut Ctx, _ticks: i32) {
        // TS: `new PseudoRandom(mg.ticks() + simpleHash(this.player.id()))` (seed `| 0` inside).
        let seed = f64::from(cx.st.ticks()) + of_det::util::simple_hash(cx.st.player(self.player).player_id());
        let rng = Prng::new(seed);
        let target_player = cx.st.owner(self.dst);
        // int: map coordinates (< 2^16).
        let base_x = cx.st.map.x(self.dst) as i32;
        let base_y = cx.st.map.y(self.dst) as i32;
        self.init = Some(MirvInit { rng, target_player, base_x, base_y });
        // TS: :98 `gridWidth` recomputed from range/minimumSpread (`grid_width()`).
        // int: Math.max/Math.min of integers.
        let w = cx.st.width() as i32;
        let h = cx.st.height() as i32;
        self.bounds = [
            std::cmp::max(0, base_x - self.range),
            std::cmp::min(w - 1, base_x + self.range),
            std::cmp::max(0, base_y - self.range),
            std::cmp::min(h - 1, base_y + self.range),
        ];
        self.staged_targets = vec![self.dst];
        let grid = self.grid();
        let map = &cx.st.map;
        self.spatial_grid = grid.build_overlap_grid(&self.staged_targets, |t| xy(map, t));
    }
    /// `tick(ticks)` (`:107-232`).
    fn tick(&mut self, cx: &mut Ctx, _ticks: i32) {
        if self.nuke.is_none() {
            let spawn = cx.st.player(self.player).can_build(cx.sc, UnitType::Mirv, self.dst, None);
            let Some(spawn) = spawn else {
                // TS: console.warn(`cannot build MIRV`) (dropped)
                self.active = false;
                return;
            };
            let target_player = cx.st.owner(self.dst);
            self.init.as_mut().ts_expect("MIRVExecution.ts:115 init").target_player = target_player;
            let spawn_tile = spawn; // TS: `this.spawnTile = spawn` (stored with `launch`, below)
            let nuke = cx.player_mut(self.player).build_unit(
                UnitType::Mirv,
                spawn,
                UnitParams {
                    target_tile: Some(Some(self.dst)),
                    target_player: Some(Some(target_player)),
                    ..UnitParams::default()
                },
            );
            self.nuke = Some(nuke);
            cx.record_mirv_launch();
            cx.st.stats_cx().bomb_launch(self.player, target_player, UnitType::Mirv);

            // Betrayal on launch — only once the missile has actually spawned, so
            // a fizzled launch pays no diplomatic cost.
            if target_player.is_player() {
                let alliance = cx.st.player(self.player).alliance_with(target_player);
                if let Some(alliance) = alliance {
                    cx.player_mut(self.player).break_alliance(alliance);
                }
                if target_player != self.player {
                    cx.player_mut(target_player).update_relation(self.player, -100.0);
                    cx.player_mut(self.player).update_relation(target_player, -100.0);
                }
            }
            let base_x = f64::from(self.init.as_ref().ts_expect("MIRVExecution.ts:136 baseX").base_x);
            let base_y = f64::from(self.init.as_ref().ts_expect("MIRVExecution.ts:137 baseY").base_y);
            let nuke_x = f64::from(cx.st.map.x(cx.st.unit(nuke).tile()));
            let x = js::floor((base_x + nuke_x) / 2.0);
            let y = js::max(0.0, base_y - 500.0) + 50.0;
            let separate_dst = cx.st.map.ref_(x, y);

            self.speed = self.calculate_deterministic_speed(cx.st, spawn_tile, separate_dst);
            // TS: `UniversalPathFinding.Parabola(this.mg, { increment: this.speed })`.
            let path_finder = ParabolaPathFinder::new(Some(ParabolaOptions {
                increment: Some(self.speed),
                distance_based_height: None,
                direction_up: None,
                ignore_map_bounds: None,
            }));
            // TS: `spawnTile`, `separateDst` and `pathFinder` are all set by here; the
            // snapshot's `launch` block keys on `pathFinder` (`:543-550`).
            self.launch = Some(MirvLaunch { spawn_tile, separate_dst, path_finder });
            let speed = self.speed;
            let launch = self.launch.as_mut().ts_expect("MIRVExecution.ts:148 pathFinder");
            let map = &cx.st.map;
            let mut result = launch.path_finder.next(map, spawn_tile, separate_dst, Some(speed));
            while let PathResult::Next(node) = result {
                self.full_path.push(node);
                result = launch.path_finder.next(map, spawn_tile, separate_dst, Some(speed));
            }

            // TS: :162-169 displayIncomingUnit (view, dropped).

            // after sending a nuke set the missilesilo on cooldown
            let silo = cx
                .st
                .player(self.player)
                .units_of(&[UnitType::MissileSilo])
                .into_iter()
                .find(|&s| cx.st.unit(s).tile() == spawn);
            if let Some(silo) = silo {
                cx.unit_mut(silo).launch();
            }
        }

        // make the MIRV inactive if it was destroyed or cancelled externally
        let nuke = self.nuke.ts_expect("MIRVExecution.ts:181 this.nuke");
        if !cx.st.unit(nuke).is_active() {
            self.active = false;
            self.cancel_warheads(cx);
            return;
        }

        // int: path lengths/indices. TS: `this.fullPath.length - this.pathIndex`.
        let remaining_ticks = self.full_path.len() as i64 - i64::from(self.path_index);

        // Stagger destination finding across ticks 20 down to 11 before separation
        if remaining_ticks <= 20
            && remaining_ticks > 10
            && self.consecutive_overlap_failures < SATURATION_THRESHOLD
            && self.staged_targets.len() < self.warhead_count_usize()
        {
            for _attempt in 0..100 {
                if self.staged_targets.len() >= self.warhead_count_usize() {
                    break;
                }

                let target = self.try_generate_target(cx);
                self.push_target(cx, target);
                if self.consecutive_overlap_failures >= SATURATION_THRESHOLD {
                    break;
                }
            }
        }

        // At <= 10 ticks before separation, re-validate tile ownership, finalize targets, sort, and instantiate NukeExecutions for SAM detection
        if remaining_ticks <= 10 && !self.warheads_spawned {
            self.warheads_spawned = true;
            // Compensate missed precomputing targets if remainingTicks started <15
            let extra_attempts = (10 - remaining_ticks) * 50;
            #[allow(clippy::cast_possible_truncation)] // int: 500 + 50 × (10 - remaining) ≤ 1000
            self.finalize_destinations(cx, (500 + extra_attempts) as i32);
            self.spawn_warheads_with_wait(cx, remaining_ticks);
        }

        if (self.path_index as usize) < self.full_path.len() {
            let node = self.full_path[self.path_index as usize]; // in-range: checked above
            self.path_index += 1;
            cx.unit_mut(nuke).move_to(node);
        } else {
            self.separate(cx);
            self.active = false;
            // Record stats
            let target_player = self.init.as_ref().ts_expect("MIRVExecution.ts:230 targetPlayer").target_player;
            cx.st.stats_cx().bomb_land(self.player, target_player, UnitType::Mirv);
        }
    }
}

// CS v1 `MIRV v2 (execution)` record (`MIRVExecution.ts:523-559`), dump field order: `init` is
// `null` before `init`, `launch` before the launch tick; `fullPath`/`stagedTargets` are
// `w.tiles(..)` (TYPED U32); `warheadExecutions` are exec keys. Written at merge1 by the
// integrator on behalf of P8.
crate::impl_canon_record!(MirvInit {
    "random" => rng,
    "targetPlayer" => target_player,
    "baseX" => base_x,
    "baseY" => base_y,
});
crate::impl_canon_record!(MirvLaunch {
    "spawnTile" => spawn_tile,
    "separateDst" => separate_dst,
    "pathFinder" => path_finder,
});
impl crate::canon::Canon for MirvExec {
    fn canon(&self, cx: &mut crate::canon::CanonCx<'_>) -> crate::canon::Cs {
        use crate::canon::Cs;
        Cs::Obj(vec![
            ("active".into(), Cs::Bool(self.active)),
            ("player".into(), self.player.canon(cx)),
            ("dst".into(), self.dst.canon(cx)),
            ("range".into(), self.range.canon(cx)),
            ("rangeSquared".into(), self.range_squared.canon(cx)),
            ("minimumSpread".into(), self.minimum_spread.canon(cx)),
            ("warheadCount".into(), self.warhead_count.canon(cx)),
            ("init".into(), self.init.canon(cx)),
            ("nuke".into(), self.nuke.canon(cx)),
            ("launch".into(), self.launch.canon(cx)),
            ("speed".into(), self.speed.canon(cx)),
            ("fullPath".into(), crate::canon::tiles(self.full_path.iter().copied())),
            ("pathIndex".into(), self.path_index.canon(cx)),
            ("stagedTargets".into(), crate::canon::tiles(self.staged_targets.iter().copied())),
            ("warheadsSpawned".into(), Cs::Bool(self.warheads_spawned)),
            ("warheadExecutions".into(), self.warhead_executions.canon(cx)),
            ("consecutiveOverlapFailures".into(), self.consecutive_overlap_failures.canon(cx)),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::ExecStore;
    use crate::ids::ExecKind;
    use crate::scratch::Scratch;

    /// A 2000×2000 test geometry: TileRef = y * W + x.
    const W: u32 = 2000;
    fn txy(t: TileRef) -> (f64, f64) {
        (f64::from(t % W), f64::from(t / W))
    }
    fn tref(x: u32, y: u32) -> TileRef {
        y * W + x
    }
    fn grid_at(bx: u32, by: u32) -> Grid {
        let m = MirvExec::new(SmallId(1), tref(bx, by));
        Grid {
            base_x: f64::from(bx),
            base_y: f64::from(by),
            range: f64::from(m.range),
            minimum_spread: f64::from(m.minimum_spread),
            grid_width: m.grid_width(),
        }
    }

    #[test]
    fn new_matches_ts_field_initialisers() {
        let m = MirvExec::new(SmallId(4), 1234);
        assert!(m.is_active() && !m.active_during_spawn_phase());
        assert_eq!((m.range, m.range_squared, m.minimum_spread, m.warhead_count), (1500, 2_250_000, 55, 350));
        assert_eq!(m.speed, -1.0);
        assert!(m.init.is_none() && m.launch.is_none() && m.nuke.is_none());
        assert!(m.spatial_grid.is_empty() && m.staged_targets.is_empty() && m.warhead_executions.is_empty());
        // TS: Math.ceil(3000 / 55) + 2 = 55 + 2
        assert_eq!(m.grid_width(), 57.0);
        assert_eq!(m.owner(), SmallId(4));
    }

    #[test]
    fn overlap_grid_two_slots_per_cell() {
        let g = grid_at(1000, 1000);
        let base = tref(1000, 1000);
        let mut grid = g.build_overlap_grid(&[base], txy);
        assert_eq!(grid.len(), 57 * 57 * 2);
        // base sits at rel (1500, 1500) → cell (27, 27) (1500 / 55 = 27.27).
        let off = (27 * 57 + 27) * 2;
        assert_eq!((grid[off], grid[off + 1]), (base as i32, -1));
        // a second tile in the same cell fills slot 1; a third is dropped by addToGrid.
        let t2 = tref(1010, 1010);
        let t3 = tref(1020, 1000);
        g.add_to_grid(&mut grid, t2, txy);
        g.add_to_grid(&mut grid, t3, txy);
        assert_eq!((grid[off], grid[off + 1]), (base as i32, t2 as i32));
        // a full target cell always overlaps (`:407-410`), even far from both entries.
        assert!(g.is_overlapping(&grid, 1030.0, 1030.0, txy));
        // remove slot 0: slot 1 shifts down (`:340-342`).
        g.remove_from_grid(&mut grid, base, txy);
        assert_eq!((grid[off], grid[off + 1]), (t2 as i32, -1));
        g.remove_from_grid(&mut grid, t2, txy);
        assert_eq!((grid[off], grid[off + 1]), (-1, -1));
        // removing from an empty (finalized) grid and adding to it are no-ops.
        let mut empty: Vec<i32> = Vec::new();
        g.remove_from_grid(&mut empty, base, txy);
        g.add_to_grid(&mut empty, base, txy);
        assert!(empty.is_empty());
        // an empty grid reads `undefined` at the target cell → overlapping (`!== -1`).
        assert!(g.is_overlapping(&empty, 1000.0, 1000.0, txy));
    }

    #[test]
    fn overlap_is_manhattan_below_minimum_spread() {
        let g = grid_at(1000, 1000);
        let base = tref(1000, 1000);
        let grid = g.build_overlap_grid(&[base], txy);
        // dx + dy < 55 overlaps; == 55 does not (strict `<`, `:425`).
        assert!(g.is_overlapping(&grid, 1030.0, 1024.0, txy)); // 54
        assert!(!g.is_overlapping(&grid, 1030.0, 1025.0, txy)); // 55
        assert!(!g.is_overlapping(&grid, 1055.0, 1000.0, txy)); // dx = 55: not < 55
        assert!(g.is_overlapping(&grid, 946.0, 1000.0, txy)); // dx = 54, other cell, neighbour scan
        // outside the grid square → overlapping (`:399-401`).
        assert!(g.is_overlapping(&grid, 1000.0 + 1700.0, 1000.0, txy)); // rel 3200 → cell 58 ≥ 57
        assert!(!g.is_overlapping(&grid, 1000.0 + 1600.0, 1000.0, txy)); // rel 3100 → cell 56: inside
        assert!(g.is_overlapping(&grid, -501.0, 1000.0, txy));
    }

    /// `calculateDeterministicSpeed`'s integer part, values worked by hand from `:472-520`.
    #[test]
    fn mirv_speed_normalisation() {
        // ideal ticks 40/4 = 10 == base: target = 1000 → num = round(1000.4*256)*100 = 256102*100.
        // (25610200 + 500) / 1000 = 25610.7 → 25610 / 256 = 100.0390625
        assert_eq!(mirv_speed_from_lengths(40.0, 4.0, 10.0, 1000.4), 25610.0 / 256.0);
        // long flight: ideal ticks 50 → 5000 scaled; diff = 4000; sqrt = 63.24 → 63*14 = 882;
        // 4000*10/100 = 400 → target = 1000 + 882 + 400 = 2282.
        // actual 2000: round(512000) * 100 = 51200000; (51200000 + 1141) / 2282 = 22436.9 → 22436.
        assert_eq!(mirv_speed_from_lengths(200.0, 4.0, 10.0, 2000.0), 22436.0 / 256.0);
        // short flight: ideal ticks 2 → 200 scaled; diff = 800; sqrt = 28.28 → 28*10 = 280
        // → target = 720. actual 100: 25600*100 = 2560000; (2560000 + 360)/720 = 3556.05 → 3556.
        assert_eq!(mirv_speed_from_lengths(8.0, 4.0, 10.0, 100.0), 3556.0 / 256.0);
        // very short: target clamps to MATH_SCALE (100); tiny length → max(1, ..).
        // ideal 0 → diff 1000 → sqrt 31.6 → 310 → target 690; actual 0 → 0 → max(1, 0) = 1.
        assert_eq!(mirv_speed_from_lengths(0.0, 4.0, 10.0, 0.0), 1.0);
        // base_ticks 1 → scaled 100; ideal 0 → diff 100 → 10*10 = 100 → 0 → clamp 100.
        // actual 10: 2560*100 = 256000; (256000 + 50)/100 = 2560.5 → 2560 → 10.
        assert_eq!(mirv_speed_from_lengths(0.0, 4.0, 1.0, 10.0), 10.0);
        // Math.round half-up at .5: 0.5/256*256 = 0.5 → round → 1 (not banker's / away-from-zero
        // differences matter only for negatives; lengths are positive).
        assert_eq!(js::round(0.5), 1.0);
    }

    /// `if (target)` (`:202`, `:254`) is falsy for TileRef 0: a generated target at tile 0 is
    /// dropped (never staged, never gridded), exactly as TS drops it.
    #[test]
    fn target_tile_zero_is_falsy() {
        let st = State::default();
        let mut st2 = st;
        let mut sc = Scratch::new();
        let cx = Ctx::new(&mut st2, &mut sc, None);
        let mut m = MirvExec::new(SmallId(1), 10);
        m.init = Some(MirvInit { rng: Prng::new(1.0), target_player: SmallId(2), base_x: 0, base_y: 0 });
        m.staged_targets = vec![10];
        m.spatial_grid = vec![-1; 57 * 57 * 2];
        m.push_target(&cx, Some(0));
        m.push_target(&cx, None);
        assert_eq!(m.staged_targets, vec![10]);
        assert!(m.spatial_grid.iter().all(|&v| v == -1));
    }

    /// FILE_MAP §4 P8 design test: "MIRV cancel of detached warheads". A MIRV lists two bound
    /// warheads: one still in `live`, one already compacted out (detached, pinned only by the
    /// MIRV). The MIRV's cancel loop (`:181-187`) queues CancelNuke ×2 then Unpin ×2; after the
    /// driver applies them, both warheads are inactive, the listed one keeps its listing hold,
    /// and the detached one's slot is freed (its last hold was the MIRV's).
    #[test]
    fn mirv_cancel_of_detached_warheads() {
        let mut st = State::default();
        let mut sc = Scratch::new();
        let mut ex = ExecStore::default();
        st.start_tick = Some(0); // past the spawn phase (compaction keeps `isActive()` only)

        let mirv_id = ex.ar.insert(Exec::Mirv(MirvExec::new(SmallId(1), 10)));
        let w_live =
            ex.ar.insert(Exec::Nuke(NukeExec::new(UnitType::MirvWarhead, SmallId(1), 11, Tri::Some(5), 7.0, 2, true)));
        let w_detached =
            ex.ar.insert(Exec::Nuke(NukeExec::new(UnitType::MirvWarhead, SmallId(1), 12, Tri::Some(5), 7.0, 3, true)));
        ex.live.extend([mirv_id, w_live]);
        // both bound to the MIRV (BindWarhead adds one hold each) ...
        ex.ar.hold(w_live);
        ex.ar.hold(w_detached);
        ex.ar.mirv.get_mut(mirv_id.idx).warhead_executions = vec![WarheadRef::Id(w_live), WarheadRef::Id(w_detached)];
        // ... and the detached one already ran to completion and was compacted out of `live`.
        ex.ar.nuke.get_mut(w_detached.idx).active = false;
        ex.ar.release(w_detached); // compaction drops its listing hold
        assert!(ex.ar.is_allocated(w_detached), "pinned by the MIRV");
        assert_eq!((ex.ar.holds(w_live), ex.ar.holds(w_detached)), (2, 1));

        // The MIRV's cancel loop, then the driver applying its queue right after its tick.
        {
            let mut cx = Ctx::new(&mut st, &mut sc, None);
            let m = ex.ar.mirv.get_mut(mirv_id.idx);
            m.active = false;
            m.cancel_warheads(&mut cx);
            assert!(m.warhead_executions.is_empty(), "warheadExecutions = []");
            assert_eq!(
                cx.st.exec_cmds,
                vec![
                    ExecCmd::CancelNuke(WarheadRef::Id(w_live)),
                    ExecCmd::CancelNuke(WarheadRef::Id(w_detached)),
                    ExecCmd::Unpin(WarheadRef::Id(w_live)),
                    ExecCmd::Unpin(WarheadRef::Id(w_detached)),
                ]
            );
            ex.apply_exec_cmds(&mut cx, mirv_id);
            assert!(cx.st.exec_cmds.is_empty());
        }
        assert!(!ex.ar.is_active(w_live), "cancel() sets active = false");
        assert_eq!(ex.ar.holds(w_live), 1, "still listed in live");
        assert!(!ex.ar.is_allocated(w_detached), "detached warhead freed after the MIRV's unpin");
        // compaction then removes the inactive MIRV and the cancelled warhead; no double release.
        ex.remove_inactive_executions(&st);
        assert!(ex.live.is_empty());
        assert!(!ex.ar.is_allocated(mirv_id) && !ex.ar.is_allocated(w_live));
        assert_eq!(mirv_id.kind, ExecKind::Mirv);
    }

    /// The warhead spawn loop (`:283-300`) queues `add_execution` + `BindWarhead { k }` per
    /// staged target, in staged order, with `waitBase + random.nextInt(0, 15)` waits and the
    /// 70/140/210/280 speed steps; the driver then binds each entry to its drained id with a
    /// MIRV hold.
    #[test]
    fn spawned_warheads_bind_in_order() {
        let mut st = State::default();
        let mut sc = Scratch::new();
        let mut ex = ExecStore::default();
        let mut m = MirvExec::new(SmallId(1), 10);
        // the parts of init/launch the loop reads
        m.init = Some(MirvInit { rng: Prng::new(42.0), target_player: SmallId(2), base_x: 0, base_y: 0 });
        m.launch = Some(MirvLaunch { spawn_tile: 1, separate_dst: 77, path_finder: ParabolaPathFinder::default() });
        m.staged_targets = (0..300).map(|i| 1000 + i).collect();
        let mirv_id = ex.ar.insert(Exec::Mirv(m));
        ex.live.push(mirv_id);

        let mut rng = Prng::new(42.0);
        let expect_wait: Vec<i32> = (0..300).map(|_| js::to_int32(2.0 + rng.next_int(0.0, 15.0))).collect();
        {
            let mut cx = Ctx::new(&mut st, &mut sc, None);
            ex.ar.mirv.get_mut(mirv_id.idx).queue_warheads(&mut cx, 2, 6.0);
            assert_eq!(cx.st.exec_inbox.len(), 300);
            ex.after_exec_call(&mut cx, mirv_id);
        }
        let m = ex.ar.mirv.get(mirv_id.idx);
        assert_eq!(st.uninit.len(), 300);
        assert_eq!(m.warhead_executions.len(), 300);
        for (k, w) in m.warhead_executions.iter().enumerate() {
            let WarheadRef::Id(id) = *w else { panic!("unbound warhead {k}") };
            assert_eq!(id, st.uninit[k], "bound to the k-th drained id");
            assert_eq!(ex.ar.holds(id), 2, "listing + MIRV pin");
            let n = ex.ar.nuke.get(id.idx);
            let off = match k {
                0..70 => 0.0,
                70..140 => 1.0,
                140..210 => 2.0,
                210..280 => 3.0,
                _ => 4.0,
            };
            assert_eq!((n.nuke_type, n.player, n.dst), (UnitType::MirvWarhead, SmallId(1), 1000 + k as u32));
            assert_eq!(
                (n.src, n.speed, n.wait_ticks, n.rocket_direction_up),
                (Tri::Some(77), 6.0 + off, expect_wait[k], true)
            );
        }
        assert!(expect_wait.iter().all(|&w| (2..=16).contains(&w)), "nextInt(0, 15) ∈ [0, 14]");
    }
}
