// 細部の段（docs/map-style.md の「細部の段で描き足すもの」）。倍率 32 から上で、寄りの描き方の上に
// 家の屋根の形、屋上の設備、工場の屋根の筋、街路樹と低木、停止線、3 つ並びの信号を描き足す。
// 頂点は、細部の段に入っているときだけ、見えている範囲（と少しの余白）のぶんだけ作る。
// どれも中核から届いた形と建物のマスの座標だけから決め、乱数を使わないので、同じ街を同じ場面で開けば同じ絵になる。

import { BUILDING_STRIDE, SEGMENT_STRIDE, SIGNAL_STRIDE } from "../protocol/messages.ts";
import { carriagewayHalfWidth, HIGHWAY_CODE, hasSidewalks, SIDEWALK_WIDTH } from "./lanes.ts";
import { type Color, hex } from "./palette.ts";
import {
  asphaltHalfWidth,
  BUILDING_EDGE,
  BUILDING_INSET,
  buildingEdgeColor,
  DIAGONAL_ASPHALT_HALF,
  Geometry,
  isDiagonal,
  isThrough,
  ROAD_VISUAL_HALF,
  roadNet,
  SIGNAL_BASE_COLOR,
  SIGNAL_COLOR,
  signalLampCenter,
  type Tile,
  tilesOnLine,
  ZEBRA_COLOR,
  ZEBRA_LENGTH,
} from "./roads.ts";
import { showsFine } from "./stages.ts";

/** ワールド座標（マス）の長方形。min は含み、max は含まない。 */
export type TileRect = { minX: number; minY: number; maxX: number; maxY: number };

const contains = (outer: TileRect, inner: TileRect) =>
  inner.minX >= outer.minX &&
  inner.minY >= outer.minY &&
  inner.maxX <= outer.maxX &&
  inner.maxY <= outer.maxY;

/** 長方形 (x0, y0)〜(x1, y1) が rect に少しでもかかるか。 */
const overlaps = (r: TileRect, x0: number, y0: number, x1: number, y1: number) =>
  x1 >= r.minX && x0 <= r.maxX && y1 >= r.minY && y0 <= r.maxY;

// 区画の番号。chizu-wasm の zone_code と同じ
const RESIDENTIAL = 1;
const COMMERCIAL = 2;
const INDUSTRIAL = 3;
const OFFICE = 4;

/** 住宅の屋根の棟と隅棟の、半分の太さ（画面のピクセル）。線の太さは 1.5px である。 */
export const ROOF_RIDGE_HALF_PX = 0.75;
/** 工業の屋根の筋の、半分の太さ（画面のピクセル）と間隔（マス）。 */
export const ROOF_STRIPE_HALF_PX = 0.5;
export const ROOF_STRIPE_SPACING = 0.25;
/** 商業とオフィスの屋上の設備の、半分の大きさ（マス）。0.25 マス角である。 */
export const EQUIPMENT_HALF = 0.125;

/** 建物の左上のマスから決める 32 ビットの値。屋上の設備の数と位置に使う。 */
export function tileHash(x: number, y: number): number {
  let h = Math.imul(x | 0, 0x27d4eb2d) ^ Math.imul(y | 0, 0x165667b1);
  h = Math.imul(h ^ (h >>> 15), 0x85ebca6b);
  h = Math.imul(h ^ (h >>> 13), 0xc2b2ae35);
  return (h ^ (h >>> 16)) >>> 0;
}

