// 左下の縮尺の物差し。1 マスは 8m で、倍率 z のとき 1 ピクセルは 8 / z m になる。
// 長さは 1、2、5 の切りのよい値（m か km）にし、上限のピクセルに収まるいちばん長いものを選ぶ。

/** 1 マスの長さ（m）。 */
export const METERS_PER_TILE = 8;

/** 物差しの線の長さの上限（CSS ピクセル）。 */
export const SCALE_MAX_PX = 140;

export type ScaleBar = {
  /** 線が表す長さ（m）。 */
  meters: number;
  /** 線の長さ（CSS ピクセル）。 */
  px: number;
  /** 「100 m」「2 km」のような文字。 */
  label: string;
};

/** 長さ（m）の見出し。1000m 以上は km にする。 */
export function formatLength(meters: number): string {
  return meters >= 1000 ? `${meters / 1000} km` : `${meters} m`;
}

/** 倍率に合う物差しを決める。 */
export function scaleBar(zoom: number, maxPx: number = SCALE_MAX_PX): ScaleBar {
  const metersPerPixel = METERS_PER_TILE / zoom;
  const maxMeters = maxPx * metersPerPixel;
  // maxMeters 以下の、1、2、5 に 10 の冪をかけた値のうち最大のもの
  const decade = 10 ** Math.floor(Math.log10(maxMeters));
  const meters = [5, 2, 1].map((m) => m * decade).find((m) => m <= maxMeters) ?? decade;
  return { meters, px: Math.round(meters / metersPerPixel), label: formatLength(meters) };
}

/** 物差しの要素を書き直す。毎フレーム呼ぶので、変わらないときは何もしない。 */
export function setupScaleBar(root: HTMLElement): (zoom: number) => void {
  const label = root.querySelector<HTMLElement>("[data-scale-label]");
  const line = root.querySelector<HTMLElement>("[data-scale-line]");
  if (label === null || line === null) {
    throw new Error("縮尺の物差しの要素が足りない");
  }
  let shown = "";
  return (zoom) => {
    const bar = scaleBar(zoom);
    const key = `${bar.label}/${bar.px}`;
    if (key === shown) {
      return;
    }
    shown = key;
    label.textContent = bar.label;
    line.style.width = `${bar.px}px`;
    // test:wasm が読む
    root.dataset.meters = String(bar.meters);
    root.dataset.px = String(bar.px);
  };
}
