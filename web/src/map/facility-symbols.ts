// 施設の丸い記号（docs/map-style.md の「施設の記号」）。丸は白い縁と影を付け、中に白い絵を入れる。
// 引きでは小さな丸だけ、標準では絵つきの丸でまとまりごとに 1 つ、寄りでは施設 1 つずつに 1 つ出す。
// どこに何を出すかの計算は DOM に頼らず、描くのは Canvas 2D の重ね板（label-layer.ts）が受け持つ。

import { type FacilityGlyph, facilityStyle } from "./palette.ts";
import { stageWeights } from "./stages.ts";

/** 丸の半径（CSS ピクセル）。引きは小さな丸、標準と寄りは絵のある丸にする。 */
export const SYMBOL_RADIUS_FAR = 6;
export const SYMBOL_RADIUS = 10;
/** 丸の外の白い縁の太さ。 */
export const SYMBOL_BORDER = 1.5;
/** 影は丸を下へこのピクセル数ずらし、`#3c4043` の 30% で描く。 */
export const SYMBOL_SHADOW_PX = 1;
export const SYMBOL_SHADOW_COLOR = "rgba(60, 64, 67, 0.3)";
/** 絵の大きさ（丸の半径に対する割合）。 */
export const GLYPH_SCALE = 0.55;

/** 絵の形。座標は [-1, 1] の範囲で、y は下向きが正である。画面（Path2D）と凡例（SVG）が同じ文字列を使う。 */
export const GLYPH_PATHS: Readonly<Record<FacilityGlyph, string>> = {
  // 稲妻。上の三角形と下の三角形を、真ん中でつなぐ
  bolt: "M 0.3 -1 L -0.65 0.15 L -0.05 0.15 L -0.3 1 L 0.65 -0.2 L 0.05 -0.2 Z",
  // 水滴。上がとがり、下が丸い
  drop: "M 0 -1 C 0.1 -0.6 0.65 -0.15 0.65 0.35 A 0.65 0.65 0 0 1 -0.65 0.35 C -0.65 -0.15 -0.1 -0.6 0 -1 Z",
};

/** 記号を出すまとまり。labels.ts の FacilityLabel がそのまま当てはまる。 */
export type SymbolGroup = {
  kind: number;
  x: number;
  y: number;
  count: number;
  /** まとまりの記号を置くワールド座標。まとまりのいずれかの施設の敷地の中心である。 */
  centerX: number;
  centerY: number;
  members: readonly { x: number; y: number }[];
};

/** まとまりの識別子。記号と名前の対応づけに使う。 */
export const facilityOwner = (f: Pick<SymbolGroup, "kind" | "x" | "y">) =>
  `${f.kind}|${f.x},${f.y}`;

/**
 * 記号の種類。1 棟だけのまとまりの記号（single）は、どの段でも同じ位置に出す。
 * 2 棟以上のまとまりは、標準までまとまりの記号（group）を出し、寄りでは施設 1 つずつの記号（member）を出す。
 */
export type SymbolPhase = "single" | "group" | "member";

/** 記号 1 つ。位置はマスの単位のワールド座標で、`owner` はまとまりの識別子である。 */
export type FacilitySymbol = {
  owner: string;
  /** 記号ごとの識別子。 */
  key: string;
  kind: number;
  phase: SymbolPhase;
  x: number;
  y: number;
  /** 記号全体の濃さ。まとまりの記号と 1 つずつの記号を入れ替えるあいだ、0〜1 で変わる。 */
  alpha: number;
};

/** 画面の位置（CSS ピクセル）を持つ記号。重なる記号をずらしたあとの位置である。 */
export type PlacedSymbol = FacilitySymbol & { sx: number; sy: number };

/** 倍率ごとの、丸の半径と絵の濃さ。引きの小さな丸から、標準の絵つきの丸へ溶け込む。 */
export function symbolLook(zoom: number): { radius: number; glyphAlpha: number } {
  const w = stageWeights(zoom).marks;
  return { radius: SYMBOL_RADIUS_FAR + (SYMBOL_RADIUS - SYMBOL_RADIUS_FAR) * w, glyphAlpha: w };
}

/**
 * 倍率 zoom で出す記号。2 棟以上のまとまりは、標準までまとまりの記号を 1 つ出し、
 * 寄りに移るあいだにまとまりの記号を薄くして、施設 1 つずつの記号を濃くする。
 */
