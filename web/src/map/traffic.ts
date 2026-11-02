// 交通の主題図（docs/map-style.md の「主題図」）。Worker から届いた区間ごとの交通の値を、道路網の区間に沿った
// 線と帯の頂点に直す。区間の形（位置、太さ、交差点での縮め方）は道路網が変わったときだけ求めて持っておき、
// 値が変わったときは、その形に色を付けて頂点の並びへ書き出すだけにする。毎フレームは作らない。
// 引きと標準では道路の中の線、寄りでは車道の両端の帯にする。どちらを濃く描くかは roads.ts の roadDrawOrder が倍率で決める。

import { SEGMENT_STRIDE } from "../protocol/messages.ts";
import { HIGHWAY_CODE } from "./lanes.ts";
import { TRAFFIC_LEVELS } from "./layers.ts";
import { type Color, hex } from "./palette.ts";
import {
  drawnAsphaltHalf,
  lineHalfWidth,
  type MeshSource,
  roadNet,
  roadStyle,
  type Tile,
  trimmer,
  VERTEX_FLOATS,
} from "./roads.ts";

/** 交通の値。chizu-wasm の layers::TRAFFIC_FREE などと同じで、0 は値がない（高速道路など）。 */
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
/**
 * 細部の段の帯の幅（マス）と不透明度。寄りの帯より細く薄くし、待っている車の列を主役にする。
 * 半分の太さの下限は 0.5px（帯の太さで 1px）にする。
 */
export const TRAFFIC_FINE_BAND_WIDTH = 0.04;
export const TRAFFIC_FINE_BAND_ALPHA = 0.5;
export const TRAFFIC_FINE_BAND_MIN_PX = 0.5;
// 高速道路の破線の周期（画面のピクセル）
const LINE_DASH_PX = 8;
const BAND_DASH_PX = 6;
// 1 本の帯（四角）と 1 つの正方形の頂点の数
const QUAD_VERTICES = 6;

/** 値の色。値のない 0 と、知らない値は undefined にする。 */
export function trafficColor(value: number, alpha = 1): Color | undefined {
  const level = TRAFFIC_LEVELS[value - 1];
  if (level === undefined) {
    return undefined;
  }
  const [r, g, b] = level.color;
  return [r, g, b, alpha];
}

/** 頂点の並び。RoadLayer にそのまま渡せる。 */
export class TrafficMesh implements MeshSource {
  readonly origin: Tile;
  readonly #data: Float32Array;
  #length = 0;

  constructor(origin: Tile, maxVertices: number) {
    this.origin = origin;
    this.#data = new Float32Array(maxVertices * VERTEX_FLOATS);
  }

  get vertexCount(): number {
    return this.#length / VERTEX_FLOATS;
  }

  build(): Float32Array {
    return this.#data.subarray(0, this.#length);
  }

  /** 頂点を 1 つ書く。位置は基準のマスからの相対である。 */
  vertex(
    x: number,
    y: number,
    nx: number,
    ny: number,
    half: number,
    min: number,
    color: Color,
    extra: number,
    along: number,
    dash: number,
  ): void {
    const d = this.#data;
    let i = this.#length;
    d[i++] = x;
    d[i++] = y;
    d[i++] = nx;
    d[i++] = ny;
    d[i++] = half;
    d[i++] = min;
    d[i++] = color[0];
    d[i++] = color[1];
    d[i++] = color[2];
    d[i++] = color[3];
    d[i++] = extra;
    d[i++] = along;
    d[i++] = dash;
    this.#length = i;
  }
}

/**
 * 交通の主題図の頂点。`free` は「空いている」の線（引きでは描かない）、`busy` はやや混んでいる以上の線と
 * 高速道路の破線、`bands` は寄りの段の、車道の両端の帯である。`fineBands` は細部の段の細く薄い帯で、
 * 細部の段に入っていないときは作らずに null にする。
 */
export type TrafficMeshes = {
  free: TrafficMesh;
  busy: TrafficMesh;
  bands: TrafficMesh;
  fineBands: TrafficMesh | null;
};

