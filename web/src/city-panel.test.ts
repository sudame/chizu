import { describe, expect, test } from "vitest";
import {
  BAR_HALF_PX,
  DEMAND_FULL_SCALE,
  demandBar,
  demandBars,
  demandFactorLines,
  formatCityNumbers,
  formatUtilities,
  parseCityStats,
  shownZone,
  togglePinned,
  utilityLowHeadroom,
  utilityNotice,
  utilityShortage,
} from "./city-panel.ts";

describe("街の数字", () => {
  test("city_stats の並びを読む", () => {
    expect(
      parseCityStats([3, 40, 12, 20, -5, 0, 7, 400, 300, 25, 1, 0, 2, 0, 9, 0, 200, 150, 25, 1, 0]),
    ).toEqual({
      buildings: 3,
      households: 40,
      workplaces: 12,
      demand: [20, -5, 0, 7],
      power: 400,
      water: 300,
      utilityDemand: 25,
      powerShort: 1,
      waterShort: 0,
      powerIsolated: 2,
      waterIsolated: 0,
      powerGap: 9,
      waterGap: 0,
      mainPower: 200,
      mainWater: 150,
      mainDemand: 25,
      powerOthers: 1,
      waterOthers: 0,
      demandFactors: [[], [], [], []],
    });
    expect(formatUtilities(parseCityStats([0, 0, 0, 0, 0, 0, 0, 400, 0, 1200, 1, 2]))).toBe(
      "電気 400 / 需要 1,200・水道 0 / 需要 1,200",
    );
    expect(formatUtilities(parseCityStats([0, 0, 0, 0, 0, 0, 0, 400, 300, 10, 0, 0]))).toBe(
      "電気 400 / 需要 10・水道 300 / 需要 10",
    );
  });

  test("余りは主の網で見て、ほかの網にある施設の分は足さない", () => {
    // 合計の電気は 7,200 だが、主の網は 3,600 で需要 3,500 しかない。つながっていない発電所が 1 基ある。
    const stats = parseCityStats([
      5, 0, 0, 0, 0, 0, 0, 7200, 3000, 3500, 1, 0, 0, 0, 0, 0, 3600, 3000, 3500, 1, 0,
    ]);
    expect(formatUtilities(stats)).toBe(
      "電気 3,600 / 需要 3,500（ほかの網 1）・水道 3,000 / 需要 3,500",
    );
    expect(utilityLowHeadroom(stats)?.reasons).toEqual([
      "電気の余りが 2% になった。発電所を足す用意をする",
    ]);
  });

  test("届いていない建物がなければ知らせない", () => {
    expect(utilityShortage(parseCityStats([]), 0)).toBeNull();
  });

  test("施設が足りないときは足りない量と足すものを、網につながっていないときはその数を出す", () => {
    const stats = parseCityStats([5, 0, 0, 0, 0, 0, 0, 0, 0, 3000, 1, 1, 0, 1, 1200, 0]);
    expect(utilityShortage(stats, 1234)).toEqual({
      headline: "電気か水道が届いていない建物が 1,234 棟ある",
      reasons: [
        "電気が 1,200 足りない。発電所を足す",
        "浄水場につながっていない道路網が 1 つある。浄水場を置くか、道路でつなぐ",
      ],
    });
  });

  test("理由がないうちは、数が正でも知らせない", () => {
    expect(utilityShortage(parseCityStats([]), 3)).toBeNull();
  });

  test("余りが 10% を切ったら、足りなくなる前に知らせる", () => {
    const stats = (power: number, water: number, demand: number) =>
      parseCityStats([5, 0, 0, 0, 0, 0, 0, power, water, demand]);
    expect(utilityLowHeadroom(stats(3600, 3000, 2000))).toBeNull();
    expect(utilityLowHeadroom(stats(3600, 3000, 0))).toBeNull();
    expect(utilityLowHeadroom(stats(0, 0, 100))).toBeNull();
    expect(utilityLowHeadroom(stats(3600, 3000, 3000))).toEqual({
      headline: "電気か水道の余りが少なくなっている",
      reasons: ["水道の余りが 0% になった。浄水場を足す用意をする"],
    });
    expect(utilityLowHeadroom(stats(3600, 3600, 3300))?.reasons).toHaveLength(2);
    // 需要が供給量を超えたら、これは出さずに足りない知らせに任せる。
    expect(utilityLowHeadroom(stats(3600, 3000, 3100))).toBeNull();
  });

  test("電気が足りなくても、水道の余りが少ない知らせは隠れない", () => {
    // 電気は 3,000 に対して需要 3,100 で足りず、水道は 3,300 に対して余り 6% である。
    const stats = parseCityStats([5, 0, 0, 0, 0, 0, 0, 3000, 3300, 3100, 1, 0, 0, 0, 100, 0]);
    const shortage = utilityShortage(stats, 4);
    expect(shortage).not.toBeNull();
    expect(utilityNotice(stats, shortage)?.reasons).toEqual([
      ...(shortage?.reasons ?? []),
      "水道の余りが 6% になった。浄水場を足す用意をする",
    ]);
  });

  test("棒は住宅・商業・工業・オフィスの順に並び、正は上、負は下へ伸びる", () => {
    const stats = parseCityStats([0, 0, 0, DEMAND_FULL_SCALE / 2, -DEMAND_FULL_SCALE, 0, 999]);
    expect(demandBars(stats)).toEqual([
      { value: DEMAND_FULL_SCALE / 2, top: BAR_HALF_PX / 2, height: BAR_HALF_PX / 2 },
      { value: -DEMAND_FULL_SCALE, top: BAR_HALF_PX, height: BAR_HALF_PX },
      { value: 0, top: BAR_HALF_PX, height: 0 },
      { value: 999, top: 0, height: BAR_HALF_PX },
    ]);
  });

  test("建物がなくなると 0 に戻り、上限だと分かる言葉で出す", () => {
    expect(formatCityNumbers(parseCityStats([12, 1_200, 30, 0, 0, 0, 0]))).toBe(
      "建物 12 棟・住める世帯 1,200・職場の枠 30",
    );
    expect(formatCityNumbers(parseCityStats([0, 0, 0, 20, 0, 10, 0]))).toBe(
      "建物 0 棟・住める世帯 0・職場の枠 0",
    );
  });

  test("需要の棒は -1 から 1 に収める", () => {
    expect(demandBar(DEMAND_FULL_SCALE / 2)).toBe(0.5);
    expect(demandBar(-DEMAND_FULL_SCALE / 4)).toBe(-0.25);
    expect(demandBar(DEMAND_FULL_SCALE * 10)).toBe(1);
    expect(demandBar(-DEMAND_FULL_SCALE * 10)).toBe(-1);
  });
});

