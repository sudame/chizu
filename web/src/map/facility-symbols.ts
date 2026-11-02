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

/**
 * 記号の画面の位置を求める。まとまりの記号と 1 棟の記号は、ほかの記号と丸が重なるとき、
 * あとの記号を重ならない所までずらす（施設 1 つずつの記号は敷地の中心に置いたままにする）。
 */
export function layoutSymbols(
  groups: readonly SymbolGroup[],
  camera: { x: number; y: number; zoom: number },
  viewport: { width: number; height: number },
): PlacedSymbol[] {
  const placed: PlacedSymbol[] = facilitySymbols(groups, camera.zoom).map((s) => ({
    ...s,
    sx: (s.x - camera.x) * camera.zoom + viewport.width / 2,
    sy: (s.y - camera.y) * camera.zoom + viewport.height / 2,
  }));
  const movable = placed
    .filter((s) => s.phase !== "member")
    .sort((a, b) => a.sy - b.sy || a.sx - b.sx || (a.key < b.key ? -1 : 1));
  const min = 2 * (symbolLook(camera.zoom).radius + SYMBOL_BORDER) + 1;
  for (let pass = 0; pass < 3; pass++) {
    for (let i = 0; i < movable.length; i++) {
      for (let j = 0; j < i; j++) {
        const a = movable[j] as PlacedSymbol;
        const b = movable[i] as PlacedSymbol;
        const dx = b.sx - a.sx;
        const dy = b.sy - a.sy;
        const d = Math.hypot(dx, dy);
        if (d >= min) continue;
        const [ux, uy] = d === 0 ? [1, 0] : [dx / d, dy / d];
        b.sx = a.sx + ux * min;
        b.sy = a.sy + uy * min;
      }
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
