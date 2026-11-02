import { describe, expect, test } from "vitest";
import {
  DetailCache,
  EQUIPMENT_HALF,
  equipmentCenters,
  hasStreetTree,
  LAMP_SPACING,
  lampGeometry,
  ROOF_RIDGE_HALF_PX,
  ROOF_STRIPE_SPACING,
  roofGeometry,
  SHRUB_COLOR,
  SIGNAL_OFF_COLOR,
  STOP_LINE_WIDTH,
  STREET_TREE_COLOR,
  streetGeometry,
  type TileRect,
  tileHash,
} from "./detail.ts";
import { carriagewayHalfWidth } from "./lanes.ts";
import {
  CAR_FRONT_WINDOW,
  CAR_REAR_WINDOW,
  CAR_SHADOW_COLOR,
  WALKER_HEAD,
  WALKER_SHOULDER,
} from "./movers.ts";
import { hex } from "./palette.ts";
import {
  BUILDING_EDGE,
  BUILDING_INSET,
  buildingEdgeColor,
  DISC_SIDES,
  type Geometry,
  SIGNAL_BASE_COLOR,
  SIGNAL_COLOR,
  signalLampCenter,
  VERTEX_FLOATS,
  ZEBRA_COLOR,
} from "./roads.ts";

/** 頂点を、ワールド座標と色の組に直す。 */
function vertices(g: Geometry) {
  const data = g.build();
  const out = [];
  for (let i = 0; i < data.length; i += VERTEX_FLOATS) {
    out.push({
      x: (data[i] ?? 0) + g.origin.x,
      y: (data[i + 1] ?? 0) + g.origin.y,
      nx: data[i + 2] ?? 0,
      ny: data[i + 3] ?? 0,
      half: data[i + 4] ?? 0,
      min: data[i + 5] ?? 0,
      color: Array.from(data.subarray(i + 6, i + 10)).join(),
    });
  }
  return out;
}

const f32 = (c: readonly number[]) => Array.from(new Float32Array(c)).join();
const WIDE: TileRect = { minX: -100, minY: -100, maxX: 100, maxY: 100 };
const building = (x0: number, y0: number, x1: number, y1: number, zone: number) => [
  x0,
  y0,
  x1,
  y1,
  zone,
  1,
  3,
];

