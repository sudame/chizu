import { describe, expect, test } from "vitest";
import {
  carlessNotice,
  growthNotice,
  growthNotices,
  isolatedNotice,
  LOT_SHORTAGE_TICKS,
  lotNotice,
  parkingNotice,
  parseCarless,
  parseGrowthBlocks,
  parseIsolated,
  parseParkingShortage,
  parseStalls,
  persistedLotShortage,
  pickNext,
  pickStall,
  trackLotShortage,
} from "./growth-notice.ts";
import { dismiss, emptyMemory, reconcile } from "./notice-center.ts";

/** 区画ごとの [止まっている数, 電気, 水道] から overview の並びを作る。 */
const overview = (zones: [number, number, number][]) => {
  const flat = zones.flat();
  return [...new Array<number>(9).fill(0), ...flat, ...new Array<number>(12 - flat.length).fill(0)];
};

describe("止まっている網への移動", () => {
  const stalls = parseStalls([40, 12, 3, 90, 7, 1]);

  test("平らな配列を網ごとに読む", () => {
    expect(stalls).toEqual([
      { x: 40, y: 12, lacks: 3 },
      { x: 90, y: 7, lacks: 1 },
    ]);
    expect(parseStalls([])).toEqual([]);
    expect(parseStalls([1, 2])).toEqual([]);
  });

  test("押すたびに座標の順で次の網へ移り、最後の次は最初へ戻る", () => {
    const first = pickStall(stalls, null);
    expect(first?.x).toBe(40);
    const second = pickStall(stalls, first ?? null);
    expect(second?.x).toBe(90);
    expect(pickStall(stalls, second ?? null)?.x).toBe(40);
  });

  test("網が増減しても、前の網の代表より後ろへ移る", () => {
    const last = { x: 40, y: 12 };
    // 前の網が消えても、同じ座標に戻らず後ろの網へ移る
    expect(pickStall(parseStalls([90, 7, 1]), last)?.x).toBe(90);
    // 間に網が増えたら、その網へ移る
    expect(pickStall(parseStalls([40, 12, 3, 60, 1, 2, 90, 7, 1]), last)?.x).toBe(60);
    // 後ろの網がなければ最初へ戻る
    expect(pickStall(parseStalls([10, 5, 1, 30, 5, 1]), last)?.x).toBe(10);
  });

  test("網がなければ移らない", () => {
    expect(pickStall([], null)).toBeNull();
    expect(pickStall([], { x: 1, y: 1 })).toBeNull();
  });

  test("網の数でボタンの文言が決まり、件数が揺れても key は変わらない", () => {
    const blocks = parseGrowthBlocks(
      Array.from({ length: 24 }, (_, i) => (i === 9 || i === 10 || i === 11 ? 4 : 0)),
    );
    const bigger = parseGrowthBlocks(
      Array.from({ length: 24 }, (_, i) => (i === 9 || i === 10 || i === 11 ? 9 : 0)),
    );
    const [one] = growthNotices(blocks, parseStalls([40, 12, 3]));
    const [same] = growthNotices(bigger, parseStalls([41, 13, 3]));
    const [two] = growthNotices(blocks, stalls);
    expect(one?.jump?.label).toBe("止まっている区画へ移る");
    expect(two?.jump?.label).toContain("押すたびに次の道路網");
    expect(two?.jump?.targets).toHaveLength(2);
    expect(one?.key).toBe(same?.key);
    expect(growthNotices(blocks, [])[0]?.jump).toBeNull();
    expect(growthNotices(parseGrowthBlocks([]), stalls)).toEqual([]);
  });

  test("止まっている区画の種類が変わっても key が変わる", () => {
    const at = (zone: number) =>
      parseGrowthBlocks(
        Array.from({ length: 24 }, (_, i) => (i === 9 + zone * 3 || i === 10 + zone * 3 ? 4 : 0)),
      );
    expect(growthNotices(at(0), [])[0]?.key).not.toBe(growthNotices(at(1), [])[0]?.key);
  });

  test("足りないものが変わると key が変わる", () => {
    const power = parseGrowthBlocks(
      Array.from({ length: 24 }, (_, i) => (i === 9 || i === 10 ? 4 : 0)),
    );
    const both = parseGrowthBlocks(
      Array.from({ length: 24 }, (_, i) => (i === 9 || i === 10 || i === 11 ? 4 : 0)),
    );
    expect(growthNotices(power, [])[0]?.key).not.toBe(growthNotices(both, [])[0]?.key);
  });
});

