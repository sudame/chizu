// 主題図「公共交通」（docs/map-style.md の「主題図」）。駅まで歩く範囲の円、路線、駅を、薄めた地図の上に重ねる。
// 値は chizu-wasm の rail_stats が、1 秒ほどごとに送る。列車ごとの混み具合は、列車の描き方（trains.ts）が窓の濃さで見せる。

import { TICKS_PER_DAY } from "../utility-trend.ts";
import { laneLayout, lineColorCode, MAX_LANES, type RailLineInfo } from "./lines.ts";
import { type Color, hex } from "./palette.ts";
import type { RailStation } from "./rail.ts";
import { Geometry, type Tile } from "./roads.ts";
import { STATION_WALK_RADIUS } from "./tools.ts";

/** chizu-wasm の rail_stats の見出しの数と、駅 1 つ、路線 1 本の数の個数。 */
export const RAIL_STATS_HEADER = 4;
export const RAIL_STATS_STATION = 13;
export const RAIL_STATS_LINE = 8;

export type StationStat = {
  index: number;
  generation: number;
  /** 駅の敷地の min のマス。 */
  min: Tile;
  /** 駅の出入口のマス。出入口がなければ敷地の min。 */
  door: Tile;
  hasDoor: boolean;
  /** 徒歩圏の世帯の数と、そのうち車を持てない世帯の数。 */
  households: number;
  carless: number;
  waiting: number;
  /** 始まりからの乗った人と降りた人。 */
  boarded: number;
  alighted: number;
  /** 直前に乗りきれず人が残ったか。 */
  leftBehind: boolean;
};

export type LineStat = {
  index: number;
  generation: number;
  /** いちばん混んだ列車の乗車率（‰）。 */
  loadPermille: number;
  boarded: number;
  waiting: number;
  leftBehindStops: number;
  aboard: number;
  running: number;
};

export type RailStats = {
  tick: number;
  /** 待ちきれずに歩いた人と、途中で歩きに戻った人（どちらも始まりからの積み上げ）。 */
  gaveUp: number;
  evicted: number;
  stations: StationStat[];
  lines: LineStat[];
};

/** chizu-wasm の rail_stats の数の並びを読む。壊れていたら、読めたところまでにする。 */
export function parseRailStats(tick: number, data: ArrayLike<number>): RailStats {
  const at = (i: number) => data[i] ?? 0;
  const stationCount = at(0);
  const lineCount = at(1);
  const stations: StationStat[] = [];
  for (let k = 0; k < stationCount; k++) {
    const i = RAIL_STATS_HEADER + k * RAIL_STATS_STATION;
    if (i + RAIL_STATS_STATION > data.length) {
      break;
    }
    const min = { x: at(i + 2), y: at(i + 3) };
    const hasDoor = at(i + 6) === 1;
    stations.push({
      index: at(i),
      generation: at(i + 1),
      min,
      door: hasDoor ? { x: at(i + 4), y: at(i + 5) } : min,
      hasDoor,
      households: at(i + 7),
      carless: at(i + 8),
      waiting: at(i + 9),
      boarded: at(i + 10),
      alighted: at(i + 11),
      leftBehind: at(i + 12) === 1,
    });
  }
  const lines: LineStat[] = [];
  const linesAt = RAIL_STATS_HEADER + stationCount * RAIL_STATS_STATION;
  for (let k = 0; k < lineCount; k++) {
    const i = linesAt + k * RAIL_STATS_LINE;
    if (i + RAIL_STATS_LINE > data.length) {
      break;
    }
    lines.push({
      index: at(i),
      generation: at(i + 1),
      loadPermille: at(i + 2),
      boarded: at(i + 3),
      waiting: at(i + 4),
      leftBehindStops: at(i + 5),
      aboard: at(i + 6),
      running: at(i + 7),
    });
  }
  return { tick, gaveUp: at(2), evicted: at(3), stations, lines };
}

/** 駅の ID を文字にしたもの。 */
export function stationKey(s: { index: number; generation: number }): string {
  return `${s.index}.${s.generation}`;
}

/** 乗った人と降りた人の、その日の数。 */
export type DayRides = { boarded: number; alighted: number };

/**
 * 駅ごとの、その日の乗り降りの数。中核が持つのは始まりからの積み上げだけなので、日が変わった最初の値を控えて差を取る。
 * 開いたときの途中の日は、開いてからの数になる（`fullDay` が偽）。tick が戻ったら（読み込み、新しい街）控えを捨てる。
 */
export class RideDays {
  #day = -1;
  #lastTick = -1;
  #base = new Map<string, DayRides>();
  #fullDay = false;

