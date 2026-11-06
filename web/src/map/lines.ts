// 路線の色、一覧の読み方、地図に路線を重ねる描き方（docs/map-style.md の「鉄道」の路線）。
// 路線は中核の `RailLine` で、駅の並びと、通る線路を持つ。色は路線の添え字の順に 8 色を回して当てる。

import { hex } from "./palette.ts";
import { Geometry, type Tile } from "./roads.ts";

/** 路線の色。路線の添え字の順に当てる。車の色の表と、道路と信号の色から離してある（docs/design.md の 3.10 の 8）。 */
export const LINE_COLORS: readonly string[] = [
  "#0079c2",
  "#009944",
  "#f39800",
  "#8f76d6",
  "#e85298",
  "#00a7db",
  "#9c5e31",
  "#b0a400",
];
/** 色の呼び名。路線の名前に使う。 */
export const LINE_COLOR_NAMES: readonly string[] = [
  "青",
  "緑",
  "橙",
  "紫",
  "桃",
  "水",
  "茶",
  "黄緑",
];

/** chizu-wasm の rail_lines の、路線ごとの見出しの数の個数。 */
export const LINE_HEADER = 11;
/** 本数の上限。chizu-core の MAX_LINE_TRAINS と同じ。 */
export const MAX_LINE_TRAINS = 16;

/** 路線の色の番号。chizu-wasm の trains::line_color と同じ。 */
export function lineColorIndex(index: number): number {
  return index % LINE_COLORS.length;
}

/** 路線の色（`#rrggbb`）。 */
export function lineColorCode(index: number): string {
  return LINE_COLORS[lineColorIndex(index)] ?? "#0079c2";
}

/** 路線の名前。色の呼び名に「線」を付け、8 本を超える路線には、同じ色の何本目かを添える。 */
export function lineName(index: number): string {
  const name = `${LINE_COLOR_NAMES[lineColorIndex(index)] ?? ""}線`;
  const round = Math.floor(index / LINE_COLORS.length);
  return round === 0 ? name : `${name}（${round + 1}）`;
}

export type RailLineInfo = {
  /** 路線の ID。路線のコマンドで路線を指すのに使う。 */
  index: number;
  generation: number;
  looped: boolean;
  /** 遊ぶ人が決めた本数。 */
  trains: number;
  /** 行き詰まりを防ぐ本数の上限。 */
  cap: number;
  /** 走らせる本数（決めた本数を上限に抑えたもの）。 */
  allowed: number;
  /** 線路が変わって進路を選べなくなった。 */
  stopped: boolean;
  /** 運転間隔（tick）。列車がなければ 0。 */
  headway: number;
  /** 1 周の見込み（tick）。 */
  roundTrip: number;
  /** 駅の並び。駅の敷地の min のマスで指す。 */
  stations: Tile[];
  /** 通る線路（重ならないもの）。 */
  tracks: { a: Tile; b: Tile }[];
};

/** chizu-wasm の rail_lines の数の並びを読む。壊れていたら、読めたところまでにする。 */
export function parseLines(data: ArrayLike<number>): RailLineInfo[] {
  const out: RailLineInfo[] = [];
  let i = 0;
  while (i + LINE_HEADER <= data.length) {
    const at = (k: number) => data[i + k] ?? 0;
    const stationCount = at(9);
    const trackCount = at(10);
    const end = i + LINE_HEADER + stationCount * 2 + trackCount * 4;
    if (end > data.length) {
      break;
    }
    const stations: Tile[] = [];
    for (let k = 0; k < stationCount; k++) {
      const p = i + LINE_HEADER + k * 2;
      stations.push({ x: data[p] ?? 0, y: data[p + 1] ?? 0 });
    }
    const tracks: { a: Tile; b: Tile }[] = [];
    for (let k = 0; k < trackCount; k++) {
      const p = i + LINE_HEADER + stationCount * 2 + k * 4;
      tracks.push({
        a: { x: data[p] ?? 0, y: data[p + 1] ?? 0 },
        b: { x: data[p + 2] ?? 0, y: data[p + 3] ?? 0 },
      });
    }
    out.push({
      index: at(0),
      generation: at(1),
      looped: at(2) === 1,
      trains: at(3),
      cap: at(4),
      allowed: at(5),
      stopped: at(6) === 1,
      headway: at(7),
      roundTrip: at(8),
      stations,
      tracks,
    });
    i = end;
  }
  return out;
}

