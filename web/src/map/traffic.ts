// 交通の主題図（docs/map-style.md の「主題図」）。Worker から届いた交通のレイヤーのマスごとの値を手元に持ち、
// 道路網の区間に沿った線と帯の頂点に直す。頂点を作り直すのは、値か道路網が変わったときだけで、毎フレームは作らない。
// 引きと標準では道路の中の線、寄りでは車道の両端の帯にする。どちらを濃く描くかは roads.ts の roadDrawOrder が倍率で決める。

import { CHUNK_SIDE, SEGMENT_STRIDE } from "../protocol/messages.ts";
import { chunkKey } from "./chunk-sync.ts";
import { HIGHWAY_CODE } from "./lanes.ts";
import { TRAFFIC_LEVELS } from "./layers.ts";
import { type Color, hex } from "./palette.ts";
import {
  drawnAsphaltHalf,
  Geometry,
  isDiagonal,
  lineHalfWidth,
  roadNet,
  roadStyle,
  type Tile,
  tilesOnLine,
  trimmer,
} from "./roads.ts";

/** 交通のレイヤーの値。chizu-wasm の layers::TRAFFIC_FREE などと同じで、0 は値がない。 */
export const TRAFFIC_FREE = 1;
export const TRAFFIC_JAMMED = 4;

/** 混み具合を数えない高速道路の破線の色。 */
export const TRAFFIC_UNCOUNTED_COLOR = hex("#9aa0a6");

/** 道路の中の線の太さ。道路の線の太さに対する割合である。 */
export const TRAFFIC_LINE_SHARE = 0.55;
/** 道路の中の線の、半分の太さの下限（画面のピクセル）。線の太さの下限は 2px である。 */
export const TRAFFIC_LINE_MIN_PX = 1;
/** 詰まっている区間の線を、半分の太さでこのピクセル数だけ太くする（線の太さで 1px）。 */
export const JAMMED_EXTRA_PX = 0.5;
/** 寄りの段の、車道の両端の帯の幅（マス）と不透明度。 */
export const TRAFFIC_BAND_WIDTH = 0.06;
export const TRAFFIC_BAND_ALPHA = 0.7;
/** 帯の、半分の太さの下限（画面のピクセル）。寄りの段の始めでも 2px の帯にする。 */
export const TRAFFIC_BAND_MIN_PX = 1;
// 高速道路の破線の周期（画面のピクセル）
const LINE_DASH_PX = 8;
const BAND_DASH_PX = 6;

/** 値の色。値のない 0 と、知らない値は undefined にする。 */
export function trafficColor(value: number, alpha = 1): Color | undefined {
  const level = TRAFFIC_LEVELS[value - 1];
  if (level === undefined) {
    return undefined;
  }
  const [r, g, b] = level.color;
  return [r, g, b, alpha];
}

/** 画面に届いている交通の値。チャンクごとの 64×64 の値を持つ。 */
export class TrafficValues {
  readonly #chunks = new Map<string, Uint8Array>();
  /** 前に頂点を作ってから、値が変わったか。 */
  changed = false;

  set(cx: number, cy: number, values: Uint8Array): void {
    this.#chunks.set(chunkKey(cx, cy), values);
    this.changed = true;
  }

  remove(cx: number, cy: number): void {
    if (this.#chunks.delete(chunkKey(cx, cy))) {
      this.changed = true;
    }
  }

  clear(): void {
    this.#chunks.clear();
    this.changed = true;
  }

  /** マス (x, y) の値。届いていないチャンクのマスは 0 にする。 */
  at(x: number, y: number): number {
    const cx = Math.floor(x / CHUNK_SIDE);
    const cy = Math.floor(y / CHUNK_SIDE);
    const values = this.#chunks.get(chunkKey(cx, cy));
    if (values === undefined) {
      return 0;
    }
    return values[(y - cy * CHUNK_SIDE) * CHUNK_SIDE + (x - cx * CHUNK_SIDE)] ?? 0;
  }
}

/**
 * 交通の主題図の頂点。`free` は「空いている」の線（引きでは描かない）、`busy` はやや混んでいる以上の線と
 * 高速道路の破線、`bands` は寄りの段の、車道の両端の帯である。
 */
export type TrafficMeshes = { free: Geometry; busy: Geometry; bands: Geometry };

/** 同じ値が続くマスのまとまり。`from` と `to` は区間の始めから数えたマスの番号で、両端を含む。 */
export type Run = { from: number; to: number; value: number };

/** 値の並びを、同じ値が続くまとまりに分ける。値のない 0 は飛ばす。 */
export function runsOf(values: readonly number[]): Run[] {
  const runs: Run[] = [];
  for (const [i, value] of values.entries()) {
    const last = runs.at(-1);
    if (last !== undefined && last.value === value && last.to === i - 1) {
      last.to = i;
    } else if (value !== 0) {
      runs.push({ from: i, to: i, value });
    }
  }
  return runs;
}

