import { describe, expect, test } from "vitest";
import { parseRoadProbe, roadProbeText } from "./road-probe.ts";

describe("道路の見積もりの踏切と路線の上限", () => {
  test("空なら何も出さない", () => {
    expect(parseRoadProbe([])).toBeNull();
    expect(roadProbeText(null)).toBe("");
    expect(roadProbeText(parseRoadProbe([0]))).toBe("");
  });

  test("踏切だけができるとき", () => {
    expect(roadProbeText(parseRoadProbe([1]))).toBe("踏切ができる");
    expect(roadProbeText(parseRoadProbe([3]))).toBe("踏切が 3 本できる");
  });

  test("路線の上限が下がるとき、路線ごとに前と後を出す", () => {
    const probe = parseRoadProbe([1, 0, 0, 3, 1, 1, 0, 2, 0]);
    expect(probe?.lowered).toEqual([
      { index: 0, generation: 0, before: 3, after: 1 },
      { index: 1, generation: 0, before: 2, after: 0 },
    ]);
    expect(roadProbeText(probe)).toBe(
      "踏切ができ、青線の本数の上限が 3 から 1 に下がる、緑線の本数の上限が 2 から 0 に下がる",
    );
  });
});
