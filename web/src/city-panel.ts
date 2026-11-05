// 街の数字の表示。建物の数、世帯と職場の上限、区画の種類ごとの需要（CS2 の RCI のような棒）を出す。

import type { Notice } from "./notice-center.ts";
import {
  CITY_STATS_LENGTH,
  FACTOR_KIND,
  validCityStats,
  ZONE_COUNT,
} from "./protocol/city-stats.ts";
import {
  type DemandSample,
  demandTrend,
  formatTimeUntilOut,
  type HeadroomTrend,
  pushDemandSample,
} from "./utility-trend.ts";

/** chizu-wasm の city_stats を読んだもの。 */
export type CityStats = {
  buildings: number;
  households: number;
  workplaces: number;
  /** 住宅・商業・工業・オフィスの順。正なら建物が建つ。 */
  demand: [number, number, number, number];
  /** 道路網につながった発電所と浄水場の供給量の合計。 */
  power: number;
  water: number;
  /** 建物の電気と水道の需要の合計。電気と水道で同じ値である。 */
  utilityDemand: number;
  /** 供給量が需要に足りない道路網の数。施設のない網も数える。 */
  powerShort: number;
  waterShort: number;
  /** 建物があるのに施設のない道路網の数（需要のある網で供給量が 0）。 */
  powerIsolated: number;
  waterIsolated: number;
  /** 施設のある網で、需要が供給量を上回った分の合計。 */
  powerGap: number;
  waterGap: number;
  /** 主の網（需要がいちばん大きい網）の供給量と需要。余りは網をまたいで足さず、この網で見る。 */
  mainPower: number;
  mainWater: number;
  mainDemand: number;
  /** 主の網以外で、施設か建物のある道路網の数。 */
  powerOthers: number;
  waterOthers: number;
  /** 主の網の余りが住民の何人分か（電気、水道）。足りないときは負。peopleKnown が偽のとき（人口か需要が 0）は使わない。 */
  powerPeople: number;
  waterPeople: number;
  peopleKnown: boolean;
  /** 発電所 1 基と浄水場 1 基で増える住民の数。peopleKnown が偽のときは使わない。 */
  powerFacilityPeople: number;
  waterFacilityPeople: number;
  /** 主の網を見分ける印。主の網が別の網に替わったかを見る。主の網がなければ 0。 */
  mainNetwork: number;
  /** 住宅・商業・工業・オフィスの順の、需要の内訳。寄与の合計がその区画の需要である。 */
  demandFactors: DemandFactor[][];
};

/** 需要への寄与 1 つ。kind は chizu-core の DemandFactorKind の値、value は需要に足される量。 */
export type DemandFactor = { kind: number; value: number };

/**
 * 区画の種類ごとに、要因の数と `[kind, value]` の対が続く並びを読む。
 * 形が合わない並び（内訳のない古い形を含む）は、内訳を空にする。
 */
function parseDemandFactors(stats: number[]): DemandFactor[][] {
  const none = (): DemandFactor[][] => Array.from({ length: ZONE_COUNT }, () => []);
  if (!validCityStats(stats)) {
    return none();
  }
  let at = CITY_STATS_LENGTH;
  return none().map(() => {
    const count = stats[at] ?? 0;
    const factors: DemandFactor[] = [];
    for (let i = 0; i < count; i++) {
      factors.push({ kind: stats[at + 1 + 2 * i] ?? 0, value: stats[at + 2 + 2 * i] ?? 0 });
    }
    at += 1 + 2 * count;
    return factors;
  });
}

export function parseCityStats(stats: number[]): CityStats {
  const at = (i: number) => stats[i] ?? 0;
  return {
    demandFactors: parseDemandFactors(stats),
    buildings: at(0),
    households: at(1),
    workplaces: at(2),
    demand: [at(3), at(4), at(5), at(6)],
    power: at(7),
    water: at(8),
    utilityDemand: at(9),
    powerShort: at(10),
    waterShort: at(11),
    powerIsolated: at(12),
    waterIsolated: at(13),
    powerGap: at(14),
    waterGap: at(15),
    mainPower: at(16),
    mainWater: at(17),
    mainDemand: at(18),
    powerOthers: at(19),
    waterOthers: at(20),
    powerPeople: at(21),
    waterPeople: at(22),
    peopleKnown: at(23) === 1,
    powerFacilityPeople: at(24),
    waterFacilityPeople: at(25),
    mainNetwork: at(26),
  };
}

