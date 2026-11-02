// 地図を見るカメラ。位置はマスを単位にしたワールド座標で、画面の側だけが持つ。中核には渡さない。

import { CHUNK_SIDE } from "../protocol/messages.ts";

/** 拡大の上限と下限（1 マスを何 CSS ピクセルで描くか）。下限では 1 チャンクが 64 ピクセルになる。 */
export const MIN_ZOOM = 1;
export const MAX_ZOOM = 128;

export type Viewport = { width: number; height: number };
export type Point = { x: number; y: number };
/** チャンク座標の範囲。両端を含む。 */
export type ChunkRange = { minCx: number; maxCx: number; minCy: number; maxCy: number };

const clamp = (v: number, lo: number, hi: number) => Math.min(Math.max(v, lo), hi);

export class Camera {
  /** 画面の中心に来るワールド座標。 */
  x: number;
  y: number;
  zoom: number;

  constructor(x = 0, y = 0, zoom = 8) {
    this.x = x;
    this.y = y;
    this.zoom = clamp(zoom, MIN_ZOOM, MAX_ZOOM);
  }

  screenToWorld(sx: number, sy: number, viewport: Viewport): Point {
    return {
      x: this.x + (sx - viewport.width / 2) / this.zoom,
      y: this.y + (sy - viewport.height / 2) / this.zoom,
    };
  }

  worldToScreen(wx: number, wy: number, viewport: Viewport): Point {
    return {
      x: (wx - this.x) * this.zoom + viewport.width / 2,
      y: (wy - this.y) * this.zoom + viewport.height / 2,
    };
  }

  /** 画面の上で (dx, dy) ピクセルだけ引きずる。地図は指に付いて動く。 */
  pan(dx: number, dy: number): void {
    this.x -= dx / this.zoom;
    this.y -= dy / this.zoom;
  }

  /** 画面の点 (sx, sy) の下にあるワールドの点を動かさずに、拡大率を factor 倍する。 */
  zoomAt(sx: number, sy: number, factor: number, viewport: Viewport): void {
    const anchor = this.screenToWorld(sx, sy, viewport);
    this.zoom = clamp(this.zoom * factor, MIN_ZOOM, MAX_ZOOM);
    this.x = anchor.x - (sx - viewport.width / 2) / this.zoom;
    this.y = anchor.y - (sy - viewport.height / 2) / this.zoom;
  }

  /** 画面に少しでも入るチャンクの範囲。 */
  visibleChunks(viewport: Viewport): ChunkRange {
    const min = this.screenToWorld(0, 0, viewport);
    const max = this.screenToWorld(viewport.width, viewport.height, viewport);
    // 右端と下端はちょうど境目に来たとき、次のチャンクを含めない
    const last = (v: number) => Math.ceil(v / CHUNK_SIDE) - 1;
    return {
      minCx: Math.floor(min.x / CHUNK_SIDE),
      maxCx: Math.max(Math.floor(min.x / CHUNK_SIDE), last(max.x)),
      minCy: Math.floor(min.y / CHUNK_SIDE),
      maxCy: Math.max(Math.floor(min.y / CHUNK_SIDE), last(max.y)),
    };
  }
}

/** 範囲の中のチャンクを、ワールドの点 center に近い順に並べる。 */
export function chunksByDistance(range: ChunkRange, center: Point): [number, number][] {
  const out: [number, number][] = [];
  for (let cy = range.minCy; cy <= range.maxCy; cy++) {
    for (let cx = range.minCx; cx <= range.maxCx; cx++) {
      out.push([cx, cy]);
    }
  }
  const half = CHUNK_SIDE / 2;
  const distance = ([cx, cy]: [number, number]) =>
    Math.hypot(cx * CHUNK_SIDE + half - center.x, cy * CHUNK_SIDE + half - center.y);
  return out.sort((a, b) => distance(a) - distance(b));
}
