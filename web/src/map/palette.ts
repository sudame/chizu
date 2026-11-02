// 地図の色の表（docs/map-style.md の「色の表」）のうち、地面、水、選択と仮の表示の色。
// 値は sRGB の 16 進で書き、WebGL に渡すときは 255 で割った [0, 1] の値にする。

/** [r, g, b, a]。どれも 0〜1。 */
export type Color = readonly [number, number, number, number];

/** [r, g, b]。どれも 0〜1。 */
export type Rgb = readonly [number, number, number];

/** `#rrggbb` を [0, 1] の色にする。`alpha` は不透明度で、既定は 1。 */
export function hex(code: string, alpha = 1): Color {
  const m = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(code);
  if (m === null) {
    throw new Error(`色の書き方が違う：${code}`);
  }
  const ch = (i: number) => Number.parseInt(m[i] ?? "0", 16) / 255;
  return [ch(1), ch(2), ch(3), alpha];
}

/** `#rrggbb` を [0, 1] の RGB にする。 */
export function hexRgb(code: string): Rgb {
  const [r, g, b] = hex(code);
  return [r, g, b];
}

// 地面と水
export const GROUND_COLOR = hexRgb("#f4f2ec");
export const UNOWNED_COLOR = hexRgb("#e6e3db");
/** 買っていない土地の斜線。 */
export const UNOWNED_HATCH_COLOR = hexRgb("#d9d5ca");
export const WATER_COLOR = hexRgb("#a8cde5");
/** 水と陸の境目の線。 */
export const SHORE_COLOR = hexRgb("#8fbcd9");
/** チャンクがまだない所（画面の外）。 */
export const OUTSIDE_COLOR = UNOWNED_COLOR;

// 選択と仮の表示
const BLUE = "#1a73e8";
const RED = "#d93025";
/** 置ける仮の表示。 */
export const PREVIEW_OK_COLOR = hex(BLUE, 0.55);
/** 置けない仮の表示。 */
export const PREVIEW_BAD_COLOR = hex(RED, 0.45);
/** 種類を変える道具でなぞった直線。変わる区間より薄くする。 */
export const PREVIEW_LINE_COLOR = hex(BLUE, 0.18);
/** 取り壊す範囲の塗り。 */
export const BULLDOZE_FILL_COLOR = hex(RED, 0.25);
/** 取り壊す範囲の縁。 */
export const BULLDOZE_EDGE_COLOR = hex(RED, 1);
/** 区画を塗れるマスと、買える土地の塗り。 */
export const HINT_FILL_COLOR = hex(BLUE, 0.1);
/** 買える土地の縁。 */
export const HINT_EDGE_COLOR = hex(BLUE, 1);
/** 区画を消す仮の表示。 */
export const CLEAR_ZONE_COLOR = hex("#9aa0a6", 0.5);
/** 区画を塗れるマスに重ねる青の割合。renderer.ts のシェーダーが使う。 */
export const ZONABLE_MIX = 0.1;
export const ZONABLE_COLOR = hexRgb(BLUE);
