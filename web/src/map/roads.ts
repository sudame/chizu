// 道路網と、道具の仮の表示を WebGL2 で描く。
// 頂点の位置は、まとめて送った基準のマスからの相対で持つ。遠くの座標でも f32 の桁が足りるようにするためである。

import {
  BUILDING_STRIDE,
  FACILITY_STRIDE,
  NODE_STRIDE,
  SEGMENT_STRIDE,
  SIGNAL_STRIDE,
  SUPPLY_POWER,
  SUPPLY_WATER,
} from "../protocol/messages.ts";
import type { Camera, Viewport } from "./camera.ts";
import {
  carriagewayHalfWidth,
  crossSection,
  HIGHWAY_CODE,
  hasSidewalks,
  roadHalfWidth,
  showsLanes,
} from "./lanes.ts";

/** マスの座標。 */
export type Tile = { x: number; y: number };

/** 縮小しても図形が消えないよう、半分の太さの下限にする画面のピクセル数。 */
export const MIN_PX = 1.5;
/** 車線の区切りなど細い線の、半分の太さの下限（画面のピクセル）。 */
export const THIN_PX = 0.5;

// 半分の太さは、世界の単位の a_half を画面に直した値と、画面のピクセルの下限 a_min の大きいほうにする。
// u_alpha は、拡大したときだけ描く路面の線を、縮尺に合わせて薄くするのに使う
const VERTEX = `#version 300 es
in vec2 a_pos;
in vec2 a_normal;
in float a_half;
in float a_min;
in vec4 a_color;
uniform vec2 u_offset;
uniform float u_zoom;
uniform vec2 u_viewport;
uniform float u_alpha;
out vec4 v_color;
void main() {
  float half_px = max(a_half * u_zoom, a_min);
  vec2 p = (a_pos * u_zoom + u_offset + a_normal * half_px) / (u_viewport * 0.5);
  gl_Position = vec4(p.x, -p.y, 0.0, 1.0);
  v_color = vec4(a_color.rgb, a_color.a * u_alpha);
}`;

const FRAGMENT = `#version 300 es
precision mediump float;
in vec4 v_color;
out vec4 color;
void main() {
  color = v_color;
}`;

/** 頂点 1 つの数の個数。位置 2、法線 2、半分の太さ 1、半分の太さの下限 1、色 4。 */
export const VERTEX_FLOATS = 10;

export type Color = readonly [number, number, number, number];

/** 図形の三角形。頂点は [-1, 1] の範囲の (x, y) で、y は下向きが正である。 */
export type Triangle = readonly [
  readonly [number, number],
  readonly [number, number],
  readonly [number, number],
];

/** 稲妻。上の三角形と下の三角形を、真ん中で重ねる。 */
export const BOLT_SHAPE: readonly Triangle[] = [
  [
    [0.5, -1],
    [-0.9, 0.2],
    [0.3, 0.2],
  ],
  [
    [-0.5, 1],
    [0.9, -0.2],
    [-0.3, -0.2],
  ],
];

/** 水滴。上がとがった三角形と、下の丸みの代わりの台形で作る。 */
export const DROP_SHAPE: readonly Triangle[] = [
  [
    [0, -1],
    [-0.65, 0.2],
    [0.65, 0.2],
  ],
  [
    [-0.65, 0.2],
    [0.65, 0.2],
    [-0.4, 0.95],
  ],
  [
    [0.65, 0.2],
    [0.4, 0.95],
    [-0.4, 0.95],
  ],
];

/**
 * 道路の見た目。CS2 の俯瞰にならい、道路はマスいっぱいに描く。車道の幅は車線の数に合わせ、
 * 車道の外からマスの縁までを歩道（高速道路は橙の路肩）で埋める。車道は広い道路ほど濃くする。
 * `minPx` は、縮小したときに車道の半分の太さが下回らない画面のピクセル数で、広い道路ほど太く残す。
 */
export type RoadStyle = { asphalt: Color; edge: Color; minPx: number };

/** 道路をマスの中心からこの幅（マス）まで描く。マスいっぱいである。 */
export const ROAD_VISUAL_HALF = 0.5;
export const SIDEWALK_COLOR: Color = [0.77, 0.76, 0.73, 1];
export const SHOULDER_COLOR: Color = [0.93, 0.6, 0.2, 1];
// 道路と隣の区画の境目の線
export const CURB_COLOR: Color = [0.42, 0.42, 0.42, 1];
const CURB_HALF = 0.0125;
// 高速道路の車道の半分の幅。車線 2 本の外側（0.45〜0.5 マス）を路肩にする。外側の車線の車は 0.44 マスまでに収まる
const HIGHWAY_ASPHALT_HALF = 0.45;
// 縮小したとき、歩道（路肩）を車道より片側でこのピクセル数だけ広く残す
const EDGE_EXTRA_PX = 0.6;

export const ROAD_STYLES: Record<number, RoadStyle> = {
  1: { asphalt: [0.43, 0.43, 0.45, 1], edge: SIDEWALK_COLOR, minPx: 0.6 },
  2: { asphalt: [0.33, 0.33, 0.36, 1], edge: SIDEWALK_COLOR, minPx: 0.9 },
  3: { asphalt: [0.22, 0.22, 0.25, 1], edge: SIDEWALK_COLOR, minPx: 1.2 },
  [HIGHWAY_CODE]: { asphalt: [0.2, 0.19, 0.19, 1], edge: SHOULDER_COLOR, minPx: 1.5 },
};