/** 区間の上の帯 1 本の形。位置は基準のマスからの相対で、`a` から `b` への帯の中心線である。 */
type Strip = {
  ax: number;
  ay: number;
  bx: number;
  by: number;
  nx: number;
  ny: number;
  length: number;
};

/** 区間 1 本の、色を付ける前の形。 */
type SegmentShape = {
  highway: boolean;
  line: Strip;
  lineHalf: number;
  lineMin: number;
  /** 車道の両端の帯（左と右）。 */
  bands: [Strip, Strip];
  /** 細部の段の、車道の両端の細い帯（左と右）。 */
  fineBands: [Strip, Strip];
};

/** 2 本の区間だけがつながる角。腕の線をつなぐために、両方の値が同じなら線の色の正方形を置く。 */
type Corner = { x: number; y: number; first: number; second: number; half: number; min: number };

function strip(
  origin: Tile,
  a: Tile,
  b: Tile,
  offset: number,
  trimA: number,
  trimB: number,
): Strip {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const len = Math.hypot(dx, dy);
  const [ux, uy] = [dx / len, dy / len];
  const [nx, ny] = [-uy, ux];
  return {
    ax: a.x + 0.5 + ux * trimA + nx * offset - origin.x,
    ay: a.y + 0.5 + uy * trimA + ny * offset - origin.y,
    bx: b.x + 0.5 - ux * trimB + nx * offset - origin.x,
    by: b.y + 0.5 - uy * trimB + ny * offset - origin.y,
    nx,
    ny,
    length: len - trimA - trimB,
  };
}

function writeStrip(
  mesh: TrafficMesh,
  s: Strip,
  half: number,
  min: number,
  color: Color,
  extra: number,
  dash: number,
): void {
  const v = (x: number, y: number, side: number, along: number) =>
    mesh.vertex(x, y, s.nx * side, s.ny * side, half, min, color, extra, along, dash);
  v(s.ax, s.ay, 1, 0);
  v(s.ax, s.ay, -1, 0);
  v(s.bx, s.by, 1, s.length);
  v(s.bx, s.by, 1, s.length);
  v(s.ax, s.ay, -1, 0);
  v(s.bx, s.by, -1, s.length);
}

function writeSquare(
  mesh: TrafficMesh,
  x: number,
  y: number,
  half: number,
  min: number,
  color: Color,
  extra: number,
): void {
  for (const [sx, sy] of [
    [-1, -1],
    [1, -1],
    [-1, 1],
    [-1, 1],
    [1, -1],
    [1, 1],
  ] as const) {
    mesh.vertex(x, y, sx, sy, half, min, color, extra, 0, 0);
  }
}

const extraOf = (value: number) => (value === TRAFFIC_JAMMED ? JAMMED_EXTRA_PX : 0);

/**
 * 道路網 1 つぶんの、交通の主題図の形。道路網が変わったときに作り直し、値が変わったときは `meshes` で色を付ける。
 * 区間の並びは Worker の road_segments と同じで、値も同じ並びで受け取る。
 */
export class TrafficNet {
  readonly origin: Tile;
  readonly #shapes: (SegmentShape | null)[];
  readonly #corners: Corner[];