/** 屋上の設備の中心（マス）。数は 1〜2 個で、建物の長い辺を等分した区切りごとに 1 個ずつ置く。 */
export function equipmentCenters(
  x0: number,
  y0: number,
  x1: number,
  y1: number,
  hash: number,
): { x: number; y: number }[] {
  const count = 1 + (hash & 1);
  const wide = x1 - x0 >= y1 - y0;
  const [long0, long1] = wide ? [x0, x1] : [y0, y1];
  const [short0, short1] = wide ? [y0, y1] : [x0, x1];
  const margin = EQUIPMENT_HALF + 0.08;
  const out: { x: number; y: number }[] = [];
  for (let k = 0; k < count; k++) {
    const part = (long1 - long0) / count;
    const lo = long0 + part * k + margin;
    const hi = long0 + part * (k + 1) - margin;
    // 区切りの中と短い辺の向きの位置を、ハッシュのビットから 0〜1 の割合で選ぶ
    const u = ((hash >>> (1 + k * 8)) & 0xf) / 15;
    const v = ((hash >>> (5 + k * 8)) & 0xf) / 15;
    const along = hi > lo ? lo + (hi - lo) * u : (lo + hi) / 2;
    const s0 = short0 + margin;
    const s1 = short1 - margin;
    const across = s1 > s0 ? s0 + (s1 - s0) * v : (short0 + short1) / 2;
    out.push(wide ? { x: along, y: across } : { x: across, y: along });
  }
  return out;
}

/**
 * 建物の屋根の形。住宅は屋根の棟と隅棟（寄棟の屋根）、商業とオフィスは屋上の設備、工業は屋根の筋にする。
 * `rect` にかかる建物だけを描く。
 */
export function roofGeometry(buildings: Float64Array, rect: TileRect): Geometry {
  const geometry = new Geometry({ x: Math.floor(rect.minX), y: Math.floor(rect.minY) });
  const e = BUILDING_EDGE;
  for (let i = 0; i + BUILDING_STRIDE <= buildings.length; i += BUILDING_STRIDE) {
    const [tx0 = 0, ty0 = 0, tx1 = 0, ty1 = 0, zone = 0] = buildings.subarray(i, i + 5);
    // 本体（縁の内側）の長方形
    const x0 = tx0 + BUILDING_INSET + e;
    const y0 = ty0 + BUILDING_INSET + e;
    const x1 = tx1 + 1 - BUILDING_INSET - e;
    const y1 = ty1 + 1 - BUILDING_INSET - e;
    if (!overlaps(rect, x0, y0, x1, y1)) {
      continue;
    }
    const edge = buildingEdgeColor(zone);
    if (zone === RESIDENTIAL) {
      hipRoof(geometry, x0, y0, x1, y1, edge);
    } else if (zone === INDUSTRIAL) {
      roofStripes(geometry, x0, y0, x1, y1, edge);
    } else if (zone === COMMERCIAL || zone === OFFICE) {
      for (const c of equipmentCenters(x0, y0, x1, y1, tileHash(tx0, ty0))) {
        geometry.box(
          c.x - EQUIPMENT_HALF,
          c.y - EQUIPMENT_HALF,
          c.x + EQUIPMENT_HALF,
          c.y + EQUIPMENT_HALF,
          edge,
        );
      }
    }
  }
  return geometry;
}

/** 寄棟の屋根。長い辺の向きに棟を引き、4 つの角から棟の両端へ隅棟を引く。正方形なら棟は点になる。 */
function hipRoof(g: Geometry, x0: number, y0: number, x1: number, y1: number, color: Color) {
  const h = ROOF_RIDGE_HALF_PX;
  const w = x1 - x0;
  const d = y1 - y0;
  if (w >= d) {
    const cy = (y0 + y1) / 2;
    const [r0, r1] = [x0 + d / 2, x1 - d / 2];
    g.line(r0, cy, r1, cy, 0, color, h);
    g.line(x0, y0, r0, cy, 0, color, h);
    g.line(x0, y1, r0, cy, 0, color, h);
    g.line(x1, y0, r1, cy, 0, color, h);
    g.line(x1, y1, r1, cy, 0, color, h);
  } else {
    const cx = (x0 + x1) / 2;
    const [r0, r1] = [y0 + w / 2, y1 - w / 2];
    g.line(cx, r0, cx, r1, 0, color, h);
    g.line(x0, y0, cx, r0, 0, color, h);
    g.line(x1, y0, cx, r0, 0, color, h);
    g.line(x0, y1, cx, r1, 0, color, h);
    g.line(x1, y1, cx, r1, 0, color, h);
  }
}

