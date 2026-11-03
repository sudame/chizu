import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { TICKS_PER_SECOND } from "./map/mover-interpolation.ts";
import { BAR_SPEEDS, describeSpeed, EXTRA_SPEEDS, stepBudget, tickRate } from "./speed.ts";

const FRAME_MS = 1000 / 60;
const TICKS_PER_DAY = 7_200;

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

const source = (path: string) => readFileSync(new URL(path, import.meta.url), "utf8");

describe("Rust との写し", () => {
  it("等速の tick 数は chizu-core の TICKS_PER_SECOND_AT_NORMAL_SPEED と同じ", () => {
    const rust = source("../../crates/chizu-core/src/clock.rs").match(
      /pub const TICKS_PER_SECOND_AT_NORMAL_SPEED: u32 = (\d+);/,
    );
    expect(Number(rust?.[1])).toBe(TICKS_PER_SECOND);
  });

  it("いちばん速い段は chizu-core の SimSpeed::MAX と同じ", () => {
    const rust = source("../../crates/chizu-core/src/command.rs").match(
      /pub const MAX: SimSpeed = SimSpeed\((\d+)\);/,
    );
    expect(Math.max(...BAR_SPEEDS, ...EXTRA_SPEEDS)).toBe(Number(rust?.[1]));
  });

  it("index.html のボタンは段の並びと同じ", () => {
    const html = source("../index.html");
    const speedsIn = (id: string) => {
      const start = html.indexOf(`id="${id}"`);
      const end = html.indexOf("</div>", start);
      return [...html.slice(start, end).matchAll(/data-speed="([\d.]+)"[^>]*>([^<]*)</g)].map(
        (m) => [Number(m[1]), m[2]],
      );
    };
    expect(speedsIn("speeds")).toEqual(BAR_SPEEDS.map((s) => [s, `${s} 倍`]));
    expect(speedsIn("controls")).toEqual(EXTRA_SPEEDS.map((s) => [s, `${s} 倍`]));
  });
});

describe("速さの段", () => {
  it("1 日を 24 分、4 分、30 秒、5 秒で回す", () => {
    const daySeconds = [...BAR_SPEEDS, ...EXTRA_SPEEDS].map(
      (s) => TICKS_PER_DAY / tickRate(TICKS_PER_SECOND, s),
    );
    expect(daySeconds).toEqual([24 * 60, 4 * 60, 30, 5]);
  });
});

describe("stepBudget", () => {
  it("1 倍では 1 秒に 5 tick、12 フレームに 1 回だけ進む", () => {
    const { total, perFrame } = runOneSecond(tickRate(TICKS_PER_SECOND, 1));
    expect(total).toBe(5);
    expect(Math.max(...perFrame)).toBe(1);
    expect(perFrame.slice(0, 24).filter((t) => t > 0)).toHaveLength(2);
  });

  it("288 倍では 1 秒に 1440 tick 進む", () => {
    expect(runOneSecond(tickRate(TICKS_PER_SECOND, 288)).total).toBe(1440);
  });

  it("止めているあいだは進まず、端数も捨てる", () => {
    const step = stepBudget(0.9, 1000, TICKS_PER_SECOND, true, 0.25);
    expect(step).toEqual({ ticks: 0, budget: 0 });
  });

  it("遅れが上限を超えたぶんは捨てる", () => {
    const step = stepBudget(0, 10_000, tickRate(TICKS_PER_SECOND, 6), false, 0.25);
    expect(step.ticks).toBe(7);
  });
});

describe("HUD の速度", () => {
  it("倍率をそのまま出す", () => {
    expect(describeSpeed(1)).toBe("速度 1 倍");
    expect(describeSpeed(288)).toBe("速度 288 倍");
  });
});
