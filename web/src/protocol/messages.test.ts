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
    roadCosts: [250, 450, 750, 8000, 11314],
    serviceTable: [60000, 4, 4, 25000, 2, 2],
  };
  const roads = {
    kind: "roads",
    revision: 1,
    segments: new Float64Array([0, 0, 4, 0, 1]),
    nodes: new Float64Array([0, 0, 1, 4, 0, 1]),
  };
  const chunk = { cx: -1, cy: 2, version: 3, owned: true, tiles: new ArrayBuffer(CHUNK_AREA) };

  test("正しい形のメッセージを受け入れる", () => {
    expect(isWorkerToMain(ready)).toBe(true);
    expect(isWorkerToMain({ kind: "error", message: "失敗" })).toBe(true);
    expect(isWorkerToMain({ kind: "chunks", chunks: [chunk] })).toBe(true);
    expect(isWorkerToMain({ kind: "chunks", chunks: [] })).toBe(true);
    expect(isWorkerToMain(roads)).toBe(true);
    const probe = { kind: "service-probe", service: "WaterPump", x: 3, y: -4 };
    expect(isWorkerToMain({ ...probe, results: [null, "道路に面していない", null, null] })).toBe(
      true,
    );
    expect(isWorkerToMain({ ...probe, results: [null, null, null] })).toBe(false);
    expect(isWorkerToMain({ ...probe, results: [null, 1, null, null] })).toBe(false);
    const buildings = {
      kind: "buildings",
      buildings: new Float64Array(7),
      facilities: new Float64Array(5),
      stats: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12],
    };
    expect(isWorkerToMain({ ...buildings, facilities: new Float64Array(4) })).toBe(false);
    expect(isWorkerToMain(buildings)).toBe(true);
    expect(isWorkerToMain({ ...buildings, buildings: new Float64Array(5) })).toBe(false);
    expect(isWorkerToMain({ ...buildings, stats: [1, 2] })).toBe(false);
    expect(isWorkerToMain({ kind: "outcome", id: 3, seq: 1, outcome: "Denied", reason: "x" })).toBe(
      true,
    );
    expect(isWorkerToMain({ kind: "outcome", id: 4, outcome: "Rejected" })).toBe(true);
    const layer = { cx: 0, cy: -1, version: 5, values: new ArrayBuffer(CHUNK_AREA) };
    expect(isWorkerToMain({ kind: "layer-chunks", layer: 1, chunks: [layer] })).toBe(true);
    expect(
      isWorkerToMain({
        kind: "layer-chunks",
        layer: 1,
        chunks: [{ ...layer, values: new ArrayBuffer(8) }],
      }),
    ).toBe(false);
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
    expect(isWorkerToMain({ ...roads, segments: new Float64Array(4) })).toBe(false);
    expect(isWorkerToMain({ ...roads, nodes: [0, 0, 1] })).toBe(false);
    expect(isWorkerToMain({ kind: "outcome", outcome: "Applied" })).toBe(false);
    expect(isWorkerToMain({ kind: "outcome", id: 1, seq: "1", outcome: "Applied" })).toBe(false);
    expect(isWorkerToMain({ type: "wasm_bindgen_worker_ready" })).toBe(false);
  });
});
