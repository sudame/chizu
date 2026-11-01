import { describe, expect, test } from "vitest";
import {
  buildingColor,
  buildingGeometry,
  FACILITY_COLOR,
  Geometry,
  markCenter,
  roadGeometry,
  roadLaneGeometry,
  roadMeshes,
  unsuppliedCount,
  VERTEX_FLOATS,
} from "./roads.ts";

describe("建物", () => {
  const brightness = (c: readonly number[]) => (c[0] ?? 0) + (c[1] ?? 0) + (c[2] ?? 0);

  test("段階が上がるほど明るく、区画の色を保つ", () => {
    for (let level = 1; level < 5; level++) {
      expect(brightness(buildingColor(2, level + 1))).toBeGreaterThan(
        brightness(buildingColor(2, level)),
      );
    }
    const [r = 0, g = 0, b = 0] = buildingColor(2, 3);
    expect(b).toBeGreaterThan(r);
    expect(b).toBeGreaterThan(g);
  });

  test("建物ごとに 6 頂点で、少し内側に縮めて描く", () => {
    const g = buildingGeometry(
      new Float64Array([10, 20, 12, 21, 1, 1, 3, 30, 20, 30, 20, 3, 5, 3]),
      new Float64Array(),
    );
    expect(g.vertexCount).toBe(12);
    const data = g.build();
    expect(data[0]).toBeCloseTo(0.15);
    expect(data[VERTEX_FLOATS]).toBeCloseTo(3 - 0.15);
  });

  test("施設を先に描き、電気か水道の届かない建物には印を足す", () => {
    const facilities = new Float64Array([0, 0, 3, 3, 1]);
    // 電気も水道も届く、電気だけ届く、どちらも届かない
    const supplies = [3, 1, 0];
    const buildings = new Float64Array(
      supplies.flatMap((supply, i) => [10 + 2 * i, 20, 10 + 2 * i, 20, 1, 1, supply]),
    );
    const g = buildingGeometry(buildings, facilities);
    // 施設 1 つ、建物 3 つ、印 3 つ（水道 1、電気と水道 2）
    expect(g.vertexCount).toBe(6 * (1 + 3 + 3));
    expect(unsuppliedCount(buildings)).toBe(2);
    const data = g.build();
    expect([...data.subarray(5, 9)]).toEqual([...(FACILITY_COLOR[1] ?? [])].map(Math.fround));
    // 4 つめの四角が、電気だけ届く建物の水道の印。基準のマスは最初の建物の左上 (10, 20)
    const water = markCenter({ x: 12, y: 20 }, "water");
    expect(data[6 * 4 * VERTEX_FLOATS]).toBeCloseTo(water.x - 10);
    expect(data[6 * 4 * VERTEX_FLOATS + 1]).toBeCloseTo(water.y - 20);
    expect(water).toEqual({ x: 12.75, y: 20.3 });
  });
});

describe("Geometry", () => {
  test("区間は 2 つの三角形で、位置は基準のマスからの相対でマスの中心に置く", () => {
    const g = new Geometry({ x: 100, y: 100 });
    g.segment({ x: 100, y: 100 }, { x: 104, y: 100 }, 0.3, [0, 0, 0, 1]);
    expect(g.vertexCount).toBe(6);
    const data = g.build();
    expect(data[0]).toBe(0.5);
    expect(data[1]).toBe(0.5);
    expect(data[3]).toBe(1);
    expect(data[4]).toBeCloseTo(0.3);
    expect(data[2 * VERTEX_FLOATS]).toBe(4.5);
  });

  test("長さのない区間は描かない", () => {
    const g = new Geometry({ x: 0, y: 0 });
    g.segment({ x: 1, y: 1 }, { x: 1, y: 1 }, 0.3, [0, 0, 0, 1]);
    expect(g.vertexCount).toBe(0);
  });

  test("届いた道路網は、区間ごとに 6 頂点、ノードごとに 6 頂点にする", () => {
    const g = roadGeometry(new Float64Array([0, 0, 4, 0, 1]), new Float64Array([0, 0, 1, 4, 0, 1]));
    expect(g.vertexCount).toBe(18);
    expect(roadGeometry(new Float64Array(), new Float64Array()).vertexCount).toBe(0);
  });
});

