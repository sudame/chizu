import init, {
  initThreadPool,
  parallel_sum,
  Simulation,
  snapshot_layout,
  thread_count,
  ticks_per_second,
} from "../wasm/chizu_wasm.js";
import { ChunkSync } from "./map/chunk-sync.ts";
import type { ChunkData, MainToWorker, WorkerToMain } from "./protocol/messages.ts";
import { layout } from "./protocol/snapshot.ts";

const SUM_N = 10_000_000;
const FRAME_MS = 1000 / 60;
// 処理が追いつかないときに取り戻す遅れの上限（実時間の秒）。超えた分は捨てる
const MAX_BACKLOG_SECONDS = 0.25;
// 1 回に送るチャンクの数の上限。見えていないチャンクの地形は種から作るので、まとめて作ると tick が遅れる
const CHUNKS_PER_FRAME = 32;

function reply(message: WorkerToMain, transfer: Transferable[] = []): void {
  self.postMessage(message, transfer);
}

const chunkSync = new ChunkSync();

// 画面の手元と版の違うチャンクを送る。マスの配列は移譲するので、送ったあとの Worker の側には残らない
function sendChunks(sim: Simulation): void {
  const picked = chunkSync.collect((cx, cy) => sim.chunk_version(cx, cy), CHUNKS_PER_FRAME);
  if (picked.length === 0) {
    return;
  }
  const chunks: ChunkData[] = picked.map(({ cx, cy, version }) => ({
    cx,
    cy,
    version,
    owned: sim.chunk_owned(cx, cy),
    tiles: sim.chunk_tiles(cx, cy).buffer as ArrayBuffer,
  }));
  reply(
    { kind: "chunks", chunks },
    chunks.map((c) => c.tiles),
  );
}

// 実時間に合わせて tick を進める。何 tick 進めるかはここで決め、中核には整数の tick 数だけを渡す
function run(sim: Simulation): (paused: boolean) => void {
  const perSecond = ticks_per_second();
  let last = performance.now();
  let budget = 0;
  let paused = false;
  setInterval(() => {
    const now = performance.now();
    const rate = perSecond * sim.speed();
    budget = paused
      ? 0
      : Math.min(budget + ((now - last) / 1000) * rate, rate * MAX_BACKLOG_SECONDS);
    last = now;
    const ticks = Math.floor(budget);
    budget -= ticks;
    sim.advance(ticks);
    sendChunks(sim);
  }, FRAME_MS);
  return (p) => {
    paused = p;
    sim.set_paused(p);
  };
}

const ready = (async () => {
  const wasm = await init();
  await initThreadPool(navigator.hardwareConcurrency);
  if (JSON.stringify(JSON.parse(snapshot_layout())) !== JSON.stringify(layout)) {
    throw new Error("三重バッファの配置が WASM と画面で食い違う。WASM をビルドし直す");
  }
  const sim = new Simulation(0n);
  sim.advance(0);
  // 三重バッファを確保するとメモリが伸びることがある。伸びたあとの SharedArrayBuffer を渡す。
  // 枠の位置はこのあと変わらないので、のちにメモリが伸びても渡した SharedArrayBuffer の範囲に収まる
  const memory: unknown = wasm.memory.buffer;
  if (!(memory instanceof SharedArrayBuffer)) {
    throw new Error("WASM のメモリが SharedArrayBuffer でない");
  }
  const setPaused = run(sim);
  reply({
    kind: "ready",
    memory,
    snapshotOffset: sim.snapshot_offset(),
    threads: thread_count(),
    crossOriginIsolated: self.crossOriginIsolated,
    n: SUM_N,
    sum: parallel_sum(SUM_N),
  });
  return { sim, setPaused };
})();

ready.catch((error: unknown) => reply({ kind: "error", message: String(error) }));

self.addEventListener("message", async (event: MessageEvent<MainToWorker>) => {
  const message = event.data;
  try {
    const { sim, setPaused } = await ready;
    switch (message.kind) {
      case "command":
        sim.submit(message.command);
        break;
      case "pause":
        setPaused(message.paused);
        break;
      case "view":
        chunkSync.setView(message.chunks);
        sendChunks(sim);
        break;
    }
  } catch (error) {
    console.error(`操作を受け付けなかった：${String(error)}`);
  }
});