  update(stats: RailStats): void {
    const day = Math.floor(stats.tick / TICKS_PER_DAY);
    const restarted = stats.tick < this.#lastTick;
    if (restarted || this.#day < 0 || day !== this.#day) {
      // 日が変わった最初の値を控える。最初の 1 回は、日の途中かもしれない
      this.#fullDay = !restarted && this.#day >= 0 && day === this.#day + 1;
      this.#base = new Map(
        stats.stations.map((s) => [stationKey(s), { boarded: s.boarded, alighted: s.alighted }]),
      );
      this.#day = day;
    }
    this.#lastTick = stats.tick;
  }

  /** 駅の、その日の乗り降り。控えがなければ（新しくできた駅など）始まりからの数を引いた 0 から数える。 */
  today(s: StationStat): DayRides {
    const base = this.#base.get(stationKey(s));
    if (base === undefined) {
      // 日の途中でできた駅の積み上げは、できてからの数である
      return { boarded: s.boarded, alighted: s.alighted };
    }
    return {
      boarded: Math.max(0, s.boarded - base.boarded),
      alighted: Math.max(0, s.alighted - base.alighted),
    };
  }

  /** 控えが日の変わり目からか。偽なら、開いてからの数である。 */
  get fullDay(): boolean {
    return this.#fullDay;
  }
}

// 色
const BLUE = "#1a73e8";
/** 駅の徒歩圏の円の塗りと縁。 */
export const CATCHMENT_FILL: Color = hex(BLUE, 0.12);
export const CATCHMENT_EDGE: Color = hex(BLUE, 0.5);
/**
 * 駅の丸の色（交通の系統の灰青の 40%）。施設の記号は丸の上に重なるので、記号より外にはみ出す大きさで乗り降りの数を見せる。
 */
export const STATION_RING_COLOR: Color = hex("#5f6b7a", 0.4);
/** 乗りきれない人がいる駅の印（赤の 55%）。 */
export const STATION_ALERT_COLOR: Color = hex("#ea4335", 0.55);
/** 止まっている路線の線。 */
export const STOPPED_LINE_COLOR: Color = hex("#9aa0a6");

/** 徒歩圏の円の縁の太さ（画面のピクセル）。 */
const CATCHMENT_EDGE_PX = 1.5;

/** 線の半分の太さ（マス）の下限と上限。乗車率 0 で下限、100% で上限になる。 */
export const LOAD_HALF_MIN = 0.3;
export const LOAD_HALF_MAX = 0.9;
/** 線の画面の半分の太さの下限（ピクセル）の、乗車率 0 と 100% での値。 */
export const LOAD_PX_MIN = 1.2;
export const LOAD_PX_MAX = 3.2;

/** 乗車率（‰）から、線の半分の太さ（マス）。100% を超えても上限で止める。 */
export function loadHalf(permille: number): number {
  const t = Math.min(Math.max(permille, 0), 1000) / 1000;
  return t >= 1 ? LOAD_HALF_MAX : LOAD_HALF_MIN + (LOAD_HALF_MAX - LOAD_HALF_MIN) * t;
}

/** 乗車率（‰）から、線の画面の半分の太さの下限（ピクセル）。 */
export function loadPx(permille: number): number {
  const t = Math.min(Math.max(permille, 0), 1000) / 1000;
  return LOAD_PX_MIN + (LOAD_PX_MAX - LOAD_PX_MIN) * t;
}

/** 駅の丸の半径（マス）。その日の乗り降りの数の平方根に比べて大きくし、上限で止める。 */
export function stationRadius(rides: number): number {
  return Math.min(1.6 + 0.12 * Math.sqrt(Math.max(rides, 0)), 5);
}

/** 乗りきれない印を出す、待っている人の数。 */
export const WAITING_ALERT = 100;

/** 駅に乗りきれない人がいる印を出すか。直前の列車に乗りきれなかったか、待っている人が多い。 */
export function stationCrowded(s: StationStat): boolean {
  return s.leftBehind || s.waiting >= WAITING_ALERT;
}

export type TransitMeshes = {
  circles: Geometry;
  lines: Geometry;
  stations: Geometry;
};

const center = (st: RailStation): { x: number; y: number } => ({
  x: (st.min.x + st.max.x + 1) / 2,
  y: (st.min.y + st.max.y + 1) / 2,
});

/**
 * 公共交通の主題図のメッシュを作る。
 * - 円：出入口のマスの真ん中から、駅まで歩く範囲（中核が駅を探す物差しと同じ）
 * - 路線：路線の色の線。太さは、いちばん混んだ列車の乗車率に応じて太くする。止まっている路線は灰色の細い線にする
 * - 駅：灰青の丸。大きさはその日の乗り降りの数で変え、乗りきれない人がいる駅は赤の輪を重ねる
 */
