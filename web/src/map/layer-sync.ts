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

/** Worker の側で使う、chizu-wasm の Simulation のうち交通の区間ごとの値にかかわる部分。 */
export type TrafficSource = {
  layer_version(code: LayerCode): number;
  network_version(): number;
  traffic_segments(): Uint8Array;
};

/** 交通のレイヤーの番号。chizu-wasm の layers::LAYERS の 3 番めである。 */
const TRAFFIC_CODE: LayerCode = 3;

/**
 * 交通の値の送り方。交通の主題図を出しているあいだ、区間ごとの値を、版が変わったときだけ送る。
 * 版は LAYER_CHECK_MS ごとに確かめる。道路網の版も一緒に送り、画面が区間の並びと突き合わせられるようにする。
 */
export class TrafficSender {
  #active = false;
  #sent = Number.NaN;
  #checkedAt = Number.NEGATIVE_INFINITY;

  /** 出すデータレイヤーが変わった。交通が入っていれば、次に値を送り直す。 */
  setActive(active: LayerCode[]): void {
    this.#active = active.includes(TRAFFIC_CODE);
    this.#sent = Number.NaN;
    this.#checkedAt = Number.NEGATIVE_INFINITY;
  }

  /** now を渡さなければ、版をすぐ確かめ直す。 */
  send(
    source: TrafficSource,
    reply: (message: WorkerToMain, transfer: Transferable[]) => void,
    now = Number.POSITIVE_INFINITY,
  ): void {
    if (!this.#active || now - this.#checkedAt < LAYER_CHECK_MS) {
      return;
    }
    this.#checkedAt = Number.isFinite(now) ? now : this.#checkedAt;
    const version = source.layer_version(TRAFFIC_CODE);
    if (version === this.#sent) {
      return;
    }
    this.#sent = version;
    const values = source.traffic_segments();
    reply(
      {
        kind: "traffic",
        revision: source.network_version(),
        values: values.buffer as ArrayBuffer,
      },
      [values.buffer as ArrayBuffer],
    );
  }
}

// 版を確かめ直す間隔（実時間のミリ秒）。高速で回すと建物の表が頻繁に変わるので、値の面を作り直す回数を抑える
export const LAYER_CHECK_MS = 500;

export class LayerSender {
  #view: ChunkCoordMessage[] = [];
  readonly #layers = new Map<LayerCode, { sync: ChunkSync; version: number; checkedAt: number }>();

  /** 出すレイヤーを置き換える。どのレイヤーも、見えているチャンクをはじめから送り直す。 */
  setActive(active: LayerCode[]): void {
    this.#layers.clear();
    // 交通は区間ごとの値で送る（TrafficSender）ので、マスの値は送らない
    for (const code of active.filter((c) => c !== TRAFFIC_CODE)) {
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
