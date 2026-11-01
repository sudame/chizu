import { describe, expect, test } from "vitest";
import { Camera } from "./camera.ts";
import {
  buildRoadCommand,
  bulldozeCommand,
  buyChunkCommand,
  chooseRotation,
  clampArea,
  MAX_ROAD_TILES,
  MAX_ZONE_SIDE,
  paintZoneCommand,
  parseRoadCostTable,
  parseServiceTable,
  placeServiceCommand,
  roadCost,
  screenToTile,
  serviceFootprint,
  snapToEightDirections,
} from "./tools.ts";

const viewport = { width: 800, height: 600 };

describe("screenToTile", () => {
  test("画面の点の下のマスを、負の座標でも床で求める", () => {
    const camera = new Camera(10, -4, 8);
    expect(screenToTile(camera, 400, 300, viewport)).toEqual({ x: 10, y: -4 });
    expect(screenToTile(camera, 399, 299, viewport)).toEqual({ x: 9, y: -5 });
    expect(screenToTile(camera, 400 + 8 * 3.5, 300 - 8 * 0.5, viewport)).toEqual({ x: 13, y: -5 });
  });
});

describe("snapToEightDirections", () => {
  const o = { x: 5, y: 5 };
  const snap = (dx: number, dy: number) => {
    const t = snapToEightDirections(o, { x: o.x + dx, y: o.y + dy });
    return [t.x - o.x, t.y - o.y];
  };

  test("縦横に近ければ縦横に、45 度に近ければ斜めに寄せる", () => {
    expect(snap(10, 2)).toEqual([10, 0]);
    expect(snap(-10, -4)).toEqual([-10, 0]);
    expect(snap(1, -9)).toEqual([0, -9]);
    expect(snap(7, 5)).toEqual([6, 6]);
    expect(snap(-6, 8)).toEqual([-7, 7]);
    expect(snap(4, -4)).toEqual([4, -4]);
    expect(snap(0, 0)).toEqual([0, 0]);
  });

  test("寄せた終点はいつも 8 方向のどれかにある", () => {
    for (let dx = -12; dx <= 12; dx++) {
      for (let dy = -12; dy <= 12; dy++) {
        const [x = 0, y = 0] = snap(dx, dy);
        expect(x === 0 || y === 0 || Math.abs(x) === Math.abs(y)).toBe(true);
      }
    }
  });

  test("長さは上限で切る", () => {
    expect(snap(5000, 0)).toEqual([MAX_ROAD_TILES, 0]);
    expect(snap(-3000, 3000)).toEqual([-MAX_ROAD_TILES, MAX_ROAD_TILES]);
  });
});

describe("roadCost", () => {
  const table = parseRoadCostTable([250, 450, 750, 8000, 11314]);

  test("縦横は 1 マスの費用に長さを掛け、斜めは長さに比例させて端数を切り上げる", () => {
    expect(roadCost(table, "Small", { x: 0, y: 0 }, { x: 4, y: 0 })).toBe(1000);
    expect(roadCost(table, "Large", { x: 0, y: 0 }, { x: 0, y: -2 })).toBe(1500);
    // 450 × 3 × 11314 / 8000 = 1909.2…
    expect(roadCost(table, "Medium", { x: 0, y: 0 }, { x: 3, y: 3 })).toBe(1910);
  });

  test("短い表は受け付けない", () => {
    expect(() => parseRoadCostTable([1, 2, 3])).toThrow();
  });
});

