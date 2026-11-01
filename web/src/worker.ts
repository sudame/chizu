import init, { initThreadPool, parallel_sum, thread_count } from "../wasm/chizu_wasm.js";
import type { MainToWorker, WorkerToMain } from "./protocol.ts";

const ready = (async () => {
  await init();
  await initThreadPool(navigator.hardwareConcurrency);
})();

function reply(message: WorkerToMain): void {
  self.postMessage(message);
}

self.addEventListener("message", async (event: MessageEvent<MainToWorker>) => {
  const { n } = event.data;
  try {
    await ready;
    reply({
      kind: "sum",
      n,
      sum: parallel_sum(n),
      threads: thread_count(),
      crossOriginIsolated: self.crossOriginIsolated,
    });
  } catch (error) {
    reply({ kind: "error", message: String(error) });
  }
});
