/**
 * P4b golden-vector generator: drives the REAL upstream path code (engine @ OF_ROOT) on
 * test maps and records inputs + outputs for the Rust replay (`src/path/vectors.rs`).
 *
 *   OF_ROOT=<upstream> <ts runner> crates/of-engine/tests/p4b_vectors/gen_p4b_vectors.mts <out.json.zst> <maps dir>
 *
 * Queries come from a seeded mulberry32, so the
 * file is reproducible; the engine commit is recorded in the header.
 */
import fs from "node:fs";
import path from "node:path";
import zlib from "node:zlib";
import { execSync } from "node:child_process";
import { pathToFileURL } from "node:url";

const root = process.env.OF_ROOT;
if (!root) throw new Error("OF_ROOT unset");
const [outFile, mapsDir] = process.argv.slice(2);
if (!outFile || !mapsDir) throw new Error("usage: gen_p4b_vectors.mts <out.json.zst> <maps dir>");
const imp = (p: string) => import(pathToFileURL(path.join(root, p)).href);

const { GameMapImpl } = await imp("packages/engine-lib/src/game/GameMapImpl.ts");
const { DistanceBasedBezierCurve } = await imp("packages/engine/src/utilities/Line.ts");
const { getParabolaControlPoints, ParabolaUniversalPathFinder } = await imp(
  "packages/engine/src/pathfinding/PathFinder.Parabola.ts",
);
const { AirPathFinder } = await imp("packages/engine/src/pathfinding/PathFinder.Air.ts");
const { AStarWater } = await imp("packages/engine/src/pathfinding/algorithms/AStar.Water.ts");
const { AStarWaterBounded } = await imp("packages/engine/src/pathfinding/algorithms/AStar.WaterBounded.ts");
const { AStarRail } = await imp("packages/engine/src/pathfinding/algorithms/AStar.Rail.ts");
const { SmoothingWaterTransformer } = await imp(
  "packages/engine/src/pathfinding/transformers/SmoothingWaterTransformer.ts",
);
const { ShoreCoercingTransformer } = await imp(
  "packages/engine/src/pathfinding/transformers/ShoreCoercingTransformer.ts",
);
const { MiniMapTransformer } = await imp("packages/engine/src/pathfinding/transformers/MiniMapTransformer.ts");
const { PathFinderBuilder } = await imp("packages/engine/src/pathfinding/PathFinderBuilder.ts");
const { WaterPathFinder, PathFinding, WaterPathMemo } = await imp("packages/engine/src/pathfinding/PathFinder.ts");
const { SpatialQuery } = await imp("packages/engine/src/pathfinding/spatial/SpatialQuery.ts");

// ---------------------------------------------------------------- helpers
function mulberry32(seed: number) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}
const rnd = mulberry32(0x5eed4b);
const ri = (n: number) => Math.floor(rnd() * n);
const pick = <T>(a: T[]): T => a[ri(a.length)];
const arr = (p: ArrayLike<number> | null) => (p === null ? null : Array.from(p));
function attempt<T>(f: () => T): T | { throw: string } {
  try {
    return f();
  } catch (e) {
    return { throw: (e as Error).message };
  }
}

type MapPair = { name: string; map: any; mini: any };
function loadMap(name: string): MapPair {
  const dir = path.join(mapsDir, name);
  const man = JSON.parse(fs.readFileSync(path.join(dir, "manifest.json"), "utf8"));
  const bin = (f: string) => new Uint8Array(fs.readFileSync(path.join(dir, f)));
  return {
    name,
    map: new GameMapImpl(man.map.width, man.map.height, bin("map.bin"), man.map.num_land_tiles),
    mini: new GameMapImpl(man.map4x.width, man.map4x.height, bin("map4x.bin"), man.map4x.num_land_tiles),
  };
}
const maps: MapPair[] = ["ocean_and_land", "half_land_half_ocean", "world"].map(loadMap);
const world = maps[2];

