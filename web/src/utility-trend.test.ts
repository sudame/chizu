import { describe, expect, test } from "vitest";
import {
  demandADayAgo,
  demandTrend,
  formatTimeUntilOut,
  MIN_TREND_TICKS,
  pushDemandSample,
  TICKS_PER_DAY,
} from "./utility-trend.ts";

const N = 7; // 主の網の印

describe("需要の伸びからの見積もり", () => {
  test("伸びているなら、尽きるまでの tick を整数で求める", () => {
    // chizu-core の demand_trend のテストと同じ数。余り 1,000 で、需要が 1 日に 500 伸びる。
    const series = [{ tick: 0, demand: 1000, net: N }];
    expect(demandTrend(series, TICKS_PER_DAY, 1500, 1000, N)).toEqual({
      kind: "shrinking",
      growthPerDay: 500,
      ticksUntilOut: 2 * TICKS_PER_DAY,
    });
    // 伸びが倍なら、尽きるのは半分の時間
    expect(demandTrend(series, TICKS_PER_DAY, 2000, 1000, N)).toEqual({
      kind: "shrinking",
      growthPerDay: 1000,
      ticksUntilOut: TICKS_PER_DAY,
    });
  });

  test("伸びていなければ減っていない、余りが負なら足りない", () => {
    const series = [{ tick: 0, demand: 1000, net: N }];
    expect(demandTrend(series, TICKS_PER_DAY, 1000, 500, N)).toEqual({ kind: "notShrinking" });
    expect(demandTrend(series, TICKS_PER_DAY, 900, 500, N)).toEqual({ kind: "notShrinking" });
    expect(demandTrend(series, TICKS_PER_DAY, 1500, -1, N)).toEqual({ kind: "short" });
  });

  test("余りが増えても（施設を足しても）、伸びは変わらない", () => {
    const series = [{ tick: 0, demand: 1000, net: N }];
    const b = demandTrend(series, TICKS_PER_DAY, 1300, 3900, N);
    expect(b).toEqual({ kind: "shrinking", growthPerDay: 300, ticksUntilOut: 13 * TICKS_PER_DAY });
    expect(demandTrend(series, TICKS_PER_DAY, 1300, 300, N)).toMatchObject({ growthPerDay: 300 });
  });

  test("1 日前の控えがないあいだは見積もらない", () => {
    expect(demandTrend([], TICKS_PER_DAY, 1500, 1000, N)).toBeNull();
    const series = [{ tick: 0, demand: 1000, net: N }];
    expect(demandTrend(series, TICKS_PER_DAY - 1, 1500, 1000, N)).toBeNull();
    expect(demandTrend(series, TICKS_PER_DAY, 1500, 1000, N)?.kind).toBe("shrinking");
  });

  test("主の網が替わったときは見積もらない", () => {
    const series = [{ tick: 0, demand: 100, net: N + 1 }];
    expect(demandTrend(series, TICKS_PER_DAY, 1500, 1000, N)).toBeNull();
  });
});

describe("1 日前の需要", () => {
  test("1 日前以前でいちばん新しい控えの値を使う（控えのあいだは値が変わっていない）", () => {
    const series = [
      { tick: 0, demand: 1000, net: N },
      { tick: 1200, demand: 1200, net: N },
    ];
    expect(demandADayAgo(series, TICKS_PER_DAY + 600, N)).toBe(1000);
    expect(demandADayAgo(series, TICKS_PER_DAY + 1200, N)).toBe(1200);
  });

  test("静かな時間が長くても、直前の値をそのまま使う", () => {
    // 1 日前の前後 1 日、控えがない（数字が変わらず、画面に何も届かなかった）
    const series = [
      { tick: 1000, demand: 1000, net: N },
      { tick: 3 * TICKS_PER_DAY, demand: 1100, net: N },
    ];
    expect(demandADayAgo(series, 2 * TICKS_PER_DAY, N)).toBe(1000);
    expect(demandADayAgo(series, 3 * TICKS_PER_DAY, N)).toBe(1000);
  });

  test("1 日前以前の控えがないとき、網の印が違うときは求めない", () => {
    expect(demandADayAgo([{ tick: 100, demand: 1000, net: N }], TICKS_PER_DAY, N)).toBeNull();
    expect(demandADayAgo([{ tick: 0, demand: 1000, net: N + 1 }], TICKS_PER_DAY, N)).toBeNull();
  });
});

describe("控え", () => {
  test("近すぎる数字は末尾の控えを上書きする。末尾が窓の先頭なら残して後ろに足す", () => {
    let s = pushDemandSample([], 0, 1000, N);
    s = pushDemandSample(s, 10, 990, N);
    // 末尾（tick 0）は窓の先頭なので残り、tick 10 が後ろに付く
    expect(s.map((x) => x.tick)).toEqual([0, 10]);
    s = pushDemandSample(s, 40, 980, N);
    // 末尾（tick 10）は先頭ではないので、新しい値で上書きされる
    expect(s).toEqual([
      { tick: 0, demand: 1000, net: N },
      { tick: 40, demand: 980, net: N },
    ]);
  });

  test("静かな時間の前の最後の変化は、近い数字でも控えに残る", () => {
    // tick 0 に 100、tick 30 に 150 になり、そのあと 7300 まで数字が来ない。tick 100 の本当の値は 150。
    let s = pushDemandSample([], 0, 100, N);
    s = pushDemandSample(s, 30, 150, N);
    s = pushDemandSample(s, 7300, 200, N);
    expect(demandADayAgo(s, 7300, N)).toBe(150);
  });

  test("1 日前より古い控えは、1 日前以前でいちばん新しい 1 つだけ残す", () => {
    let s = pushDemandSample([], 0, 1000, N);
    s = pushDemandSample(s, 100, 990, N);
    s = pushDemandSample(s, TICKS_PER_DAY + 50, 900, N);
    // 1 日前（tick 50）以前でいちばん新しい控えは tick 0
    expect(s.map((x) => x.tick)).toEqual([0, 100, TICKS_PER_DAY + 50]);
    s = pushDemandSample(s, TICKS_PER_DAY + 150, 880, N);
    // 1 日前（tick 150）以前でいちばん新しい控えは tick 100 なので、tick 0 は捨てる
    expect(s.map((x) => x.tick)).toEqual([100, TICKS_PER_DAY + 50, TICKS_PER_DAY + 150]);
  });

  test("tick が戻ったとき（読み込みや新しい街）は数え直す", () => {
    const s = [
      { tick: 1000, demand: 800, net: N },
      { tick: 2000, demand: 700, net: N },
    ];
    expect(pushDemandSample(s, 500, 650, N)).toEqual([{ tick: 500, demand: 650, net: N }]);
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
