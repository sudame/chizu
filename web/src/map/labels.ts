// 地図のラベル（道路名、地区名、施設名）の置き方（docs/map-style.md の「ラベル」）。
// どのラベルを出すかを縮尺の段で決め、優先の高い順に置き、重なるものは置かない。
// 字の画像を作って描くのは label-layer.ts で、ここは DOM に頼らない計算だけを持つ。

import { BUILDING_STRIDE, FACILITY_STRIDE, SEGMENT_STRIDE } from "../protocol/messages.ts";
import {
  activeSymbols,
  facilityOwner,
  layoutSymbols,
  SYMBOL_BORDER,
  symbolLook,
} from "./facility-symbols.ts";
import {
  chunkOf,
  districtName,
  type FacilityGroup,
  facilityGroupNames,
  roadLineKey,
  roadName,
  type Seed,
} from "./names.ts";
import { FACILITY_STYLES } from "./palette.ts";

/** 画面のピクセルの長方形。x0 ≤ x1、y0 ≤ y1。 */
export type Rect = { x0: number; y0: number; x1: number; y1: number };

export type Viewport = { width: number; height: number };
export type CameraView = { x: number; y: number; zoom: number };

// 区間の種類の番号（chizu-wasm の segment_kind_code と同じ）
const KIND_MEDIUM = 2;
const KIND_LARGE = 3;
const KIND_HIGHWAY = 4;

/** ラベルの種類。道路は区間の種類ごとに分ける。 */
export type LabelKind = "highway" | "district" | "facility" | "large" | "medium" | "small";

// ラベルの段。倍率がこの値以上になると次の段に移る。引き、標準、寄り、地区の名前を消す段の順である。
// 地区の名前は、溶け込みの表の「地区の名前が消える 32〜40」の終わりの 40 で消す
export const LABEL_STAGE_ZOOMS: readonly number[] = [4, 12, 40];
/** 段を戻すのは、境目の倍率よりこれだけ引いたとき。境目で行き来してもラベルが点滅しないようにする。 */
export const STAGE_HIDE_LAG = 0.5;
export const STAGE_FAR = 0;
export const STAGE_STANDARD = 1;
export const STAGE_NEAR = 2;
export const STAGE_DETAIL = 3;

/**
 * 倍率と前の段から、ラベルの段を求める。寄るときは境目の倍率で次の段に移り、
 * 引くときは境目より STAGE_HIDE_LAG だけ引いたときに前の段へ戻る。
 */
export function nextLabelStage(zoom: number, prev: number): number {
  const up = LABEL_STAGE_ZOOMS.filter((z) => zoom >= z).length;
  const down = LABEL_STAGE_ZOOMS.filter((z) => zoom >= z - STAGE_HIDE_LAG).length;
  if (up > prev) return up;
  if (down < prev) return down;
  return prev;
}

/** 字の見た目。 */
export type TextStyle = {
  /** 字の大きさ（CSS ピクセル）。 */
  size: number;
  weight: number;
  /** 字の間隔（CSS ピクセル）。 */
  spacing: number;
  color: string;
  /** 不透明度。 */
  alpha: number;
  /** 高速道路の緑の盾に白い字で置くか。 */
  shield: boolean;
};

export type LabelStyle = { priority: number; style: TextStyle };

const GRAY = "#5f6368";
const GRAY_NEAR = "#70757a";
const LARGE_ROAD = "#6b5310";
/** 施設の名前の色。施設の種類（ServiceKind の値）ごと。 */
export const FACILITY_LABEL_COLORS: Readonly<Record<number, string>> = Object.fromEntries(
  Object.entries(FACILITY_STYLES).map(([kind, style]) => [kind, style.name]),
);

const text = (
  size: number,
  weight: number,
  color: string,
  spacing = 0,
  alpha = 1,
  shield = false,
): TextStyle => ({ size, weight, spacing, color, alpha, shield });

/**
 * 段ごとの、ラベルの種類の優先と見た目。出さない段は null。優先は小さいほど先に置く。
 * 高速 > 地区（引き） > 施設 > 地区（標準） > 大 > 中 > 小 > 地区（寄り）の順である。
 */
