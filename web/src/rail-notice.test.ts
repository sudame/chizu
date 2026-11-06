import { describe, expect, test } from "vitest";
import type { RailLineInfo } from "./map/lines.ts";
import type { LineStat, RailStats, StationStat } from "./map/transit.ts";
import {
  busiestStations,
  CROWDED_PERMILLE,
  GAVE_UP_MIN_SPAN,
  GAVE_UP_PER_DAY,
  GaveUpWindow,
  isCrowded,
  OVERLAP_PERMILLE,
  RecentRides,
  railNotices,
  UNUSED_DAYS,
} from "./rail-notice.ts";
import { TICKS_PER_DAY } from "./utility-trend.ts";

const line = (over: Partial<RailLineInfo> = {}): RailLineInfo => ({
  index: 0,
  generation: 0,
  looped: false,
  trains: 2,
  cap: 4,
  allowed: 2,
  stopped: false,
  headway: 300,
  roundTrip: 600,
  stations: [
    { x: 10, y: 20 },
    { x: 50, y: 20 },
  ],
  tracks: [],
  waitPlaces: 3,
  sharedWith: [],
  ...over,
});
const lineStat = (over: Partial<LineStat> = {}): LineStat => ({
  index: 0,
  generation: 0,
  loadPermille: 300,
  boarded: 100,
  waiting: 0,
  leftBehindStops: 0,
  aboard: 100,
  running: 2,
  ...over,
});
const stationStat = (over: Partial<StationStat> = {}): StationStat => ({
  index: 0,
  generation: 0,
  min: { x: 10, y: 20 },
  door: { x: 12, y: 22 },
  hasDoor: true,
  households: 100,
  carless: 10,
  waiting: 0,
  boarded: 0,
  alighted: 0,
  leftBehind: false,
  shared: 0,
  ...over,
});
const stats = (lines: LineStat[], stations: StationStat[] = []): RailStats => ({
  tick: 1000,
  gaveUp: 0,
  evicted: 0,
  stations,
  lines,
});
const name = (t: { x: number }) => `${t.x} 番の駅`;

describe("鉄道の知らせ", () => {
  test("問題のない鉄道では何も出さない", () => {
    expect(railNotices([line()], stats([lineStat()]), 0, name)).toEqual([]);
    expect(railNotices([], null, null, name)).toEqual([]);
  });

  test("上限が 0 の路線は、理由と一緒に走れない知らせにする", () => {
    const halted = line({ cap: 0, allowed: 0, waitPlaces: 1 });
    const [n] = railNotices([halted], stats([lineStat()]), null, name);
    expect(n?.id).toBe("rail-halted");
    expect(n?.headline).toContain("青線");
    expect(n?.details[0]).toContain("待てる所が足りない");
    expect(n?.jump?.targets).toEqual([{ x: 10, y: 20 }]);
  });

  test("止まっている路線は、走れない知らせとは別に出す", () => {
    const stopped = line({ stopped: true, cap: 0 });
    const ids = railNotices([stopped], stats([lineStat()]), null, name).map((n) => n.id);
    expect(ids).toEqual(["rail-stopped"]);
  });

  test("いちばん混んだ列車が目安を超えるか、乗りきれなかった路線は混みすぎとして出す", () => {
    const l = line();
    expect(isCrowded(l, stats([lineStat({ loadPermille: CROWDED_PERMILLE - 1 })]))).toBe(false);
    expect(isCrowded(l, stats([lineStat({ loadPermille: CROWDED_PERMILLE })]))).toBe(true);
    expect(isCrowded(l, stats([lineStat({ leftBehindStops: 1 })]))).toBe(true);
    // 走る列車がなければ、混みすぎとは言わない
    expect(isCrowded(l, stats([lineStat({ loadPermille: 1000, running: 0 })]))).toBe(false);
    expect(isCrowded(l, null)).toBe(false);
  });

  test("混みすぎの知らせは、本数を増やせるかで勧める手を変え、乗車率が揺れても key は変わらない", () => {
    const room = railNotices([line()], stats([lineStat({ loadPermille: 950 })]), null, name)[0];
    expect(room?.details[0]).toContain("95%");
    expect(room?.details[0]).toContain("本数を増やすと解ける");
    const full = railNotices(
      [line({ trains: 4 })],
      stats([lineStat({ loadPermille: 990 })]),
      null,
      name,
    )[0];
    expect(full?.details[0]).toContain("上限に達している");
    const again = railNotices([line()], stats([lineStat({ loadPermille: 1000 })]), null, name)[0];
    expect(again?.key).toBe(room?.key);
  });

  test("歩いた人が目安に届くと、待っている人の多い駅を並べて出す", () => {
    const st = stats(
      [lineStat()],
      [
        stationStat({ index: 0, min: { x: 10, y: 20 }, waiting: 5 }),
        stationStat({ index: 1, min: { x: 50, y: 20 }, waiting: 80 }),
        stationStat({ index: 2, min: { x: 90, y: 20 }, waiting: 0 }),
      ],
    );
    expect(railNotices([line()], st, GAVE_UP_PER_DAY - 1, name)).toEqual([]);
    expect(railNotices([line()], st, null, name)).toEqual([]);
    const [n] = railNotices([line()], st, GAVE_UP_PER_DAY, name);
    expect(n?.id).toBe("rail-gave-up");
    expect(n?.details[1]).toBe("50 番の駅：80 人が待っている");
    expect(n?.details[2]).toBe("10 番の駅：5 人が待っている");
    expect(n?.details).toHaveLength(3);
    expect(n?.jump?.ordered).toBe(true);
    expect(n?.jump?.targets[0]).toEqual({ x: 50, y: 20 });
  });

  test("待っている人がいる駅がなければ、移る先を出さない", () => {
    const [n] = railNotices([line()], stats([lineStat()], [stationStat()]), 100, name);
    expect(n?.jump).toBeNull();
  });

  test("待っている人の多い順に並べ、同じ数なら ID の順にする", () => {
    const s = stats(
      [],
      [
        stationStat({ index: 3, waiting: 10 }),
        stationStat({ index: 1, waiting: 10 }),
        stationStat({ index: 2, waiting: 30 }),
        stationStat({ index: 4, waiting: 1 }),
        stationStat({ index: 5, waiting: 1 }),
      ],
    );
    expect(busiestStations(s).map((x) => x.index)).toEqual([2, 1, 3]);
  });
});

