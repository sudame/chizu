// 街の数字の表示。建物の数、世帯と職場の上限、区画の種類ごとの需要（CS2 の RCI のような棒）を出す。

/** chizu-wasm の city_stats を読んだもの。 */
export type CityStats = {
  buildings: number;
  households: number;
  workplaces: number;
  /** 住宅・商業・工業・オフィスの順。正なら建物が建つ。 */
  demand: [number, number, number, number];
};

export function parseCityStats(stats: number[]): CityStats {
  const at = (i: number) => stats[i] ?? 0;
  return {
    buildings: at(0),
    households: at(1),
    workplaces: at(2),
    demand: [at(3), at(4), at(5), at(6)],
  };
}

/** 需要を棒の長さ（-1〜1）に直す。この値で棒が端まで届く。 */
export const DEMAND_FULL_SCALE = 100;

export function demandBar(value: number): number {
  return Math.max(-1, Math.min(1, value / DEMAND_FULL_SCALE));
}

const BAR_HALF_PX = 20;

export function renderCityStats(stats: CityStats): void {
  const numbers = document.querySelector<HTMLElement>("#city-numbers");
  if (numbers !== null) {
    const n = (v: number) => v.toLocaleString("ja-JP");
    numbers.textContent = `建物 ${n(stats.buildings)} 棟・世帯 ${n(stats.households)}・職場 ${n(stats.workplaces)}`;
    // test:wasm が読む
    numbers.dataset.buildings = String(stats.buildings);
  }
  const bars = document.querySelectorAll<HTMLElement>("#demand .bar");
  bars.forEach((bar, i) => {
    const v = demandBar(stats.demand[i] ?? 0);
    const height = Math.abs(v) * BAR_HALF_PX;
    // 正の需要は真ん中の線から上へ、負の需要は下へ伸ばす
    bar.style.height = `${height}px`;
    bar.style.top = `${v >= 0 ? BAR_HALF_PX - height : BAR_HALF_PX}px`;
    bar.title = String(stats.demand[i] ?? 0);
    bar.dataset.demand = String(stats.demand[i] ?? 0);
  });
}