/** 需要を棒の長さ（-1〜1）に直す。この値で棒が端まで届く。 */
export const DEMAND_FULL_SCALE = 100;

export function demandBar(value: number): number {
  return Math.max(-1, Math.min(1, value / DEMAND_FULL_SCALE));
}

/** 棒の入れ物の高さの半分（ピクセル）。真ん中の線が需要 0 である。 */
export const BAR_HALF_PX = 20;

/** 建物の数と、住める世帯と職場の上限の 1 行。 */
export function formatCityNumbers(stats: CityStats): string {
  const n = (v: number) => v.toLocaleString("ja-JP");
  return `建物 ${n(stats.buildings)} 棟・住める世帯 ${n(stats.households)}・職場の枠 ${n(stats.workplaces)}`;
}

/**
 * 余りを見る網ごとの供給量と需要。建物がある街では主の網の値で、ほかの網の分は足さない。
 * 建物がまだないうちは主の網がないので、施設の供給量の合計を出す。
 */
function mainView(stats: CityStats) {
  const hasMain = stats.mainDemand > 0;
  return {
    power: hasMain ? stats.mainPower : stats.power,
    water: hasMain ? stats.mainWater : stats.water,
    demand: hasMain ? stats.mainDemand : stats.utilityDemand,
    powerOthers: hasMain ? stats.powerOthers : 0,
    waterOthers: hasMain ? stats.waterOthers : 0,
  };
}

/** 余りが住民の何人分かの言い方。余りは 10 人の位に切り捨て、足りない人数は 10 人の位に切り上げて「約」を付ける。 */
export function formatPeople(people: number): string {
  const n = (v: number) => v.toLocaleString("ja-JP");
  if (people < 0) {
    // 足りない人数は多めに数える（中核と同じ向き）ので、10 人の位で切り上げる
    const short = -people;
    return `約 ${n(short >= 100 ? Math.ceil(short / 10) * 10 : short)} 人分足りない`;
  }
  return `あと 約 ${n(people >= 100 ? Math.floor(people / 10) * 10 : people)} 人分`;
}

/** 電気と水道の、主の網の供給量と需要の 1 行。ほかに道路網があれば、その数を添える。 */
export function formatUtilities(stats: CityStats): string {
  const n = (v: number) => v.toLocaleString("ja-JP");
  const m = mainView(stats);
  const others = (count: number) => (count > 0 ? `（ほかの網 ${n(count)}）` : "");
  const people = (v: number) => (stats.peopleKnown ? `（${formatPeople(v)}）` : "");
  return `電気 ${n(m.power)} / 需要 ${n(m.demand)}${people(stats.powerPeople)}${others(m.powerOthers)}・水道 ${n(m.water)} / 需要 ${n(m.demand)}${people(stats.waterPeople)}${others(m.waterOthers)}`;
}

/** 電気と水道の、需要の伸びから見積もった、余りが尽きるまで。 */
export type UtilityTrends = { power: HeadroomTrend | null; water: HeadroomTrend | null };

/**
 * 電気と水道の、余りが尽きるまでの手がかり。需要が伸びて余りが減っているときだけ、
 * 施設 1 基で増える人数と、このペースで足りなくなるまでの時間（時計の時間）を、種類ごとに 1 行返す。減っていないときは空。
 */
export function utilityTrendHints(stats: CityStats, trends: UtilityTrends): string[] {
  if (!stats.peopleKnown) {
    return [];
  }
  const hints: string[] = [];
  const add = (name: string, facility: string, onePeople: number, trend: HeadroomTrend | null) => {
    if (trend?.kind !== "shrinking" || onePeople <= 0) {
      return;
    }
    // 余りの人数と同じく、10 人の位に切り捨てて「約」を付ける
    const each = (onePeople >= 100 ? Math.floor(onePeople / 10) * 10 : onePeople).toLocaleString(
      "ja-JP",
    );
    hints.push(
      `${name}：${facility} 1 基で 約 ${each} 人分。このペースだと ${formatTimeUntilOut(trend.ticksUntilOut)}足りなくなる`,
    );
  };
  add("電気", "発電所", stats.powerFacilityPeople, trends.power);
  add("水道", "浄水場", stats.waterFacilityPeople, trends.water);
  return hints;
}

