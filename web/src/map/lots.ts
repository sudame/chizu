// 建物の駐車場（docs/design.md の 3.6「地図の描き方」と、docs/map-style.md の「駐車場」）。
// 敷地を切り欠いた所の形と位置は中核が決める（chizu-core の parking_layout と lot_end）。chizu-wasm の lots が
// 切り欠いた所の長方形と枠の並びをワールドのマスの座標で送ってくるので、ここはそれを描く頂点に直すだけで、
// 切り欠き方を画面の側で決め直さない。
// 描くものは段で変わる。引きでは描かず、標準では切り欠いた所を淡い面で塗り、寄りでは縁と枠の白線を足す。
// 建物の絵は、切り欠いた所を除いて描く。
// 私道の敷地（docs/design.md の 3.8）は、私道の長方形と家の長方形も届く。私道は駐車場の面と同じ段で描き、
// 建物の本体は家の長方形だけにする。

import {
  LOT_FACE_STRIDE,
  LOT_HEADER,
  LOT_STRIP_STRIDE,
  lotRecordLength,
} from "../protocol/messages.ts";
import { hex } from "./palette.ts";
import type { Geometry } from "./roads.ts";

/** 駐車場の面の色（docs/map-style.md の色の表の「駐車場」）。 */
export const LOT_FACE_COLOR = hex("#e3e1dc");
/** 駐車場の縁の色。寄りで 1px の線にする。 */
export const LOT_EDGE_COLOR = hex("#c9c4b9");
/** 枠の白線の色。寄りで 1px の線にする。 */
export const LOT_LINE_COLOR = hex("#ffffff");
/** 私道の面の色（docs/map-style.md の色の表の「私道」）。 */
export const LANE_FACE_COLOR = hex("#fbfaf6");
/** 私道の縁の色。寄りで 1px の線にする。 */
export const LANE_EDGE_COLOR = hex("#d6d0c4");
/** 縁と白線の太さの半分（画面のピクセル）。線の太さは 1px である。 */
export const LOT_LINE_HALF_PX = 0.5;

/** ワールド座標（マス）の長方形。 */
export type Rect = { x0: number; y0: number; x1: number; y1: number };

/** 枠の並び 1 本。枠は車の入る側（open）に直角な向きに、count 個並ぶ。 */
export type Strip = Rect & { openX: number; openY: number; count: number };

/** 建物 1 つの駐車場の切り欠き。 */
export type Lot = {
  /** 平面の台数。 */
  surface: number;
  /** 建物の中の台数。 */
  indoor: number;
  /** 切り欠いた所を覆う、重ならない長方形。 */
  faces: Rect[];
  strips: Strip[];
  /** 私道の長方形。私道の敷地でなければ空。 */
  lanes: Rect[];
  /** 私道の脇の家の長方形。空でなければ、建物の本体はこの家だけになる。 */
  houses: Rect[];
};

/** 建物の左上のマスから駐車場を引く鍵。 */
export const lotKey = (x: number, y: number) => `${x},${y}`;

/** chizu-wasm の lots の数を、建物の左上のマスで引ける表にする。形が崩れていれば、読めたところまでにする。 */
export function parseLots(data: ArrayLike<number>): Map<string, Lot> {
  const lots = new Map<string, Lot>();
  let i = 0;
  while (i + LOT_HEADER <= data.length) {
    const at = (k: number) => data[i + k] ?? 0;
    const nFaces = at(4);
    const nStrips = at(5);
    const nLanes = at(6);
    const nHouses = at(7);
    const end = i + lotRecordLength(nFaces, nStrips, nLanes, nHouses);
    if (!(end <= data.length)) {
      break;
    }
    const strips: Strip[] = [];
    let k = i + LOT_HEADER;
    const rects = (n: number): Rect[] => {
      const out: Rect[] = [];
      for (let f = 0; f < n; f++, k += LOT_FACE_STRIDE) {
        out.push({
          x0: data[k] ?? 0,
          y0: data[k + 1] ?? 0,
          x1: data[k + 2] ?? 0,
          y1: data[k + 3] ?? 0,
        });
      }
      return out;
    };
    const faces = rects(nFaces);
    for (let s = 0; s < nStrips; s++, k += LOT_STRIP_STRIDE) {
      strips.push({
        x0: data[k] ?? 0,
        y0: data[k + 1] ?? 0,
        x1: data[k + 2] ?? 0,
        y1: data[k + 3] ?? 0,
        openX: data[k + 4] ?? 0,
        openY: data[k + 5] ?? 0,
        count: data[k + 6] ?? 0,
      });
    }
    const lanes = rects(nLanes);
    const houses = rects(nHouses);
    lots.set(lotKey(at(0), at(1)), { surface: at(2), indoor: at(3), faces, strips, lanes, houses });
    i = end;
  }
  return lots;
}

