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
  type StripRole,
  showsLanes,
} from "./lanes.ts";

/** マスの座標。 */
export type Tile = { x: number; y: number };

// 縮小しても線が消えないよう、半分の太さは画面で 1.5 ピクセルを下回らせない。
// 車線の区切りなど細い線は、半分の太さを負の値で渡し、0.5 ピクセルまで細くする
const MIN_HALF_PX = "1.5";
const MIN_THIN_HALF_PX = "0.5";

const VERTEX = `#version 300 es
in vec2 a_pos;
in vec2 a_normal;
in float a_half;
in vec4 a_color;
uniform vec2 u_offset;
uniform float u_zoom;
uniform vec2 u_viewport;
out vec4 v_color;
void main() {
  float half_px = a_half > 0.0
    ? max(a_half * u_zoom, ${MIN_HALF_PX})
    : (a_half < 0.0 ? max(-a_half * u_zoom, ${MIN_THIN_HALF_PX}) : 0.0);
  vec2 p = (a_pos * u_zoom + u_offset + a_normal * half_px) / (u_viewport * 0.5);
  gl_Position = vec4(p.x, -p.y, 0.0, 1.0);
  v_color = a_color;
}`;

const FRAGMENT = `#version 300 es
precision mediump float;
in vec4 v_color;
out vec4 color;
void main() {
  color = v_color;
}`;

/** 頂点 1 つの数の個数。位置 2、法線 2、半分の太さ 1、色 4。 */
export const VERTEX_FLOATS = 9;

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

