import { describe, expect, it } from "vitest";
import { describeSpeed, shownSpeed, splitSpeed, stepBudget, tickRate } from "./speed.ts";

const FRAME_MS = 1000 / 60;
const PER_SECOND = 30;

// 1 秒ぶんのフレームを回して、進んだ tick 数と 1 フレームごとの tick 数を返す
function runOneSecond(rate: number): { total: number; perFrame: number[] } {
  let budget = 0;
  const perFrame: number[] = [];
  for (let i = 0; i < 60; i++) {
    const step = stepBudget(budget, FRAME_MS, rate, false, 0.25);
    budget = step.budget;
    perFrame.push(step.ticks);
  }
  return { total: perFrame.reduce((a, b) => a + b, 0), perFrame };
}

describe("splitSpeed", () => {
  it("1 以上はそのまま中核の速度にする", () => {
    expect(splitSpeed(1)).toEqual({ core: 1, pace: 1 });
    expect(splitSpeed(48)).toEqual({ core: 48, pace: 1 });
  });

  it("1 未満は中核の速度を 1 にして、倍率を Worker に渡す", () => {
    expect(splitSpeed(0.5)).toEqual({ core: 1, pace: 0.5 });
    expect(splitSpeed(0.25)).toEqual({ core: 1, pace: 0.25 });
  });
});

describe("tickRate", () => {
  it("中核の速度と倍率の積を、1 秒の tick 数にする", () => {
    expect(tickRate(PER_SECOND, 1, 1)).toBe(30);
    expect(tickRate(PER_SECOND, 1, 0.5)).toBe(15);
    expect(tickRate(PER_SECOND, 1, 0.25)).toBe(7.5);
    expect(tickRate(PER_SECOND, 48, 1)).toBe(1440);
  });
});

describe("stepBudget", () => {
  it("1 倍では 1 秒に 30 tick 進む", () => {
    expect(runOneSecond(PER_SECOND).total).toBe(30);
  });

  it("0.5 倍では 1 秒に 15 tick、4 フレームに 1 回だけ進む", () => {
    const { total, perFrame } = runOneSecond(PER_SECOND * 0.5);
    expect(total).toBe(15);
    expect(Math.max(...perFrame)).toBe(1);
    expect(perFrame.slice(0, 8)).toEqual([0, 0, 0, 1, 0, 0, 0, 1]);
  });

  it("0.25 倍では 1 秒に 7 か 8 tick、8 フレームに 1 回だけ進む", () => {
    const { total, perFrame } = runOneSecond(PER_SECOND * 0.25);
    expect(total).toBeGreaterThanOrEqual(7);
    expect(total).toBeLessThanOrEqual(8);
    expect(perFrame.slice(0, 16)).toEqual([0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1]);
  });

  it("止めているあいだは進まず、端数も捨てる", () => {
    const step = stepBudget(0.9, 1000, PER_SECOND, true, 0.25);
    expect(step).toEqual({ ticks: 0, budget: 0 });
  });

  it("遅れが上限を超えたぶんは捨てる", () => {
    const step = stepBudget(0, 10_000, PER_SECOND, false, 0.25);
    expect(step.ticks).toBe(7);
  });
});

describe("HUD の速度", () => {
  it("0.25 と 0.5 をそのまま出す", () => {
    expect(describeSpeed(shownSpeed(1, 0.25))).toBe("速度 0.25 倍");
    expect(describeSpeed(shownSpeed(1, 0.5))).toBe("速度 0.5 倍");
  });

  it("遅くしていないときは中核の速度を出す", () => {
    expect(describeSpeed(shownSpeed(1, 1))).toBe("速度 1 倍");
    expect(describeSpeed(shownSpeed(48, 1))).toBe("速度 48 倍");
  });
});
