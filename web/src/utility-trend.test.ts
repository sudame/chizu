import { describe, expect, test } from "vitest";
import {
  demandADayAgo,
  demandTrend,
  formatTimeUntilOut,
  MIN_TREND_TICKS,
  pushDemandSample,
  TICKS_PER_DAY,
} from "./utility-trend.ts";

describe("需要の伸びからの見積もり", () => {
  test("伸びているなら、尽きるまでの tick を整数で求める", () => {
    // chizu-core の demand_trend のテストと同じ数。余り 1,000 で、需要が 1 日に 500 伸びる。
    const series = [{ tick: 0, demand: 1000 }];
    expect(demandTrend(series, TICKS_PER_DAY, 1500, 1000)).toEqual({
      kind: "shrinking",
      growthPerDay: 500,
      ticksUntilOut: 2 * TICKS_PER_DAY,
    });
    // 伸びが倍なら、尽きるのは半分の時間
    expect(demandTrend(series, TICKS_PER_DAY, 2000, 1000)).toEqual({
      kind: "shrinking",
      growthPerDay: 1000,
      ticksUntilOut: TICKS_PER_DAY,
    });
  });

  test("伸びていなければ減っていない、余りが負なら足りない", () => {
    const series = [{ tick: 0, demand: 1000 }];
    expect(demandTrend(series, TICKS_PER_DAY, 1000, 500)).toEqual({ kind: "notShrinking" });
    expect(demandTrend(series, TICKS_PER_DAY, 900, 500)).toEqual({ kind: "notShrinking" });
    expect(demandTrend(series, TICKS_PER_DAY, 1500, -1)).toEqual({ kind: "short" });
  });

  test("余りが増えても（施設を足しても）、伸びは変わらない", () => {
    const series = [{ tick: 0, demand: 1000 }];
    const a = demandTrend(series, TICKS_PER_DAY, 1300, 300);
    const b = demandTrend(series, TICKS_PER_DAY, 1300, 3900);
    expect(a?.kind === "shrinking" && b?.kind === "shrinking" && a.growthPerDay).toBe(300);
    expect(b).toEqual({ kind: "shrinking", growthPerDay: 300, ticksUntilOut: 13 * TICKS_PER_DAY });
  });

  test("1 日前の控えがないあいだは見積もらない", () => {
    expect(demandTrend([], TICKS_PER_DAY, 1500, 1000)).toBeNull();
    const series = [{ tick: 0, demand: 1000 }];
    expect(demandTrend(series, TICKS_PER_DAY - 1, 1500, 1000)).toBeNull();
    expect(demandTrend(series, TICKS_PER_DAY, 1500, 1000)?.kind).toBe("shrinking");
  });
});

describe("1 日前の需要", () => {
  test("その tick をはさむ 2 つの控えを直線でつなぐ（小数は 0 向きに切り捨て）", () => {
    const series = [
      { tick: 0, demand: 1000 },
      { tick: 1200, demand: 1200 },
    ];
    expect(demandADayAgo(series, TICKS_PER_DAY + 600)).toBe(1100);
    expect(demandADayAgo(series, TICKS_PER_DAY)).toBe(1000);
    // chizu-core の interpolate_demand と同じ向き
    expect(
      demandADayAgo(
        [
          { tick: 100, demand: 1000 },
          { tick: 400, demand: 995 },
        ],
        TICKS_PER_DAY + 200,
      ),
    ).toBe(999);
  });

  test("はさむ控えがないとき、離れすぎているときは求めない", () => {
    expect(demandADayAgo([{ tick: 100, demand: 1000 }], TICKS_PER_DAY)).toBeNull();
    expect(demandADayAgo([{ tick: 0, demand: 1000 }], TICKS_PER_DAY + 5)).toBeNull();
    const far = [
      { tick: 0, demand: 1000 },
      { tick: TICKS_PER_DAY / 2, demand: 1100 },
    ];
    expect(demandADayAgo(far, TICKS_PER_DAY + 1800)).toBeNull();
  });
});

describe("控え", () => {
  test("間隔が短いあいだは足さず、1 日とはさむ間隔より古い控えは捨てる", () => {
    let s = pushDemandSample([], 0, 1000);
    s = pushDemandSample(s, 10, 990);
    expect(s).toEqual([{ tick: 0, demand: 1000 }]);
    s = pushDemandSample(s, 100, 990);
    s = pushDemandSample(s, TICKS_PER_DAY + 1800 + 50, 900);
    expect(s).toEqual([
      { tick: 100, demand: 990 },
      { tick: TICKS_PER_DAY + 1800 + 50, demand: 900 },
    ]);
  });

  test("tick が戻ったとき（読み込みや新しい街）は数え直す", () => {
    const s = [
      { tick: 1000, demand: 800 },
      { tick: 2000, demand: 700 },
    ];
    expect(pushDemandSample(s, 500, 650)).toEqual([{ tick: 500, demand: 650 }]);
  });

  test("施設を足したなどで余りが変わっても、控えは捨てない", () => {
    // 控えは需要だけを持つので、供給量の変化は関わらない
    const s = pushDemandSample(pushDemandSample([], 0, 1000), 3000, 1200);
    expect(s).toHaveLength(2);
  });
});

describe("尽きるまでの時間の言い方", () => {
  test("時計の時間で言う", () => {
    expect(formatTimeUntilOut(100)).toBe("1 時間もたたずに");
    expect(formatTimeUntilOut(8 * MIN_TREND_TICKS)).toBe("約 8 時間で");
    expect(formatTimeUntilOut(8.9 * MIN_TREND_TICKS)).toBe("約 8 時間で");
    expect(formatTimeUntilOut(2.9 * TICKS_PER_DAY)).toBe("約 2 日で");
    expect(formatTimeUntilOut(2 * TICKS_PER_DAY)).toBe("約 2 日で");
  });
});
