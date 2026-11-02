import { describe, expect, test } from "vitest";
import { HIGHWAY_CODE } from "./lanes.ts";
import { TRAFFIC_LEVELS } from "./layers.ts";
import {
  asphaltHalfWidth,
  DIAGONAL_ASPHALT_HALF,
  lineHalfWidth,
  type MeshSource,
  roadDrawOrder,
  VERTEX_FLOATS,
} from "./roads.ts";
import {
  JAMMED_EXTRA_PX,
  TRAFFIC_BAND_ALPHA,
  TRAFFIC_BAND_WIDTH,
  TRAFFIC_FINE_BAND_ALPHA,
  TRAFFIC_FINE_BAND_WIDTH,
  TRAFFIC_LINE_SHARE,
  TRAFFIC_UNCOUNTED_COLOR,
  TrafficLines,
  TrafficNet,
  trafficColor,
} from "./traffic.ts";

const segmentsOf = (...s: [number, number, number, number, number][]) => new Float64Array(s.flat());
const nodesOf = (...tiles: [number, number][]) =>
  new Float64Array(tiles.flatMap(([x, y]) => [x, y, 1, 0, 0]));

/** 頂点の並びを、頂点ごとの値の組に直す。位置はワールド座標に戻す。 */
function vertices(g: MeshSource) {
  const data = g.build();
  const out = [];
  for (let i = 0; i < data.length; i += VERTEX_FLOATS) {
    out.push({
      x: (data[i] ?? 0) + g.origin.x,
      y: (data[i + 1] ?? 0) + g.origin.y,
      nx: data[i + 2] ?? 0,
      ny: data[i + 3] ?? 0,
      half: data[i + 4] ?? 0,
      color: Array.from(data.subarray(i + 6, i + 10)),
      extra: data[i + 10] ?? 0,
      dash: data[i + 12] ?? 0,
    });
  }
  return out;
}

const f32 = (c: readonly number[]) => Array.from(new Float32Array(c)).join();
const colorOf = (value: number, alpha = 1) => f32(trafficColor(value, alpha) ?? []);

