import { describe, expect, it } from "vitest";
import { describeInspect, formatDuration, INSPECT_LENGTH, parseInspect } from "./inspector.ts";

const pad = (head: number[]) => [...head, ...Array(INSPECT_LENGTH - head.length).fill(-1)];

describe("parseInspect", () => {
  it("空の配列や長さの違う配列は何もないものとして扱う", () => {
    expect(parseInspect([])).toBeNull();
    expect(parseInspect([1, 1, 1])).toBeNull();
  });

  it("住宅を読む", () => {
    expect(parseInspect([1, 1, 2, 1, 0, 8, 3, 9, 62, 80, 50, 55, 330])).toEqual({
      kind: "building",
      zone: "Residential",
      level: 2,
      power: true,
      water: false,
      capacity: 8,
      occupied: 3,
      residents: {
        count: 9,
        happiness: 62,
        commute: 80,
        services: 50,
        wealth: 55,
        commuteSeconds: 330,
      },
    });
  });

  it("住民のいない住宅は住民の様子が null になり、職のない住民の通勤は null になる", () => {
    const empty = parseInspect(pad([1, 1, 1, 1, 1, 2, 0, 0]));
    expect(empty).toMatchObject({ kind: "building", residents: null });
    const jobless = parseInspect([1, 1, 1, 1, 1, 2, 1, 2, 40, 100, 100, 0, -1]);
    expect(jobless).toMatchObject({ residents: { commuteSeconds: null } });
  });

  it("職場と施設を読む", () => {
    expect(parseInspect(pad([1, 3, 4, 1, 1, 12, 5, -1]))).toMatchObject({
      zone: "Industrial",
      capacity: 12,
      occupied: 5,
      residents: null,
    });
    expect(parseInspect(pad([2, 2, 1000, 600]))).toEqual({
      kind: "facility",
      service: "WaterPump",
      capacity: 1000,
      upkeepPerDay: 600,
    });
  });

  it("知らない区画や施設は読まない", () => {
    expect(parseInspect(pad([1, 9, 1, 1, 1, 1, 1]))).toBeNull();
    expect(parseInspect(pad([2, 9, 1, 1]))).toBeNull();
    expect(parseInspect(pad([7]))).toBeNull();
  });
});

describe("describeInspect", () => {
  it("住宅の世帯、住民、幸福度とその要素、電気と水道、通勤を出す", () => {
    const view = parseInspect([1, 1, 2, 1, 0, 8, 3, 9, 62, 80, 50, 55, 330]);
    expect(view).not.toBeNull();
    const text = describeInspect(view as NonNullable<typeof view>);
    expect(text.title).toBe("住宅（段階 2）");
    expect(text.rows).toEqual([
      { label: "世帯", value: "3 / 8" },
      { label: "住民", value: "9 人" },
      { label: "幸福度", value: "62 / 100" },
      { label: "　通勤", value: "80" },
      { label: "　生活サービス", value: "50" },
      { label: "　暮らし向き", value: "55" },
      { label: "電気", value: "届いている" },
      { label: "水道", value: "届いていない" },
      { label: "通勤の所要時間", value: "約 6 分" },
    ]);
  });

  it("住民のいない住宅は幸福度と通勤を出さない", () => {
    const view = parseInspect(pad([1, 1, 1, 0, 0, 2, 0, 0]));
    const text = describeInspect(view as NonNullable<typeof view>);
    expect(text.rows.map((r) => r.label)).toEqual(["世帯", "住民", "電気", "水道"]);
    expect(text.rows[1]?.value).toBe("0 人");
  });

  it("職場は種類と段階、働く人の数と上限、電気と水道を出す", () => {
    const view = parseInspect(pad([1, 2, 3, 1, 1, 1200, 1100]));
    const text = describeInspect(view as NonNullable<typeof view>);
    expect(text.title).toBe("商業の職場（段階 3）");
    expect(text.rows).toEqual([
      { label: "働く人", value: "1,100 / 1,200 人" },
      { label: "電気", value: "届いている" },
      { label: "水道", value: "届いている" },
    ]);
  });

  it("施設は種類、供給量、維持費を出す", () => {
    const view = parseInspect(pad([2, 1, 1200, 1200]));
    const text = describeInspect(view as NonNullable<typeof view>);
    expect(text).toEqual({
      title: "発電所",
      rows: [
        { label: "供給量", value: "1,200" },
        { label: "維持費", value: "1,200 / 日" },
      ],
    });
  });
});

describe("formatDuration", () => {
  it("1 分に満たなければ秒、それ以上は分で丸める", () => {
    expect(formatDuration(0)).toBe("0 秒");
    expect(formatDuration(59)).toBe("59 秒");
    expect(formatDuration(60)).toBe("約 1 分");
    expect(formatDuration(89)).toBe("約 1 分");
    expect(formatDuration(90)).toBe("約 2 分");
  });
});