/** 工場の屋根の筋。長い辺の向きに、ROOF_STRIPE_SPACING マスごとに引く。 */
function roofStripes(g: Geometry, x0: number, y0: number, x1: number, y1: number, color: Color) {
  const h = ROOF_STRIPE_HALF_PX;
  const wide = x1 - x0 >= y1 - y0;
  const [s0, s1] = wide ? [y0, y1] : [x0, x1];
  for (
    let s = s0 + ROOF_STRIPE_SPACING;
    s < s1 - ROOF_STRIPE_SPACING / 2;
    s += ROOF_STRIPE_SPACING
  ) {
    if (wide) {
      g.line(x0, s, x1, s, 0, color, h);
    } else {
      g.line(s, y0, s, y1, 0, color, h);
    }
  }
}

/** 街路樹と低木の色と半径（マス）。街路樹の半径は、画面で 1.5px を下回らない。 */
export const STREET_TREE_COLOR = hex("#bcd6a4");
export const SHRUB_COLOR = hex("#cfe3bb");
export const STREET_TREE_HALF = 0.07;
export const SHRUB_HALF = 0.04;
/** 停止線の、腕の向きの太さ（マス）と、横断歩道の外側からのすき間（マス）。 */
export const STOP_LINE_WIDTH = 0.04;
const STOP_LINE_GAP = 0.05;
// 低木を置く、マスの中心からの道路の向きの位置。0.25 マスおきに並べ、街路樹のあるマスでは真ん中の 2 つを省く
const SHRUB_STEPS = [-0.375, -0.125, 0.125, 0.375];

/** 街路樹を置くマスか。ワールドの座標の偶奇で決めるので、道路を引き直しても同じ所に立つ。 */
export const hasStreetTree = (t: Tile) => (t.x + t.y) % 2 === 0;

/**
 * 道路の細部。歩道の外の植え込みに、2 マスごとの街路樹と、そのあいだの低木の点を並べる。
 * 交差点の腕ごとに、横断歩道の外側の、入ってくる車線の側に停止線を引く。`rect` にかかるものだけを描く。
 * 高速道路には歩道も横断歩道もないので描かない。斜めの区間には植え込みがないので、街路樹は描かない。
 */
export function streetGeometry(
  segments: Float64Array,
  nodes: Float64Array,
  rect: TileRect,
): Geometry {
  const geometry = new Geometry({ x: Math.floor(rect.minX), y: Math.floor(rect.minY) });
  const { ends, nodeList, each } = roadNet(segments, nodes);
  const inRect = (t: Tile) =>
    t.x + 1 >= rect.minX && t.x <= rect.maxX && t.y + 1 >= rect.minY && t.y <= rect.maxY;
  // 植え込みの帯がつながるのは、区間の中のマスと、まっすぐ通り抜けるノードのマスである
  const planted = (t: Tile, end: boolean) => {
    if (!end) {
      return true;
    }
    const node = ends.get(`${t.x},${t.y}`);
    return node !== undefined && isThrough(node);
  };
  const throughDrawn = new Set<string>();
  const shrubs: [number, number][] = [];
  const trees: [number, number][] = [];
  each((a, b, kind) => {
    if (!hasSidewalks(kind) || isDiagonal(a, b)) {
      return;
    }
    if (
      !overlaps(
        rect,
        Math.min(a.x, b.x),
        Math.min(a.y, b.y),
        Math.max(a.x, b.x) + 1,
        Math.max(a.y, b.y) + 1,
      )
    ) {
      return;
    }
    const len = Math.hypot(b.x - a.x, b.y - a.y);
    if (len === 0) {
      return;
    }
    const [ux, uy] = [(b.x - a.x) / len, (b.y - a.y) / len];
    const outer = asphaltHalfWidth(kind) + SIDEWALK_WIDTH;
    const offset = (outer + ROAD_VISUAL_HALF) / 2;
    const tiles = tilesOnLine(a, b);
    for (const [k, t] of tiles.entries()) {
      const end = k === 0 || k === tiles.length - 1;
      if (!inRect(t) || !planted(t, end)) {
        continue;
      }
      // 通り抜けるノードのマスは、両側の区間で 2 度描かない
      if (end) {
        const key = `${t.x},${t.y}`;
        if (throughDrawn.has(key)) {
          continue;
        }
        throughDrawn.add(key);
      }
      const tree = hasStreetTree(t);
      for (const side of [-1, 1]) {
        const cx = t.x + 0.5 - uy * offset * side;
        const cy = t.y + 0.5 + ux * offset * side;
        if (tree) {
          trees.push([cx, cy]);
        }
        for (const step of SHRUB_STEPS) {
          if (!tree || Math.abs(step) > 0.25) {
            shrubs.push([cx + ux * step, cy + uy * step]);
          }
        }
      }
    }
  });
  for (const [x, y] of shrubs) {
    geometry.disc(x, y, SHRUB_HALF, SHRUB_COLOR, 0);
  }
  for (const [x, y] of trees) {
    geometry.disc(x, y, STREET_TREE_HALF, STREET_TREE_COLOR);
  }
  for (const { x, y, node } of nodeList) {
    if (node.arms.length < 3 || !inRect({ x, y })) {
      continue;
    }
    for (const arm of node.arms) {
      const kind = segments[arm.segment * SEGMENT_STRIDE + 4] ?? 0;
      if (kind !== HIGHWAY_CODE) {
        stopLine(geometry, { x, y }, arm, node.half, kind);
      }
    }
  }
  return geometry;
}