// 長方形の重なりの判定の誤差
const EPS = 1e-9;

const area = (r: Rect) => (r.x1 - r.x0) * (r.y1 - r.y0);

/** 長方形 r から長方形 cut を除いた残りを、重ならない長方形で返す。 */
export function subtract(r: Rect, cut: Rect): Rect[] {
  const x0 = Math.max(r.x0, cut.x0);
  const y0 = Math.max(r.y0, cut.y0);
  const x1 = Math.min(r.x1, cut.x1);
  const y1 = Math.min(r.y1, cut.y1);
  if (x1 - x0 <= EPS || y1 - y0 <= EPS) {
    return [r];
  }
  const out: Rect[] = [
    { x0: r.x0, y0: r.y0, x1: r.x1, y1: y0 },
    { x0: r.x0, y0: y1, x1: r.x1, y1: r.y1 },
    { x0: r.x0, y0, x1: x0, y1 },
    { x0: x1, y0, x1: r.x1, y1 },
  ];
  return out.filter((p) => p.x1 - p.x0 > EPS && p.y1 - p.y0 > EPS);
}

/** 長方形 r から cuts をすべて除いた残り。 */
export function carve(r: Rect, cuts: readonly Rect[]): Rect[] {
  return cuts.reduce<Rect[]>((pieces, cut) => pieces.flatMap((p) => subtract(p, cut)), [r]);
}

/** 長方形を 4 辺とも d（マス）だけ広げる（負なら縮める）。 */
export const grow = (r: Rect, d: number): Rect => ({
  x0: r.x0 - d,
  y0: r.y0 - d,
  x1: r.x1 + d,
  y1: r.y1 + d,
});

/** 2 つの長方形の重なり。重ならなければ null。 */
export function intersect(a: Rect, b: Rect): Rect | null {
  const r = {
    x0: Math.max(a.x0, b.x0),
    y0: Math.max(a.y0, b.y0),
    x1: Math.min(a.x1, b.x1),
    y1: Math.min(a.y1, b.y1),
  };
  return r.x1 - r.x0 > EPS && r.y1 - r.y0 > EPS ? r : null;
}

/** 建物の敷地（建物のマスの角から角まで）。 */
export const siteOf = (x0: number, y0: number, x1: number, y1: number): Rect => ({
  x0,
  y0,
  x1: x1 + 1,
  y1: y1 + 1,
});

/**
 * 切り欠きを除いた本体の、短い辺の下限（マス）。1×1 の建物の前面の帯が 2〜3 台になると、帯の奥行き 5m
 * （0.625 マス）のあとに残る本体が 3m しかなく、さらに縮めると壁 1 枚に見えるので、これを下回るときは縮めない。
 */
export const MIN_BODY = 0.3;

const shortSide = (r: Rect) => Math.min(r.x1 - r.x0, r.y1 - r.y0);

/**
 * 建物の本体の形。`site` を `inset` だけ縮めた長方形から、駐車場の切り欠きを除く。
 * `edge` は縁の幅で、縁の内側の本体は、切り欠きの側も縁の幅だけ内へ縮める。
 * 切り欠きを除いた本体のいちばん広い長方形が `MIN_BODY` を下回るときは、敷地を縮めずに（`inset` を 0 にして）作り直す。
 */
export function bodyShape(
  site: Rect,
  inset: number,
  edge: number,
  lot: Lot | undefined,
): { outer: Rect[]; inner: Rect[] } {
  // 私道の敷地は、中核の決めた家の長方形を、縮めずにそのまま本体にする
  if (lot !== undefined && lot.houses.length > 0) {
    return { outer: lot.houses, inner: lot.houses.map((h) => grow(h, -edge)) };
  }
  const faces = lot?.faces ?? [];
  const build = (d: number) => {
    const outer = grow(site, -d);
    return {
      outer: carve(outer, faces),
      inner: carve(
        grow(outer, -edge),
        faces.map((f) => grow(f, edge)),
      ),
    };
  };
  const shape = build(inset);
  const widest = largestPiece(shape.outer);
  if (lot !== undefined && widest !== undefined && shortSide(widest) < MIN_BODY && inset > 0) {
    return build(0);
  }
  return shape;
}

