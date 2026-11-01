import { describeMessage, isWorkerToMain, type MainToWorker } from "./protocol.ts";

const SUM_N = 10_000_000;

const status = document.querySelector<HTMLElement>("#status");
if (status === null) {
  throw new Error("#status が見つからない");
}

const worker = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });

worker.addEventListener("message", (event: MessageEvent<unknown>) => {
  const message = event.data;
  if (!isWorkerToMain(message)) {
    return;
  }
  status.textContent = describeMessage(message);
  status.dataset.state = message.kind === "sum" ? "done" : "error";
  if (message.kind === "sum") {
    status.dataset.n = String(message.n);
    status.dataset.sum = String(message.sum);
    status.dataset.threads = String(message.threads);
    status.dataset.workerIsolated = String(message.crossOriginIsolated);
  }
});

worker.addEventListener("error", (event) => {
  status.textContent = `Worker を起こせなかった：${event.message}`;
  status.dataset.state = "error";
});

const request: MainToWorker = { kind: "sum", n: SUM_N };
worker.postMessage(request);
