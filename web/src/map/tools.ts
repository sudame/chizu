// 道具の操作をコマンドに直す部分。DOM に触らないので、Vitest で確かめられる。

import {
  CHUNK_SIDE,
  type CommandMessage,
  type RoadKindName,
  type ZoneName,
} from "../protocol/messages.ts";
import type { Camera, Viewport } from "./camera.ts";
import type { Tile } from "./roads.ts";

export type { Tile };

/** 1 回で引ける道路の長さの上限（マス）。chizu-core の MAX_ROAD_TILES と同じ。 */
export const MAX_ROAD_TILES = 1024;

export type Tool =
  | { kind: "none" }
  | { kind: "road"; road: RoadKindName }
  | { kind: "bulldoze" }
  | { kind: "zone"; zone: ZoneName }
  | { kind: "buy" };

/** 画面の点（CSS ピクセル）の下にあるマス。 */
export function screenToTile(camera: Camera, sx: number, sy: number, viewport: Viewport): Tile {
  const w = camera.screenToWorld(sx, sy, viewport);
  return { x: Math.floor(w.x), y: Math.floor(w.y) };
}

// 22.5 度の正接。これより寝ていれば縦横に、立っていれば斜めに寄せる
const TAN_22_5 = Math.SQRT2 - 1;

/** 始点から見たカーソルのマスを、縦横と 45 度の 8 方向のうち近いものに寄せた終点。長さは上限で切る。 */
export function snapToEightDirections(from: Tile, cursor: Tile): Tile {
  const dx = cursor.x - from.x;
  const dy = cursor.y - from.y;
  const ax = Math.abs(dx);
  const ay = Math.abs(dy);
  let sx = Math.sign(dx);
  let sy = Math.sign(dy);
  let len: number;
  if (ay <= ax * TAN_22_5) {
    sy = 0;
    len = ax;
  } else if (ax <= ay * TAN_22_5) {
    sx = 0;
    len = ay;
  } else {
    len = Math.round((ax + ay) / 2);
  }
  len = Math.min(len, MAX_ROAD_TILES);
  return { x: from.x + sx * len, y: from.y + sy * len };
}

/** chizu-wasm の road_cost_table の中身。 */
export type RoadCostTable = {
  perTile: Record<RoadKindName, number>;
  tileMm: number;
  diagonalTileMm: number;
};

export function parseRoadCostTable(table: ArrayLike<number>): RoadCostTable {
  const at = (i: number) => {
    const v = table[i];
    if (v === undefined) {
      throw new Error("道路の費用の表が短い");
    }
    return v;
  };
  return {
    perTile: { Small: at(0), Medium: at(1), Large: at(2) },
    tileMm: at(3),
    diagonalTileMm: at(4),
  };
}

/** 8 方向に寄せた道路の建設費。中核の road_cost と同じく、長さ（mm）に比例させて端数を切り上げる。 */
export function roadCost(table: RoadCostTable, kind: RoadKindName, from: Tile, to: Tile): number {
  const steps = Math.max(Math.abs(to.x - from.x), Math.abs(to.y - from.y));
  const diagonal = to.x !== from.x && to.y !== from.y;
  const length = steps * (diagonal ? table.diagonalTileMm : table.tileMm);
  return Math.ceil((table.perTile[kind] * length) / table.tileMm);
}

export function buildRoadCommand(kind: RoadKindName, from: Tile, to: Tile): CommandMessage {
  return { kind: "BuildRoad", road: kind, fromX: from.x, fromY: from.y, toX: to.x, toY: to.y };
}

/** 2 つの角のマスを両端に含む長方形を消す。角の順番は問わない。 */
export function bulldozeCommand(a: Tile, b: Tile): CommandMessage {
  return {
    kind: "Bulldoze",
    minX: Math.min(a.x, b.x),
    minY: Math.min(a.y, b.y),
    maxX: Math.max(a.x, b.x),
    maxY: Math.max(a.y, b.y),
  };
}

/** 2 つの角のマスを両端に含む長方形に区画を塗る。Clear なら区画を消す。 */
export function paintZoneCommand(zone: ZoneName, a: Tile, b: Tile): CommandMessage {
  return {
    kind: "PaintZone",
    zone,
    minX: Math.min(a.x, b.x),
    minY: Math.min(a.y, b.y),
    maxX: Math.max(a.x, b.x),
    maxY: Math.max(a.y, b.y),
  };
}

/** マスを含むチャンクを買う。 */
export function buyChunkCommand(at: Tile): CommandMessage {
  return {
    kind: "BuyChunk",
    cx: Math.floor(at.x / CHUNK_SIDE),
    cy: Math.floor(at.y / CHUNK_SIDE),
  };
}
