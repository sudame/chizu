import { describe, expect, test } from "vitest";
import {
  buildingColor,
  buildingGeometry,
  FACILITY_COLOR,
  Geometry,
  markCenter,
  miterTrim,
  roadGeometry,
  roadLaneGeometry,
  roadMeshes,
  SIGNAL_COLOR,
  signalGeometry,
  signalLampCenter,
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
    const g = roadGeometry(
      new Float64Array([0, 0, 4, 0, 1]),
      new Float64Array([0, 0, 1, 0, 4, 0, 1, 0]),
    );
    expect(g.vertexCount).toBe(18);
    expect(roadGeometry(new Float64Array(), new Float64Array()).vertexCount).toBe(0);
  });
});

describe("車線ごとの道路", () => {
  const nodesOf = (...tiles: [number, number][]) =>
    new Float64Array(tiles.flatMap(([x, y]) => [x, y, 1, 0]));
  // 6 頂点ごとの塊の、i 番めの頂点の x と y
  const at = (g: Geometry, block: number, vertex: number) => {
    const data = g.build();
    const i = (block * 6 + vertex) * VERTEX_FLOATS;
    return { x: data[i] ?? 0, y: data[i + 1] ?? 0, half: data[i + 4] ?? 0 };
  };

  test("中くらいの道路 1 本は、両端の埋め、車道、歩道 2、両端の四角、中央線と区切り 2 になる", () => {
    const g = roadLaneGeometry(new Float64Array([0, 0, 4, 0, 2]), nodesOf([0, 0], [4, 0]));
    expect(g.vertexCount).toBe(6 * (2 + 1 + 2 + 2 + 3));
  });

  test("帯は中心線の左右へ車線の幅ごとにずれて並ぶ", () => {
    const g = roadLaneGeometry(new Float64Array([0, 0, 4, 0, 2]), nodesOf([0, 0], [4, 0]));
    const ys = new Set<string>();
    // 2 つの埋めの四角のあとに、車道、歩道 2、四角 2、中央線、区切り 2 が続く
    for (const block of [2, 3, 4, 7, 8, 9]) {
      ys.add(at(g, block, 0).y.toFixed(2));
    }
    // 基準のマスは (0, 0) で、中心線は y = 0.5。中くらいの道路の車線は 0.16 マス幅で、
    // 歩道は車道の端（0.32）の外 0.04 にある
    expect([...ys].sort()).toEqual(["0.14", "0.34", "0.50", "0.66", "0.86"]);
  });

  test("細い線の半分の太さは負で渡し、車道の帯は正で渡す", () => {
    const g = roadLaneGeometry(new Float64Array([0, 0, 4, 0, 3]), nodesOf([0, 0], [4, 0]));
    // 埋め 2 つのあとの最初の帯が車道で、半分の太さは車線 3 本分（0.14 × 3）
    expect(at(g, 2, 0).half).toBeCloseTo(0.42);
    // 歩道と線は細い線の扱い
    expect(at(g, 3, 0).half).toBeLessThan(0);
    expect(at(g, g.vertexCount / 6 - 1, 0).half).toBeLessThan(0);
  });

  test("3 本以上が集まる交差点では、線が交差点の中を横切らないよう縮める", () => {
    // (4, 0) に 3 本が集まる
    const segments = new Float64Array([0, 0, 4, 0, 1, 4, 0, 8, 0, 1, 4, 0, 4, 4, 1]);
    const g = roadLaneGeometry(segments, nodesOf([0, 0], [4, 0], [8, 0], [4, 4]));
    // 行き止まりの埋め 3、車道 3、歩道 6、ノードの四角 4、中央線 3
    expect(g.vertexCount).toBe(6 * (3 + 3 + 6 + 4 + 3));
    // 区間 (0,0)-(4,0) の中央線は、終点側が交差点の半分の幅（0.3）だけ手前で終わる
    expect(at(g, 16, 2).x).toBeCloseTo(4.5 - 0.3);
  });

  test("直角の曲がり角では、歩道が折れ曲がりに沿ってつながり、外側の角が埋まる", () => {
    // 東へ行って、(4, 0) で南へ折れる
    const segments = new Float64Array([0, 0, 4, 0, 1, 4, 0, 4, 4, 1]);
    const g = roadLaneGeometry(segments, nodesOf([0, 0], [4, 0], [4, 4]));
    // 埋め 3、車道 2 のあとに、区間 0 の歩道（北、南）、区間 1 の歩道（東、西）が続く
    const [northEnd, southEnd] = [at(g, 5, 2), at(g, 6, 2)];
    // 折れる側（南、角の内側）は 0.26 縮み、反対側（北、外側）は 0.26 伸びて角の外でつながる
    expect(southEnd.x).toBeCloseTo(4.5 - 0.26);
    expect(northEnd.x).toBeCloseTo(4.5 + 0.26);
    // 区間 1 の歩道の始まりは、外側（東）が角の位置 y = 0.5 - 0.26、内側（西）が 0.5 + 0.26
    const [eastStart, westStart] = [at(g, 7, 0), at(g, 8, 0)];
    expect(eastStart.y).toBeCloseTo(0.5 - 0.26);
    expect(westStart.y).toBeCloseTo(0.5 + 0.26);
    // 角のノードには、車道に覆われない外側の角を埋める四角がある（半分の幅 0.3）
    expect(at(g, 1, 0).x).toBeCloseTo(4.5);
    expect(at(g, 1, 0).half).toBeCloseTo(0.3);
  });

  test("縮小用と拡大用の両方の頂点を作る", () => {
    const meshes = roadMeshes(new Float64Array([0, 0, 4, 0, 1]), nodesOf([0, 0], [4, 0]));
    expect(meshes.simple.vertexCount).toBe(18);
    expect(meshes.lanes.vertexCount).toBeGreaterThan(18);
  });
});