/** 主の網の需要の控え。renderCityStats が足していく。電気と水道で同じ値である。 */
let demandSeries: DemandSample[] = [];

/** 控えを捨てる。 */
export function resetUtilityTrends(): void {
  demandSeries = [];
}

/**
 * 控えに足して、いまの減り方を返す。1 日前の同じ時刻からの需要の伸びで見積もるので、
 * 街が 1 日たつまでと、読み込みなどで控えを捨てた直後の 1 日は null である。
 * tick は Worker の tick である。
 */
export function trackUtilityTrends(stats: CityStats, tick: number): UtilityTrends {
  const m = mainView(stats);
  demandSeries = pushDemandSample(demandSeries, tick, m.demand, stats.mainNetwork);
  return {
    power: demandTrend(demandSeries, tick, m.demand, m.power - m.demand, stats.mainNetwork),
    water: demandTrend(demandSeries, tick, m.demand, m.water - m.demand, stats.mainNetwork),
  };
}

/**
 * 電気か水道が届いていない建物の数と、その理由。足りているときは null。
 * 建物が建った直後は、届き方が決まる前の建物が数に入る。理由がないうちは知らせない。
 */
export type UtilityShortage = { headline: string; reasons: string[] };

export function utilityShortage(stats: CityStats, unsupplied: number): UtilityShortage | null {
  if (unsupplied <= 0) {
    return null;
  }
  const n = (v: number) => v.toLocaleString("ja-JP");
  const reasons: string[] = [];
  const add = (name: string, facility: string, gap: number, isolated: number) => {
    if (gap > 0) {
      reasons.push(`${name}が ${n(gap)} 足りない。${facility}を足す`);
    }
    if (isolated > 0) {
      reasons.push(
        `${facility}につながっていない道路網が ${n(isolated)} つある。${facility}を置くか、道路でつなぐ`,
      );
    }
  };
  add("電気", "発電所", stats.powerGap, stats.powerIsolated);
  add("水道", "浄水場", stats.waterGap, stats.waterIsolated);
  if (reasons.length === 0) {
    return null;
  }
  return {
    headline: `電気か水道が届いていない建物が ${n(unsupplied)} 棟ある`,
    reasons,
  };
}

/** 余りがこの割合（%）を切ったら、足りなくなる前に知らせる。 */
export const LOW_HEADROOM_PERCENT = 10;

/**
 * 届いていない建物がまだないが、電気か水道の余りが少なくなっているときの知らせ。余裕があるときは null。
 * 余りは主の網の供給量と需要で見る。ほかの網にある施設の分は足さない。
 */
export function utilityLowHeadroom(stats: CityStats): UtilityShortage | null {
  const reasons: string[] = [];
  const m = mainView(stats);
  const add = (name: string, facility: string, supply: number, people: number) => {
    if (supply <= 0 || m.demand > supply) {
      return;
    }
    const left = Math.floor(((supply - m.demand) * 100) / supply);
    if (left < LOW_HEADROOM_PERCENT) {
      const rest = stats.peopleKnown ? `（${formatPeople(people)}）` : "";
      reasons.push(`${name}の余りが ${left}% になった${rest}。${facility}を足す用意をする`);
    }
  };
  add("電気", "発電所", m.power, stats.powerPeople);
  add("水道", "浄水場", m.water, stats.waterPeople);
  if (reasons.length === 0) {
    return null;
  }
  return { headline: "電気か水道の余りが少なくなっている", reasons };
}

/** 足りない知らせと、足りていないほうの余りの知らせを並べる。どちらもなければ null。 */
export function utilityNotice(
  stats: CityStats,
  shortage: UtilityShortage | null,
): UtilityShortage | null {
  const low = utilityLowHeadroom(stats);
  if (shortage !== null && low !== null) {
    return { headline: shortage.headline, reasons: [...shortage.reasons, ...low.reasons] };
  }
  return shortage ?? low;
}