describe("コマンドの組み立て", () => {
  test("道路は始点と終点と種類を送る", () => {
    expect(buildRoadCommand("Large", { x: -1, y: 2 }, { x: 3, y: 2 })).toEqual({
      kind: "BuildRoad",
      road: "Large",
      fromX: -1,
      fromY: 2,
      toX: 3,
      toY: 2,
    });
  });

  test("ブルドーザは角の順番によらず min と max をそろえる", () => {
    expect(bulldozeCommand({ x: 4, y: -1 }, { x: -2, y: 3 })).toEqual({
      kind: "Bulldoze",
      minX: -2,
      minY: -1,
      maxX: 4,
      maxY: 3,
    });
  });

  test("区画は角の順番によらず min と max をそろえて塗る", () => {
    expect(paintZoneCommand("Office", { x: 3, y: 9 }, { x: -1, y: 2 })).toEqual({
      kind: "PaintZone",
      zone: "Office",
      minX: -1,
      minY: 2,
      maxX: 3,
      maxY: 9,
    });
  });

  test("区画の長方形は 1 辺が上限を超えないところで止める", () => {
    const from = { x: 10, y: -5 };
    expect(clampArea(from, { x: 20, y: 0 }, MAX_ZONE_SIDE)).toEqual({ x: 20, y: 0 });
    expect(clampArea(from, { x: 1000, y: -1000 }, MAX_ZONE_SIDE)).toEqual({ x: 265, y: -260 });
    const end = clampArea(from, { x: -999, y: 999 }, MAX_ZONE_SIDE);
    const c = paintZoneCommand("Residential", from, end);
    expect(c.kind === "PaintZone" && [c.maxX - c.minX + 1, c.maxY - c.minY + 1]).toEqual([
      MAX_ZONE_SIDE,
      MAX_ZONE_SIDE,
    ]);
  });

  test("土地はマスを含むチャンクを買う", () => {
    expect(buyChunkCommand({ x: 64, y: -1 })).toEqual({ kind: "BuyChunk", cx: 1, cy: -1 });
    expect(buyChunkCommand({ x: 63, y: -64 })).toEqual({ kind: "BuyChunk", cx: 0, cy: -1 });
    expect(buyChunkCommand({ x: -65, y: 0 })).toEqual({ kind: "BuyChunk", cx: -2, cy: 0 });
  });
});

describe("施設", () => {
  test("向きごとの占める長方形は、間口が道路に沿い、奥行きが道路から離れる", () => {
    const at = { x: 10, y: 10 };
    // 0：道路は上。間口は右へ、奥行きは下へ
    expect(serviceFootprint(at, 0, 4, 2)).toEqual({ min: { x: 10, y: 10 }, max: { x: 13, y: 11 } });
    // 1：道路は右。間口は下へ、奥行きは左へ
    expect(serviceFootprint(at, 1, 4, 2)).toEqual({ min: { x: 9, y: 10 }, max: { x: 10, y: 13 } });
    // 2：道路は下。間口は左へ、奥行きは上へ
    expect(serviceFootprint(at, 2, 4, 2)).toEqual({ min: { x: 7, y: 9 }, max: { x: 10, y: 10 } });
    // 3：道路は左。間口は上へ、奥行きは右へ
    expect(serviceFootprint(at, 3, 4, 2)).toEqual({ min: { x: 10, y: 7 }, max: { x: 11, y: 10 } });
  });

  test("中核が置けるとした最初の向きを選び、どれも置けなければ置けないものとして返す", () => {
    // 高速道路の脇：北（0）は歩道がなくて置けず、東（1）の普通の道路には置ける
    const probe = ["道路に面していない", null, "ほかの施設か道路がある", "道路に面していない"];
    expect(chooseRotation(probe, null)).toEqual({ rotation: 1, placeable: true, reason: null });
    const none = ["道路に面していない", "道路に面していない", "水辺を通る", "道路に面していない"];
    expect(chooseRotation(none, null)).toEqual({
      rotation: 0,
      placeable: false,
      reason: "道路に面していない",
    });
  });

  test("手で回した向きは、置けなくてもそのまま使う", () => {
    const probe = [null, "道路に面していない", null, null];
    expect(chooseRotation(probe, 1)).toEqual({
      rotation: 1,
      placeable: false,
      reason: "道路に面していない",
    });
    expect(chooseRotation([], null).placeable).toBe(false);
  });

  test("施設の表を読み、置くコマンドを組み立てる", () => {
    expect(parseServiceTable([60000, 4, 4, 25000, 2, 2]).WaterPump).toEqual({
      cost: 25000,
      width: 2,
      depth: 2,
    });
    expect(() => parseServiceTable([1, 2])).toThrow();
    expect(placeServiceCommand("PowerPlant", { x: -3, y: 4 }, 2)).toEqual({
      kind: "PlaceBuilding",
      service: "PowerPlant",
      atX: -3,
      atY: 4,
      rotation: 2,
    });
  });
});
