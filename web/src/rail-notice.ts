// 鉄道の知らせ。遊ぶ人が打つ手を知るためのもので、通知センター（notice-center.ts）に出す。
// 出す知らせは、走れない路線、止まっている路線、混みすぎる路線、待ちきれずに歩いた人が多い駅の 4 つである。
// つまみの値（混みすぎとみなす乗車率、歩いた人の数）は、docs/design.md の 3.10 の「作る順の 8 で決めた細部」にある。

import { capZeroReason } from "./line-panel.ts";
import { lineName, type RailLineInfo } from "./map/lines.ts";
import type { Tile } from "./map/roads.ts";
import type { RailStats, StationStat } from "./map/transit.ts";
import type { Notice } from "./notice-center.ts";
import { TICKS_PER_DAY } from "./utility-trend.ts";

/** 「混みすぎる」とみなす、いちばん混んだ列車の乗車率（‰）。乗りきれない人が出始める手前である。 */
export const CROWDED_PERMILLE = 900;

/** 「待ちきれずに歩いた人が多い」とみなす、1 日あたりの人数。 */
export const GAVE_UP_PER_DAY = 20;

/** 知らせの詳しい文と、移る先に並べる駅の数の上限。 */
export const LISTED_STATIONS = 3;

/** 歩いた人の数を見る、直近の長さ（tick）。ゲーム内の 1 日である。 */
export const GAVE_UP_WINDOW = TICKS_PER_DAY;

/** 1 日あたりの歩いた人の数を求めるのに、少なくとも見ている長さ（tick）。短すぎる区間の比は揺れる。 */
export const GAVE_UP_MIN_SPAN = TICKS_PER_DAY / 4;

/**
 * 待ちきれずに歩いた人の積み上げから、1 日あたりの数を求める。
 * 直近 1 日の差を取る。見ている長さが短いうちは null にし、tick が戻ったら（読み込み、新しい街）捨てて数え直す。
 */
export class GaveUpWindow {
  #samples: { tick: number; gaveUp: number }[] = [];