export type BarLayout = { value: number; top: number; height: number };

/** 住宅・商業・工業・オフィスの順の棒の位置。正の需要は真ん中の線から上へ、負の需要は下へ伸ばす。 */
export function demandBars(stats: CityStats): BarLayout[] {
  return stats.demand.map((value) => {
    const v = demandBar(value);
    const height = Math.abs(v) * BAR_HALF_PX;
    return { value, top: v >= 0 ? BAR_HALF_PX - height : BAR_HALF_PX, height };
  });
}

/** 住宅・商業・工業・オフィスの順の、区画の名前。 */
export const ZONE_NAMES = ["住宅", "商業", "工業", "オフィス"] as const;

/** chizu-core の DemandFactorKind の値。Rust から書き出した city-stats-layout.json の表である。 */
const KIND = FACTOR_KIND;

/**
 * 要因の言い方。寄与の符号で意味が変わる要因（幸福度、住宅の税）は言い方を変え、
 * それ以外は符号に関係のない言い方にする（正のときも負のときも同じ文で読める）。
 * 区画の種類（0 住宅、1 商業、2 工業、3 オフィス）によって、同じ要因でも指すものが違う。
 */
export function factorLabel(zone: number, kind: number, value: number): string {
  const positive = value > 0;
  switch (kind) {
    case KIND.base:
      return "基本の需要";
    case KIND.vacantJobs:
      return "空いている職場が多い";
    case KIND.happiness:
      return positive ? "幸福度が高い" : "幸福度が低い";
    case KIND.tax:
      if (zone === 0) {
        return positive ? "税が低い" : "税が高い";
      }
      return "税が高く、職場を増やしにくい";
    case KIND.vacantHomes:
      return "空き家が多い";
    case KIND.jobless:
      return "職のない住民が多い";
    case KIND.shoppers:
      return "住民の買い物に職場が要る";
    case KIND.workers:
      return "働く年齢の住民に職場が要る";
    case KIND.existing:
      return `いまの${ZONE_NAMES[zone] ?? ""}の職場`;
    default:
      return "そのほか";
  }
}

export type FactorLine = { label: string; value: number };

/** 寄与が 0 の要因を除き、寄与の絶対値の大きい順に並べる。同じ大きさなら正を先にする。 */
export function demandFactorLines(stats: CityStats, zone: number): FactorLine[] {
  return (stats.demandFactors[zone] ?? [])
    .filter((f) => f.value !== 0)
    .map((f) => ({ label: factorLabel(zone, f.kind, f.value), value: f.value }))
    .sort((a, b) => Math.abs(b.value) - Math.abs(a.value) || b.value - a.value);
}

/** 内訳を出す区画。押した区画は固定され、指を置いた区画は置いているあいだだけ出る。 */
export type FactorView = { pinned: number | null; hover: number | null };

export function shownZone(view: FactorView): number | null {
  return view.hover ?? view.pinned;
}

/**
 * 別の区画を押すとそちらに固定される。固定されている区画をもう一度押すと、指を置いたままでも隠れる
 * （指を置いている区画の表示も消す。置き直すと、また出る）。
 */
export function togglePinned(view: FactorView, zone: number): FactorView {
  if (view.pinned === zone) {
    return { pinned: null, hover: null };
  }
  return { ...view, pinned: zone };
}

let latestStats: CityStats | null = null;
let factorView: FactorView = { pinned: null, hover: null };

function formatSigned(value: number): string {
  return value > 0 ? `+${value.toLocaleString("ja-JP")}` : value.toLocaleString("ja-JP");
}

