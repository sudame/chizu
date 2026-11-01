// テストで書き手を模すためのもの。手順は crates/chizu-wasm/src/snapshot.rs の publish と同じにする。

import { layout, slotBase } from "./snapshot.ts";

export function allocateSnapshotBuffer(): SharedArrayBuffer {
  const buffer = new SharedArrayBuffer(layout.totalWords * 4);
  const control = new Int32Array(buffer, 0, layout.controlWords);
  Atomics.store(control, layout.control.latest, layout.initial.latest);
  Atomics.store(control, layout.control.version, layout.version);
  return buffer;
}

export class TestWriter {
  readonly #control: Int32Array;
  readonly #words: Uint32Array;
  #writing = layout.initial.writer;

  constructor(buffer: SharedArrayBuffer) {
    this.#control = new Int32Array(buffer, 0, layout.controlWords);
    this.#words = new Uint32Array(buffer, 0, layout.totalWords);
  }

  get writing(): number {
    return this.#writing;
  }

  /** 書き込み中の枠を `fill` で埋め、最新の枠として渡す。 */
  publish(fill: (words: Uint32Array, base: number) => void): void {
    fill(this.#words, slotBase(this.#writing));
    const previous = Atomics.exchange(
      this.#control,
      layout.control.latest,
      this.#writing | layout.fresh,
    );
    this.#writing = previous & layout.indexMask;
  }
}

/** 枠の先頭の数値と移動体の語のうち、確かめに使う範囲。 */
export const CHECKED_WORDS = layout.slotHeaderWords + 64 * layout.moverWords;

/** 確かめに使う範囲の語をすべて `value` にする。 */
export function fillAll(value: number): (words: Uint32Array, base: number) => void {
  return (words, base) => words.fill(value, base, base + CHECKED_WORDS);
}
