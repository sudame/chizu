import { describe, expect, test } from "vitest";
import { Geometry, roadGeometry, VERTEX_FLOATS } from "./roads.ts";

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