  update(tick: number, gaveUp: number): void {
    const last = this.#samples[this.#samples.length - 1];
    if (last !== undefined && (tick < last.tick || gaveUp < last.gaveUp)) {
      this.#samples = [];
    }
    if (last === undefined || tick > last.tick) {
      this.#samples.push({ tick, gaveUp });
    }
    // 1 日より古い標本は、1 日の端をまたぐ 1 つだけ残して捨てる
    while ((this.#samples[1]?.tick ?? Number.POSITIVE_INFINITY) <= tick - GAVE_UP_WINDOW) {
      this.#samples.shift();
    }
  }

  /** 1 日あたりの歩いた人の数。見ている長さが足りなければ null。 */
  perDay(): number | null {
    const first = this.#samples[0];
    const last = this.#samples[this.#samples.length - 1];
    if (first === undefined || last === undefined || last.tick - first.tick < GAVE_UP_MIN_SPAN) {
      return null;
    }
    return ((last.gaveUp - first.gaveUp) * TICKS_PER_DAY) / (last.tick - first.tick);
  }
}

const n = (v: number) => v.toLocaleString("ja-JP");
const percent = (permille: number) => `${Math.round(permille / 10)}%`;

/** 路線の読み取り用の数を、路線の ID で引けるようにする。 */
function lineStatOf(stats: RailStats | null, line: RailLineInfo) {
  return stats?.lines.find((l) => l.index === line.index && l.generation === line.generation);
}

/** 路線の最初の駅（敷地の min のマス）。知らせから地図を移す先にする。 */
function firstStation(line: RailLineInfo): Tile[] {
  const first = line.stations[0];
  return first === undefined ? [] : [first];
}

/** 路線が混みすぎているか。走る列車があり、いちばん混んだ列車が目安を超えるか、直前の列車に乗りきれなかった。 */
export function isCrowded(line: RailLineInfo, stats: RailStats | null): boolean {
  const s = lineStatOf(stats, line);
  return (
    s !== undefined &&
    s.running > 0 &&
    (s.loadPermille >= CROWDED_PERMILLE || s.leftBehindStops > 0)
  );
}

/** 待っている人が多い順の駅。待っている人のいない駅は含めない。同じ数なら ID の順。 */
export function busiestStations(stats: RailStats, limit = LISTED_STATIONS): StationStat[] {
  return stats.stations
    .filter((s) => s.waiting > 0)
    .sort((a, b) => b.waiting - a.waiting || a.index - b.index || a.generation - b.generation)
    .slice(0, limit);
}

/**
 * 鉄道の知らせ。`gaveUpPerDay` は直近 1 日で待ちきれずに歩いた人の数（見ている長さが足りなければ null）、
 * `nameOf` は駅の敷地の min のマスから駅の名前を引く。
 */
export function railNotices(
  lines: readonly RailLineInfo[],
  stats: RailStats | null,
  gaveUpPerDay: number | null,
  nameOf: (station: Tile) => string,
): Notice[] {
  const notices: Notice[] = [];
  const key = (list: readonly RailLineInfo[]) =>
    list.map((l) => `${l.index}.${l.generation}`).join(",");

  const stopped = lines.filter((l) => l.stopped);
  if (stopped.length > 0) {
    notices.push({
      id: "rail-stopped",
      key: key(stopped),
      headline: `止まっている路線がある（${stopped.map((l) => lineName(l.index)).join("、")}）`,
      details: ["線路がつながっていないので、列車が走れない。線路を引いてつなぐか、路線を引き直す"],
      jump: { label: "駅へ移る", targets: stopped.flatMap(firstStation) },
    });
  }

  const halted = lines.filter((l) => !l.stopped && l.cap === 0);
  if (halted.length > 0) {
    notices.push({
      id: "rail-halted",
      key: key(halted),
      headline: `列車を走らせられない路線がある（${halted.map((l) => lineName(l.index)).join("、")}）`,
      details: halted.map((l) => `${lineName(l.index)}：${capZeroReason(l)}`),
      jump: { label: "駅へ移る", targets: halted.flatMap(firstStation) },
    });
  }

  const crowded = lines.filter((l) => isCrowded(l, stats));
  if (crowded.length > 0 && stats !== null) {
    const details = crowded.map((l) => {
      const s = lineStatOf(stats, l);
      const room =
        l.trains < l.cap
          ? "本数を増やすと解ける"
          : "本数の上限に達している。線路を足して待てる所を増やすか、路線を分ける";
      return `${lineName(l.index)}：いちばん混んだ列車が ${percent(s?.loadPermille ?? 0)}。${room}`;
    });
    notices.push({
      id: "rail-crowded",
      key: key(crowded),
      headline: `列車が混みすぎている路線がある（${crowded.map((l) => lineName(l.index)).join("、")}）`,
      details,
      jump: { label: "駅へ移る", targets: crowded.flatMap(firstStation) },
    });
  }

  if (stats !== null && gaveUpPerDay !== null && gaveUpPerDay >= GAVE_UP_PER_DAY) {
    const busy = busiestStations(stats);
    notices.push({
      id: "rail-gave-up",
      // 人数のように揺れる値は key に入れない
      key: "gave-up",
      headline: "駅で待ちきれずに歩く人が多い",
      details: [
        `1 日に約 ${n(Math.round(gaveUpPerDay))} 人が、待ちきれずに歩いている。本数を増やすと減る`,
        ...busy.map((s) => `${nameOf(s.min)}：${n(s.waiting)} 人が待っている`),
      ],
      jump:
        busy.length === 0
          ? null
          : { label: "駅へ移る", targets: busy.map((s) => s.min), ordered: true },
    });
  }
  return notices.map((nt) =>
    nt.jump !== null && nt.jump.targets.length === 0 ? { ...nt, jump: null } : nt,
  );
}