export function labelStyle(kind: LabelKind, stage: number): LabelStyle | null {
  const near = stage >= STAGE_NEAR;
  const roadSize = near ? 12 : 11;
  switch (kind) {
    case "highway":
      return { priority: 1, style: text(roadSize, 500, "#ffffff", 0, 1, true) };
    case "district":
      if (stage === STAGE_FAR) return { priority: 2, style: text(17, 600, GRAY, 4) };
      if (stage === STAGE_STANDARD) return { priority: 4, style: text(14, 600, GRAY, 2) };
      if (stage === STAGE_NEAR) return { priority: 8, style: text(13, 600, GRAY_NEAR, 2, 0.8) };
      return null;
    case "facility":
      return stage === STAGE_FAR ? null : { priority: 3, style: text(12, 600, GRAY) };
    case "large":
      return { priority: 5, style: text(roadSize, 500, LARGE_ROAD) };
    case "medium":
      return stage === STAGE_FAR ? null : { priority: 6, style: text(roadSize, 500, GRAY) };
    case "small":
      return near ? { priority: 7, style: text(roadSize, 500, GRAY) } : null;
  }
}

/** 区間の種類の番号から、道路のラベルの種類を求める。 */
export function roadLabelKind(kindCode: number): LabelKind {
  if (kindCode === KIND_HIGHWAY) return "highway";
  if (kindCode === KIND_LARGE) return "large";
  if (kindCode === KIND_MEDIUM) return "medium";
  return "small";
}

/** 字の向き。横書き、縦書き、斜め（angle の角度だけ回す）。 */
export type Orientation = "horizontal" | "vertical" | "angled";

/** ラベルの画像の大きさ（CSS ピクセル）。縁取りと盾を含む。 */
export type LabelSize = { w: number; h: number };
export type Measure = (text: string, style: TextStyle, vertical: boolean) => LabelSize;

/** 置く候補のラベル。 */
export type Candidate = {
  /** 段をまたいでも同じものを指す識別子。見た目が変わると別のラベルとして出し入れする。 */
  id: string;
  priority: number;
  text: string;
  style: TextStyle;
  vertical: boolean;
  /** 回す角度（ラジアン）。 */
  angle: number;
  /** 基準の点のワールド座標（マスの単位）。 */
  wx: number;
  wy: number;
  /** 回す前の画像の左上の、基準の点からのずれ（CSS ピクセル）。 */
  ox: number;
  oy: number;
  w: number;
  h: number;
  /** 画面の上で占める範囲。回したときは、回した画像を囲む長方形。 */
  box: Rect;
  /** 施設の名前のとき、名前を添える記号の識別子。その記号とは、余白をとらずに重なりを調べる。 */
  owner?: string;
  /** 同じ値の候補は、1 つだけ置く。置き方の違う候補を、優先の高い順に並べるのに使う。 */
  group?: string;
};

// ---- 道路 ----

/**
 * 同じ直線の上で続いている、同じ種類の区間をつないだもの。端点はマスの中心のワールド座標。
 * key は直線の鍵と種類で、同じ直線の上の離れた区間も同じ鍵になる。道路を延ばしても変わらない。
 */
export type RoadRun = {
  key: string;
  /** 直線の原点から a までの、直線に沿ったワールドの距離（マス）。繰り返す位置をこの原点から決める。 */
  start: number;
  kind: number;
  name: string;
  orientation: Orientation;
  ax: number;
  ay: number;
  bx: number;
  by: number;
};

type Interval = { lo: number; hi: number };

/** 直線の鍵の向きごとに、マスの座標と直線の上の位置 t を行き来する。 */
function lineParam(key: string, x: number, y: number): number {
  return key.startsWith("V") ? y : x;
}
function linePoint(key: string, t: number): { x: number; y: number } {
  const c = Number(key.slice(1));
  switch (key[0]) {
    case "H":
      return { x: t, y: c };
    case "V":
      return { x: c, y: t };
    case "D":
      return { x: t, y: t - c };
    default:
      return { x: t, y: c - t };
  }
}

