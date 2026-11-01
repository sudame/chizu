// 移動体の位置を、前の tick と今の tick のあいだで補間する。
// 前の位置は移動体の ID ごとに画面の側で覚える。三重バッファは今の tick の位置だけを持つ。
// 位置は基準のチャンクが変わっても比べられるよう、ワールド座標（マス）の f64 で持つ。

import { CHUNK_SIDE } from "../protocol/messages.ts";
import { layout } from "../protocol/snapshot.ts";

/** 等速（1 倍）で 1 秒に進む tick の数。chizu-core の TICKS_PER_SECOND_AT_NORMAL_SPEED と同じ。 */
export const TICKS_PER_SECOND = 30;

/** これより長く間が空いた tick は、補間せずその場に置く（保存データの読み込みや長い停止のあと）。 */
const MAX_INTERPOLATED_SECONDS = 0.5;

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

export class MoverInterpolator {
  #tick = -1;
  #count = 0;
  // 今の tick の位置。次に tick が進んだとき、これが前の位置になる
  #ids = new Uint32Array(0);
  #x = new Float64Array(0);
  #y = new Float64Array(0);
  // 前の tick の位置。#ids と同じ並びではなく、#prevIndex で ID から引く
  #prevX = new Float64Array(0);
  #prevY = new Float64Array(0);
  #prevIndex = new Map<number, number>();
  // 今の並びに合わせた前の位置
  #fromX = new Float64Array(0);
  #fromY = new Float64Array(0);
  #aligned = false;
  // 今の tick に着いた時刻と、次の tick が来るはずの間隔（ミリ秒）
  #since = 0;
  #span = 0;

  /** 補間の割合（0 で前の tick の位置、1 で今の tick の位置）。 */
  fraction(now: number, paused: boolean): number {
    if (paused || this.#span <= 0) {
      return 1;
    }
    return Math.min(1, Math.max(0, (now - this.#since) / this.#span));
  }

  /**
   * 補間した位置を `out`（f32 の並び、基準のチャンクからの相対）の x・y に書く。
   * 向きと種類と ID は枠のままにするので、呼び出し側が先に `out` へ枠の語を写しておく。
   */
  apply(frame: MoverFrame, now: number, out: Float32Array): void {
    const n = frame.count;
    const stride = layout.moverWords;
    const ox = frame.baseCx * CHUNK_SIDE;
    const oy = frame.baseCy * CHUNK_SIDE;
    this.#grow(n);

    let sameOrder = this.#aligned && n === this.#count;
    if (sameOrder) {
      for (let i = 0; i < n; i++) {
        if (frame.words[i * stride + layout.mover.id] !== this.#ids[i]) {
          sameOrder = false;
          break;
        }
      }
    }

    if (frame.tick !== this.#tick) {
      this.#advance(frame, now);
      sameOrder = false;
    }

    // 今の位置を覚える。ID の並びが変わったときは、前の位置も今の並びに合わせ直す
    for (let i = 0; i < n; i++) {
      const at = i * stride;
      this.#ids[i] = frame.words[at + layout.mover.id] ?? 0;
      this.#x[i] = ox + (frame.floats[at + layout.mover.x] ?? 0);
      this.#y[i] = oy + (frame.floats[at + layout.mover.y] ?? 0);
    }
    this.#count = n;
    if (!sameOrder) {
      for (let i = 0; i < n; i++) {
        const j = this.#prevIndex.get(this.#ids[i] ?? 0);
        // 前の位置がない（新しく出た）移動体は、その場に描く
        this.#fromX[i] = j === undefined ? (this.#x[i] ?? 0) : (this.#prevX[j] ?? 0);
        this.#fromY[i] = j === undefined ? (this.#y[i] ?? 0) : (this.#prevY[j] ?? 0);
      }
      this.#aligned = true;
    }

    const t = this.fraction(now, frame.paused);
    for (let i = 0; i < n; i++) {
      const at = i * stride;
      const x = this.#x[i] ?? 0;
      const y = this.#y[i] ?? 0;
      const fx = this.#fromX[i] ?? x;
      const fy = this.#fromY[i] ?? y;
      out[at + layout.mover.x] = fx + (x - fx) * t - ox;
      out[at + layout.mover.y] = fy + (y - fy) * t - oy;
    }
  }

  // tick が進んだ。今の位置を前の位置に回し、次の tick までの間隔を決める
  #advance(frame: MoverFrame, now: number): void {
    const delta = frame.tick - this.#tick;
    const rate = TICKS_PER_SECOND * frame.speed;
    const seconds = rate > 0 ? delta / rate : Infinity;
    this.#prevIndex.clear();
    if (this.#tick < 0 || delta <= 0 || seconds > MAX_INTERPOLATED_SECONDS) {
      // 最初の枠、巻き戻し、長い間: 前の位置は持たず、その場に描く
      this.#span = 0;
      this.#since = now;
    } else {
      [this.#prevX, this.#x] = [this.#x, this.#prevX];
      [this.#prevY, this.#y] = [this.#y, this.#prevY];
      for (let i = 0; i < this.#count; i++) {
        this.#prevIndex.set(this.#ids[i] ?? 0, i);
      }
      const span = seconds * 1000;
      // 前の区間が終わる頃に次の tick が届いたなら、区間をつなげて速さが揺れないようにする
      const expected = this.#since + this.#span;
      const late = now - expected;
      this.#since = this.#span > 0 && late >= 0 && late < span ? expected : now;
      this.#span = span;
    }
    this.#tick = frame.tick;
  }

  #grow(n: number): void {
    if (this.#ids.length >= n) {
      return;
    }
    const size = Math.max(n, this.#ids.length * 2, 256);
    const widen = <T extends Uint32Array | Float64Array>(old: T, make: (size: number) => T): T => {
      const next = make(size);
      next.set(old);
      return next;
    };
    this.#ids = widen(this.#ids, (s) => new Uint32Array(s));
    this.#x = widen(this.#x, (s) => new Float64Array(s));
    this.#y = widen(this.#y, (s) => new Float64Array(s));
    this.#prevX = widen(this.#prevX, (s) => new Float64Array(s));
    this.#prevY = widen(this.#prevY, (s) => new Float64Array(s));
    this.#fromX = new Float64Array(size);
    this.#fromY = new Float64Array(size);
    this.#aligned = false;
  }
}
