// 駐車場に停まっている車（docs/design.md の 3.6「地図の描き方」と、docs/map-style.md の「駐車場」）。
// 停めてある台数は中核が数え（chizu-wasm の parked）、どの枠に置くかは画面が決める。
// 枠は並びの順に、枠の並びの先頭から詰めて埋める。車の色は、建物の位置と枠の番号から決める。
// 描画だけで、乱数は使わない。台数が変わると、同じ枠の車は同じ色のまま、増減した枠だけが変わる。
// 車と人が現れるのと同じ倍率（寄りの段）から出し、細部の段で走る車と同じ窓と屋根を足す。

import { type Lot, lotKey, type Strip } from "./lots.ts";
import { CAR_COLORS, carColorIndex } from "./mover-colors.ts";
import { CAR_FRONT_WINDOW, CAR_REAR_WINDOW } from "./movers.ts";
import type { Color } from "./palette.ts";
import { Geometry } from "./roads.ts";
import { showsFine, showsMovers } from "./stages.ts";

/** 車の長さと幅（マス）。1 マスは 8m で、長さ 4m、幅 1.8m の乗用車にする。枠は 5m × 2.5m である。 */
export const PARKED_CAR_LENGTH = 0.5;
export const PARKED_CAR_WIDTH = 0.225;
/** 車の縁の暗さ。走る車と同じ（車の色の 70%）。 */
const EDGE_SHADE = 0.7;
/** 車の縁の太さ（マス）。 */
const EDGE = 0.012;
/** 窓の幅（車の幅の半分に対する割合）。走る車と同じ。 */
const WINDOW_SIDE = 0.72;
/** 窓を白へ寄せる割合と、屋根を白へ寄せる割合。走る車と同じ。 */
const WINDOW_LIGHTEN = 0.55;
const ROOF_LIGHTEN = 0.15;

export type TileRect = { minX: number; minY: number; maxX: number; maxY: number };

/** 停めてある台数。建物の左上のマスで引く。 */
export function parseParked(data: ArrayLike<number>): Map<string, number> {
  const out = new Map<string, number>();
  for (let i = 0; i + 3 <= data.length; i += 3) {
    out.set(lotKey(data[i] ?? 0, data[i + 1] ?? 0), data[i + 2] ?? 0);
  }
  return out;
}

const mix = (c: readonly [number, number, number], t: number): Color => [
  c[0] + (1 - c[0]) * t,
  c[1] + (1 - c[1]) * t,
  c[2] + (1 - c[2]) * t,
  1,
];
const shade = (c: readonly [number, number, number], k: number): Color => [
  c[0] * k,
  c[1] * k,
  c[2] * k,
  1,
];

/** 建物の左上のマスと枠の番号から、車の色の番号を決める。 */
export function parkedColorIndex(x: number, y: number, slot: number): number {
  return carColorIndex(Math.imul(x, 7919) + Math.imul(y, 104729) + Math.imul(slot, 31));
}

/** 枠 1 つの中心と、車の長い辺の向き。 */
type Slot = { cx: number; cy: number; alongX: boolean; forward: number };

/** 枠の並び 1 本の枠。車の入る側の反対（枠の奥）へ鼻を向ける。 */
export function slotsOf(s: Strip): Slot[] {
  const out: Slot[] = [];
  if (s.count <= 0) {
    return out;
  }
  if (s.openX !== 0) {
    const step = (s.y1 - s.y0) / s.count;
    for (let k = 0; k < s.count; k++) {
      out.push({
        cx: (s.x0 + s.x1) / 2,
        cy: s.y0 + step * (k + 0.5),
        alongX: true,
        forward: -s.openX,
      });
    }
  } else {
    const step = (s.x1 - s.x0) / s.count;
    for (let k = 0; k < s.count; k++) {
      out.push({
        cx: s.x0 + step * (k + 0.5),
        cy: (s.y0 + s.y1) / 2,
        alongX: false,
        forward: -s.openY,
      });
    }
  }
  return out;
}

/** 停めてある台数ぶんの枠。並びの順に先頭から詰める。台数が平面の枠より多い分は建物の中にあるので、描かない。 */
export function filledSlots(lot: Lot, parked: number): { slot: Slot; index: number }[] {
  const out: { slot: Slot; index: number }[] = [];
  let index = 0;
  for (const s of lot.strips) {
    for (const slot of slotsOf(s)) {
      if (index >= parked) {
        return out;
      }
      out.push({ slot, index });
      index++;
    }
  }
  return out;
}

/** 車 1 台を、axis に沿った長方形で置く。u は車の長い辺に沿った座標（前が正、-1〜1）、v は横（-1〜1）。 */
function carBox(
  g: Geometry,
  slot: Slot,
  u0: number,
  u1: number,
  v0: number,
  v1: number,
  color: Color,
  length = PARKED_CAR_LENGTH,
  width = PARKED_CAR_WIDTH,
): void {
  const hl = length / 2;
  const hw = width / 2;
  const a = slot.forward * u0 * hl;
  const b = slot.forward * u1 * hl;
  const lo = Math.min(a, b);
  const hi = Math.max(a, b);
  if (slot.alongX) {
    g.box(slot.cx + lo, slot.cy + v0 * hw, slot.cx + hi, slot.cy + v1 * hw, color);
  } else {
    g.box(slot.cx + v0 * hw, slot.cy + lo, slot.cx + v1 * hw, slot.cy + hi, color);
  }
}

