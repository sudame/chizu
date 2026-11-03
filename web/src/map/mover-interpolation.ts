// 移動体の位置を、前の tick と今の tick のあいだで補間する。
// 前の位置は移動体ごとに画面の側で覚える。三重バッファは今の tick の位置だけを持つ。
// 位置は基準のチャンクが変わっても比べられるよう、ワールド座標（マス）の f64 で持つ。
//
// 向きも、前の tick の向きから今の tick の向きへ近いほうに回して補間する。交差点を曲がる車の向きが、
// tick ごとに段になって変わらないようにする。
//
// 補間の区間の長さは、速度の値からではなく、tick が届いた時刻の差から測る。
// Worker が tick を間引いて遅い速度を作るときや、重くて公称の速さで進めないときも、実際の間隔に追いつく。

import { CHUNK_SIDE } from "../protocol/messages.ts";
import { layout } from "../protocol/snapshot.ts";

/** 等速（1 倍）で 1 秒に進む tick の数。chizu-core の TICKS_PER_SECOND_AT_NORMAL_SPEED と同じ。 */
export const TICKS_PER_SECOND = 30;

/** これより長い区間になる tick の進みは、補間せずその場に置く（保存データの読み込みなど）。 */
const MAX_INTERPOLATED_MS = 500;
/** 1 tick あたりの間隔を、届いた時刻の差から均すときの、新しい値の重み。 */
const SMOOTHING = 0.2;
/** 測った間隔が、均した値のこの倍数より外れたら、速度が切り替わったとみなして測り直す。 */
const OUTLIER_RATIO = 3;
/** 前の tick からこれより空いたら、止まっていたとみなし、測った間隔を使わない。 */
const MAX_MEASURED_GAP_MS = 1000;
/** 向きの 1 周の値。chizu-wasm の向きの値（x 軸から y 軸へ回る角度）と同じ。 */
const FULL_TURN = 65536;
/** 1 tick の向きの変わりがこれより大きければ（折り返しなど）、回さずにその場で変える。1 周の 3/8（135 度）。 */
const MAX_TURN = (FULL_TURN * 3) / 8;

/**
 * 向き `from` から `to` へ、近いほうに `t` の割合だけ回した向き。変わりが MAX_TURN より大きければ、
 * `t` が 1 になるまで `from` のままにする。
 */
export function turnHeading(from: number, to: number, t: number): number {
  const diff = ((((to - from) % FULL_TURN) + FULL_TURN * 1.5) % FULL_TURN) - FULL_TURN / 2;
  if (Math.abs(diff) > MAX_TURN) {
    return t >= 1 ? to : from;
  }
  const h = Math.round(from + diff * t) % FULL_TURN;
  return h < 0 ? h + FULL_TURN : h;
}

/** 補間の入力になる、1 枠ぶんの移動体の語の読み方。 */
export type MoverFrame = {
  tick: number;
  speed: number;
  paused: boolean;
  baseCx: number;
  baseCy: number;
  count: number;
  /** 枠の語を 1 体 layout.moverWords 語で並べたもの。 */
  words: Uint32Array;
  /** words と同じバッファを f32 として見たもの。 */
  floats: Float32Array;
};

/**
 * 移動を区別する鍵。添え字は使い回されるので世代と組にし、車と歩く人も混ぜない。
 * 2^32 × 2^16 × 4 は 2^50 で、f64 に収まる。
 */
export function moverKey(index: number, generation: number, kind: number): number {
  return (index * 65536 + (generation & 0xffff)) * 4 + (kind & 3);
}

export class MoverInterpolator {
  #tick = -1;
  #count = 0;
  // 今の tick の位置
  #keys = new Float64Array(0);
  #x = new Float64Array(0);
  #y = new Float64Array(0);
  #h = new Float64Array(0);
  // 前の位置。今の並びと同じではなく、#prevIndex で鍵から引く
  #prevX = new Float64Array(0);
  #prevY = new Float64Array(0);
  #prevH = new Float64Array(0);
  #prevIndex = new Map<number, number>();
  // 今の並びに合わせた前の位置
  #fromX = new Float64Array(0);
  #fromY = new Float64Array(0);
  #fromH = new Float64Array(0);
  #aligned = false;
  // 今の tick に着いた時刻と、補間にかける時間（ミリ秒）
  #since = 0;
  #span = 0;
  // 前に tick が進んだ枠が届いた時刻と、1 tick あたりの間隔の均した値（ミリ秒）
  #lastArrival = Number.NaN;
  #msPerTick = 0;

