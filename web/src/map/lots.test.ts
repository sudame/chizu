import { describe, expect, test } from "vitest";
import { roofGeometry } from "./detail.ts";
import {
  carve,
  grow,
  LOT_EDGE_COLOR,
  LOT_FACE_COLOR,
  LOT_LINE_COLOR,
  largestPiece,
  parseLots,
  type Rect,
  subtract,
} from "./lots.ts";
import { buildingMeshes, type Geometry, roadDrawOrder, VERTEX_FLOATS } from "./roads.ts";

const area = (r: Rect) => (r.x1 - r.x0) * (r.y1 - r.y0);

const overlaps = (a: Rect, b: Rect) =>
  a.x0 < b.x1 - 1e-9 && b.x0 < a.x1 - 1e-9 && a.y0 < b.y1 - 1e-9 && b.y0 < a.y1 - 1e-9;

/** 長方形（6 頂点）を並べた頂点から、ワールド座標の長方形を取り出す。 */
function boxes(g: Geometry): Rect[] {
  const data = g.build();
  const out: Rect[] = [];
  for (let i = 0; i + 6 * VERTEX_FLOATS <= data.length; i += 6 * VERTEX_FLOATS) {
    const xs: number[] = [];
    const ys: number[] = [];
    for (let k = 0; k < 6; k++) {
      xs.push((data[i + k * VERTEX_FLOATS] ?? 0) + g.origin.x);
      ys.push((data[i + k * VERTEX_FLOATS + 1] ?? 0) + g.origin.y);
    }
    out.push({
      x0: Math.min(...xs),
      y0: Math.min(...ys),
      x1: Math.max(...xs),
      y1: Math.max(...ys),
    });
  }
  return out;
}

const colorOf = (g: Geometry, vertex: number) =>
  Array.from(g.build().subarray(vertex * VERTEX_FLOATS + 6, vertex * VERTEX_FLOATS + 10));
const f32 = (c: readonly number[]) => Array.from(new Float32Array(c));

// 4×4 の建物 (10, 20)〜(13, 23)。道路は南（y の大きい側）で、西の端の 2 列を切り欠く。
// 1 列めは奥行き 4 マスすべて、2 列めは道路の側の 1 マスだけ（5 マス）。枠の並びは 2 本
const BUILDING = [10, 20, 13, 23, 4, 5, 3];
const FACES: Rect[] = [
  { x0: 10, y0: 20, x1: 11, y1: 24 },
  { x0: 11, y0: 23, x1: 12, y1: 24 },
];
// 数の並び：見出し 6、長方形 2 つ、枠の並び 2 本
const LOT = [
  ...[10, 20, 14, 25, 2, 2],
  ...[10, 20, 11, 24],
  ...[11, 23, 12, 24],
  ...[10.375, 20, 11, 24, -1, 0, 12],
  ...[11, 23, 11.625, 24, 1, 0, 3],
];

describe("parseLots", () => {
  test("建物の左上のマスで引ける表に直す", () => {
    const second = LOT.map((v, i) => (i < 2 ? v + 20 : v));
    const lots = parseLots(new Float64Array([...LOT, ...second]));
    expect([...lots.keys()]).toEqual(["10,20", "30,40"]);
  });

  test("見出しの数のとおりに長方形と枠の並びを読む", () => {
    const lot = parseLots(new Float64Array(LOT)).get("10,20");
    expect(lot?.surface).toBe(14);
    expect(lot?.indoor).toBe(25);
    expect(lot?.faces).toEqual(FACES);
    expect(lot?.strips[0]).toEqual({
      x0: 10.375,
      y0: 20,
      x1: 11,
      y1: 24,
      openX: -1,
      openY: 0,
      count: 12,
    });
    expect(lot?.strips[1]?.count).toBe(3);
  });

  test("形が崩れていれば、読めたところまでにする", () => {
    expect(parseLots(new Float64Array(LOT.slice(0, -1))).size).toBe(0);
    expect(parseLots(new Float64Array()).size).toBe(0);
  });
});

describe("切り欠いた残りの形", () => {
  test("長方形から別の長方形を除くと、重ならない残りが面積を保つ", () => {
    const r = { x0: 0, y0: 0, x1: 4, y1: 4 };
    const cut = { x0: 0, y0: 1, x1: 2, y1: 3 };
    const rest = subtract(r, cut);
    expect(rest.reduce((a, p) => a + area(p), 0)).toBeCloseTo(area(r) - area(cut));
    for (const [i, p] of rest.entries()) {
      expect(overlaps(p, cut)).toBe(false);
      for (const q of rest.slice(i + 1)) {
        expect(overlaps(p, q)).toBe(false);
      }
    }
  });

  test("重ならない切り欠きは何も除かず、全体を覆う切り欠きは何も残さない", () => {
    const r = { x0: 0, y0: 0, x1: 2, y1: 2 };
    expect(subtract(r, { x0: 5, y0: 5, x1: 6, y1: 6 })).toEqual([r]);
    expect(carve(r, [grow(r, 1)])).toEqual([]);
  });

  test("2 つの切り欠きを除いた L 字は、いちばん広い長方形を選べる", () => {
    const site = { x0: 10, y0: 20, x1: 14, y1: 24 };
    const rest = carve(site, FACES);
    expect(rest.reduce((a, p) => a + area(p), 0)).toBeCloseTo(16 - 5);
    // 道路の側の 1 マスを除いた奥の 3 列（3×3）が、東の 2 列（2×4）より広い
    expect(largestPiece(rest)).toEqual({ x0: 11, y0: 20, x1: 14, y1: 23 });
  });
});