/**
 * 道路の区間（chizu-wasm の road_segments）を、同じ直線の上で続いている同じ種類のものごとにつなぐ。
 * 名前は直線の鍵と種類で決まるので、つないだものは 1 つの名前を持つ。
 */
export function roadRuns(seed: Seed, segments: ArrayLike<number>): RoadRun[] {
  const groups = new Map<string, { lineKey: string; kind: number; spans: Interval[] }>();
  const runs: RoadRun[] = [];
  for (let i = 0; i + SEGMENT_STRIDE <= segments.length; i += SEGMENT_STRIDE) {
    const ax = segments[i] ?? 0;
    const ay = segments[i + 1] ?? 0;
    const bx = segments[i + 2] ?? 0;
    const by = segments[i + 3] ?? 0;
    const kind = segments[i + 4] ?? 0;
    const lineKey = roadLineKey({ x: ax, y: ay }, { x: bx, y: by });
    if (lineKey.startsWith("X")) {
      // 45 度でない向きは作れないが、来たらその区間だけで置く
      runs.push({
        key: `${lineKey}|${kind}`,
        start: 0,
        kind,
        name: roadName(seed, lineKey, kind),
        orientation: "angled",
        ax: ax + 0.5,
        ay: ay + 0.5,
        bx: bx + 0.5,
        by: by + 0.5,
      });
      continue;
    }
    const groupKey = `${lineKey}|${kind}`;
    let group = groups.get(groupKey);
    if (group === undefined) {
      group = { lineKey, kind, spans: [] };
      groups.set(groupKey, group);
    }
    const t0 = lineParam(lineKey, ax, ay);
    const t1 = lineParam(lineKey, bx, by);
    group.spans.push({ lo: Math.min(t0, t1), hi: Math.max(t0, t1) });
  }
  for (const [groupKey, { lineKey, kind, spans }] of groups) {
    spans.sort((a, b) => a.lo - b.lo || a.hi - b.hi);
    const merged: Interval[] = [];
    for (const s of spans) {
      const last = merged[merged.length - 1];
      if (last !== undefined && s.lo <= last.hi) {
        last.hi = Math.max(last.hi, s.hi);
      } else {
        merged.push({ ...s });
      }
    }
    const name = roadName(seed, lineKey, kind);
    const orientation: Orientation = lineKey.startsWith("H")
      ? "horizontal"
      : lineKey.startsWith("V")
        ? "vertical"
        : "angled";
    for (const { lo, hi } of merged) {
      const a = linePoint(lineKey, lo);
      const b = linePoint(lineKey, hi);
      runs.push({
        key: groupKey,
        start: (lo + 0.5) * (orientation === "angled" ? Math.SQRT2 : 1),
        kind,
        name,
        orientation,
        ax: a.x + 0.5,
        ay: a.y + 0.5,
        bx: b.x + 0.5,
        by: b.y + 0.5,
      });
    }
  }
  return runs;
}

// 道路の名前は、見えている長さが「字の長さ + ROAD_LABEL_SLACK」より短い区間には置かない
export const ROAD_LABEL_SLACK = 40;
// 長い区間は、この間隔（CSS ピクセル）ごとに繰り返す
export const ROAD_LABEL_REPEAT = 360;

/** 線分 a→b のうち、長方形 [0, w] × [0, h] に入る部分を、a からの割合 [t0, t1] で返す。入らなければ null。 */
function clipLine(
  ax: number,
  ay: number,
  bx: number,
  by: number,
  w: number,
  h: number,
): [number, number] | null {
  let t0 = 0;
  let t1 = 1;
  const dx = bx - ax;
  const dy = by - ay;
  const edges: [number, number][] = [
    [-dx, ax],
    [dx, w - ax],
    [-dy, ay],
    [dy, h - ay],
  ];
  for (const [p, q] of edges) {
    if (p === 0) {
      if (q < 0) return null;
      continue;
    }
    const r = q / p;
    if (p < 0) {
      if (r > t1) return null;
      t0 = Math.max(t0, r);
    } else {
      if (r < t0) return null;
      t1 = Math.min(t1, r);
    }
  }
  return t0 <= t1 ? [t0, t1] : null;
}