describe("1 日あたりの歩いた人の数", () => {
  test("見ている長さが足りないうちは null", () => {
    const w = new GaveUpWindow();
    w.update(0, 0);
    w.update(GAVE_UP_MIN_SPAN - 1, 50);
    expect(w.perDay()).toBeNull();
  });

  test("直近 1 日の差を、1 日あたりに直す", () => {
    const w = new GaveUpWindow();
    w.update(0, 0);
    w.update(TICKS_PER_DAY / 2, 30);
    expect(w.perDay()).toBeCloseTo(60);
    w.update(TICKS_PER_DAY, 60);
    expect(w.perDay()).toBeCloseTo(60);
    // 1 日より前の標本は捨て、古い増え方は引きずらない
    w.update(TICKS_PER_DAY * 1.5, 70);
    expect(w.perDay()).toBeCloseTo(40);
    w.update(TICKS_PER_DAY * 2, 80);
    expect(w.perDay()).toBeCloseTo(20);
  });

  test("tick が戻ったら（読み込み、新しい街）数え直す", () => {
    const w = new GaveUpWindow();
    w.update(0, 0);
    w.update(TICKS_PER_DAY, 500);
    w.update(100, 0);
    expect(w.perDay()).toBeNull();
  });

  test("reset すると、tick も積み上げも前の街より大きい街の値と混ざらない", () => {
    const w = new GaveUpWindow();
    w.update(1000, 0);
    w.update(1000 + GAVE_UP_MIN_SPAN, 0);
    // 別の街（tick 1500、歩いた人 500）を開く
    w.reset();
    w.update(1500, 500);
    expect(w.perDay()).toBeNull();
    w.update(1500 + GAVE_UP_MIN_SPAN, 500);
    expect(w.perDay()).toBeCloseTo(0);
  });
});

