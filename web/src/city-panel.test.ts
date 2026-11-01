import { describe, expect, test } from "vitest";
import {
  BAR_HALF_PX,
  DEMAND_FULL_SCALE,
  demandBar,
  demandBars,
  formatCityNumbers,
  parseCityStats,
} from "./city-panel.ts";

describe("街の数字", () => {
  test("city_stats の並びを読む", () => {
    expect(parseCityStats([3, 40, 12, 20, -5, 0, 7])).toEqual({
      buildings: 3,
      households: 40,
      workplaces: 12,
      demand: [20, -5, 0, 7],
    });
  });

  test("棒は住宅・商業・工業・オフィスの順に並び、正は上、負は下へ伸びる", () => {
    const stats = parseCityStats([0, 0, 0, DEMAND_FULL_SCALE / 2, -DEMAND_FULL_SCALE, 0, 999]);
    expect(demandBars(stats)).toEqual([
      { value: DEMAND_FULL_SCALE / 2, top: BAR_HALF_PX / 2, height: BAR_HALF_PX / 2 },
      { value: -DEMAND_FULL_SCALE, top: BAR_HALF_PX, height: BAR_HALF_PX },
      { value: 0, top: BAR_HALF_PX, height: 0 },
      { value: 999, top: 0, height: BAR_HALF_PX },
    ]);
  });

  test("建物がなくなると 0 に戻り、上限だと分かる言葉で出す", () => {
    expect(formatCityNumbers(parseCityStats([12, 1_200, 30, 0, 0, 0, 0]))).toBe(
      "建物 12 棟・住める世帯 1,200・職場の枠 30",
    );
    expect(formatCityNumbers(parseCityStats([0, 0, 0, 20, 0, 10, 0]))).toBe(
      "建物 0 棟・住める世帯 0・職場の枠 0",
    );
  });

  test("需要の棒は -1 から 1 に収める", () => {
    expect(demandBar(DEMAND_FULL_SCALE / 2)).toBe(0.5);
    expect(demandBar(-DEMAND_FULL_SCALE / 4)).toBe(-0.25);
    expect(demandBar(DEMAND_FULL_SCALE * 10)).toBe(1);
    expect(demandBar(-DEMAND_FULL_SCALE * 10)).toBe(-1);
  });
});
