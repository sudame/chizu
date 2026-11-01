import init, {
  initThreadPool,
  parallel_sum,
  road_cost_table,
  Simulation,
  service_table,
  snapshot_layout,
  thread_count,
  ticks_per_second,
} from "../wasm/chizu_wasm.js";
import { ChunkSync } from "./map/chunk-sync.ts";
import { LayerSender } from "./map/layer-sync.ts";
import type {
  ChunkData,
  CommandMessage,
  MainToWorker,
  Outcome,
  WorkerToMain,
} from "./protocol/messages.ts";
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
const layerSender = new LayerSender();

// 画面の手元と版の違うチャンクを送る。マスの配列は移譲するので、送ったあとの Worker の側には残らない
function sendChunks(sim: Simulation): void {
  const picked = chunkSync.collect((cx, cy) => sim.chunk_tiles_version(cx, cy), CHUNKS_PER_FRAME);
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

// 記録の通し番号から、画面が振った番号を引く
const pending = new Map<number, number>();

function submit(sim: Simulation, id: number, command: CommandMessage): void {
  try {
    pending.set(sim.submit(command), id);
  } catch (error) {
    reply({ kind: "outcome", id, outcome: "Rejected", reason: String(error) });
  }
}

function sendOutcomes(sim: Simulation): void {
  for (const report of sim.take_reports() as { seq: number; outcome: Outcome; reason?: string }[]) {
    const id = pending.get(report.seq);
    pending.delete(report.seq);
    if (id !== undefined) {
      const { seq, outcome, reason } = report;
      reply({ kind: "outcome", id, seq, outcome, reason });
    }
  }
}

// 道路網は版が変わったときだけ、全体を送り直す。区間が増えて送る量が問題になったら、
// 見えているチャンクにかかる区間だけを、チャンクの版と一緒に送る形に変える
let sentNetworkRevision = -1;
function sendRoads(sim: Simulation): void {
  const revision = sim.network_version();
  if (revision === sentNetworkRevision) {
    return;
  }
  sentNetworkRevision = revision;
  const segments = sim.road_segments();
  const nodes = sim.road_nodes();
  reply({ kind: "roads", revision, segments, nodes }, [segments.buffer, nodes.buffer]);
}

// 建物と街の数字は、建物の表か道路網が変わったときだけ全体を送り直す。足りない網の数は網のつながりで変わる
let sentBuildingsVersion = -1;
function sendBuildings(sim: Simulation): void {
  const version = sim.supply_version();
  if (version === sentBuildingsVersion) {
    return;
  }
  sentBuildingsVersion = version;
  const buildings = sim.buildings();
  const facilities = sim.facilities();
  reply({ kind: "buildings", buildings, facilities, stats: [...sim.city_stats()] }, [
    buildings.buffer,
    facilities.buffer,
  ]);
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
    sendOutcomes(sim);
    sendRoads(sim);
    sendBuildings(sim);
    sendChunks(sim);
    layerSender.send(sim, reply, CHUNKS_PER_FRAME, now);
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
    roadCosts: [...road_cost_table()],
    serviceTable: [...service_table()],
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
        // 適用はその場で済むので、止めているときも結果と道路をすぐ返す
        submit(sim, message.id, message.command);
        sendOutcomes(sim);
        sendRoads(sim);
        sendBuildings(sim);
        layerSender.send(sim, reply, CHUNKS_PER_FRAME);
        break;
      case "layers":
        layerSender.setActive(message.active);
        break;
      case "pause":
        setPaused(message.paused);
        break;
      case "probe-service": {
        const { service, x, y } = message;
        const code = service === "PowerPlant" ? 1 : 2;
        const results = [...sim.service_probe(code, x, y)] as (string | null)[];
        reply({ kind: "service-probe", service, x, y, results });
        break;
      }
      case "feedback": {
        const commands = sim.encode_log().buffer as ArrayBuffer;
        reply({ kind: "feedback", state: sim.feedback_state(), commands }, [commands]);
        break;
      }
      case "view":
        // 送るのはタイマーの側だけにして、1 周期に送る数を CHUNKS_PER_FRAME に抑える
        chunkSync.setView(message.visible, message.forgotten);
        layerSender.setView(message.visible, message.forgotten);
        break;
    }
  } catch (error) {
    console.error(`操作を受け付けなかった：${String(error)}`);
    // 画面は返事を待っているので、失敗しても返す
    if (message.kind === "feedback") {
      reply({ kind: "feedback-error", message: String(error) });
    }
  }
});
