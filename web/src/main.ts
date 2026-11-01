import { isWorkerToMain, type MainToWorker } from "./protocol/messages.ts";
import { type SnapshotHeader, SnapshotReader } from "./protocol/snapshot.ts";

function element<T extends Element>(selector: string): T {
  const found = document.querySelector<T>(selector);
  if (found === null) {
    throw new Error(`${selector} が見つからない`);
  }
  return found;
}

const status = element<HTMLElement>("#status");
const canvas = element<HTMLCanvasElement>("#hud");
const pauseButton = element<HTMLButtonElement>("#pause");
const context = canvas.getContext("2d");
if (context === null) {
  throw new Error("canvas の 2D の描画を使えない");
}

const worker = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });

function send(message: MainToWorker): void {
  worker.postMessage(message);
}

const pad = (n: number) => String(n).padStart(2, "0");

function describeClock(header: SnapshotHeader): string {
  const hour = Math.floor(header.secondOfDay / 3600);
  const minute = Math.floor(header.secondOfDay / 60) % 60;
  return `${header.day + 1} 日目 ${pad(hour)}:${pad(minute)}`;
}

function draw(ctx: CanvasRenderingContext2D, header: SnapshotHeader): void {
  ctx.clearRect(0, 0, canvas.width, canvas.height);
  ctx.fillStyle = "#222";
  ctx.font = "20px sans-serif";
  ctx.fillText(describeClock(header), 12, 30);
  ctx.fillText(`お金 ${header.money.toLocaleString("ja-JP")}`, 12, 58);
  ctx.fillText(header.paused ? "一時停止中" : `速度 ${header.speed} 倍`, 12, 86);
  // test:wasm が読む
  canvas.dataset.tick = String(header.tick);
  canvas.dataset.speed = String(header.speed);
  canvas.dataset.paused = String(header.paused);
  canvas.dataset.day = String(header.day);
  canvas.dataset.secondOfDay = String(header.secondOfDay);
  canvas.dataset.money = String(header.money);
}

function startDrawing(reader: SnapshotReader, ctx: CanvasRenderingContext2D): void {
  const frame = () => {
    const header = reader.acquire();
    if (header !== null) {
      draw(ctx, header);
    }
    requestAnimationFrame(frame);
  };
  requestAnimationFrame(frame);
}

let paused = false;
pauseButton.addEventListener("click", () => {
  paused = !paused;
  pauseButton.textContent = paused ? "再開" : "一時停止";
  send({ kind: "pause", paused });
});

for (const button of document.querySelectorAll<HTMLButtonElement>("button[data-speed]")) {
  button.addEventListener("click", () => {
    send({
      kind: "command",
      command: { kind: "SetSimSpeed", speed: Number(button.dataset.speed) },
    });
  });
}

worker.addEventListener("message", (event: MessageEvent<unknown>) => {
  const message = event.data;
  if (!isWorkerToMain(message)) {
    return;
  }
  if (message.kind === "error") {
    status.textContent = `WASM を動かせなかった：${message.message}`;
    status.dataset.state = "error";
    return;
  }
  let reader: SnapshotReader;
  try {
    reader = new SnapshotReader(message.memory, message.snapshotOffset);
  } catch (error) {
    status.textContent = `三重バッファを読めない：${String(error)}`;
    status.dataset.state = "error";
    return;
  }
  status.textContent = `動いている（スレッド ${message.threads} 本）`;
  status.dataset.state = "running";
  status.dataset.n = String(message.n);
  status.dataset.sum = String(message.sum);
  status.dataset.threads = String(message.threads);
  status.dataset.workerIsolated = String(message.crossOriginIsolated);
  startDrawing(reader, context);
});

worker.addEventListener("error", (event) => {
  status.textContent = `Worker を起こせなかった：${event.message}`;
  status.dataset.state = "error";
});
