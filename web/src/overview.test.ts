import { describe, expect, test } from "vitest";
import {
  formatHappiness,
  formatNextMilestone,
  formatPopulation,
  formatRemaining,
  happinessParts,
  MilestoneTracker,
  parseOverview,
} from "./overview.ts";

// 区画ごとの数（4 種類 × 3）。駐車場の点数はそのあとにある
const blocks = Array<number>(12).fill(0);

// 人口、幸福度、通勤、生活サービス、暮らし向き、段階、次の人口、いまの枠、次の枠、区画ごとの数、駐車場
const at = (population: number, milestone: number, slots: number) =>
  parseOverview([population, 64, 80, 90, 41, milestone, 600, slots, slots + 4, ...blocks, 70]);

describe("overview", () => {
  test("overview の並びを読む", () => {
    expect(parseOverview([300, 64, 80, 90, 41, 1, 600, 9, 13, ...blocks, 70])).toEqual({
      population: 300,
      happiness: 64,
      commute: 80,
      services: 90,
      parking: 70,
      wealth: 41,
      milestone: 1,
      nextPopulation: 600,
      slots: 9,
      nextSlots: 13,
    });
  });

  test("人口と幸福度を桁区切りで整形する", () => {
    const o = parseOverview([12_345, 64]);
    expect(formatPopulation(o)).toBe("人口 12,345");
    expect(formatHappiness(o)).toBe("幸福度 64 / 100");
  });

  test("次の段階までの残りを出す", () => {
    expect(formatRemaining(parseOverview([100, 50, 0, 0, 0, 0, 250, 6, 9]))).toBe(
      "次の段階まであと 150 人",
    );
  });

  test("残りが負にならない", () => {
    expect(formatRemaining(parseOverview([300, 50, 0, 0, 0, 0, 250, 6, 9]))).toBe(
      "次の段階まであと 0 人",
    );
  });

  test("次の段階で増える枠を出す", () => {
    expect(formatNextMilestone(parseOverview([100, 50, 0, 0, 0, 0, 250, 6, 9]))).toBe(
      "250 人に届くと、買える土地の枠が 3 増える",
    );
  });

  test("幸福度の内訳は通勤・生活サービス・駐車場・暮らし向きの順", () => {
    expect(happinessParts(at(0, 0, 6)).map((p) => p.text)).toEqual([
      "通勤 80 / 100",
      "生活サービス 90 / 100",
      "駐車場 70 / 100",
      "暮らし向き 41 / 100",
    ]);
  });
});

describe("MilestoneTracker", () => {
  test("最初に見た値は、すでに届いた段階でも知らせない", () => {
    expect(new MilestoneTracker().observe(at(700, 2, 13))).toBeNull();
  });

  test("段階が上がったら届いたことと増えた枠を知らせる", () => {
    const t = new MilestoneTracker();
    expect(t.observe(at(240, 0, 6))).toBeNull();
    expect(t.observe(at(240, 0, 6))).toBeNull();
    expect(t.observe(at(251, 1, 9))).toEqual({
      headline: "人口が段階 1 に届いた",
      detail: "買える土地の枠が 3 増えて 9 になった",
    });
    // 同じ段階の間は知らせない
    expect(t.observe(at(260, 1, 9))).toBeNull();
  });

  test("一度に何段階も進んだら、増えた枠をまとめて知らせる", () => {
    const t = new MilestoneTracker();
    t.observe(at(10, 0, 6));
    expect(t.observe(at(1_300, 3, 18))?.detail).toBe("買える土地の枠が 12 増えて 18 になった");
  });

  test("人口が減っても段階は戻らないので知らせない", () => {
    const t = new MilestoneTracker();
    t.observe(at(300, 1, 9));
    expect(t.observe(at(100, 1, 9))).toBeNull();
  });
});