function tilesWhere(m: any, pred: (t: number) => boolean): number[] {
  const out: number[] = [];
  for (let t = 0; t < m.width() * m.height(); t++) if (pred(t)) out.push(t);
  return out;
}
const waterOf = (m: any) => tilesWhere(m, (t) => m.isWater(t));
const shoreOf = (m: any) => tilesWhere(m, (t) => m.isShore(t));
const landOf = (m: any) => tilesWhere(m, (t) => m.isLand(t));
/** A tile near `t` (within `r` in x and y), clamped to the map. */
function near(m: any, t: number, r: number): number {
  const x = Math.min(m.width() - 1, Math.max(0, m.x(t) + ri(2 * r + 1) - r));
  const y = Math.min(m.height() - 1, Math.max(0, m.y(t) + ri(2 * r + 1) - r));
  return m.ref(x, y);
}
function nearIn(m: any, pool: number[], t: number, r: number): number {
  const c = pool.filter((p) => Math.abs(m.x(p) - m.x(t)) <= r && Math.abs(m.y(p) - m.y(t)) <= r);
  return c.length > 0 ? pick(c) : pick(pool);
}

const cases: Record<string, unknown[]> = {};

// ---------------------------------------------------------------- Line (DistanceBasedBezierCurve)
{
  const out: unknown[] = [];
  // Dyadic inputs (k/1024): exact in decimal, so the JSON round-trip is exact too.
  const coords = () => {
    const v = rnd() * 400 - 60;
    const k = ri(4);
    return k === 0 ? Math.round(v) : k === 1 ? Math.round(v * 2) / 2 : Math.round(v * 1024) / 1024;
  };
  for (let i = 0; i < 300; i++) {
    const pts = [0, 1, 2, 3].map(() => ({ x: coords(), y: coords() }));
    if (i % 25 === 0) pts[3] = { ...pts[0] }; // closed curve
    const increment = pick([1, 2, 3, 4, 5.5, 6, 10, 0.3, 2.501]);
    const c = new DistanceBasedBezierCurve(pts[0], pts[1], pts[2], pts[3], increment);
    const steps: unknown[] = [];
    for (let s = 0; s < 40; s++) {
      const d = pick([0.1, 1, 2.5, 4, 7, 0.002]);
      const p = c.increment(d);
      const st = c.getState();
      steps.push([d, p === null ? null : [p.x, p.y], st.currentIndex, st.accumulatedDistanceScaled]);
    }
    out.push({
      pts: pts.map((p) => [p.x, p.y]),
      increment,
      cached: c.getAllPoints().flatMap((p: any) => [p.x, p.y]),
      // accumDist (an integer) = getLength * 256 exactly
      length256: DistanceBasedBezierCurve.getLength(pts[0], pts[1], pts[2], pts[3]) * 256,
      steps,
    });
  }
  cases.line = out;
}

// ---------------------------------------------------------------- Parabola
{
  const pm = new GameMapImpl(200, 150, new Uint8Array(200 * 150).fill(0x20), 0);
  const out: unknown[] = [];
  for (let i = 0; i < 200; i++) {
    const from = pm.ref(ri(200), ri(150));
    const to = i % 20 === 0 ? from : pm.ref(ri(200), ri(150));
    const options: any = {};
    if (rnd() < 0.5) options.increment = pick([1, 2, 3, 4, 6, 10, 2.5]);
    if (rnd() < 0.4) options.distanceBasedHeight = rnd() < 0.5;
    if (rnd() < 0.4) options.directionUp = rnd() < 0.5;
    if (rnd() < 0.3) options.ignoreMapBounds = rnd() < 0.5;
    const opt = Object.keys(options).length === 0 && rnd() < 0.5 ? undefined : options;
    const cp = getParabolaControlPoints(pm, from, to, opt).map((p: any) => [p.x, p.y]);
    const fp = attempt(() => new ParabolaUniversalPathFinder(pm, opt).findPath(from, to));
    const speed = pick([undefined, 1, 2.5, 4, 8, 12]);
    const f = new ParabolaUniversalPathFinder(pm, opt);
    const steps: unknown[] = [];
    let cur = from;
    for (let s = 0; s < 400; s++) {
      const r = attempt(() => f.next(cur, to, speed));
      if ("throw" in (r as any)) {
        steps.push(r);
        break;
      }
      const rr = r as any;
      steps.push([rr.status, rr.node ?? null, f.currentIndex()]);
      if (rr.status !== 0) break;
      cur = rr.node;
    }
    out.push({ from, to, options: opt ?? null, cp, find: fp, speed: speed ?? null, steps });
  }
  cases.parabola = { map: [200, 150], cases: out } as any;
}

