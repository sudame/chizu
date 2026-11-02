import { describe, expect, test } from "vitest";
import {
  carriagewayHalfWidth,
  crossSection,
  HIGHWAY_CODE,
  laneCenter,
  lanesPerSide,
  laneWidth,
  roadHalfWidth,
  SIDEWALK_WIDTH,
} from "./lanes.ts";

describe("道路の断面", () => {
  test("車線の数は小さい道路から順に片側 1、2、3 本で、高速道路は 2 本", () => {
    expect([1, 2, 3, HIGHWAY_CODE].map(lanesPerSide)).toEqual([1, 2, 3, 2]);
  });

  test("道路は 1 マスに収まり、半分の幅は小 0.3、中 0.4、大 0.5、高速 0.5", () => {
    const halves = [1, 2, 3, HIGHWAY_CODE].map(roadHalfWidth);
    expect(halves.map((h) => Number(h.toFixed(6)))).toEqual([0.3, 0.4, 0.5, 0.5]);
    for (const h of halves) {
      expect(h).toBeLessThanOrEqual(0.5);
    }
    // 高速道路は中くらいの道路より細くない
    expect(roadHalfWidth(HIGHWAY_CODE)).toBeGreaterThanOrEqual(roadHalfWidth(2));
    expect(carriagewayHalfWidth(3)).toBeCloseTo(3 * laneWidth(3));
    expect(roadHalfWidth(1)).toBeCloseTo(laneWidth(1) + SIDEWALK_WIDTH);
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
      [-2 * laneWidth(3), -laneWidth(3), laneWidth(3), 2 * laneWidth(3)].map((v) =>
        expect.closeTo(v),
      ),
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
      [2_800 / 8_000, 1_680 / 8_000, 560 / 8_000].map((v) => expect.closeTo(v)),
    );
    expect(laneCenter(1, 0)).toBeCloseTo(880 / 8_000);
    // 範囲を越えた番号は端の車線に丸める
    expect(laneCenter(2, 9)).toBeCloseTo(laneCenter(2, 1));
  });
});