describe("直近の乗った人の数と、使われない駅の知らせ", () => {
  const at = (tick: number, stations: StationStat[]): RailStats => ({
    ...stats([lineStat()], stations),
    tick,
  });
  const DAYS = UNUSED_DAYS * TICKS_PER_DAY;

  test("日数たつまでは判断せず、たったあとは窓の差を返す", () => {
    const r = new RecentRides();
    const s = (boarded: number) => stationStat({ boarded });
    r.update(at(0, [s(10)]));
    r.update(at(DAYS - 1, [s(10)]));
    expect(r.boardedIn(s(0))).toBeNull();
    r.update(at(DAYS, [s(10)]));
    expect(r.boardedIn(s(0))).toBe(0);
    r.update(at(DAYS + TICKS_PER_DAY, [s(25)]));
    expect(r.boardedIn(s(0))).toBe(15);
  });

  test("あとから足された駅は、足されてから日数たつまで判断しない", () => {
    const r = new RecentRides();
    r.update(at(0, [stationStat({ index: 0 })]));
    r.update(at(DAYS, [stationStat({ index: 0 }), stationStat({ index: 1 })]));
    expect(r.boardedIn({ index: 0, generation: 0 })).toBe(0);
    expect(r.boardedIn({ index: 1, generation: 0 })).toBeNull();
  });

  test("reset や tick の巻き戻しで捨てる", () => {
    const r = new RecentRides();
    r.update(at(0, [stationStat()]));
    r.update(at(DAYS, [stationStat()]));
    r.update(at(10, [stationStat()]));
    expect(r.boardedIn({ index: 0, generation: 0 })).toBeNull();
    r.update(at(DAYS + 10, [stationStat()]));
    r.reset();
    expect(r.boardedIn({ index: 0, generation: 0 })).toBeNull();
  });

  const stub = (rides: Record<number, number | null>) => ({
    boardedIn: (s: { index: number }) => rides[s.index] ?? null,
  });

  test("乗る人が 0 の駅を知らせ、原因の手がかりを添える", () => {
    const st = stats(
      [lineStat()],
      [
        stationStat({ index: 0, min: { x: 10, y: 20 }, households: 700, shared: 690 }),
        stationStat({ index: 1, min: { x: 50, y: 20 }, households: 700, shared: 690 }),
        stationStat({ index: 2, min: { x: 90, y: 20 }, households: 0 }),
      ],
    );
    const lines = [
      line({
        stations: [
          { x: 10, y: 20 },
          { x: 50, y: 20 },
        ],
      }),
    ];
    const [n] = railNotices(lines, st, null, name, stub({ 0: 40, 1: 0, 2: 0 }));
    expect(n?.id).toBe("rail-unused");
    expect(n?.details[0]).toContain("50 番の駅：直近 3 日、乗る人が 0 人");
    expect(n?.details[0]).toContain("ほぼ重なり");
    expect(n?.details[1]).toContain("どの路線にも入っていない");
    expect(n?.jump?.targets).toEqual([
      { x: 50, y: 20 },
      { x: 90, y: 20 },
    ]);
    expect(railNotices(lines, st, null, name, stub({ 0: 40, 1: 5, 2: null }))).not.toContainEqual(
      expect.objectContaining({ id: "rail-unused" }),
    );
  });

  test("隣の駅と大きく重なる駅は、使われていないほうだけを知らせる", () => {
    const heavy = Math.ceil((OVERLAP_PERMILLE * 700) / 1000);
    const mk = (index: number, x: number, boarded: number) =>
      stationStat({
        index,
        min: { x, y: 20 },
        door: { x, y: 22 },
        households: 700,
        shared: heavy,
        boarded,
      });
    const st = stats([lineStat()], [mk(0, 10, 500), mk(1, 60, 30)]);
    const [n] = railNotices([line()], st, null, name, stub({}));
    expect(n?.id).toBe("rail-overlap");
    expect(n?.details).toHaveLength(1);
    expect(n?.details[0]).toContain("60 番の駅");
    expect(n?.details[0]).toContain("10 番の駅 と重なる");
    // 遠くて徒歩圏が重ならない、重なりの割合が低い駅は出さない
    const far = stats([lineStat()], [mk(0, 10, 500), { ...mk(1, 400, 30) }]);
    expect(railNotices([line()], far, null, name, stub({}))).toEqual([]);
    const low = stats([lineStat()], [mk(0, 10, 500), { ...mk(1, 60, 30), shared: 100 }]);
    expect(railNotices([line()], low, null, name, stub({}))).toEqual([]);
  });
});