/**
 * 交差点 `at` の腕 `arm` の停止線。横断歩道の外側に、入ってくる車線（左側通行なので、交差点から出ていく向きの右）の
 * 幅だけ、腕を横切る線を引く。
 */
function stopLine(
  geometry: Geometry,
  at: Tile,
  arm: { dx: number; dy: number },
  half: number,
  kind: number,
): void {
  const sx = Math.sign(arm.dx);
  const sy = Math.sign(arm.dy);
  const diagonal = sx !== 0 && sy !== 0;
  const next = { x: at.x + sx, y: at.y + sy };
  const len = Math.hypot(sx, sy);
  const carriage = carriagewayHalfWidth(kind);
  const width = diagonal ? Math.min(carriage, DIAGONAL_ASPHALT_HALF) : carriage;
  const start = (diagonal ? half * Math.SQRT2 : half) + ZEBRA_LENGTH + STOP_LINE_GAP;
  geometry.strip(
    at,
    next,
    width / 2,
    width / 2,
    ZEBRA_COLOR,
    0,
    start,
    len - start - STOP_LINE_WIDTH,
  );
}

/** 3 つ並びの信号の、消えている灯火の色。 */
export const SIGNAL_OFF_COLOR = hex("#5f6368");
// 3 つ並びの灯火の半径と間隔、台の半分の長さと半分の太さ（マス）
export const LAMP_HALF = 0.045;
export const LAMP_SPACING = 0.11;
const HOUSING_HALF_LENGTH = 0.17;
const HOUSING_HALF = 0.065;
// 灯火の並び。chizu-core の Light の値（0 青、1 黄、2 赤）で、入ってくる車から見て左から青、黄、赤の順にする
const LAMP_ORDER = [0, 1, 2] as const;

/**
 * 腕ごとの、3 つ並びの信号。暗い台の上に、入ってくる車から見て左から青、黄、赤の灯火を並べ、点いている灯火だけを
 * 明るくする。台は腕を横切る向きに置く。`rect` にかかる腕だけを描く。
 */
