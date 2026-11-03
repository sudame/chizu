/**
 * 速さの段は、等速の何倍かで持つ。1 倍は 1 秒に TICKS_PER_SECOND tick で、1 日（7,200 tick）を 24 分で回す。
 * 帯に出す段と、詳細の欄に出す段を合わせて、1 日 24 分（1 倍）から 5 秒（288 倍）までを分ける。
 */

/** 帯に出す速さ。眺める（1 日 24 分）、ふつうに遊ぶ（4 分）、早送り（30 秒）。 */
export const BAR_SPEEDS = [1, 6, 48] as const;
/** 詳細の欄に出す速さ。最高速（1 日 5 秒）。chizu-core の SimSpeed::MAX と同じ。 */
export const EXTRA_SPEEDS = [288] as const;

export function describeSpeed(speed: number): string {
  return `速度 ${speed} 倍`;
}

/** 1 秒に進める tick 数。 */
export function tickRate(perSecond: number, speed: number): number {
  return perSecond * speed;
}

/**
 * 1 フレームぶんの tick 数を決める。budget は端数の持ち越しで、1 に満たないフレームは 0 tick になる。
 * 60 フレーム毎秒で、1 倍は 12 フレームに 1 tick 進む。
 */
export function stepBudget(
  budget: number,
  elapsedMs: number,
  rate: number,
  paused: boolean,
  maxBacklogSeconds: number,
): { ticks: number; budget: number } {
  const next = paused ? 0 : Math.min(budget + (elapsedMs / 1000) * rate, rate * maxBacklogSeconds);
  const ticks = Math.floor(next);
  return { ticks, budget: next - ticks };
}