/**
 * 1 tick が移動の何秒か。時計では 1 tick が 12 秒だが、移動は時計より 12 倍遅い尺度で進めるので、
 * 移動の上では 1 tick が 1 秒にあたる（docs/design.md の 1.6 と 3.10 の 8）。
 */
const MOVE_SECONDS_PER_TICK = 1;

/** tick の長さを、移動の分で書く。運転間隔や 1 周の時間に使う。 */
export function moveMinutes(ticks: number): string {
  const minutes = (ticks * MOVE_SECONDS_PER_TICK) / 60;
  return minutes < 10
    ? `${(Math.round(minutes * 10) / 10).toFixed(1)} 分`
    : `${Math.round(minutes)} 分`;
}

/** 路線の線の半分の太さ（マス）。引きと標準では、線路の太さ（0.8 マス）の中に収まる太さにする。 */
export const LINE_HALF = 0.22;
/** 引きと標準の線の、画面の半分の太さの下限（ピクセル）。 */
export const LINE_MIN_PX = 1.2;
/** 寄りと細部では、複線の真ん中（2 本の線路のあいだ）に細く引く。 */
export const LINE_NEAR_HALF = 0.05;
export const LINE_NEAR_MIN_PX = 0.8;

export type LineMeshes = {
  /** 引きと標準。 */
  far: Geometry;
  /** 寄りと細部。 */
  near: Geometry;
};

/**
 * 路線を線路の上に路線の色で重ねる。止まっている路線は描かない。
 * 同じ線路を通る路線が複数あるときは、路線の添え字の順に横へ並べる（線路の幅に収まるよう、1 本の太さを抑える）。
 */
export function lineMeshes(lines: readonly RailLineInfo[]): LineMeshes {
  const live = lines.filter((l) => !l.stopped && l.tracks.length > 0);
  const origin = live[0]?.tracks[0]?.a ?? { x: 0, y: 0 };
  const meshes: LineMeshes = { far: new Geometry(origin), near: new Geometry(origin) };
  // 線路ごとに通る路線の添え字を集める。向きによらず同じ線路として数える
  const key = (a: Tile, b: Tile) => {
    const first = a.x < b.x || (a.x === b.x && a.y <= b.y);
    const [p, q] = first ? [a, b] : [b, a];
    return `${p.x},${p.y},${q.x},${q.y}`;
  };
  const users = new Map<string, number[]>();
  for (const line of live) {
    for (const { a, b } of line.tracks) {
      const k = key(a, b);
      const list = users.get(k);
      if (list === undefined) {
        users.set(k, [line.index]);
      } else {
        list.push(line.index);
      }
    }
  }
  for (const list of users.values()) {
    list.sort((x, y) => x - y);
  }
  for (const line of live) {
    const color = hex(lineColorCode(line.index));
    for (const { a, b } of line.tracks) {
      const list = users.get(key(a, b)) ?? [line.index];
      const n = list.length;
      const slot = list.indexOf(line.index);
      // a から b への右が正。線路の中心線を中心に、n 本を等しい幅で並べる
      const half = Math.min(LINE_HALF, (0.8 - 0.1 * (n - 1)) / (2 * n));
      const offset = (slot - (n - 1) / 2) * 2 * half * 1.1;
      const forward = a.x < b.x || (a.x === b.x && a.y <= b.y);
      const shift = forward ? offset : -offset;
      meshes.far.strip(a, b, shift, half, color, LINE_MIN_PX);
      const nearHalf = Math.min(LINE_NEAR_HALF, half);
      meshes.near.strip(a, b, shift * (nearHalf / half), nearHalf, color, LINE_NEAR_MIN_PX);
    }
  }
  return meshes;
}

/** 路線が駅 `at`（敷地の min のマス）に止まるか。 */
export function stopsAt(line: RailLineInfo, at: Tile): boolean {
  return line.stations.some((s) => s.x === at.x && s.y === at.y);
}

/** 路線の駅の並びを、始点から終点（環状なら始点に戻る）の順に、名前をつないだ文にする。 */
export function stationChain(line: RailLineInfo, nameOf: (t: Tile) => string): string {
  const names = line.stations.map(nameOf);
  if (line.looped && names.length > 0) {
    names.push(names[0] ?? "");
  }
  return names.join(" → ");
}
