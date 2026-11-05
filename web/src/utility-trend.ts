// 電気と水道の余りが尽きるまでの見積もり。画面だけの値で、状態には入れない。
// 式は chizu-core の service::demand_trend と同じで、整数で切り捨てる。
// 余りの差ではなく、1 日前の同じ時刻からの需要の伸びを使う。施設を足しても需要は変わらないので、足した直後でも見積もれ、
// 朝の建設の波を 1 日の伸びと取り違えない。主の網が替わったときは、需要が飛ぶので見積もらない。

/** 1 日の tick の数。chizu-core の TICKS_PER_DAY と同じ。 */
export const TICKS_PER_DAY = 7200;

/** 時計の 1 時間の tick の数。chizu-core の MIN_TREND_TICKS と同じ。 */
export const MIN_TREND_TICKS = TICKS_PER_DAY / 24;

/** 需要の控えを足す間隔（tick）。これより短いあいだは足さない（古いほうを残す）。 */
const SAMPLE_GAP_TICKS = 60;

/**
 * ある時点の、主の網の需要と、その網を見分ける印（chizu-core の SupplyBalance::network_mark）。
 * 主の網が別の網に替わると需要が飛ぶので、印が違う控えは比べない。
 */
export type DemandSample = { tick: number; demand: number; net: number };

/**
 * 控えを足す。画面に届くのは数字が変わったときだけなので、控えのあいだは値が変わっていない。
 * そのため、1 日前の需要は「1 日前以前でいちばん新しい控えの値」で、間の長さに上限は置かない。
 * 1 日前より古い控えは、1 日前以前でいちばん新しい 1 つだけ残して捨てる。
 * tick が戻ったとき（読み込みや新しい街）は、そこから数え直す。
 */
export function pushDemandSample(
  series: readonly DemandSample[],
  tick: number,
  demand: number,
  net: number,
): DemandSample[] {
  const last = series[series.length - 1];
  if (last === undefined || tick < last.tick) {
    return [{ tick, demand, net }];
  }
  // 近すぎる数字は、末尾の控えを新しい値で上書きする（静かな時間の前の最後の変化を残すため）。
  // ただし末尾が窓の先頭（1 日前の値を見つけるのに要る控え）なら、上書きせずに残して後ろに足す。
  const all =
    tick - last.tick < SAMPLE_GAP_TICKS && series.length > 1
      ? [...series.slice(0, -1), { tick, demand, net }]
      : [...series, { tick, demand, net }];
  let from = 0;
  all.forEach((s, i) => {
    if (s.tick <= tick - TICKS_PER_DAY) {
      from = i;
    }
  });
  return all.slice(from);
}

/**
 * tick の 1 日前の需要。1 日前以前でいちばん新しい控えの値である（控えのあいだは値が変わっていない）。
 * 街がまだ 1 日たっていないとき、そのような控えがないとき、
 * その控えの網の印が `net`（いまの主の網）と違うとき（主の網が替わった）は null。
 */
export function demandADayAgo(
  series: readonly DemandSample[],
  tick: number,
  net: number,
): number | null {
  const target = tick - TICKS_PER_DAY;
  if (target < 0) {
    return null;
  }
  let lo: DemandSample | undefined;
  for (const s of series) {
    if (s.tick <= target) {
      lo = s;
    }
  }
  if (lo === undefined || lo.net !== net) {
    return null;
  }
  return lo.demand;
}

export type HeadroomTrend =
  | { kind: "short" }
  | { kind: "notShrinking" }
  | { kind: "shrinking"; growthPerDay: number; ticksUntilOut: number };

/**
 * 余り（供給量 − 需要。人数ではなく、供給量と同じ単位）と、1 日前の同じ時刻からの需要の伸びから、このペースの先を見積もる。
 * 1 日前の需要が決まらないときは null。
 */
export function demandTrend(
  series: readonly DemandSample[],
  tick: number,
  demand: number,
  headroom: number,
  net: number,
): HeadroomTrend | null {
  const before = demandADayAgo(series, tick, net);
  if (before === null) {
    return null;
  }
  if (headroom < 0) {
    return { kind: "short" };
  }
  const growth = demand - before;
  if (growth <= 0) {
    return { kind: "notShrinking" };
  }
  return {
    kind: "shrinking",
    growthPerDay: growth,
    ticksUntilOut: Math.floor((headroom * TICKS_PER_DAY) / growth),
  };
}

/** 尽きるまでの時間の言い方。時計の時間（ゲーム内）で言い、1 時間未満は「1 時間もたたずに」とする。 */
export function formatTimeUntilOut(ticks: number): string {
  const hours = ticks / MIN_TREND_TICKS;
  if (hours < 1) {
    return "1 時間もたたずに";
  }
  if (hours >= 48) {
    return `約 ${Math.floor(hours / 24).toLocaleString("ja-JP")} 日で`;
  }
  return `約 ${Math.floor(hours).toLocaleString("ja-JP")} 時間で`;
}