/** 回した画像を囲む長方形の、中心からの半分の幅と高さ。 */
function rotatedHalf(w: number, h: number, angle: number): [number, number] {
  const c = Math.abs(Math.cos(angle));
  const s = Math.abs(Math.sin(angle));
  return [(w * c + h * s) / 2, (w * s + h * c) / 2];
}

const toScreen = (camera: CameraView, vp: Viewport, wx: number, wy: number) => ({
  x: (wx - camera.x) * camera.zoom + vp.width / 2,
  y: (wy - camera.y) * camera.zoom + vp.height / 2,
});
const toWorld = (camera: CameraView, vp: Viewport, sx: number, sy: number) => ({
  x: camera.x + (sx - vp.width / 2) / camera.zoom,
  y: camera.y + (sy - vp.height / 2) / camera.zoom,
});

/**
 * 道路の名前の候補。直線の原点から ROAD_LABEL_REPEAT ごとの位置に置き、画面に収まるものだけを出す。
 * 位置は直線の上で決まっているので、カメラを動かしても地図と一緒に動き、道路を延ばしても同じラベルが残る。
 * その位置が 1 つも入らない短い区間は、区間の真ん中に置く。
 */
export function roadCandidates(
  runs: readonly RoadRun[],
  camera: CameraView,
  vp: Viewport,
  stage: number,
  measure: Measure,
): Candidate[] {
  const out: Candidate[] = [];
  for (const run of runs) {
    const ls = labelStyle(roadLabelKind(run.kind), stage);
    if (ls === null) continue;
    const a = toScreen(camera, vp, run.ax, run.ay);
    const b = toScreen(camera, vp, run.bx, run.by);
    const clip = clipLine(a.x, a.y, b.x, b.y, vp.width, vp.height);
    if (clip === null) continue;
    const length = Math.hypot(b.x - a.x, b.y - a.y);
    if (length === 0) continue;
    const ux = (b.x - a.x) / length;
    const uy = (b.y - a.y) / length;
    const lo = clip[0] * length;
    const hi = clip[1] * length;
    const shield = ls.style.shield;
    const vertical = !shield && run.orientation === "vertical";
    let angle = 0;
    if (!shield && run.orientation === "angled") {
      angle = Math.atan2(uy, ux);
      // 字が逆さにならないよう、-90〜90 度に収める
      if (angle > Math.PI / 2) angle -= Math.PI;
      else if (angle <= -Math.PI / 2) angle += Math.PI;
    }
    const size = measure(run.name, ls.style, vertical);
    // 線に沿った長さ。盾は回さないので、線の向きに測った長さにする
    const along = vertical
      ? size.h
      : shield
        ? Math.abs(ux) * size.w + Math.abs(uy) * size.h
        : size.w;
    const need = along + ROAD_LABEL_SLACK;
    if (hi - lo < need) continue;
    // 直線の原点からの距離（画面のピクセル）で、区間は [origin, origin + length] にある
    const origin = run.start * camera.zoom;
    const half = ROAD_LABEL_REPEAT / 2;
    const first = Math.ceil((origin + need / 2 - half) / ROAD_LABEL_REPEAT);
    const last = Math.floor((origin + length - need / 2 - half) / ROAD_LABEL_REPEAT);
    const spots: [string, number][] = [];
    for (let k = first; k <= last; k++) {
      spots.push([String(k), half + k * ROAD_LABEL_REPEAT - origin]);
    }
    if (spots.length === 0) spots.push([`c${run.start}`, length / 2]);
    const [hw, hh] = rotatedHalf(size.w, size.h, angle);
    // 見えている部分に、字の長さと余白ごと収まるものだけを出す
    const visible = spots.filter(([, p]) => p - need / 2 >= lo && p + need / 2 <= hi);
    for (const [tag, p] of visible) {
      const sx = a.x + ux * p;
      const sy = a.y + uy * p;
      const world = toWorld(camera, vp, sx, sy);
      out.push({
        id: `road|${run.key}|${tag}|${styleKey(ls.style)}`,
        priority: ls.priority,
        text: run.name,
        style: ls.style,
        vertical,
        angle,
        wx: world.x,
        wy: world.y,
        ox: -size.w / 2,
        oy: -size.h / 2,
        w: size.w,
        h: size.h,
        box: { x0: sx - hw, y0: sy - hh, x1: sx + hw, y1: sy + hh },
      });
    }
  }
  return out;
}