// ---------------------------------------------------------------- AirPathFinder
{
  const am = new GameMapImpl(64, 64, new Uint8Array(64 * 64).fill(0x20), 0);
  const out: unknown[] = [];
  for (let i = 0; i < 200; i++) {
    const seed = pick([0, 1, ri(5000), ri(100000), 2147483647, -5]);
    const game: any = { ticks: () => seed, x: (t: number) => am.x(t), y: (t: number) => am.y(t), ref: (x: number, y: number) => am.ref(x, y) };
    const from = am.ref(ri(64), ri(64));
    const to = i % 25 === 0 ? from : am.ref(ri(64), ri(64));
    out.push({ seed, from, to, path: new AirPathFinder(game).findPath(from, to) });
  }
  cases.air = { map: [64, 64], cases: out } as any;
}

// ---------------------------------------------------------------- AStarWater (mini maps)
{
  const out: unknown[] = [];
  for (const mp of maps) {
    const mini = mp.mini;
    const a = new AStarWater(mini); // one instance per chain, reused across queries (as TS)
    const water = waterOf(mini);
    const shore = shoreOf(mini);
    const n = mp === world ? 60 : 40;
    for (let i = 0; i < n; i++) {
      const s = pick(water);
      const g = i % 7 === 3 && shore.length > 0 ? pick(shore) : mp === world && i % 2 ? nearIn(mini, water, s, 80) : pick(water);
      const start = i % 5 === 4 ? [s, pick(water), pick(water)] : s;
      out.push({ map: mp.name, start, goal: g, path: arr(a.findPath(start, g)) });
    }
  }
  cases.astar_water = out;
}

// ---------------------------------------------------------------- AStarWaterBounded (main maps)
{
  const out: unknown[] = [];
  for (const mp of maps) {
    const m = mp.map;
    for (const cap of [10000, 400]) {
      const b = new AStarWaterBounded(m, cap);
      const water = waterOf(m);
      for (let i = 0; i < (mp === world ? 50 : 20); i++) {
        const g = pick(water);
        const s = nearIn(m, water, g, mp === world ? 40 : 8);
        const kind = i % 4;
        if (kind === 0) {
          out.push({ map: mp.name, cap, op: "find", start: [s], goal: g, path: arr(b.findPath(s, g)) });
        } else {
          const pad = ri(15);
          const xs = [m.x(s), m.x(g)];
          const ys = [m.y(s), m.y(g)];
          const bounds = {
            minX: Math.max(0, Math.min(...xs) - pad),
            maxX: Math.min(m.width() - 1, Math.max(...xs) + pad),
            minY: Math.max(0, Math.min(...ys) - pad),
            maxY: Math.min(m.height() - 1, Math.max(...ys) + pad),
          };
          if (kind === 3) bounds.maxX = Math.max(bounds.minX, bounds.maxX - pad - 2); // goal may be clamped
          const starts = kind === 2 ? [s, nearIn(m, water, g, 10)] : [s];
          out.push({
            map: mp.name,
            cap,
            op: "bounded",
            start: starts,
            goal: g,
            bounds: [bounds.minX, bounds.maxX, bounds.minY, bounds.maxY],
            path: arr(b.searchBounded(starts.length === 1 ? starts[0] : starts, g, bounds)),
          });
        }
      }
    }
  }
  cases.bounded = out;
}

// ---------------------------------------------------------------- AStarRail (mini maps)
{
  const out: unknown[] = [];
  for (const mp of maps) {
    const mini = mp.mini;
    const r = new AStarRail(mini);
    const land = landOf(mini);
    if (land.length === 0) continue;
    for (let i = 0; i < (mp === world ? 50 : 20); i++) {
      const s = pick(land);
      const g = mp === world ? nearIn(mini, land, s, 60) : pick(land);
      const start = i % 6 === 5 ? [s, pick(land)] : s;
      out.push({ map: mp.name, start, goal: g, path: arr(r.findPath(start, g)) });
    }
  }
  cases.rail = out;
}

