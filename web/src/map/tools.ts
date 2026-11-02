// 道具の操作をコマンドに直す部分。DOM に触らないので、Vitest で確かめられる。

import {
  CHUNK_SIDE,
  type CommandMessage,
  type RoadKindName,
  type ServiceName,
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
  | { kind: "service"; service: ServiceName }
  | { kind: "signal"; on: boolean }
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

/** 1 回で塗れる区画の長方形の 1 辺の上限（マス）。chizu-core の MAX_ZONE_SIDE と同じ。 */
export const MAX_ZONE_SIDE = 256;

/** 始点から見たカーソルのマスを、長方形の 1 辺が max マスを超えないところで止める。 */
export function clampArea(from: Tile, cursor: Tile, max: number): Tile {
  const clamp = (start: number, v: number) =>
    Math.min(Math.max(v, start - max + 1), start + max - 1);
  return { x: clamp(from.x, cursor.x), y: clamp(from.y, cursor.y) };
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

/** chizu-wasm の service_table の中身。 */
export type ServiceInfo = { cost: number; width: number; depth: number };

export function parseServiceTable(table: ArrayLike<number>): Record<ServiceName, ServiceInfo> {
  const at = (i: number) => {
    const v = table[i];
    if (v === undefined) {
      throw new Error("施設の表が短い");
    }
    return v;
  };
  return {
    PowerPlant: { cost: at(0), width: at(1), depth: at(2) },
    WaterPump: { cost: at(3), width: at(4), depth: at(5) },
  };
}

/**
 * 施設の rotation ごとの、間口のマスから道路へ向かう 1 歩。chizu-core の growth::DIRS と同じ順である。
 */
export const ROTATION_STEPS: readonly (readonly [number, number])[] = [
  [0, -1],
  [1, 0],
  [0, 1],
  [-1, 0],
];

const stepOf = (rotation: number) => ROTATION_STEPS[((rotation % 4) + 4) % 4] ?? [0, -1];

/**
 * 間口のマス at から置く施設が占める長方形。間口は道路に沿って width マス、奥へ depth マス伸びる。
 * chizu-core の lot_tiles と同じ向きに並べる。
 */
export function serviceFootprint(
  at: Tile,
  rotation: number,
  width: number,
  depth: number,
): { min: Tile; max: Tile } {
  const [dx, dy] = stepOf(rotation);
  const along = [-dy, dx];
  const back = [-dx, -dy];
  const far = {
    x: at.x + (along[0] ?? 0) * (width - 1) + (back[0] ?? 0) * (depth - 1),
    y: at.y + (along[1] ?? 0) * (width - 1) + (back[1] ?? 0) * (depth - 1),
  };
  return {
    min: { x: Math.min(at.x, far.x), y: Math.min(at.y, far.y) },
    max: { x: Math.max(at.x, far.x), y: Math.max(at.y, far.y) },
  };
}

/** 中核に問い合わせた、向き 0〜3 ごとの置けるか。置けるなら null、置けないなら理由の文。 */
export type ServiceProbe = (string | null)[];

export type ServiceChoice = { rotation: number; placeable: boolean; reason: string | null };

/**
 * 置く向きを決める。手で決めた向きがあればそれを使い、なければ置ける最初の向きを選ぶ。
 * どの向きにも置けなければ向き 0 を、置けないものとして返す。
 */
export function chooseRotation(probe: ServiceProbe, manual: number | null): ServiceChoice {
  const at = (r: number) => (probe[r] === undefined ? "まだ調べていない" : probe[r]);
  const pick = (rotation: number): ServiceChoice => {
    const reason = at(rotation) ?? null;
    return { rotation, placeable: reason === null, reason };
  };
  if (manual !== null) {
    return pick(manual);
  }
  const ok = [0, 1, 2, 3].find((r) => at(r) === null);
  return pick(ok ?? 0);
}

export function placeServiceCommand(
  service: ServiceName,
  at: Tile,
  rotation: number,
): CommandMessage {
  return { kind: "PlaceBuilding", service, atX: at.x, atY: at.y, rotation };
}

/** マスの交差点に信号を置くか（on が true）、外す。 */
export function signalCommand(at: Tile, on: boolean): CommandMessage {
  return { kind: "SetSignal", x: at.x, y: at.y, on };
}

/** マスを含むチャンクを買う。 */
export function buyChunkCommand(at: Tile): CommandMessage {
  return {
    kind: "BuyChunk",
    cx: Math.floor(at.x / CHUNK_SIDE),
    cy: Math.floor(at.y / CHUNK_SIDE),
  };
}
