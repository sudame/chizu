// 縮尺の段（docs/map-style.md の「縮尺の段」と「溶け込み」）。引き、標準、寄りの描き分けと、
// 段のあいだで前の描き方と次の描き方を薄く出し入れする重みを、倍率から求める。

/** 倍率の幅 [a, b]。重みは a で 0、b で 1 になる。 */
export type Band = readonly [number, number];

/** 小の道路の濃さが 55% から 100% になる幅。 */
export const MINOR_ROAD_BAND: Band = [2, 5];
/** 小の道路の、引いたときの濃さ。 */
export const MINOR_ROAD_MIN_ALPHA = 0.55;
/** 街区の面から建物のまとまりへ移る幅。 */
export const BLOCK_BAND: Band = [3, 5];
/**
 * 道路の線から車道と歩道へ（路面の線、横断歩道、信号も）、建物のまとまりから 1 軒ずつへ、
 * 空き地の模様が現れる幅。
 */
export const DETAIL_BAND: Band = [10, 14];
/** 車と人が現れる幅。 */
export const MOVER_BAND: Band = [12, 16];

/** これより引いたら移動体を描かず、Worker にも移動体の範囲を頼まない。 */
export const MIN_MOVER_ZOOM = MOVER_BAND[0];

/** GLSL の smoothstep と同じ。a で 0、b で 1 になり、両端でなめらかにつなぐ。 */
export function smoothstep(a: number, b: number, x: number): number {
  const t = Math.min(Math.max((x - a) / (b - a), 0), 1);
  return t * t * (3 - 2 * t);
}

const weight = ([a, b]: Band, zoom: number) => smoothstep(a, b, zoom);

/** 倍率ごとの、描き分けの重み。どれも 0〜1 である。 */
export type StageWeights = {
  /** 小の道路の線の濃さ（55%〜100%）。 */
  minorRoad: number;
  /** 街区の面の濃さ。引きで 1、標準に入ると 0。 */
  blocks: number;
  /** 建物のまとまりの濃さ。引きで 0、標準で 1、寄りに入ると 0。 */
  clusters: number;
  /** 寄りの描き方（車線、歩道、路面の線、横断歩道、信号、1 軒ずつの建物の縁と影、空き地の模様）の濃さ。 */
  detail: number;
  /** 車と人の濃さ。 */
  movers: number;
  /** 電気や水道が届いていない印の濃さ。引きでは描かない。 */
  marks: number;
};

export function stageWeights(zoom: number): StageWeights {
  const block = weight(BLOCK_BAND, zoom);
  const detail = weight(DETAIL_BAND, zoom);
  return {
    minorRoad: MINOR_ROAD_MIN_ALPHA + (1 - MINOR_ROAD_MIN_ALPHA) * weight(MINOR_ROAD_BAND, zoom),
    blocks: 1 - block,
    clusters: block * (1 - detail),
    detail,
    movers: showsMovers(zoom) ? weight(MOVER_BAND, zoom) : 0,
    marks: block,
  };
}

/** 移動体を描き、Worker に範囲を頼む倍率か。 */
export function showsMovers(zoom: number): boolean {
  return zoom >= MIN_MOVER_ZOOM;
}
