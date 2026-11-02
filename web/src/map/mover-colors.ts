// 移動体の色（docs/map-style.md の「移動体」）。車は移動体の ID から決めた色にばらつかせ、歩く人は車にない赤紫にする。
// 色はどれも鮮やかな色にし、地面、道路の塗りと縁、歩道、路面の線のどの色からも離す。
// 信号の台と灯火（青、黄、赤）の色からも離す（mover-colors.test.ts）。
// 白、銀、黄、橙の車は、道路の白と黄、高速道路の橙、横断歩道の縞と見分けられないので使わない。
// test:wasm と、画面のシェーダーの両方がこの値を使う。

import { hexRgb, type Rgb } from "./palette.ts";

export type { Rgb };

export const CAR_COLORS: readonly Rgb[] = [
  "#a82e25",
  "#3b6fcf",
  "#0f6f78",
  "#7552c2",
  "#6b8e23",
  "#3e8bc0",
  "#85593a",
  "#6b7a90",
].map(hexRgb);

export const WALKER_COLOR: Rgb = hexRgb("#d6336c");

/** 移動体の ID から、車の色の番号を決める。movers.ts のシェーダーと同じ計算である。 */
export function carColorIndex(id: number): number {
  let h = Math.imul(id >>> 0, 0x9e3779b1);
  h = Math.imul(h ^ (h >>> 16), 0x85ebca6b);
  h = (h ^ (h >>> 13)) >>> 0;
  return h % CAR_COLORS.length;
}

/** 画素の色（0〜255）を移動体の色と見なすときの、各色の差の上限。 */
export const MOVER_MATCH_TOLERANCE = 24;

/**
 * 画素の色（0〜255）が、車か歩く人の色のどれかに近いか。test:wasm が移動体が描かれたかを見るのに使う。
 * 道路の色はどの移動体の色からも、この差の 2 倍より離してある（mover-colors.test.ts）。
 */
export function moverColorAt(pixel: readonly number[]): boolean {
  return [...CAR_COLORS, WALKER_COLOR].some((m) =>
    m.every((v, i) => Math.abs(v * 255 - (pixel[i] ?? 0)) <= MOVER_MATCH_TOLERANCE),
  );
}
