// 道路を引く前の見積もりに添える、踏切と路線の上限への響き。
// 中核の答え（chizu-wasm の road_probe）は、先頭が新しくできる踏切の数、続いて本数の上限が下がる路線ごとに
// `[添え字, 世代, 前の上限, 後の上限]` である。道路を引いて線路が割れると待てる所が減り、路線の上限が下がる。

import { lineName } from "./map/lines.ts";

export const ROAD_PROBE_LINE = 4;

export type LoweredLine = { index: number; generation: number; before: number; after: number };

export type RoadProbe = {
  /** 新しくできる踏切の数。 */
  crossings: number;
  /** 本数の上限が下がる路線。 */
  lowered: LoweredLine[];
};

/** chizu-wasm の road_probe の数の並びを読む。空（線路がない、引けない）なら null。 */
export function parseRoadProbe(data: ArrayLike<number>): RoadProbe | null {
  if (data.length === 0) {
    return null;
  }
  const lowered: LoweredLine[] = [];
  for (let i = 1; i + ROAD_PROBE_LINE <= data.length; i += ROAD_PROBE_LINE) {
    lowered.push({
      index: data[i] ?? 0,
      generation: data[i + 1] ?? 0,
      before: data[i + 2] ?? 0,
      after: data[i + 3] ?? 0,
    });
  }
  return { crossings: data[0] ?? 0, lowered };
}

/** 見積もりの文。踏切ができないなら空。 */
export function roadProbeText(probe: RoadProbe | null): string {
  if (probe === null || probe.crossings === 0) {
    return "";
  }
  const made = probe.crossings === 1 ? "踏切ができ" : `踏切が ${probe.crossings} 本でき`;
  if (probe.lowered.length === 0) {
    return `${made}る`;
  }
  const lines = probe.lowered.map(
    (l) => `${lineName(l.index)}の本数の上限が ${l.before} から ${l.after} に下がる`,
  );
  return `${made}、${lines.join("、")}`;
}
