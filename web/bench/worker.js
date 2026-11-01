// scripts/bench-wasm.ts が開く Worker。受け取ったスレッド数で rayon のスレッドプールを作り、測った結果を返す。
// 1 つの WASM のインスタンスではスレッドプールを 1 度しか作れないので、スレッド数ごとに Worker を開き直す。
import init, { initThreadPool, run_all } from "/pkg/chizu_bench_wasm.js";

self.onmessage = async ({ data }) => {
  try {
    await init();
    await initThreadPool(data.threads);
    self.postMessage({ ok: true, results: JSON.parse(run_all()) });
  } catch (error) {
    self.postMessage({ ok: false, error: String(error) });
  }
};