// ---- 地区 ----

/** 地区の名前を出すのに要る、チャンクの建物の数。 */
export const DISTRICT_MIN_BUILDINGS = 12;

/** 地区の名前と、置く位置（建物の重心のワールド座標）。 */
export type DistrictLabel = { cx: number; cy: number; name: string; x: number; y: number };

/**
 * 建物（chizu-wasm の buildings）から、建物が DISTRICT_MIN_BUILDINGS 棟以上あるチャンクの地区の名前と、
 * 建物の重心を求める。建物は左上のマスのチャンクに数える。`isWaterside` はチャンクに水辺があるか。
 */
export function districtLabels(
  seed: Seed,
  buildings: ArrayLike<number>,
  isWaterside: (cx: number, cy: number) => boolean,
): DistrictLabel[] {
  const sums = new Map<string, { cx: number; cy: number; n: number; sx: number; sy: number }>();
  for (let i = 0; i + BUILDING_STRIDE <= buildings.length; i += BUILDING_STRIDE) {
    const x0 = buildings[i] ?? 0;
    const y0 = buildings[i + 1] ?? 0;
    const x1 = buildings[i + 2] ?? 0;
    const y1 = buildings[i + 3] ?? 0;
    const cx = chunkOf(x0);
    const cy = chunkOf(y0);
    const key = `${cx},${cy}`;
    let s = sums.get(key);
    if (s === undefined) {
      s = { cx, cy, n: 0, sx: 0, sy: 0 };
      sums.set(key, s);
    }
    s.n++;
    s.sx += (x0 + x1 + 1) / 2;
    s.sy += (y0 + y1 + 1) / 2;
  }
  const out: DistrictLabel[] = [];
  for (const { cx, cy, n, sx, sy } of sums.values()) {
    if (n < DISTRICT_MIN_BUILDINGS) continue;
    out.push({
      cx,
      cy,
      name: districtName(seed, cx, cy, isWaterside(cx, cy)),
      x: sx / n,
      y: sy / n,
    });
  }
  return out;
}

const inside = (box: Rect, vp: Viewport) =>
  box.x0 >= 0 && box.y0 >= 0 && box.x1 <= vp.width && box.y1 <= vp.height;

/** 地区の名前の候補。画面に収まるものだけを出す。 */
export function districtCandidates(
  districts: readonly DistrictLabel[],
  camera: CameraView,
  vp: Viewport,
  stage: number,
  measure: Measure,
): Candidate[] {
  const ls = labelStyle("district", stage);
  if (ls === null) return [];
  const out: Candidate[] = [];
  for (const d of districts) {
    const s = toScreen(camera, vp, d.x, d.y);
    const size = measure(d.name, ls.style, false);
    const box = {
      x0: s.x - size.w / 2,
      y0: s.y - size.h / 2,
      x1: s.x + size.w / 2,
      y1: s.y + size.h / 2,
    };
    if (!inside(box, vp)) continue;
    out.push({
      id: `district|${d.cx},${d.cy}|${d.name}|${styleKey(ls.style)}`,
      priority: ls.priority,
      text: d.name,
      style: ls.style,
      vertical: false,
      angle: 0,
      wx: d.x,
      wy: d.y,
      ox: -size.w / 2,
      oy: -size.h / 2,
      w: size.w,
      h: size.h,
      box,
    });
  }
  return out;
}

// ---- 施設 ----

