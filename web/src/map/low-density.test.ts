import { describe, expect, test } from "vitest";
import { roofGeometry } from "./detail.ts";
import { homeCount, type Rect, siteOf } from "./lots.ts";
import {
  BUILDING_INSET,
  buildingColor,
  buildingMeshes,
  CLUSTER_INSET,
  type Geometry,
  VERTEX_FLOATS,
} from "./roads.ts";
import { ZONE_RESIDENTIAL, ZONE_RESIDENTIAL_LOW } from "./zones.ts";

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
const wide = { minX: 0, minY: 0, maxX: 40, maxY: 40 };
const building = (x0: number, y0: number, x1: number, y1: number, zone: number, level: number) =>
  new Float64Array([x0, y0, x1, y1, zone, level, 3]);
const mesh = (b: Float64Array) => buildingMeshes(b, new Float64Array());

describe("通りに面した低密度の家の棟の数", () => {
  const LOW = ZONE_RESIDENTIAL_LOW;
  test.each([
    // [w, h, 段階, 棟の数]。chizu-core の households と同じ式（4 マスあたり段階ぶん、切り上げ）
    [1, 1, 1, 1],
    [1, 1, 2, 1],
    [2, 1, 1, 1],
    [2, 1, 2, 1],
    [1, 2, 2, 1],
    [2, 2, 1, 1],
    [2, 2, 2, 2],
  ])("%i×%i の段階 %i は %i 棟", (w, h, level, homes) => {
    expect(homeCount(LOW, level, siteOf(0, 0, w - 1, h - 1), undefined)).toBe(homes);
  });

  test("高密度の住宅は、世帯が増えても棟に分けない", () => {
    expect(homeCount(ZONE_RESIDENTIAL, 2, siteOf(0, 0, 1, 1), undefined)).toBe(1);
  });

  test("私道の敷地（家の長方形が届いている）は、届いた家のままにする", () => {
    const lot = {
      surface: 0,
      indoor: 0,
      faces: [],
      strips: [],
      lanes: [],
      houses: [{ x0: 0, y0: 0, x1: 1, y1: 1 }],
    };
    expect(homeCount(LOW, 2, siteOf(0, 0, 1, 1), lot)).toBe(1);
  });

  test("棟の数は長い辺のマスの数を超えない", () => {
    expect(homeCount(LOW, 5, siteOf(0, 0, 0, 0), undefined)).toBe(1);
  });
});

describe("低密度の家の絵", () => {
  const LOW = ZONE_RESIDENTIAL_LOW;
  const HIGH = ZONE_RESIDENTIAL;

  test("世帯が 2 つの家は、標準でも寄りでも 2 棟に分かれ、棟のあいだに隙間がある", () => {
    const m = mesh(building(10, 20, 11, 21, LOW, 2));
    for (const g of [m.clusters, m.bodies]) {
      const b = boxes(g).sort((p, q) => p.x0 - q.x0);
      expect(b.length).toBe(2);
      expect(b[1]?.x0 ?? 0).toBeGreaterThan((b[0]?.x1 ?? 0) + 0.05);
    }
  });

  test("同じ大きさの高密度の住宅は、1 棟のままである", () => {
    const m = mesh(building(10, 20, 11, 21, HIGH, 2));
    expect(boxes(m.clusters).length).toBe(1);
    expect(boxes(m.bodies).length).toBe(1);
  });

  test("世帯が 1 つの家は、敷地を縮めた 1 棟である", () => {
    const m = mesh(building(10, 20, 10, 20, LOW, 1));
    const [c] = boxes(m.clusters);
    expect(c?.x0).toBeCloseTo(10 + CLUSTER_INSET);
    expect(c?.x1).toBeCloseTo(11 - CLUSTER_INSET);
    const [d] = boxes(m.details);
    expect(d?.x0).toBeCloseTo(10 + BUILDING_INSET);
  });

  test("低密度の建物は、高密度と違う色で塗る", () => {
    const low = mesh(building(10, 20, 10, 20, LOW, 1));
    const high = mesh(building(10, 20, 10, 20, HIGH, 1));
    expect(colorOf(low.clusters, 0)).toEqual(f32(buildingColor(LOW)));
    expect(colorOf(high.clusters, 0)).toEqual(f32(buildingColor(HIGH)));
    expect(colorOf(low.clusters, 0)).not.toEqual(colorOf(high.clusters, 0));
    expect(colorOf(low.bodies, 0)).not.toEqual(colorOf(high.bodies, 0));
    // details は影のあとに縁が続く。最後の頂点は縁の色である
    const last = (g: Geometry) => g.vertexCount - 1;
    expect(colorOf(low.details, last(low.details))).not.toEqual(
      colorOf(high.details, last(high.details)),
    );
  });

  test("細部の屋根は、低密度でも高密度でも寄棟で、棟ごとに描く", () => {
    // 正方形の棟は棟の線が点になるので、隅棟の 4 本になる。長方形の棟は棟と隅棟の 5 本である
    expect(roofGeometry(building(10, 20, 10, 20, LOW, 1), wide).vertexCount).toBe(6 * 4);
    expect(roofGeometry(building(10, 20, 10, 20, HIGH, 1), wide).vertexCount).toBe(6 * 4);
    expect(roofGeometry(building(10, 20, 11, 20, LOW, 2), wide).vertexCount).toBe(6 * 5);
    // 2×2 の段階 2 は、低密度なら 2 棟（1 棟ずつ縦長）、高密度なら 1 棟（正方形）
    expect(roofGeometry(building(10, 20, 11, 21, LOW, 2), wide).vertexCount).toBe(6 * 5 * 2);
    expect(roofGeometry(building(10, 20, 11, 21, HIGH, 2), wide).vertexCount).toBe(6 * 4);
  });

  test("屋根は棟の本体の内側に収まる", () => {
    const b = building(10, 20, 11, 21, LOW, 2);
    const g = roofGeometry(b, wide);
    const bodies = boxes(mesh(b).bodies);
    const data = g.build();
    for (let i = 0; i < data.length; i += VERTEX_FLOATS) {
      const x = (data[i] ?? 0) + g.origin.x;
      const y = (data[i + 1] ?? 0) + g.origin.y;
      expect(
        bodies.some(
          (r) => x > r.x0 - 1e-6 && x < r.x1 + 1e-6 && y > r.y0 - 1e-6 && y < r.y1 + 1e-6,
        ),
      ).toBe(true);
    }
  });
});