/** 切り欠きを除いた本体のうち、いちばん広い長方形。屋根や屋上の設備はここに置く。 */
export function largestPiece(pieces: readonly Rect[]): Rect | undefined {
  return pieces.reduce<Rect | undefined>(
    (best, p) => (best === undefined || area(p) > area(best) + EPS ? p : best),
    undefined,
  );
}

/** 駐車場を描く頂点。面は標準から、縁と白線は寄りから出す。 */
export type LotMeshes = { lots: Geometry; lotEdges: Geometry; lotLines: Geometry };

/**
 * 建物 1 つの駐車場を頂点にする。`site` は建物の敷地である。
 * 縁は、敷地の外周に接する辺だけに引く。建物の本体に接する辺には、本体の縁があるので引かない。
 */
export function addLot(meshes: LotMeshes, site: Rect, lot: Lot): void {
  for (const r of lot.lanes) {
    addLane(meshes, r);
  }
  for (const f of lot.faces) {
    meshes.lots.box(f.x0, f.y0, f.x1, f.y1, LOT_FACE_COLOR);
    const onBoundary = (v: number, edge: number) => Math.abs(v - edge) < EPS;
    const h = LOT_LINE_HALF_PX;
    if (onBoundary(f.y0, site.y0)) {
      meshes.lotEdges.line(f.x0, f.y0, f.x1, f.y0, 0, LOT_EDGE_COLOR, h);
    }
    if (onBoundary(f.y1, site.y1)) {
      meshes.lotEdges.line(f.x0, f.y1, f.x1, f.y1, 0, LOT_EDGE_COLOR, h);
    }
    if (onBoundary(f.x0, site.x0)) {
      meshes.lotEdges.line(f.x0, f.y0, f.x0, f.y1, 0, LOT_EDGE_COLOR, h);
    }
    if (onBoundary(f.x1, site.x1)) {
      meshes.lotEdges.line(f.x1, f.y0, f.x1, f.y1, 0, LOT_EDGE_COLOR, h);
    }
  }
  for (const s of lot.strips) {
    slotLines(meshes.lotLines, s);
  }
}

/**
 * 私道 1 本を頂点にする。面は駐車場の面と同じ段から出す。縁は長い辺の 2 本（敷地の外周の側と、家と枠のある側）に引き、
 * 道路に面した辺と行き止まりの辺には引かない。道路に面した辺は道路とつながり、行き止まりの辺は庭に続く。
 */
function addLane(meshes: LotMeshes, r: Rect): void {
  meshes.lots.box(r.x0, r.y0, r.x1, r.y1, LANE_FACE_COLOR);
  const h = LOT_LINE_HALF_PX;
  // 私道の長い辺の 2 本に縁を引く
  if (r.x1 - r.x0 < r.y1 - r.y0) {
    meshes.lotEdges.line(r.x0, r.y0, r.x0, r.y1, 0, LANE_EDGE_COLOR, h);
    meshes.lotEdges.line(r.x1, r.y0, r.x1, r.y1, 0, LANE_EDGE_COLOR, h);
  } else {
    meshes.lotEdges.line(r.x0, r.y0, r.x1, r.y0, 0, LANE_EDGE_COLOR, h);
    meshes.lotEdges.line(r.x0, r.y1, r.x1, r.y1, 0, LANE_EDGE_COLOR, h);
  }
}

/** 枠の白線。枠を分ける線と、車の入る側の反対の辺の線を引く。車の入る側は開けておく。 */
function slotLines(g: Geometry, s: Strip): void {
  const h = LOT_LINE_HALF_PX;
  if (s.count <= 0) {
    return;
  }
  if (s.openX !== 0) {
    const step = (s.y1 - s.y0) / s.count;
    for (let k = 0; k <= s.count; k++) {
      g.line(s.x0, s.y0 + step * k, s.x1, s.y0 + step * k, 0, LOT_LINE_COLOR, h);
    }
    const back = s.openX > 0 ? s.x0 : s.x1;
    g.line(back, s.y0, back, s.y1, 0, LOT_LINE_COLOR, h);
  } else {
    const step = (s.x1 - s.x0) / s.count;
    for (let k = 0; k <= s.count; k++) {
      g.line(s.x0 + step * k, s.y0, s.x0 + step * k, s.y1, 0, LOT_LINE_COLOR, h);
    }
    const back = s.openY > 0 ? s.y0 : s.y1;
    g.line(s.x0, back, s.x1, back, 0, LOT_LINE_COLOR, h);
  }
}
