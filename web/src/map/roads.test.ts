import { describe, expect, test } from "vitest";
import {
  buildingColor,
  buildingGeometry,
  FACILITY_COLOR,
  Geometry,
  markCenter,
  roadGeometry,
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
    // 施設 1 つ、建物 3 つ、印は水滴 2 つ（9 頂点）と稲妻 1 つ（6 頂点）
    expect(g.vertexCount).toBe(6 * (1 + 3) + 9 * 2 + 6);
    expect(unsuppliedCount(buildings)).toBe(2);
    const data = g.build();
    expect([...data.subarray(5, 9)]).toEqual([...(FACILITY_COLOR[1] ?? [])].map(Math.fround));
    // 印の最初が、電気だけ届く建物の水道の印。基準のマスは最初の建物の左上 (10, 20)
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
