import type { Notice } from "./notice-center.ts";

// 区画が建たない理由の知らせ。電気か水道の余裕がなくて新築が止まっている間口を、区画の種類ごとに数えて出す。
// 施設はあるが街の道路網につながっていない場合も、別の知らせとして出す。知らせを並べて見せるのは notice-center.ts である。

/** 区画の種類の順。chizu-wasm の overview の並びと合わせる。 */
export const ZONE_NAMES = ["住宅", "商業", "工業", "オフィス"] as const;

/** overview のうち、区画ごとの数が始まる位置。 */
export const GROWTH_OFFSET = 9;

/** 区画の種類ごとの、新築が止まっている間口の数と、そのうち電気、水道の余裕がない数。 */
export type GrowthBlocks = { blocked: number; noPower: number; noWater: number }[];

export function parseGrowthBlocks(overview: number[]): GrowthBlocks {
  const at = (i: number) => overview[i] ?? 0;
  return ZONE_NAMES.map((_, z) => {
    const base = GROWTH_OFFSET + z * 3;
    return { blocked: at(base), noPower: at(base + 1), noWater: at(base + 2) };
  });
}

/** つながっていない施設 1 つ。x, y は施設の道路に面したマス、kind は施設の種類の値（発電所が 1、浄水場が 2）。 */
export type Isolated = { x: number; y: number; kind: number };

/** worker から届いた平らな配列 [x, y, 種類, ...] を読む。 */
export function parseIsolated(flat: number[]): Isolated[] {
  const out: Isolated[] = [];
  for (let i = 0; i + 2 < flat.length; i += 3) {
    out.push({ x: flat[i] ?? 0, y: flat[i + 1] ?? 0, kind: flat[i + 2] ?? 0 });
  }
  return out;
}

/** 施設はあるが、街の道路網につながっていない。なければ null。 */
export function isolatedNotice(isolated: Isolated[]): GrowthNotice | null {
  if (isolated.length === 0) {
    return null;
  }
  const count = (kind: number) => isolated.filter((f) => f.kind === kind).length;
  const parts = [
    ...(count(1) > 0 ? [`発電所 ${count(1)}`] : []),
    ...(count(2) > 0 ? [`浄水場 ${count(2)}`] : []),
  ];
  return {
    headline: `施設はあるが、街の道路網につながっていない（${parts.join("・")}）`,
    action: "道路でつなぐか、街の道路網のそばに置き直す。つながっていない分の余りは数えない",
  };
}

export type GrowthNotice = { headline: string; action: string };

/** 止まっている間口がなければ null。 */
export function growthNotice(blocks: GrowthBlocks): GrowthNotice | null {
  const n = (v: number) => v.toLocaleString("ja-JP");
  const sum = (f: (b: GrowthBlocks[number]) => number) => blocks.reduce((a, b) => a + f(b), 0);
  const power = sum((b) => b.noPower) > 0;
  const water = sum((b) => b.noWater) > 0;
  const zones = blocks.flatMap((b, z) =>
    b.blocked > 0 ? [`${ZONE_NAMES[z]} ${n(b.blocked)}`] : [],
  );
  if (zones.length === 0 || (!power && !water)) {
    return null;
  }
  const what = power && water ? "電気と水道" : power ? "電気" : "水道";
  const facility = power && water ? "発電所と浄水場" : power ? "発電所" : "浄水場";
  return {
    headline: `${what}の余裕がなく、新築が止まっている区画がある（${zones.join("・")} 間口）`,
    action: `${facility}を置くか、${facility}のある道路網につなぐ`,
  };
}

/** 新築が止まっている道路網 1 つ。x, y は止まっている間口のマスで、lacks は電気が 1、水道が 2 の和。 */
export type Stall = { x: number; y: number; lacks: number };

/** worker から届いた平らな配列 [x, y, 足りない資源, ...] を読む。 */
export function parseStalls(flat: number[]): Stall[] {
  const out: Stall[] = [];
  for (let i = 0; i + 2 < flat.length; i += 3) {
    out.push({ x: flat[i] ?? 0, y: flat[i + 1] ?? 0, lacks: flat[i + 2] ?? 0 });
  }
  return out;
}

const before = (a: { x: number; y: number }, b: { x: number; y: number }) =>
  a.x < b.x || (a.x === b.x && a.y < b.y);

/**
 * 前に移った網の代表のマス last より座標の順で後ろにある、最初の網を選ぶ。後ろがなければ最初の網へ戻る。
 * last が null なら最初の網である。網の数が変わっても、同じ網に戻ったり、飛ばしたりしにくい。
 */
