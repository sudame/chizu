// 道路を引くときの、既存の道路への吸い付きと、つながりの判定。DOM に触らないので Vitest で確かめられる。

import { NODE_STRIDE, SEGMENT_STRIDE } from "../protocol/messages.ts";
import { HIGHWAY_CODE } from "./lanes.ts";
import type { Tile } from "./roads.ts";
import { MAX_ROAD_TILES, snapToEightDirections } from "./tools.ts";

/** 吸い付く範囲（マス）。縦横斜めに 1 マスまでである。 */
export const SNAP_RADIUS = 1;

/**
 * 道路のマスの種類。node は区間の端（つなげるところ）、road は区間の途中。
 * 高速道路の途中と、外の世界につながる外側の端は、中核が道路をつなぐことを断るので、
 * 吸い付き先にもつながりにも数えない。
 */
type RoadTileKind = "node" | "road" | "highway" | "outside";

const RANK: Record<RoadTileKind, number> = { highway: 1, road: 1, node: 2, outside: 3 };

type SegmentLine = { ax: number; ay: number; bx: number; by: number; highway: boolean };

/** 道路が通るマスの索引。 */
export type RoadIndex = {
  readonly tiles: ReadonlyMap<string, RoadTileKind>;
  readonly size: number;
  readonly segments: readonly SegmentLine[];
  /** マスを通る区間の番号。 */
  readonly through: ReadonlyMap<string, readonly number[]>;
};

const key = (x: number, y: number) => `${x},${y}`;

export const EMPTY_ROAD_INDEX: RoadIndex = {
  tiles: new Map(),
  size: 0,
  segments: [],
  through: new Map(),
};

/** マス t が、区間の直線の上にあるか。 */
function onLine(s: SegmentLine, t: Tile): boolean {
  const dx = s.bx - s.ax;
  const dy = s.by - s.ay;
  const len = Math.max(Math.abs(dx), Math.abs(dy));
  const k = Math.max(Math.abs(t.x - s.ax), Math.abs(t.y - s.ay));
  return k <= len && t.x === s.ax + Math.sign(dx) * k && t.y === s.ay + Math.sign(dy) * k;
}

/**
 * Worker から届いた区間とノードから、道路が通るマスの索引を作る。
 * ノードの 4 つ目の数は、外の世界につながる外側の端なら 1 である。
 */
export function buildRoadIndex(segments: Float64Array, nodes: Float64Array): RoadIndex {
  const tiles = new Map<string, RoadTileKind>();
  const lines: SegmentLine[] = [];
  const through = new Map<string, number[]>();
  const mark = (x: number, y: number, kind: RoadTileKind) => {
    const k = key(x, y);
    const old = tiles.get(k);
    if (old === undefined || RANK[kind] > RANK[old]) {
      tiles.set(k, kind);
    }
  };
  for (let i = 0; i + SEGMENT_STRIDE <= segments.length; i += SEGMENT_STRIDE) {
    const [ax = 0, ay = 0, bx = 0, by = 0, kind = 0] = segments.subarray(i, i + SEGMENT_STRIDE);
    const dx = bx - ax;
    const dy = by - ay;
    const len = Math.max(Math.abs(dx), Math.abs(dy));
    const straight = dx === 0 || dy === 0 || Math.abs(dx) === Math.abs(dy);
    const highway = kind === HIGHWAY_CODE;
    const inner: RoadTileKind = highway ? "highway" : "road";
    const id = lines.length;
    lines.push({ ax, ay, bx, by, highway });
    const pass = (x: number, y: number) => {
      const k = key(x, y);
      const list = through.get(k);
      if (list === undefined) {
        through.set(k, [id]);
      } else {
        list.push(id);
      }
    };
    if (straight) {
      for (let k = 1; k < len; k++) {
        mark(ax + Math.sign(dx) * k, ay + Math.sign(dy) * k, inner);
        pass(ax + Math.sign(dx) * k, ay + Math.sign(dy) * k);
      }
    }
    // 高速道路の端がどちらの端かは、ノードから分かる
    mark(ax, ay, highway ? "highway" : "node");
    mark(bx, by, highway ? "highway" : "node");
    pass(ax, ay);
    pass(bx, by);
  }
  for (let i = 0; i + NODE_STRIDE <= nodes.length; i += NODE_STRIDE) {
    mark(nodes[i] ?? 0, nodes[i + 1] ?? 0, nodes[i + 3] === 1 ? "outside" : "node");
  }
  return { tiles, size: tiles.size, segments: lines, through };
}

/**
 * 中核の check_road が断る道路か（高速道路の途中を通る、既存の道路と同じ歩みを共有する、
 * 斜めの角で既存の斜めの道路と交わる）。マスが建設できる土地かは見ない。
 */
export function isRefused(index: RoadIndex, from: Tile, to: Tile): boolean {
  const lines = index.segments;
  const through = (t: Tile) => index.through.get(key(t.x, t.y)) ?? [];
  const sharesStep = (p: Tile, q: Tile) =>
    through(p).some((id) => {
      const s = lines[id];
      return s !== undefined && onLine(s, q);
    });
  const tiles = [...lineTiles(from, to)];
  for (const t of tiles) {
    const kind = index.tiles.get(key(t.x, t.y));
    if (kind !== "node" && through(t).some((id) => lines[id]?.highway === true)) {
      return true;
    }
  }
  const diagonal = from.x !== to.x && from.y !== to.y;
  for (let k = 0; k + 1 < tiles.length; k++) {
    const p = tiles[k] as Tile;
    const q = tiles[k + 1] as Tile;
    if (sharesStep(p, q)) {
      return true;
    }
    if (diagonal && sharesStep({ x: q.x, y: p.y }, { x: p.x, y: q.y })) {
      return true;
    }
  }
  return false;
}