/** 内訳の欄を、いま出す区画に合わせて作り直す。出す区画がなければ隠す。 */
function renderFactors(): void {
  const panel = document.querySelector<HTMLElement>("#demand-factors");
  if (panel === null) {
    return;
  }
  const zone = shownZone(factorView);
  document.querySelectorAll<HTMLElement>("#demand .slot-button").forEach((button, i) => {
    button.setAttribute("aria-pressed", String(factorView.pinned === i));
  });
  if (zone === null || latestStats === null) {
    panel.hidden = true;
    panel.replaceChildren();
    return;
  }
  const lines = demandFactorLines(latestStats, zone);
  const total = latestStats.demand[zone] ?? 0;
  const widest = Math.max(1, ...lines.map((l) => Math.abs(l.value)));
  panel.hidden = false;
  panel.dataset.zone = String(zone);
  const head = document.createElement("div");
  head.className = "factors-head";
  head.textContent = `${ZONE_NAMES[zone]}の需要 ${formatSigned(total)}`;
  panel.replaceChildren(head);
  for (const line of lines) {
    const row = document.createElement("div");
    row.className = `factor ${line.value > 0 ? "plus" : "minus"}`;
    const label = document.createElement("span");
    label.textContent = line.label;
    const bar = document.createElement("span");
    bar.className = "factor-bar";
    bar.style.width = `${Math.max(2, Math.round((Math.abs(line.value) / widest) * 40))}px`;
    const value = document.createElement("span");
    value.className = "factor-value";
    value.textContent = formatSigned(line.value);
    row.append(label, bar, value);
    panel.append(row);
  }
}

/** 需要の棒を押すか指を置くと、その区画の内訳が出るようにする。 */
export function setupDemandFactors(): void {
  document.querySelectorAll<HTMLElement>("#demand .slot-button").forEach((button, i) => {
    button.addEventListener("click", () => {
      factorView = togglePinned(factorView, i);
      renderFactors();
    });
    button.addEventListener("pointerenter", () => {
      factorView = { ...factorView, hover: i };
      renderFactors();
    });
    button.addEventListener("pointerleave", () => {
      factorView = { ...factorView, hover: null };
      renderFactors();
    });
  });
}

/** 電気と水道の知らせ。通知センターに渡す。件数は揺れるので、key は足りないか余りが少ないかだけで決める。 */
/** 知らせの状態。どの理由が出ているか（電気か水道か、足りないか、孤立した網か、余りが少ないか）で決め、数は入れない。 */
export function utilityKey(reasons: string[], short: boolean): string {
  return `${short ? "short" : "low"}:${reasons.map((r) => r.replace(/[\d,]+/g, "#")).join("|")}`;
}

export function utilityNotices(stats: CityStats, unsupplied: number): Notice[] {
  const shortage = utilityShortage(stats, unsupplied);
  const shown = utilityNotice(stats, shortage);
  if (shown === null) {
    return [];
  }
  return [
    {
      id: "utility",
      key: utilityKey(shown.reasons, shortage !== null),
      headline: shown.headline,
      details: shown.reasons,
      jump: null,
    },
  ];
}

export function renderCityStats(stats: CityStats, unsupplied = 0, tick = 0): void {
  latestStats = stats;
  renderFactors();
  const numbers = document.querySelector<HTMLElement>("#city-numbers");
  if (numbers !== null) {
    numbers.textContent = formatCityNumbers(stats);
    // test:wasm が読む
    numbers.dataset.buildings = String(stats.buildings);
  }
  const utilities = document.querySelector<HTMLElement>("#city-utilities");
  if (utilities !== null) {
    utilities.textContent = formatUtilities(stats);
  }
  const shortage = utilityShortage(stats, unsupplied);
  utilities?.classList.toggle("short", shortage !== null);
  const hints = utilityTrendHints(stats, trackUtilityTrends(stats, tick));
  const hintBox = document.querySelector<HTMLElement>("#city-utility-hints");
  if (hintBox !== null) {
    hintBox.textContent = hints.join("\n");
    hintBox.hidden = hints.length === 0;
  }
  const layouts = demandBars(stats);
  document.querySelectorAll<HTMLElement>("#demand .bar").forEach((bar, i) => {
    const layout = layouts[i];
    if (layout === undefined) {
      return;
    }
    bar.style.height = `${layout.height}px`;
    bar.style.top = `${layout.top}px`;
    bar.title = String(layout.value);
    bar.dataset.demand = String(layout.value);
  });
}
