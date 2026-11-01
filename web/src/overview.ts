// 人口、幸福度、人口のマイルストーンの表示。左上のパネルに出し、届いたときは数秒で消える知らせを出す。

/** chizu-wasm の overview を読んだもの。 */
export type Overview = {
  /** 家のある住民の数。 */
  population: number;
  /** 家のある住民の幸福度の平均（0〜100）。 */
  happiness: number;
  /** 幸福度の 3 つの要素の平均（0〜100）。 */
  commute: number;
  services: number;
  wealth: number;
  /** 届いた段階。0 ならまだどこにも届いていない。 */
  milestone: number;
  /** 次の段階に届く人口。 */
  nextPopulation: number;
  /** いまの段階までで買える土地の枠の数と、次の段階で買える枠の数。 */
  slots: number;
  nextSlots: number;
};

export function parseOverview(values: number[]): Overview {
  const at = (i: number) => values[i] ?? 0;
  return {
    population: at(0),
    happiness: at(1),
    commute: at(2),
    services: at(3),
    wealth: at(4),
    milestone: at(5),
    nextPopulation: at(6),
    slots: at(7),
    nextSlots: at(8),
  };
}

/** 幸福度の満点。 */
export const HAPPINESS_FULL = 100;

const n = (v: number) => v.toLocaleString("ja-JP");

export function formatPopulation(o: Overview): string {
  return `人口 ${n(o.population)}`;
}

export function formatHappiness(o: Overview): string {
  return `幸福度 ${n(o.happiness)} / ${HAPPINESS_FULL}`;
}

/** 次の段階までの残り。 */
export function formatRemaining(o: Overview): string {
  return `次の段階まであと ${n(Math.max(0, o.nextPopulation - o.population))} 人`;
}

/** 次の段階に届く人口と、そこで増える土地の枠。 */
export function formatNextMilestone(o: Overview): string {
  const gain = Math.max(0, o.nextSlots - o.slots);
  return `${n(o.nextPopulation)} 人に届くと、買える土地の枠が ${n(gain)} 増える`;
}

/** 幸福度の内訳の行。 */
export function happinessParts(o: Overview): { label: string; value: number; text: string }[] {
  return [
    { label: "通勤", value: o.commute },
    { label: "生活サービス", value: o.services },
    { label: "暮らし向き", value: o.wealth },
  ].map((p) => ({ ...p, text: `${p.label} ${n(p.value)} / ${HAPPINESS_FULL}` }));
}

/** 段階に届いたときの知らせの文。 */
export type MilestoneNotice = { headline: string; detail: string };

export function milestoneNoticeText(
  milestone: number,
  slotsBefore: number,
  slotsNow: number,
): MilestoneNotice {
  const gain = Math.max(0, slotsNow - slotsBefore);
  return {
    headline: `人口が段階 ${n(milestone)} に届いた`,
    detail: `買える土地の枠が ${n(gain)} 増えて ${n(slotsNow)} になった`,
  };
}

/**
 * マイルストーンに届いたかを、届いた段階の移り変わりから見分ける。
 * 最初に見た値は基準にするだけで知らせない。保存を読み込み直したときに、すでに届いている段階を
 * 「届いた」と言わないためである。一時停止中は街が進まないので段階も変わらず、知らせは出ない。
 */
export class MilestoneTracker {
  private seen: { milestone: number; slots: number } | null = null;

  observe(o: Overview): MilestoneNotice | null {
    const prev = this.seen;
    this.seen = { milestone: o.milestone, slots: o.slots };
    if (prev === null || o.milestone <= prev.milestone) {
      return null;
    }
    return milestoneNoticeText(o.milestone, prev.slots, o.slots);
  }
}

/** 知らせを出しておく時間（ミリ秒）。 */
export const NOTICE_MS = 6000;

const tracker = new MilestoneTracker();
let noticeTimer: ReturnType<typeof setTimeout> | undefined;

function showNotice(notice: MilestoneNotice): void {
  const el = document.querySelector<HTMLElement>("#milestone-notice");
  if (el === null) {
    return;
  }
  el.replaceChildren();
  const head = document.createElement("strong");
  head.textContent = notice.headline;
  const detail = document.createElement("div");
  detail.textContent = notice.detail;
  el.append(head, detail);
  el.hidden = false;
  el.dataset.count = String(Number(el.dataset.count ?? 0) + 1);
  clearTimeout(noticeTimer);
  noticeTimer = setTimeout(() => {
    el.hidden = true;
  }, NOTICE_MS);
}

export function renderOverview(o: Overview): void {
  const set = (selector: string, text: string) => {
    const el = document.querySelector<HTMLElement>(selector);
    if (el !== null) {
      el.textContent = text;
    }
  };
  set("#overview-population", formatPopulation(o));
  set("#overview-happiness", formatHappiness(o));
  set("#overview-remaining", formatRemaining(o));
  set("#overview-next", formatNextMilestone(o));
  const parts = document.querySelector<HTMLElement>("#overview-parts");
  if (parts !== null) {
    parts.replaceChildren(
      ...happinessParts(o).map((p) => {
        const line = document.createElement("div");
        line.textContent = p.text;
        return line;
      }),
    );
  }
  const root = document.querySelector<HTMLElement>("#overview");
  if (root !== null) {
    // test:wasm が読む
    root.dataset.population = String(o.population);
    root.dataset.milestone = String(o.milestone);
  }
  const notice = tracker.observe(o);
  if (notice !== null) {
    showNotice(notice);
  }
}
