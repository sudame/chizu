// 移動体の位置を、前の tick と今の tick のあいだで補間する。
// 前の位置は移動体ごとに画面の側で覚える。三重バッファは今の tick の位置だけを持つ。
// 位置は基準のチャンクが変わっても比べられるよう、ワールド座標（マス）の f64 で持つ。
//
// 車は、Worker が描いた道筋（chizu-wasm の car_path::Paths）の番号と、道筋の上で進んだ割合を持つ。
// 前の tick と今の tick が同じ道筋の上か、つながった道筋の上にあれば、そのあいだを道筋に沿って、長さに
// 比例して動かす。交差点を曲がる車が、弦の上を通って角を削らないようにする。道筋の形は、網が変わった
// ときだけ Worker から届く（MoverPaths）。
//
// 道筋に沿えないときは、位置をまっすぐ移し、向きも前の tick の向きから今の tick の向きへ近いほうに
// 回して補間する。
//
// 補間の区間の長さは、速度の値からではなく、tick が届いた時刻の差から測る。
// 重くて公称の速さで進めないときも、実際の間隔に追いつく。

import { CHUNK_SIDE } from "../protocol/messages.ts";
import { layout } from "../protocol/snapshot.ts";

/** 等速（1 倍）で 1 秒に進む tick の数。chizu-core の TICKS_PER_SECOND_AT_NORMAL_SPEED と同じ。 */
export const TICKS_PER_SECOND = 5;

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
/** 1 tick の向きの変わりがこれより大きければ（折り返しなど）、回さずにすぐ変える。1 周の 3/8（135 度）。 */
const MAX_TURN = (FULL_TURN * 3) / 8;
/** 道筋の 1 点の数の個数（x、y、始まりからの長さ、向き）。chizu-wasm の car_path::Path::encode と同じ。 */
const POINT_STRIDE = 4;
/** まっすぐ補間した移動体を道筋に乗せ直すのは、補間がこの割合まで進んでいるときに限る（位置が跳ばないため）。 */
const RELATCH_FRACTION = 0.9;
/** 前の道筋の終わりと次の道筋の始まりがこれより近ければ（マス）、つながっているとみなす。40mm。 */
const JOIN_TILES = 0.005;

/**
 * 向き `from` から `to` へ、近いほうに `t` の割合だけ回した向き。変わりが MAX_TURN より大きければ
 * 回さず、すぐに `to` にする。
 */
export function turnHeading(from: number, to: number, t: number): number {
  const diff = ((((to - from) % FULL_TURN) + FULL_TURN * 1.5) % FULL_TURN) - FULL_TURN / 2;
  if (Math.abs(diff) > MAX_TURN) {
    return to;
  }
  const h = Math.round(from + diff * t) % FULL_TURN;
  return h < 0 ? h + FULL_TURN : h;
}

/** 角度（ラジアン）を、1 周を 65536 とした向きの値に直す。chizu-wasm の car_path::angle_code と同じ。 */
export function angleCode(angle: number): number {
  const code = Math.round((angle / (2 * Math.PI)) * FULL_TURN) % FULL_TURN;
  return code < 0 ? code + FULL_TURN : code;
}

/** 道筋の点の並び。1 点ごとに x、y（マス）、始まりからの長さ（マス）、向き（ラジアン）。 */
export type MoverPath = Float64Array;

/** 道筋の長さ（マス）。 */
export function pathLength(path: MoverPath): number {
  return path[path.length - 2] ?? 0;
}

/** 道筋の上の位置と向き（ラジアン）。 */
export type PathPoint = { x: number; y: number; angle: number };

/** 道筋の上で、始まりから `s` だけ進んだ所の位置と向き（ラジアン）を `out` に書いて返す。 */
export function pathAt(
  path: MoverPath,
  s: number,
  out: PathPoint = { x: 0, y: 0, angle: 0 },
): PathPoint {
  const n = Math.floor(path.length / POINT_STRIDE);
  const p = (k: number, i: number) => path[k * POINT_STRIDE + i] ?? 0;
  // s より先にある最初の点を探す
  let lo = 0;
  let hi = n;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (p(mid, 2) <= s) {
      lo = mid + 1;
    } else {
      hi = mid;
    }
  }
  if (lo === 0 || lo >= n) {
    const k = lo === 0 ? 0 : n - 1;
    out.x = p(k, 0);
    out.y = p(k, 1);
    out.angle = p(k, 3);
    return out;
  }
  const a = lo - 1;
  const t = (s - p(a, 2)) / (p(lo, 2) - p(a, 2));
  const turn = ((((p(lo, 3) - p(a, 3)) % (2 * Math.PI)) + 3 * Math.PI) % (2 * Math.PI)) - Math.PI;
  out.x = p(a, 0) + (p(lo, 0) - p(a, 0)) * t;
  out.y = p(a, 1) + (p(lo, 1) - p(a, 1)) * t;
  out.angle = p(a, 3) + turn * t;
  return out;
}