describe("交通の主題図の形", () => {
  // レビューの再現。x = 0 と x = 4 の縦の 2 本が詰まり、そのあいだの横の 1 本（y = 2）が空いている。
  // 縦の 2 本は、横の 1 本と交わる交差点で上下に分かれる
  const segments = segmentsOf(
    [0, 0, 0, 2, 2],
    [0, 2, 0, 4, 2],
    [4, 0, 4, 2, 2],
    [4, 2, 4, 4, 2],
    [0, 2, 4, 2, 2],
  );
  const nodes = nodesOf([0, 0], [0, 2], [0, 4], [4, 0], [4, 2], [4, 4]);
  const net = new TrafficNet(segments, nodes);
  const meshes = net.meshes(new Uint8Array([4, 4, 4, 4, 1]));

  test("交差点にはさまれた空いている区間は、端まで緑の線にする", () => {
    const free = vertices(meshes.free);
    expect(free.length).toBe(6);
    expect(free.every((v) => v.color.join() === colorOf(1))).toBe(true);
    // 交差点のマスの中心から、向こうの交差点のマスの中心まで
    expect(Math.min(...free.map((v) => v.x))).toBeCloseTo(0.5);
    expect(Math.max(...free.map((v) => v.x))).toBeCloseTo(4.5);
    expect(free.every((v) => Math.abs(v.y - 2.5) < 1e-6)).toBe(true);
    // 詰まっている縦の線は、横の区間にかからない
    const busy = vertices(meshes.busy);
    expect(busy.every((v) => v.color.join() === colorOf(4))).toBe(true);
    expect(busy.every((v) => v.x === 0.5 || v.x === 4.5)).toBe(true);
  });

  test("線は道路の太さの 55% で、詰まっている区間は 1px 太い", () => {
    const line = TRAFFIC_LINE_SHARE * lineHalfWidth(2, { x: 0, y: 0 }, { x: 4, y: 0 });
    const busy = vertices(meshes.busy);
    expect(busy.every((v) => Math.abs(v.half - line) < 1e-6)).toBe(true);
    expect(busy.every((v) => v.extra === JAMMED_EXTRA_PX)).toBe(true);
    expect(vertices(meshes.free).every((v) => v.extra === 0)).toBe(true);
  });

  test("交差点（3 本がつながる）には線の色の正方形を置かない", () => {
    // 正方形は法線が (±1, ±1) の頂点でできる
    const squares = [...vertices(meshes.free), ...vertices(meshes.busy)].filter(
      (v) => Math.abs(v.nx) === 1 && Math.abs(v.ny) === 1,
    );
    expect(squares).toEqual([]);
  });

  test("寄りの帯は、車道の両端の縁石の内側に、幅 0.06 マスで不透明度 70% で引く", () => {
    const bands = vertices(meshes.bands).filter(
      (v) => v.color.join() === colorOf(1, TRAFFIC_BAND_ALPHA),
    );
    expect(bands.length).toBe(12);
    expect(bands.every((v) => Math.abs(v.half - TRAFFIC_BAND_WIDTH / 2) < 1e-6)).toBe(true);
    // 帯の中心は縁石から半分の幅だけ内側にあり、帯の外の縁が縁石（車道の端）にそろう
    const center = asphaltHalfWidth(2) - TRAFFIC_BAND_WIDTH / 2;
    const offsets = new Set(bands.map((v) => Math.round((v.y - 2.5) * 1e6) / 1e6));
    expect([...offsets].sort((a, b) => a - b)).toEqual(
      [-center, center].map((v) => Math.round(v * 1e6) / 1e6),
    );
  });

  test("細部の段の帯は、寄りの帯より細く薄く、外の縁を縁石にそろえる", () => {
    expect(meshes.fineBands).toBeNull();
    const fine = net.meshes(new Uint8Array([4, 4, 4, 4, 1]), true).fineBands;
    expect(fine).not.toBeNull();
    const bands = vertices(fine as MeshSource).filter(
      (v) => v.color.join() === colorOf(1, TRAFFIC_FINE_BAND_ALPHA),
    );
    expect(TRAFFIC_FINE_BAND_WIDTH).toBe(0.04);
    expect(TRAFFIC_FINE_BAND_ALPHA).toBe(0.5);
    expect(bands.length).toBe(12);
    expect(bands.every((v) => Math.abs(v.half - TRAFFIC_FINE_BAND_WIDTH / 2) < 1e-6)).toBe(true);
    const center = asphaltHalfWidth(2) - TRAFFIC_FINE_BAND_WIDTH / 2;
    const offsets = new Set(bands.map((v) => Math.round((v.y - 2.5) * 1e6) / 1e6));
    expect([...offsets].sort((a, b) => a - b)).toEqual(
      [-center, center].map((v) => Math.round(v * 1e6) / 1e6),
    );
  });

  test("値のない区間は描かない", () => {
    const empty = net.meshes(new Uint8Array(5));
    expect(empty.free.vertexCount + empty.busy.vertexCount + empty.bands.vertexCount).toBe(0);
  });

  test("高速道路は混み具合によらず、灰の破線にする", () => {
    const highway = new TrafficNet(
      segmentsOf([0, 0, 6, 0, HIGHWAY_CODE]),
      nodesOf([0, 0], [6, 0]),
    ).meshes(new Uint8Array([0]));
    const busy = vertices(highway.busy);
    expect(busy.length).toBe(6);
    expect(busy.every((v) => v.color.join() === f32(TRAFFIC_UNCOUNTED_COLOR))).toBe(true);
    expect(busy.every((v) => v.dash > 0)).toBe(true);
    expect(vertices(highway.bands).every((v) => v.dash > 0)).toBe(true);
    expect(highway.free.vertexCount).toBe(0);
  });

  test("2 本だけがつながる角では、両方が同じ値なら線をつなぐ正方形を置く", () => {
    // (0, 0) から (3, 0)、(3, 0) から (3, 3) へ折れる
    const corner = new TrafficNet(
      segmentsOf([0, 0, 3, 0, 1], [3, 0, 3, 3, 1]),
      nodesOf([0, 0], [3, 0], [3, 3]),
    );
    const isSquare = (v: { nx: number; ny: number }) =>
      Math.abs(v.nx) === 1 && Math.abs(v.ny) === 1;
    const same = vertices(corner.meshes(new Uint8Array([2, 2])).busy).filter(isSquare);
    expect(same.length).toBe(6);
    expect(same.every((v) => v.x === 3.5 && v.y === 0.5)).toBe(true);
    expect(same.every((v) => v.color.join() === colorOf(2))).toBe(true);
    // 値が違えば、どちらの色にもしない
    const mixed = corner.meshes(new Uint8Array([1, 3]));
    expect([...vertices(mixed.free), ...vertices(mixed.busy)].filter(isSquare)).toEqual([]);
  });

  test("斜めの区間は、脇の建物にかからない細い車道に合わせて帯を引く", () => {
    const diagonal = new TrafficNet(segmentsOf([0, 0, 3, 3, 3]), nodesOf([0, 0], [3, 3]));
    const meshes = diagonal.meshes(new Uint8Array([3]));
    const line = vertices(meshes.busy);
    expect(line.length).toBe(6);
    // 線は区間の両端（マスの中心）を結ぶ
    expect(line.some((v) => v.x === 0.5 && v.y === 0.5)).toBe(true);
    expect(line.some((v) => v.x === 3.5 && v.y === 3.5)).toBe(true);
    const lineHalf = TRAFFIC_LINE_SHARE * lineHalfWidth(3, { x: 0, y: 0 }, { x: 3, y: 3 });
    expect(line.every((v) => Math.abs(v.half - lineHalf) < 1e-6)).toBe(true);
    // 帯は、中心線から (細い車道の半分の幅 − 帯の半分の幅) だけ横にずれる
    const offset = DIAGONAL_ASPHALT_HALF - TRAFFIC_BAND_WIDTH / 2;
    const first = vertices(meshes.bands)[0];
    const away = Math.abs((first?.x ?? 0) - (first?.y ?? 0)) / Math.SQRT2;
    expect(away).toBeCloseTo(offset);
  });

  test("4 段の色は決まりの色にする", () => {
    expect([1, 2, 3, 4].map((v) => trafficColor(v))).toEqual(TRAFFIC_LEVELS.map((l) => l.color));
    expect(trafficColor(0)).toBeUndefined();
  });
});