describe("growth notice", () => {
  test("overview から区画ごとの数を読む", () => {
    expect(
      parseGrowthBlocks(
        overview([
          [0, 0, 0],
          [3, 3, 1],
        ]),
      )[1],
    ).toEqual({ blocked: 3, noPower: 3, noWater: 1 });
  });

  test("止まっている間口がなければ知らせない", () => {
    expect(growthNotice(parseGrowthBlocks(overview([])))).toBeNull();
  });

  test("電気と水道の両方がなければ、両方を挙げて施設を置くよう促す", () => {
    const n = growthNotice(
      parseGrowthBlocks(
        overview([
          [4, 4, 4],
          [2, 2, 2],
        ]),
      ),
    );
    expect(n?.headline).toContain("電気と水道の余裕がなく");
    expect(n?.headline).toContain("住宅 4・商業 2");
    expect(n?.action).toBe("発電所と浄水場を置くか、発電所と浄水場のある道路網につなぐ");
  });

  test("電気だけが足りないときは電気だけを挙げる", () => {
    const n = growthNotice(
      parseGrowthBlocks(
        overview([
          [0, 0, 0],
          [0, 0, 0],
          [5, 5, 0],
        ]),
      ),
    );
    expect(n?.headline).toContain("電気の余裕がなく");
    expect(n?.headline).not.toContain("水道");
    expect(n?.action).toContain("発電所");
  });
});

describe("つながっていない施設の知らせ", () => {
  const isolated = parseIsolated([10, -59, 1, 14, -59, 1, 3, 4, 2]);
  const growing = parseGrowthBlocks(
    Array.from({ length: 24 }, (_, i) => (i === 9 || i === 10 ? 4 : 0)),
  );

  test("平らな配列を施設ごとに読む", () => {
    expect(isolated).toEqual([
      { x: 10, y: -59, kind: 1 },
      { x: 14, y: -59, kind: 1 },
      { x: 3, y: 4, kind: 2 },
    ]);
    expect(parseIsolated([1, 2])).toEqual([]);
  });

  test("施設の種類ごとの数を見出しに出す", () => {
    expect(isolatedNotice([])).toBeNull();
    const n = isolatedNotice(isolated);
    expect(n?.headline).toBe("施設はあるが、街の道路網につながっていない（発電所 2・浄水場 1）");
    expect(n?.action).toContain("道路でつなぐ");
  });

  test("区画が止まっていなくても、孤立した施設があれば知らせて移れる", () => {
    const none = parseGrowthBlocks([]);
    expect(growthNotices(none, [], [])).toEqual([]);
    expect(growthNotices(none, [], isolated.slice(0, 1))[0]?.jump?.label).toBe(
      "つながっていない施設へ移る",
    );
    expect(growthNotices(none, [], isolated)[0]?.jump?.label).toContain("押すたびに次の場所");
  });

  test("区画の知らせと施設の知らせは、別の知らせとして並び、移る先も別々", () => {
    const list = growthNotices(growing, parseStalls([40, 12, 3]), isolated);
    expect(list.map((n) => n.id)).toEqual(["stall", "cut"]);
    expect(list[0]?.headline).toContain("電気の余裕がなく");
    expect(list[0]?.jump?.targets).toHaveLength(1);
    expect(list[1]?.headline).toContain("施設はあるが、街の道路網につながっていない");
    expect(list[1]?.jump?.targets).toHaveLength(3);
  });

  test("移る先は座標の順で、施設も数に入る", () => {
    const targets = [...parseStalls([40, 12, 3]), ...isolated];
    expect(pickStall(targets, null)).toMatchObject({ x: 3, y: 4 });
    expect(pickStall(targets, { x: 3, y: 4 })).toMatchObject({ x: 10, y: -59 });
  });
});

