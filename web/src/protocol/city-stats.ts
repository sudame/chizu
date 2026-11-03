// chizu-wasm の city_stats の並び。固定の並びの個数と要因の番号は city-stats-layout.json にあり、
// Rust 側のテストがこのファイルと突き合わせる。
import layout from "./city-stats-layout.json" with { type: "json" };

/** 固定の並びの個数。そのあとに需要の内訳が続く。 */
export const CITY_STATS_LENGTH: number = layout.fixedLength;

/** 区画の種類の数（住宅・商業・工業・オフィス）。 */
export const ZONE_COUNT: number = layout.zones;

/** 需要の要因の番号。chizu-core の DemandFactorKind の値。 */
export const FACTOR_KIND = layout.factorKinds;

/**
 * 並びの形が合っているか。固定の並びのあとに、区画の種類ごとに「件数、[要因の番号, 寄与] を件数だけ」が続き、
 * 長さがちょうど合うこと。件数は 0 以上の整数でなければならない。
 */
export function validCityStats(stats: number[]): boolean {
  if (stats.length < CITY_STATS_LENGTH || !stats.every((x) => Number.isFinite(x))) {
    return false;
  }
  let at = CITY_STATS_LENGTH;
  for (let zone = 0; zone < ZONE_COUNT; zone++) {
    const count = stats[at];
    if (count === undefined || !Number.isInteger(count) || count < 0) {
      return false;
    }
    at += 1 + 2 * count;
  }
  return at === stats.length;
}