export function roadStyle(kindCode: number): RoadStyle {
  return ROAD_STYLES[kindCode] ?? (ROAD_STYLES[1] as RoadStyle);
}

/** 車道として濃く塗る半分の幅（マス）。高速道路は路肩の分だけ細い。 */
export function asphaltHalfWidth(kindCode: number): number {
  return kindCode === HIGHWAY_CODE ? HIGHWAY_ASPHALT_HALF : carriagewayHalfWidth(kindCode);
}

// 斜めの区間は、隣り合う道路のマスが角でしか接しない。車道の帯は角を越えて脇のマスにかかるので、
// 脇のマスの建物（角から 0.15 マス内側）にかからない幅に抑える。歩道は道路のマスを階段状に塗る
const DIAGONAL_ASPHALT_HALF = 0.2;

const isDiagonal = (a: Tile, b: Tile) => a.x !== b.x && a.y !== b.y;

/** 区間 a〜b に描く車道の半分の幅（マス）。斜めの区間では脇のマスの建物にかからない幅に抑える。 */
export function drawnAsphaltHalf(kindCode: number, a: Tile, b: Tile): number {
  const half = asphaltHalfWidth(kindCode);
  return isDiagonal(a, b) ? Math.min(half, DIAGONAL_ASPHALT_HALF) : half;
}

/** 縦横か 45 度の斜めの線 a〜b が通るマスを、a から順に返す。 */
export function tilesOnLine(a: Tile, b: Tile): Tile[] {
  const steps = Math.max(Math.abs(b.x - a.x), Math.abs(b.y - a.y));
  const [sx, sy] = [Math.sign(b.x - a.x), Math.sign(b.y - a.y)];
  return Array.from({ length: steps + 1 }, (_, k) => ({ x: a.x + sx * k, y: a.y + sy * k }));
}

/**
 * 線 a〜b が通るマスを、マスいっぱいの四角で塗る。斜めの線でも道路のマスだけを階段状に塗るので、
 * 押したときに選ばれるマスと見た目がそろう。道路を引くときの仮の表示にも使う。
 */
export function roadFootprint(geometry: Geometry, a: Tile, b: Tile, color: Color, min = 0): void {
  for (const t of tilesOnLine(a, b)) {
    geometry.square(t, ROAD_VISUAL_HALF, color, min);
  }
}

// 路面の線（中央線、車線の区切り）は、この倍率から描き始め、MARKING_FULL_ZOOM で濃さがそろう。
// 縮小したときに細かい線がちらつかないようにするためである
export const MARKING_ZOOM = 10;
export const MARKING_FULL_ZOOM = 16;

/** 倍率 zoom での路面の線の濃さ（0〜1）。 */
export function markingAlpha(zoom: number): number {
  return Math.min(Math.max((zoom - MARKING_ZOOM) / (MARKING_FULL_ZOOM - MARKING_ZOOM), 0), 1);
}

export const CENTER_LINE_COLOR: Color = [0.98, 0.8, 0.22, 0.9];
export const DIVIDER_COLOR: Color = [0.95, 0.95, 0.95, 0.6];

/** 頂点を三角形の列として積む。座標は基準のマスからの相対のマスの単位で、マスの中心は +0.5 にある。 */
export class Geometry {
  readonly #data: number[] = [];
  readonly origin: Tile;

  constructor(origin: Tile) {
    this.origin = origin;
  }

  get vertexCount(): number {
    return this.#data.length / VERTEX_FLOATS;
  }

