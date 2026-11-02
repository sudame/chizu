import { describe, expect, it } from "vitest";
import { formatLength, SCALE_MAX_PX, scaleBar } from "./scale-bar.ts";

describe("縮尺の物差し", () => {
  it("始めの倍率 8 では、1 ピクセルが 1m なので 100m になる", () => {
    const bar = scaleBar(8);
    expect(bar.meters).toBe(100);
    expect(bar.px).toBe(100);
    expect(bar.label).toBe("100 m");
  });

  it("引いた倍率 1 では、1 ピクセルが 8m なので 1 km になる", () => {
    const bar = scaleBar(1);
    expect(bar.meters).toBe(1000);
    expect(bar.label).toBe("1 km");
    expect(bar.px).toBe(125);
  });

  it("長さは 1、2、5 のどれかに 10 の冪をかけた値になる", () => {
    for (const zoom of [1, 1.3, 2, 3, 4.7, 8, 12, 20, 32, 64, 128]) {
      const { meters } = scaleBar(zoom);
      const mantissa = meters / 10 ** Math.floor(Math.log10(meters));
      expect([1, 2, 5]).toContain(Math.round(mantissa * 1e6) / 1e6);
    }
  });

  it("線は 140px に収まり、次に長い切りのよい値は収まらない", () => {
    for (const zoom of [1, 1.3, 2, 3, 4.7, 8, 12, 20, 32, 64, 128]) {
      const bar = scaleBar(zoom);
      expect(bar.px).toBeLessThanOrEqual(SCALE_MAX_PX);
      // 切りのよい値の並びで 1 つ上の長さは、140px を超える
      const mantissa = Math.round(bar.meters / 10 ** Math.floor(Math.log10(bar.meters)));
      const next =
        mantissa === 1 ? bar.meters * 2 : mantissa === 2 ? bar.meters * 2.5 : bar.meters * 2;
      expect(next / (8 / zoom)).toBeGreaterThan(SCALE_MAX_PX);
    }
  });

  it("寄ると m の小さな値になる", () => {
    expect(scaleBar(128).label).toBe("5 m");
    expect(scaleBar(32).label).toBe("20 m");
  });

  it("1000m 以上は km で書く", () => {
    expect(formatLength(999)).toBe("999 m");
    expect(formatLength(2000)).toBe("2 km");
  });
});
