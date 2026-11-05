import { describe, expect, test } from "vitest";
import {
  formatTimeUntilOut,
  headroomTrend,
  MIN_TREND_TICKS,
  pushHeadroomSample,
  TICKS_PER_DAY,
} from "./utility-trend.ts";

describe("余りの減り方", () => {
  test("減っているなら、1 日あたりの減りと尽きるまでの tick を整数で求める", () => {
    // chizu-core の headroom_trend のテストと同じ数
    const series = [{ tick: 0, people: 1500 }];
    expect(headroomTrend(series, TICKS_PER_DAY, 1000)).toEqual({
      kind: "shrinking",
      dropPerDay: 500,
      ticksUntilOut: 2 * TICKS_PER_DAY,
    });
    expect(headroomTrend(series, TICKS_PER_DAY / 2, 1000)).toEqual({
      kind: "shrinking",
      dropPerDay: 1000,
      ticksUntilOut: TICKS_PER_DAY,
    });
  });

  test("増えたか同じなら減っていない、負なら足りない", () => {
    const series = [{ tick: 0, people: 1000 }];
    expect(headroomTrend(series, TICKS_PER_DAY, 1500)).toEqual({ kind: "notShrinking" });
    expect(headroomTrend(series, TICKS_PER_DAY, 1000)).toEqual({ kind: "notShrinking" });
    expect(headroomTrend(series, TICKS_PER_DAY, -1)).toEqual({ kind: "short" });
  });

  test("1 時間に満たないあいだと、控えがないあいだは見積もらない", () => {
    const series = [{ tick: 0, people: 1500 }];
    expect(headroomTrend(series, MIN_TREND_TICKS - 1, 1000)).toBeNull();
    expect(headroomTrend(series, MIN_TREND_TICKS, 1000)?.kind).toBe("shrinking");
    expect(headroomTrend([], 100, 1000)).toBeNull();
  });
});

describe("控え", () => {
  test("間隔が短いあいだは足さず、1 日より古い控えは捨てる", () => {
    let s = pushHeadroomSample([], 0, 1000);
    s = pushHeadroomSample(s, 10, 990);
    expect(s).toEqual([{ tick: 0, people: 1000 }]);
    s = pushHeadroomSample(s, 100, 990);
    s = pushHeadroomSample(s, TICKS_PER_DAY + 50, 900);
    expect(s).toEqual([
      { tick: 100, people: 990 },
      { tick: TICKS_PER_DAY + 50, people: 900 },
    ]);
  });

  test("tick が戻ったときと、余りが大きく増えたとき（施設を足したとき）は数え直す", () => {
    const s = [
      { tick: 1000, people: 800 },
      { tick: 2000, people: 700 },
    ];
    expect(pushHeadroomSample(s, 500, 650)).toEqual([{ tick: 500, people: 650 }]);
    expect(pushHeadroomSample(s, 3000, 2200)).toEqual([{ tick: 3000, people: 2200 }]);
    // 端数の揺れでは数え直さない
    expect(pushHeadroomSample(s, 3000, 720)).toHaveLength(3);
  });
});

describe("尽きるまでの時間の言い方", () => {
  test("時計の時間で言う", () => {
    expect(formatTimeUntilOut(100)).toBe("1 時間もたたずに");
    expect(formatTimeUntilOut(8 * MIN_TREND_TICKS)).toBe("約 8 時間で");
    expect(formatTimeUntilOut(8.4 * MIN_TREND_TICKS)).toBe("約 8 時間で");
    expect(formatTimeUntilOut(2 * TICKS_PER_DAY)).toBe("約 2 日で");
  });
});