  #vertex(
    x: number,
    y: number,
    nx: number,
    ny: number,
    half: number,
    min: number,
    color: Color,
  ): void {
    this.#data.push(x - this.origin.x, y - this.origin.y, nx, ny, half, min, ...color);
  }

  /** マス a の中心からマス b の中心への線。半分の太さは画面で `min` ピクセルを下回らない。 */
  segment(a: Tile, b: Tile, half: number, color: Color, min = MIN_PX): void {
    const ax = a.x + 0.5;
    const ay = a.y + 0.5;
    const bx = b.x + 0.5;
    const by = b.y + 0.5;
    const len = Math.hypot(bx - ax, by - ay);
    if (len === 0) {
      return;
    }
    const nx = -(by - ay) / len;
    const ny = (bx - ax) / len;
    const v = (x: number, y: number, s: number) =>
      this.#vertex(x, y, nx * s, ny * s, half, min, color);
    v(ax, ay, 1);
    v(ax, ay, -1);
    v(bx, by, 1);
    v(bx, by, 1);
    v(ax, ay, -1);
    v(bx, by, -1);
  }

  /**
   * マス a の中心からマス b の中心への、中心線から `offset`（マス）だけ横にずらした帯。
   * 半分の太さ `half` は世界の単位で、画面では `min` ピクセルを下回らない。
   * 両端は `trimA` と `trimB`（マス）だけ内側に縮める。`offset` の正は進む向き（a から b）の右である。
   */
  strip(
    a: Tile,
    b: Tile,
    offset: number,
    half: number,
    color: Color,
    min: number,
    trimA = 0,
    trimB = 0,
  ): void {
    const dx = b.x - a.x;
    const dy = b.y - a.y;
    const len = Math.hypot(dx, dy);
    if (len === 0 || len <= trimA + trimB) {
      return;
    }
    const [ux, uy] = [dx / len, dy / len];
    const [nx, ny] = [-uy, ux];
    const ax = a.x + 0.5 + ux * trimA + nx * offset;
    const ay = a.y + 0.5 + uy * trimA + ny * offset;
    const bx = b.x + 0.5 - ux * trimB + nx * offset;
    const by = b.y + 0.5 - uy * trimB + ny * offset;
    const v = (x: number, y: number, s: number) =>
      this.#vertex(x, y, nx * s, ny * s, half, min, color);
    v(ax, ay, 1);
    v(ax, ay, -1);
    v(bx, by, 1);
    v(bx, by, 1);
    v(ax, ay, -1);
    v(bx, by, -1);
  }

  /** マスの中心に置く正方形。 */
  square(t: Tile, half: number, color: Color, min = MIN_PX): void {
    this.dot(t.x + 0.5, t.y + 0.5, half, color, min);
  }

  /** ワールド座標 (x, y)（マスの単位）を中心にした正方形。 */
  dot(x: number, y: number, half: number, color: Color, min = MIN_PX): void {
    const v = (sx: number, sy: number) => this.#vertex(x, y, sx, sy, half, min, color);
    v(-1, -1);
    v(1, -1);
    v(-1, 1);
    v(-1, 1);
    v(1, -1);
    v(1, 1);
  }

  /**
   * 中心 (x, y)（マスの単位）に置く、三角形の組み合わせの図形。点は [-1, 1] の範囲の形の座標で、
   * half の分だけ広げる。縮小しても半分の太さの下限（画面のピクセル）より小さくならない。
   */
  shape(x: number, y: number, half: number, color: Color, triangles: readonly Triangle[]): void {
    for (const t of triangles) {
      for (const [sx, sy] of t) {
        this.#vertex(x, y, sx, sy, half, MIN_PX, color);
      }
    }
  }

  /** 角のマスを両端に含む長方形。inset（マス）だけ内側に縮める。 */
  rect(min: Tile, max: Tile, color: Color, inset = 0): void {
    this.box(min.x + inset, min.y + inset, max.x + 1 - inset, max.y + 1 - inset, color);
  }

  /** ワールド座標の (x0, y0) から (x1, y1) までの長方形。 */
  box(x0: number, y0: number, x1: number, y1: number, color: Color): void {
    const v = (x: number, y: number) => this.#vertex(x, y, 0, 0, 0, 0, color);
    v(x0, y0);
    v(x1, y0);
    v(x0, y1);
    v(x0, y1);
    v(x1, y0);
    v(x1, y1);
  }

  build(): Float32Array {
    return new Float32Array(this.#data);
  }
}

/** 道路の端（歩道を含む）までの半分の幅（マス）。 */
export function halfWidthOf(kindCode: number): number {
  return roadHalfWidth(kindCode);
}

// 区画の色（地面の塗りと凡例も同じ値を使う）。建物の色は、これを暗くし、段階が上がるほど明るくする
export const ZONE_BASE: Record<number, readonly [number, number, number]> = {
  1: [0.15, 0.95, 0.15],
  2: [0.25, 0.45, 0.95],
  3: [0.95, 0.8, 0.2],
  4: [0.3, 0.85, 0.95],
};
const BUILDING_INSET = 0.15;
export const GRASS_COLOR: readonly [number, number, number] = [0.56, 0.72, 0.42];
// 区画を塗った地面は、区画の色を明るい地の色に混ぜた薄い色にし、マスの縁を区画の濃い色の枠で示す。
// ZONE_MIX は地面の、ZONE_EDGE_MIX は枠の、区画の色の割合である
const ZONE_PAPER: readonly [number, number, number] = [0.88, 0.89, 0.85];
export const ZONE_MIX = 0.42;
export const ZONE_EDGE_MIX = 0.85;

const zoneMix = (zone: number, base: readonly number[], k: number): Color => {
  const c = ZONE_BASE[zone] ?? [0.6, 0.6, 0.6];
  const m = (i: number) => (base[i] ?? 0) * (1 - k) + (c[i] ?? 0) * k;
  return [m(0), m(1), m(2), 1];
};

/** 区画を塗った地面の色。 */
export function zoneGroundColor(zone: number): Color {
  return zoneMix(zone, ZONE_PAPER, ZONE_MIX);
}

/** 区画のマスの縁の枠の色。 */
export function zoneEdgeColor(zone: number): Color {
  return zoneMix(zone, GRASS_COLOR, ZONE_EDGE_MIX);
}

/** 段階 level（1〜5）の建物の色。区画の色を少し灰色に寄せ、区画の地面より濃くする。 */
export function buildingColor(zone: number, level: number): Color {
  const [r, g, b] = zoneMix(zone, [0.5, 0.5, 0.5], 0.8);
  const k = 0.42 + 0.09 * Math.min(Math.max(level, 1), 5);
  return [r * k, g * k, b * k, 1];
}

// 建物の縁は本体の色をこの割合に暗くし、幅は BUILDING_EDGE（マス）にする。
// 影は右下へ BUILDING_SHADOW（マス）ずらした半透明の黒で、建物の大きさを見やすくする
const BUILDING_EDGE = 0.05;
const BUILDING_EDGE_SHADE = 0.68;
const BUILDING_SHADOW = 0.08;
const SHADOW_COLOR: Color = [0, 0, 0, 0.28];

// 施設の色。発電所は橙、浄水場は青緑。値は ServiceKind の値である
export const FACILITY_COLOR: Record<number, Color> = {
  1: [0.95, 0.5, 0.1, 1],
  2: [0.1, 0.65, 0.65, 1],
};
// 電気か水道が届いていない建物の印。黄の稲妻と青の水滴を左上に並べる
export const NO_POWER_COLOR: Color = [1, 0.9, 0.1, 1];
export const NO_WATER_COLOR: Color = [0.2, 0.5, 1, 1];
const MARK_HALF = 0.2;

