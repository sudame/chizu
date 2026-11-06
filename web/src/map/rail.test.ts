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