// ---------------------------------------------------------------- transformers and chains
const chainOf = (mp: MapPair) =>
  PathFinderBuilder.create(new AStarWater(mp.mini))
    .wrap((pf: any) => new ShoreCoercingTransformer(pf, mp.mini))
    .wrap((pf: any) => new MiniMapTransformer(pf, mp.map, mp.mini))
    .build();
{
  // Smoothing over AStarWater on the minimap (the HPA chain's smoother, fed by a simple core).
  const out: unknown[] = [];
  const mp = world;
  const water = waterOf(mp.mini);
  const sm = new SmoothingWaterTransformer(new AStarWater(mp.mini), mp.mini);
  for (let i = 0; i < 40; i++) {
    const s = pick(water);
    const g = nearIn(mp.mini, water, s, i % 2 ? 150 : 40);
    const raw = new AStarWater(mp.mini).findPath(s, g);
    out.push({ map: mp.name, from: s, to: g, raw: arr(raw), smoothed: arr(sm.findPath(s, g)) });
  }
  cases.smoothing = out;
}
{
  const out: unknown[] = [];
  for (const mp of maps) {
    const sh = new ShoreCoercingTransformer(new AStarWater(mp.mini), mp.mini);
    const shore = shoreOf(mp.mini);
    const water = waterOf(mp.mini);
    for (let i = 0; i < (mp === world ? 40 : 15); i++) {
      const a = pick(shore);
      const b = mp === world ? nearIn(mp.mini, shore, a, 60) : pick(shore);
      const from = i % 4 === 3 ? [a, pick(water), pick(shore)] : a;
      out.push({ map: mp.name, from, to: b, path: arr(sh.findPath(from, b)) });
    }
  }
  cases.shore = out;
}
{
  const out: unknown[] = [];
  for (const mp of maps) {
    const chain = chainOf(mp);
    const shore = shoreOf(mp.map);
    const water = waterOf(mp.map);
    for (let i = 0; i < (mp === world ? 50 : 20); i++) {
      const a = i % 2 ? pick(shore) : pick(water);
      const pool = i % 3 ? water : shore;
      const b = mp === world ? nearIn(mp.map, pool, a, 200) : pick(pool);
      const from = i % 5 === 4 ? [a, pick(shore), pick(water)] : a;
      out.push({ map: mp.name, from, to: b, path: arr(attempt(() => chain.findPath(from, b)) as any) });
    }
  }
  cases.simple_chain = out;
}
{
  const out: unknown[] = [];
  for (const mp of maps) {
    const chain = PathFinderBuilder.create(new AStarRail(mp.mini))
      .wrap((pf: any) => new MiniMapTransformer(pf, mp.map, mp.mini))
      .build();
    const land = landOf(mp.map);
    if (land.length === 0) continue;
    for (let i = 0; i < (mp === world ? 40 : 15); i++) {
      const a = pick(land);
      const b = mp === world ? nearIn(mp.map, land, a, 120) : pick(land);
      out.push({ map: mp.name, from: a, to: b, path: arr(chain.findPath(a, b)) });
    }
  }
  cases.rail_chain = out;
}

// ---------------------------------------------------------------- WaterPathMemo (shrunk budget)
{
  const out: unknown[] = [];
  const mp = world;
  const chain = chainOf(mp);
  const water = waterOf(mp.map);
  let wv = 0;
  const memo = new WaterPathMemo(chain, mp.map.width() * mp.map.height(), () => wv, 6000);
  const pool = Array.from({ length: 12 }, () => pick(water));
  for (let i = 0; i < 80; i++) {
    if (i === 50) wv++;
    const a = pick(pool);
    const b = nearIn(mp.map, pool, a, 300);
    const from = i % 9 === 8 ? [a] : a;
    const p = memo.findPath(from, b);
    out.push({
      wv,
      from,
      to: b,
      path: arr(p),
      entries: memo.entryCount,
      bytes: memo.byteCount,
      keys: Array.from((memo as any).paths.keys()),
    });
  }
  cases.memo = { max_bytes: 6000, cases: out } as any;
}

