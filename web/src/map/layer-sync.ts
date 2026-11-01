// Worker の側の、データレイヤーの値の送り方。画面に出しているレイヤーだけを、版が変わったチャンクだけ送る。
// チャンクのマスと同じく ChunkSync で送った版を覚える。版はチャンクによらず、レイヤーごとに 1 つである。

import type {
  ChunkCoordMessage,
  LayerChunkData,
  LayerCode,
  WorkerToMain,
} from "../protocol/messages.ts";
import { ChunkSync } from "./chunk-sync.ts";

/** Worker の側で使う、chizu-wasm の Simulation のうちデータレイヤーにかかわる部分。 */
export type LayerSource = {
  layer_version(code: LayerCode): number;
  layer_chunk(code: LayerCode, cx: number, cy: number): Uint8Array;
};

// 版を確かめ直す間隔（実時間のミリ秒）。高速で回すと建物の表が頻繁に変わるので、値の面を作り直す回数を抑える
export const LAYER_CHECK_MS = 500;

export class LayerSender {
  #view: ChunkCoordMessage[] = [];
  readonly #layers = new Map<LayerCode, { sync: ChunkSync; version: number; checkedAt: number }>();

  /** 出すレイヤーを置き換える。どのレイヤーも、見えているチャンクをはじめから送り直す。 */
  setActive(active: LayerCode[]): void {
    this.#layers.clear();
    for (const code of active) {
      const sync = new ChunkSync();
      sync.setView(this.#view, []);
      this.#layers.set(code, { sync, version: Number.NaN, checkedAt: -Infinity });
    }
  }

  setView(visible: ChunkCoordMessage[], forgotten: ChunkCoordMessage[]): void {
    this.#view = visible;
    for (const { sync } of this.#layers.values()) {
      sync.setView(visible, forgotten);
    }
  }

  /** 版の変わったチャンクを、レイヤーごとに最大 limit 個送る。now を渡さなければ版をすぐ確かめ直す。 */
  send(
    source: LayerSource,
    reply: (message: WorkerToMain, transfer: Transferable[]) => void,
    limit: number,
    now = Number.POSITIVE_INFINITY,
  ): void {
    for (const [code, layer] of this.#layers) {
      if (now - layer.checkedAt >= LAYER_CHECK_MS) {
        layer.version = source.layer_version(code);
        layer.checkedAt = Number.isFinite(now) ? now : layer.checkedAt;
      }
      const picked = layer.sync.collect(() => layer.version, limit);
      if (picked.length === 0) {
        continue;
      }
      const chunks: LayerChunkData[] = picked.map(({ cx, cy, version }) => ({
        cx,
        cy,
        version,
        values: source.layer_chunk(code, cx, cy).buffer as ArrayBuffer,
      }));
      reply(
        { kind: "layer-chunks", layer: code, chunks },
        chunks.map((c) => c.values),
      );
    }
  }
}
