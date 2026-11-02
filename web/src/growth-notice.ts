// 区画が建たない理由の知らせ。電気か水道の余裕がなくて新築が止まっている間口を、区画の種類ごとに数えて出す。
// 施設はあるが街の道路網につながっていない場合も、同じ知らせに出す。

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

/** 知らせの見た目。ボタンの文言は移る先の数と種類で決まる。 */
export type NoticeView = { headline: string; action: string; jump: string | null };

/**
 * 止まっている区画の知らせと、つながっていない施設の知らせを 1 つにまとめる。どちらもなければ null。
 * 両方あるときは、見出しは区画の知らせで、施設の知らせを行動の文の後ろに足す。
 */
export function noticeView(
  blocks: GrowthBlocks,
  stalls: Stall[],
  isolated: Isolated[] = [],
): NoticeView | null {
  const growth = growthNotice(blocks);
  const cut = isolatedNotice(isolated);
  if (growth === null && cut === null) {
    return null;
  }
  const targets = (growth === null ? 0 : stalls.length) + isolated.length;
  const what =
    growth !== null && cut !== null
      ? "止まっている区画・つながっていない施設"
      : cut !== null
        ? "つながっていない施設"
        : "止まっている区画";
  const jump =
    targets === 0
      ? null
      : targets > 1
        ? `${what}へ移る（押すたびに次の${cut === null ? "道路網" : "場所"}）`
        : `${what}へ移る`;
  if (growth !== null && cut !== null) {
    return { headline: growth.headline, action: `${growth.action}。${cut.headline}`, jump };
  }
  const only = growth ?? cut;
  return only === null ? null : { ...only, jump };
}

/** 前に出した見た目と同じなら、DOM に触らなくてよい。触ると読み上げが繰り返され、フォーカスも外れる。 */
export function sameView(a: NoticeView | null, b: NoticeView | null): boolean {
  return (
    a === b ||
    (a !== null &&
      b !== null &&
      a.headline === b.headline &&
      a.action === b.action &&
      a.jump === b.jump)
  );
}

/**
 * 知らせを押せるようにする。押すと止まっている道路網の区画へ jump を呼び、網が複数あれば押すたびに次へ移る。
 * 要素は index.html に置いてあり、ここでは文言だけを書き換える。読み上げるのは見出しの文だけである。
 */
export function setupGrowthNotice(jump: (x: number, y: number) => void): {
  render: (blocks: GrowthBlocks, stalls: Stall[], isolated?: Isolated[]) => void;
} {
  const root = document.querySelector<HTMLElement>("#growth-notice");
  const head = document.querySelector<HTMLElement>("#growth-notice-head");
  const action = document.querySelector<HTMLElement>("#growth-notice-action");
  const button = document.querySelector<HTMLButtonElement>("#growth-notice-jump");
  let targets: { x: number; y: number }[] = [];
  let last: { x: number; y: number } | null = null;
  let shown: NoticeView | null = null;
  button?.addEventListener("click", () => {
    const picked = pickStall(targets, last);
    if (picked === null) {
      return;
    }
    last = { x: picked.x, y: picked.y };
    jump(picked.x, picked.y);
  });
  return {
    render(blocks, next, isolated = []) {
      const view = noticeView(blocks, next, isolated);
      // 区画の知らせが出ていないときの止まっている網には移らない
      const stalled = growthNotice(blocks) === null ? [] : next;
      targets = [...stalled, ...isolated];
      if (view === null) {
        // 知らせが消えたら、次は最初の網から数え直す
        last = null;
      }
      if (root === null || head === null || action === null || button === null) {
        return;
      }
      if (sameView(shown, view)) {
        return;
      }
      shown = view;
      root.hidden = view === null;
      if (view !== null) {
        head.textContent = view.headline;
        action.textContent = view.action;
        button.hidden = view.jump === null;
        button.textContent = view.jump ?? "";
      }
    },
  };
}
