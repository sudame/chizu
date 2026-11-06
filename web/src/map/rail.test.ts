import { describe, expect, test } from "vitest";
import { hex } from "./palette.ts";
import {
  closedCrossingMeshes,
  crossingMeshes,
  parseCrossings,
  parseStations,
  parseTracks,
  RAIL_BED_COLOR,
  RAIL_FAR_COLOR,
  RAIL_MID_COLOR,
  railMeshes,
  stationFrame,
} from "./rail.ts";
import { roadDrawOrder } from "./roads.ts";

describe("線路と駅の数の並び", () => {
  test("線路は両端のマスと駅の線路かを読む", () => {
    expect(parseTracks([3, 8, 12, 8, 1, 12, 8, 20, 8, 0])).toEqual([
      { a: { x: 3, y: 8 }, b: { x: 12, y: 8 }, station: true },
      { a: { x: 12, y: 8 }, b: { x: 20, y: 8 }, station: false },
    ]);
    expect(parseTracks([])).toEqual([]);
  });

  test("駅は敷地と道路へ向かう向きを読む", () => {
    expect(parseStations([3, 7, 12, 9, 0, 1])).toEqual([
      { min: { x: 3, y: 7 }, max: { x: 12, y: 9 }, toward: { x: 0, y: 1 } },
    ]);
  });
});

describe("色の表", () => {
  test("引きは灰色、標準は濃い灰色、寄りの道床は砂色", () => {
    expect(RAIL_FAR_COLOR).toEqual(hex("#80868b"));
    expect(RAIL_MID_COLOR).toEqual(hex("#5f6368"));
    expect(RAIL_BED_COLOR).toEqual(hex("#d8d3c7"));
  });
});

describe("駅の敷地の座標", () => {
  const south = { min: { x: 3, y: 7 }, max: { x: 12, y: 9 }, toward: { x: 0, y: 1 } };
  const north = { min: { x: 3, y: 7 }, max: { x: 12, y: 9 }, toward: { x: 0, y: -1 } };
  const east = { min: { x: 3, y: 7 }, max: { x: 5, y: 16 }, toward: { x: 1, y: 0 } };

  test("間口の長さは道路に沿う辺の長さである", () => {
    expect(stationFrame(south).frontage).toBe(10);
    expect(stationFrame(east).frontage).toBe(10);
  });

  test("道路の側へ進むと、敷地の道路に面した辺に着く", () => {
    // 敷地は y 7〜9 なので、南の道路に面した辺は y = 10、真ん中は y = 8.5
    expect(stationFrame(south).point(0, 1.5).y).toBe(10);
    expect(stationFrame(south).point(0, 0).y).toBe(8.5);
    expect(stationFrame(north).point(0, 1.5).y).toBe(7);
    expect(stationFrame(east).point(0, 1.5).x).toBe(6);
  });
});

describe("線路と駅の頂点", () => {
  const tracks = parseTracks([3, 8, 12, 8, 1, 12, 8, 20, 8, 0]);
  const stations = parseStations([3, 7, 12, 9, 0, 1]);

  test("線路も駅もなければ、どれも空である", () => {
    const empty = railMeshes([], []);
    for (const mesh of Object.values(empty)) {
      expect(mesh.vertexCount).toBe(0);
    }
  });

  test("線路があれば、段ごとの頂点が作られ、駅がなければ駅の頂点はない", () => {
    const m = railMeshes(tracks, []);
    for (const name of ["far", "mid", "bed", "rails", "sleepers"] as const) {
      expect(m[name].vertexCount).toBeGreaterThan(0);
    }
    expect(m.station.vertexCount).toBe(0);
    expect(m.stationFine.vertexCount).toBe(0);
    // レールは線路 1 本につき、上りと下りの 2 本ずつ
    expect(m.rails.vertexCount).toBe(tracks.length * 4 * 6);
  });

  test("駅はホームの面 2 つと駅舎、白線 2 本を作る", () => {
    const m = railMeshes(tracks, stations);
    expect(m.station.vertexCount).toBeGreaterThan(0);
    expect(m.stationFine.vertexCount).toBe(2 * 6);
  });
});

