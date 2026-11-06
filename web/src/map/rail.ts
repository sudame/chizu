// 線路と駅の描き方（docs/map-style.md の「鉄道」）。線路は複線の 1 組を、中核の線路 1 本（ノードからノードまで）ごとに
// 直線で描く。段ごとに描き方を替える。
// - 引き：`#80868b` の線（太さは道路の中と同じ）
// - 標準：`#5f6368` の線の中に白の破線（地理院地図の鉄道の記号）
// - 寄り：道床の面に、上りと下りの 2 本の線路（レールは 1px の 2 本ずつ）
// - 細部：枕木
// 駅の敷地の面は施設の描き方（roads.ts の facilities）が引きから描く。ここでは寄りの段で、ホームの面と駅舎を描き足す。

import { RAIL_STATION_STRIDE, RAIL_TRACK_STRIDE } from "../protocol/messages.ts";
import { hex } from "./palette.ts";
import { Geometry, type Tile } from "./roads.ts";

export const RAIL_FAR_COLOR = hex("#80868b");
export const RAIL_MID_COLOR = hex("#5f6368");
export const RAIL_DASH_COLOR = hex("#ffffff");
export const RAIL_BED_COLOR = hex("#d8d3c7");
export const RAIL_RAIL_COLOR = hex("#80868b");
export const RAIL_SLEEPER_COLOR = hex("#bdb6a8");
export const STATION_PLATFORM_COLOR = hex("#dadce0");
export const STATION_BUILDING_COLOR = hex("#c5ccd5");
export const STATION_BUILDING_EDGE_COLOR = hex("#8f98a5");
export const STATION_PLATFORM_LINE_COLOR = hex("#ffffff");

/** 引きと標準の線の、世界の半分の幅（マス）。道路の「中」（0.80）と同じ。 */
export const RAIL_LINE_HALF = 0.4;
/** 引きと標準の線の、画面の半分の幅の下限（ピクセル）。太さの下限 2px の半分。 */
export const RAIL_LINE_MIN_PX = 1;
/** 標準の線の中の白の破線の、半分の幅（マス）と下限（ピクセル）と周期（ピクセル）。 */
const DASH_HALF = 0.1;
const DASH_MIN_PX = 0.5;
const DASH_PERIOD_PX = 8;
/** 寄りの道床の半分の幅（マス）。1 マスの幅いっぱいである。 */
const BED_HALF = 0.5;
/** 複線の、上りと下りの線路の中心が、線路の中心線から離れる幅（マス）。 */
export const TRACK_OFFSET = 0.2;
/** 線路 1 本の 2 本のレールが、線路の中心から離れる幅（マス）。軌間 1,067mm の半分ほどである。 */
const RAIL_OFFSET = 0.065;
/** レールの、画面の半分の幅（ピクセル）。1px の線にする。 */
const RAIL_MIN_PX = 0.5;
/** 枕木の、世界の間隔（マス）と、枕木の長さの半分（マス）。間隔は 0.6m で、負にして世界の距離で破線にする。 */
const SLEEPER_PERIOD = 0.075;
const SLEEPER_HALF = 0.14;
/** 駅舎を、駅の間口の両端からこれだけ（マス）内側に置く。 */
const BUILDING_MARGIN = 1;
/** 駅舎の、道路の側の列のうち、道路に近い側の厚み（マス）と、道路の側の縁からの余白（マス）。 */
const BUILDING_DEPTH = 0.5;
const BUILDING_EDGE_GAP = 0.05;
/** ホームの端の白線を、ホームの線路側の縁から離す幅（マス）。 */
const PLATFORM_LINE_GAP = 0.08;
const PLATFORM_LINE_HALF = 0.03;

export type RailTrack = { a: Tile; b: Tile; station: boolean };
export type RailStation = { min: Tile; max: Tile; toward: Tile };

/** chizu-wasm の rail_tracks の数の並びを読む。 */
export function parseTracks(data: ArrayLike<number>): RailTrack[] {
  const out: RailTrack[] = [];
  for (let i = 0; i + RAIL_TRACK_STRIDE <= data.length; i += RAIL_TRACK_STRIDE) {
    out.push({
      a: { x: data[i] ?? 0, y: data[i + 1] ?? 0 },
      b: { x: data[i + 2] ?? 0, y: data[i + 3] ?? 0 },
      station: data[i + 4] === 1,
    });
  }
  return out;
}