// ---------------------------------------------------------------- WaterPathFinder (stagger, memo, versions)
{
  const out: unknown[] = [];
  const mp = world;
  const g: any = {
    v: 0,
    waterGraphVersion() {
      return this.v;
    },
    miniWaterHPA: () => null,
    miniWaterGraph: () => null,
    miniMap: () => mp.mini,
    map: () => mp.map,
    isValidRef: (r: number) => mp.map.isValidRef(r),
    manhattanDist: (a: number, b: number) => mp.map.manhattanDist(a, b),
  };
  const water = waterOf(mp.map);
  const ports = Array.from({ length: 6 }, () => pick(water));
  const finders = [new WaterPathFinder(g, 3, true), new WaterPathFinder(g, 0, false), new WaterPathFinder(g, 2, false)];
  const pos = finders.map(() => pick(ports));
  const dst = finders.map(() => pick(ports));
  const ops: unknown[] = [];
  for (let i = 0; i < 260; i++) {
    const k = ri(finders.length);
    const f = finders[k];
    const roll = rnd();
    let rec: unknown;
    if (i % 37 === 36) {
      g.v++;
      rec = { op: "bump_graph", v: g.v };
    } else if (i % 61 === 60) {
      (mp.map as any).waterVersion_++;
      rec = { op: "bump_water", wv: mp.map.waterVersion() };
    } else if (roll < 0.6) {
      const dist = pick([undefined, undefined, 0, 3, 10]);
      const r = f.next(pos[k], dst[k], dist);
      rec = { op: "next", k, from: pos[k], to: dst[k], dist: dist ?? null, status: r.status, node: (r as any).node ?? null };
      if (r.status === 0) pos[k] = (r as any).node;
      else if (r.status === 2) {
        pos[k] = dst[k];
        dst[k] = pick(ports);
      } else dst[k] = pick(ports);
      if (rnd() < 0.05) pos[k] = pick(ports); // teleport off-path
    } else if (roll < 0.8) {
      const to = pick(ports);
      rec = { op: "find", k, from: pos[k], to, path: arr(f.findPath(pos[k], to)) };
    } else if (roll < 0.95) {
      rec = { op: "traversal", k, from: pos[k], to: dst[k], path: arr(f.pathForTraversal(pos[k], dst[k])) };
    } else {
      rec = { op: "rebuilt", k, v: f.rebuilt };
    }
    const st = f.getState();
    const memo = k === 0 ? (f as any).stepper.innerFinder() : null;
    ops.push({
      rec,
      state: [st.waterGraphVersion, st.rebuilt, st.staggerCountdown, st.pendingVersion, st.stepper.pathIndex, st.stepper.lastTo, st.stepper.path?.length ?? null],
      memo: memo ? [memo.entryCount, memo.byteCount] : null,
    });
  }
  out.push({ map: mp.name, finders: [[3, true], [0, false], [2, false]], ops });
  cases.water_pf = out;
}

// ---------------------------------------------------------------- PathFinding.Rail / .Air steppers
{
  const out: unknown[] = [];
  const mp = world;
  const g: any = {
    ticks: () => 777,
    x: (t: number) => mp.map.x(t),
    y: (t: number) => mp.map.y(t),
    ref: (x: number, y: number) => mp.map.ref(x, y),
    miniMap: () => mp.mini,
    map: () => mp.map,
    isValidRef: (r: number) => mp.map.isValidRef(r),
    manhattanDist: (a: number, b: number) => mp.map.manhattanDist(a, b),
  };
  const land = landOf(mp.map);
  for (const kind of ["rail", "air"]) {
    for (let c = 0; c < 10; c++) {
      const s = kind === "rail" ? PathFinding.Rail(g) : PathFinding.Air(g);
      let cur = pick(land);
      const from = cur;
      const to = nearIn(mp.map, land, cur, 80);
      const steps: unknown[] = [];
      for (let i = 0; i < 300; i++) {
        const r = s.next(cur, to);
        steps.push([r.status, (r as any).node ?? null]);
        if (r.status !== 0) break;
        cur = (r as any).node;
      }
      out.push({ kind, from, to, steps });
    }
  }
  cases.steppers = out;
}