export function lampGeometry(lights: Float64Array, rect: TileRect): Geometry {
  const geometry = new Geometry({ x: Math.floor(rect.minX), y: Math.floor(rect.minY) });
  for (let i = 0; i + SIGNAL_STRIDE <= lights.length; i += SIGNAL_STRIDE) {
    const [x = 0, y = 0, dx = 0, dy = 0, light = 2] = lights.subarray(i, i + SIGNAL_STRIDE);
    if (!overlaps(rect, x, y, x + 1, y + 1)) {
      continue;
    }
    const c = signalLampCenter(x, y, dx, dy);
    const len = Math.hypot(dx, dy) || 1;
    // 入ってくる車の左（交差点から出ていく向きの右）。y は下向きなので (-dy, dx) である
    const [lx, ly] = [-dy / len, dx / len];
    const h = HOUSING_HALF_LENGTH;
    geometry.line(
      c.x - lx * h,
      c.y - ly * h,
      c.x + lx * h,
      c.y + ly * h,
      HOUSING_HALF,
      SIGNAL_BASE_COLOR,
      1,
    );
    for (const [k, lamp] of LAMP_ORDER.entries()) {
      const s = (1 - k) * LAMP_SPACING;
      const color = lamp === light ? (SIGNAL_COLOR[lamp] ?? SIGNAL_OFF_COLOR) : SIGNAL_OFF_COLOR;
      geometry.disc(c.x + lx * s, c.y + ly * s, LAMP_HALF, color, 0.75);
    }
  }
  return geometry;
}

/** 細部の頂点。作り直したものだけを入れる。 */
export type DetailMeshes = { roofs?: Geometry; streets?: Geometry; lamps?: Geometry };

/**
 * 細部の頂点を、要るときだけ作る。細部の段より引いているあいだは何も作らない。
 * 作るときは見えている範囲に余白を足した範囲のぶんだけ作り、画面がその範囲の中にあるうちは作り直さない。
 * 建物、道路網、信号が変わったときは、変わったものだけを同じ範囲で作り直す。
 */
export class DetailCache {
  #buildings: Float64Array = new Float64Array();
  #segments: Float64Array = new Float64Array();
  #nodes: Float64Array = new Float64Array();
  #lights: Float64Array = new Float64Array();
  #rect: TileRect | null = null;
  #dirty = { roofs: true, streets: true, lamps: true };
  /** 頂点を作った回数。test:wasm が、引いたときに作っていないかを見るのに使う。 */
  builds = 0;

  setBuildings(buildings: Float64Array): void {
    this.#buildings = buildings;
    this.#dirty.roofs = true;
  }

  setRoads(segments: Float64Array, nodes: Float64Array): void {
    this.#segments = segments;
    this.#nodes = nodes;
    this.#dirty.streets = true;
  }

  setSignals(lights: Float64Array): void {
    this.#lights = lights;
    this.#dirty.lamps = true;
  }

  /** 次に要るときに、すべて作り直す（WebGL のコンテキストを作り直したときなど）。 */
  invalidate(): void {
    this.#rect = null;
  }

  /**
   * 倍率 `zoom` で、見えている範囲 `view` を描くのに作り直す頂点を返す。作り直すものがなければ null を返す。
   */
  update(zoom: number, view: TileRect): DetailMeshes | null {
    if (!showsFine(zoom)) {
      return null;
    }
    let all = false;
    // 寄ったあとも引いたときの広い範囲を持ち続けると、見えない所の頂点まで描くので、広すぎる範囲も作り直す
    const tooWide =
      this.#rect !== null && this.#rect.maxX - this.#rect.minX > 3 * (view.maxX - view.minX) + 4;
    if (this.#rect === null || !contains(this.#rect, view) || tooWide) {
      // 画面の幅と高さの半分ずつを余白にし、少し動かしただけでは作り直さない
      const mx = (view.maxX - view.minX) / 2 + 1;
      const my = (view.maxY - view.minY) / 2 + 1;
      this.#rect = {
        minX: Math.floor(view.minX - mx),
        minY: Math.floor(view.minY - my),
        maxX: Math.ceil(view.maxX + mx),
        maxY: Math.ceil(view.maxY + my),
      };
      all = true;
    }
    const rect = this.#rect;
    const out: DetailMeshes = {};
    if (all || this.#dirty.roofs) {
      out.roofs = roofGeometry(this.#buildings, rect);
    }
    if (all || this.#dirty.streets) {
      out.streets = streetGeometry(this.#segments, this.#nodes, rect);
    }
    if (all || this.#dirty.lamps) {
      out.lamps = lampGeometry(this.#lights, rect);
    }
    this.#dirty = { roofs: false, streets: false, lamps: false };
    if (out.roofs === undefined && out.streets === undefined && out.lamps === undefined) {
      return null;
    }
    this.builds++;
    return out;
  }
}