/** 印の中心。建物の左上のマスの中で、電気は左、水道は右に置く。 */
export function markCenter(min: Tile, which: "power" | "water"): { x: number; y: number } {
  return { x: min.x + (which === "power" ? 0.25 : 0.75), y: min.y + 0.3 };
}

const shade = ([r, g, b, a]: Color, k: number): Color => [r * k, g * k, b * k, a];

/**
 * Worker から届いた建物と施設を頂点に直す。建物は隣と見分けられるよう、少し内側に縮めて描く。
 * 下から順に、すべての影、施設と建物の縁と本体、届いていない印を重ねる。
 */
export function buildingGeometry(buildings: Float64Array, facilities: Float64Array): Geometry {
  const geometry = new Geometry({
    x: buildings[0] ?? facilities[0] ?? 0,
    y: buildings[1] ?? facilities[1] ?? 0,
  });
  const boxes: [number, number, number, number, Color][] = [];
  for (let i = 0; i + FACILITY_STRIDE <= facilities.length; i += FACILITY_STRIDE) {
    const [x0 = 0, y0 = 0, x1 = 0, y1 = 0, kind = 0] = facilities.subarray(i, i + FACILITY_STRIDE);
    const color = FACILITY_COLOR[kind] ?? [0.6, 0.6, 0.6, 1];
    boxes.push([x0 + 0.05, y0 + 0.05, x1 + 0.95, y1 + 0.95, color]);
  }
  const marks: [number, number, number][] = [];
  for (let i = 0; i + BUILDING_STRIDE <= buildings.length; i += BUILDING_STRIDE) {
    const [x0 = 0, y0 = 0, x1 = 0, y1 = 0, zone = 0, level = 1, supply = 0] = buildings.subarray(
      i,
      i + BUILDING_STRIDE,
    );
    const color = buildingColor(zone, level);
    const [i0, i1] = [BUILDING_INSET, 1 - BUILDING_INSET];
    boxes.push([x0 + i0, y0 + i0, x1 + i1, y1 + i1, color]);
    marks.push([x0, y0, supply]);
  }
  for (const [x0, y0, x1, y1] of boxes) {
    const d = BUILDING_SHADOW;
    geometry.box(x0 + d, y0 + d, x1 + d, y1 + d, SHADOW_COLOR);
  }
  for (const [x0, y0, x1, y1, color] of boxes) {
    const e = BUILDING_EDGE;
    geometry.box(x0, y0, x1, y1, shade(color, BUILDING_EDGE_SHADE));
    geometry.box(x0 + e, y0 + e, x1 - e, y1 - e, color);
  }
  // 印は建物より上に描く
  for (const [x, y, supply] of marks) {
    if ((supply & SUPPLY_POWER) === 0) {
      const c = markCenter({ x, y }, "power");
      geometry.shape(c.x, c.y, MARK_HALF, NO_POWER_COLOR, BOLT_SHAPE);
    }
    if ((supply & SUPPLY_WATER) === 0) {
      const c = markCenter({ x, y }, "water");
      geometry.shape(c.x, c.y, MARK_HALF, NO_WATER_COLOR, DROP_SHAPE);
    }
  }
  return geometry;
}

/** 電気か水道が届いていない建物の数。 */
export function unsuppliedCount(buildings: Float64Array): number {
  let n = 0;
  for (let i = BUILDING_STRIDE - 1; i < buildings.length; i += BUILDING_STRIDE) {
    if ((buildings[i] ?? 0) !== SUPPLY_POWER + SUPPLY_WATER) {
      n++;
    }
  }
  return n;
}

/** ノードにつながる区間の 1 本。`dx`、`dy` はノードから区間の向こうの端へ向かう単位ベクトルである。 */
type Arm = { segment: number; dx: number; dy: number };

/**
 * 道路のノード。`half` はつながる区間の道路の端（歩道を含む）までのいちばん広い半分の幅、`asphalt` は
 * 濃く塗る車道のいちばん広い半分の幅で、`asphaltColor` はその車道の色である。`minPx` は縮小したときの
 * 車道の太さの下限のいちばん大きい値、`sidewalk` は歩道のある区間がつながっているかである。
 */
type RoadNode = {
  half: number;
  asphalt: number;
  asphaltColor: Color;
  minPx: number;
  sidewalk: boolean;
  arms: Arm[];
};
type RoadNodes = Map<string, RoadNode>;

const nodeKey = (x: number, y: number) => `${x},${y}`;

/** ノードごとに、つながる区間と、いちばん広い区間の幅を求める。 */
function roadNodes(segments: Float64Array): RoadNodes {
  const nodes: RoadNodes = new Map();
  const touch = (x: number, y: number, other: Tile, kind: number, segment: number) => {
    const key = nodeKey(x, y);
    const style = roadStyle(kind);
    const node = nodes.get(key) ?? {
      half: 0,
      asphalt: 0,
      asphaltColor: style.asphalt,
      minPx: 0,
      sidewalk: false,
      arms: [],
    };
    node.half = Math.max(node.half, halfWidthOf(kind));
    if (asphaltHalfWidth(kind) > node.asphalt) {
      node.asphalt = asphaltHalfWidth(kind);
      node.asphaltColor = style.asphalt;
    }
    node.minPx = Math.max(node.minPx, style.minPx);
    node.sidewalk ||= hasSidewalks(kind);
    const len = Math.hypot(other.x - x, other.y - y);
    node.arms.push({ segment, dx: (other.x - x) / len, dy: (other.y - y) / len });
    nodes.set(key, node);
  };
  for (let i = 0, n = 0; i + SEGMENT_STRIDE <= segments.length; i += SEGMENT_STRIDE, n++) {
    const [ax = 0, ay = 0, bx = 0, by = 0, kind = 0] = segments.subarray(i, i + SEGMENT_STRIDE);
    if (ax === bx && ay === by) {
      continue;
    }
    touch(ax, ay, { x: bx, y: by }, kind, n);
    touch(bx, by, { x: ax, y: ay }, kind, n);
  }
  return nodes;
}