describe("車線ごとの道路", () => {
  test("中くらいの道路 1 本は、車道、歩道 2、中央線、車線の区切り 2 の 6 本の帯になる", () => {
    const g = roadLaneGeometry(
      new Float64Array([0, 0, 4, 0, 2]),
      new Float64Array([0, 0, 1, 4, 0, 1]),
    );
    expect(g.vertexCount).toBe(6 * 6);
  });

  test("帯は中心線の左右へ車線の幅ごとにずれて並ぶ", () => {
    const g = roadLaneGeometry(
      new Float64Array([0, 0, 4, 0, 2]),
      new Float64Array([0, 0, 1, 4, 0, 1]),
    );
    const centers = new Set<number>();
    const data = g.build();
    for (let i = 0; i < g.vertexCount; i += 6) {
      // 帯の始点の位置。法線の分は含まない
      centers.add(Number((data[i * VERTEX_FLOATS + 1] ?? 0).toFixed(3)));
    }
    // 基準のマスは (0, 0) で、道路の中心線は y = 0.5。中くらいの道路は車線が片側 2 本なので、
    // 車道の端から順に歩道（0.05、0.95）、区切り（0.3、0.7）、中心線（0.5）が並ぶ
    expect([...centers].sort((a, b) => a - b)).toEqual([0.05, 0.3, 0.5, 0.7, 0.95]);
  });

  test("細い線の半分の太さは負で渡し、車道の帯は正で渡す", () => {
    const g = roadLaneGeometry(
      new Float64Array([0, 0, 4, 0, 3]),
      new Float64Array([0, 0, 1, 4, 0, 1]),
    );
    const data = g.build();
    const halves = new Set<number>();
    for (let i = 0; i < g.vertexCount; i++) {
      halves.add(Math.sign(data[i * VERTEX_FLOATS + 4] ?? 0));
    }
    expect([...halves].sort()).toEqual([-1, 1]);
    // 最初の帯が車道で、半分の太さは車線 3 本分
    expect(data[4]).toBeCloseTo(0.6);
  });

  test("3 本以上が集まる交差点では、線が交差点の中を横切らないよう縮める", () => {
    // (4, 0) に 3 本が集まる
    const segments = new Float64Array([0, 0, 4, 0, 1, 4, 0, 8, 0, 1, 4, 0, 4, 4, 1]);
    const nodes = new Float64Array([0, 0, 1, 4, 0, 0, 8, 0, 1, 4, 4, 1]);
    const g = roadLaneGeometry(segments, nodes);
    const data = g.build();
    // 帯 4 本（車道、歩道 2、中央線）× 3 区間 + 交差点の四角 1 つ = 6 × 13
    expect(g.vertexCount).toBe(6 * 13);
    // 描く順は、車道 3 区間、歩道 6 本、交差点の四角、中央線 3 本。
    // 区間 (0,0)-(4,0) の中央線は、終点側が交差点の半分の幅（0.3）だけ手前で終わる
    const center = 3 * 6 + 6 * 6 + 6;
    const endX = data[(center + 2) * VERTEX_FLOATS] ?? 0;
    expect(endX).toBeCloseTo(4.5 - 0.3);
  });

  test("縮小用と拡大用の両方の頂点を作る", () => {
    const meshes = roadMeshes(
      new Float64Array([0, 0, 4, 0, 1]),
      new Float64Array([0, 0, 1, 4, 0, 1]),
    );
    expect(meshes.simple.vertexCount).toBe(18);
    expect(meshes.lanes.vertexCount).toBeGreaterThan(18);
  });
});