/** 同じ種類で、隙間がこのマス数以内の施設は、1 つのまとまりにする。 */
export const FACILITY_GROUP_GAP = 2;
/** 施設の名前は、まとまりの中心（記号の位置）の右のこの距離（CSS ピクセル）に置く。 */
export const FACILITY_LABEL_OFFSET = 15;

/** マスの単位のワールド座標の点。 */
export type Point = { x: number; y: number };

/**
 * 施設のまとまり。座標は左上のマスと、まとまりの記号を置くワールド座標（中心にいちばん近い施設の敷地の中心）。
 * `members` は施設 1 つずつの中心で、寄りの段ではここに記号を 1 つずつ置く。
 */
export type FacilityLabel = {
  kind: number;
  x: number;
  y: number;
  count: number;
  name: string;
  centerX: number;
  centerY: number;
  members: Point[];
};

type Box = { x0: number; y0: number; x1: number; y1: number; kind: number };

/**
 * 施設（chizu-wasm の facilities）を、同じ種類で互いに FACILITY_GROUP_GAP マス以内にあるものでまとめ、
 * まとまりごとの名前と棟の数を求める。
 */
export function facilityLabels(
  seed: Seed,
  facilities: ArrayLike<number>,
  isWaterside: (cx: number, cy: number) => boolean,
): FacilityLabel[] {
  const boxes: Box[] = [];
  for (let i = 0; i + FACILITY_STRIDE <= facilities.length; i += FACILITY_STRIDE) {
    boxes.push({
      x0: facilities[i] ?? 0,
      y0: facilities[i + 1] ?? 0,
      x1: facilities[i + 2] ?? 0,
      y1: facilities[i + 3] ?? 0,
      kind: facilities[i + 4] ?? 0,
    });
  }
  // まとまりを union-find で作る。施設は多くて数十なので、総当たりで足りる
  const parent = boxes.map((_, i) => i);
  const find = (i: number): number => {
    let r = i;
    while (parent[r] !== r) r = parent[r] ?? r;
    parent[i] = r;
    return r;
  };
  for (let i = 0; i < boxes.length; i++) {
    for (let j = i + 1; j < boxes.length; j++) {
      const a = boxes[i] as Box;
      const b = boxes[j] as Box;
      if (a.kind !== b.kind) continue;
      // 隙間のマス数。隣り合えば 0
      const gapX = Math.max(a.x0 - b.x1, b.x0 - a.x1) - 1;
      const gapY = Math.max(a.y0 - b.y1, b.y0 - a.y1) - 1;
      if (Math.max(gapX, gapY) <= FACILITY_GROUP_GAP) {
        parent[find(i)] = find(j);
      }
    }
  }
  const groups = new Map<number, Box & { count: number; members: Point[] }>();
  boxes.forEach((b, i) => {
    const r = find(i);
    const g = groups.get(r);
    const center = { x: (b.x0 + b.x1 + 1) / 2, y: (b.y0 + b.y1 + 1) / 2 };
    if (g === undefined) {
      groups.set(r, { ...b, count: 1, members: [center] });
    } else {
      g.members.push(center);
      g.x0 = Math.min(g.x0, b.x0);
      g.y0 = Math.min(g.y0, b.y0);
      g.x1 = Math.max(g.x1, b.x1);
      g.y1 = Math.max(g.y1, b.y1);
      g.count++;
    }
  });
  const list = [...groups.values()].sort((a, b) => a.y0 - b.y0 || a.x0 - b.x0 || a.kind - b.kind);
  const named: FacilityGroup[] = list.map((g) => ({ x: g.x0, y: g.y0, kind: g.kind }));
  const names = facilityGroupNames(seed, named, isWaterside);
  return list.map((g, i) => ({
    kind: g.kind,
    x: g.x0,
    y: g.y0,
    count: g.count,
    name: names[i] ?? "",
    ...centerOn(nearestTo((g.x0 + g.x1 + 1) / 2, (g.y0 + g.y1 + 1) / 2, g.members)),
    members: g.members,
  }));
}