export function facilitySymbols(groups: readonly SymbolGroup[], zoom: number): FacilitySymbol[] {
  const detail = stageWeights(zoom).detail;
  const out: FacilitySymbol[] = [];
  for (const g of groups) {
    const owner = facilityOwner(g);
    const at = (phase: SymbolPhase, x: number, y: number, alpha: number) =>
      out.push({ owner, key: `${owner}@${x},${y}`, kind: g.kind, phase, x, y, alpha });
    if (g.count <= 1) {
      at("single", g.centerX, g.centerY, 1);
      continue;
    }
    if (detail < 1) at("group", g.centerX, g.centerY, 1 - detail);
    if (detail > 0) {
      for (const m of g.members) at("member", m.x, m.y, detail);
    }
  }
  return out;
}

/** 画面の外でも、この幅（CSS ピクセル）の余白の中の記号は、出す記号に入れる。 */
const VIEW_MARGIN = 120;
/** ずらす先と間引く記号を決める倍率は、この刻みに丸める。パンでは変わらず、ズームでは刻みごとに決め直す。 */
const LAYOUT_ZOOM_STEP = 0.25;

/** ずらした量（CSS ピクセル）。null は、重なりを解けないので出さない記号である。 */
type Shift = { dx: number; dy: number } | null;

/**
 * まとまりの記号と 1 棟の記号の、ずらす先と間引く記号。位置は地図の座標と丸めた倍率だけで決め、
 * 画面の位置には頼らない。並べる順は地図の座標とまとまりの鍵である。
 * ほかの記号と丸が重なるとき、あとの記号を、重ならない所までずらす（もとの位置から丸の直径まで）。
 * それでも重なるときは、あとの記号を出さない（引きでは間引く）。施設 1 つずつの記号は、敷地の中心に置いたままにする。
 */
export function symbolShifts(groups: readonly SymbolGroup[], zoom: number): Map<string, Shift> {
  const bz = Math.max(LAYOUT_ZOOM_STEP, Math.round(zoom / LAYOUT_ZOOM_STEP) * LAYOUT_ZOOM_STEP);
  const min = 2 * (symbolLook(bz).radius + SYMBOL_BORDER) + 1;
  // どの倍率でも、まとまりの記号と 1 棟の記号が両方ある前提で決める（まとまりの記号が消える寄りでも同じ結果にする）
  const items: { key: string; x: number; y: number }[] = [];
  for (const g of groups) {
    const owner = facilityOwner(g);
    items.push({ key: `${owner}@${g.centerX},${g.centerY}`, x: g.centerX * bz, y: g.centerY * bz });
  }
  items.sort((a, b) => a.y - b.y || a.x - b.x || (a.key < b.key ? -1 : 1));
  // 並べた記号を、格子（1 辺 min）に入れて、近くの記号とだけ比べる
  const cells = new Map<string, { x: number; y: number }[]>();
  const cell = (v: number) => Math.floor(v / min);
  const near = (x: number, y: number) => {
    const out: { x: number; y: number }[] = [];
    for (let dx = -1; dx <= 1; dx++) {
      for (let dy = -1; dy <= 1; dy++) {
        out.push(...(cells.get(`${cell(x) + dx},${cell(y) + dy}`) ?? []));
      }
    }
    return out;
  };
  const result = new Map<string, Shift>();
  for (const item of items) {
    let { x, y } = item;
    const overlapping = () => near(x, y).filter((a) => Math.hypot(x - a.x, y - a.y) < min - 1e-6);
    for (let pass = 0; pass < 4; pass++) {
      const hits = overlapping();
      if (hits.length === 0) break;
      for (const a of hits) {
        const d = Math.hypot(x - a.x, y - a.y);
        const [ux, uy] = d === 0 ? [1, 0] : [(x - a.x) / d, (y - a.y) / d];
        x = a.x + ux * min;
        y = a.y + uy * min;
        // もとの位置から、丸の直径より離さない
        const away = Math.hypot(x - item.x, y - item.y);
        if (away > min) {
          x = item.x + ((x - item.x) / away) * min;
          y = item.y + ((y - item.y) / away) * min;
        }
      }
    }
    if (overlapping().length > 0) {
      result.set(item.key, null);
      continue;
    }
    const k = `${cell(x)},${cell(y)}`;
    const list = cells.get(k);
    if (list === undefined) cells.set(k, [{ x, y }]);
    else list.push({ x, y });
    result.set(item.key, { dx: x - item.x, dy: y - item.y });
  }
  return result;
}

