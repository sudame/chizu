import { describe, expect, test } from "vitest";
import { hex } from "./palette.ts";
import {
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

  test("まっすぐにつながる 2 本には、角も丸も足さない", () => {
    const straight = parseTracks([0, 0, 5, 0, 0, 5, 0, 9, 0, 0]);
    expect(points(railMeshes(straight, []).bed).length).toBe(2 * 6);
  });
});