describe("空き区画がない知らせ", () => {
  const COMMERCIAL = 1;

  test("続いた長さが足りるまでは出ず、足りたら出る", () => {
    let since = trackLotShortage([], [0, 1, 0, 0], 1000);
    expect(since).toEqual([null, 1000, null, null]);
    expect(persistedLotShortage(since, 1000 + LOT_SHORTAGE_TICKS - 1)).toEqual([]);
    since = trackLotShortage(since, [0, 1, 0, 0], 1000 + LOT_SHORTAGE_TICKS);
    expect(since[COMMERCIAL]).toBe(1000);
    expect(persistedLotShortage(since, 1000 + LOT_SHORTAGE_TICKS)).toEqual([COMMERCIAL]);
  });

  test("区画が埋まるか空きができたら数え直し、すぐ戻っても出ない", () => {
    let since = trackLotShortage([], [0, 1, 0, 0], 0);
    since = trackLotShortage(since, [0, 0, 0, 0], LOT_SHORTAGE_TICKS - 10);
    expect(since).toEqual([null, null, null, null]);
    since = trackLotShortage(since, [0, 1, 0, 0], LOT_SHORTAGE_TICKS);
    expect(persistedLotShortage(since, LOT_SHORTAGE_TICKS + 10)).toEqual([]);
  });

  test("読み込み直後の tick が大きくても、最初の overview では出さない", () => {
    const T = 5 * LOT_SHORTAGE_TICKS;
    const since = trackLotShortage([], [0, 1, 0, 0], T);
    expect(persistedLotShortage(since, T)).toEqual([]);
    expect(persistedLotShortage(since, T + LOT_SHORTAGE_TICKS)).toEqual([COMMERCIAL]);
  });

  test("tick が戻ったら数え直す", () => {
    const since = trackLotShortage([5000, null, null, null], [1, 0, 0, 0], 100);
    expect(since[0]).toBe(100);
  });

  test("見出しに区画の種類を挙げる", () => {
    expect(lotNotice([])).toBeNull();
    expect(lotNotice([COMMERCIAL])?.headline).toBe(
      "商業の需要があるのに、空いている商業の区画がない",
    );
    expect(lotNotice([1, 3])?.headline).toBe(
      "商業・オフィスの需要があるのに、空いている区画がない",
    );
  });

  test("空き区画の知らせは移る先を持たず、区画の種類が key になる", () => {
    const none = parseGrowthBlocks([]);
    const [only] = growthNotices(none, [], [], [COMMERCIAL]);
    expect(only?.id).toBe("lot");
    expect(only?.headline).toContain("商業の需要があるのに");
    expect(only?.jump).toBeNull();
    expect(only?.key).not.toBe(growthNotices(none, [], [], [COMMERCIAL, 3])[0]?.key);
    const both = growthNotices(none, [], [{ x: 1, y: 2, kind: 1 }], [COMMERCIAL]);
    expect(both.map((n) => n.id)).toEqual(["lot", "cut"]);
    expect(growthNotices(none, [], [], [])).toEqual([]);
  });

  describe("駐車場が足りない建物の知らせ", () => {
    const flat = [12, 30, 1, 5, 40, 8, 1, 3, 2, 2, 0, 4];

    test("平らな配列を読み、種類ごとの棟数を見出しに出す", () => {
      const shortages = parseParkingShortage(flat);
      expect(shortages).toHaveLength(3);
      expect(shortages[0]).toEqual({ x: 12, y: 30, zone: 1, full: 5 });
      expect(parkingNotice([])).toBeNull();
      expect(parkingNotice(shortages)?.headline).toBe(
        "駐車場が足りない建物がある（住宅 1 棟・商業 2 棟）",
      );
    });

    test("建物へ移れ、件数が揺れても key は変わらず、種類が増えると変わる", () => {
      const none = parseGrowthBlocks([]);
      const [n] = growthNotices(none, [], [], [], parseParkingShortage(flat));
      expect(n?.id).toBe("parking");
      expect(n?.jump?.targets).toHaveLength(3);
      expect(n?.jump?.label).toContain("押すたびに次の建物");
      const more = growthNotices(none, [], [], [], parseParkingShortage([...flat, 9, 9, 1, 3]));
      expect(more[0]?.key).toBe(n?.key);
      const other = growthNotices(none, [], [], [], parseParkingShortage([9, 9, 2, 3]));
      expect(other[0]?.key).not.toBe(n?.key);
    });

    test("満車が続くあいだ出て、解消したら一覧から消える", () => {
      const none = parseGrowthBlocks([]);
      const shown = growthNotices(none, [], [], [], parseParkingShortage(flat));
      const first = reconcile(emptyMemory(), shown, 100);
      expect(first.visible.map((v) => v.id)).toEqual(["parking"]);
      const again = reconcile(first.memory, shown, 200);
      expect(again.visible.map((v) => v.id)).toEqual(["parking"]);
      expect(again.fresh).toEqual([]);
      const solved = reconcile(again.memory, growthNotices(none, [], [], [], []), 300);
      expect(solved.visible).toEqual([]);
    });
  });
});

