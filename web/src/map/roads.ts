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
  SIDEWALK_WIDTH,
} from "./lanes.ts";
import { type Color, GROUND_COLOR, hex } from "./palette.ts";
import { stageWeights } from "./stages.ts";

export type { Color };

/** マスの座標。 */
export type Tile = { x: number; y: number };

/** 縮小しても図形が消えないよう、半分の太さの下限にする画面のピクセル数。 */
export const MIN_PX = 1.5;
/** 車線の区切りなど細い線の、半分の太さの下限（画面のピクセル）。 */
export const THIN_PX = 0.5;

// 半分の太さは、世界の単位の a_half を画面に直した値と、画面のピクセルの下限 a_min の大きいほうに、
// 画面のピクセルの a_extra を足した値にする。a_dash が正なら、a_along（世界の距離）に沿って、
// 画面で a_dash ピクセルの周期の破線にする。
// u_alpha は、拡大したときだけ描く路面の線を、縮尺に合わせて薄くするのに使う。
// u_fade は、色を地面の色から本来の色へ寄せる割合で、引いたときの小の道路を薄く描くのに使う。
// 不透明のまま色を薄めるので、帯とノードの四角が重なる所だけ濃くなることがない
const glslVec3 = ([r, g, b]: readonly number[]) => `vec3(${r}, ${g}, ${b})`;

const VERTEX = `#version 300 es
in vec2 a_pos;
in vec2 a_normal;
in float a_half;
in float a_min;
in vec4 a_color;
in float a_extra;
in float a_along;
in float a_dash;
uniform vec2 u_offset;
uniform float u_zoom;
uniform vec2 u_viewport;
uniform float u_alpha;
uniform float u_fade;
out vec4 v_color;
out float v_distance;
flat out float v_dash;
void main() {
  float half_px = max(a_half * u_zoom, a_min) + a_extra;
  vec2 p = (a_pos * u_zoom + u_offset + a_normal * half_px) / (u_viewport * 0.5);
  gl_Position = vec4(p.x, -p.y, 0.0, 1.0);
  v_color = vec4(mix(${glslVec3(GROUND_COLOR)}, a_color.rgb, u_fade), a_color.a * u_alpha);
  v_distance = a_along * u_zoom;
  v_dash = a_dash;
}`;

const FRAGMENT = `#version 300 es
precision mediump float;
in vec4 v_color;
in float v_distance;
flat in float v_dash;
out vec4 color;
void main() {
  // 破線は、discard せずに不透明度で抜く（実線は v_dash が 0）
  float on = v_dash > 0.0 ? step(mod(v_distance, v_dash), v_dash * 0.5) : 1.0;
  color = vec4(v_color.rgb, v_color.a * on);
}`;

/** 頂点 1 つの数の個数。位置 2、法線 2、半分の太さ 1、半分の太さの下限 1、色 4、足す太さ 1、破線の位置 1、破線の周期 1。 */
export const VERTEX_FLOATS = 13;

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
 * 道路の見た目（docs/map-style.md の「道路」）。`asphalt` は車道の塗り、`edge` は車道の縁の色である。
 * 縮小したときは、縁の色の帯の上に塗りの帯を重ねて縁取りにする。拡大したときは、車道の端（縁石）の線にする。
 * `minPx` は、縮小したときに車道の半分の太さが下回らない画面のピクセル数で、広い道路ほど太く残す。
 */
export type RoadStyle = { asphalt: Color; edge: Color; minPx: number };

/** 道路をマスの中心からこの幅（マス）まで描く。マスいっぱいである。 */
export const ROAD_VISUAL_HALF = 0.5;
export const SIDEWALK_COLOR = hex("#e6e2d8");
/** 歩道の外の縁の線。 */
export const SIDEWALK_EDGE_COLOR = hex("#cfc8ba");
/** 歩道の外からマスの縁までの植え込み。 */
export const PLANTING_COLOR = hex("#e4eedb");
// 高速道路の車道の半分の幅。車線 2 本の外側（0.45〜0.5 マス）を、縁の色の路肩にする。外側の車線の車は 0.44 マスまでに収まる
const HIGHWAY_ASPHALT_HALF = 0.45;
// 縮小したとき、縁の帯を塗りの帯より片側でこのピクセル数だけ広く残す
export const EDGE_EXTRA_PX = 1;

// 縮小したときの線の、世界の幅の半分（マス）。決まりの表の幅（小 0.62、中 0.80、大 0.96、高速 1.00）の半分である。
// 画面での太さは `max(下限, 世界の幅 × 倍率)` で、下限は RoadStyle の minPx（表の下限の半分）にする
const LINE_HALF: Record<number, number> = { 1: 0.31, 2: 0.4, 3: 0.48, [HIGHWAY_CODE]: 0.5 };

