// 鉄道の知らせ。遊ぶ人が打つ手を知るためのもので、通知センター（notice-center.ts）に出す。
// 出す知らせは、走れない路線、止まっている路線、混みすぎる路線、待ちきれずに歩いた人が多い駅、
// 乗る人のいない駅、徒歩圏が隣の駅と大きく重なる駅の 6 つである。
// つまみの値（混みすぎとみなす乗車率、歩いた人の数、乗る人を見る日数、重なりの割合）は、
// docs/design.md の 3.10 の「作る順の 8 で決めた細部」にある。

import { capZeroReason } from "./line-panel.ts";
import { lineName, type RailLineInfo } from "./map/lines.ts";
import type { Tile } from "./map/roads.ts";
import { STATION_WALK_RADIUS } from "./map/tools.ts";
import { type RailStats, type StationStat, stationKey } from "./map/transit.ts";
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
 * 直近 1 日の差を取る。見ている長さが短いうちは null にする。別の街になるときは `reset` で捨てて数え直す
 * （tick も積み上げも前の街より大きい街を開くと、標本だけでは別の街と気づけない）。tick や積み上げが戻ったときも捨てる。
 */
export class GaveUpWindow {
  #samples: { tick: number; gaveUp: number }[] = [];

  /** 標本を捨てる。別の街（読み込み、新しい街）になるときに呼ぶ。 */
  reset(): void {
    this.#samples = [];
  }

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

/** 「乗る人がいない」かを見る、直近の長さ（日）。画面を開いてからこの日数たつまでは判断しない。 */
export const UNUSED_DAYS = 3;

/** 「隣の駅と大きく重なる」とみなす、徒歩圏の世帯のうち、ほかの駅の徒歩圏にも入る世帯の割合（‰）。chizu-cli の overlapPermille と同じ物差しである。 */
export const OVERLAP_PERMILLE = 800;

/** 乗った人の標本を取る最短の間隔（tick）。 */
export const RIDE_SAMPLE_GAP = TICKS_PER_DAY / 12;

type RideSample = { tick: number; boarded: Map<string, number> };

/**
 * 駅ごとの直近の乗った人の数。中核が持つのは始まりからの積み上げだけなので、`UNUSED_DAYS` 日前の標本との差を取る。
 * 駅が `UNUSED_DAYS` 日より前から見えていなければ、その駅は null（まだ判断できない）。
 * 別の街になるときは `reset` で捨てる。tick や積み上げが戻ったときも捨てる。
 */
export class RecentRides {
  #samples: RideSample[] = [];
  #latest: RideSample | null = null;
  #firstSeen = new Map<string, number>();

  reset(): void {
    this.#samples = [];
    this.#latest = null;
    this.#firstSeen = new Map();
  }

