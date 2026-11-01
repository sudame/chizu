import { describe, expect, test } from "vitest";
import { cssColor, legendSections, trianglePoints, zoneGroundColor } from "./legend.ts";
import { BOLT_SHAPE, NO_POWER_COLOR, NO_WATER_COLOR } from "./map/roads.ts";

describe("凡例", () => {
  test("区画・建物の段階・印・施設の順に並ぶ", () => {
    const sections = legendSections();
    expect(sections.map((s) => s.items.length)).toEqual([4, 5, 2, 2]);
    expect(sections[0]?.items.map((i) => i.label)).toEqual(["住宅", "商業", "工業", "オフィス"]);
    expect(sections[3]?.items.map((i) => i.label)).toEqual(["発電所", "浄水場"]);
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

  test("区画の地面の色は草地に区画の色を混ぜたもの", () => {
    // 住宅：緑 0.15, 0.95, 0.15 を 0.55 の割合で草地 0.56, 0.72, 0.42 に混ぜる
    expect(zoneGroundColor(1).map((v) => Number(v.toFixed(3)))).toEqual([0.335, 0.846, 0.271, 1]);
  });

  test("色の変換は 0〜255 に収める", () => {
    expect(cssColor([1, 0.5, -1, 1])).toBe("rgb(255 128 0)");
  });

  test("図形の座標を SVG の大きさに直す", () => {
    const first = BOLT_SHAPE[0];
    expect(first).toBeDefined();
    if (first !== undefined) {
      expect(trianglePoints(first, 16)).toBe("12,0 0.8,9.6 10.4,9.6");
    }
  });
});
