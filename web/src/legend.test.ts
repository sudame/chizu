import { describe, expect, test } from "vitest";
import { cssColor, legendSections, trianglePoints } from "./legend.ts";
import { WALKER_COLOR } from "./map/mover-colors.ts";
import {
  buildingColor,
  buildingEdgeColor,
  NO_POWER_COLOR,
  NO_WATER_COLOR,
  roadStyle,
  zoneGroundColor,
} from "./map/roads.ts";

describe("凡例", () => {
  test("区画・建物・印・施設・道路・移動体の順に並ぶ。交通は主題図の凡例に出す", () => {
    const sections = legendSections();
    expect(sections.map((s) => s.items.length)).toEqual([4, 4, 2, 2, 4, 2]);
    expect(sections[0]?.items.map((i) => i.label)).toEqual(["住宅", "商業", "工業", "オフィス"]);
    expect(sections[3]?.items.map((i) => i.label)).toEqual(["発電所", "浄水場"]);
    expect(sections[4]?.items.map((i) => i.label)).toEqual([
      "小さい道路",
      "中くらいの道路",
      "大きい道路",
      "高速道路",
    ]);
  });

  test("道路は車道の塗りの色を、種類ごとの縁の色で示す", () => {
    const roads = legendSections()[4]?.items ?? [];
    expect(roads[0]?.color).toBe(cssColor(roadStyle(1).asphalt));
    expect(roads[0]?.border).toBe(cssColor(roadStyle(1).edge));
    // 高速道路の縁は小の道路と違う色
    expect(roads[3]?.border).not.toBe(roads[0]?.border);
    expect(legendSections()[5]?.items[1]?.color).toBe(cssColor(WALKER_COLOR));
  });

  test("印の色は描画の定数と同じで、形を持つ", () => {
    const marks = legendSections()[2]?.items ?? [];
    expect(marks[0]?.color).toBe(cssColor(NO_POWER_COLOR));
    expect(marks[1]?.color).toBe(cssColor(NO_WATER_COLOR));
    expect(marks.every((m) => m.icon !== undefined)).toBe(true);
  });

  test("建物は区画と同じ順で、建物の色を縁の色で囲んで示す", () => {
    const buildings = legendSections()[1]?.items ?? [];
    expect(buildings.map((i) => i.label)).toEqual(["住宅", "商業", "工業", "オフィス"]);
    expect(buildings[1]?.color).toBe(cssColor(buildingColor(2)));
    expect(buildings[1]?.border).toBe(cssColor(buildingEdgeColor(2)));
  });

  test("区画は地面の色に、建物の縁の色を添える", () => {
    const zones = legendSections()[0]?.items ?? [];
    expect(zones[0]?.color).toBe(cssColor(zoneGroundColor(1)));
    expect(zones[0]?.border).toBe(cssColor(buildingEdgeColor(1)));
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