/** 点 (x, y) にいちばん近い点。同じ距離なら上、左の順に選ぶ。 */
function nearestTo(x: number, y: number, points: readonly Point[]): Point {
  let best = points[0] as Point;
  let bestD = Number.POSITIVE_INFINITY;
  for (const p of points) {
    const d = (p.x - x) ** 2 + (p.y - y) ** 2;
    if (d < bestD || (d === bestD && (p.y < best.y || (p.y === best.y && p.x < best.x)))) {
      best = p;
      bestD = d;
    }
  }
  return best;
}
const centerOn = (p: Point) => ({ centerX: p.x, centerY: p.y });

/** 施設のラベルの文字。標準の段では、2 棟以上のまとまりに棟の数を添える。 */
export function facilityText(f: FacilityLabel, stage: number): string {
  return stage === STAGE_STANDARD && f.count > 1 ? `${f.name}（${f.count} 棟）` : f.name;
}

/** 1 つのまとまりの名前について試す候補の数の上限。 */
const MAX_FACILITY_ALTERNATIVES = 12;
// 記号の下と上に置くときの、丸の縁から名前までの隙間（CSS ピクセル）
const BELOW_GAP = 3;

/**
 * 施設の名前の候補。画面に収まるものだけを出す。名前は、記号の右、左、下、上の順で試す（同じ `group` の候補は
 * 1 つだけ置く）。寄りの段は記号が 1 つずつ並ぶので、いちばん右の記号から順に試す。
 */
export function facilityCandidates(
  facilities: readonly FacilityLabel[],
  camera: CameraView,
  vp: Viewport,
  stage: number,
  measure: Measure,
): Candidate[] {
  const ls = labelStyle("facility", stage);
  if (ls === null) return [];
  const near = stage >= STAGE_NEAR;
  const symbols = activeSymbols(layoutSymbols(facilities, camera, vp), near);
  const reach = symbolLook(camera.zoom).radius + SYMBOL_BORDER;
  const out: Candidate[] = [];
  for (const f of facilities) {
    const owner = facilityOwner(f);
    const style = { ...ls.style, color: FACILITY_LABEL_COLORS[f.kind] ?? ls.style.color };
    const label = facilityText(f, stage);
    const size = measure(label, style, false);
    const anchors = symbols.filter((s) => s.owner === owner).sort((a, b) => b.x - a.x || a.y - b.y);
    let count = 0;
    for (const anchor of anchors) {
      // 画面の位置（ずらしたあと）を基準に、記号の右、左、下、上の順に置く
      const sides: [string, number, number][] = [
        ["r", anchor.sx + FACILITY_LABEL_OFFSET, anchor.sy - size.h / 2],
        ["l", anchor.sx - FACILITY_LABEL_OFFSET - size.w, anchor.sy - size.h / 2],
        ["b", anchor.sx - size.w / 2, anchor.sy + reach + BELOW_GAP],
        ["t", anchor.sx - size.w / 2, anchor.sy - reach - BELOW_GAP - size.h],
      ];
      for (const [side, x0, y0] of sides) {
        const box = { x0, y0, x1: x0 + size.w, y1: y0 + size.h };
        if (!inside(box, vp) || count >= MAX_FACILITY_ALTERNATIVES) continue;
        count++;
        const home = toScreen(camera, vp, anchor.x, anchor.y);
        out.push({
          id: `facility|${anchor.key}|${side}|${label}|${styleKey(style)}`,
          priority: ls.priority,
          text: label,
          style,
          vertical: false,
          angle: 0,
          wx: anchor.x,
          wy: anchor.y,
          ox: x0 - home.x,
          oy: y0 - home.y,
          w: size.w,
          h: size.h,
          box,
          owner: anchor.key,
          group: owner,
        });
      }
    }
  }
  return out;
}

// ---- 置き方 ----

/** 見た目を、画像を覚えておく鍵にする。 */
export function styleKey(s: TextStyle): string {
  return `${s.size}/${s.weight}/${s.spacing}/${s.color}/${s.alpha}/${s.shield ? 1 : 0}`;
}

