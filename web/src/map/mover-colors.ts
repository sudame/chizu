// 移動体の色。車は移動体の ID から決めた色にばらつかせ、歩く人は車にない赤紫にする。
// 色はどれも、道路の車道と歩道の色から見分けられる明るさか鮮やかさにする。
// test:wasm と、画面のシェーダーの両方がこの値を使う。

export type Rgb = readonly [number, number, number];

export const CAR_COLORS: readonly Rgb[] = [
  [0.94, 0.94, 0.92],
  [0.72, 0.75, 0.79],
  [0.82, 0.2, 0.18],
  [0.2, 0.42, 0.82],
  [0.96, 0.8, 0.2],
  [0.15, 0.62, 0.62],
  [0.93, 0.5, 0.15],
  [0.55, 0.78, 0.35],
];

export const WALKER_COLOR: Rgb = [0.93, 0.27, 0.64];

/** 移動体の ID から、車の色の番号を決める。movers.ts のシェーダーと同じ計算である。 */
export function carColorIndex(id: number): number {
  let h = Math.imul(id >>> 0, 0x9e3779b1);
  h = Math.imul(h ^ (h >>> 16), 0x85ebca6b);
  h = (h ^ (h >>> 13)) >>> 0;
  return h % CAR_COLORS.length;
}
