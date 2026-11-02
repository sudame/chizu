import { describe, expect, test } from "vitest";
import { CHUNK_SIDE } from "../protocol/messages.ts";
import { HIGHWAY_CODE } from "./lanes.ts";
import { TRAFFIC_LEVELS } from "./layers.ts";
import {
  asphaltHalfWidth,
  type Geometry,
  lineHalfWidth,
  roadDrawOrder,
  VERTEX_FLOATS,
} from "./roads.ts";
import {
  JAMMED_EXTRA_PX,
  runSpan,
  runsOf,
  TRAFFIC_BAND_ALPHA,
  TRAFFIC_BAND_WIDTH,
  TRAFFIC_LINE_SHARE,
  TRAFFIC_UNCOUNTED_COLOR,
  TrafficValues,
  trafficColor,
  trafficMeshes,
} from "./traffic.ts";

const segmentsOf = (...s: [number, number, number, number, number][]) => new Float64Array(s.flat());
const nodesOf = (...tiles: [number, number][]) =>
  new Float64Array(tiles.flatMap(([x, y]) => [x, y, 1, 0, 0]));

/** 頂点の並びを、頂点ごとの値の組に直す。 */
function vertices(g: Geometry) {
  const data = g.build();
  const out = [];
  for (let i = 0; i < data.length; i += VERTEX_FLOATS) {
    out.push({
      x: (data[i] ?? 0) + g.origin.x,
      y: (data[i + 1] ?? 0) + g.origin.y,
      half: data[i + 4] ?? 0,
      color: Array.from(data.subarray(i + 6, i + 10)),
      extra: data[i + 10] ?? 0,
      dash: data[i + 12] ?? 0,
    });
  }
  return out;
}

const f32 = (c: readonly number[]) => Array.from(new Float32Array(c));
const colorOf = (value: number, alpha = 1) => f32(trafficColor(value, alpha) ?? []);

describe("交通の値", () => {
  test("チャンクごとの値を読み、届いていないマスは 0 にする", () => {
    const values = new TrafficValues();
    const chunk = new Uint8Array(CHUNK_SIDE * CHUNK_SIDE);
    chunk[3 * CHUNK_SIDE + 2] = 4;
    values.set(-1, 0, chunk);
    expect(values.changed).toBe(true);
    expect(values.at(-CHUNK_SIDE + 2, 3)).toBe(4);
    expect(values.at(-CHUNK_SIDE + 2, 4)).toBe(0);
    expect(values.at(2, 3)).toBe(0);
    values.changed = false;
    values.remove(-1, 0);
    expect(values.changed).toBe(true);
    expect(values.at(-CHUNK_SIDE + 2, 3)).toBe(0);
  });

  test("同じ値が続くマスをまとめ、値のないマスは飛ばす", () => {
    expect(runsOf([1, 1, 0, 2, 2, 2, 4, 1])).toEqual([
      { from: 0, to: 1, value: 1 },
      { from: 3, to: 5, value: 2 },
      { from: 6, to: 6, value: 4 },
      { from: 7, to: 7, value: 1 },
    ]);
    // 0 をはさんだ同じ値は別のまとまり
    expect(runsOf([2, 0, 2])).toHaveLength(2);
  });

  test("まとまりの範囲は、マスの境目の真ん中で分け、区間の両端は縮めた端まで伸ばす", () => {
    expect(runSpan({ from: 0, to: 1, value: 1 }, 4, 1, 0.3, 0)).toEqual({ start: 0.3, end: 1.5 });
    expect(runSpan({ from: 2, to: 4, value: 2 }, 4, 1, 0, 0.3)).toEqual({ start: 1.5, end: 3.7 });
  });
});

