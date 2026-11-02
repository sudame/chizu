// 施設の丸い記号（docs/map-style.md の「施設の記号」）。丸は白い縁と影を付け、中に白い絵を入れる。
// 引きでは小さな丸だけ、標準では絵つきの丸でまとまりごとに 1 つ、寄りでは施設 1 つずつに 1 つ出す。
// どこに何を出すかの計算は DOM に頼らず、描くのは Canvas 2D の重ね板（label-layer.ts）が受け持つ。

import { facilityOwner } from "./labels.ts";
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
  centerX: number;
  centerY: number;
  members: readonly { x: number; y: number }[];
};

/** 記号 1 つ。位置はマスの単位のワールド座標で、`owner` はまとまりの識別子（labels.ts の facilityOwner）である。 */
export type FacilitySymbol = {
  owner: string;
  kind: number;
  x: number;
  y: number;
  /** 記号全体の濃さ。まとまりの記号と 1 つずつの記号を入れ替えるあいだ、0〜1 で変わる。 */
  alpha: number;
};

/** 倍率ごとの、丸の半径と絵の濃さ。引きの小さな丸から、標準の絵つきの丸へ溶け込む。 */
export function symbolLook(zoom: number): { radius: number; glyphAlpha: number } {
  const w = stageWeights(zoom).marks;
  return { radius: SYMBOL_RADIUS_FAR + (SYMBOL_RADIUS - SYMBOL_RADIUS_FAR) * w, glyphAlpha: w };
}

/**
 * 倍率 zoom で出す記号。2 棟以上のまとまりは、標準までまとまりの中心に 1 つ出し、
 * 寄りに移るあいだにまとまりの記号を薄くして、施設 1 つずつの記号を濃くする。
 * 1 棟だけのまとまりは、どの段でも同じ位置に 1 つだけ出す。
 */
export function facilitySymbols(groups: readonly SymbolGroup[], zoom: number): FacilitySymbol[] {
  const detail = stageWeights(zoom).detail;
  const out: FacilitySymbol[] = [];
  for (const g of groups) {
    const owner = facilityOwner(g);
    if (g.count <= 1) {
      out.push({ owner, kind: g.kind, x: g.centerX, y: g.centerY, alpha: 1 });
      continue;
    }
    if (detail < 1) {
      out.push({ owner, kind: g.kind, x: g.centerX, y: g.centerY, alpha: 1 - detail });
    }
    if (detail > 0) {
      for (const m of g.members) {
        out.push({ owner, kind: g.kind, x: m.x, y: m.y, alpha: detail });
      }
    }
  }
  return out;
}

/** 記号が画面で占める範囲（縁を含む）。ラベルを置けない範囲に使う。 */
export function symbolBox(
  s: FacilitySymbol,
  camera: { x: number; y: number; zoom: number },
  viewport: { width: number; height: number },
  radius: number,
): { x0: number; y0: number; x1: number; y1: number } {
  const sx = (s.x - camera.x) * camera.zoom + viewport.width / 2;
  const sy = (s.y - camera.y) * camera.zoom + viewport.height / 2;
  const r = radius + SYMBOL_BORDER;
  return { x0: sx - r, y0: sy - r, x1: sx + r, y1: sy + r + SYMBOL_SHADOW_PX };
}

/** 丸い記号を、画面の (x, y)（CSS ピクセル）に描く。ctx の座標は CSS ピクセルにそろえておく。 */
export function drawSymbol(
  ctx: CanvasRenderingContext2D,
  s: FacilitySymbol,
  x: number,
  y: number,
  look: { radius: number; glyphAlpha: number },
): void {
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
