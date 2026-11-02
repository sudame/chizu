// 移動体の色。車は移動体の ID から決めた色にばらつかせ、歩く人は車にない赤紫にする。
// 色はどれも鮮やかな色にし、道路の車道、歩道、路肩、路面の線（白、黄）のどの色からも離す。
// 白や銀の車は、横断歩道の縞や歩道と見分けられないので使わない。
// test:wasm と、画面のシェーダーの両方がこの値を使う。

export type Rgb = readonly [number, number, number];

export const CAR_COLORS: readonly Rgb[] = [
  [0.82, 0.2, 0.18],
  [0.2, 0.42, 0.82],
  [0.12, 0.62, 0.6],
  [0.52, 0.3, 0.8],
  [0.62, 0.86, 0.18],
  [0.45, 0.76, 0.96],
  [0.55, 0.32, 0.14],
  [0.1, 0.45, 0.25],
];

export const WALKER_COLOR: Rgb = [0.93, 0.27, 0.64];

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