  update(stats: RailStats): void {
    const boarded = new Map(stats.stations.map((st) => [stationKey(st), st.boarded]));
    const last = this.#latest;
    if (
      last !== null &&
      (stats.tick < last.tick || [...boarded].some(([k, v]) => v < (last.boarded.get(k) ?? 0)))
    ) {
      this.reset();
    }
    for (const k of [...this.#firstSeen.keys()]) {
      if (!boarded.has(k)) {
        this.#firstSeen.delete(k);
      }
    }
    for (const k of boarded.keys()) {
      if (!this.#firstSeen.has(k)) {
        this.#firstSeen.set(k, stats.tick);
      }
    }
    const sample = { tick: stats.tick, boarded };
    this.#latest = sample;
    const tail = this.#samples[this.#samples.length - 1];
    if (tail === undefined || stats.tick - tail.tick >= RIDE_SAMPLE_GAP) {
      this.#samples.push(sample);
    }
    // 窓より古い標本は、窓の端をまたぐ 1 つだけ残して捨てる
    const window = UNUSED_DAYS * TICKS_PER_DAY;
    while ((this.#samples[1]?.tick ?? Number.POSITIVE_INFINITY) <= stats.tick - window) {
      this.#samples.shift();
    }
  }

  /** 直近 `UNUSED_DAYS` 日に乗った人の数。まだ判断できなければ null。 */
  boardedIn(station: { index: number; generation: number }): number | null {
    const latest = this.#latest;
    const base = this.#samples[0];
    if (latest === null || base === undefined || base.tick > latest.tick - UNUSED_DAYS * TICKS_PER_DAY) {
      return null;
    }
    const key = stationKey(station);
    const seen = this.#firstSeen.get(key);
    const was = base.boarded.get(key);
    const now = latest.boarded.get(key);
    if (seen === undefined || seen > base.tick || was === undefined || now === undefined) {
      return null;
    }
    return now - was;
  }
}

/** 駅の徒歩圏のうち、ほかの駅の徒歩圏にも入る世帯の割合（‰）。世帯がいなければ 0。 */
export function overlapPermille(s: StationStat): number {
  return s.households === 0 ? 0 : Math.floor((s.shared * 1000) / s.households);
}

const near = (a: StationStat, b: StationStat) =>
  (a.door.x - b.door.x) ** 2 + (a.door.y - b.door.y) ** 2 <=
  (2 * STATION_WALK_RADIUS) ** 2;

/**
 * 徒歩圏が隣の駅と大きく重なり、その隣の駅より使われていない駅と、そのときの隣の駅。
 * 使われ方は、直近の乗った人が分かっていればそれ、分からなければ始まりからの積み上げで比べる。同じなら世帯の多いほう、ID の若いほうを残す。
 */
export function overlappedStations(
  stats: RailStats,
  recent: Pick<RecentRides, "boardedIn"> | null,
): { station: StationStat; partner: StationStat }[] {
  const use = (s: StationStat, byRecent: boolean) =>
    byRecent ? (recent?.boardedIn(s) ?? 0) : s.boarded;
  const out: { station: StationStat; partner: StationStat }[] = [];
  for (const s of stats.stations) {
    if (!s.hasDoor || s.households === 0 || overlapPermille(s) < OVERLAP_PERMILLE) {
      continue;
    }
    let partner: StationStat | null = null;
    for (const o of stats.stations) {
      if (o === s || !o.hasDoor || !near(s, o)) {
        continue;
      }
      const byRecent = recent?.boardedIn(s) != null && recent.boardedIn(o) != null;
      const rank = (x: StationStat) => [use(x, byRecent), x.households, -x.index];
      const [a, b] = [rank(o), rank(s)];
      const better = a.findIndex((v, i) => v !== b[i]);
      if (better >= 0 && (a[better] ?? 0) > (b[better] ?? 0)) {
        const cur: StationStat | null = partner;
        if (cur === null || o.households > cur.households) {
          partner = o;
        }
      }
    }
    if (partner !== null) {
      out.push({ station: s, partner });
    }
  }
  return out;
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
  recent: Pick<RecentRides, "boardedIn"> | null = null,
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
  if (stats !== null && recent !== null) {
    const unused = stats.stations.filter((st) => recent.boardedIn(st) === 0);
    if (unused.length > 0) {
      const onLine = (st: StationStat) =>
        lines.some((l) => l.stations.some((t) => t.x === st.min.x && t.y === st.min.y));
      const hint = (st: StationStat) => {
        if (!onLine(st)) {
          return "どの路線にも入っていない。路線に入れるか、取り壊して維持費を減らす";
        }
        if (!st.hasDoor) {
          return "道路に面していないので、歩いて来られない。道路に面した所に置き直す";
        }
        if (st.households === 0) {
          return "徒歩圏に家がない。家のある所へ置き直すか、取り壊して維持費を減らす";
        }
        if (overlapPermille(st) >= OVERLAP_PERMILLE) {
          return "徒歩圏が隣の駅とほぼ重なり、そちらが使われている。取り壊して維持費を減らすか、離れた所へ置き直す";
        }
        return "家からの道が通じているか、列車が来ているかを確かめる";
      };
      notices.push({
        id: "rail-unused",
        key: unused.map(stationKey).join(","),
        headline: `乗る人のいない駅がある（${unused.map((st) => nameOf(st.min)).join("、")}）`,
        details: unused.map(
          (st) => `${nameOf(st.min)}：直近 ${UNUSED_DAYS} 日、乗る人が 0 人。${hint(st)}`,
        ),
        jump: { label: "駅へ移る", targets: unused.map((st) => st.min), ordered: true },
      });
    }
    const unusedKeys = new Set(unused.map(stationKey));
    const overlapped = overlappedStations(stats, recent).filter(
      (o) => !unusedKeys.has(stationKey(o.station)),
    );
    if (overlapped.length > 0) {
      notices.push({
        id: "rail-overlap",
        key: overlapped.map((o) => stationKey(o.station)).join(","),
        headline: `徒歩圏が隣の駅と大きく重なる駅がある（${overlapped
          .map((o) => nameOf(o.station.min))
          .join("、")}）`,
        details: overlapped.map(
          (o) =>
            `${nameOf(o.station.min)}：徒歩圏の ${percent(overlapPermille(o.station))} が ${nameOf(o.partner.min)} と重なる。駅を足しても乗る人は増えにくい。離れた所へ置き直すか、取り壊して維持費を減らす`,
        ),
        jump: {
          label: "駅へ移る",
          targets: overlapped.map((o) => o.station.min),
          ordered: true,
        },
      });
    }
  }
  return notices.map((nt) =>
    nt.jump !== null && nt.jump.targets.length === 0 ? { ...nt, jump: null } : nt,
  );
}
