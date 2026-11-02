import { describe, expect, test } from "vitest";
import { cssColor, legendSections, trianglePoints } from "./legend.ts";
import { WALKER_COLOR } from "./map/mover-colors.ts";
import {
  NO_POWER_COLOR,
  NO_WATER_COLOR,
  roadStyle,
  zoneEdgeColor,
  zoneGroundColor,
} from "./map/roads.ts";

describe("凡例", () => {
  test("区画・建物の段階・印・交通・施設・道路・移動体の順に並ぶ", () => {
    const sections = legendSections();
    expect(sections.map((s) => s.items.length)).toEqual([4, 5, 2, 4, 2, 4, 2]);
    expect(sections[0]?.items.map((i) => i.label)).toEqual(["住宅", "商業", "工業", "オフィス"]);
    expect(sections[3]?.items.map((i) => i.label)).toEqual([
      "空いている",
      "やや混んでいる",
      "混んでいる",
      "詰まっている",
    ]);
    expect(sections[4]?.items.map((i) => i.label)).toEqual(["発電所", "浄水場"]);
    expect(sections[5]?.items.map((i) => i.label)).toEqual([
      "小さい道路",
      "中くらいの道路",
      "大きい道路",
      "高速道路",
    ]);
  });

  test("道路は車道の色を、歩道か路肩の色の縁で示す", () => {
    const roads = legendSections()[5]?.items ?? [];
    expect(roads[0]?.color).toBe(cssColor(roadStyle(1).asphalt));
    expect(roads[0]?.border).toBe(cssColor(roadStyle(1).edge));
    // 高速道路の路肩は歩道と違う色
    expect(roads[3]?.border).not.toBe(roads[0]?.border);
    expect(legendSections()[6]?.items[1]?.color).toBe(cssColor(WALKER_COLOR));
  });

  test("印の色は描画の定数と同じで、形を持つ", () => {
    const marks = legendSections()[2]?.items ?? [];
    expect(marks[0]?.color).toBe(cssColor(NO_POWER_COLOR));
    expect(marks[1]?.color).toBe(cssColor(NO_WATER_COLOR));
    expect(marks.every((m) => m.icon !== undefined)).toBe(true);
  });

  test("建物の段階は上がるほど明るい", () => {
    const levels = legendSections()[1]?.items ?? [];
    const sum = (css: string) => (css.match(/\d+/g) ?? []).reduce((a, v) => a + Number(v), 0);
    const sums = levels.map((i) => sum(i.color));
    expect(sums).toEqual([...sums].sort((a, b) => a - b));
    expect(new Set(sums).size).toBe(5);
  });

  test("区画は薄い地面の色に、濃い枠の色を添える", () => {
    const zones = legendSections()[0]?.items ?? [];
    expect(zones[0]?.color).toBe(cssColor(zoneGroundColor(1)));
    expect(zones[0]?.border).toBe(cssColor(zoneEdgeColor(1)));
  });

  test("色の変換は 0〜255 に収める", () => {
    expect(cssColor([1, 0.5, -1, 1])).toBe("rgb(255 128 0)");
  });

  test("図形の座標を SVG の大きさに直す", () => {
    expect(
      trianglePoints(
        [
          [-1, -1],
          [1, -1],
          [0, 1],
        ],
        10,
      ),
    ).toBe("0,0 10,0 5,10");
  });
});
