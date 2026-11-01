// チャンクの版番号による差分の受け渡し。
// Worker の側の ChunkSync は、画面の手元にある版を覚えておき、版の違うチャンクだけを選ぶ。
// 画面の側の ChunkStore は、届いたチャンクを持ち、手元の版を Worker に伝える。

import type { ChunkData, ChunkKnown } from "../protocol/messages.ts";

export const chunkKey = (cx: number, cy: number) => `${cx},${cy}`;

export type ChunkVersion = { cx: number; cy: number; version: number };

export class ChunkSync {
  #view: ChunkKnown[] = [];

  /** 画面から届いた「見えているチャンクと手元の版」に置き換える。 */
  setView(chunks: ChunkKnown[]): void {
    this.#view = chunks.map(({ cx, cy, version }) => ({ cx, cy, version }));
  }

  /**
   * 見えているチャンクのうち、手元と版の違うものを並びの順に最大 limit 個選び、送ったものとして覚える。
   * 選ばれなかったものは次に呼んだときに選ぶ。
   */
  collect(versionOf: (cx: number, cy: number) => number, limit: number): ChunkVersion[] {
    const out: ChunkVersion[] = [];
    for (const known of this.#view) {
      if (out.length >= limit) {
        break;
      }
      const version = versionOf(known.cx, known.cy);
      if (known.version !== version) {
        known.version = version;
        out.push({ cx: known.cx, cy: known.cy, version });
      }
    }
    return out;
  }
}

export type StoredChunk = {
  cx: number;
  cy: number;
  version: number;
  owned: boolean;
  tiles: Uint8Array;
};

export class ChunkStore {
  readonly #chunks = new Map<string, StoredChunk>();

  get size(): number {
    return this.#chunks.size;
  }

  get(cx: number, cy: number): StoredChunk | undefined {
    return this.#chunks.get(chunkKey(cx, cy));
  }

  all(): IterableIterator<StoredChunk> {
    return this.#chunks.values();
  }

  /** 届いたチャンクで手元を書き換え、書き換えたものを返す。手元と同じ版のものは無視する。 */
  receive(chunks: ChunkData[]): StoredChunk[] {
    const updated: StoredChunk[] = [];
    for (const { cx, cy, version, owned, tiles } of chunks) {
      if (this.get(cx, cy)?.version === version) {
        continue;
      }
      const stored = { cx, cy, version, owned, tiles: new Uint8Array(tiles) };
      this.#chunks.set(chunkKey(cx, cy), stored);
      updated.push(stored);
    }
    return updated;
  }

  /** Worker に伝える、チャンクごとの手元の版。並びは渡した順のまま。 */
  known(coords: [number, number][]): ChunkKnown[] {
    return coords.map(([cx, cy]) => ({ cx, cy, version: this.get(cx, cy)?.version ?? null }));
  }

  /** 数が max を超えていたら、keep に当たらないものを超えた分だけ捨て、捨てたものを返す。 */
  evict(keep: (cx: number, cy: number) => boolean, max: number): StoredChunk[] {
    const removed: StoredChunk[] = [];
    for (const [key, chunk] of this.#chunks) {
      if (this.#chunks.size <= max) {
        break;
      }
      if (!keep(chunk.cx, chunk.cy)) {
        this.#chunks.delete(key);
        removed.push(chunk);
      }
    }
    return removed;
  }
}