/** つなげるマスか。高速道路の途中は含まない。 */
function connectable(index: RoadIndex, t: Tile): RoadTileKind | null {
  const kind = index.tiles.get(key(t.x, t.y));
  return kind === "node" || kind === "road" ? kind : null;
}

/** 道路のマスか。高速道路の途中も含む。 */
export function isRoadTile(index: RoadIndex, t: Tile): boolean {
  return index.tiles.has(key(t.x, t.y));
}

const aligned = (from: Tile, to: Tile) => {
  const dx = Math.abs(to.x - from.x);
  const dy = Math.abs(to.y - from.y);
  return dx === 0 || dy === 0 || dx === dy;
};

/**
 * `around` の周り（SNAP_RADIUS マス）にある、つなげる道路のマスのうち最良のもの。
 * 端を途中より優先し、同じ種類なら `prefer` に近いものを選ぶ。`accept` が偽のマスは選ばない。
 */
function nearestRoad(
  index: RoadIndex,
  around: Tile,
  prefer: Tile,
  accept: (t: Tile) => boolean,
): Tile | null {
  let best: { tile: Tile; rank: number; dist: number } | null = null;
  for (let dy = -SNAP_RADIUS; dy <= SNAP_RADIUS; dy++) {
    for (let dx = -SNAP_RADIUS; dx <= SNAP_RADIUS; dx++) {
      const tile = { x: around.x + dx, y: around.y + dy };
      const kind = connectable(index, tile);
      if (kind === null || !accept(tile)) {
        continue;
      }
      const rank = kind === "node" ? 0 : 1;
      const dist = (tile.x - prefer.x) ** 2 + (tile.y - prefer.y) ** 2;
      if (best === null || rank < best.rank || (rank === best.rank && dist < best.dist)) {
        best = { tile, rank, dist };
      }
    }
  }
  return best?.tile ?? null;
}

/** 始点にするマスを、近くの道路の端か区間の上のマスに吸い付ける。道路の上ならそのまま。 */
export function snapRoadStart(index: RoadIndex, cursor: Tile): Tile {
  if (connectable(index, cursor) !== null) {
    return cursor;
  }
  return nearestRoad(index, cursor, cursor, () => true) ?? cursor;
}

/**
 * 終点を求める。まず始点から 8 方向のどれかに寄せ、その終点の近くに道路があれば吸い付ける。
 * 吸い付けたあとも始点から縦横か 45 度の直線にならない道路は選ばない。
 */
export function snapRoadEnd(index: RoadIndex, start: Tile, cursor: Tile): Tile {
  const line = snapToEightDirections(start, cursor);
  if (connectable(index, line) !== null) {
    return line;
  }
  const near = nearestRoad(index, line, cursor, (t) => {
    const len = Math.max(Math.abs(t.x - start.x), Math.abs(t.y - start.y));
    return len > 0 && len <= MAX_ROAD_TILES && aligned(start, t) && !isRefused(index, start, t);
  });
  return near ?? line;
}

/** 始点から終点までの直線のマス。両端を含む。 */
function* lineTiles(from: Tile, to: Tile): Generator<Tile> {
  const len = Math.max(Math.abs(to.x - from.x), Math.abs(to.y - from.y));
  const sx = Math.sign(to.x - from.x);
  const sy = Math.sign(to.y - from.y);
  for (let k = 0; k <= len; k++) {
    yield { x: from.x + sx * k, y: from.y + sy * k };
  }
}

/**
 * 引こうとしている道路が、既存のどの道路ともつながらないか。
 * 道路が 1 本もない街では、比べるものがないので偽にする。
 */
export function isDisconnected(index: RoadIndex, from: Tile, to: Tile): boolean {
  if (index.size === 0) {
    return false;
  }
  for (const t of lineTiles(from, to)) {
    if (connectable(index, t) !== null) {
      return false;
    }
  }
  return true;
}

/** つながらない道路をプレビューで知らせる文。 */
export const DISCONNECTED_NOTICE =
  "既存の道路につながらない：電気と水道が届かず、区画が建たない（Alt を押すと吸い付かない）";

/**
 * 道路の始点と終点を求める。始点を決めていなければ、始点はカーソルを吸い付けたマスで、終点も同じにする。
 * `noSnap`（Alt を押している）なら吸い付けず、終点は8方向に寄せるだけにする。
 */
export function roadEndpoints(
  index: RoadIndex,
  start: Tile | null,
  cursor: Tile,
  noSnap: boolean,
): { from: Tile; to: Tile } {
  if (start === null) {
    const from = noSnap ? cursor : snapRoadStart(index, cursor);
    return { from, to: from };
  }
  const to = noSnap ? snapToEightDirections(start, cursor) : snapRoadEnd(index, start, cursor);
  return { from: start, to };
}
