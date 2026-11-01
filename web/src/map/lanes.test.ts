import { describe, expect, test } from "vitest";
import {
  carriagewayHalfWidth,
  crossSection,
  HIGHWAY_CODE,
  LANE_DETAIL_ZOOM,
  LANE_WIDTH,
  laneCenter,
  lanesPerSide,
  roadHalfWidth,
  SIDEWALK_WIDTH,
  showsLanes,
} from "./lanes.ts";

describe("道路の断面", () => {
  test("車線の数は小さい道路から順に片側 1、2、3 本で、高速道路は 2 本", () => {
    expect([1, 2, 3, HIGHWAY_CODE].map(lanesPerSide)).toEqual([1, 2, 3, 2]);
  });

  test("幅は車線の数に比例し、歩道がある道路は両端に歩道の幅を足す", () => {
    expect(carriagewayHalfWidth(3)).toBeCloseTo(3 * LANE_WIDTH);
    expect(roadHalfWidth(1)).toBeCloseTo(LANE_WIDTH + SIDEWALK_WIDTH);
    expect(roadHalfWidth(3)).toBeCloseTo(3 * LANE_WIDTH + SIDEWALK_WIDTH);
    expect(roadHalfWidth(HIGHWAY_CODE)).toBeCloseTo(2 * LANE_WIDTH);
    expect(roadHalfWidth(1)).toBeLessThan(roadHalfWidth(2));
    expect(roadHalfWidth(2)).toBeLessThan(roadHalfWidth(3));
  });

  test("小さい道路には中央線と歩道だけがあり、車線の区切りはない", () => {
    const roles = crossSection(1).map((s) => s.role);
    expect(roles).toEqual(["asphalt", "sidewalk", "sidewalk", "center"]);
  });

  test("大きい道路には中央線のほかに、片側に 2 本ずつ車線の区切りがある", () => {
    const strips = crossSection(3);
    expect(strips.filter((s) => s.role === "center")).toHaveLength(1);
    const dividers = strips.filter((s) => s.role === "divider");
    expect(dividers.map((s) => s.offset).sort((a, b) => a - b)).toEqual(
      [-2 * LANE_WIDTH, -LANE_WIDTH, LANE_WIDTH, 2 * LANE_WIDTH].map((v) => expect.closeTo(v)),
    );
  });

  test("高速道路に歩道はない", () => {
    expect(crossSection(HIGHWAY_CODE).some((s) => s.role === "sidewalk")).toBe(false);
  });

  test("歩道は車道の外側に、両側へ置く", () => {
    const sidewalks = crossSection(2).filter((s) => s.role === "sidewalk");
    expect(sidewalks.map((s) => Math.sign(s.offset)).sort()).toEqual([-1, 1]);
    for (const s of sidewalks) {
      expect(Math.abs(s.offset) - s.half).toBeCloseTo(carriagewayHalfWidth(2));
    }
  });

  test("車線の中心は端の車線ほど中心線から遠く、中核の車線の位置と同じ値になる", () => {
    // 中核の lane_shift の試験と同じ値（mm を 8,000 で割ったマス）
    expect([0, 1, 2].map((i) => laneCenter(3, i))).toEqual(
      [4_000 / 8_000, 2_400 / 8_000, 800 / 8_000].map((v) => expect.closeTo(v)),
    );
    expect(laneCenter(1, 0)).toBeCloseTo(800 / 8_000);
    // 範囲を越えた番号は端の車線に丸める
    expect(laneCenter(2, 9)).toBeCloseTo(laneCenter(2, 1));
  });

  test("縮小しすぎると車線ごとには描かない", () => {
    expect(showsLanes(LANE_DETAIL_ZOOM)).toBe(true);
    expect(showsLanes(LANE_DETAIL_ZOOM - 0.1)).toBe(false);
  });
});