describe("交通の主題図の状態", () => {
  const two = segmentsOf([0, 0, 4, 0, 1], [4, 0, 8, 0, 1]);
  const twoNodes = nodesOf([0, 0], [4, 0], [8, 0]);

  test("同じ網の値が届いたときだけ描き、値が変わらなければ作り直さない", () => {
    const lines = new TrafficLines();
    lines.setRoads(1, two, twoNodes);
    expect(lines.take()).toBeNull();
    lines.setValues(1, new Uint8Array([1, 2]));
    const meshes = lines.take();
    expect(meshes?.free.vertexCount).toBeGreaterThan(0);
    expect(meshes?.busy.vertexCount).toBeGreaterThan(0);
    expect(lines.take()).toBeUndefined();
    lines.setValues(1, new Uint8Array([1, 2]));
    expect(lines.take()).toBeUndefined();
    lines.setValues(1, new Uint8Array([1, 1]));
    expect(lines.take()?.busy.vertexCount).toBe(0);
  });

  test("区間を取り壊すと、新しい網の値が届くまで線を消し、届いたら残った区間だけを描く", () => {
    const lines = new TrafficLines();
    lines.setRoads(1, two, twoNodes);
    lines.setValues(1, new Uint8Array([3, 3]));
    // 2 本の線と、つなぎ目の正方形
    expect(lines.take()?.busy.vertexCount).toBe(18);
    lines.setRoads(2, segmentsOf([0, 0, 4, 0, 1]), nodesOf([0, 0], [4, 0]));
    expect(lines.take()).toBeNull();
    // 古い網の値は、新しい網には当てない
    lines.setValues(1, new Uint8Array([2, 2]));
    expect(lines.take()).toBeNull();
    lines.setValues(2, new Uint8Array([3]));
    expect(lines.take()?.busy.vertexCount).toBe(6);
  });

  test("細部の段に入ったときだけ、値が変わらなくても細部の帯を足して作り直す", () => {
    const lines = new TrafficLines();
    lines.setRoads(1, two, twoNodes);
    lines.setValues(1, new Uint8Array([1, 2]));
    expect(lines.take(false)?.fineBands).toBeNull();
    expect(lines.take(true)?.fineBands?.vertexCount).toBeGreaterThan(0);
    expect(lines.take(true)).toBeUndefined();
    // 引いても作り直さない（細部の帯は描かないだけ）
    expect(lines.take(false)).toBeUndefined();
    lines.setValues(1, new Uint8Array([2, 2]));
    expect(lines.take(false)?.fineBands).toBeNull();
    expect(lines.take(true)?.fineBands?.vertexCount).toBeGreaterThan(0);
  });

  test("主題図を交通から外すと、線を消す", () => {
    const lines = new TrafficLines();
    lines.setRoads(1, two, twoNodes);
    lines.setValues(1, new Uint8Array([1, 1]));
    expect(lines.take()).not.toBeNull();
    lines.clear();
    expect(lines.take()).toBeNull();
  });

  test("大きな網でも、区間ごとに線 1 本と帯 2 本を作る", () => {
    // 1 辺 61 本の格子（縦横 7,320 区間）
    const side = 61;
    const segs: [number, number, number, number, number][] = [];
    const nodeTiles: [number, number][] = [];
    for (let i = 0; i <= side; i++) {
      for (let j = 0; j <= side; j++) {
        nodeTiles.push([i * 4, j * 4]);
        if (i < side) {
          segs.push([i * 4, j * 4, i * 4 + 4, j * 4, 2]);
        }
        if (j < side) {
          segs.push([i * 4, j * 4, i * 4, j * 4 + 4, 2]);
        }
      }
    }
    const net = new TrafficNet(segmentsOf(...segs), nodesOf(...nodeTiles));
    expect(net.segmentCount).toBe(2 * side * (side + 1));
    const values = new Uint8Array(net.segmentCount).map((_, i) => (i % 4) + 1);
    const meshes = net.meshes(values);
    // 1 区間に線 1 本と帯 2 本。空いている区間（4 本に 1 本）は空いているの線に入る
    expect(meshes.free.vertexCount + meshes.busy.vertexCount).toBe(net.segmentCount * 6);
    expect(meshes.free.vertexCount).toBe((net.segmentCount / 4) * 6);
    expect(meshes.bands.vertexCount).toBe(net.segmentCount * 12);
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

  test("細部の段では、寄りの帯を細く薄い帯に 32〜48 で入れ替える", () => {
    expect(alphaOf(32, "trafficFineBands")).toBe(0);
    expect(alphaOf(32, "trafficBands")).toBe(1);
    expect(alphaOf(40, "trafficFineBands")).toBeCloseTo(0.5);
    expect(alphaOf(40, "trafficBands")).toBeCloseTo(0.5);
    expect(alphaOf(64, "trafficFineBands")).toBe(1);
    expect(alphaOf(64, "trafficBands")).toBe(0);
  });

  test("交通の主題図は道路と信号の上、仮の表示の下に描く", () => {
    const order = roadDrawOrder(12).map(([name]) => name);
    expect(order.indexOf("signals")).toBeLessThan(order.indexOf("trafficFree"));
    expect(order.indexOf("trafficBands")).toBeLessThan(order.indexOf("preview"));
  });
});
