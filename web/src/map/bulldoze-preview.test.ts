import { describe, expect, test } from "vitest";
import {
  bulldozeSummary,
  isEmptyPreview,
  parseBulldozePreview,
  removedRoadTiles,
} from "./bulldoze-preview.ts";

// 道路のマス、施設、建物、家をなくす世帯、区間、線路のマス、駅、線路の数に、
// 区間 1 本、施設 1 つ、建物 2 棟が続く
const SAMPLE = [21, 1, 2, 5, 1, 0, 0, 0, 10, 10, 30, 10, 3, 3, 4, 4, 11, 8, 12, 9, 20, 8, 21, 9];

// 線路のマス 18、駅 1 つ、線路 2 本だけ
const RAIL = [0, 0, 0, 0, 0, 18, 1, 2, 3, 7, 12, 9, 3, 8, 12, 8, 12, 8, 20, 8];

describe("parseBulldozePreview", () => {
  test("数の並びを区間、施設、建物に読む", () => {
    expect(parseBulldozePreview(SAMPLE)).toEqual({
      roadTiles: 21,
      railTiles: 0,
      displacedHouseholds: 5,
      segments: [{ from: { x: 10, y: 10 }, to: { x: 30, y: 10 } }],
      facilities: [{ min: { x: 3, y: 3 }, max: { x: 4, y: 4 } }],
      buildings: [
        { min: { x: 11, y: 8 }, max: { x: 12, y: 9 } },
        { min: { x: 20, y: 8 }, max: { x: 21, y: 9 } },
      ],
      stations: [],
      tracks: [],
    });
  });

  test("駅と線路を読む", () => {
    expect(parseBulldozePreview(RAIL)).toEqual({
      roadTiles: 0,
      railTiles: 18,
      displacedHouseholds: 0,
      segments: [],
      facilities: [],
      buildings: [],
      stations: [{ min: { x: 3, y: 7 }, max: { x: 12, y: 9 } }],
      tracks: [
        { from: { x: 3, y: 8 }, to: { x: 12, y: 8 } },
        { from: { x: 12, y: 8 }, to: { x: 20, y: 8 } },
      ],
    });
  });

  test("空や長さの合わない並びは null", () => {
    expect(parseBulldozePreview([])).toBeNull();
    expect(parseBulldozePreview(SAMPLE.slice(0, -1))).toBeNull();
  });
});

describe("bulldozeSummary", () => {
  test("消える数を並べる", () => {
    const p = parseBulldozePreview(SAMPLE);
    expect(p && bulldozeSummary(p)).toBe(
      "消える：道路 21 マス・建物 2 棟・施設 1・家をなくす世帯 5",
    );
  });

  test("消えないものは出さず、何も消えなければ空", () => {
    const p = parseBulldozePreview([4, 0, 0, 0, 1, 0, 0, 0, 1, 1, 5, 1]);
    expect(p && bulldozeSummary(p)).toBe("消える：道路 4 マス");
    const none = parseBulldozePreview([0, 0, 0, 0, 0, 0, 0, 0]);
    expect(none && isEmptyPreview(none)).toBe(true);
    expect(none && bulldozeSummary(none)).toBe("");
  });

  test("線路のマスと駅の数を出す", () => {
    const p = parseBulldozePreview(RAIL);
    expect(p && isEmptyPreview(p)).toBe(false);
    expect(p && bulldozeSummary(p)).toBe("消える：線路 18 マス・駅 1");
  });
});

describe("removedRoadTiles", () => {
  test("1 マスのドラッグでも区間のマスをすべて挙げ、交差点は 1 度だけ", () => {
    const p = parseBulldozePreview([7, 0, 0, 0, 2, 0, 0, 0, 0, 0, 3, 0, 3, 0, 3, 3]);
    const tiles = p ? removedRoadTiles(p) : [];
    expect(tiles).toHaveLength(7);
    expect(tiles).toContainEqual({ x: 0, y: 0 });
    expect(tiles).toContainEqual({ x: 3, y: 3 });
  });

  test("斜めの区間は 45 度でたどる", () => {
    const p = parseBulldozePreview([3, 0, 0, 0, 1, 0, 0, 0, 0, 0, 2, 2]);
    expect(p && removedRoadTiles(p)).toEqual([
      { x: 0, y: 0 },
      { x: 1, y: 1 },
      { x: 2, y: 2 },
    ]);
  });
});