describe("駐車場のある建物の絵", () => {
  const lots = new Float64Array(LOT);
  const m = buildingMeshes(new Float64Array(BUILDING), new Float64Array(), lots);

  test("標準の段のまとまりも、寄りの段の影、縁、本体も、切り欠いた所に重ならない", () => {
    for (const g of [m.clusters, m.details, m.bodies]) {
      const pieces = boxes(g);
      expect(pieces.length).toBeGreaterThan(0);
      for (const p of pieces) {
        for (const f of FACES) {
          // 影の右下へのずれは画面のピクセルで頂点の法線に持つので、位置は切り欠きに重ならない
          expect(overlaps(p, f)).toBe(false);
        }
      }
    }
  });

  test("まとまりの面積は、敷地を縮めた長方形から切り欠いた分を引いたものである", () => {
    const cut = FACES.reduce((a, f) => a + area(f), 0);
    const clustered = boxes(m.clusters).reduce((a, p) => a + area(p), 0);
    const inset = 0.06;
    const shrunk = (4 - 2 * inset) ** 2;
    // 切り欠きは敷地の外周に接するので、縮めた長方形からはみ出す分は引かない
    expect(clustered).toBeLessThan(shrunk);
    expect(clustered).toBeGreaterThan(shrunk - cut);
  });

  test("駐車場の面は切り欠いた所と同じで、色は地図の決まりの面の色である", () => {
    expect(boxes(m.lots)).toEqual(FACES);
    expect(colorOf(m.lots, 0)).toEqual(f32(LOT_FACE_COLOR));
  });

  test("縁は敷地の外周に接する辺だけに引き、本体に接する辺には引かない", () => {
    // 1 つめの長方形は西、北、南の 3 辺が外周で、東は本体との境である。2 つめは南の 1 辺だけ
    expect(m.lotEdges.vertexCount).toBe(6 * (3 + 1));
    expect(colorOf(m.lotEdges, 0)).toEqual(f32(LOT_EDGE_COLOR));
  });

  test("枠の白線は、枠を分ける線と奥の辺の線で、車の入る側の辺には引かない", () => {
    // 12 枠の並びは 13 本と奥の 1 本、3 枠の並びは 4 本と奥の 1 本
    expect(m.lotLines.vertexCount).toBe(6 * (13 + 1 + 4 + 1));
    expect(colorOf(m.lotLines, 0)).toEqual(f32(LOT_LINE_COLOR));
  });

  test("駐車場のない建物は、これまでと同じ絵になる", () => {
    const plain = buildingMeshes(new Float64Array(BUILDING), new Float64Array());
    expect(plain.lots.vertexCount).toBe(0);
    expect(plain.lotEdges.vertexCount).toBe(0);
    expect(plain.lotLines.vertexCount).toBe(0);
    expect(plain.bodies.vertexCount).toBe(6);
    expect(plain.clusters.vertexCount).toBe(6);
  });

  test("細部の段の屋根は、切り欠いた所に置かない", () => {
    const wide = { minX: 0, minY: 0, maxX: 40, maxY: 40 };
    const g = roofGeometry(new Float64Array(BUILDING), wide, parseLots(lots));
    const data = g.build();
    expect(data.length).toBeGreaterThan(0);
    for (let i = 0; i < data.length; i += VERTEX_FLOATS) {
      const x = (data[i] ?? 0) + g.origin.x;
      const y = (data[i + 1] ?? 0) + g.origin.y;
      for (const f of FACES) {
        const inside = x > f.x0 + 1e-9 && x < f.x1 - 1e-9 && y > f.y0 + 1e-9 && y < f.y1 - 1e-9;
        expect(inside).toBe(false);
      }
    }
  });
});

describe("段ごとの駐車場", () => {
  const alpha = (zoom: number, name: string) =>
    roadDrawOrder(zoom).find(([n]) => n === name)?.[1] ?? Number.NaN;

  test("引きでは何も描かない", () => {
    for (const name of ["lots", "lotEdges", "lotLines"]) {
      expect(alpha(1, name)).toBe(0);
      expect(alpha(2.5, name)).toBe(0);
    }
  });

  test("標準では面だけを描き、縁と枠の線は描かない", () => {
    expect(alpha(8, "lots")).toBe(1);
    expect(alpha(8, "lotEdges")).toBe(0);
    expect(alpha(8, "lotLines")).toBe(0);
    // 引きから標準へは、建物のまとまりと同じ倍率の幅で溶け込む
    expect(alpha(4, "lots")).toBeGreaterThan(0);
    expect(alpha(4, "lots")).toBeLessThan(1);
  });

  test("寄りでは、枠の白線を車と人と同じ倍率 12〜16 で溶け込ませる", () => {
    expect(alpha(12, "lotLines")).toBe(0);
    expect(alpha(14, "lotLines")).toBeGreaterThan(0);
    expect(alpha(14, "lotLines")).toBeLessThan(1);
    expect(alpha(16, "lotLines")).toBe(1);
    expect(alpha(20, "lots")).toBe(1);
    expect(alpha(20, "lotEdges")).toBe(1);
  });

  test("細部でも、寄りと同じ描き方のままである", () => {
    for (const name of ["lots", "lotEdges", "lotLines"]) {
      expect(alpha(64, name)).toBe(1);
    }
  });
});
