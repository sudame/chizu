import { Camera, type ChunkRange, chunksByDistance, type Viewport } from "./map/camera.ts";
import { ChunkStore } from "./map/chunk-sync.ts";
import { MapRenderer } from "./map/renderer.ts";
import { CHUNK_SIDE, isWorkerToMain, type MainToWorker } from "./protocol/messages.ts";
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
const mapCanvas = element<HTMLCanvasElement>("#map");
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

// 手元に置くチャンクの数の上限。超えたら見えていないものから捨てる
const MAX_STORED_CHUNKS = 4096;
// ホイールの 1 ピクセル分の拡大率の変わり方
const WHEEL_ZOOM_RATE = 0.0015;
const WHEEL_LINE_PIXELS = 16;

// 開始地点の買ってあるチャンク（-1〜1）の真ん中から見始める
const camera = new Camera(CHUNK_SIDE / 2, CHUNK_SIDE / 2, 4);
const store = new ChunkStore();
const renderer = new MapRenderer(mapCanvas);

const viewport = (): Viewport => ({
  width: mapCanvas.clientWidth,
  height: mapCanvas.clientHeight,
});
const inRange = (r: ChunkRange, cx: number, cy: number) =>
  cx >= r.minCx && cx <= r.maxCx && cy >= r.minCy && cy <= r.maxCy;

let sentRange = "";
// 見えているチャンクの範囲が変わったときだけ、手元の版を添えて Worker に伝える
function sendView(range: ChunkRange): void {
  const key = `${range.minCx},${range.maxCx},${range.minCy},${range.maxCy}`;
  if (key === sentRange) {
    return;
  }
  sentRange = key;
  for (const removed of store.evict((cx, cy) => inRange(range, cx, cy), MAX_STORED_CHUNKS)) {
    renderer.remove(removed.cx, removed.cy);
  }
  const center = { x: camera.x, y: camera.y };
  send({ kind: "view", chunks: store.known(chunksByDistance(range, center)) });
}

function drawMap(): void {
  const vp = viewport();
  const range = camera.visibleChunks(vp);
  sendView(range);
  const drawn = renderer.draw(camera, vp, range);
  // test:wasm が読む
  mapCanvas.dataset.drawnChunks = String(drawn);
  mapCanvas.dataset.visibleChunks = String(
    (range.maxCx - range.minCx + 1) * (range.maxCy - range.minCy + 1),
  );
  mapCanvas.dataset.storedChunks = String(store.size);
  mapCanvas.dataset.cameraX = String(camera.x);
  mapCanvas.dataset.cameraY = String(camera.y);
  mapCanvas.dataset.zoom = String(camera.zoom);
}

function startDrawing(reader: SnapshotReader, ctx: CanvasRenderingContext2D): void {
  const frame = () => {
    const header = reader.acquire();
    if (header !== null) {
      draw(ctx, header);
    }
    drawMap();
    requestAnimationFrame(frame);
  };
  requestAnimationFrame(frame);
}

let dragging: { id: number; x: number; y: number } | null = null;
mapCanvas.addEventListener("pointerdown", (event) => {
  if (event.button !== 0) {
    return;
  }
  dragging = { id: event.pointerId, x: event.clientX, y: event.clientY };
  mapCanvas.setPointerCapture(event.pointerId);
});
mapCanvas.addEventListener("pointermove", (event) => {
  if (dragging?.id !== event.pointerId) {
    return;
  }
  camera.pan(event.clientX - dragging.x, event.clientY - dragging.y);
  dragging.x = event.clientX;
  dragging.y = event.clientY;
});
const endDrag = (event: PointerEvent) => {
  if (dragging?.id === event.pointerId) {
    dragging = null;
  }
};
mapCanvas.addEventListener("pointerup", endDrag);
mapCanvas.addEventListener("pointercancel", endDrag);
mapCanvas.addEventListener(
  "wheel",
  (event) => {
    event.preventDefault();
    const pixels = event.deltaMode === WheelEvent.DOM_DELTA_LINE ? WHEEL_LINE_PIXELS : 1;
    const rect = mapCanvas.getBoundingClientRect();
    camera.zoomAt(
      event.clientX - rect.left,
      event.clientY - rect.top,
      Math.exp(-event.deltaY * pixels * WHEEL_ZOOM_RATE),
      viewport(),
    );
  },
  { passive: false },
);

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
  if (message.kind === "chunks") {
    for (const chunk of store.receive(message.chunks)) {
      renderer.upload(chunk);
    }
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
