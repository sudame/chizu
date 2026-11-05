// 電気と水道の余り（住民の何人分か）の減り方。画面だけの見積もりで、状態には入れない。
// 式は chizu-core の service::headroom_trend と同じで、整数で切り捨てる。

/** 1 日の tick の数。chizu-core の TICKS_PER_DAY と同じ。 */
export const TICKS_PER_DAY = 7200;

/** 減り方を求めるのに要る最短の間隔（tick）。ゲーム内の 1 時間。chizu-core の MIN_TREND_TICKS と同じ。 */
export const MIN_TREND_TICKS = TICKS_PER_DAY / 24;

/** 余りの控えを足す間隔（tick）。これより短いあいだは足さない。 */
const SAMPLE_GAP_TICKS = 60;

/**
 * 余りがこれ（人数）より増えたら、施設を足したなどで流れが変わったとみなし、前の控えを捨てる。
 * 人口の端数の揺れでは捨てない大きさにする。
 */
const RESET_RISE_PEOPLE = 50;

/** ある時点の、余りの人数。 */
export type HeadroomSample = { tick: number; people: number };

/**
 * 控えを足す。控えは直近 1 日分だけ持つ。
 * tick が戻ったとき（読み込みや新しい街）と、余りが大きく増えたとき（施設を足したとき）は、そこから数え直す。
 */
export function pushHeadroomSample(
  series: readonly HeadroomSample[],
  tick: number,
  people: number,
): HeadroomSample[] {
  const last = series[series.length - 1];
  if (last === undefined || tick < last.tick || people > last.people + RESET_RISE_PEOPLE) {
    return [{ tick, people }];
  }
  if (tick - last.tick < SAMPLE_GAP_TICKS) {
    return [...series];
  }
  return [...series, { tick, people }].filter((s) => s.tick >= tick - TICKS_PER_DAY);
}

export type HeadroomTrend =
  | { kind: "short" }
  | { kind: "notShrinking" }
  | { kind: "shrinking"; dropPerDay: number; ticksUntilOut: number };

/**
 * 控えのいちばん古い値といまの値から、このペースの先を見積もる。
 * 控えが 1 時間分に満たないとき、控えがないときは null。
 */
export function headroomTrend(
  series: readonly HeadroomSample[],
  tick: number,
  people: number,
): HeadroomTrend | null {
  const first = series[0];
  if (first === undefined || tick - first.tick < MIN_TREND_TICKS) {
    return null;
  }
  if (people < 0) {
    return { kind: "short" };
  }
  const elapsed = tick - first.tick;
  const drop = first.people - people;
  if (drop <= 0) {
    return { kind: "notShrinking" };
  }
  return {
    kind: "shrinking",
    dropPerDay: Math.floor((drop * TICKS_PER_DAY) / elapsed),
    ticksUntilOut: Math.floor((people * elapsed) / drop),
  };
}

/** 尽きるまでの時間の言い方。時計の時間（ゲーム内）で言い、1 時間未満は「1 時間もたたずに」とする。 */
export function formatTimeUntilOut(ticks: number): string {
  const hours = ticks / MIN_TREND_TICKS;
  if (hours < 1) {
    return "1 時間もたたずに";
  }
  if (hours >= 48) {
    return `約 ${Math.round(hours / 24).toLocaleString("ja-JP")} 日で`;
  }
  return `約 ${Math.round(hours).toLocaleString("ja-JP")} 時間で`;
}
