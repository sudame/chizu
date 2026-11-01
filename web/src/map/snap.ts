// 道路を引くときの、既存の道路への吸い付きと、つながりの判定。DOM に触らないので Vitest で確かめられる。

import { NODE_STRIDE, SEGMENT_STRIDE } from "../protocol/messages.ts";
import { HIGHWAY_CODE } from "./lanes.ts";
import type { Tile } from "./roads.ts";
import { MAX_ROAD_TILES, snapToEightDirections } from "./tools.ts";

/** 吸い付く範囲（マス）。縦横斜めに 1 マスまでである。 */
export const SNAP_RADIUS = 1;

/**
 * 道路のマスの種類。node は区間の端（つなげるところ）、road は区間の途中。
 * 高速道路の途中は中核が道路をつなぐことを断るので、吸い付き先にもつながりにも数えない。
 */
type RoadTileKind = "node" | "road" | "highway";

/** 道路が通るマスの索引。 */
export type RoadIndex = {
  readonly tiles: ReadonlyMap<string, RoadTileKind>;
  readonly size: number;
};

const key = (x: number, y: number) => `${x},${y}`;

export const EMPTY_ROAD_INDEX: RoadIndex = { tiles: new Map(), size: 0 };

/** Worker から届いた区間とノードから、道路が通るマスの索引を作る。 */
export function buildRoadIndex(segments: Float64Array, nodes: Float64Array): RoadIndex {
  const tiles = new Map<string, RoadTileKind>();
  const mark = (x: number, y: number, kind: RoadTileKind) => {
    const k = key(x, y);
    const old = tiles.get(k);
    // 端であることが、途中であることより優先する
    if (old === "node" || (old === "road" && kind === "highway")) {
      return;
    }
    tiles.set(k, kind);
  };
  for (let i = 0; i + SEGMENT_STRIDE <= segments.length; i += SEGMENT_STRIDE) {
    const [ax = 0, ay = 0, bx = 0, by = 0, kind = 0] = segments.subarray(i, i + SEGMENT_STRIDE);
    const dx = bx - ax;
    const dy = by - ay;
    const len = Math.max(Math.abs(dx), Math.abs(dy));
    const straight = dx === 0 || dy === 0 || Math.abs(dx) === Math.abs(dy);
    const inner: RoadTileKind = kind === HIGHWAY_CODE ? "highway" : "road";
    if (straight) {
      for (let k = 1; k < len; k++) {
        mark(ax + Math.sign(dx) * k, ay + Math.sign(dy) * k, inner);
      }
    }
    mark(ax, ay, "node");
    mark(bx, by, "node");
  }
  for (let i = 0; i + NODE_STRIDE <= nodes.length; i += NODE_STRIDE) {
    mark(nodes[i] ?? 0, nodes[i + 1] ?? 0, "node");
  }
  return { tiles, size: tiles.size };
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
    return len > 0 && len <= MAX_ROAD_TILES && aligned(start, t);
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