/** chizu-wasm の rail_stations の数の並びを読む。 */
export function parseStations(data: ArrayLike<number>): RailStation[] {
  const out: RailStation[] = [];
  for (let i = 0; i + RAIL_STATION_STRIDE <= data.length; i += RAIL_STATION_STRIDE) {
    out.push({
      min: { x: data[i] ?? 0, y: data[i + 1] ?? 0 },
      max: { x: data[i + 2] ?? 0, y: data[i + 3] ?? 0 },
      toward: { x: data[i + 4] ?? 0, y: data[i + 5] ?? 0 },
    });
  }
  return out;
}

export type RailMeshes = {
  /** 引き。 */
  far: Geometry;
  /** 標準。 */
  mid: Geometry;
  /** 寄り。道床の面。 */
  bed: Geometry;
  /** 寄り。レール。 */
  rails: Geometry;
  /** 細部。枕木。 */
  sleepers: Geometry;
  /** 寄り。駅のホームの面と駅舎。 */
  station: Geometry;
  /** 細部。ホームの端の白線。 */
  stationFine: Geometry;
};

/**
 * 駅の敷地を、道路の側から数えた 3 列に分けるための座標。`s` は間口に沿った位置、`d` は敷地の真ん中から
 * 道路へ向かう位置で、どちらもマスの単位である。列 0 が道路の側（駅舎とホーム）、列 1 が線路、列 2 が奥のホーム。
 */
export function stationFrame(st: RailStation): {
  point: (s: number, d: number) => { x: number; y: number };
  frontage: number;
} {
  const [ux, uy] = [st.toward.x, st.toward.y];
  const [fx, fy] = [-uy, ux];
  const cx = (st.min.x + st.max.x + 1) / 2;
  const cy = (st.min.y + st.max.y + 1) / 2;
  const frontage = ux !== 0 ? st.max.y - st.min.y + 1 : st.max.x - st.min.x + 1;
  return {
    point: (s, d) => ({ x: cx + fx * s + ux * d, y: cy + fy * s + uy * d }),
    frontage,
  };
}

/** 駅の敷地の中の長方形（間口の位置 s0〜s1、道路への位置 d0〜d1）を塗る。 */
function stationBox(
  g: Geometry,
  st: RailStation,
  s0: number,
  s1: number,
  d0: number,
  d1: number,
  color: readonly [number, number, number, number],
): void {
  const { point } = stationFrame(st);
  const p = point(s0, d0);
  const q = point(s1, d1);
  g.box(Math.min(p.x, q.x), Math.min(p.y, q.y), Math.max(p.x, q.x), Math.max(p.y, q.y), color);
}

function stationOutline(
  g: Geometry,
  st: RailStation,
  s0: number,
  s1: number,
  d0: number,
  d1: number,
): void {
  const { point } = stationFrame(st);
  const p = point(s0, d0);
  const q = point(s1, d1);
  g.outline(
    Math.min(p.x, q.x),
    Math.min(p.y, q.y),
    Math.max(p.x, q.x),
    Math.max(p.y, q.y),
    STATION_BUILDING_EDGE_COLOR,
    1,
  );
}

type End = "a" | "b";

/**
 * 線路のつなぎ目。ノードに線路がちょうど 2 本あって向きが変わるところ（曲がり角）では、前後の線を
 * 曲がりの二等分線でそろえて切り、道床もレールも 1 本の帯につなぐ（マイター継ぎ）。
 * 曲がり角でないノードは、端の形だけを決める。
 */
class Joints {
  readonly #tracks: readonly RailTrack[];
  readonly #nodes = new Map<string, { track: number; end: End }[]>();

  constructor(tracks: readonly RailTrack[]) {
    this.#tracks = tracks;
    tracks.forEach((t, track) => {
      for (const end of ["a", "b"] as const) {
        const key = Joints.key(t[end]);
        const list = this.#nodes.get(key) ?? [];
        list.push({ track, end });
        this.#nodes.set(key, list);
      }
    });
  }

  static key(t: Tile): string {
    return `${t.x},${t.y}`;
  }

