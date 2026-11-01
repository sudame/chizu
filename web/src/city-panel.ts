// 街の数字の表示。建物の数、世帯と職場の上限、区画の種類ごとの需要（CS2 の RCI のような棒）を出す。

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
};

export function parseCityStats(stats: number[]): CityStats {
  const at = (i: number) => stats[i] ?? 0;
  return {
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

/** 電気と水道の供給量と需要の 1 行。 */
export function formatUtilities(stats: CityStats): string {
  const n = (v: number) => v.toLocaleString("ja-JP");
  const d = n(stats.utilityDemand);
  return `電気 ${n(stats.power)} / 需要 ${d}・水道 ${n(stats.water)} / 需要 ${d}`;
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

export type BarLayout = { value: number; top: number; height: number };

/** 住宅・商業・工業・オフィスの順の棒の位置。正の需要は真ん中の線から上へ、負の需要は下へ伸ばす。 */
export function demandBars(stats: CityStats): BarLayout[] {
  return stats.demand.map((value) => {
    const v = demandBar(value);
    const height = Math.abs(v) * BAR_HALF_PX;
    return { value, top: v >= 0 ? BAR_HALF_PX - height : BAR_HALF_PX, height };
  });
}

export function renderCityStats(stats: CityStats, unsupplied = 0): void {
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
  const notice = document.querySelector<HTMLElement>("#city-shortage");
  if (notice !== null) {
    notice.hidden = shortage === null;
    notice.dataset.unsupplied = String(unsupplied);
    notice.replaceChildren();
    if (shortage !== null) {
      const head = document.createElement("strong");
      head.textContent = shortage.headline;
      notice.append(head);
      for (const reason of shortage.reasons) {
        const line = document.createElement("div");
        line.textContent = reason;
        notice.append(line);
      }
    }
  }
  utilities?.classList.toggle("short", shortage !== null);
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
