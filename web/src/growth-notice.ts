// 区画が建たない理由の知らせ。電気か水道の余裕がなくて新築が止まっている間口を、区画の種類ごとに数えて出す。

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

/** 押すたびに次の網を選ぶ。網の数が変わっても範囲に収める。網がなければ null。 */
export function pickStall(stalls: Stall[], cursor: number): { stall: Stall; next: number } | null {
  const at = stalls.length === 0 ? 0 : ((cursor % stalls.length) + stalls.length) % stalls.length;
  const stall = stalls[at];
  return stall === undefined ? null : { stall, next: at + 1 };
}

/**
 * 知らせを押せるようにする。押すと止まっている道路網の区画へ jump を呼び、網が複数あれば押すたびに次へ移る。
 * 作り直すとボタンのフォーカスが外れるので、中身が変わらないあいだは DOM に触らない。
 */
export function setupGrowthNotice(jump: (x: number, y: number) => void): {
  render: (blocks: GrowthBlocks, stalls: Stall[]) => void;
} {
  const el = document.querySelector<HTMLElement>("#growth-notice");
  let stalls: Stall[] = [];
  let cursor = 0;
  let shown = "";
  const go = () => {
    const picked = pickStall(stalls, cursor);
    if (picked === null) {
      return;
    }
    cursor = picked.next;
    jump(picked.stall.x, picked.stall.y);
  };
  return {
    render(blocks, next) {
      if (el === null) {
        return;
      }
      stalls = next;
      const notice = growthNotice(blocks);
      el.hidden = notice === null;
      if (notice === null) {
        shown = "";
        el.replaceChildren();
        return;
      }
      const key = JSON.stringify([notice, stalls.length]);
      if (key === shown) {
        return;
      }
      shown = key;
      const head = document.createElement("strong");
      head.textContent = notice.headline;
      const action = document.createElement("div");
      action.textContent = notice.action;
      el.replaceChildren(head, action);
      if (stalls.length > 0) {
        const button = document.createElement("button");
        button.type = "button";
        button.textContent =
          stalls.length > 1
            ? "止まっている区画へ移る（押すたびに次の道路網）"
            : "止まっている区画へ移る";
        button.addEventListener("click", go);
        el.append(button);
      }
    },
  };
}