  /** 線路 i の、ノードから遠ざかる向きの単位ベクトル。 */
  #outward(track: number, end: End): [number, number] {
    const t = this.#tracks[track];
    if (t === undefined) {
      return [0, 0];
    }
    const [from, to] = end === "a" ? [t.a, t.b] : [t.b, t.a];
    const len = Math.hypot(to.x - from.x, to.y - from.y);
    return len === 0 ? [0, 0] : [(to.x - from.x) / len, (to.y - from.y) / len];
  }

  /** 曲がり角なら、通る向きの内積と外積、通る向きで先に来る端を返す。 */
  #bend(
    track: number,
    end: End,
  ): { c: number; cross: number; incoming: boolean; len: number } | null {
    const t = this.#tracks[track];
    if (t === undefined) {
      return null;
    }
    const ends = this.#nodes.get(Joints.key(t[end]));
    if (ends?.length !== 2) {
      return null;
    }
    const [e0, e1] = ends;
    if (e0 === undefined || e1 === undefined || e0.track === e1.track) {
      return null;
    }
    const u0 = this.#outward(e0.track, e0.end);
    const u1 = this.#outward(e1.track, e1.end);
    // 端 0 から入って端 1 へ出る向き
    const d1 = [-u0[0], -u0[1]] as const;
    const c = d1[0] * u1[0] + d1[1] * u1[1];
    if (c > 1 - 1e-9 || c < -1 + 1e-9) {
      return null;
    }
    const cross = d1[0] * u1[1] - d1[1] * u1[0];
    const self = e0.track === track && e0.end === end;
    const len = Math.hypot(t.b.x - t.a.x, t.b.y - t.a.y);
    return { c, cross, incoming: self, len };
  }

  /**
   * 線路 i の `end` の端を、中心線から横に `offset`（`strip` と同じ向きが正）ずれた線のために縮める長さ（マス）。
   * 負なら伸ばす。曲がり角でなければ null。
   */
  trim(track: number, end: End, offset: number): number | null {
    const bend = this.#bend(track, end);
    if (bend === null) {
      return null;
    }
    // 通る向きと線路の向きが同じなら、横のずれの正負も同じ
    const same = (end === "b") === bend.incoming;
    const k = ((same ? 1 : -1) * offset * bend.cross) / (1 + bend.c);
    const limit = bend.len / 2;
    return Math.max(-limit, Math.min(limit, k));
  }

  /** 枕木が曲がりの二等分線を越えて、隣の線路の枕木と重ならないための縮める長さ。外側の端の横幅 `reach` から決める。 */
  sleeperTrim(track: number, end: End, reach: number): number | null {
    const bend = this.#bend(track, end);
    if (bend === null) {
      return null;
    }
    return Math.min((reach * Math.abs(bend.cross)) / (1 + bend.c), bend.len / 2);
  }

  /** 道床の面。曲がり角は二等分線で斜めに切り、行き止まりは半マス伸ばし、分岐は丸か四角で埋める。 */
  bed(g: Geometry, track: number): void {
    const t = this.#tracks[track];
    if (t === undefined) {
      return;
    }
    const dx = t.b.x - t.a.x;
    const dy = t.b.y - t.a.y;
    const len = Math.hypot(dx, dy);
    if (len === 0) {
      return;
    }
    const [ux, uy] = [dx / len, dy / len];
    const [nx, ny] = [-uy, ux];
    const corner = (end: End, side: number): [number, number] => {
      const base = end === "a" ? t.a : t.b;
      const sign = end === "a" ? 1 : -1;
      const o = side * BED_HALF;
      const k = this.trim(track, end, o);
      const along = k ?? this.#capTrim(track, end);
      return [base.x + 0.5 + nx * o + sign * ux * along, base.y + 0.5 + ny * o + sign * uy * along];
    };
    g.polygon([corner("a", 1), corner("a", -1), corner("b", -1), corner("b", 1)], RAIL_BED_COLOR);
    for (const end of ["a", "b"] as const) {
      const node = t[end];
      const ends = this.#nodes.get(Joints.key(node)) ?? [];
      if (ends.length < 3 && ends.length !== 0) {
        continue;
      }
      // 3 本以上が集まるノードは、向きが集まる所を埋める
      const allAxis = ends.every((e) =>
        this.#outward(e.track, e.end).some((v) => Math.abs(v) < 1e-9),
      );
      if (allAxis) {
        g.square(node, BED_HALF, RAIL_BED_COLOR, 0);
      } else {
        g.disc(node.x + 0.5, node.y + 0.5, BED_HALF, RAIL_BED_COLOR, 0);
      }
    }
  }

  /** 曲がり角でない端の、中心線に沿った伸ばし方（マスの、負なら縮める）。行き止まりは半マス伸ばす。 */
  #capTrim(track: number, end: End): number {
    const t = this.#tracks[track];
    const ends = t === undefined ? undefined : this.#nodes.get(Joints.key(t[end]));
    return ends?.length === 1 ? -BED_HALF : 0;
  }
}