describe("需要の内訳", () => {
  // 住宅 2 件、商業 0 件、工業 0 件、オフィス 3 件の内訳。件数のあとに [kind, value] が続く。
  const tail = [2, 0, 20, 4, -30, 0, 0, 3, 7, 140, 3, -9, 8, -200];
  const stats = parseCityStats([...new Array(21).fill(0), ...tail]);

  test("city_stats の末尾から区画の種類ごとの内訳を読む", () => {
    expect(stats.demandFactors).toEqual([
      [
        { kind: 0, value: 20 },
        { kind: 4, value: -30 },
      ],
      [],
      [],
      [
        { kind: 7, value: 140 },
        { kind: 3, value: -9 },
        { kind: 8, value: -200 },
      ],
    ]);
    // 内訳が付いていない並びでも読める。
    expect(parseCityStats(new Array(21).fill(0)).demandFactors).toEqual([[], [], [], []]);
  });

  test("寄与の大きい順に並べ、正と負が分かる言葉で出す", () => {
    expect(demandFactorLines(stats, 3)).toEqual([
      { label: "オフィスの職場がすでに多い", value: -200 },
      { label: "働く年齢の住民に職場が要る", value: 140 },
      { label: "税が高く、職場を増やしにくい", value: -9 },
    ]);
    expect(demandFactorLines(stats, 0).map((l) => l.label)).toEqual(["空き家が多い", "基本の需要"]);
  });

  test("寄与が 0 の要因は出さず、同じ大きさなら正が先に来る", () => {
    const s = parseCityStats([...new Array(21).fill(0), 2, 1, 0, 3, 0, 0, 0, 0]);
    expect(demandFactorLines(s, 0)).toEqual([]);
    const tie = parseCityStats([...new Array(21).fill(0), 2, 3, -5, 1, 5, 0, 0, 0]);
    expect(demandFactorLines(tie, 0).map((l) => l.value)).toEqual([5, -5]);
  });

  test("幸福度と税は、符号で言い方が変わる", () => {
    const s = parseCityStats([...new Array(21).fill(0), 2, 2, 10, 3, 12, 0, 0, 0, 0, 0, 0, 0]);
    expect(demandFactorLines(s, 0).map((l) => l.label)).toEqual(["税が低い", "幸福度が高い"]);
    const bad = parseCityStats([...new Array(21).fill(0), 2, 2, -10, 3, -12, 0, 0, 0, 0, 0, 0, 0]);
    expect(demandFactorLines(bad, 0).map((l) => l.label)).toEqual(["税が高い", "幸福度が低い"]);
  });

  test("押すと固定され、もう一度押すと隠れる。指を置いているあいだは固定より優先して出る", () => {
    let view = { pinned: null as number | null, hover: null as number | null };
    expect(shownZone(view)).toBeNull();
    view = togglePinned(view, 3);
    expect(shownZone(view)).toBe(3);
    view = { ...view, hover: 1 };
    expect(shownZone(view)).toBe(1);
    view = { ...view, hover: null };
    expect(shownZone(view)).toBe(3);
    view = togglePinned(view, 2);
    expect(shownZone(view)).toBe(2);
    view = togglePinned(view, 2);
    expect(shownZone(view)).toBeNull();
  });
});
