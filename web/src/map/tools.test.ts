import { describe, expect, test } from "vitest";
import { Camera } from "./camera.ts";
import {
  buildRoadCommand,
  bulldozeCommand,
  buyChunkCommand,
  MAX_ROAD_TILES,
  paintZoneCommand,
  parseRoadCostTable,
  roadCost,
  screenToTile,
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

  test("土地はマスを含むチャンクを買う", () => {
    expect(buyChunkCommand({ x: 64, y: -1 })).toEqual({ kind: "BuyChunk", cx: 1, cy: -1 });
    expect(buyChunkCommand({ x: 63, y: -64 })).toEqual({ kind: "BuyChunk", cx: 0, cy: -1 });
    expect(buyChunkCommand({ x: -65, y: 0 })).toEqual({ kind: "BuyChunk", cx: -2, cy: 0 });
  });
});
