/**
 * 1 倍より遅い速度は、中核の速度を 1 のままにして、Worker が tick を進める頻度だけを落として作る。
 * 記録に残るのは整数の速度だけなので、状態ハッシュと保存の形式は変わらない。
 */

export interface SplitSpeed {
  /** 中核に渡す速度（SetSimSpeed）。 */
  core: number;
  /** Worker が tick を進める頻度にかける倍率。1 以下。 */
  pace: number;
}

/** 画面で選んだ倍率を、中核の速度と Worker の倍率に分ける。 */
export function splitSpeed(speed: number): SplitSpeed {
  return speed < 1 ? { core: 1, pace: speed } : { core: speed, pace: 1 };
}

/** 画面に出す倍率。遅くしているあいだは中核の速度が 1 なので、Worker の倍率のほうを出す。 */
export function shownSpeed(core: number, pace: number): number {
  return pace < 1 ? pace : core;
}

export function describeSpeed(speed: number): string {
  return `速度 ${speed} 倍`;
}

/**
 * 1 フレームぶんの tick 数を決める。budget は端数の持ち越しで、1 に満たないフレームは 0 tick になる。
 * 60 フレーム毎秒で、1 倍は 2 フレームに 1 tick、0.5 倍は 4 フレームに 1 tick、0.25 倍は 8 フレームに 1 tick 進む。
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