describe("miterTrim", () => {
  const east = { x: 1, y: 0 };
  const south = { x: 0, y: 1 };
  const north = { x: 0, y: -1 };

  test("まっすぐなら縮めない", () => {
    expect(miterTrim(east, east, 0.26)).toBeCloseTo(0);
  });

  test("曲がる側の内側は縮み、外側は伸びる", () => {
    // 東から南へ曲がるのは、画面で右へ曲がる。右（南）の側が内側である
    expect(miterTrim(east, south, 0.26)).toBeCloseTo(0.26);
    expect(miterTrim(east, south, -0.26)).toBeCloseTo(-0.26);
    expect(miterTrim(east, north, 0.26)).toBeCloseTo(-0.26);
  });

  test("折り返しでは縮めも伸ばしもしない", () => {
    expect(miterTrim(east, { x: -1, y: 0 }, 0.26)).toBe(0);
  });

  test("45 度の折れ曲がりは tan(22.5 度) 倍", () => {
    const d = { x: Math.SQRT1_2, y: Math.SQRT1_2 };
    expect(miterTrim(east, d, 0.2)).toBeCloseTo(0.2 * Math.tan(Math.PI / 8));
  });
});

describe("信号", () => {
  test("灯火は腕の側の、入ってくる車線の側に置く", () => {
    // 東の腕から入る車は西へ進むので、左側通行では南（y の大きい側）の車線を走る
    const east = signalLampCenter(10, 10, 1, 0);
    expect(east.x).toBeGreaterThan(10.5);
    expect(east.y).toBeGreaterThan(10.5);
    // 北の腕から入る車は南へ進み、東（x の大きい側）の車線を走る
    const north = signalLampCenter(10, 10, 0, -1);
    expect(north.y).toBeLessThan(10.5);
    expect(north.x).toBeGreaterThan(10.5);
  });

  test("腕ごとに台と灯火の 2 つの四角を、灯火の色で描く", () => {
    const lights = new Float64Array([4, 5, 1, 0, 0, 4, 5, 0, 1, 2]);
    const g = signalGeometry(lights);
    expect(g.vertexCount).toBe(2 * 2 * 6);
    const data = g.build();
    const colorAt = (vertex: number) =>
      Array.from(data.subarray(vertex * VERTEX_FLOATS + 5, vertex * VERTEX_FLOATS + 9));
    // 1 本めの灯火（7 番めの頂点から）は青、2 本めは赤
    expect(colorAt(6)).toEqual(Array.from(new Float32Array(SIGNAL_COLOR[0] ?? [])));
    expect(colorAt(18)).toEqual(Array.from(new Float32Array(SIGNAL_COLOR[2] ?? [])));
  });
});
