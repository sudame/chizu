// 電気と水道の余りが尽きるまでの見積もり。画面だけの値で、状態には入れない。
// 式は chizu-core の service::demand_trend と同じで、整数で切り捨てる。
// 余りの差ではなく、1 日前の同じ時刻からの需要の伸びを使う。施設を足しても需要は変わらないので、足した直後でも見積もれ、
// 朝の建設の波を 1 日の伸びと取り違えない。

/** 1 日の tick の数。chizu-core の TICKS_PER_DAY と同じ。 */
export const TICKS_PER_DAY = 7200;

/** 時計の 1 時間の tick の数。chizu-core の MIN_TREND_TICKS と同じ。 */
export const MIN_TREND_TICKS = TICKS_PER_DAY / 24;

/** 需要の控えを足す間隔（tick）。これより短いあいだは足さない。 */
const SAMPLE_GAP_TICKS = 60;

/**
 * 1 日前をはさむ 2 つの控えの最大の間隔（tick）。ゲーム内の 6 時間。
 * 画面は数十 tick ごとに控えを足すので、ふつうは超えない。超えるのは、タブを閉じていたあとなど。
 */
const MAX_BRACKET_TICKS = TICKS_PER_DAY / 4;

/** ある時点の、主の網の需要。 */
export type DemandSample = { tick: number; demand: number };

/**
 * 控えを足す。控えは、1 日前をはさむのに要る分（はさむ間隔を含めて 1 日強）だけ持つ。
 * tick が戻ったとき（読み込みや新しい街）は、そこから数え直す。
 */
export function pushDemandSample(
  series: readonly DemandSample[],
  tick: number,
  demand: number,
): DemandSample[] {
  const last = series[series.length - 1];
  if (last === undefined || tick < last.tick) {
    return [{ tick, demand }];
  }
  if (tick - last.tick < SAMPLE_GAP_TICKS) {
    return [...series];
  }
  const keepFrom = tick - TICKS_PER_DAY - MAX_BRACKET_TICKS;
  return [...series, { tick, demand }].filter((s) => s.tick >= keepFrom);
}

/**
 * tick の 1 日前の需要を、その tick をはさむ 2 つの控えを直線でつないで求める。
 * chizu-core の interpolate_demand と同じく、増減の小数は 0 向きに切り捨てる。
 * 街がまだ 1 日たっていないとき、はさむ控えがないとき、離れすぎているときは null。
 */
export function demandADayAgo(series: readonly DemandSample[], tick: number): number | null {
  const target = tick - TICKS_PER_DAY;
  if (target < 0) {
    return null;
  }
  let lo: DemandSample | undefined;
  let hi: DemandSample | undefined;
  for (const s of series) {
    if (s.tick <= target) {
      lo = s;
    }
    if (s.tick >= target && hi === undefined) {
      hi = s;
    }
  }
  if (lo === undefined || hi === undefined || hi.tick - lo.tick > MAX_BRACKET_TICKS) {
    return null;
  }
  if (hi.tick === lo.tick) {
    return lo.demand === hi.demand ? lo.demand : null;
  }
  return (
    lo.demand + Math.trunc(((hi.demand - lo.demand) * (target - lo.tick)) / (hi.tick - lo.tick))
  );
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
): HeadroomTrend | null {
  const before = demandADayAgo(series, tick);
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