  /** 補間の割合（0 で前の tick の位置、1 で今の tick の位置）。 */
  fraction(now: number, paused: boolean): number {
    if (paused || this.#span <= 0) {
      return 1;
    }
    return Math.min(1, Math.max(0, (now - this.#since) / this.#span));
  }

  /**
   * 補間した位置を `out`（f32 の並び、基準のチャンクからの相対）の x・y に書く。
   * 向きは `out` と同じバッファの語に書く。種類と ID は枠のままにするので、呼び出し側が先に `out` へ
   * 枠の語を写しておく。
   */
  apply(frame: MoverFrame, now: number, out: Float32Array): void {
    const n = frame.count;
    const stride = layout.moverWords;
    const ox = frame.baseCx * CHUNK_SIDE;
    const oy = frame.baseCy * CHUNK_SIDE;
    this.#grow(n);

    const keyAt = (i: number) =>
      moverKey(
        frame.words[i * stride + layout.mover.id] ?? 0,
        frame.words[i * stride + layout.mover.generation] ?? 0,
        (frame.words[i * stride + layout.mover.headingKind] ?? 0) >>> 16,
      );

    let sameOrder = this.#aligned && n === this.#count;
    if (sameOrder) {
      for (let i = 0; i < n; i++) {
        if (keyAt(i) !== this.#keys[i]) {
          sameOrder = false;
          break;
        }
      }
    }

    if (frame.tick !== this.#tick) {
      this.#advance(frame, now);
      sameOrder = false;
    }

    // 今の位置を覚える。並びが変わったときは、前の位置も今の並びに合わせ直す
    for (let i = 0; i < n; i++) {
      const at = i * stride;
      this.#keys[i] = keyAt(i);
      this.#x[i] = ox + (frame.floats[at + layout.mover.x] ?? 0);
      this.#y[i] = oy + (frame.floats[at + layout.mover.y] ?? 0);
      this.#h[i] = (frame.words[at + layout.mover.headingKind] ?? 0) & 0xffff;
    }
    this.#count = n;
    if (!sameOrder) {
      for (let i = 0; i < n; i++) {
        const j = this.#prevIndex.get(this.#keys[i] ?? 0);
        // 前の位置がない（新しく出た）移動体は、その場に描く
        this.#fromX[i] = j === undefined ? (this.#x[i] ?? 0) : (this.#prevX[j] ?? 0);
        this.#fromY[i] = j === undefined ? (this.#y[i] ?? 0) : (this.#prevY[j] ?? 0);
        this.#fromH[i] = j === undefined ? (this.#h[i] ?? 0) : (this.#prevH[j] ?? 0);
      }
      this.#aligned = true;
    }

    const t = this.fraction(now, frame.paused);
    const words = new Uint32Array(out.buffer, out.byteOffset, out.length);
    for (let i = 0; i < n; i++) {
      const at = i * stride;
      const h = this.#h[i] ?? 0;
      const word = words[at + layout.mover.headingKind] ?? 0;
      words[at + layout.mover.headingKind] =
        ((word & 0xffff0000) | turnHeading(this.#fromH[i] ?? h, h, t)) >>> 0;
      const x = this.#x[i] ?? 0;
      const y = this.#y[i] ?? 0;
      const fx = this.#fromX[i] ?? x;
      const fy = this.#fromY[i] ?? y;
      out[at + layout.mover.x] = fx + (x - fx) * t - ox;
      out[at + layout.mover.y] = fy + (y - fy) * t - oy;
    }
  }

  // tick が進んだ。いま見えている位置を前の位置にし、補間にかける時間を決める
  #advance(frame: MoverFrame, now: number): void {
    const delta = frame.tick - this.#tick;
    const nominal = 1000 / (TICKS_PER_SECOND * Math.max(frame.speed, 1e-6));
    const valid = this.#tick >= 0 && delta > 0;
    this.#updateInterval(valid ? delta : 0, now, nominal);
    this.#prevIndex.clear();
    const span = this.#msPerTick * delta;
    if (!valid || span > MAX_INTERPOLATED_MS || delta * nominal > MAX_INTERPOLATED_MS) {
      // 最初の枠、巻き戻し、長い間: 前の位置は持たず、その場に描く
      this.#span = 0;
      this.#since = now;
    } else {
      // 区間の途中で次の tick が届いても、いま見えている位置から動き出し、跳ばないようにする
      const t = this.fraction(now, false);
      for (let i = 0; i < this.#count; i++) {
        const x = this.#x[i] ?? 0;
        const y = this.#y[i] ?? 0;
        const fx = this.#fromX[i] ?? x;
        const fy = this.#fromY[i] ?? y;
        this.#prevX[i] = fx + (x - fx) * t;
        this.#prevY[i] = fy + (y - fy) * t;
        const h = this.#h[i] ?? 0;
        this.#prevH[i] = turnHeading(this.#fromH[i] ?? h, h, t);
        this.#prevIndex.set(this.#keys[i] ?? 0, i);
      }
      this.#span = span;
      this.#since = now;
    }
    this.#tick = frame.tick;
  }

  #updateInterval(delta: number, now: number, nominal: number): void {
    const gap = now - this.#lastArrival;
    this.#lastArrival = now;
    if (delta <= 0 || this.#msPerTick <= 0 || !(gap > 0) || gap > MAX_MEASURED_GAP_MS) {
      // 測れる前の到着がない、または止まっていたあと: 公称の間隔から始める
      this.#msPerTick = nominal;
      return;
    }
    const sample = gap / delta;
    if (sample > this.#msPerTick * OUTLIER_RATIO || sample < this.#msPerTick / OUTLIER_RATIO) {
      // 速度が切り替わった: 測った値で測り直す
      this.#msPerTick = sample;
      return;
    }
    this.#msPerTick += (sample - this.#msPerTick) * SMOOTHING;
  }

  #grow(n: number): void {
    if (this.#keys.length >= n) {
      return;
    }
    const size = Math.max(n, this.#keys.length * 2, 256);
    const widen = (old: Float64Array<ArrayBuffer>): Float64Array<ArrayBuffer> => {
      const next = new Float64Array(size);
      next.set(old);
      return next;
    };
    this.#keys = widen(this.#keys);
    this.#x = widen(this.#x);
    this.#y = widen(this.#y);
    this.#h = widen(this.#h);
    this.#prevX = widen(this.#prevX);
    this.#prevY = widen(this.#prevY);
    this.#prevH = widen(this.#prevH);
    this.#fromX = widen(this.#fromX);
    this.#fromY = widen(this.#fromY);
    this.#fromH = widen(this.#fromH);
    this.#aligned = false;
  }
}