describe("描く順", () => {
  const alphaOf = (zoom: number, name: string) =>
    roadDrawOrder(zoom).find(([n]) => n === name)?.[1] ?? Number.NaN;

  test("引きは線だけ、標準は白の破線のある線、寄りは道床とレール、細部は枕木を描く", () => {
    expect([alphaOf(1.5, "railFar"), alphaOf(1.5, "railMid"), alphaOf(1.5, "railBed")]).toEqual([
      1, 0, 0,
    ]);
    expect([alphaOf(8, "railFar"), alphaOf(8, "railMid"), alphaOf(8, "railBed")]).toEqual([
      0, 1, 0,
    ]);
    expect([alphaOf(20, "railMid"), alphaOf(20, "railBed"), alphaOf(20, "railSleepers")]).toEqual([
      0, 1, 0,
    ]);
    expect(alphaOf(64, "railSleepers")).toBe(1);
    expect(alphaOf(64, "railStationFine")).toBe(1);
    expect(alphaOf(20, "railStation")).toBe(1);
    expect(alphaOf(8, "railStation")).toBe(0);
  });

  test("段の境目では、前の段と次の段の両方を薄く描く", () => {
    const far = alphaOf(4, "railFar");
    const mid = alphaOf(4, "railMid");
    expect(far).toBeGreaterThan(0);
    expect(far).toBeLessThan(1);
    expect(mid).toBeGreaterThan(0);
    expect(mid).toBeLessThan(1);
  });
});

describe("曲がり角のつなぎ目", () => {
  // (0,0) から東へ (5,0)、そこから南東へ (8,3) の 45 度の曲がり
  const bend = parseTracks([0, 0, 5, 0, 0, 5, 0, 8, 3, 0]);

  // 道床の面の頂点を (x, y) の組で取り出す。頂点は 13 個の数で、先頭の 2 つが座標である
  const points = (mesh: { build(): Float32Array }) => {
    const data = mesh.build();
    const out: [number, number][] = [];
    for (let i = 0; i + 13 <= data.length; i += 13) {
      out.push([data[i] ?? 0, data[i + 1] ?? 0]);
    }
    return out;
  };

  test("道床は曲がり角で、前後の帯が同じ二等分線の上で終わる", () => {
    const m = railMeshes(bend, []);
    const pts = points(m.bed);
    // 帯 2 本で 2 三角形ずつ。曲がり角には丸も四角も足さない
    expect(pts.length).toBe(2 * 6);
    // (5.5, 0.5) の角: 外側（北）は 0.5 * tan(22.5 度) だけ伸び、内側（南）は同じだけ縮む
    const t = Math.tan(Math.PI / 8) * 0.5;
    const has = (x: number, y: number) =>
      pts.some(([px, py]) => Math.abs(px - x) < 1e-5 && Math.abs(py - y) < 1e-5);
    expect(has(5.5 + t, 0)).toBe(true);
    expect(has(5.5 - t, 1)).toBe(true);
  });

  test("レールは曲がり角で、前後の線の端が同じ点で会う", () => {
    const m = railMeshes(bend, []);
    const pts = points(m.rails);
    // 前の線の終わりの点（x が 5 付近）に、次の線の始まりの点が重なる
    const near = (p: [number, number], q: [number, number]) =>
      Math.abs(p[0] - q[0]) < 1e-5 && Math.abs(p[1] - q[1]) < 1e-5;
    const first = pts.slice(0, 6 * 4);
    const second = pts.slice(6 * 4);
    const ends = first.filter(([x]) => x > 4);
    const starts = second.filter(([x]) => x < 6);
    expect(ends.length).toBeGreaterThan(0);
    for (const e of ends) {
      expect(starts.some((s) => near(s, e))).toBe(true);
    }
  });

  test("枕木は、曲がり角の二等分線の上の 1 本から前後へ並び、両端の隙間は枕木 1 本ぶんほどに収まる", () => {
    const m = railMeshes(bend, []);
    const data = m.sleepers.build();
    // 破線のある帯（頂点 6 つ）ごとに、a の端と b の端の破線の位置と周期を読む
    const strips: { d0: number; d1: number; period: number }[] = [];
    for (let i = 0; i + 6 * 13 <= data.length; i += 6 * 13) {
      const period = Math.abs(data[i + 12] ?? 0);
      if (period > 0) {
        strips.push({ d0: data[i + 11] ?? 0, d1: data[i + 2 * 13 + 11] ?? 0, period });
      }
    }
    // 2 本の線路に、上りと下りの 2 レーンずつ
    expect(strips.length).toBe(4);
    for (const { d0, d1, period } of strips) {
      // 破線は、位置 d の mod 周期が周期の半分より小さいときに描く
      const firstOn = Math.ceil(d0 / period) * period;
      const lastOnEnd = Math.floor(d1 / period) * period + period / 2;
      const startGap = firstOn - d0;
      const endGap = d1 - Math.min(lastOnEnd, d1);
      expect(startGap).toBeLessThanOrEqual(period / 2 + 1e-6);
      expect(endGap).toBeLessThanOrEqual(period / 2 + 1e-6);
      // 角の隙間は、二等分線の上の枕木の厚みの半分と合わせても、枕木 1 本ぶん（周期）に収まる
      expect(startGap + period / 4).toBeLessThan(period);
    }
    // 二等分線の上の枕木が、レーンごとに 1 本ある（頂点 6 つで、破線のない帯）
    const solid = [];
    for (let i = 0; i + 6 * 13 <= data.length; i += 6 * 13) {
      if ((data[i + 12] ?? 0) === 0) {
        solid.push(i);
      }
    }
    expect(solid.length).toBe(2);
  });

  test("まっすぐにつながる 2 本には、角も丸も足さない", () => {
    const straight = parseTracks([0, 0, 5, 0, 0, 5, 0, 9, 0, 0]);
    expect(points(railMeshes(straight, []).bed).length).toBe(2 * 6);
  });
});