// ---------------------------------------------------------------- SpatialQuery: closestShore + refineStartTile
{
  // Synthetic 96×64 map with land blobs and owners.
  const W = 96,
    H = 64;
  const terrain = new Uint8Array(W * H).fill(0x20);
  for (let b = 0; b < 14; b++) {
    const cx = ri(W),
      cy = ri(H),
      r = 3 + ri(9);
    for (let y = 0; y < H; y++)
      for (let x = 0; x < W; x++) if ((x - cx) ** 2 + (y - cy) ** 2 <= r * r) terrain[y * W + x] = 0x80;
  }
  for (let y = 0; y < H; y++)
    for (let x = 0; x < W; x++) {
      const i = y * W + x;
      const land = (terrain[i] & 0x80) !== 0;
      for (const [nx, ny] of [[x - 1, y], [x + 1, y], [x, y - 1], [x, y + 1]]) {
        if (nx < 0 || nx >= W || ny < 0 || ny >= H) continue;
        if (((terrain[ny * W + nx] & 0x80) !== 0) !== land) {
          terrain[i] |= 0x40;
          break;
        }
      }
    }
  const sm = new GameMapImpl(W, H, terrain, 0);
  const owners: number[] = [];
  for (let t = 0; t < W * H; t++) {
    const o = sm.isLand(t) ? ((sm.x(t) >> 4) + (sm.y(t) >> 4)) % 4 : 0;
    owners.push(o);
    if (o !== 0) sm.setOwnerID(t, o);
  }
  const g: any = {
    map: () => sm,
    width: () => W,
    height: () => H,
    isShore: (t: number) => sm.isShore(t),
    isLand: (t: number) => sm.isLand(t),
    ownerID: (t: number) => sm.ownerID(t),
  };
  const q = new SpatialQuery(g);
  const out: unknown[] = [];
  for (let i = 0; i < 150; i++) {
    const owner = ri(4);
    const tile = ri(W * H);
    const maxDist = pick([50, 0, 3, 10, 200]);
    out.push({ owner, tile, maxDist, res: q.closestShore({ smallID: () => owner }, tile, maxDist) });
  }
  // refineStartTile on the world main map, through the private method.
  const wg: any = { map: () => world.map };
  const wq = new SpatialQuery(wg);
  const chain = chainOf(world);
  const wshore = shoreOf(world.map);
  const wwater = waterOf(world.map);
  const refine: unknown[] = [];
  for (let i = 0; i < 40; i++) {
    const a = pick(wshore);
    const shores = wshore.filter((s) => Math.abs(world.map.x(s) - world.map.x(a)) <= 12 && Math.abs(world.map.y(s) - world.map.y(a)) <= 12);
    const target = nearIn(world.map, wwater, a, 250);
    const p = chain.findPath(shores, target);
    if (p === null || p.length === 0) continue;
    refine.push({ shores, target, path: Array.from(p), res: (wq as any).refineStartTile(p, shores, wg) });
  }
  cases.spatial = { map: [W, H], terrain: Array.from(terrain), owners, closest: out, refine } as any;
}

// ---------------------------------------------------------------- write
const commit = (() => {
  try {
    return execSync("git rev-parse HEAD", { cwd: root }).toString().trim();
  } catch {
    return process.env.OF_ENGINE_COMMIT ?? "unknown";
  }
})();
const doc = {
  format: "p4b-path-vectors/1",
  engine_commit: commit,
  generator: "crates/of-engine/tests/p4b_vectors/gen_p4b_vectors.mts",
  maps: maps.map((m) => ({ name: m.name, w: m.map.width(), h: m.map.height(), mw: m.mini.width(), mh: m.mini.height() })),
  cases,
};
const json = Buffer.from(JSON.stringify(doc));
fs.writeFileSync(outFile, zlib.zstdCompressSync(json, { params: { [zlib.constants.ZSTD_c_compressionLevel]: 19 } }));
console.log(`wrote ${outFile}: ${json.length} bytes json, ${Object.entries(cases).map(([k, v]) => `${k}=${Array.isArray(v) ? v.length : "obj"}`).join(" ")}`);
