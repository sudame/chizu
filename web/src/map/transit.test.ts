import { describe, expect, test } from "vitest";
import type { RailLineInfo } from "./lines.ts";
import type { RailStation } from "./rail.ts";
import { STATION_WALK_RADIUS } from "./tools.ts";
import {
  householdsText,
  LOAD_HALF_MAX,
  LOAD_HALF_MIN,
  loadHalf,
  loadPx,
  parseRailStats,
  RAIL_STATS_HEADER,
  RAIL_STATS_LINE,
  RAIL_STATS_STATION,
  type RailStats,
  RideDays,
  type StationStat,
  stationCrowded,
  stationRadius,
  transitMeshes,
  transitTexts,
  WAITING_ALERT,
} from "./transit.ts";

// 駅 1 つ：添え字、世代、min の x、y、出入口の x、y、出入口があるか、世帯、車を持てない世帯、待つ人、乗った人、降りた人、残ったか
const station = (over: Partial<Record<number, number>> = {}): number[] => {
  const base = [3, 0, 10, 20, 12, 22, 1, 40, 6, 5, 100, 80, 0];
  return base.map((v, i) => over[i] ?? v);
};
const line = (over: Partial<Record<number, number>> = {}): number[] => {
  const base = [0, 0, 450, 300, 12, 1, 80, 2];
  return base.map((v, i) => over[i] ?? v);
};
const data = (stations: number[][], lines: number[][], gaveUp = 7, evicted = 2) => [
  stations.length,
  lines.length,
  gaveUp,
  evicted,
  ...stations.flat(),
  ...lines.flat(),
];

describe("rail_stats を読む", () => {
  test("駅と路線の数を、見出しと並びの個数どおりに読む", () => {
    expect(RAIL_STATS_HEADER).toBe(4);
    const stats = parseRailStats(500, data([station(), station({ 0: 4, 9: 150 })], [line()]));
    expect(stats.tick).toBe(500);
    expect([stats.gaveUp, stats.evicted]).toEqual([7, 2]);
    expect(stats.stations).toHaveLength(2);
    expect(stats.stations[0]).toMatchObject({
      index: 3,
      min: { x: 10, y: 20 },
      door: { x: 12, y: 22 },
      hasDoor: true,
      households: 40,
      carless: 6,
      waiting: 5,
      boarded: 100,
      alighted: 80,
      leftBehind: false,
    });
    expect(stats.stations[1]?.waiting).toBe(150);
    expect(stats.lines).toEqual([
      {
        index: 0,
        generation: 0,
        loadPermille: 450,
        boarded: 300,
        waiting: 12,
        leftBehindStops: 1,
        aboard: 80,
        running: 2,
      },
    ]);
  });

  test("出入口のない駅は、出入口を敷地の min にして hasDoor を偽にする", () => {
    const stats = parseRailStats(0, data([station({ 6: 0, 4: 0, 5: 0 })], []));
    expect(stats.stations[0]?.hasDoor).toBe(false);
    expect(stats.stations[0]?.door).toEqual({ x: 10, y: 20 });
  });

  test("途中で切れた数は、読めたところまでにする", () => {
    const full = data([station(), station()], [line()]);
    expect(
      parseRailStats(0, full.slice(0, RAIL_STATS_HEADER + RAIL_STATS_STATION + 3)),
    ).toMatchObject({ stations: [{ index: 3 }], lines: [] });
    expect(parseRailStats(0, full.slice(0, full.length - 1)).lines).toHaveLength(0);
    expect(RAIL_STATS_LINE).toBe(8);
    expect(parseRailStats(0, []).stations).toEqual([]);
  });
});

describe("その日の乗り降り", () => {
  const at = (tick: number, boarded: number) =>
    parseRailStats(tick, data([station({ 10: boarded, 11: boarded })], []));
  const first = (stats: RailStats): StationStat => {
    const st = stats.stations[0];
    if (st === undefined) {
      throw new Error("駅がない");
    }
    return st;
  };

  test("開いたときは、開いてからの数を出し、日が変わると日の数になる", () => {
    const days = new RideDays();
    days.update(at(1000, 100));
    expect(days.today(first(at(1000, 100)))).toEqual({ boarded: 0, alighted: 0 });
    expect(days.fullDay).toBe(false);
    days.update(at(1500, 160));
    expect(days.today(first(at(1500, 160)))).toEqual({ boarded: 60, alighted: 60 });
    // 次の日の最初の値が控えになる
    days.update(at(7300, 400));
    expect(days.fullDay).toBe(true);
    expect(days.today(first(at(7300, 400)))).toEqual({ boarded: 0, alighted: 0 });
    days.update(at(9000, 450));
    expect(days.today(first(at(9000, 450)))).toEqual({ boarded: 50, alighted: 50 });
  });

  test("tick が戻ったら控えを捨てる", () => {
    const days = new RideDays();
    days.update(at(9000, 500));
    days.update(at(100, 5));
    expect(days.fullDay).toBe(false);
    expect(days.today(first(at(100, 5)))).toEqual({ boarded: 0, alighted: 0 });
  });

  test("reset すると、前の街の控えを引きずらない", () => {
    const days = new RideDays();
    days.update(at(1000, 100));
    // 別の街（tick は大きく、積み上げも多い）を開く
    days.reset();
    days.update(at(1500, 900));
    expect(days.today(first(at(1500, 900)))).toEqual({ boarded: 0, alighted: 0 });
    expect(days.fullDay).toBe(false);
  });

  test("日の変わり目の積み上げを起点にすると、日の頭の数が今日に入る", () => {
    const days = new RideDays();
    days.update(at(7000, 100));
    // 日が変わった tick（7200）の値。駅 3.0 は乗った 120、降りた 90
    days.markDay(7200, [0, 3, 0, 120, 90]);
    expect(days.fullDay).toBe(true);
    // 約 1 秒あと（tick 7300）に届いた rail_stats でも、日の頭からの数になる
    days.update(at(7300, 150));
    expect(days.today(first(at(7300, 150)))).toEqual({ boarded: 30, alighted: 60 });
  });

  test("控えのない駅は、積み上げの数をそのまま今日の数にする", () => {
    const days = new RideDays();
    days.update(parseRailStats(1000, data([], [])));
    expect(days.today(first(at(1000, 30)))).toEqual({ boarded: 30, alighted: 30 });
  });
});

