import { describe, expect, test } from "vitest";
import { DEMAND_FULL_SCALE, demandBar, parseCityStats } from "./city-panel.ts";

describe("街の数字", () => {
  test("city_stats の並びを読む", () => {
    expect(parseCityStats([3, 40, 12, 20, -5, 0, 7])).toEqual({
      buildings: 3,
      households: 40,
      workplaces: 12,
      demand: [20, -5, 0, 7],
    });
  });

  test("需要の棒は -1 から 1 に収める", () => {
    expect(demandBar(DEMAND_FULL_SCALE / 2)).toBe(0.5);
    expect(demandBar(-DEMAND_FULL_SCALE / 4)).toBe(-0.25);
    expect(demandBar(DEMAND_FULL_SCALE * 10)).toBe(1);
    expect(demandBar(-DEMAND_FULL_SCALE * 10)).toBe(-1);
  });
});