  constructor(segments: Float64Array, nodes: Float64Array) {
    this.origin = { x: nodes[0] ?? 0, y: nodes[1] ?? 0 };
    const origin = this.origin;
    const { ends, nodeList, each } = roadNet(segments, nodes);
    const trimOf = trimmer(segments, ends);
    this.#shapes = [];
    each((a, b, kind, n) => {
      if (a.x === b.x && a.y === b.y) {
        this.#shapes.push(null);
        return;
      }
      // 帯は、外側の縁を縁石にそろえる
      const band = (side: number, width: number): Strip => {
        const offset = side * (drawnAsphaltHalf(kind, a, b) - width / 2);
        const trimA = trimOf(n, false, offset, (node) => node.asphalt);
        const trimB = trimOf(n, true, offset, (node) => node.asphalt);
        return strip(origin, a, b, offset, trimA, trimB);
      };
      this.#shapes.push({
        highway: kind === HIGHWAY_CODE,
        line: strip(origin, a, b, 0, 0, 0),
        lineHalf: TRAFFIC_LINE_SHARE * lineHalfWidth(kind, a, b),
        lineMin: Math.max(TRAFFIC_LINE_MIN_PX, TRAFFIC_LINE_SHARE * roadStyle(kind).minPx),
        bands: [band(-1, TRAFFIC_BAND_WIDTH), band(1, TRAFFIC_BAND_WIDTH)],
        fineBands: [band(-1, TRAFFIC_FINE_BAND_WIDTH), band(1, TRAFFIC_FINE_BAND_WIDTH)],
      });
    });
    // 2 本だけがつながる角では、線の端どうしのあいだに切れ目が出るので、正方形でふさぐ。
    // 交差点（3 本以上）は、交わる線どうしが中心で重なるので置かない
    this.#corners = [];
    for (const { x, y, node } of nodeList) {
      const [p, q] = node.arms;
      if (node.arms.length !== 2 || p === undefined || q === undefined) {
        continue;
      }
      const kindOf = (segment: number) => segments[segment * SEGMENT_STRIDE + 4] ?? 0;
      if (kindOf(p.segment) === HIGHWAY_CODE || kindOf(q.segment) === HIGHWAY_CODE) {
        continue;
      }
      this.#corners.push({
        x: x + 0.5 - origin.x,
        y: y + 0.5 - origin.y,
        first: p.segment,
        second: q.segment,
        half:
          TRAFFIC_LINE_SHARE *
          Math.min(node.line, lineHalfWidth(kindOf(p.segment), { x, y }, { x, y })),
        min: Math.max(TRAFFIC_LINE_MIN_PX, TRAFFIC_LINE_SHARE * node.minPx),
      });
    }
  }

  /** 区間の数。値の並びの長さはこれにそろえる。 */
  get segmentCount(): number {
    return this.#shapes.length;
  }

  /**
   * 区間ごとの値 `values`（Worker の traffic_segments）で色を付けた頂点を作る。
   * `fine` が false なら、細部の段の帯は作らない。
   */
  meshes(values: Uint8Array, fine = false): TrafficMeshes {
    let free = 0;
    let busy = 0;
    let bands = 0;
    for (const [i, shape] of this.#shapes.entries()) {
      if (shape === null) {
        continue;
      }
      const value = values[i] ?? 0;
      if (shape.highway) {
        busy++;
        bands += 2;
      } else if (trafficColor(value) !== undefined) {
        if (value === TRAFFIC_FREE) {
          free++;
        } else {
          busy++;
        }
        bands += 2;
      }
    }
    const corners = this.#corners.length;
    const out: TrafficMeshes = {
      free: new TrafficMesh(this.origin, (free + corners) * QUAD_VERTICES),
      busy: new TrafficMesh(this.origin, (busy + corners) * QUAD_VERTICES),
      bands: new TrafficMesh(this.origin, bands * QUAD_VERTICES),
      fineBands: fine ? new TrafficMesh(this.origin, bands * QUAD_VERTICES) : null,
    };
    const uncounted = TRAFFIC_UNCOUNTED_COLOR;
    const uncountedBand: Color = [uncounted[0], uncounted[1], uncounted[2], TRAFFIC_BAND_ALPHA];
    const uncountedFine: Color = [
      uncounted[0],
      uncounted[1],
      uncounted[2],
      TRAFFIC_FINE_BAND_ALPHA,
    ];
    const bandHalf = TRAFFIC_BAND_WIDTH / 2;
    const fineHalf = TRAFFIC_FINE_BAND_WIDTH / 2;
    const fineMesh = out.fineBands;
    for (const [i, shape] of this.#shapes.entries()) {
      if (shape === null) {
        continue;
      }
      if (shape.highway) {
        // 高速道路は混み具合を数えないので、どの段でも灰の破線にする
        writeStrip(out.busy, shape.line, shape.lineHalf, shape.lineMin, uncounted, 0, LINE_DASH_PX);
        for (const band of shape.bands) {
          writeStrip(
            out.bands,
            band,
            bandHalf,
            TRAFFIC_BAND_MIN_PX,
            uncountedBand,
            0,
            BAND_DASH_PX,
          );
        }
        if (fineMesh !== null) {
          for (const band of shape.fineBands) {
            writeStrip(
              fineMesh,
              band,
              fineHalf,
              TRAFFIC_FINE_BAND_MIN_PX,
              uncountedFine,
              0,
              BAND_DASH_PX,
            );
          }
        }
        continue;
      }
      const value = values[i] ?? 0;
      const color = trafficColor(value);
      const banded = trafficColor(value, TRAFFIC_BAND_ALPHA);
      if (color === undefined || banded === undefined) {
        continue;
      }
      const line = value === TRAFFIC_FREE ? out.free : out.busy;
      writeStrip(line, shape.line, shape.lineHalf, shape.lineMin, color, extraOf(value), 0);
      for (const band of shape.bands) {
        if (band.length > 0) {
          writeStrip(out.bands, band, bandHalf, TRAFFIC_BAND_MIN_PX, banded, 0, 0);
        }
      }
      const fineColor = trafficColor(value, TRAFFIC_FINE_BAND_ALPHA);
      if (fineMesh !== null && fineColor !== undefined) {
        for (const band of shape.fineBands) {
          if (band.length > 0) {
            writeStrip(fineMesh, band, fineHalf, TRAFFIC_FINE_BAND_MIN_PX, fineColor, 0, 0);
          }
        }
      }
    }
    for (const c of this.#corners) {
      const value = values[c.first] ?? 0;
      const color = trafficColor(value);
      if (color === undefined || values[c.second] !== value) {
        continue;
      }
      const mesh = value === TRAFFIC_FREE ? out.free : out.busy;
      writeSquare(mesh, c.x, c.y, c.half, c.min, color, extraOf(value));
    }
    return out;
  }
}