const edgeColorOf = (node: RoadNode): Color => (node.sidewalk ? SIDEWALK_COLOR : SHOULDER_COLOR);

type RoadNet = {
  ends: RoadNodes;
  nodeList: { x: number; y: number; node: RoadNode }[];
  /** 区間ごとの両端と種類。 */
  each: (fn: (a: Tile, b: Tile, kind: number, index: number) => void) => void;
};

function roadNet(segments: Float64Array, nodes: Float64Array): RoadNet {
  const ends = roadNodes(segments);
  const nodeList: RoadNet["nodeList"] = [];
  for (let i = 0; i + NODE_STRIDE <= nodes.length; i += NODE_STRIDE) {
    const [x = 0, y = 0] = nodes.subarray(i, i + 2);
    const node = ends.get(nodeKey(x, y));
    if (node !== undefined) {
      nodeList.push({ x, y, node });
    }
  }
  const each: RoadNet["each"] = (fn) => {
    for (let i = 0, n = 0; i + SEGMENT_STRIDE <= segments.length; i += SEGMENT_STRIDE, n++) {
      const [ax = 0, ay = 0, bx = 0, by = 0, kind = 0] = segments.subarray(i, i + SEGMENT_STRIDE);
      fn({ x: ax, y: ay }, { x: bx, y: by }, kind, n);
    }
  };
  return { ends, nodeList, each };
}

/**
 * Worker から届いた道路網の形を頂点に直す。基準のマスは最初のノードにする。縮小したときの描き方で、
 * 区間ごとに、マスいっぱいの歩道（路肩）の色の帯の上に、種類ごとの太さと濃さの車道の帯を重ねる。
 * 縮小しても種類を見分けられるよう、帯の太さの下限を種類ごとに変える。
 */
export function roadGeometry(segments: Float64Array, nodes: Float64Array): Geometry {
  const geometry = new Geometry({ x: nodes[0] ?? 0, y: nodes[1] ?? 0 });
  const { nodeList, each } = roadNet(segments, nodes);
  each((a, b, kind) => {
    const style = roadStyle(kind);
    if (isDiagonal(a, b)) {
      roadFootprint(geometry, a, b, style.edge, style.minPx + EDGE_EXTRA_PX);
    } else {
      geometry.segment(a, b, ROAD_VISUAL_HALF, style.edge, style.minPx + EDGE_EXTRA_PX);
    }
  });
  for (const { x, y, node } of nodeList) {
    geometry.square({ x, y }, ROAD_VISUAL_HALF, edgeColorOf(node), node.minPx + EDGE_EXTRA_PX);
  }
  each((a, b, kind) => {
    const style = roadStyle(kind);
    geometry.segment(a, b, drawnAsphaltHalf(kind, a, b), style.asphalt, style.minPx);
  });
  for (const { x, y, node } of nodeList) {
    geometry.square({ x, y }, node.asphalt, node.asphaltColor, node.minPx);
  }
  return geometry;
}

// これより急に折り返す角では、継ぎ目の線を縮めも伸ばしもしない
const MAX_MITER_ANGLE = 2.4;

/**
 * ちょうど 2 本がつながる角で、帯が折れ曲がりに沿ってつながるよう、端をどれだけ縮めるか（マス）。
 * 中心線から `offset` だけ横にずらした帯の端が、角の内側では縮み、外側では伸びる。負なら伸ばす。
 * `first` から `second` へ折れ曲がる道筋の上で、2 本の帯がつながる位置である。
 */
export function miterTrim(
  first: { x: number; y: number },
  second: { x: number; y: number },
  offset: number,
): number {
  const angle = Math.atan2(
    first.x * second.y - first.y * second.x,
    first.x * second.x + first.y * second.y,
  );
  if (Math.abs(angle) > MAX_MITER_ANGLE) {
    return 0;
  }
  return offset * Math.tan(angle / 2);
}

/**
 * 帯の端を縮める長さ（マス）を求める関数を作る。`atEnd` は区間の終わり側のノードかどうか。
 * 3 本以上が集まる交差点では `crossing(node)` だけ縮め（斜めの区間は √2 倍）、2 本だけがつながる角では
 * 折れ曲がりに沿ってつなぐ。行き止まりでは縮めない。
 */
