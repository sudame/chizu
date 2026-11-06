import { describe, expect, test } from "vitest";
import { roofGeometry } from "./detail.ts";
import {
  carve,
  GARAGE_DECK_COLOR,
  GARAGE_DECK_EDGE_COLOR,
  GARAGE_DECK_INSET,
  grow,
  LANE_EDGE_COLOR,
  LANE_FACE_COLOR,
  LOT_EDGE_COLOR,
  LOT_FACE_COLOR,
  LOT_LINE_COLOR,
  largestPiece,
  MIN_BODY,
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
// 数の並び：見出し 8、長方形 2 つ、枠の並び 2 本
const LOT = [
  ...[10, 20, 14, 25, 2, 2, 0, 0],
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

describe("私道の敷地の絵", () => {
  // 2×6 の住宅 (10, 20)〜(11, 25)。道路は南（y の大きい側）で、西の端の列に私道が通る。
  // 家は 3 軒で、枠はそれぞれ私道の東に 1 台ずつ
  const SITE = [10, 20, 11, 25, 1, 5, 3];
  const lane = { x0: 10, y0: 21.14, x1: 10.375, y1: 26 };
  const houses = [
    { x0: 11.075, y0: 24.675, x1: 11.925, y1: 25.925 },
    { x0: 11.075, y0: 22.675, x1: 11.925, y1: 23.925 },
    { x0: 11.075, y0: 20.675, x1: 11.925, y1: 21.925 },
  ];
  const slots = houses.map((h) => ({ x0: 10.375, y0: h.y0 + 0.47, x1: 11, y1: h.y0 + 0.78 }));
  const rect = (r: Rect) => [r.x0, r.y0, r.x1, r.y1];
  const data = new Float64Array([
    ...[10, 20, 3, 0, 3, 3, 1, 3],
    ...slots.flatMap(rect),
    ...slots.flatMap((r) => [...rect(r), -1, 0, 1]),
    ...rect(lane),
    ...houses.flatMap(rect),
  ]);
  const m = buildingMeshes(new Float64Array(SITE), new Float64Array(), data);

  test("私道と家を読む", () => {
    const lot = parseLots(data).get("10,20");
    expect(lot?.lanes).toEqual([lane]);
    expect(lot?.houses).toEqual(houses);
    expect(lot?.strips.length).toBe(3);
  });

  test("建物の本体は家だけで、私道にも枠にも重ならない", () => {
    for (const g of [m.clusters, m.bodies]) {
      const pieces = boxes(g);
      expect(pieces.length).toBe(houses.length);
      for (const p of pieces) {
        expect(houses.some((h) => overlaps(p, h))).toBe(true);
        for (const f of [lane, ...slots]) {
          expect(overlaps(p, f)).toBe(false);
        }
      }
    }
  });

  test("私道は駐車場と同じ段の面で、私道の色で塗る", () => {
    const near = (a: Rect, b: Rect) =>
      Math.max(
        Math.abs(a.x0 - b.x0),
        Math.abs(a.y0 - b.y0),
        Math.abs(a.x1 - b.x1),
        Math.abs(a.y1 - b.y1),
      ) < 1e-4;
    expect(boxes(m.lots).some((b) => near(b, lane))).toBe(true);
    expect(colorOf(m.lots, 0)).toEqual(f32(LANE_FACE_COLOR));
    // 私道の縁は長い辺の 2 本、枠の縁は敷地の外周に接する辺がないので引かない
    expect(m.lotEdges.vertexCount).toBe(6 * 2);
    expect(colorOf(m.lotEdges, 0)).toEqual(f32(LANE_EDGE_COLOR));
  });

  test("細部の段の屋根は、家ごとに描く", () => {
    const wide = { minX: 0, minY: 0, maxX: 40, maxY: 40 };
    const g = roofGeometry(new Float64Array(SITE), wide, parseLots(data));
    // 寄棟の屋根は 1 軒につき棟と隅棟の 5 本
    expect(g.vertexCount).toBe(6 * 5 * houses.length);
  });
});

describe("1×1 の建物の前面の帯", () => {
  // 道路は南（y の大きい側）で、帯は西の端から道路の側に 0.625 マスの奥行きで、枠の数ぶんだけ延びる
  const strip = (slots: number): number[] => {
    const length = slots * 0.3125;
    const r = [10, 20.375, 10 + length, 21];
    return [10, 20, slots, 0, 1, 1, 0, 0, ...r, ...r, 0, -1, slots];
  };

  test.each([1, 2, 3])("枠が %i 台でも、本体は短い辺が下限以上ある", (slots) => {
    const m = buildingMeshes(
      new Float64Array([10, 20, 10, 20, 1, 5, 3]),
      new Float64Array(),
      new Float64Array(strip(slots)),
    );
    const short = (g: Geometry) => {
      const w = largestPiece(boxes(g)) as Rect;
      return Math.min(w.x1 - w.x0, w.y1 - w.y0);
    };
    // 標準のまとまりは下限以上、寄りの縁（縮めた本体）も下限以上ある
    expect(short(m.clusters)).toBeGreaterThanOrEqual(MIN_BODY - 1e-9);
    expect(short(m.details)).toBeGreaterThanOrEqual(MIN_BODY - 1e-9);
    // 縁の内側の本体も、壁 1 枚にならない（縁の幅 0.05 を両側から引いても 0.25 マス以上）
    expect(short(m.bodies)).toBeGreaterThanOrEqual(0.25);
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

describe("共同の駐車場の絵", () => {
  // 平面の駐車場は 2×2 マス (10, 20)〜(11, 21)。枠の並びだけで、切り欠く長方形は 0 個
  const PLANE = [10, 20, 11, 21, 3];
  const planeStrips = [
    ...[10.375, 20.0625, 11, 21.9375, -1, 0, 6],
    ...[11, 20.0625, 11.625, 21.9375, 1, 0, 6],
  ];
  const planeLot = new Float64Array([...[10, 20, 12, 0, 0, 2, 0, 0], ...planeStrips]);
  // 駐車場ビルは 4×4 マス (30, 20)〜(33, 23)。屋上の 48 枠を 4 本の並びで置く
  const GARAGE = [30, 20, 33, 23, 4];
  const garageLot = new Float64Array([
    ...[30, 20, 48, 192, 0, 1, 0, 0],
    ...[30.375, 20.125, 31, 23.875, -1, 0, 12],
  ]);

  test("枠の並びだけの駐車場は、枠の白線を引き、面と縁は切り欠かない", () => {
    const m = buildingMeshes(new Float64Array(), new Float64Array([...PLANE]), planeLot);
    expect(m.lots.vertexCount).toBe(0);
    expect(m.lotEdges.vertexCount).toBe(0);
    // 並び 1 本につき、枠を分ける線が 7 本と奥の線が 1 本
    expect(m.lotLines.vertexCount).toBe(6 * 8 * 2);
    expect(colorOf(m.lotLines, 0)).toEqual(f32(LOT_LINE_COLOR));
  });

  test("平面の駐車場に屋上の面は足さず、駐車場ビルだけに足す", () => {
    const plane = buildingMeshes(new Float64Array(), new Float64Array(PLANE), planeLot);
    const garage = buildingMeshes(new Float64Array(), new Float64Array(GARAGE), garageLot);
    // 施設の面は 5 つ（縁の 4 本と面）。駐車場ビルは屋上の面が 1 つ足される
    expect(plane.facilities.vertexCount).toBe(6 * 5);
    expect(garage.facilities.vertexCount).toBe(6 * 6);
    expect(plane.lotEdges.vertexCount).toBe(0);
    expect(garage.lotEdges.vertexCount).toBe(6 * 4);
    expect(colorOf(garage.lotEdges, 0)).toEqual(f32(GARAGE_DECK_EDGE_COLOR));
    const deck = boxes(garage.facilities).at(-1) as Rect;
    expect(deck.x0).toBeCloseTo(30 + GARAGE_DECK_INSET, 4);
    expect(deck.y1).toBeCloseTo(24 - GARAGE_DECK_INSET, 4);
    const verts = garage.facilities.vertexCount;
    expect(colorOf(garage.facilities, verts - 1)).toEqual(f32(GARAGE_DECK_COLOR));
  });

  test("駐車場ビルの屋上の面は、敷地の壁の内側に収まる", () => {
    const garage = buildingMeshes(new Float64Array(), new Float64Array(GARAGE), garageLot);
    const deck = boxes(garage.facilities).at(-1) as Rect;
    const site = { x0: 30, y0: 20, x1: 34, y1: 24 };
    expect(deck.x0).toBeGreaterThan(site.x0);
    expect(deck.x1).toBeLessThan(site.x1);
    expect(deck.y0).toBeGreaterThan(site.y0);
    expect(deck.y1).toBeLessThan(site.y1);
  });
});