/** 道筋の上で、(x, y) にいちばん近い点の始まりからの長さ `s`。その点の位置と向きを `out` に書く。 */
function nearestOnPath(path: MoverPath, x: number, y: number, out: PathPoint): { s: number } {
  const n = Math.floor(path.length / POINT_STRIDE);
  const p = (k: number, i: number) => path[k * POINT_STRIDE + i] ?? 0;
  let best = Number.POSITIVE_INFINITY;
  let bestS = 0;
  for (let k = 0; k + 1 < n; k++) {
    const ax = p(k, 0);
    const ay = p(k, 1);
    const dx = p(k + 1, 0) - ax;
    const dy = p(k + 1, 1) - ay;
    const len2 = dx * dx + dy * dy;
    const u = len2 === 0 ? 0 : Math.min(1, Math.max(0, ((x - ax) * dx + (y - ay) * dy) / len2));
    const d = Math.hypot(ax + dx * u - x, ay + dy * u - y);
    if (d < best) {
      best = d;
      bestS = p(k, 2) + (p(k + 1, 2) - p(k, 2)) * u;
    }
  }
  pathAt(path, bestS, out);
  return { s: bestS };
}

/** 道筋 `a` の終わりと道筋 `b` の始まりが同じ所にあるか。 */
function joins(a: MoverPath, b: MoverPath): boolean {
  const n = a.length;
  const dx = (a[n - 4] ?? 0) - (b[0] ?? 0);
  const dy = (a[n - 3] ?? 0) - (b[1] ?? 0);
  return Math.hypot(dx, dy) < JOIN_TILES;
}

/**
 * take_mover_paths の並びに、画面へ送る中身があるか。忘れる道筋か新しい道筋が 1 つでもあれば送る。
 * 新しい道筋の数は、忘れる番号の並びのあと（`2 + 忘れる数`）にある。
 */
export function hasMoverPaths(data: Float64Array): boolean {
  if (data[0] === 1) {
    return true;
  }
  const dropped = data[1] ?? 0;
  return dropped > 0 || (data[2 + dropped] ?? 0) > 0;
}

/** Worker から届いた、車を描く道筋。番号は網が変わっても使い回さない。 */
export class MoverPaths {
  readonly #paths = new Map<number, MoverPath>();

  /**
   * chizu-wasm の take_mover_paths の並びを受け取る。先頭は前の道筋をすべて忘れるか（1 なら忘れる）、
   * 忘れる道筋の数とその番号、新しい道筋の数で、新しい道筋ごとに、番号、点の数、点の並びが続く。
   */
  receive(data: Float64Array): void {
    if (data[0] === 1) {
      this.#paths.clear();
    }
    const dropped = data[1] ?? 0;
    for (let k = 0; k < dropped; k++) {
      this.#paths.delete(data[2 + k] ?? 0);
    }
    const count = data[2 + dropped] ?? 0;
    let at = 3 + dropped;
    for (let k = 0; k < count && at + 2 <= data.length; k++) {
      const id = data[at] ?? 0;
      const n = data[at + 1] ?? 0;
      const end = at + 2 + n * POINT_STRIDE;
      if (end > data.length) {
        break;
      }
      this.#paths.set(id, data.slice(at + 2, end));
      at = end;
    }
  }

  get(id: number): MoverPath | undefined {
    return id === 0 ? undefined : this.#paths.get(id);
  }