describe("車を持てない世帯の知らせ", () => {
  // 世帯の数、理由（範囲に駐車場がない、満車、料金、待ち、道路に面していない）、世帯の多い区切り（x, y, 世帯の数）
  const flat = [120, 80, 30, 10, 0, 0, 44, 12, 50, 20, 9, 31, 8];

  test("平らな配列を読み、世帯の多い所を並びのまま持つ", () => {
    expect(parseCarless(flat)).toEqual({
      total: 120,
      reasons: [80, 30, 10, 0, 0],
      areas: [
        { x: 44, y: 12, households: 50 },
        { x: 20, y: 9, households: 31 },
      ],
    });
    // 世帯がいなければ空
    expect(parseCarless([0, 0, 0, 0, 0, 0])).toEqual({
      total: 0,
      reasons: [0, 0, 0, 0, 0],
      areas: [],
    });
  });

  test("車を持てない世帯があるときだけ出し、世帯の数と理由を添える", () => {
    expect(carlessNotice(parseCarless([0, 0, 0, 0, 0, 0]))).toBeNull();
    const notice = carlessNotice(parseCarless(flat));
    expect(notice?.headline).toBe("車を持てない世帯がある（120 世帯）");
    expect(notice?.action).toContain("範囲に駐車場がない 80・満車 30・料金が高い 10");
    // 待っている世帯だけのときは、理由の文を出さない
    expect(carlessNotice(parseCarless([3, 0, 0, 0, 3, 0]))?.action).not.toContain("・");
    expect(notice?.action).toContain("直線で 32 マス");
    expect(notice?.action).not.toContain("道路を引く");
    // 料金が上限を超える世帯がいるときは、料金を下げるよう促す
    expect(notice?.action).toContain("駐車場の料金を下げる");
    expect(carlessNotice(parseCarless([30, 30, 0, 0, 0, 0]))?.action).not.toContain("料金を下げる");
    // 範囲の駐車場が道路に面していない世帯がいるときは、駐車場の前に道路を引くよう促す
    const cut = carlessNotice(parseCarless([40, 0, 0, 0, 0, 40]));
    expect(cut?.action).toContain("駐車場が道路に面していない 40");
    expect(cut?.action).toContain("駐車場の前に道路を引く");
  });

  test("世帯の多い所へ、並びの順に移り、最後の次は最初へ戻る", () => {
    const none = parseGrowthBlocks([]);
    const [n] = growthNotices(none, [], [], [], [], parseCarless(flat));
    expect(n?.id).toBe("carless");
    expect(n?.jump?.ordered).toBe(true);
    expect(n?.jump?.label).toContain("押すたびに次の所");
    const targets = n?.jump?.targets ?? [];
    const first = pickNext(targets, null);
    expect(first).toMatchObject({ x: 44, y: 12 });
    const second = pickNext(targets, first);
    expect(second).toMatchObject({ x: 20, y: 9 });
    expect(pickNext(targets, second)).toMatchObject({ x: 44, y: 12 });
    // 並びが変わって前の先がなくなったら、いちばん多い所から
    expect(pickNext(targets, { x: 1, y: 1 })).toMatchObject({ x: 44, y: 12 });
    expect(pickNext([], null)).toBeNull();
  });

  test("世帯の数が揺れても key は変わらず、世帯がなくなれば消える", () => {
    const none = parseGrowthBlocks([]);
    const key = (carless: number[]) =>
      growthNotices(none, [], [], [], [], parseCarless(carless))[0]?.key;
    expect(key(flat)).toBe(key([121, 80, 31, 10, 0, 0, 44, 12, 51]));
    expect(growthNotices(none, [], [], [], [], parseCarless([0, 0, 0, 0, 0, 0]))).toEqual([]);
    const shown = growthNotices(none, [], [], [], [], parseCarless(flat));
    const first = reconcile(emptyMemory(), shown, 100);
    expect(first.visible.map((v) => v.id)).toEqual(["carless"]);
    const solved = reconcile(first.memory, [], 200);
    expect(solved.visible).toEqual([]);
  });

  test("閉じると、車を持てない世帯が続くあいだは出し直さない", () => {
    const none = parseGrowthBlocks([]);
    const shown = growthNotices(none, [], [], [], [], parseCarless(flat));
    const first = reconcile(emptyMemory(), shown, 100);
    expect(first.fresh.map((f) => f.id)).toEqual(["carless"]);
    const [notice] = first.visible;
    if (notice === undefined) {
      throw new Error("知らせが出ていない");
    }
    const closed = dismiss(first.memory, notice, 100);
    // 世帯の数が増えても、閉じたままで札も出さない
    const grown = growthNotices(
      none,
      [],
      [],
      [],
      [],
      parseCarless([200, 150, 50, 0, 0, 0, 44, 12, 90]),
    );
    const again = reconcile(closed, grown, 200);
    expect(again.visible).toEqual([]);
    expect(again.fresh).toEqual([]);
  });
});
