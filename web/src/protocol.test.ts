import { describe, expect, test } from "vitest";
import { describeMessage, isWorkerToMain } from "./protocol.ts";

describe("isWorkerToMain", () => {
  test("正しい形のメッセージを受け入れる", () => {
    expect(
      isWorkerToMain({ kind: "sum", n: 3, sum: 6n, threads: 4, crossOriginIsolated: true }),
    ).toBe(true);
    expect(isWorkerToMain({ kind: "error", message: "失敗" })).toBe(true);
  });

  test("形の違うメッセージを捨てる", () => {
    expect(isWorkerToMain(null)).toBe(false);
    expect(
      isWorkerToMain({ kind: "sum", n: 3, sum: 6, threads: 4, crossOriginIsolated: true }),
    ).toBe(false);
    expect(isWorkerToMain({ kind: "unknown" })).toBe(false);
    expect(isWorkerToMain({ type: "wasm_bindgen_worker_ready" })).toBe(false);
  });
});

test("和の結果を画面の文言にする", () => {
  expect(
    describeMessage({ kind: "sum", n: 3, sum: 6n, threads: 4, crossOriginIsolated: true }),
  ).toBe("1 から 3 までの和は 6（スレッド 4 本）");
});