describe("建物の屋根", () => {
  // 本体（縁の内側）の端
  const lo = BUILDING_INSET + BUILDING_EDGE;

  test("住宅は、長い辺の向きの棟と、4 つの角から棟の両端への隅棟を、縁の色の 1.5px の線で引く", () => {
    // 横に 3 マスの住宅
    const g = roofGeometry(new Float64Array(building(0, 0, 2, 0, 1)), WIDE);
    const v = vertices(g);
    // 棟 1 本と隅棟 4 本
    expect(v.length).toBe(5 * 6);
    expect(v.every((p) => p.color === f32(buildingEdgeColor(1)))).toBe(true);
    expect(v.every((p) => p.half === 0 && p.min === ROOF_RIDGE_HALF_PX)).toBe(true);
    // 棟は横向きで、建物の高さの真ん中にあり、両端から高さの半分だけ内側で止まる
    const ridge = v.slice(0, 6);
    const depth = 1 - 2 * lo;
    expect(ridge.every((p) => Math.abs(p.y - 0.5) < 1e-6)).toBe(true);
    expect(Math.min(...ridge.map((p) => p.x))).toBeCloseTo(lo + depth / 2);
    expect(Math.max(...ridge.map((p) => p.x))).toBeCloseTo(3 - lo - depth / 2);
    // 隅棟は本体の角から始まる
    const xs = new Set(v.map((p) => Math.round(p.x * 1e4) / 1e4));
    expect(xs.has(Math.round(lo * 1e4) / 1e4)).toBe(true);
    expect(xs.has(Math.round((3 - lo) * 1e4) / 1e4)).toBe(true);
  });

  test("正方形の住宅は、棟が点になり、4 本の隅棟が真ん中で会う", () => {
    const v = vertices(roofGeometry(new Float64Array(building(2, 2, 2, 2, 1)), WIDE));
    expect(v.length).toBe(4 * 6);
    expect(v.filter((p) => Math.abs(p.x - 2.5) < 1e-4 && Math.abs(p.y - 2.5) < 1e-4).length).toBe(
      4 * 3,
    );
  });

  test("縦に長い住宅は、棟を縦に引く", () => {
    const v = vertices(roofGeometry(new Float64Array(building(5, 5, 5, 7, 1)), WIDE));
    const ridge = v.slice(0, 6);
    expect(ridge.every((p) => Math.abs(p.x - 5.5) < 1e-6)).toBe(true);
  });

  test("工業は、長い辺の向きに 0.25 マスごとの屋根の筋を引く", () => {
    const v = vertices(roofGeometry(new Float64Array(building(0, 0, 3, 1, 3)), WIDE));
    expect(v.every((p) => p.color === f32(buildingEdgeColor(3)))).toBe(true);
    const ys = [...new Set(v.map((p) => Math.round(p.y * 1e4) / 1e4))].sort((a, b) => a - b);
    // 本体の高さ 2 - 2 × 0.2 = 1.6 マスに、0.25 マスおき（縁から 0.25 マス、1.5 マスは縁に近すぎるので引かない）
    expect(ys.length).toBe(5);
    for (let i = 1; i < ys.length; i++) {
      expect((ys[i] ?? 0) - (ys[i - 1] ?? 0)).toBeCloseTo(ROOF_STRIPE_SPACING);
    }
    // 筋は横向きで、本体の左端から右端まで
    expect(Math.min(...v.map((p) => p.x))).toBeCloseTo(lo);
    expect(Math.max(...v.map((p) => p.x))).toBeCloseTo(4 - lo);
  });

  test("商業とオフィスは、建物の左上のマスから決めた 1〜2 個の設備を、本体の中に置く", () => {
    for (const zone of [2, 4]) {
      for (let x = 0; x < 12; x++) {
        const g = roofGeometry(new Float64Array(building(x, 3, x + 1, 4, zone)), WIDE);
        const v = vertices(g);
        const count = v.length / 6;
        expect(count === 1 || count === 2).toBe(true);
        expect(count).toBe(1 + (tileHash(x, 3) & 1));
        expect(v.every((p) => p.color === f32(buildingEdgeColor(zone)))).toBe(true);
        for (const p of v) {
          expect(p.x).toBeGreaterThanOrEqual(x + lo - 1e-6);
          expect(p.x).toBeLessThanOrEqual(x + 2 - lo + 1e-6);
          expect(p.y).toBeGreaterThanOrEqual(3 + lo - 1e-6);
          expect(p.y).toBeLessThanOrEqual(5 - lo + 1e-6);
        }
        // 同じ場所なら同じ絵になる
        expect(
          roofGeometry(new Float64Array(building(x, 3, x + 1, 4, zone)), WIDE).build(),
        ).toEqual(g.build());
      }
    }
  });

  test("設備は 0.25 マス角で、2 個のときは長い辺を分けた別々の区切りに置く", () => {
    const centers = equipmentCenters(0, 0, 3, 1, 0b11);
    expect(centers.length).toBe(2);
    const [a, b] = centers;
    expect(Math.abs((a?.x ?? 0) - (b?.x ?? 0))).toBeGreaterThanOrEqual(2 * EQUIPMENT_HALF);
    expect(a?.x ?? 0).toBeLessThan(1.5);
    expect(b?.x ?? 0).toBeGreaterThan(1.5);
  });

  test("範囲にかからない建物は描かない", () => {
    const buildings = new Float64Array([
      ...building(0, 0, 1, 0, 1),
      ...building(50, 50, 51, 50, 1),
    ]);
    const v = vertices(roofGeometry(buildings, { minX: 40, minY: 40, maxX: 60, maxY: 60 }));
    expect(v.length).toBe(5 * 6);
    expect(v.every((p) => p.x > 50)).toBe(true);
  });
});