const ROAD_COLOR: Color = [0.32, 0.32, 0.35, 1];
const HIGHWAY_COLOR: Color = [0.78, 0.55, 0.25, 1];
const NODE_COLOR: Color = [0.5, 0.5, 0.54, 1];
const NODE_HALF = 0.5;
const ASPHALT_COLOR: Color = [0.24, 0.24, 0.27, 1];
const HIGHWAY_ASPHALT_COLOR: Color = [0.55, 0.4, 0.2, 1];
const SIDEWALK_COLOR: Color = [0.5, 0.5, 0.53, 1];
const CENTER_LINE_COLOR: Color = [0.95, 0.8, 0.2, 1];
const DIVIDER_COLOR: Color = [0.85, 0.85, 0.88, 1];

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

  #vertex(x: number, y: number, nx: number, ny: number, half: number, color: Color): void {
    this.#data.push(x - this.origin.x, y - this.origin.y, nx, ny, half, ...color);
  }

  /** マス a の中心からマス b の中心への線。 */
  segment(a: Tile, b: Tile, half: number, color: Color): void {
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
    const v = (x: number, y: number, s: number) => this.#vertex(x, y, nx * s, ny * s, half, color);
    v(ax, ay, 1);
    v(ax, ay, -1);
    v(bx, by, 1);
    v(bx, by, 1);
    v(ax, ay, -1);
    v(bx, by, -1);
  }

  /**
   * マス a の中心からマス b の中心への、中心線から `offset`（マス）だけ横にずらした帯。
   * 半分の太さ `half` は世界の単位で、画面では `thin` のとき 0.5 ピクセル、そうでなければ 1.5 ピクセルを下回らない。
   * 両端は `trimA` と `trimB`（マス）だけ内側に縮める。`offset` の正は進む向き（a から b）の右である。
   */
  strip(
    a: Tile,
    b: Tile,
    offset: number,
    half: number,
    color: Color,
    thin: boolean,
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
    const h = thin ? -half : half;
    const v = (x: number, y: number, s: number) => this.#vertex(x, y, nx * s, ny * s, h, color);
    v(ax, ay, 1);
    v(ax, ay, -1);
    v(bx, by, 1);
    v(bx, by, 1);
    v(ax, ay, -1);
    v(bx, by, -1);
  }

  /** マスの中心に置く正方形。 */
  square(t: Tile, half: number, color: Color): void {
    this.dot(t.x + 0.5, t.y + 0.5, half, color);
  }

  /** ワールド座標 (x, y)（マスの単位）を中心にした正方形。 */
  dot(x: number, y: number, half: number, color: Color): void {
    const v = (sx: number, sy: number) => this.#vertex(x, y, sx, sy, half, color);
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
        this.#vertex(x, y, sx, sy, half, color);
      }
    }
  }

  /** 角のマスを両端に含む長方形。inset（マス）だけ内側に縮める。 */
  rect(min: Tile, max: Tile, color: Color, inset = 0): void {
    const v = (x: number, y: number) => this.#vertex(x, y, 0, 0, 0, color);
    const [x0, y0, x1, y1] = [min.x + inset, min.y + inset, max.x + 1 - inset, max.y + 1 - inset];
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
// 区画を塗った地面の色。草地の色に区画の色を ZONE_MIX の割合で混ぜる
export const GRASS_COLOR: readonly [number, number, number] = [0.56, 0.72, 0.42];
export const ZONE_MIX = 0.55;

/** 段階 level（1〜5）の建物の色。 */
export function buildingColor(zone: number, level: number): Color {
  const [r, g, b] = ZONE_BASE[zone] ?? [0.6, 0.6, 0.6];
  const k = 0.35 + 0.1 * Math.min(Math.max(level, 1), 5);
  return [r * k, g * k, b * k, 1];
}

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

/** Worker から届いた建物と施設を頂点に直す。建物は隣と見分けられるよう、少し内側に縮めて描く。 */
export function buildingGeometry(buildings: Float64Array, facilities: Float64Array): Geometry {
  const geometry = new Geometry({
    x: buildings[0] ?? facilities[0] ?? 0,
    y: buildings[1] ?? facilities[1] ?? 0,
  });
  for (let i = 0; i + FACILITY_STRIDE <= facilities.length; i += FACILITY_STRIDE) {
    const [x0 = 0, y0 = 0, x1 = 0, y1 = 0, kind = 0] = facilities.subarray(i, i + FACILITY_STRIDE);
    const color = FACILITY_COLOR[kind] ?? [0.6, 0.6, 0.6, 1];
    geometry.rect({ x: x0, y: y0 }, { x: x1, y: y1 }, color, 0.05);
  }
  const marks: [number, number, number][] = [];
  for (let i = 0; i + BUILDING_STRIDE <= buildings.length; i += BUILDING_STRIDE) {
    const [x0 = 0, y0 = 0, x1 = 0, y1 = 0, zone = 0, level = 1, supply = 0] = buildings.subarray(
      i,
      i + BUILDING_STRIDE,
    );
    const color = buildingColor(zone, level);
    geometry.rect({ x: x0, y: y0 }, { x: x1, y: y1 }, color, BUILDING_INSET);
    marks.push([x0, y0, supply]);
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

type RoadNode = { half: number; carriage: number; kind: number; arms: Arm[] };
type RoadNodes = Map<string, RoadNode>;

const nodeKey = (x: number, y: number) => `${x},${y}`;

/** ノードごとに、つながる区間と、いちばん広い区間の幅を求める。 */
function roadNodes(segments: Float64Array): RoadNodes {
  const nodes: RoadNodes = new Map();
  const touch = (x: number, y: number, other: Tile, kind: number, segment: number) => {
    const key = nodeKey(x, y);
    const node = nodes.get(key) ?? { half: 0, carriage: 0, kind, arms: [] };
    const half = halfWidthOf(kind);
    if (half >= node.half) {
      node.kind = kind;
    }
    node.half = Math.max(node.half, half);
    node.carriage = Math.max(node.carriage, carriagewayHalfWidth(kind));
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

/** Worker から届いた道路網の形を頂点に直す。基準のマスは最初のノードにする。縮小したときの、1 本の線の描き方である。 */
export function roadGeometry(segments: Float64Array, nodes: Float64Array): Geometry {
  const geometry = new Geometry({ x: nodes[0] ?? 0, y: nodes[1] ?? 0 });
  const halves = roadNodes(segments);
  for (let i = 0; i + SEGMENT_STRIDE <= segments.length; i += SEGMENT_STRIDE) {
    const [ax = 0, ay = 0, bx = 0, by = 0, kind = 0] = segments.subarray(i, i + SEGMENT_STRIDE);
    const color = kind === HIGHWAY_CODE ? HIGHWAY_COLOR : ROAD_COLOR;
    geometry.segment({ x: ax, y: ay }, { x: bx, y: by }, halfWidthOf(kind), color);
  }
  for (let i = 0; i + NODE_STRIDE <= nodes.length; i += NODE_STRIDE) {
    const [x = 0, y = 0] = nodes.subarray(i, i + 2);
    const half = halves.get(nodeKey(x, y))?.half ?? NODE_HALF;
    geometry.square({ x, y }, half, NODE_COLOR);
  }
  return geometry;
}

const STRIP_COLOR: Record<Exclude<StripRole, "asphalt">, Color> = {
  sidewalk: SIDEWALK_COLOR,
  center: CENTER_LINE_COLOR,
  divider: DIVIDER_COLOR,
};

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
 * 拡大したときの、車線ごとの道路の描き方。下から順に、行き止まりと曲がり角の外側を埋める歩道の色の四角、
 * 車道、歩道、交差点と曲がり角の車道の色の四角、中央線と車線の区切りの線を重ねる。
 * 3 本以上が集まる交差点では、線を交差点の手前で止める。2 本だけがつながる角では、線を折れ曲がりに沿ってつなぐ。
 */
export function roadLaneGeometry(segments: Float64Array, nodes: Float64Array): Geometry {
  const geometry = new Geometry({ x: nodes[0] ?? 0, y: nodes[1] ?? 0 });
  const ends = roadNodes(segments);
  const asphaltOf = (kind: number) =>
    kind === HIGHWAY_CODE ? HIGHWAY_ASPHALT_COLOR : ASPHALT_COLOR;
  const nodeList: { x: number; y: number; node: RoadNode }[] = [];
  for (let i = 0; i + NODE_STRIDE <= nodes.length; i += NODE_STRIDE) {
    const [x = 0, y = 0] = nodes.subarray(i, i + 2);
    const node = ends.get(nodeKey(x, y));
    if (node !== undefined) {
      nodeList.push({ x, y, node });
    }
  }

  // 帯の端を縮める長さ。`atEnd` は区間の終わり側のノードかどうか
  const trimOf = (index: number, atEnd: boolean, role: StripRole, offset: number): number => {
    const [ax = 0, ay = 0, bx = 0, by = 0] = segments.subarray(
      index * SEGMENT_STRIDE,
      (index + 1) * SEGMENT_STRIDE,
    );
    const [x, y] = atEnd ? [bx, by] : [ax, ay];
    const node = ends.get(nodeKey(x, y));
    if (node === undefined || role === "asphalt") {
      return 0;
    }
    if (node.arms.length >= 3) {
      if (role === "sidewalk") {
        return 0;
      }
      return ax !== bx && ay !== by ? node.half * Math.SQRT2 : node.half;
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

  const strips = (roles: StripRole[]) => {
    for (let i = 0, n = 0; i + SEGMENT_STRIDE <= segments.length; i += SEGMENT_STRIDE, n++) {
      const [ax = 0, ay = 0, bx = 0, by = 0, kind = 0] = segments.subarray(i, i + SEGMENT_STRIDE);
      const [a, b] = [
        { x: ax, y: ay },
        { x: bx, y: by },
      ];
      for (const strip of crossSection(kind)) {
        if (!roles.includes(strip.role)) {
          continue;
        }
        if (strip.role === "asphalt") {
          geometry.strip(a, b, strip.offset, strip.half, asphaltOf(kind), false);
        } else {
          const trimA = trimOf(n, false, strip.role, strip.offset);
          const trimB = trimOf(n, true, strip.role, strip.offset);
          geometry.strip(
            a,
            b,
            strip.offset,
            strip.half,
            STRIP_COLOR[strip.role],
            true,
            trimA,
            trimB,
          );
        }
      }
    }
  };

  // 行き止まりと曲がり角の外側の、車道に覆われない所を埋める
  for (const { x, y, node } of nodeList) {
    if (node.arms.length < 3) {
      const color = hasSidewalks(node.kind) ? SIDEWALK_COLOR : asphaltOf(node.kind);
      geometry.square({ x, y }, node.half, color);
    }
  }
  strips(["asphalt"]);
  strips(["sidewalk"]);
  // 交差点は歩道の上まで車道の色で埋める。角は車道の幅だけ埋め、歩道は残す
  for (const { x, y, node } of nodeList) {
    const crossing = node.arms.length >= 3;
    geometry.square({ x, y }, crossing ? node.half : node.carriage, asphaltOf(node.kind));
  }
  strips(["center", "divider"]);
  return geometry;
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

/** 縮小したときの 1 本の線と、拡大したときの車線ごとの描き方。 */
export type RoadMeshes = { simple: Geometry; lanes: Geometry };

export function roadMeshes(segments: Float64Array, nodes: Float64Array): RoadMeshes {
  return { simple: roadGeometry(segments, nodes), lanes: roadLaneGeometry(segments, nodes) };
}

type Mesh = { buffer: WebGLBuffer; vao: WebGLVertexArrayObject; count: number; origin: Tile };

// 道路と建物は、1 つ変わるたびに全体を受け取り直して頂点を作り直す。重くなったらチャンクごとのメッシュに分ける。
export class RoadLayer {
  readonly #gl: WebGL2RenderingContext;
  readonly #program: WebGLProgram;
  readonly #u: Record<"offset" | "zoom" | "viewport", WebGLUniformLocation | null>;
  readonly #buildings: Mesh;
  readonly #roads: Mesh;
  readonly #lanes: Mesh;
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
    };
    this.#buildings = this.#mesh();
    this.#roads = this.#mesh();
    this.#lanes = this.#mesh();
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
      ["a_color", 4, 5],
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
    const roads = showsLanes(camera.zoom) ? this.#lanes : this.#roads;
    for (const mesh of [this.#buildings, roads, this.#signals, this.#preview]) {
      if (mesh.count === 0) {
        continue;
      }
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
