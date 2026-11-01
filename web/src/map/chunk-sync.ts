// チャンクの版番号による差分の受け渡し。
// Worker の側の ChunkSync は、画面に送った版を正として覚えておき、版の違うチャンクだけを選ぶ。
// 画面の側の ChunkStore は、届いたチャンクを持ち、捨てたチャンクを Worker に伝える。

import type { ChunkCoordMessage, ChunkData } from "../protocol/messages.ts";

export const chunkKey = (cx: number, cy: number) => `${cx},${cy}`;

export type ChunkVersion = { cx: number; cy: number; version: number };

export class ChunkSync {
  #view: ChunkCoordMessage[] = [];
  // 画面に送った版。画面が捨てたと伝えてくるまで、画面の手元（届く途中を含む）にあるとみなす
  readonly #sent = new Map<string, number>();

  /**
   * 見えているチャンク（先に送ってほしい順）に置き換え、画面が捨てたチャンクを忘れる。
   * 送った版の記録は、ここでは下げない。
   */
  setView(visible: ChunkCoordMessage[], forgotten: ChunkCoordMessage[]): void {
    for (const { cx, cy } of forgotten) {
      this.#sent.delete(chunkKey(cx, cy));
    }
    this.#view = visible.map(({ cx, cy }) => ({ cx, cy }));
  }

  /**
   * 見えているチャンクのうち、送った版と中核の版が違うものを並びの順に最大 limit 個選び、
   * 送ったものとして覚える。選ばれなかったものは次に呼んだときに選ぶ。
   */
  collect(versionOf: (cx: number, cy: number) => number, limit: number): ChunkVersion[] {
    const out: ChunkVersion[] = [];
    for (const { cx, cy } of this.#view) {
      if (out.length >= limit) {
        break;
      }
      const key = chunkKey(cx, cy);
      const version = versionOf(cx, cy);
      if (this.#sent.get(key) !== version) {
        this.#sent.set(key, version);
        out.push({ cx, cy, version });
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

/** 届いたチャンクの手元の置き場。捨てるときは、最後に使ったのが古いものから捨てる。 */
export class ChunkStore {
  // Map の反復は入れた順なので、使うたびに入れ直して、先頭を最も古く使ったものにする
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

  /** 届いたチャンクで手元を書き換え、書き換えたものを返す。手元と同じ版のものは使ったことだけ記録する。 */
  receive(chunks: ChunkData[]): StoredChunk[] {
    const updated: StoredChunk[] = [];
    for (const { cx, cy, version, owned, tiles } of chunks) {
      const key = chunkKey(cx, cy);
      const current = this.#chunks.get(key);
      this.#chunks.delete(key);
      if (current?.version === version) {
        this.#chunks.set(key, current);
        continue;
      }
      const stored = { cx, cy, version, owned, tiles: new Uint8Array(tiles) };
      this.#chunks.set(key, stored);
      updated.push(stored);
    }
    return updated;
  }

  /** 見えているチャンクを、いま使ったものとして記録する。 */
  touch(coords: [number, number][]): void {
    for (const [cx, cy] of coords) {
      const key = chunkKey(cx, cy);
      const chunk = this.#chunks.get(key);
      if (chunk !== undefined) {
        this.#chunks.delete(key);
        this.#chunks.set(key, chunk);
      }
    }
  }

  /** 数が max を超えていたら、keep に当たらないものを古く使った順に超えた分だけ捨て、捨てたものを返す。 */
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
