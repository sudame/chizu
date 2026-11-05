import type { Notice } from "./notice-center.ts";
import { CARLESS_FIXED } from "./protocol/messages.ts";

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
 * 前に移った先 last の次を、targets の並びのまま選ぶ。last が並びになければ（先頭、または並びが変わった）先頭を選ぶ。
 * 最後まで来たら先頭へ戻る。並びの先頭が一番大きい所なら、押し始めはそこへ移る。
 */
export function pickNext<T extends { x: number; y: number }>(
  targets: T[],
  last: { x: number; y: number } | null,
): T | null {
  if (last === null) {
    return targets[0] ?? null;
  }
  const at = targets.findIndex((t) => t.x === last.x && t.y === last.y);
  return targets[at + 1] ?? targets[0] ?? null;
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

/** 駐車場が足りない建物 1 つ。x, y は建物が占めるマスの最小の座標、zone は区画の種類の番号、full は満車で車をやめた数。 */
export type ParkingShortage = { x: number; y: number; zone: number; full: number };

/** worker から届いた平らな配列 [x, y, 区画の種類, 満車で車をやめた数, ...] を読む。 */
export function parseParkingShortage(flat: number[]): ParkingShortage[] {
  const out: ParkingShortage[] = [];
  for (let i = 0; i + 3 < flat.length; i += 4) {
    out.push({
      x: flat[i] ?? 0,
      y: flat[i + 1] ?? 0,
      zone: flat[i + 2] ?? 0,
      full: flat[i + 3] ?? 0,
    });
  }
  return out;
}

/** 満車で車をやめた建物がある。種類ごとの棟数を見出しに出す。なければ null。 */
export function parkingNotice(shortages: ParkingShortage[]): GrowthNotice | null {
  if (shortages.length === 0) {
    return null;
  }
  const parts = ZONE_NAMES.flatMap((name, z) => {
    const n = shortages.filter((s) => s.zone === z).length;
    return n > 0 ? [`${name} ${n} 棟`] : [];
  });
  return {
    headline: `駐車場が足りない建物がある（${parts.join("・")}）`,
    action: "行き先の近くに、駐車場を誰でも使える商業・工業・オフィスの建物を足す",
  };
}

/** 車を持てない世帯の数と、理由ごとの数、世帯の多い所。chizu-wasm の carless を読んだもの。 */
export type Carless = {
  total: number;
  /** 範囲に駐車場がない、満車、料金が上限を超える、借りるのを待っている、範囲の駐車場が道路に面していない、の順の世帯の数。 */
  reasons: number[];
  /** 世帯の多い区切り（8×8 マス）の真ん中のマスと世帯の数。多い順。 */
  areas: { x: number; y: number; households: number }[];
};

/** worker から届いた [世帯の数, 理由ごとの数 ×5, x, y, 世帯の数, ...] を読む。 */
export function parseCarless(flat: number[]): Carless {
  const at = (i: number) => flat[i] ?? 0;
  const areas: Carless["areas"] = [];
  for (let i = CARLESS_FIXED; i + 2 < flat.length; i += 3) {
    areas.push({ x: at(i), y: at(i + 1), households: at(i + 2) });
  }
  return { total: at(0), reasons: [1, 2, 3, 4, 5].map(at), areas };
}

/** 車を持てない世帯がある。世帯の数と、いちばん多い理由を見出しに出す。なければ null。 */
export function carlessNotice(carless: Carless): GrowthNotice | null {
  if (carless.total === 0) {
    return null;
  }
  const n = (v: number) => v.toLocaleString("ja-JP");
  const [noLot = 0, full = 0, fee = 0, , noRoad = 0] = carless.reasons;
  const parts = [
    ...(noLot > 0 ? [`範囲に駐車場がない ${n(noLot)}`] : []),
    ...(full > 0 ? [`満車 ${n(full)}`] : []),
    ...(fee > 0 ? [`料金が高い ${n(fee)}`] : []),
    ...(noRoad > 0 ? [`駐車場が道路に面していない ${n(noRoad)}`] : []),
  ];
  const hint =
    noRoad > 0
      ? "駐車場の前に道路を引く。世帯の多い所の近くには、共同の駐車場も置く。"
      : "世帯の多い所の近くに、共同の駐車場を置く。";
  const feeHint = fee > 0 ? "料金が高い世帯は、収支の欄で駐車場の料金を下げると借りる。" : "";
  return {
    headline: `車を持てない世帯がある（${n(carless.total)} 世帯）`,
    action:
      (parts.length > 0 ? `${parts.join("・")}。` : "") +
      `${hint}${feeHint}直線で 32 マス（256m）の中の駐車場だけを借りる`,
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
  parking: ParkingShortage[] = [],
  carless: Carless = { total: 0, reasons: [0, 0, 0, 0, 0], areas: [] },
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
  const park = parkingNotice(parking);
  if (park !== null) {
    out.push({
      id: "parking",
      key: [...new Set(parking.map((s) => s.zone))].sort().join(","),
      headline: park.headline,
      details: [park.action],
      jump: {
        label: jumpLabel("駐車場が足りない建物", parking.length, "建物"),
        targets: parking,
      },
    });
  }
  const poor = carlessNotice(carless);
  if (poor !== null) {
    out.push({
      id: "carless",
      // 世帯の数のように揺れる値は入れない。車を持てない世帯がなくなるまで、同じ状態である
      key: "carless",
      headline: poor.headline,
      details: [poor.action],
      jump: {
        label: jumpLabel("車を持てない世帯の多い所", carless.areas.length, "所"),
        targets: carless.areas,
        ordered: true,
      },
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
