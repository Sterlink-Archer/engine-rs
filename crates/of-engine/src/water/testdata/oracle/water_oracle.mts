/**
 * P4a golden oracle (MOD-CC + MOD-PATH-WATER-lite + the WaterManager nuke sequence), produced by
 * the REAL engine at the pin. Run it with a TypeScript runner against an upstream checkout:
 *
 *   OF_ROOT=<upstream checkout> <ts runner> water_oracle.mts water_oracle.json
 *   zstd -19 --rm water_oracle.json
 *
 * The Rust side (`water/tests.rs`, `oracle_*`) replays every recorded call on the same map bins
 * and compares. Hashes are FNV-1a-32 over u32 words (`fnv` below; Rust `fnv32`).
 */
import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
import { pathToFileURL } from "node:url";

const root = process.env.OF_ROOT!;
const out = process.argv[2];
const imp = (p: string) => import(pathToFileURL(path.join(root, p)).href);
const { GameMapImpl } = await imp("packages/engine-lib/src/game/GameMapImpl.ts");
const { ConnectedComponents } = await imp("packages/engine/src/pathfinding/algorithms/ConnectedComponents.ts");
const { AbstractGraphBuilder } = await imp("packages/engine/src/pathfinding/algorithms/AbstractGraph.ts");
const { AStarWaterHierarchical } = await imp("packages/engine/src/pathfinding/algorithms/AStar.WaterHierarchical.ts");
const { BFSGrid } = await imp("packages/engine/src/pathfinding/algorithms/BFS.Grid.ts");
const { WaterManager } = await imp("packages/engine/src/game/WaterManager.ts");

const SRC = [
  "packages/engine/src/game/WaterManager.ts",
  "packages/engine/src/pathfinding/algorithms/AbstractGraph.ts",
  "packages/engine/src/pathfinding/algorithms/AStar.AbstractGraph.ts",
  "packages/engine/src/pathfinding/algorithms/AStar.WaterHierarchical.ts",
  "packages/engine/src/pathfinding/algorithms/AStar.WaterBounded.ts",
  "packages/engine/src/pathfinding/algorithms/BFS.Grid.ts",
  "packages/engine/src/pathfinding/algorithms/ConnectedComponents.ts",
  "packages/engine/src/pathfinding/algorithms/PriorityQueue.ts",
  "packages/engine-lib/src/game/GameMapImpl.ts",
];
const sources = Object.fromEntries(
  SRC.map((p) => [p, crypto.createHash("sha256").update(fs.readFileSync(path.join(root, p))).digest("hex")]),
);