describe("道路の細部", () => {
  const nodesOf = (...tiles: [number, number][]) =>
    new Float64Array(tiles.flatMap(([x, y]) => [x, y, 1, 0, 0]));

  test("歩道の外に、2 マスごとの街路樹と、そのあいだに 0.25 マスおきの低木を並べる", () => {
    // 横の小の道路。両端の行き止まりのマスには植え込みがないので、x = 1〜7 に並べる
    const v = vertices(
      streetGeometry(new Float64Array([0, 4, 8, 4, 1]), nodesOf([0, 4], [8, 4]), WIDE),
    );
    const discs = (color: readonly number[]) => {
      const centers = v.filter((p) => p.color === f32(color) && p.nx === 0 && p.ny === 0);
      return [...new Set(centers.map((p) => `${Math.round(p.x * 1e4) / 1e4},${p.y.toFixed(4)}`))];
    };
    const trees = discs(STREET_TREE_COLOR);
    const shrubs = discs(SHRUB_COLOR);
    // 街路樹は、x + y が偶数のマス（x = 2、4、6）の両側
    expect(trees.length).toBe(3 * 2);
    expect(hasStreetTree({ x: 2, y: 4 })).toBe(true);
    expect(hasStreetTree({ x: 3, y: 4 })).toBe(false);
    // 低木は、街路樹のないマスに 4 つ、あるマスに 2 つずつ、両側に
    expect(shrubs.length).toBe((4 * 4 + 3 * 2) * 2);
    // 丸は八角形で、街路樹は低木の上に描く
    expect(v.length).toBe((trees.length + shrubs.length) * DISC_SIDES * 3);
    const firstTree = v.findIndex((p) => p.color === f32(STREET_TREE_COLOR));
    const lastShrub = v.map((p) => p.color).lastIndexOf(f32(SHRUB_COLOR));
    expect(lastShrub).toBeLessThan(firstTree);
    // 植え込みは歩道の外（中心から 0.38 マスより外）からマスの縁までのあいだ
    const ys = new Set(trees.map((t) => Number(t.split(",")[1])));
    for (const y of ys) {
      expect(Math.abs(y - 4.5)).toBeGreaterThan(carriagewayHalfWidth(1) + 0.08);
      expect(Math.abs(y - 4.5)).toBeLessThan(0.5);
    }
  });

  test("通り抜けるノードのマスにも、1 度だけ植える。高速道路と斜めの区間には植えない", () => {
    const through = vertices(
      streetGeometry(
        new Float64Array([0, 0, 4, 0, 2, 4, 0, 8, 0, 2]),
        nodesOf([0, 0], [4, 0], [8, 0]),
        WIDE,
      ),
    );
    const one = vertices(
      streetGeometry(new Float64Array([0, 0, 8, 0, 2]), nodesOf([0, 0], [8, 0]), WIDE),
    );
    expect(through.length).toBe(one.length);
    const highway = streetGeometry(
      new Float64Array([0, 0, 8, 0, 4]),
      nodesOf([0, 0], [8, 0]),
      WIDE,
    );
    expect(highway.vertexCount).toBe(0);
    const diagonal = streetGeometry(
      new Float64Array([0, 0, 6, 6, 2]),
      nodesOf([0, 0], [6, 6]),
      WIDE,
    );
    expect(diagonal.vertexCount).toBe(0);
  });

  test("交差点の腕ごとに、横断歩道の外に、入ってくる車線の幅だけ停止線を引く", () => {
    // (4, 4) に 3 本が集まる。東の腕だけを見る
    const segments = new Float64Array([0, 4, 4, 4, 2, 4, 4, 8, 4, 2, 4, 4, 4, 8, 2]);
    const nodes = nodesOf([0, 4], [4, 4], [8, 4], [4, 8]);
    const v = vertices(streetGeometry(segments, nodes, WIDE)).filter(
      (p) => p.color === f32(ZEBRA_COLOR),
    );
    expect(v.length).toBe(3 * 6);
    const east = v.filter((p) => p.x > 5);
    expect(east.length).toBe(6);
    // 腕の向きの太さは 0.04 マス
    const xs = east.map((p) => p.x);
    expect(Math.max(...xs) - Math.min(...xs)).toBeCloseTo(STOP_LINE_WIDTH);
    // 東の腕から入ってくる車は西へ進み、左側通行なので南（y の大きい側）の車線を走る
    const carriage = carriagewayHalfWidth(2);
    const ys = east.map((p) => p.y + p.nx * 0 + p.ny * p.half);
    expect(Math.min(...ys)).toBeCloseTo(4.5);
    expect(Math.max(...ys)).toBeCloseTo(4.5 + carriage);
  });

  test("範囲にかからない道路の細部は作らない", () => {
    const g = streetGeometry(new Float64Array([0, 4, 8, 4, 1]), nodesOf([0, 4], [8, 4]), {
      minX: 50,
      minY: 50,
      maxX: 60,
      maxY: 60,
    });
    expect(g.vertexCount).toBe(0);
  });
});