describe("踏切", () => {
  // (30, 8) を縦の小さな道路が横切り、線路は横に通る
  const crossings = parseCrossings([30, 8, 0, 1, 1, 0, 1]);
  const info = new Map(crossings.map((c) => [`${c.at.x},${c.at.y}`, c]));

  test("踏切はマスと、道路と線路の向きと、道路の種類を読む", () => {
    expect(crossings).toEqual([
      { at: { x: 30, y: 8 }, road: { x: 0, y: 1 }, rail: { x: 1, y: 0 }, kind: 1 },
    ]);
    expect(parseCrossings([])).toEqual([]);
  });

  test("踏切がなければ何も描かない", () => {
    expect(crossingMeshes([]).base.vertexCount).toBe(0);
    expect(closedCrossingMeshes(new Map(), []).arms.vertexCount).toBe(0);
  });

  test("踏切は踏板、レール、縞、台を描き、踏切の数に比例して頂点が増える", () => {
    const one = crossingMeshes(crossings).base.vertexCount;
    expect(one).toBeGreaterThan(0);
    const two = crossingMeshes(parseCrossings([30, 8, 0, 1, 1, 0, 1, 40, 8, 0, 1, 1, 0, 1]));
    expect(two.base.vertexCount).toBe(2 * one);
  });

  test("閉じている踏切だけに、棒と点いた警報灯が出る", () => {
    const open = closedCrossingMeshes(info, []);
    expect([open.arms, open.lampsA, open.lampsB].map((g) => g.vertexCount)).toEqual([0, 0, 0]);
    const closed = closedCrossingMeshes(info, [30, 8]);
    expect(closed.arms.vertexCount).toBeGreaterThan(0);
    // 警報灯は台ごとに 2 つで、交互に点く 2 組は同じ数になる
    expect(closed.lampsA.vertexCount).toBeGreaterThan(0);
    expect(closed.lampsA.vertexCount).toBe(closed.lampsB.vertexCount);
    // 形を知らない踏切は飛ばす
    expect(closedCrossingMeshes(info, [99, 99]).arms.vertexCount).toBe(0);
  });

  test("棒は入ってくる車線の側に下りる（左側通行）", () => {
    const g = closedCrossingMeshes(info, [30, 8]).arms;
    const xs = Array.from(g.build())
      .filter((_, i) => i % 13 === 0)
      .map((x) => x + g.origin.x);
    // 縦の道路（北から南へ進む車は西側を通る）なので、棒は中心 30.5 の両側にある
    expect(Math.min(...xs)).toBeLessThan(30.5);
    expect(Math.max(...xs)).toBeGreaterThan(30.5);
  });

  test("踏切は寄りから描き、引きと標準では描かない。警報灯は交互に点く", () => {
    const alphaOf = (zoom: number, name: string, blink: 0 | 1 = 0) =>
      roadDrawOrder(zoom, blink).find(([n]) => n === name)?.[1] ?? Number.NaN;
    expect([alphaOf(2, "crossings"), alphaOf(8, "crossings")]).toEqual([0, 0]);
    expect(alphaOf(20, "crossings")).toBe(1);
    expect(alphaOf(64, "crossingArms")).toBe(1);
    expect([alphaOf(20, "crossingLampsA", 0), alphaOf(20, "crossingLampsB", 0)]).toEqual([1, 0]);
    expect([alphaOf(20, "crossingLampsA", 1), alphaOf(20, "crossingLampsB", 1)]).toEqual([0, 1]);
  });
});