function trimmer(segments: Float64Array, ends: RoadNodes) {
  return (
    index: number,
    atEnd: boolean,
    offset: number,
    crossing: (node: RoadNode) => number,
  ): number => {
    const [ax = 0, ay = 0, bx = 0, by = 0] = segments.subarray(
      index * SEGMENT_STRIDE,
      (index + 1) * SEGMENT_STRIDE,
    );
    const [x, y] = atEnd ? [bx, by] : [ax, ay];
    const node = ends.get(nodeKey(x, y));
    if (node === undefined) {
      return 0;
    }
    if (node.arms.length >= 3) {
      const t = crossing(node);
      return ax !== bx && ay !== by ? t * Math.SQRT2 : t;
    }
    const other = node.arms.find((arm) => arm.segment !== index);
    if (other === undefined) {
      return 0;
    }
    const len = Math.hypot(bx - ax, by - ay);
    const u = { x: (bx - ax) / len, y: (by - ay) / len };
    return atEnd
      ? miterTrim(u, { x: other.dx, y: other.dy }, offset)
      : miterTrim({ x: -other.dx, y: -other.dy }, u, offset);
  };
}

/**
 * 拡大したときの、車線ごとの道路の描き方。下から順に、ノードのマスを埋める歩道（路肩）の色の四角、
 * 車道の外からマスの縁までの歩道（路肩）の帯、隣の区画との境目の線、車道、ノードの車道の四角を重ねる。
 * 交差点の四角は車道の幅だけを埋めるので、交差点の角には歩道が残る。
 * 路面の線は、縮尺に合わせて薄くするので roadMarkingGeometry で別に作る。
 */
export function roadLaneGeometry(segments: Float64Array, nodes: Float64Array): Geometry {
  const geometry = new Geometry({ x: nodes[0] ?? 0, y: nodes[1] ?? 0 });
  const { ends, nodeList, each } = roadNet(segments, nodes);
  const trimOf = trimmer(segments, ends);

  for (const { x, y, node } of nodeList) {
    geometry.square({ x, y }, ROAD_VISUAL_HALF, edgeColorOf(node), 0);
  }
  // 歩道（路肩）の帯。交差点ではノードの四角が埋めるので縮めない。斜めの区間は道路のマスを階段状に塗る
  each((a, b, kind, n) => {
    if (isDiagonal(a, b)) {
      roadFootprint(geometry, a, b, roadStyle(kind).edge);
      return;
    }
    const inner = asphaltHalfWidth(kind);
    const half = (ROAD_VISUAL_HALF - inner) / 2;
    for (const side of [-1, 1]) {
      const offset = side * (inner + half);
      const trimA = trimOf(n, false, offset, () => 0);
      const trimB = trimOf(n, true, offset, () => 0);
      geometry.strip(a, b, offset, half, roadStyle(kind).edge, 0, trimA, trimB);
    }
  });
  // 歩道と隣の区画との境目の線。交差点では、ほかの腕の道路に入らないようマスの縁で止め、
  // 腕のないマスの辺に引く。高速道路は路肩の色で縁が分かるので引かない。斜めの区間には引かない
  each((a, b, kind, n) => {
    if (!hasSidewalks(kind) || isDiagonal(a, b)) {
      return;
    }
    for (const side of [-1, 1]) {
      const offset = side * (ROAD_VISUAL_HALF - CURB_HALF);
      const trimA = trimOf(n, false, offset, () => ROAD_VISUAL_HALF);
      const trimB = trimOf(n, true, offset, () => ROAD_VISUAL_HALF);
      geometry.strip(a, b, offset, CURB_HALF, CURB_COLOR, THIN_PX, trimA, trimB);
    }
  });
  for (const { x, y, node } of nodeList) {
    if (node.sidewalk && node.arms.length !== 2) {
      nodeCurbs(geometry, { x, y }, node);
    }
  }
  each((a, b, kind) => {
    geometry.strip(a, b, 0, drawnAsphaltHalf(kind, a, b), roadStyle(kind).asphalt, 0);
  });
  for (const { x, y, node } of nodeList) {
    geometry.square({ x, y }, node.asphalt, node.asphaltColor, 0);
  }
  return geometry;
}

// ノードのマスの 4 つの辺。`dx`、`dy` は辺の外向き、`a`〜`b` は辺に沿う線（マスの中心からの相対）、
// `offset` はその線から辺までのずれである
const NODE_SIDES = [
  { dx: 0, dy: -1, a: [-0.5, 0], b: [0.5, 0], offset: -1 },
  { dx: 0, dy: 1, a: [-0.5, 0], b: [0.5, 0], offset: 1 },
  { dx: -1, dy: 0, a: [0, -0.5], b: [0, 0.5], offset: 1 },
  { dx: 1, dy: 0, a: [0, -0.5], b: [0, 0.5], offset: -1 },
] as const;

/** 行き止まりと交差点のノードの、縦横の腕がない辺に、隣の区画との境目の線を引く。 */
function nodeCurbs(geometry: Geometry, at: Tile, node: RoadNode): void {
  for (const side of NODE_SIDES) {
    if (node.arms.some((arm) => arm.dx === side.dx && arm.dy === side.dy)) {
      continue;
    }
    const a = { x: at.x + side.a[0], y: at.y + side.a[1] };
    const b = { x: at.x + side.b[0], y: at.y + side.b[1] };
    const offset = side.offset * (ROAD_VISUAL_HALF - CURB_HALF);
    geometry.strip(a, b, offset, CURB_HALF, CURB_COLOR, THIN_PX);
  }
}

const MARKING_COLOR: Record<"center" | "divider", Color> = {
  center: CENTER_LINE_COLOR,
  divider: DIVIDER_COLOR,
};

/**
 * 拡大したときに車道の上に描く路面の線。中央線と車線の区切りと、交差点の横断歩道の縞である。
 * 3 本以上が集まる交差点では、線を交差点の手前（横断歩道の始まり）で止める。2 本だけがつながる角では、
 * 線を折れ曲がりに沿ってつなぐ。
 */