/** 縮小したときの線の、世界の半分の幅（マス）。斜めの区間は脇のマスの建物にかからない幅に抑える。 */
export function lineHalfWidth(kindCode: number, a: Tile, b: Tile): number {
  const half = LINE_HALF[kindCode] ?? 0.31;
  return isDiagonal(a, b) ? Math.min(half, DIAGONAL_ASPHALT_HALF) : half;
}

export const ROAD_STYLES: Record<number, RoadStyle> = {
  1: { asphalt: hex("#ffffff"), edge: hex("#c8c2b6"), minPx: 0.9 },
  2: { asphalt: hex("#ffffff"), edge: hex("#b8b2a5"), minPx: 1.1 },
  3: { asphalt: hex("#ffe69a"), edge: hex("#ddb04a"), minPx: 1.6 },
  [HIGHWAY_CODE]: { asphalt: hex("#f7b267"), edge: hex("#d4843a"), minPx: 2.0 },
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

/** 種類ごとの中央線と車線の区切りの色。小の道路には中央線も区切りもない。 */
export const MARKING_COLORS: Record<number, { center: Color; divider: Color } | undefined> = {
  2: { center: hex("#d9d3c7"), divider: hex("#e6e1d7") },
  3: { center: hex("#e0b450"), divider: hex("#f0cf7c") },
  [HIGHWAY_CODE]: { center: hex("#ffffff"), divider: hex("#fde3c4") },
};

/**
 * 線 a〜b が通るマスの縁。縦横の線は 1 つの長方形の縁にし、斜めの線と 1 マスは、通るマスごとの縁にする。
 * `dashPx` が正なら、画面でその周期の破線にする。
 */
export function footprintOutline(
  geometry: Geometry,
  a: Tile,
  b: Tile,
  color: Color,
  px: number,
  dashPx = 0,
): void {
  if (isDiagonal(a, b)) {
    for (const t of tilesOnLine(a, b)) {
      geometry.outline(t.x, t.y, t.x + 1, t.y + 1, color, px, dashPx);
    }
    return;
  }
  geometry.outline(
    Math.min(a.x, b.x),
    Math.min(a.y, b.y),
    Math.max(a.x, b.x) + 1,
    Math.max(a.y, b.y) + 1,
    color,
    px,
    dashPx,
  );
}

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
    extra = 0,
    along = 0,
    dash = 0,
  ): void {
    this.#data.push(
      x - this.origin.x,
      y - this.origin.y,
      nx,
      ny,
      half,
      min,
      ...color,
      extra,
      along,
      dash,
    );
  }

  /** マス a の中心からマス b の中心への線。半分の太さは画面で `min` ピクセルを下回らない。 */
  segment(a: Tile, b: Tile, half: number, color: Color, min = MIN_PX, extra = 0): void {
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
      this.#vertex(x, y, nx * s, ny * s, half, min, color, extra);
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
  square(t: Tile, half: number, color: Color, min = MIN_PX, extra = 0): void {
    this.dot(t.x + 0.5, t.y + 0.5, half, color, min, extra);
  }

  /** ワールド座標 (x, y)（マスの単位）を中心にした正方形。 */
  dot(x: number, y: number, half: number, color: Color, min = MIN_PX, extra = 0): void {
    const v = (sx: number, sy: number) => this.#vertex(x, y, sx, sy, half, min, color, extra);
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

  /**
   * ワールド座標の (x0, y0) から (x1, y1) までの長方形。`shiftPx` があれば、右下へ画面でそのピクセル数だけ
   * ずらす（縮尺によらない）。
   */
  box(x0: number, y0: number, x1: number, y1: number, color: Color, shiftPx = 0): void {
    const n = shiftPx === 0 ? 0 : 1;
    const v = (x: number, y: number) => this.#vertex(x, y, n, n, 0, shiftPx, color);
    v(x0, y0);
    v(x1, y0);
    v(x0, y1);
    v(x0, y1);
    v(x1, y0);
    v(x1, y1);
  }

  /**
   * ワールド座標の (x0, y0) から (x1, y1) までの長方形の縁。太さは画面で `px` ピクセルで、縮尺によらない。
   * 辺の真ん中に線を引くので、半分は長方形の外にはみ出す。
   */
  outline(
    x0: number,
    y0: number,
    x1: number,
    y1: number,
    color: Color,
    px: number,
    dashPx = 0,
  ): void {
    const h = px / 2;
    const v = (x: number, y: number, nx: number, ny: number, along: number) =>
      this.#vertex(x, y, nx, ny, 0, h, color, 0, along, dashPx);
    // 辺ごとに、外側と内側の 2 点ずつを両端に置く。角では隣の辺と重なって、角まで塗る
    const edges: [number, number, number, number, number, number][] = [
      [x0, y0, x1, y0, 0, -1],
      [x1, y0, x1, y1, 1, 0],
      [x1, y1, x0, y1, 0, 1],
      [x0, y1, x0, y0, -1, 0],
    ];
    for (const [ax, ay, bx, by, nx, ny] of edges) {
      // 辺の向き (dx, dy) に沿って両端を h だけ外へ伸ばす
      const dx = Math.sign(bx - ax);
      const dy = Math.sign(by - ay);
      const corner = (x: number, y: number, along: number, side: number) =>
        v(x, y, dx * along + nx * side, dy * along + ny * side, dx !== 0 ? x : y);
      corner(ax, ay, -1, 1);
      corner(ax, ay, -1, -1);
      corner(bx, by, 1, 1);
      corner(bx, by, 1, 1);
      corner(ax, ay, -1, -1);
      corner(bx, by, 1, -1);
    }
  }

  build(): Float32Array {
    return new Float32Array(this.#data);
  }
}

/** 道路の端（歩道を含む）までの半分の幅（マス）。 */
export function halfWidthOf(kindCode: number): number {
  return roadHalfWidth(kindCode);
}

/** 区画の色。住宅は緑、商業は青、工業は黄、オフィスは水色（CS2 にならう）。凡例と道具の仮の表示も同じ値を使う。 */
type ZoneColors = {
  /** 区画の地面（標準と寄り）。 */
  ground: Color;
  /** 街区の面・空き（引き）。 */
  blockEmpty: Color;
  /** 街区の面・建物（引き。区画を塗る仮の表示にも使う）。 */
  block: Color;
  building: Color;
  edge: Color;
};

export const ZONE_COLORS: Record<number, ZoneColors> = {
  1: {
    ground: hex("#eef4e6"),
    blockEmpty: hex("#e3edd7"),
    block: hex("#cde0b9"),
    building: hex("#d3e3c0"),
    edge: hex("#a7c18e"),
  },
  2: {
    ground: hex("#eaf0f8"),
    blockEmpty: hex("#dde6f3"),
    block: hex("#bdcfeb"),
    building: hex("#c9d7ee"),
    edge: hex("#91a9d4"),
  },
  3: {
    ground: hex("#f5efdf"),
    blockEmpty: hex("#efe5c9"),
    block: hex("#e1cd99"),
    building: hex("#e6d7ae"),
    edge: hex("#c0a462"),
  },
  4: {
    ground: hex("#e7f3f4"),
    blockEmpty: hex("#d8ebed"),
    block: hex("#b3dade"),
    building: hex("#c2e0e3"),
    edge: hex("#84bbc1"),
  },
};
const UNKNOWN_ZONE: ZoneColors = {
  ground: hex("#f4f2ec"),
  blockEmpty: hex("#e6e6e6"),
  block: hex("#d0d0d0"),
  building: hex("#d8d8d8"),
  edge: hex("#a8a8a8"),
};
const zoneColors = (zone: number): ZoneColors => ZONE_COLORS[zone] ?? UNKNOWN_ZONE;
const BUILDING_INSET = 0.15;

/** 区画の地面の色。 */
export function zoneGroundColor(zone: number): Color {
  return zoneColors(zone).ground;
}

/** 区画を塗る仮の表示の色。街区の面・建物の色の 70% である。 */
export function zoneBlockColor(zone: number): Color {
  const [r, g, b] = zoneColors(zone).block;
  return [r, g, b, 0.7];
}

/** 建物の色。建物の段階（1〜5）は色では分けない。 */
export function buildingColor(zone: number): Color {
  return zoneColors(zone).building;
}

/** 建物の縁の色。 */
export function buildingEdgeColor(zone: number): Color {
  return zoneColors(zone).edge;
}

// 建物の縁の幅は BUILDING_EDGE（マス）にする。
// 影は右下へ画面で BUILDING_SHADOW_PX ずらした `#3c4043` の 18% で、建物の大きさを見やすくする
const BUILDING_EDGE = 0.05;
const BUILDING_EDGE_SHADE = 0.68;
const BUILDING_SHADOW_PX = 2;
const SHADOW_COLOR = hex("#3c4043", 0.18);

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

type Box = { x0: number; y0: number; x1: number; y1: number; color: Color; edge: Color };

/** 標準の段で、建物のまとまりを描くときに内側に縮める幅（マス）。隣の建物とのあいだに細い隙間だけを残す。 */
export const CLUSTER_INSET = 0.06;

/**
 * 建物と施設の頂点。段ごとに濃さを変えて重ねるので、描く順に分けて持つ。
 * - `clusters`：標準の段の、建物のまとまり（隙間を細く、縁なし）
 * - `details`：寄りの段の、建物と施設の影と、建物の縁
 * - `facilities`：施設の縁と本体。どの段でも描く
 * - `bodies`：寄りの段の、1 軒ずつの建物の本体
 * - `marks`：電気か水道が届いていない印
 */
export type BuildingMeshes = {
  clusters: Geometry;
  details: Geometry;
  facilities: Geometry;
  bodies: Geometry;
  marks: Geometry;
};

/**
 * Worker から届いた建物と施設を頂点に直す。寄りの段では、建物は隣と見分けられるよう少し内側に縮め、
 * 影と縁を付ける。標準の段では、隙間を細くした縁なしの面にする。引きの段の街区の面は、チャンクのシェーダーで塗る。
 */
export function buildingMeshes(buildings: Float64Array, facilities: Float64Array): BuildingMeshes {
  const origin = {
    x: buildings[0] ?? facilities[0] ?? 0,
    y: buildings[1] ?? facilities[1] ?? 0,
  };
  const meshes: BuildingMeshes = {
    clusters: new Geometry(origin),
    details: new Geometry(origin),
    facilities: new Geometry(origin),
    bodies: new Geometry(origin),
    marks: new Geometry(origin),
  };
  const facilityBoxes: Box[] = [];
  for (let i = 0; i + FACILITY_STRIDE <= facilities.length; i += FACILITY_STRIDE) {
    const [x0 = 0, y0 = 0, x1 = 0, y1 = 0, kind = 0] = facilities.subarray(i, i + FACILITY_STRIDE);
    const color = FACILITY_COLOR[kind] ?? [0.6, 0.6, 0.6, 1];
    facilityBoxes.push({
      x0: x0 + 0.05,
      y0: y0 + 0.05,
      x1: x1 + 0.95,
      y1: y1 + 0.95,
      color,
      edge: shade(color, BUILDING_EDGE_SHADE),
    });
  }
  const buildingBoxes: Box[] = [];
  const marks: [number, number, number][] = [];
  for (let i = 0; i + BUILDING_STRIDE <= buildings.length; i += BUILDING_STRIDE) {
    const [x0 = 0, y0 = 0, x1 = 0, y1 = 0, zone = 0, , supply = 0] = buildings.subarray(
      i,
      i + BUILDING_STRIDE,
    );
    const [i0, i1] = [BUILDING_INSET, 1 - BUILDING_INSET];
    buildingBoxes.push({
      x0: x0 + i0,
      y0: y0 + i0,
      x1: x1 + i1,
      y1: y1 + i1,
      color: buildingColor(zone),
      edge: buildingEdgeColor(zone),
    });
    const c = CLUSTER_INSET;
    meshes.clusters.box(x0 + c, y0 + c, x1 + 1 - c, y1 + 1 - c, buildingColor(zone));
    marks.push([x0, y0, supply]);
  }
  for (const { x0, y0, x1, y1 } of [...facilityBoxes, ...buildingBoxes]) {
    meshes.details.box(x0, y0, x1, y1, SHADOW_COLOR, BUILDING_SHADOW_PX);
  }
  const e = BUILDING_EDGE;
  for (const { x0, y0, x1, y1, color, edge } of facilityBoxes) {
    meshes.facilities.box(x0, y0, x1, y1, edge);
    meshes.facilities.box(x0 + e, y0 + e, x1 - e, y1 - e, color);
  }
  // 寄りの段に移るあいだは、本体を濃く描いたまま縁を濃くしていくので、縁と本体を分ける
  for (const { x0, y0, x1, y1, color, edge } of buildingBoxes) {
    meshes.details.box(x0, y0, x1, y1, edge);
    meshes.bodies.box(x0 + e, y0 + e, x1 - e, y1 - e, color);
  }
  for (const [x, y, supply] of marks) {
    if ((supply & SUPPLY_POWER) === 0) {
      const c = markCenter({ x, y }, "power");
      meshes.marks.shape(c.x, c.y, MARK_HALF, NO_POWER_COLOR, BOLT_SHAPE);
    }
    if ((supply & SUPPLY_WATER) === 0) {
      const c = markCenter({ x, y }, "water");
      meshes.marks.shape(c.x, c.y, MARK_HALF, NO_WATER_COLOR, DROP_SHAPE);
    }
  }
  return meshes;
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
 * 塗る車道のいちばん広い半分の幅で、`asphaltColor` と `edgeColor` はその車道の塗りと縁の色である。
 * `minPx` は縮小したときの車道の太さの下限のいちばん大きい値、`sidewalk` は歩道のある区間がつながっているかである。
 */
type RoadNode = {
  half: number;
  asphalt: number;
  asphaltColor: Color;
  edgeColor: Color;
  /** つなぐ区間の、縮小したときの線の世界の半分の幅のいちばん大きい値。 */
  line: number;
  minPx: number;
  /** つなぐ区間のいちばん大きい種類の番号。 */
  widest: number;
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
      edgeColor: style.edge,
      line: 0,
      minPx: 0,
      widest: 0,
      sidewalk: false,
      arms: [],
    };
    node.half = Math.max(node.half, halfWidthOf(kind));
    if (asphaltHalfWidth(kind) > node.asphalt) {
      node.asphalt = asphaltHalfWidth(kind);
      node.asphaltColor = style.asphalt;
      node.edgeColor = style.edge;
    }
    node.line = Math.max(node.line, LINE_HALF[kind] ?? 0.31);
    node.minPx = Math.max(node.minPx, style.minPx);
    node.widest = Math.max(node.widest, kind);
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

// ノードのマスを拡大したときに埋める色。歩道のある道路がつながっていれば歩道の色、高速道路だけなら縁の色にする
const groundOf = (node: RoadNode): Color => (node.sidewalk ? SIDEWALK_COLOR : node.edgeColor);

type RoadNet = {
  ends: RoadNodes;
  /** `noCrosswalk` は横断歩道を外した腕の向きのビット（chizu-wasm の road_nodes の 5 つめ）である。 */
  nodeList: { x: number; y: number; node: RoadNode; noCrosswalk: number }[];
  /** 区間ごとの両端と種類。 */
  each: (fn: (a: Tile, b: Tile, kind: number, index: number) => void) => void;
};

function roadNet(segments: Float64Array, nodes: Float64Array): RoadNet {
  const ends = roadNodes(segments);
  const nodeList: RoadNet["nodeList"] = [];
  for (let i = 0; i + NODE_STRIDE <= nodes.length; i += NODE_STRIDE) {
    const [x = 0, y = 0] = nodes.subarray(i, i + 2);
    const noCrosswalk = nodes[i + 4] ?? 0;
    const node = ends.get(nodeKey(x, y));
    if (node !== undefined) {
      nodeList.push({ x, y, node, noCrosswalk });
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

/** 小の道路の種類の番号。引いたときに細く薄く描く。 */
export const MINOR_CODE = 1;

/** 線で描くときに、どの道路を描くか。`minor` は小の道路だけ、`major` は中以上の道路だけである。 */
export type RoadPart = "all" | "minor" | "major";

/**
 * Worker から届いた道路網の形を頂点に直す。基準のマスは最初のノードにする。縮小したときの描き方で、
 * 区間ごとに、縁の色の帯の上に、種類ごとの塗りの帯を重ねて縁取りにする。
 * 縮小しても種類を見分けられるよう、帯の太さの下限を種類ごとに変える。
 * `part` で小の道路とそれ以外を分ける。小の道路だけを引いたときに薄く描くためで、
 * ノードは、つながる区間のいちばん大きい種類の側に入れる。
 */
export function roadGeometry(
  segments: Float64Array,
  nodes: Float64Array,
  part: RoadPart = "all",
): Geometry {
  const geometry = new Geometry({ x: nodes[0] ?? 0, y: nodes[1] ?? 0 });
  const { nodeList, each } = roadNet(segments, nodes);
  const takes = (kind: number) => part === "all" || (part === "minor") === (kind === MINOR_CODE);
  const parted = nodeList.filter(({ node }) => takes(node.widest));
  // 縁の帯は、塗りの帯と同じ幅（世界と画面の下限）に、画面で両側に 1px ずつ足す
  each((a, b, kind) => {
    if (takes(kind)) {
      const style = roadStyle(kind);
      geometry.segment(a, b, lineHalfWidth(kind, a, b), style.edge, style.minPx, EDGE_EXTRA_PX);
    }
  });
  for (const { x, y, node } of parted) {
    geometry.square({ x, y }, node.line, node.edgeColor, node.minPx, EDGE_EXTRA_PX);
  }
  each((a, b, kind) => {
    if (takes(kind)) {
      const style = roadStyle(kind);
      geometry.segment(a, b, lineHalfWidth(kind, a, b), style.asphalt, style.minPx);
    }
  });
  for (const { x, y, node } of parted) {
    geometry.square({ x, y }, node.line, node.asphaltColor, node.minPx);
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
 * 拡大したときの、車線ごとの道路の描き方。下から順に、ノードのマスを埋める歩道（高速道路は縁）の色の四角、
 * 車道の外の植え込み（高速道路は縁の色の路肩）、歩道とその外の縁の線、車道、車道の縁石の線、ノードの車道の四角を重ねる。
 * 交差点の四角は車道の幅だけを埋めるので、交差点の角には歩道が残る。
 * 路面の線は、縮尺に合わせて薄くするので roadMarkingGeometry で別に作る。
 */
export function roadLaneGeometry(segments: Float64Array, nodes: Float64Array): Geometry {
  const geometry = new Geometry({ x: nodes[0] ?? 0, y: nodes[1] ?? 0 });
  const { ends, nodeList, each } = roadNet(segments, nodes);
  const trimOf = trimmer(segments, ends);
  // 歩道と植え込みの帯は、ノードのマスの手前（マスの縁）で止める。まっすぐ通り抜けるノードでは、帯をつなげる
  const nodeTrim = (n: number, atEnd: boolean) => {
    const [ax = 0, ay = 0, bx = 0, by = 0] = segments.subarray(
      n * SEGMENT_STRIDE,
      (n + 1) * SEGMENT_STRIDE,
    );
    const node = ends.get(atEnd ? nodeKey(bx, by) : nodeKey(ax, ay));
    return node === undefined || isThrough(node) ? 0 : ROAD_VISUAL_HALF;
  };

  for (const { x, y, node } of nodeList) {
    if (!isThrough(node)) {
      geometry.square({ x, y }, ROAD_VISUAL_HALF, groundOf(node), 0);
    }
  }
  // 車道の外の帯。斜めの区間は道路のマスを階段状に塗る
  each((a, b, kind, n) => {
    if (isDiagonal(a, b)) {
      roadFootprint(geometry, a, b, hasSidewalks(kind) ? SIDEWALK_COLOR : roadStyle(kind).edge);
      return;
    }
    const inner = asphaltHalfWidth(kind);
    // 歩道のある道路は、歩道の外からマスの縁までが植え込み。高速道路は車道の外が縁の色の路肩である
    const outer = hasSidewalks(kind) ? inner + SIDEWALK_WIDTH : inner;
    const half = (ROAD_VISUAL_HALF - outer) / 2;
    const color = hasSidewalks(kind) ? PLANTING_COLOR : roadStyle(kind).edge;
    for (const side of [-1, 1]) {
      geometry.strip(
        a,
        b,
        side * (outer + half),
        half,
        color,
        0,
        nodeTrim(n, false),
        nodeTrim(n, true),
      );
    }
  });
  // 歩道と、その外の縁の線。斜めの区間と高速道路には歩道がない
  each((a, b, kind, n) => {
    if (!hasSidewalks(kind) || isDiagonal(a, b)) {
      return;
    }
    const inner = asphaltHalfWidth(kind);
    const [trimA, trimB] = [nodeTrim(n, false), nodeTrim(n, true)];
    for (const side of [-1, 1]) {
      const offset = side * (inner + SIDEWALK_WIDTH / 2);
      geometry.strip(a, b, offset, SIDEWALK_WIDTH / 2, SIDEWALK_COLOR, 0, trimA, trimB);
      const line = side * (inner + SIDEWALK_WIDTH);
      geometry.strip(a, b, line, 0, SIDEWALK_EDGE_COLOR, THIN_PX, trimA, trimB);
    }
  });
  // ノードのマスの、腕のない辺。歩道の色で埋めたマスの縁に、歩道の外の縁の線を引く
  for (const { x, y, node } of nodeList) {
    if (node.sidewalk && !isThrough(node)) {
      nodeCurbs(geometry, { x, y }, node);
    }
  }
  each((a, b, kind) => {
    geometry.strip(a, b, 0, drawnAsphaltHalf(kind, a, b), roadStyle(kind).asphalt, 0);
  });
  // 車道の縁石の線。斜めの区間には引かない。交差点では、ノードの車道の四角の手前で止める
  each((a, b, kind, n) => {
    if (isDiagonal(a, b)) {
      return;
    }
    for (const side of [-1, 1]) {
      const offset = side * asphaltHalfWidth(kind);
      const trimA = trimOf(n, false, offset, (node) => node.asphalt);
      const trimB = trimOf(n, true, offset, (node) => node.asphalt);
      geometry.strip(a, b, offset, 0, roadStyle(kind).edge, THIN_PX, trimA, trimB);
    }
  });
  for (const { x, y, node } of nodeList) {
    geometry.square({ x, y }, node.asphalt, node.asphaltColor, 0);
  }
  return geometry;
}

/** 2 本の縦横の区間が、まっすぐ反対の向きにつながるノードか。 */
function isThrough(node: RoadNode): boolean {
  const [p, q] = node.arms;
  return (
    node.arms.length === 2 &&
    p !== undefined &&
    q !== undefined &&
    p.dx === -q.dx &&
    p.dy === -q.dy &&
    (p.dx === 0 || p.dy === 0)
  );
}

// ノードのマスの 4 つの辺。`dx`、`dy` は辺の外向き、`a`〜`b` は辺に沿う線（マスの中心からの相対）、
// `offset` はその線から辺までのずれである
const NODE_SIDES = [
  { dx: 0, dy: -1, a: [-0.5, 0], b: [0.5, 0], offset: -1 },
  { dx: 0, dy: 1, a: [-0.5, 0], b: [0.5, 0], offset: 1 },
  { dx: -1, dy: 0, a: [0, -0.5], b: [0, 0.5], offset: 1 },
  { dx: 1, dy: 0, a: [0, -0.5], b: [0, 0.5], offset: -1 },
] as const;

/** 行き止まりと交差点のノードの、縦横の腕がない辺に、歩道の外の縁の線を引く。 */
function nodeCurbs(geometry: Geometry, at: Tile, node: RoadNode): void {
  for (const side of NODE_SIDES) {
    if (node.arms.some((arm) => arm.dx === side.dx && arm.dy === side.dy)) {
      continue;
    }
    const a = { x: at.x + side.a[0], y: at.y + side.a[1] };
    const b = { x: at.x + side.b[0], y: at.y + side.b[1] };
    const offset = side.offset * ROAD_VISUAL_HALF;
    geometry.strip(a, b, offset, 0, SIDEWALK_EDGE_COLOR, THIN_PX);
  }
}

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
    const colors = MARKING_COLORS[kind];
    if (colors === undefined) {
      return;
    }
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
      const color = colors[strip.role];
      geometry.strip(a, b, strip.offset, strip.half, color, THIN_PX, trimA, trimB);
    }
  });
  // 交差点の腕ごとに、交差点の手前の車道を横切る横断歩道の縞を描く。高速道路の腕と、横断歩道を外した腕には描かない
  for (const { x, y, node, noCrosswalk } of nodeList) {
    if (node.arms.length < 3) {
      continue;
    }
    for (const arm of node.arms) {
      const kind = segments[arm.segment * SEGMENT_STRIDE + 4] ?? 0;
      if (kind !== HIGHWAY_CODE && (noCrosswalk & headingBit(arm)) === 0) {
        const diagonal = arm.dx !== 0 && arm.dy !== 0;
        const carriage = carriagewayHalfWidth(kind);
        const width = diagonal ? Math.min(carriage, DIAGONAL_ASPHALT_HALF) : carriage;
        zebra(geometry, { x, y }, arm, node.half, width);
      }
    }
  }
  return geometry;
}

// 腕の向きの並び。chizu-core の Heading の値の順（東から南回り）で、マスの y は下へ増える
const HEADINGS = [
  [1, 0],
  [1, 1],
  [0, 1],
  [-1, 1],
  [-1, 0],
  [-1, -1],
  [0, -1],
  [1, -1],
] as const;

/** 腕の向きの、chizu-wasm の road_nodes の横断歩道を外した腕の数でのビット。8 方向でなければ 0。 */
export function headingBit(arm: { dx: number; dy: number }): number {
  const sx = Math.sign(Math.round(arm.dx * 2));
  const sy = Math.sign(Math.round(arm.dy * 2));
  const i = HEADINGS.findIndex(([x, y]) => x === sx && y === sy);
  return i < 0 ? 0 : 1 << i;
}

/**
 * 交差点の、横断歩道を置ける腕。`x`、`y` は交差点のマス、`dx`、`dy` は交差点から出ていく 1 歩、
 * `removed` は横断歩道を外してあるか、`cx`、`cy` は縞の真ん中のワールド座標（マスの単位）である。
 */
export type CrosswalkArm = {
  x: number;
  y: number;
  dx: number;
  dy: number;
  removed: boolean;
  cx: number;
  cy: number;
};

/** 3 本以上が集まる交差点の、高速道路でない腕を、横断歩道を外したかと縞の真ん中の位置とともに並べる。 */
export function crosswalkArms(segments: Float64Array, nodes: Float64Array): CrosswalkArm[] {
  const { nodeList } = roadNet(segments, nodes);
  const out: CrosswalkArm[] = [];
  for (const { x, y, node, noCrosswalk } of nodeList) {
    if (node.arms.length < 3) {
      continue;
    }
    for (const arm of node.arms) {
      const kind = segments[arm.segment * SEGMENT_STRIDE + 4] ?? 0;
      if (kind === HIGHWAY_CODE) {
        continue;
      }
      const diagonal = arm.dx !== 0 && arm.dy !== 0;
      const along = (diagonal ? node.half * Math.SQRT2 : node.half) + ZEBRA_LENGTH / 2;
      const len = Math.hypot(Math.sign(arm.dx), Math.sign(arm.dy));
      const dx = Math.sign(arm.dx);
      const dy = Math.sign(arm.dy);
      out.push({
        x,
        y,
        dx,
        dy,
        removed: (noCrosswalk & headingBit(arm)) !== 0,
        cx: x + 0.5 + (dx / len) * along,
        cy: y + 0.5 + (dy / len) * along,
      });
    }
  }
  return out;
}

/** 横断歩道の縞（と、入れるときの停止線）。白い車道の上でも黄の車道の上でも見える色にする。 */
export const ZEBRA_COLOR = hex("#a9b3c2");
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
  0: hex("#34a853"),
  1: hex("#fbbc04"),
  2: hex("#ea4335"),
};
export const SIGNAL_BASE_COLOR = hex("#3c4043");
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

/**
 * 縮小したときの描き方（小の道路と、中以上の道路）と、拡大したときの車線ごとの描き方と、
 * その上に重ねる路面の線。
 */
export type RoadMeshes = { minor: Geometry; major: Geometry; lanes: Geometry; markings: Geometry };

export function roadMeshes(segments: Float64Array, nodes: Float64Array): RoadMeshes {
  return {
    minor: roadGeometry(segments, nodes, "minor"),
    major: roadGeometry(segments, nodes, "major"),
    lanes: roadLaneGeometry(segments, nodes),
    markings: roadMarkingGeometry(segments, nodes),
  };
}

type Mesh = { buffer: WebGLBuffer; vao: WebGLVertexArrayObject; count: number; origin: Tile };

const MESH_NAMES = [
  "clusters",
  "details",
  "facilities",
  "bodies",
  "marks",
  "minor",
  "major",
  "lanes",
  "markings",
  "signals",
  "preview",
] as const;
export type MeshName = (typeof MESH_NAMES)[number];

/**
 * 倍率 zoom で、下から順に描くメッシュと、その不透明度と、地面の色から本来の色へ寄せる割合。
 * 不透明度が 0 のメッシュは描かない。段の境目の幅（stages.ts）のあいだだけ、前の段と次の段の両方を描く。
 */
export function roadDrawOrder(zoom: number): [MeshName, number, number][] {
  const w = stageWeights(zoom);
  // 線で描く道路は、寄りの描き方がそろうまでその下に残す。車線ごとの描き方は、
  // 歩道と植え込みでマスいっぱいを覆うので、上に重ねて濃くしていけば線が見えなくなる
  const lines = w.detail < 1 ? 1 : 0;
  const order: [MeshName, number][] = [
    ["clusters", w.clusters],
    ["details", w.detail],
    ["facilities", 1],
    // 寄りに移るあいだは、本体を濃いまま描き、縁と影と隙間だけを濃くしていく
    ["bodies", w.detail > 0 ? 1 : 0],
    ["marks", w.marks],
    ["minor", lines],
    ["major", lines],
    ["lanes", w.detail],
    ["markings", w.detail],
    ["signals", w.detail],
    ["preview", 1],
  ];
  // 小の道路は不透明に描き、色を地面の色に寄せて薄くする
  return order.map(([name, alpha]) => [name, alpha, name === "minor" ? w.minorRoad : 1]);
}

// 道路と建物は、1 つ変わるたびに全体を受け取り直して頂点を作り直す。重くなったらチャンクごとのメッシュに分ける。
export class RoadLayer {
  readonly #gl: WebGL2RenderingContext;
  readonly #program: WebGLProgram;
  readonly #u: Record<
    "offset" | "zoom" | "viewport" | "alpha" | "fade",
    WebGLUniformLocation | null
  >;
  readonly #meshes: Record<MeshName, Mesh>;

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
      fade: gl.getUniformLocation(this.#program, "u_fade"),
    };
    this.#meshes = Object.fromEntries(MESH_NAMES.map((name) => [name, this.#mesh()])) as Record<
      MeshName,
      Mesh
    >;
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
      ["a_extra", 1, 10],
      ["a_along", 1, 11],
      ["a_dash", 1, 12],
    ];
    for (const [name, size, offset] of attributes) {
      const location = gl.getAttribLocation(this.#program, name);
      gl.enableVertexAttribArray(location);
      gl.vertexAttribPointer(location, size, gl.FLOAT, false, stride, offset * 4);
    }
    gl.bindVertexArray(null);
    return { buffer, vao, count: 0, origin: { x: 0, y: 0 } };
  }

  #upload(name: MeshName, geometry: Geometry): void {
    const gl = this.#gl;
    const mesh = this.#meshes[name];
    gl.bindBuffer(gl.ARRAY_BUFFER, mesh.buffer);
    gl.bufferData(gl.ARRAY_BUFFER, geometry.build(), gl.DYNAMIC_DRAW);
    mesh.count = geometry.vertexCount;
    mesh.origin = geometry.origin;
  }

  setBuildings(meshes: BuildingMeshes): void {
    this.#upload("clusters", meshes.clusters);
    this.#upload("details", meshes.details);
    this.#upload("facilities", meshes.facilities);
    this.#upload("bodies", meshes.bodies);
    this.#upload("marks", meshes.marks);
  }

  setRoads(meshes: RoadMeshes): void {
    this.#upload("minor", meshes.minor);
    this.#upload("major", meshes.major);
    this.#upload("lanes", meshes.lanes);
    this.#upload("markings", meshes.markings);
  }

  setSignals(geometry: Geometry): void {
    this.#upload("signals", geometry);
  }

  setPreview(geometry: Geometry | null): void {
    this.#upload("preview", geometry ?? new Geometry({ x: 0, y: 0 }));
  }

  draw(camera: Camera, viewport: Viewport): void {
    const gl = this.#gl;
    gl.useProgram(this.#program);
    gl.uniform1f(this.#u.zoom, camera.zoom);
    gl.uniform2f(this.#u.viewport, viewport.width, viewport.height);
    gl.enable(gl.BLEND);
    // 色だけを重ね、フレームバッファのアルファは 1 のまま保つ（キャンバスがページの背景と混ざって白く飛ばないように）
    gl.blendFuncSeparate(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA, gl.ZERO, gl.ONE);
    for (const [name, alpha, fade] of roadDrawOrder(camera.zoom)) {
      const mesh = this.#meshes[name];
      if (mesh.count === 0 || alpha === 0) {
        continue;
      }
      gl.uniform1f(this.#u.alpha, alpha);
      gl.uniform1f(this.#u.fade, fade);
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