describe("路線の太さ", () => {
  test("乗車率に応じて太くなり、100% を超えても上限で止まる", () => {
    expect(loadHalf(0)).toBe(LOAD_HALF_MIN);
    expect(loadHalf(1000)).toBe(LOAD_HALF_MAX);
    expect(loadHalf(2000)).toBe(LOAD_HALF_MAX);
    expect(loadHalf(500)).toBeGreaterThan(loadHalf(250));
    expect(loadPx(900)).toBeGreaterThan(loadPx(100));
  });

  test("駅の丸は乗り降りが多いほど大きく、上限で止まる", () => {
    expect(stationRadius(0)).toBeLessThan(stationRadius(100));
    expect(stationRadius(1_000_000)).toBe(5);
  });

  test("乗りきれない人が残ったか、待つ人が多い駅に印を付ける", () => {
    const [ok, left, many] = parseRailStats(
      0,
      data([station(), station({ 12: 1 }), station({ 9: WAITING_ALERT })], []),
    ).stations;
    expect([ok, left, many].map((s) => s !== undefined && stationCrowded(s))).toEqual([
      false,
      true,
      true,
    ]);
  });
});

describe("メッシュ", () => {
  const info = (over: Partial<RailLineInfo> = {}): RailLineInfo => ({
    index: 0,
    generation: 0,
    looped: false,
    trains: 2,
    cap: 4,
    allowed: 2,
    stopped: false,
    headway: 300,
    roundTrip: 600,
    stations: [],
    tracks: [{ a: { x: 12, y: 22 }, b: { x: 112, y: 22 } }],
    waitPlaces: 3,
    sharedWith: [],
    ...over,
  });
  const sites: RailStation[] = [
    { min: { x: 10, y: 20 }, max: { x: 19, y: 22 }, toward: { x: 0, y: 1 } },
  ];
  const rides = () => ({ boarded: 10, alighted: 10 });

  test("円は出入口から描き、出入口のない駅には描かない", () => {
    const stats = parseRailStats(0, data([station()], [line()]));
    const m = transitMeshes([info()], sites, stats, rides);
    // 円は多角形の三角形と縁の線で、頂点がある
    expect(m.circles.vertexCount).toBeGreaterThan(0);
    const none = parseRailStats(0, data([station({ 6: 0 })], [line()]));
    expect(transitMeshes([info()], sites, none, rides).circles.vertexCount).toBe(0);
    expect(STATION_WALK_RADIUS).toBe(64);
  });

  test("混んだ路線は、空いた路線と同じ線路でも頂点の幅が変わるが、頂点の数は変わらない", () => {
    const quiet = parseRailStats(0, data([station()], [line({ 2: 0 })]));
    const busy = parseRailStats(0, data([station()], [line({ 2: 1000 })]));
    const a = transitMeshes([info()], sites, quiet, rides).lines;
    const b = transitMeshes([info()], sites, busy, rides).lines;
    expect(a.vertexCount).toBe(b.vertexCount);
    // 半分の太さは 5 番めの数（頂点 1 つ 13 個の数のうち）
    expect(b.build()[4]).toBeGreaterThan(a.build()[4] ?? 0);
  });

  test("乗りきれない人がいる駅は、赤の輪を重ねるぶん頂点が増える", () => {
    const plain = parseRailStats(0, data([station()], []));
    const crowded = parseRailStats(0, data([station({ 12: 1 })], []));
    const a = transitMeshes([], sites, plain, rides).stations.vertexCount;
    const b = transitMeshes([], sites, crowded, rides).stations.vertexCount;
    expect(b).toBeGreaterThan(a);
  });
});

describe("世帯の数の文", () => {
  test("車を持てない世帯がいれば添える", () => {
    expect(householdsText({ households: 1200, carless: 0 })).toBe("1,200 世帯");
    expect(householdsText({ households: 1200, carless: 35 })).toBe("1,200 世帯（車なし 35）");
  });

  test("出入口のある駅の敷地の真ん中に文字を置く", () => {
    const sites: RailStation[] = [
      { min: { x: 10, y: 20 }, max: { x: 19, y: 22 }, toward: { x: 0, y: 1 } },
      { min: { x: 50, y: 20 }, max: { x: 59, y: 22 }, toward: { x: 0, y: 1 } },
    ];
    const stats = parseRailStats(0, data([station(), station({ 2: 50, 6: 0 })], []));
    expect(transitTexts(sites, stats)).toEqual([
      { x: 15, y: 21.5, text: "40 世帯（車なし 6）", short: "40 世帯" },
    ]);
    expect(transitTexts(sites, null)).toEqual([]);
  });
});