describe("交通の主題図の形", () => {
  // (0, 0) から (4, 0) の中の道路。0〜1 のマスは空いている、2〜4 のマスは詰まっている
  const segments = segmentsOf([0, 0, 4, 0, 2]);
  const nodes = nodesOf([0, 0], [4, 0]);
  const valueAt = (x: number, y: number) => (y !== 0 ? 0 : x <= 1 ? 1 : 4);
  const meshes = trafficMeshes(segments, nodes, valueAt);

  test("空いている区間は空いているの線に、ほかは混んでいる線に分ける", () => {
    const free = vertices(meshes.free);
    const busy = vertices(meshes.busy);
    expect(free.every((v) => v.color.join() === colorOf(1).join())).toBe(true);
    expect(busy.every((v) => v.color.join() === colorOf(4).join())).toBe(true);
    // 空いている線は 0 のマスの中心から 1 と 2 の境目まで、詰まっている線はそこから 4 のマスの中心まで
    const xs = (vs: { x: number; half: number }[], half: number) =>
      vs.filter((v) => Math.abs(v.half - half) < 1e-6).map((v) => v.x);
    const line = TRAFFIC_LINE_SHARE * lineHalfWidth(2, { x: 0, y: 0 }, { x: 4, y: 0 });
    expect(Math.min(...xs(free, line))).toBeCloseTo(0.5);
    expect(Math.max(...xs(free, line))).toBeCloseTo(2);
    expect(Math.min(...xs(busy, line))).toBeCloseTo(2);
    expect(Math.max(...xs(busy, line))).toBeCloseTo(4.5);
  });

  test("線は道路の太さの 55% で、詰まっている区間は 1px 太い", () => {
    const busy = vertices(meshes.busy);
    const line = TRAFFIC_LINE_SHARE * lineHalfWidth(2, { x: 0, y: 0 }, { x: 4, y: 0 });
    expect(busy.some((v) => Math.abs(v.half - line) < 1e-6)).toBe(true);
    expect(busy.every((v) => v.extra === JAMMED_EXTRA_PX)).toBe(true);
    expect(vertices(meshes.free).every((v) => v.extra === 0)).toBe(true);
  });

  test("寄りの帯は、車道の両端の縁石の内側に、幅 0.06 マスで不透明度 70% で引く", () => {
    const bands = vertices(meshes.bands);
    expect(bands.length).toBeGreaterThan(0);
    expect(bands.every((v) => Math.abs(v.half - TRAFFIC_BAND_WIDTH / 2) < 1e-6)).toBe(true);
    expect(bands.every((v) => Math.abs((v.color[3] ?? 0) - TRAFFIC_BAND_ALPHA) < 1e-6)).toBe(true);
    const edge = asphaltHalfWidth(2);
    // 帯の中心は縁石から半分の幅だけ内側にあり、帯の外の縁が縁石（車道の端）にそろう
    const center = edge - TRAFFIC_BAND_WIDTH / 2;
    const offsets = new Set(bands.map((v) => Math.round((v.y - 0.5) * 1e6) / 1e6));
    expect([...offsets].sort((a, b) => a - b)).toEqual(
      [-center, center].map((v) => Math.round(v * 1e6) / 1e6),
    );
    expect(bands.some((v) => v.color.join() === colorOf(1, TRAFFIC_BAND_ALPHA).join())).toBe(true);
    expect(bands.some((v) => v.color.join() === colorOf(4, TRAFFIC_BAND_ALPHA).join())).toBe(true);
  });

  test("値が届いていない道路は描かない", () => {
    const empty = trafficMeshes(segments, nodes, () => 0);
    expect(empty.free.vertexCount + empty.busy.vertexCount + empty.bands.vertexCount).toBe(0);
  });

  test("高速道路は混み具合によらず、灰の破線にする", () => {
    const highway = trafficMeshes(
      segmentsOf([0, 0, 6, 0, HIGHWAY_CODE]),
      nodesOf([0, 0], [6, 0]),
      () => 3,
    );
    const busy = vertices(highway.busy);
    expect(busy.length).toBeGreaterThan(0);
    expect(busy.every((v) => v.color.join() === f32(TRAFFIC_UNCOUNTED_COLOR).join())).toBe(true);
    expect(busy.every((v) => v.dash > 0)).toBe(true);
    expect(vertices(highway.bands).every((v) => v.dash > 0)).toBe(true);
    expect(highway.free.vertexCount).toBe(0);
  });

  test("4 段の色は決まりの色にする", () => {
    expect([1, 2, 3, 4].map((v) => trafficColor(v))).toEqual(TRAFFIC_LEVELS.map((l) => l.color));
    expect(trafficColor(0)).toBeUndefined();
  });
});

describe("交通の主題図の段", () => {
  const alphaOf = (zoom: number, name: string) =>
    roadDrawOrder(zoom).find(([n]) => n === name)?.[1] ?? -1;

  test("引きでは、やや混んでいる以上の線だけを描く", () => {
    expect(alphaOf(3, "trafficBusy")).toBe(1);
    expect(alphaOf(3, "trafficFree")).toBe(0);
    expect(alphaOf(3, "trafficBands")).toBe(0);
  });

  test("標準では、空いているの緑も含めて線で描く", () => {
    expect(alphaOf(8, "trafficBusy")).toBe(1);
    expect(alphaOf(8, "trafficFree")).toBe(1);
    expect(alphaOf(8, "trafficBands")).toBe(0);
    // 緑は 4〜6 で現れる
    expect(alphaOf(5, "trafficFree")).toBeCloseTo(0.5);
  });

  test("寄りでは、線をやめて車道の両端の帯にする。10〜14 で入れ替わる", () => {
    expect(alphaOf(20, "trafficBusy")).toBe(0);
    expect(alphaOf(20, "trafficFree")).toBe(0);
    expect(alphaOf(20, "trafficBands")).toBe(1);
    expect(alphaOf(12, "trafficBusy")).toBeCloseTo(0.5);
    expect(alphaOf(12, "trafficBands")).toBeCloseTo(0.5);
  });

  test("交通の主題図は道路と信号の上、仮の表示の下に描く", () => {
    const order = roadDrawOrder(12).map(([name]) => name);
    expect(order.indexOf("signals")).toBeLessThan(order.indexOf("trafficFree"));
    expect(order.indexOf("trafficBands")).toBeLessThan(order.indexOf("preview"));
  });
});