/**
 * 画面の側の、交通の主題図の状態。道路網と、Worker から届いた区間ごとの値を、道路網の版で突き合わせて持つ。
 * 版の合わない値（道路網が変わる前の値）では描かない。
 */
export class TrafficLines {
  #net: TrafficNet | null = null;
  #roads: { segments: Float64Array; nodes: Float64Array } | null = null;
  #revision = Number.NaN;
  #values: Uint8Array | null = null;
  #valuesRevision = Number.NaN;
  #changed = false;
  #fine = false;

  /** 道路網が変わった。形は、次に描くときに作り直す（交通を出していないあいだは作らない）。 */
  setRoads(revision: number, segments: Float64Array, nodes: Float64Array): void {
    this.#roads = { segments, nodes };
    this.#net = null;
    this.#revision = revision;
    this.#changed = true;
  }

  /** 区間ごとの値が届いた。前と同じ値なら何もしない。 */
  setValues(revision: number, values: Uint8Array): void {
    const old = this.#values;
    if (
      revision === this.#valuesRevision &&
      old !== null &&
      old.length === values.length &&
      old.every((v, i) => v === values[i])
    ) {
      return;
    }
    this.#values = values;
    this.#valuesRevision = revision;
    this.#changed = true;
  }

  /** 値を捨てる。主題図を交通から外したときに呼ぶ。 */
  clear(): void {
    this.#values = null;
    this.#valuesRevision = Number.NaN;
    this.#changed = true;
  }

  /**
   * 前に呼んでから変わっていれば、新しい頂点を返す。描くものがなければ null、変わっていなければ undefined を返す。
   * `fine` は細部の段の帯を作るかで、細部の段に入ったときは、値が変わっていなくても帯を足して作り直す。
   */
  take(fine = false): TrafficMeshes | null | undefined {
    if (fine !== this.#fine) {
      this.#fine = fine;
      this.#changed ||= fine;
    }
    if (!this.#changed) {
      return undefined;
    }
    this.#changed = false;
    const values = this.#values;
    if (this.#roads === null || values === null || this.#valuesRevision !== this.#revision) {
      return null;
    }
    this.#net ??= new TrafficNet(this.#roads.segments, this.#roads.nodes);
    if (values.length !== this.#net.segmentCount) {
      return null;
    }
    return this.#net.meshes(values, fine);
  }
}