  get size(): number {
    return this.#paths.size;
  }
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

/** 見えている位置。道筋の上にいれば、その番号と長さも持つ（番号 0 なら道筋の上にいない）。 */
type Seen = { x: number; y: number; h: number; path: number; s: number };

const NO_PATHS = new MoverPaths();

export class MoverInterpolator {
  #tick = -1;
  #count = 0;
  // 今の tick の位置と、道筋の番号、前の tick の道筋とのあいだに通り抜けた道筋、道筋の上で進んだ割合
  #keys = new Float64Array(0);
  #x = new Float64Array(0);
  #y = new Float64Array(0);
  #h = new Float64Array(0);
  #path = new Float64Array(0);
  #via = new Float64Array(0);
  #frac = new Float64Array(0);
  // 前に見えていた位置。今の並びと同じではなく、#prevIndex で鍵から引く
  #prevX = new Float64Array(0);
  #prevY = new Float64Array(0);
  #prevH = new Float64Array(0);
  #prevPath = new Float64Array(0);
  #prevS = new Float64Array(0);
  #prevIndex = new Map<number, number>();
  // 今の並びに合わせた前の位置
  #fromX = new Float64Array(0);
  #fromY = new Float64Array(0);
  #fromH = new Float64Array(0);
  #fromPath = new Float64Array(0);
  #fromS = new Float64Array(0);
  #aligned = false;
  // 今の tick に着いた時刻と、補間にかける時間（ミリ秒）
  #since = 0;
  #span = 0;
  // 前に tick が進んだ枠が届いた時刻と、1 tick あたりの間隔の均した値（ミリ秒）
  #lastArrival = Number.NaN;
  #msPerTick = 0;
  #paths: MoverPaths = NO_PATHS;
  readonly #seen: Seen = { x: 0, y: 0, h: 0, path: 0, s: 0 };
  /** これまでに apply した枠のすべてで、届いている道筋を指していた移動体の延べ数。test:wasm が読む。 */
  routed = 0;
  /** そのうち、道筋に沿って補間した移動体の延べ数。 */
  along = 0;

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
   * 枠の語を写しておく。`paths` は Worker から届いた道筋で、車を道筋に沿って動かすのに使う。
   */
  apply(frame: MoverFrame, now: number, out: Float32Array, paths: MoverPaths = NO_PATHS): void {
    const n = frame.count;
    const stride = layout.moverWords;
    const ox = frame.baseCx * CHUNK_SIDE;
    const oy = frame.baseCy * CHUNK_SIDE;
    this.#grow(n);
    this.#paths = paths;

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
      this.#path[i] = frame.words[at + layout.mover.path] ?? 0;
      this.#via[i] = frame.words[at + layout.mover.via] ?? 0;
      this.#frac[i] = frame.floats[at + layout.mover.frac] ?? 0;
    }
    this.#count = n;
    if (!sameOrder) {
      for (let i = 0; i < n; i++) {
        const j = this.#prevIndex.get(this.#keys[i] ?? 0);
        if (j === undefined) {
          // 前の位置がない（新しく出た）移動体は、その場に描く
          this.#fromX[i] = this.#x[i] ?? 0;
          this.#fromY[i] = this.#y[i] ?? 0;
          this.#fromH[i] = this.#h[i] ?? 0;
          const path = this.#path[i] ?? 0;
          const p = paths.get(path);
          this.#fromPath[i] = p ? path : 0;
          this.#fromS[i] = p ? (this.#frac[i] ?? 0) * pathLength(p) : 0;
        } else {
          this.#fromX[i] = this.#prevX[j] ?? 0;
          this.#fromY[i] = this.#prevY[j] ?? 0;
          this.#fromH[i] = this.#prevH[j] ?? 0;
          this.#fromPath[i] = this.#prevPath[j] ?? 0;
          this.#fromS[i] = this.#prevS[j] ?? 0;
        }
      }
      this.#aligned = true;
    }

    const t = this.fraction(now, frame.paused);
    const words = new Uint32Array(out.buffer, out.byteOffset, out.length);
    const seen = this.#seen;
    let routed = 0;
    let along = 0;
    for (let i = 0; i < n; i++) {
      const at = i * stride;
      this.#visible(i, t, seen);
      if (paths.get(this.#path[i] ?? 0) !== undefined) {
        routed++;
        if (seen.path !== 0) {
          along++;
        }
      }
      const word = words[at + layout.mover.headingKind] ?? 0;
      words[at + layout.mover.headingKind] = ((word & 0xffff0000) | seen.h) >>> 0;
      out[at + layout.mover.x] = seen.x - ox;
      out[at + layout.mover.y] = seen.y - oy;
    }
    this.routed += routed;
    this.along += along;
  }

