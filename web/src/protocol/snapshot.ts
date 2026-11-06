// 描画用のスナップショットの三重バッファを、読み手（メインスレッド）の側から読む。
// 入れ替えの規約と書き手の側は crates/chizu-wasm/src/snapshot.rs にある。
// 配置の定数は snapshot-layout.json にあり、Rust 側のテストがこのファイルと突き合わせる。

import layout from "./snapshot-layout.json" with { type: "json" };

export { layout };

export type SnapshotHeader = {
  tick: number;
  day: number;
  secondOfDay: number;
  money: number;
  population: number;
  speed: number;
  paused: boolean;
  moverCount: number;
  /** 移動体の座標の基準にするチャンク。移動体の x・y はこのチャンクの原点からのマスの数である。 */
  baseCx: number;
  baseCy: number;
  /** 列車の車両の数。 */
  trainCarCount: number;
  /** 車両の座標の基準にするチャンク。車両の x・y はこのチャンクの原点からのマスの数である。 */
  trainBaseCx: number;
  trainBaseCy: number;
};

/** 枠 `slot` の先頭の語の、バッファの中での語の位置。 */
export function slotBase(slot: number): number {
  return layout.controlWords + slot * layout.slotWords;
}

export class SnapshotReader {
  readonly #control: Int32Array;
  readonly #words: Uint32Array;
  readonly #signed: Int32Array;
  #reading = layout.initial.reader;
  #received = false;

  constructor(buffer: SharedArrayBuffer, byteOffset: number) {
    this.#control = new Int32Array(buffer, byteOffset, layout.controlWords);
    this.#words = new Uint32Array(buffer, byteOffset, layout.totalWords);
    this.#signed = new Int32Array(buffer, byteOffset, layout.totalWords);
    const version = Atomics.load(this.#control, layout.control.version);
    if (version !== layout.version) {
      throw new Error(`三重バッファの配置の版が違う（WASM ${version}、画面 ${layout.version}）`);
    }
  }

  /** いま読んでいる枠の番号。 */
  get reading(): number {
    return this.#reading;
  }

  /**
   * 新しく書かれた枠があれば受け取り、読んでいる枠の先頭の数値を返す。
   * まだ一度も書かれていなければ null を返す。
   */
  acquire(): SnapshotHeader | null {
    const latest = layout.control.latest;
    if ((Atomics.load(this.#control, latest) & layout.fresh) !== 0) {
      const previous = Atomics.exchange(this.#control, latest, this.#reading);
      this.#reading = previous & layout.indexMask;
      this.#received = true;
    }
    if (!this.#received) {
      return null;
    }
    const base = slotBase(this.#reading);
    const w = (i: number) => this.#words[base + i] ?? 0;
    const h = layout.header;
    return {
      tick: w(h.tickLo) + w(h.tickHi) * 2 ** 32,
      day: w(h.day),
      secondOfDay: w(h.secondOfDay),
      money: w(h.moneyLo) + (this.#signed[base + h.moneyHi] ?? 0) * 2 ** 32,
      population: w(h.population),
      speed: w(h.speed),
      paused: w(h.paused) !== 0,
      moverCount: w(h.moverCount),
      baseCx: this.#signed[base + h.baseCx] ?? 0,
      baseCy: this.#signed[base + h.baseCy] ?? 0,
      trainCarCount: w(h.trainCarCount),
      trainBaseCx: this.#signed[base + h.trainBaseCx] ?? 0,
      trainBaseCy: this.#signed[base + h.trainBaseCy] ?? 0,
    };
  }

  /**
   * 読んでいる枠の移動体の語を、1 体 moverWords 語で並べたまま返す。中身は次に acquire するまで変わらない。
   * count は acquire が返した moverCount にする。
   */
  movers(count: number): Uint32Array {
    const start = slotBase(this.#reading) + layout.slotHeaderWords;
    const n = Math.min(count, layout.maxMovers);
    return this.#words.subarray(start, start + n * layout.moverWords);
  }

  /**
   * 読んでいる枠の列車の車両の語を、1 両 moverWords 語（移動体と同じ並び）で並べたまま返す。中身は次に acquire するまで変わらない。
   * count は acquire が返した trainCarCount にする。
   */
  cars(count: number): Uint32Array {
    const start = slotBase(this.#reading) + layout.carsStart;
    const n = Math.min(count, layout.maxTrainCars);
    return this.#words.subarray(start, start + n * layout.moverWords);
  }
}