describe("3 つ並びの信号", () => {
  test("腕を横切る台に、入ってくる車から見て左から青、黄、赤を並べ、点いている灯火だけを明るくする", () => {
    // (4, 5) の東の腕が赤
    const v = vertices(lampGeometry(new Float64Array([4, 5, 1, 0, 2]), WIDE));
    // 台 1 本と灯火 3 つ
    expect(v.length).toBe(6 + 3 * DISC_SIDES * 3);
    expect(v.slice(0, 6).every((p) => p.color === f32(SIGNAL_BASE_COLOR))).toBe(true);
    const c = signalLampCenter(4, 5, 1, 0);
    const lampAt = (k: number) => v.slice(6 + k * DISC_SIDES * 3, 6 + (k + 1) * DISC_SIDES * 3);
    const lamps = [0, 1, 2].map(lampAt);
    // 東の腕から入る車は西へ進むので、左は南（y の大きい側）。青がいちばん南、赤がいちばん北
    expect(lamps.map((l) => (l[0]?.y ?? 0) - c.y)).toEqual([
      expect.closeTo(LAMP_SPACING),
      expect.closeTo(0),
      expect.closeTo(-LAMP_SPACING),
    ]);
    expect(lamps.every((l) => l.every((p) => Math.abs(p.x - c.x) < 1e-4))).toBe(true);
    expect(lamps[0]?.[0]?.color).toBe(f32(SIGNAL_OFF_COLOR));
    expect(lamps[1]?.[0]?.color).toBe(f32(SIGNAL_OFF_COLOR));
    expect(lamps[2]?.[0]?.color).toBe(f32(SIGNAL_COLOR[2] ?? []));
    // 青のときは、青だけが明るい
    const green = vertices(lampGeometry(new Float64Array([4, 5, 1, 0, 0]), WIDE));
    expect(green[6]?.color).toBe(f32(SIGNAL_COLOR[0] ?? []));
    expect(green[6 + 2 * DISC_SIDES * 3]?.color).toBe(f32(SIGNAL_OFF_COLOR));
  });

  test("範囲にかからない信号は描かない", () => {
    const g = lampGeometry(new Float64Array([4, 5, 1, 0, 2]), {
      minX: 20,
      minY: 20,
      maxX: 30,
      maxY: 30,
    });
    expect(g.vertexCount).toBe(0);
  });
});