  /**
   * 移動体 `i` の、補間の割合 `t` の所に見える位置を `seen` に書く。前の tick と今の tick が同じ道筋か、
   * つながった道筋（あいだに通り抜けた道筋があればそれも）の上にあれば、道筋に沿って動かす。
   * そうでなければ、まっすぐ動かす。
   */
  #visible(i: number, t: number, seen: Seen): void {
    if (this.#along(i, t, seen)) {
      return;
    }
    const x = this.#x[i] ?? 0;
    const y = this.#y[i] ?? 0;
    const h = this.#h[i] ?? 0;
    const fx = this.#fromX[i] ?? x;
    const fy = this.#fromY[i] ?? y;
    seen.x = fx + (x - fx) * t;
    seen.y = fy + (y - fy) * t;
    seen.h = turnHeading(this.#fromH[i] ?? h, h, t);
    seen.path = 0;
    seen.s = 0;
  }

  #along(i: number, t: number, seen: Seen): boolean {
    const paths = this.#paths;
    const id = this.#path[i] ?? 0;
    const to = paths.get(id);
    const fromId = this.#fromPath[i] ?? 0;
    const from = paths.get(fromId);
    if (to === undefined || from === undefined) {
      return false;
    }
    const s1 = (this.#frac[i] ?? 0) * pathLength(to);
    const s0 = this.#fromS[i] ?? 0;
    if (fromId === id) {
      this.#put(seen, to, id, s0 + (s1 - s0) * t);
      return true;
    }
    // 前の道筋の s0 から終わりまで、通り抜けた道筋の全部、今の道筋の始まりから s1 までを順にたどる
    const viaId = this.#via[i] ?? 0;
    let via = paths.get(viaId);
    if (via !== undefined && !(joins(from, via) && joins(via, to))) {
      via = undefined;
    }
    if (via === undefined && !joins(from, to)) {
      return false;
    }
    const first = pathLength(from) - s0;
    const middle = via === undefined ? 0 : pathLength(via);
    let left = (first + middle + s1) * t;
    if (left <= first) {
      this.#put(seen, from, fromId, s0 + left);
      return true;
    }
    left -= first;
    if (via !== undefined && left <= middle) {
      this.#put(seen, via, viaId, left);
      return true;
    }
    this.#put(seen, to, id, Math.min(left - middle, s1));
    return true;
  }

  readonly #point: PathPoint = { x: 0, y: 0, angle: 0 };

  #put(seen: Seen, path: MoverPath, id: number, s: number): void {
    const p = pathAt(path, s, this.#point);
    seen.x = p.x;
    seen.y = p.y;
    seen.h = angleCode(p.angle);
    seen.path = id;
    seen.s = s;
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
      const seen = this.#seen;
      for (let i = 0; i < this.#count; i++) {
        this.#visible(i, t, seen);
        this.#prevX[i] = seen.x;
        this.#prevY[i] = seen.y;
        this.#prevH[i] = seen.h;
        this.#prevPath[i] = seen.path;
        this.#prevS[i] = seen.s;
        if (seen.path === 0 && t >= RELATCH_FRACTION) {
          // まっすぐ補間した枠でも、tick の終わりには今の道筋に乗せる。
          // 乗せないと、道筋のない枠から始まった車は、次も道筋から始まれない
          const id = this.#path[i] ?? 0;
          const p = this.#paths.get(id);
          if (p !== undefined) {
            // 見えている位置に最も近い道筋の点から始める。tick の終わりの位置から始めると、次の枠で前へ跳ぶ
            const at = nearestOnPath(p, seen.x, seen.y, this.#point);
            this.#prevPath[i] = id;
            this.#prevS[i] = at.s;
            this.#prevH[i] = angleCode(this.#point.angle);
          }
        }
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
    this.#path = widen(this.#path);
    this.#via = widen(this.#via);
    this.#frac = widen(this.#frac);
    this.#prevX = widen(this.#prevX);
    this.#prevY = widen(this.#prevY);
    this.#prevH = widen(this.#prevH);
    this.#prevPath = widen(this.#prevPath);
    this.#prevS = widen(this.#prevS);
    this.#fromX = widen(this.#fromX);
    this.#fromY = widen(this.#fromY);
    this.#fromH = widen(this.#fromH);
    this.#fromPath = widen(this.#fromPath);
    this.#fromS = widen(this.#fromS);
    this.#aligned = false;
  }
}
