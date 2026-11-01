import { describe, expect, test } from "vitest";
import { CHUNK_AREA, isWorkerToMain } from "./messages.ts";

describe("isWorkerToMain", () => {
  const ready = {
    kind: "ready",
    memory: new SharedArrayBuffer(8),
    snapshotOffset: 0,
    threads: 4,
    crossOriginIsolated: true,
    n: 3,
    sum: 6n,
  };
  const chunk = { cx: -1, cy: 2, version: 3, owned: true, tiles: new ArrayBuffer(CHUNK_AREA) };

  test("正しい形のメッセージを受け入れる", () => {
    expect(isWorkerToMain(ready)).toBe(true);
    expect(isWorkerToMain({ kind: "error", message: "失敗" })).toBe(true);
    expect(isWorkerToMain({ kind: "chunks", chunks: [chunk] })).toBe(true);
    expect(isWorkerToMain({ kind: "chunks", chunks: [] })).toBe(true);
  });

  test("形の違うメッセージを捨てる", () => {
    expect(isWorkerToMain(null)).toBe(false);
    expect(isWorkerToMain({ ...ready, memory: new ArrayBuffer(8) })).toBe(false);
    expect(isWorkerToMain({ ...ready, sum: 6 })).toBe(false);
    expect(isWorkerToMain({ kind: "unknown" })).toBe(false);
    expect(
      isWorkerToMain({ kind: "chunks", chunks: [{ ...chunk, tiles: new ArrayBuffer(8) }] }),
    ).toBe(false);
    expect(isWorkerToMain({ kind: "chunks", chunks: [{ ...chunk, cx: 0.5 }] })).toBe(false);
    expect(isWorkerToMain({ type: "wasm_bindgen_worker_ready" })).toBe(false);
  });
});