function fnv(a: ArrayLike<number>): string {
  let h = 0x811c9dc5;
  for (let i = 0; i < a.length; i++) {
    h ^= a[i] >>> 0;
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return h.toString(16).padStart(8, "0");
}

// mulberry32 (query generation only; the queries are recorded, Rust never re-derives them)
function rng(seed: number) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function loadMap(name: string, bin: string, key: "map" | "map4x" | "map16x") {
  const dir = path.join(root, "tests/testdata/maps", name);
  const man = JSON.parse(fs.readFileSync(path.join(dir, "manifest.json"), "utf8"));
  const data = new Uint8Array(fs.readFileSync(path.join(dir, bin)));
  const md = man[key];
  return new GameMapImpl(md.width, md.height, data, md.num_land_tiles);
}

function ccSummary(cc: any) {
  const s = cc.snapshot();
  const ids = s.componentIds as Uint8Array | Uint16Array;
  return {
    u16: ids instanceof Uint16Array,
    idsHash: fnv(ids),
    sizes: s.componentSizes.map((x: number | undefined) => (x === undefined ? -1 : x)),
    parents: s.parents,
    maxId: s.maxId,
    landMarker: s.landMarker,
  };
}

function graphDump(g: any) {
  const nodes: number[] = [];
  for (const n of g.getAllNodes()) nodes.push(n.x, n.y, n.tile, n.componentId);
  const edges: number[] = [];
  for (const e of g.getAllEdges()) edges.push(e.id, e.nodeA, e.nodeB, e.cost, e.clusterX, e.clusterY);
  const clusters: number[][] = [];
  for (let cy = 0; cy < g.clustersY; cy++)
    for (let cx = 0; cx < g.clustersX; cx++) clusters.push([...(g.getCluster(cx, cy)?.nodeIds ?? [])]);
  return { clustersX: g.clustersX, clustersY: g.clustersY, nodes, edges, clustersHash: fnv(clusters.flatMap((c) => [c.length, ...c])) };
}

function graphHash(g: any) {
  const d = graphDump(g);
  return { nodes: d.nodes.length / 4, edges: d.edges.length / 6, nodesHash: fnv(d.nodes), edgesHash: fnv(d.edges), clustersHash: d.clustersHash };
}

function cacheFilled(g: any): number[] {
  const pc = g._pathCache as (number[] | null)[];
  const out: number[] = [];
  for (let i = 0; i < pc.length; i++) if (pc[i]) out.push(i);
  return out;
}

function waterTiles(m: any): number[] {
  const t: number[] = [];
  for (let i = 0; i < m.width() * m.height(); i++) if (m.isWater(i)) t.push(i);
  return t;
}

/** Query list: 1/4 multi-source (2-4 sources), 1/4 near targets (|d| <= 20). */
function makeQueries(m: any, n: number, seed: number) {
  const r = rng(seed);
  const water = waterTiles(m);
  const pick = () => water[Math.floor(r() * water.length)];
  const qs: { from: number[]; to: number }[] = [];
  for (let i = 0; i < n; i++) {
    const k = i % 4 === 3 ? 2 + Math.floor(r() * 3) : 1;
    const from = Array.from({ length: k }, pick);
    let to = pick();
    if (i % 4 === 1) {
      const x = m.x(from[0]) + Math.floor(r() * 41) - 20;
      const y = m.y(from[0]) + Math.floor(r() * 41) - 20;
      if (m.isValidCoord(x, y) && m.isWater(m.ref(x, y))) to = m.ref(x, y);
    }
    qs.push({ from, to });
  }
  return qs;
}

function runQueries(hpa: any, qs: { from: number[]; to: number }[]) {
  return qs.map((q, i) => {
    const p = hpa.findPath(q.from.length === 1 ? q.from[0] : q.from, q.to) as number[] | null;
    return p === null ? { len: -1 } : { len: p.length, hash: fnv(p), ...(i < 10 ? { path: p } : {}) };
  });
}

const result: any = { sources, cc: {}, graph: {}, hpa: {}, nuke: {} };

// 1. MOD-CC
for (const [name, bin, key] of [
  ["world", "map4x.bin", "map4x"],
  ["world", "map16x.bin", "map16x"],
  ["giantworldmap", "map16x.bin", "map16x"],
  ["half_land_half_ocean", "map4x.bin", "map4x"],
  ["ocean_and_land", "map4x.bin", "map4x"],
] as const) {
  const m = loadMap(name, bin, key);
  const cc = new ConnectedComponents(m);
  cc.initialize();
  result.cc[`${name}/${bin}`] = ccSummary(cc);
}

// 2. graph + 3. HPA on world map4x (the Normal-size world minimap)
{
  const m = loadMap("world", "map4x.bin", "map4x");
  const cc = new ConnectedComponents(m);
  cc.initialize();
  const g = new AbstractGraphBuilder(m, AbstractGraphBuilder.CLUSTER_SIZE, undefined, undefined, cc, new BFSGrid(m.width() * m.height())).build();
  result.graph["world/map4x.bin"] = graphDump(g);
  const hpa = new AStarWaterHierarchical(m, g, { cachePaths: true });
  const qs = makeQueries(m, 400, 0x5eed);
  result.hpa["world/map4x.bin"] = { queries: qs, results: runQueries(hpa, qs), cacheFilled: cacheFilled(g) };
}

// 4. WaterManager nuke sequence on the Compact world (main = map4x, mini = map16x)
{
  const map = loadMap("world", "map4x.bin", "map4x");
  const mini = loadMap("world", "map16x.bin", "map16x");
  const wm = new WaterManager(map, mini, false);
  const r = rng(0xbeef);
  const coastal: number[] = [];
  const inland: number[] = [];
  for (let t = 0; t < map.width() * map.height(); t++) {
    if (!map.isLand(t) || map.isImpassable(t)) continue;
    const hasWaterNb = map.neighbors(t).some((n: number) => map.isWater(n));
    (hasWaterNb ? coastal : inland).push(t);
  }
  const nukeCircle = (cx: number, cy: number, rad: number) => {
    const r2 = rad * rad;
    for (let y = Math.max(0, cy - rad); y <= Math.min(map.height() - 1, cy + rad); y++)
      for (let x = Math.max(0, cx - rad); x <= Math.min(map.width() - 1, cx + rad); x++) {
        const dx = x - cx, dy = y - cy;
        if (dx * dx + dy * dy > r2) continue;
        const t = map.ref(x, y);
        if (map.isLand(t) && !map.hasOwner(t) && !map.isImpassable(t)) wm.queueTile(t);
      }
  };
  let tick = 0;
  const phases: any[] = [];
  const initialGraph = graphHash(wm.miniWaterGraph()); // before any query or nuke
  const qsBase = makeQueries(mini, 120, 0x77);
  const hpaInitial = runQueries(wm.miniWaterHPA(), qsBase);
  for (let p = 0; p < 6; p++) {
    const nukes: number[][] = [];
    const count = 1 + Math.floor(r() * 3);
    for (let k = 0; k < count; k++) {
      const src = p % 3 === 2 ? inland : coastal;
      const t = src[Math.floor(r() * src.length)];
      nukes.push([map.x(t), map.y(t), 4 + Math.floor(r() * 14)]);
    }
    for (const [x, y, rad] of nukes) nukeCircle(x, y, rad);
    const v0 = wm.waterGraphVersion();
    const changed = wm.tick(tick++);
    const afterFlush = {
      changedLen: changed.length,
      changedHash: fnv(changed),
      changedHead: changed.slice(0, 40),
      mainHash: fnv((map as any).terrain),
      miniHash: fnv((mini as any).terrain),
      mainWaterVersion: map.waterVersion(),
      miniWaterVersion: mini.waterVersion(),
      dirtyMini: [...(wm as any)._dirtyMiniTiles],
      cc: ccSummary((wm as any)._miniWaterCC),
    };
    // stale-graph window: queries route on the OLD graph over the NEW terrain (Q-WATERLAG)
    const stale = runQueries(wm.miniWaterHPA(), qsBase);
    const staleCache = cacheFilled(wm.miniWaterGraph());
    let ticks = 0;
    while (wm.waterGraphVersion() === v0 && ticks < 40) {
      wm.tick(tick++);
      ticks++;
    }
    const after = runQueries(wm.miniWaterHPA(), qsBase);
    phases.push({
      nukes,
      afterFlush,
      stale,
      staleCache,
      rebuildTicks: ticks,
      version: wm.waterGraphVersion(),
      lastRebuildTick: (wm as any)._waterGraphLastRebuildTick,
      graph: graphHash(wm.miniWaterGraph()),
      after,
      afterCache: cacheFilled(wm.miniWaterGraph()),
    });
  }
  result.nuke["world-compact"] = { queries: qsBase, initial: hpaInitial, initialGraph, phases };
}

fs.writeFileSync(out, JSON.stringify(result));
console.log(`wrote ${out}`);