export function roadMarkingGeometry(segments: Float64Array, nodes: Float64Array): Geometry {
  const geometry = new Geometry({ x: nodes[0] ?? 0, y: nodes[1] ?? 0 });
  const { ends, nodeList, each } = roadNet(segments, nodes);
  const trimOf = trimmer(segments, ends);
  each((a, b, kind, n) => {
    for (const strip of crossSection(kind)) {
      if (strip.role !== "center" && strip.role !== "divider") {
        continue;
      }
      // 斜めの区間では、細く描いた車道からはみ出す区切りの線を描かない
      if (Math.abs(strip.offset) + strip.half > drawnAsphaltHalf(kind, a, b)) {
        continue;
      }
      const trimA = trimOf(n, false, strip.offset, (node) => node.half);
      const trimB = trimOf(n, true, strip.offset, (node) => node.half);
      const color = MARKING_COLOR[strip.role];
      geometry.strip(a, b, strip.offset, strip.half, color, THIN_PX, trimA, trimB);
    }
  });
  // 交差点の腕ごとに、交差点の手前の車道を横切る横断歩道の縞を描く。高速道路の腕には横断歩道がない
  for (const { x, y, node } of nodeList) {
    if (node.arms.length < 3) {
      continue;
    }
    for (const arm of node.arms) {
      const kind = segments[arm.segment * SEGMENT_STRIDE + 4] ?? 0;
      if (kind !== HIGHWAY_CODE) {
        const diagonal = arm.dx !== 0 && arm.dy !== 0;
        const carriage = carriagewayHalfWidth(kind);
        const width = diagonal ? Math.min(carriage, DIAGONAL_ASPHALT_HALF) : carriage;
        zebra(geometry, { x, y }, arm, node.half, width);
      }
    }
  }
  return geometry;
}

export const ZEBRA_COLOR: Color = [0.95, 0.95, 0.95, 0.9];
// 縞の、腕の向きの長さと、横の間隔と太さの半分（マス）
const ZEBRA_LENGTH = 0.16;
const ZEBRA_SPACING = 0.14;
const ZEBRA_HALF = 0.035;

/**
 * 交差点 `at` の腕 `arm` の横断歩道。道路の端（歩道を含む、交差点から `half`）から腕の向きへ
 * `ZEBRA_LENGTH` の長さで、車道の幅（中心線から両側へ `carriage`）に縞を並べる。
 * 交差点の角には歩道が残るので、縞は角の歩道のあいだの車道を渡る。
 */
export function zebra(
  geometry: Geometry,
  at: Tile,
  arm: { dx: number; dy: number },
  half: number,
  carriage: number,
): void {
  const diagonal = arm.dx !== 0 && arm.dy !== 0;
  const sx = Math.sign(arm.dx);
  const sy = Math.sign(arm.dy);
  const next = { x: at.x + sx, y: at.y + sy };
  const len = Math.hypot(sx, sy);
  const start = diagonal ? half * Math.SQRT2 : half;
  const count = Math.max(1, Math.floor((2 * carriage) / ZEBRA_SPACING));
  const first = -((count - 1) * ZEBRA_SPACING) / 2;
  for (let k = 0; k < count; k++) {
    const offset = first + k * ZEBRA_SPACING;
    geometry.strip(
      at,
      next,
      offset,
      ZEBRA_HALF,
      ZEBRA_COLOR,
      THIN_PX,
      start,
      len - start - ZEBRA_LENGTH,
    );
  }
}

// 信号の灯火の色。値は chizu-core の Light の値（0 青、1 黄、2 赤）である
export const SIGNAL_COLOR: Record<number, Color> = {
  0: [0.2, 0.9, 0.35, 1],
  1: [1, 0.8, 0.1, 1],
  2: [0.95, 0.2, 0.2, 1],
};
const SIGNAL_BASE_COLOR: Color = [0.12, 0.12, 0.14, 1];
const SIGNAL_BASE_HALF = 0.16;
const SIGNAL_LAMP_HALF = 0.1;
// 灯火を置く位置。交差点の中心から腕の向きへ ALONG、入ってくる車線の側へ SIDE（マス）だけずらす
const SIGNAL_ALONG = 0.42;
const SIGNAL_SIDE = 0.2;

/**
 * 腕 1 本の灯火を置く位置（マスの単位）。ノード (x, y) の中心から腕の向き (dx, dy) へ進め、
 * 腕から入ってくる車の車線の側に寄せる。左側通行で y は下向きなので、入ってくる向き (-dx, -dy) の左は (-dy, dx) である。
 */
export function signalLampCenter(x: number, y: number, dx: number, dy: number): Tile {
  const len = Math.hypot(dx, dy) || 1;
  const [ux, uy] = [dx / len, dy / len];
  return {
    x: x + 0.5 + ux * SIGNAL_ALONG - uy * SIGNAL_SIDE,
    y: y + 0.5 + uy * SIGNAL_ALONG + ux * SIGNAL_SIDE,
  };
}

/** Worker から届いた信号の灯火を頂点に直す。腕ごとに、暗い台の上に灯火の色の四角を置く。 */
export function signalGeometry(lights: Float64Array): Geometry {
  const geometry = new Geometry({ x: lights[0] ?? 0, y: lights[1] ?? 0 });
  for (let i = 0; i + SIGNAL_STRIDE <= lights.length; i += SIGNAL_STRIDE) {
    const [x = 0, y = 0, dx = 0, dy = 0, light = 2] = lights.subarray(i, i + SIGNAL_STRIDE);
    const c = signalLampCenter(x, y, dx, dy);
    geometry.dot(c.x, c.y, SIGNAL_BASE_HALF, SIGNAL_BASE_COLOR);
    geometry.dot(c.x, c.y, SIGNAL_LAMP_HALF, SIGNAL_COLOR[light] ?? SIGNAL_BASE_COLOR);
  }
  return geometry;
}