// ずらす先は、まとまりの並びと丸めた倍率ごとに 1 度だけ求め、パンでは求め直さない
let shiftCache: {
  groups: readonly SymbolGroup[];
  zoom: number;
  shifts: Map<string, Shift>;
} | null = null;
function cachedShifts(groups: readonly SymbolGroup[], zoom: number): Map<string, Shift> {
  const bz = Math.round(zoom / LAYOUT_ZOOM_STEP);
  if (shiftCache === null || shiftCache.groups !== groups || shiftCache.zoom !== bz) {
    shiftCache = { groups, zoom: bz, shifts: symbolShifts(groups, zoom) };
  }
  return shiftCache.shifts;
}

/**
 * 記号の画面の位置を求める。ずらし方はカメラの位置に頼らず（symbolShifts）、ここでは地図の位置に足して
 * 画面の座標にするだけにする。画面と余白の中の記号だけを返す。
 */
export function layoutSymbols(
  groups: readonly SymbolGroup[],
  camera: { x: number; y: number; zoom: number },
  viewport: { width: number; height: number },
): PlacedSymbol[] {
  const shifts = cachedShifts(groups, camera.zoom);
  const m = VIEW_MARGIN;
  const placed: PlacedSymbol[] = [];
  for (const s of facilitySymbols(groups, camera.zoom)) {
    let shift: Shift = { dx: 0, dy: 0 };
    if (s.phase !== "member") {
      // null は出さない記号、見つからないときはずらさない
      shift = shifts.has(s.key) ? (shifts.get(s.key) ?? null) : { dx: 0, dy: 0 };
      if (shift === null) continue;
    }
    const sx = (s.x - camera.x) * camera.zoom + viewport.width / 2 + shift.dx;
    const sy = (s.y - camera.y) * camera.zoom + viewport.height / 2 + shift.dy;
    if (sx >= -m && sy >= -m && sx <= viewport.width + m && sy <= viewport.height + m) {
      placed.push({ ...s, sx, sy });
    }
  }
  return placed;
}

/** 名前を置く段（寄りか）で、いま出している記号。標準まではまとまりの記号、寄りは 1 つずつの記号である。 */
export function activeSymbols(placed: readonly PlacedSymbol[], near: boolean): PlacedSymbol[] {
  return placed.filter((s) => s.phase !== (near ? "group" : "member"));
}

/** 記号が画面で占める範囲（縁と影を含む）。ラベルを置けない範囲に使う。 */
export function symbolBox(
  s: { sx: number; sy: number },
  radius: number,
): { x0: number; y0: number; x1: number; y1: number } {
  const r = radius + SYMBOL_BORDER;
  return { x0: s.sx - r, y0: s.sy - r, x1: s.sx + r, y1: s.sy + r + SYMBOL_SHADOW_PX };
}

/** 丸い記号を、画面の (x, y)（CSS ピクセル）に描く。ctx の座標は CSS ピクセルにそろえておく。 */
export function drawSymbol(
  ctx: CanvasRenderingContext2D,
  s: PlacedSymbol,
  look: { radius: number; glyphAlpha: number },
): void {
  const { sx: x, sy: y } = s;
  const style = facilityStyle(s.kind);
  const circle = (cy: number, r: number) => {
    ctx.beginPath();
    ctx.arc(x, cy, r, 0, Math.PI * 2);
    ctx.fill();
  };
  ctx.globalAlpha = s.alpha;
  ctx.fillStyle = SYMBOL_SHADOW_COLOR;
  circle(y + SYMBOL_SHADOW_PX, look.radius + SYMBOL_BORDER);
  ctx.fillStyle = "#ffffff";
  circle(y, look.radius + SYMBOL_BORDER);
  ctx.fillStyle = style.circle;
  circle(y, look.radius);
  if (look.glyphAlpha > 0) {
    ctx.globalAlpha = s.alpha * look.glyphAlpha;
    ctx.fillStyle = "#ffffff";
    ctx.save();
    ctx.translate(x, y);
    const k = look.radius * GLYPH_SCALE;
    ctx.scale(k, k);
    ctx.fill(new Path2D(GLYPH_PATHS[style.glyph]));
    ctx.restore();
  }
}