export function pickStall<T extends { x: number; y: number }>(
  stalls: T[],
  last: { x: number; y: number } | null,
): T | null {
  const sorted = [...stalls].sort((a, b) => (before(a, b) ? -1 : before(b, a) ? 1 : 0));
  if (last === null) {
    return sorted[0] ?? null;
  }
  return sorted.find((s) => before(last, s)) ?? sorted[0] ?? null;
}

/**
 * 空き区画がない状態が続いたら知らせる長さ（tick）。ゲーム内の半日である（1 日は 7200 tick）。
 * 区画が建ち尽くした直後や、塗り足してすぐ建ち尽くす間は需要が残っていても、
 * 次の区画を塗る間に知らせが出入りしてちらつく。半日あれば、気づいて塗る時間として足りる。
 */
export const LOT_SHORTAGE_TICKS = 3600;

/** 区画の種類ごとに、空き区画がない状態になった tick。なければ null。 */
export type LotShortageSince = (number | null)[];

/**
 * overview の flags（区画の種類ごとの 1 か 0）と今の tick から、空き区画のない状態が始まった tick を更新する。
 * 偽に戻った区画は数え直す。tick が戻った（読み込みや新しい街）ときも数え直す。
 */
export function trackLotShortage(
  since: LotShortageSince,
  flags: number[],
  tick: number,
): LotShortageSince {
  return ZONE_NAMES.map((_, z) => {
    if ((flags[z] ?? 0) !== 1) {
      return null;
    }
    const start = since[z] ?? null;
    return start === null || start > tick ? tick : start;
  });
}

/** 空き区画がない状態が LOT_SHORTAGE_TICKS 以上続いている区画の番号。 */
export function persistedLotShortage(since: LotShortageSince, tick: number): number[] {
  return ZONE_NAMES.flatMap((_, z) => {
    const start = since[z] ?? null;
    return start !== null && tick - start >= LOT_SHORTAGE_TICKS ? [z] : [];
  });
}

/** 需要があるのに空き区画がない。zones は区画の種類の番号で、なければ null。 */
export function lotNotice(zones: number[]): GrowthNotice | null {
  if (zones.length === 0) {
    return null;
  }
  const names = zones.map((z) => ZONE_NAMES[z]);
  const one = names.length === 1;
  return {
    headline: one
      ? `${names[0]}の需要があるのに、空いている${names[0]}の区画がない`
      : `${names.join("・")}の需要があるのに、空いている区画がない`,
    action: one ? `${names[0]}の区画を塗り足す` : "その種類の区画を塗り足す",
  };
}

const jumpLabel = (what: string, count: number, next: string) =>
  count > 1 ? `${what}へ移る（押すたびに次の${next}）` : `${what}へ移る`;

/**
 * 止まっている区画、空き区画がない、つながっていない施設の知らせを、1 つずつの知らせとして並べる。
 * key は知らせの状態（何が足りないか、どの種類か）で、件数のように揺れる値は入れない。
 */
export function growthNotices(
  blocks: GrowthBlocks,
  stalls: Stall[],
  isolated: Isolated[] = [],
  lotZones: number[] = [],
): Notice[] {
  const out: Notice[] = [];
  const growth = growthNotice(blocks);
  if (growth !== null) {
    const sum = (f: (b: GrowthBlocks[number]) => number) => blocks.reduce((a, b) => a + f(b), 0);
    out.push({
      id: "stall",
      key: `${sum((b) => b.noPower) > 0}:${sum((b) => b.noWater) > 0}:${blocks.flatMap((b, z) => (b.blocked > 0 ? [z] : [])).join(",")}`,
      headline: growth.headline,
      details: [growth.action],
      jump:
        stalls.length === 0
          ? null
          : { label: jumpLabel("止まっている区画", stalls.length, "道路網"), targets: stalls },
    });
  }
  const lot = lotNotice(lotZones);
  if (lot !== null) {
    out.push({
      id: "lot",
      key: lotZones.join(","),
      headline: lot.headline,
      details: [lot.action],
      jump: null,
    });
  }
  const cut = isolatedNotice(isolated);
  if (cut !== null) {
    out.push({
      id: "cut",
      key: [...new Set(isolated.map((f) => f.kind))].sort().join(","),
      headline: cut.headline,
      details: [cut.action],
      jump: {
        label: jumpLabel("つながっていない施設", isolated.length, "場所"),
        targets: isolated,
      },
    });
  }
  return out;
}