/** 縮小したときの描き方と、拡大したときの車線ごとの描き方と、その上に重ねる路面の線。 */
export type RoadMeshes = { simple: Geometry; lanes: Geometry; markings: Geometry };

export function roadMeshes(segments: Float64Array, nodes: Float64Array): RoadMeshes {
  return {
    simple: roadGeometry(segments, nodes),
    lanes: roadLaneGeometry(segments, nodes),
    markings: roadMarkingGeometry(segments, nodes),
  };
}

type Mesh = { buffer: WebGLBuffer; vao: WebGLVertexArrayObject; count: number; origin: Tile };

// 道路と建物は、1 つ変わるたびに全体を受け取り直して頂点を作り直す。重くなったらチャンクごとのメッシュに分ける。
export class RoadLayer {
  readonly #gl: WebGL2RenderingContext;
  readonly #program: WebGLProgram;
  readonly #u: Record<"offset" | "zoom" | "viewport" | "alpha", WebGLUniformLocation | null>;
  readonly #buildings: Mesh;
  readonly #roads: Mesh;
  readonly #lanes: Mesh;
  readonly #markings: Mesh;
  readonly #signals: Mesh;
  readonly #preview: Mesh;

  constructor(
    gl: WebGL2RenderingContext,
    link: (vertex: string, fragment: string) => WebGLProgram,
  ) {
    this.#gl = gl;
    this.#program = link(VERTEX, FRAGMENT);
    this.#u = {
      offset: gl.getUniformLocation(this.#program, "u_offset"),
      zoom: gl.getUniformLocation(this.#program, "u_zoom"),
      viewport: gl.getUniformLocation(this.#program, "u_viewport"),
      alpha: gl.getUniformLocation(this.#program, "u_alpha"),
    };
    this.#buildings = this.#mesh();
    this.#roads = this.#mesh();
    this.#lanes = this.#mesh();
    this.#markings = this.#mesh();
    this.#signals = this.#mesh();
    this.#preview = this.#mesh();
  }

  #mesh(): Mesh {
    const gl = this.#gl;
    const vao = gl.createVertexArray();
    const buffer = gl.createBuffer();
    gl.bindVertexArray(vao);
    gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
    const stride = VERTEX_FLOATS * 4;
    const attributes: [string, number, number][] = [
      ["a_pos", 2, 0],
      ["a_normal", 2, 2],
      ["a_half", 1, 4],
      ["a_min", 1, 5],
      ["a_color", 4, 6],
    ];
    for (const [name, size, offset] of attributes) {
      const location = gl.getAttribLocation(this.#program, name);
      gl.enableVertexAttribArray(location);
      gl.vertexAttribPointer(location, size, gl.FLOAT, false, stride, offset * 4);
    }
    gl.bindVertexArray(null);
    return { buffer, vao, count: 0, origin: { x: 0, y: 0 } };
  }

  #upload(mesh: Mesh, geometry: Geometry): void {
    const gl = this.#gl;
    gl.bindBuffer(gl.ARRAY_BUFFER, mesh.buffer);
    gl.bufferData(gl.ARRAY_BUFFER, geometry.build(), gl.DYNAMIC_DRAW);
    mesh.count = geometry.vertexCount;
    mesh.origin = geometry.origin;
  }

  setBuildings(geometry: Geometry): void {
    this.#upload(this.#buildings, geometry);
  }

  setRoads(meshes: RoadMeshes): void {
    this.#upload(this.#roads, meshes.simple);
    this.#upload(this.#lanes, meshes.lanes);
    this.#upload(this.#markings, meshes.markings);
  }

  setSignals(geometry: Geometry): void {
    this.#upload(this.#signals, geometry);
  }

  setPreview(geometry: Geometry | null): void {
    this.#upload(this.#preview, geometry ?? new Geometry({ x: 0, y: 0 }));
  }

  draw(camera: Camera, viewport: Viewport): void {
    const gl = this.#gl;
    gl.useProgram(this.#program);
    gl.uniform1f(this.#u.zoom, camera.zoom);
    gl.uniform2f(this.#u.viewport, viewport.width, viewport.height);
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
    const lanes = showsLanes(camera.zoom);
    const marking = lanes ? markingAlpha(camera.zoom) : 0;
    const meshes: [Mesh, number][] = [
      [this.#buildings, 1],
      [lanes ? this.#lanes : this.#roads, 1],
      [this.#markings, marking],
      [this.#signals, 1],
      [this.#preview, 1],
    ];
    for (const [mesh, alpha] of meshes) {
      if (mesh.count === 0 || alpha === 0) {
        continue;
      }
      gl.uniform1f(this.#u.alpha, alpha);
      gl.uniform2f(
        this.#u.offset,
        (mesh.origin.x - camera.x) * camera.zoom,
        (mesh.origin.y - camera.y) * camera.zoom,
      );
      gl.bindVertexArray(mesh.vao);
      gl.drawArrays(gl.TRIANGLES, 0, mesh.count);
    }
    gl.bindVertexArray(null);
    gl.disable(gl.BLEND);
  }
}