export function transitMeshes(
  lines: readonly RailLineInfo[],
  stations: readonly RailStation[],
  stats: RailStats | null,
  rides: (s: StationStat) => DayRides,
): TransitMeshes {
  const origin = stations[0]?.min ?? lines[0]?.tracks[0]?.a ?? { x: 0, y: 0 };
  const meshes: TransitMeshes = {
    circles: new Geometry(origin),
    lines: new Geometry(origin),
    stations: new Geometry(origin),
  };
  const statOf = new Map(stats?.stations.map((s) => [`${s.min.x},${s.min.y}`, s]));
  for (const s of stats?.stations ?? []) {
    if (s.hasDoor) {
      meshes.circles.circle(
        s.door.x + 0.5,
        s.door.y + 0.5,
        STATION_WALK_RADIUS,
        CATCHMENT_FILL,
        CATCHMENT_EDGE,
        CATCHMENT_EDGE_PX,
      );
    }
  }
  addLines(
    meshes.lines,
    lines,
    new Map(stats?.lines.map((l) => [`${l.index}.${l.generation}`, l])),
  );
  for (const st of stations) {
    const stat = statOf.get(`${st.min.x},${st.min.y}`);
    const c = center(st);
    const day = stat === undefined ? { boarded: 0, alighted: 0 } : rides(stat);
    const r = stationRadius(day.boarded + day.alighted);
    if (stat !== undefined && stationCrowded(stat)) {
      meshes.stations.disc(c.x, c.y, r + 0.8, STATION_ALERT_COLOR, 13);
    }
    meshes.stations.disc(c.x, c.y, r, STATION_RING_COLOR, 11);
  }
  return meshes;
}

function addLines(
  g: Geometry,
  lines: readonly RailLineInfo[],
  stats: ReadonlyMap<string, LineStat>,
): void {
  const key = (a: Tile, b: Tile) => {
    const first = a.x < b.x || (a.x === b.x && a.y <= b.y);
    const [p, q] = first ? [a, b] : [b, a];
    return `${p.x},${p.y},${q.x},${q.y}`;
  };
  const live = lines.filter((l) => l.tracks.length > 0);
  const users = new Map<string, number[]>();
  for (const line of live) {
    for (const { a, b } of line.tracks) {
      const list = users.get(key(a, b)) ?? [];
      list.push(line.index);
      users.set(key(a, b), list);
    }
  }
  for (const list of users.values()) {
    list.sort((x, y) => x - y);
  }
  const permilleOf = (line: RailLineInfo) =>
    stats.get(`${line.index}.${line.generation}`)?.loadPermille ?? 0;
  for (const line of live) {
    const dim = line.stopped;
    const load = permilleOf(line);
    const half = dim ? LOAD_HALF_MIN * 0.6 : loadHalf(load);
    const min = dim ? LOAD_PX_MIN : loadPx(load);
    const base = dim ? STOPPED_LINE_COLOR : hex(lineColorCode(line.index));
    // 走らせる列車が 0 本の路線は薄く描く
    const color: Color = line.allowed === 0 && !dim ? [base[0], base[1], base[2], 0.5] : base;
    for (const { a, b } of line.tracks) {
      const list = users.get(key(a, b)) ?? [line.index];
      const lanes = Math.min(list.length, MAX_LANES);
      // 並べる線の太さに合わせて間隔を決める。路線の数が多いときは線路の幅に収まるよう細くする
      const fit = laneLayout(list.length).half;
      const h = Math.min(half, fit * (LOAD_HALF_MAX / 0.22));
      const pitch = 2 * h * 1.1;
      const slot = Math.min(list.indexOf(line.index), lanes - 1);
      const offset = (slot - (lanes - 1) / 2) * pitch;
      const forward = a.x < b.x || (a.x === b.x && a.y <= b.y);
      g.strip(a, b, forward ? offset : -offset, h, color, min);
    }
  }
}

/**
 * 地図に重ねる文字 1 つ。座標はマスの単位。`text` は車を持てない世帯まで書いた文、`short` は世帯の数だけの文である。
 * 引いたときは、駅どうしが近くて文字が重なるので、短い文にする。
 */
export type TransitText = { x: number; y: number; text: string; short: string };

/** 駅の徒歩圏の世帯の数を、駅の丸の下に書く。車を持てない世帯がいれば添える。 */
export function transitTexts(
  stations: readonly RailStation[],
  stats: RailStats | null,
): TransitText[] {
  const statOf = new Map(stats?.stations.map((s) => [`${s.min.x},${s.min.y}`, s]));
  const out: TransitText[] = [];
  for (const st of stations) {
    const stat = statOf.get(`${st.min.x},${st.min.y}`);
    if (stat === undefined || !stat.hasDoor) {
      continue;
    }
    const c = center(st);
    out.push({
      x: c.x,
      y: c.y,
      text: householdsText(stat),
      short: householdsText({ households: stat.households, carless: 0 }),
    });
  }
  return out;
}

/** 徒歩圏の世帯の文。 */
export function householdsText(s: Pick<StationStat, "households" | "carless">): string {
  const n = s.households.toLocaleString("ja-JP");
  return s.carless > 0 ? `${n} 世帯（車なし ${s.carless.toLocaleString("ja-JP")}）` : `${n} 世帯`;
}