/** 停めてある車の頂点。車の色の面は寄りから、窓と屋根は細部の段から出す。 */
export type ParkedMeshes = { cars: Geometry; fine: Geometry };

const overlaps = (r: TileRect, x0: number, y0: number, x1: number, y1: number) =>
  x1 >= r.minX && x0 <= r.maxX && y1 >= r.minY && y0 <= r.maxY;

/**
 * 範囲 `rect` に枠のある駐車場の、停まっている車を頂点にする。細部の頂点（`fine`）は `withFine` のときだけ作る。
 */
export function parkedGeometry(
  lots: ReadonlyMap<string, Lot>,
  keys: ReadonlyMap<string, number>,
  rect: TileRect,
  withFine: boolean,
): ParkedMeshes {
  const origin = { x: rect.minX, y: rect.minY };
  const meshes: ParkedMeshes = { cars: new Geometry(origin), fine: new Geometry(origin) };
  for (const [key, parked] of keys) {
    const lot = lots.get(key);
    if (lot === undefined || parked <= 0) {
      continue;
    }
    const near = lot.strips.some((s) => overlaps(rect, s.x0, s.y0, s.x1, s.y1));
    if (!near) {
      continue;
    }
    const [bx = 0, by = 0] = key.split(",").map(Number);
    for (const { slot, index } of filledSlots(lot, parked)) {
      if (!overlaps(rect, slot.cx, slot.cy, slot.cx, slot.cy)) {
        continue;
      }
      const base = CAR_COLORS[parkedColorIndex(bx, by, index)] ?? CAR_COLORS[0] ?? [0.5, 0.5, 0.5];
      const g = meshes.cars;
      const e = EDGE / (PARKED_CAR_LENGTH / 2);
      const ew = EDGE / (PARKED_CAR_WIDTH / 2);
      carBox(g, slot, -1, 1, -1, 1, shade(base, EDGE_SHADE));
      carBox(g, slot, -1 + e, 1 - e, -1 + ew, 1 - ew, mix(base, 0));
      if (withFine) {
        const f = meshes.fine;
        const [fa = 0, fb = 0] = CAR_FRONT_WINDOW;
        const [ra = 0, rb = 0] = CAR_REAR_WINDOW;
        const glass = mix(base, WINDOW_LIGHTEN);
        carBox(f, slot, fa, fb, -WINDOW_SIDE, WINDOW_SIDE, glass);
        carBox(f, slot, ra, rb, -WINDOW_SIDE, WINDOW_SIDE, glass);
        carBox(f, slot, rb, fa, -WINDOW_SIDE, WINDOW_SIDE, mix(base, ROOF_LIGHTEN));
      }
    }
  }
  return meshes;
}

const contains = (outer: TileRect, inner: TileRect) =>
  outer.minX <= inner.minX &&
  outer.minY <= inner.minY &&
  outer.maxX >= inner.maxX &&
  outer.maxY >= inner.maxY;

/**
 * 停まっている車の頂点を、要るときだけ作る。寄りの段より引いているあいだは作らない。
 * 見えている範囲に画面の 1/4 ずつの余白を足した範囲のぶんだけ作り、画面がその範囲の中にあるうちは作り直さない。
 * 台数か駐車場の形が変わったとき、細部の段に出入りしたときは作り直す。
 */
export class ParkedCache {
  #lots: ReadonlyMap<string, Lot> = new Map();
  #parked: ReadonlyMap<string, number> = new Map();
  #rect: TileRect | null = null;
  #fine = false;
  #dirty = true;
  /** 頂点を作った回数。test:wasm が、引いたときに作っていないかを見るのに使う。 */
  builds = 0;

  setLots(lots: ReadonlyMap<string, Lot>): void {
    this.#lots = lots;
    this.#dirty = true;
  }

  setParked(parked: ReadonlyMap<string, number>): void {
    this.#parked = parked;
    this.#dirty = true;
  }

  /** 次に要るときに、作り直す（WebGL のコンテキストを作り直したときなど）。 */
  invalidate(): void {
    this.#dirty = true;
  }

  update(zoom: number, view: TileRect): ParkedMeshes | null {
    if (!showsMovers(zoom)) {
      return null;
    }
    const fine = showsFine(zoom);
    const tooWide =
      this.#rect !== null && this.#rect.maxX - this.#rect.minX > 2.5 * (view.maxX - view.minX) + 4;
    const outside = this.#rect === null || !contains(this.#rect, view) || tooWide;
    if (!this.#dirty && !outside && fine === this.#fine) {
      return null;
    }
    if (outside) {
      const mx = (view.maxX - view.minX) / 4 + 1;
      const my = (view.maxY - view.minY) / 4 + 1;
      this.#rect = {
        minX: Math.floor(view.minX - mx),
        minY: Math.floor(view.minY - my),
        maxX: Math.ceil(view.maxX + mx),
        maxY: Math.ceil(view.maxY + my),
      };
    }
    this.#dirty = false;
    this.#fine = fine;
    this.builds++;
    return parkedGeometry(this.#lots, this.#parked, this.#rect as TileRect, fine);
  }
}