/** 線路と駅の頂点。線路も駅もなければ、どれも空である。 */
export function railMeshes(
  tracks: readonly RailTrack[],
  stations: readonly RailStation[],
): RailMeshes {
  const origin = tracks[0]?.a ?? stations[0]?.min ?? { x: 0, y: 0 };
  const meshes: RailMeshes = {
    far: new Geometry(origin),
    mid: new Geometry(origin),
    bed: new Geometry(origin),
    rails: new Geometry(origin),
    sleepers: new Geometry(origin),
    station: new Geometry(origin),
    stationFine: new Geometry(origin),
  };
  const center = (t: Tile) => ({ x: t.x + 0.5, y: t.y + 0.5 });
  // 線は、まず灰色の帯と継ぎ目の丸を全部描き、そのあとで白の破線を重ねる
  for (const { a, b } of tracks) {
    meshes.far.segment(a, b, RAIL_LINE_HALF, RAIL_FAR_COLOR, RAIL_LINE_MIN_PX);
    meshes.mid.segment(a, b, RAIL_LINE_HALF, RAIL_MID_COLOR, RAIL_LINE_MIN_PX);
    for (const t of [a, b]) {
      const c = center(t);
      meshes.far.disc(c.x, c.y, RAIL_LINE_HALF, RAIL_FAR_COLOR, RAIL_LINE_MIN_PX);
      meshes.mid.disc(c.x, c.y, RAIL_LINE_HALF, RAIL_MID_COLOR, RAIL_LINE_MIN_PX);
    }
  }
  for (const { a, b } of tracks) {
    meshes.mid.strip(a, b, 0, DASH_HALF, RAIL_DASH_COLOR, DASH_MIN_PX, 0, 0, 0, DASH_PERIOD_PX);
  }
  const joints = new Joints(tracks);
  tracks.forEach((track, i) => {
    const { a, b, station } = track;
    joints.bed(meshes.bed, i);
    for (const side of [-1, 1]) {
      const lane = side * TRACK_OFFSET;
      // 駅の線路は、両端が駅の敷地の端のマスの真ん中なので、敷地の端まで半マス伸ばす（曲がり角を除く）
      const ext = station ? -0.5 : 0;
      const sleeperJoint = (end: End) => joints.sleeperTrim(i, end, Math.abs(lane) + SLEEPER_HALF);
      meshes.sleepers.strip(
        a,
        b,
        lane,
        SLEEPER_HALF,
        RAIL_SLEEPER_COLOR,
        0,
        sleeperJoint("a") ?? ext,
        sleeperJoint("b") ?? ext,
        0,
        -SLEEPER_PERIOD,
      );
      for (const rail of [-1, 1]) {
        const o = lane + rail * RAIL_OFFSET;
        meshes.rails.strip(
          a,
          b,
          o,
          0,
          RAIL_RAIL_COLOR,
          RAIL_MIN_PX,
          joints.trim(i, "a", o) ?? ext,
          joints.trim(i, "b", o) ?? ext,
        );
      }
    }
  });
  for (const st of stations) {
    const { frontage } = stationFrame(st);
    const half = frontage / 2;
    // 道路の側の列と奥の列は、ホームの面。道路の側の列には、道路に近い側に駅舎を置く
    stationBox(meshes.station, st, -half, half, 0.5, 1.5, STATION_PLATFORM_COLOR);
    stationBox(meshes.station, st, -half, half, -1.5, -0.5, STATION_PLATFORM_COLOR);
    const s0 = -half + BUILDING_MARGIN;
    const s1 = half - BUILDING_MARGIN;
    const d0 = 1.5 - BUILDING_EDGE_GAP - BUILDING_DEPTH;
    const d1 = 1.5 - BUILDING_EDGE_GAP;
    stationBox(meshes.station, st, s0, s1, d0, d1, STATION_BUILDING_COLOR);
    stationOutline(meshes.station, st, s0, s1, d0, d1);
    // 細部では、ホームの線路側の縁に白線を引く
    const { point } = stationFrame(st);
    for (const d of [0.5 + PLATFORM_LINE_GAP, -0.5 - PLATFORM_LINE_GAP]) {
      const p = point(-half + 0.2, d);
      const q = point(half - 0.2, d);
      meshes.stationFine.line(
        p.x,
        p.y,
        q.x,
        q.y,
        PLATFORM_LINE_HALF,
        STATION_PLATFORM_LINE_COLOR,
        RAIL_MIN_PX,
      );
    }
  }
  return meshes;
}
