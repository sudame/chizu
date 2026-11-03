import { describe, expect, test } from "vitest";
import {
  growthNotice,
  isolatedNotice,
  LOT_SHORTAGE_TICKS,
  lotNotice,
  noticeView,
  parseGrowthBlocks,
  parseIsolated,
  parseStalls,
  persistedLotShortage,
  pickStall,
  sameView,
  trackLotShortage,
} from "./growth-notice.ts";

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

  test("網の数でボタンの文言が決まり、中身が同じあいだは作り直さない", () => {
    const blocks = parseGrowthBlocks(
      Array.from({ length: 21 }, (_, i) => (i === 9 || i === 10 || i === 11 ? 4 : 0)),
    );
    const one = noticeView(blocks, parseStalls([40, 12, 3]));
    const same = noticeView(blocks, parseStalls([41, 13, 3]));
    const two = noticeView(blocks, stalls);
    expect(one?.jump).toBe("止まっている区画へ移る");
    expect(two?.jump).toContain("押すたびに次の道路網");
    expect(sameView(one, same)).toBe(true);
    expect(sameView(one, two)).toBe(false);
    expect(noticeView(blocks, [])?.jump).toBeNull();
    expect(noticeView(parseGrowthBlocks([]), stalls)).toBeNull();
    expect(sameView(null, null)).toBe(true);
    expect(sameView(null, one)).toBe(false);
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
    Array.from({ length: 21 }, (_, i) => (i === 9 || i === 10 ? 4 : 0)),
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
    expect(noticeView(none, [], [])).toBeNull();
    expect(noticeView(none, [], isolated.slice(0, 1))).toMatchObject({
      jump: "つながっていない施設へ移る",
    });
    expect(noticeView(none, [], isolated)?.jump).toContain("押すたびに次の場所");
  });

  test("区画が止まっているときは、見出しは区画の知らせのまま、施設の知らせを足す", () => {
    const view = noticeView(growing, parseStalls([40, 12, 3]), isolated);
    expect(view?.headline).toContain("電気の余裕がなく");
    expect(view?.action).toContain("施設はあるが、街の道路網につながっていない");
    expect(view?.jump).toContain("止まっている区画・つながっていない施設");
    expect(sameView(view, noticeView(growing, parseStalls([40, 12, 3]), []))).toBe(false);
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

  test("ほかの知らせと一緒のときは行動の文の後ろに足し、移る先には数えない", () => {
    const none = parseGrowthBlocks([]);
    const only = noticeView(none, [], [], [COMMERCIAL]);
    expect(only?.headline).toContain("商業の需要があるのに");
    expect(only?.jump).toBeNull();
    const both = noticeView(none, [], [{ x: 1, y: 2, kind: 1 }], [COMMERCIAL]);
    expect(both?.headline).toContain("商業の需要があるのに");
    expect(both?.action).toContain("つながっていない");
    expect(both?.jump).toBe("つながっていない施設へ移る");
    expect(noticeView(none, [], [], [])).toBeNull();
  });
});