describe("細部の頂点を作るとき", () => {
  const view = (x: number, y: number): TileRect => ({
    minX: x - 5,
    minY: y - 3,
    maxX: x + 5,
    maxY: y + 3,
  });
  const make = () => {
    const cache = new DetailCache();
    cache.setBuildings(new Float64Array(building(0, 0, 0, 0, 1)));
    cache.setRoads(
      new Float64Array([0, 2, 8, 2, 1]),
      new Float64Array([0, 2, 1, 0, 0, 8, 2, 1, 0, 0]),
    );
    cache.setSignals(new Float64Array());
    return cache;
  };

  test("細部の段より引いているあいだは、何も作らない", () => {
    const cache = make();
    expect(cache.update(8, view(0, 0))).toBeNull();
    expect(cache.update(32, view(0, 0))).toBeNull();
    expect(cache.builds).toBe(0);
    const built = cache.update(33, view(0, 0));
    expect(built?.roofs?.vertexCount).toBeGreaterThan(0);
    expect(built?.streets?.vertexCount).toBeGreaterThan(0);
    expect(cache.builds).toBe(1);
  });

  test("画面が前に作った範囲の中にあるうちは作り直さず、出たらすべて作り直す", () => {
    const cache = make();
    cache.update(128, view(0, 0));
    expect(cache.update(128, view(1, 1))).toBeNull();
    const moved = cache.update(128, view(40, 40));
    expect(moved?.roofs).toBeDefined();
    expect(moved?.streets).toBeDefined();
    expect(moved?.lamps).toBeDefined();
    expect(moved?.roofs?.vertexCount).toBe(0);
  });

  test("寄って画面が前の範囲よりずっと狭くなったら、狭い範囲で作り直す", () => {
    const cache = make();
    cache.update(33, { minX: -20, minY: -11, maxX: 20, maxY: 11 });
    expect(cache.update(40, { minX: -16, minY: -9, maxX: 16, maxY: 9 })).toBeNull();
    const narrow = cache.update(128, view(0, 0));
    expect(narrow?.roofs).toBeDefined();
    expect(cache.update(128, view(0.5, 0.5))).toBeNull();
  });

  test("信号が変わったときは、信号だけを同じ範囲で作り直す", () => {
    const cache = make();
    cache.update(128, view(0, 0));
    cache.setSignals(new Float64Array([0, 2, 1, 0, 0]));
    const lit = cache.update(128, view(0, 0));
    expect(lit?.roofs).toBeUndefined();
    expect(lit?.streets).toBeUndefined();
    expect(lit?.lamps?.vertexCount).toBeGreaterThan(0);
    // 引いているあいだに変わったものは、次に寄ったときに作る
    cache.setBuildings(new Float64Array());
    expect(cache.update(16, view(0, 0))).toBeNull();
    expect(cache.update(128, view(0, 0))?.roofs?.vertexCount).toBe(0);
  });
});

describe("移動体の細部", () => {
  test("車の前の窓は後ろの窓より広く、窓のあいだが屋根になる", () => {
    const width = ([a, b]: readonly [number, number]) => b - a;
    expect(width(CAR_FRONT_WINDOW)).toBeGreaterThan(width(CAR_REAR_WINDOW));
    expect(CAR_REAR_WINDOW[1]).toBeLessThan(CAR_FRONT_WINDOW[0]);
    expect(CAR_FRONT_WINDOW[0]).toBeGreaterThan(0);
    expect(CAR_REAR_WINDOW[0]).toBeGreaterThan(-1);
    expect(CAR_FRONT_WINDOW[1]).toBeLessThan(1);
  });

  test("車の影は #3c4043 の 25%", () => {
    expect(CAR_SHADOW_COLOR).toEqual(hex("#3c4043", 0.25));
  });

  test("人の肩は進む向きに薄く横に広い楕円で、頭はその中に収まる丸", () => {
    expect(WALKER_SHOULDER[0]).toBeLessThan(WALKER_SHOULDER[1]);
    expect(WALKER_SHOULDER[1]).toBeLessThanOrEqual(1);
    expect(WALKER_HEAD).toBeLessThan(WALKER_SHOULDER[0]);
  });
});