/**
 * 区間 a〜b の上のまとまりを、区間の始めからの距離（マス）の範囲に直す。マスとマスの境目は真ん中で分け、
 * 区間の両端のまとまりは `trimA` と `trimB` だけ縮めた端まで伸ばす。
 */
export function runSpan(
  run: Run,
  steps: number,
  step: number,
  trimA: number,
  trimB: number,
): { start: number; end: number } {
  const len = steps * step;
  const start = run.from === 0 ? trimA : (run.from - 0.5) * step;
  const end = run.to === steps ? len - trimB : (run.to + 0.5) * step;
  return { start, end };
}

/** 道路網と交通の値から、交通の主題図の頂点を作る。基準のマスは最初のノードにする。 */
export function trafficMeshes(
  segments: Float64Array,
  nodes: Float64Array,
  valueAt: (x: number, y: number) => number,
): TrafficMeshes {
  const origin = { x: nodes[0] ?? 0, y: nodes[1] ?? 0 };
  const meshes: TrafficMeshes = {
    free: new Geometry(origin),
    busy: new Geometry(origin),
    bands: new Geometry(origin),
  };
  const { ends, nodeList, each } = roadNet(segments, nodes);
  const trimOf = trimmer(segments, ends);
  const lineOf = (value: number) => (value === TRAFFIC_FREE ? meshes.free : meshes.busy);
  const lineMin = (kind: number) =>
    Math.max(TRAFFIC_LINE_MIN_PX, TRAFFIC_LINE_SHARE * roadStyle(kind).minPx);
  const extraOf = (value: number) => (value === TRAFFIC_JAMMED ? JAMMED_EXTRA_PX : 0);
  const bandHalf = TRAFFIC_BAND_WIDTH / 2;

  each((a: Tile, b: Tile, kind: number, n: number) => {
    const tiles = tilesOnLine(a, b);
    const steps = tiles.length - 1;
    if (steps === 0) {
      return;
    }
    const step = isDiagonal(a, b) ? Math.SQRT2 : 1;
    const len = steps * step;
    const lineHalf = TRAFFIC_LINE_SHARE * lineHalfWidth(kind, a, b);
    const bandOffset = drawnAsphaltHalf(kind, a, b) - bandHalf;
    if (kind === HIGHWAY_CODE) {
      // 高速道路は混み具合を数えないので、どの段でも灰の破線にする
      const color = TRAFFIC_UNCOUNTED_COLOR;
      meshes.busy.strip(a, b, 0, lineHalf, color, lineMin(kind), 0, 0, 0, LINE_DASH_PX);
      for (const side of [-1, 1]) {
        const offset = side * bandOffset;
        const trimA = trimOf(n, false, offset, (node) => node.asphalt);
        const trimB = trimOf(n, true, offset, (node) => node.asphalt);
        const banded: Color = [color[0], color[1], color[2], TRAFFIC_BAND_ALPHA];
        meshes.bands.strip(
          a,
          b,
          offset,
          bandHalf,
          banded,
          TRAFFIC_BAND_MIN_PX,
          trimA,
          trimB,
          0,
          BAND_DASH_PX,
        );
      }
      return;
    }
    const runs = runsOf(tiles.map((t) => valueAt(t.x, t.y)));
    for (const run of runs) {
      const color = trafficColor(run.value);
      if (color === undefined) {
        continue;
      }
      const { start, end } = runSpan(run, steps, step, 0, 0);
      lineOf(run.value).strip(
        a,
        b,
        0,
        lineHalf,
        color,
        lineMin(kind),
        start,
        len - end,
        extraOf(run.value),
      );
    }
    for (const side of [-1, 1]) {
      const offset = side * bandOffset;
      const trimA = trimOf(n, false, offset, (node) => node.asphalt);
      const trimB = trimOf(n, true, offset, (node) => node.asphalt);
      for (const run of runs) {
        const color = trafficColor(run.value, TRAFFIC_BAND_ALPHA);
        if (color === undefined) {
          continue;
        }
        const { start, end } = runSpan(run, steps, step, trimA, trimB);
        if (end > start) {
          meshes.bands.strip(a, b, offset, bandHalf, color, TRAFFIC_BAND_MIN_PX, start, len - end);
        }
      }
    }
  });
  // ノードのマスにも線の色の四角を置き、折れ曲がりと交差点で線をつなぐ
  for (const { x, y, node } of nodeList) {
    // 高速道路だけがつながるノードは、混み具合を数えないので置かない
    const counted = node.arms.some(
      (arm) => segments[arm.segment * SEGMENT_STRIDE + 4] !== HIGHWAY_CODE,
    );
    const value = counted ? valueAt(x, y) : 0;
    const color = trafficColor(value);
    if (color === undefined) {
      continue;
    }
    const min = Math.max(TRAFFIC_LINE_MIN_PX, TRAFFIC_LINE_SHARE * node.minPx);
    lineOf(value).square({ x, y }, TRAFFIC_LINE_SHARE * node.line, color, min, extraOf(value));
  }
  return meshes;
}