/** 置けない範囲。パネルと施設の記号である。記号は、名前を持つまとまりの識別子を `owner` に持つ。 */
export type Obstacle = Rect & { owner?: string };

/** 1 画面で調べる候補の数の上限。 */
export const MAX_CANDIDATES = 200;
/** ラベルの外側にとる余白（CSS ピクセル）。 */
export const LABEL_MARGIN = 4;

const overlaps = (a: Rect, b: Rect) => a.x0 < b.x1 && b.x0 < a.x1 && a.y0 < b.y1 && b.y0 < a.y1;

/**
 * 優先の高い順に置き、すでに置いたラベルや、パネルの下になる所（obstacles）と重なるものは置かない。
 * 同じ優先のあいだでは、前に出ていたラベル（shown）を先に置く。候補は MAX_CANDIDATES まで調べる。
 */
export function placeLabels(
  candidates: readonly Candidate[],
  shown: ReadonlySet<string>,
  obstacles: readonly Obstacle[],
): Candidate[] {
  const order = candidates
    .map((c, i) => ({ c, i, prev: shown.has(c.id) ? 0 : 1 }))
    .sort((a, b) => a.c.priority - b.c.priority || a.prev - b.prev || a.i - b.i)
    .slice(0, MAX_CANDIDATES);
  const placed: Candidate[] = [];
  const taken: Rect[] = [];
  const doneGroups = new Set<string>();
  for (const { c } of order) {
    if (c.group !== undefined && doneGroups.has(c.group)) continue;
    const m = LABEL_MARGIN;
    const padded = { x0: c.box.x0 - m, y0: c.box.y0 - m, x1: c.box.x1 + m, y1: c.box.y1 + m };
    // 自分のまとまりの記号は、名前を記号の右 15px に置くと余白に入るので、余白をとらずに調べる
    const blocked = obstacles.some((o) =>
      overlaps(o.owner !== undefined && o.owner === c.owner ? c.box : padded, o),
    );
    if (blocked || taken.some((t) => overlaps(padded, t))) {
      continue;
    }
    placed.push(c);
    taken.push(c.box);
    if (c.group !== undefined) doneGroups.add(c.group);
  }
  return placed;
}

// ---- 出し入れ ----

/** ラベルを出すときも消すときも、この時間（ミリ秒）をかけて不透明度を変える。 */
export const FADE_MS = 200;

export type FadingLabel = { label: Candidate; alpha: number; target: number };

/** ラベルを倍率ではなく時間で出し入れする。置いたものは濃くし、置かなくなったものは薄くして消す。 */
export class LabelFader {
  readonly #items = new Map<string, FadingLabel>();

  /** 置いたラベルに入れ替える。前に出ていて置かなくなったものは、その場で薄くしていく。 */
  update(placed: readonly Candidate[]): void {
    for (const item of this.#items.values()) item.target = 0;
    for (const label of placed) {
      const item = this.#items.get(label.id);
      if (item === undefined) {
        this.#items.set(label.id, { label, alpha: 0, target: 1 });
      } else {
        item.label = label;
        item.target = 1;
      }
    }
  }

  /** 時間を dt ミリ秒進める。まだ変わっている途中のものがあれば true を返す。 */
  step(dt: number): boolean {
    const delta = dt / FADE_MS;
    let moving = false;
    for (const [id, item] of this.#items) {
      if (item.alpha < item.target) {
        item.alpha = Math.min(item.target, item.alpha + delta);
      } else if (item.alpha > item.target) {
        item.alpha = Math.max(item.target, item.alpha - delta);
      }
      if (item.alpha === 0 && item.target === 0) {
        this.#items.delete(id);
      } else if (item.alpha !== item.target) {
        moving = true;
      }
    }
    return moving;
  }

  /** 出しているラベル（薄くしている途中のものを含む）。 */
  items(): IterableIterator<FadingLabel> {
    return this.#items.values();
  }

  /** いま出す側にあるラベルの識別子。 */
  shown(): Set<string> {
    const out = new Set<string>();
    for (const [id, item] of this.#items) if (item.target === 1) out.add(id);
    return out;
  }
}
