import init, {
  initThreadPool,
  parallel_sum,
  road_cost_table,
  type Simulation,
  service_table,
  snapshot_layout,
  thread_count,
  ticks_per_second,
} from "../wasm/chizu_wasm.js";
import { ChunkSync } from "./map/chunk-sync.ts";
import { LayerSender, TrafficSender } from "./map/layer-sync.ts";
import type {
  ChunkData,
  CommandMessage,
  MainToWorker,
  MoverArea,
  Outcome,
  WorkerToMain,
} from "./protocol/messages.ts";
import { OVERVIEW_LENGTH } from "./protocol/messages.ts";
import { layout } from "./protocol/snapshot.ts";
import { startSimulation } from "./save/worker-side.ts";
import { stepBudget, tickRate } from "./speed.ts";

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
const trafficSender = new TrafficSender();

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

function setMoverView(sim: Simulation, area: MoverArea | null): void {
  const a = area ?? { minX: 0, minY: 0, maxX: 0, maxY: 0 };
  sim.set_mover_view(a.minX, a.minY, a.maxX, a.maxY, area === null);
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

// 信号の灯火は tick ごとに変わりうるので毎回求め、前と違うときだけ送る
let sentSignals = "";
function sendSignals(sim: Simulation): void {
  const lights = sim.signal_lights();
  const key = lights.join(",");
  if (key === sentSignals) {
    return;
  }
  sentSignals = key;
  reply({ kind: "signals", lights }, [lights.buffer]);
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
  const stats = [...sim.city_stats()];
  sentStats = stats.join(",");
  reply({ kind: "buildings", buildings, facilities, stats }, [buildings.buffer, facilities.buffer]);
}

// 需要は税率、幸福度、人口、職のない住民で、建物の表が変わらなくても動く。住民の数だけ数え直すので、間引いて送る。
// 送るのは前と数が違うときだけで、コマンドのあとはすぐ確かめる。一時停止中は街が進まないので、タイマーからは確かめない
const STATS_INTERVAL_MS = 500;
let lastStatsAt = Number.NEGATIVE_INFINITY;
let sentStats = "";
function sendCityStats(sim: Simulation, now: number, force = false): void {
  if (!force && now - lastStatsAt < STATS_INTERVAL_MS) {
    return;
  }
  lastStatsAt = now;
  const stats = [...sim.city_stats()];
  const key = stats.join(",");
  if (key !== sentStats) {
    sentStats = key;
    reply({ kind: "cityStats", stats });
  }
}

// 収支の見込みは全住民を数えて出すので、毎フレームは求めない。送るのは前と数が違うときだけにする。
// 税収は住民の動きで tick ごとに動くが、画面に出す精度ではこの間隔で足りる
const FINANCE_INTERVAL_MS = 500;
let lastFinanceAt = Number.NEGATIVE_INFINITY;
let lastFinance = "";
function sendFinance(sim: Simulation, now: number, force = false): void {
  if (!force && now - lastFinanceAt < FINANCE_INTERVAL_MS) {
    return;
  }
  lastFinanceAt = now;
  const values = [...sim.finance()];
  const key = values.join(",");
  if (key !== lastFinance) {
    lastFinance = key;
    reply({ kind: "finance", values });
  }
}

// 人口と幸福度は建物の表が変わらなくても動く。住民の数だけ数え直すので、間引いて送る。
// 一時停止中は街が進まないので、最初の 1 回のあとは送らない。最初の 1 回を画面は基準にする
const OVERVIEW_INTERVAL_MS = 500;
let overviewSentAt = Number.NEGATIVE_INFINITY;
function sendOverview(sim: Simulation, now: number, paused: boolean, force = false): void {
  const first = overviewSentAt === Number.NEGATIVE_INFINITY;
  if (!force && !first && (paused || now - overviewSentAt < OVERVIEW_INTERVAL_MS)) {
    return;
  }
  overviewSentAt = now;
  // 固定の並びのあとに、止まっている道路網の並びが続く
  const all = [...sim.overview()];
  reply({
    kind: "overview",
    overview: all.slice(0, OVERVIEW_LENGTH),
    stalls: all.slice(OVERVIEW_LENGTH),
    isolated: [...sim.isolated_facilities()],
    lotShortage: [...sim.lot_shortage()],
    tick: sim.current_tick(),
  });
}

// 実時間に合わせて tick を進める。何 tick 進めるかはここで決め、中核には整数の tick 数だけを渡す
function run(sim: Simulation): {
  setPaused: (paused: boolean) => void;
  setPace: (pace: number) => void;
} {
  const perSecond = ticks_per_second();
  let last = performance.now();
  let budget = 0;
  let paused = false;
  // 1 倍より遅いときの倍率。中核の速度は 1 のままで、進める頻度だけを落とす
  let pace = 1;
  setInterval(() => {
    const now = performance.now();
    const rate = tickRate(perSecond, sim.speed(), pace);
    const step = stepBudget(budget, now - last, rate, paused, MAX_BACKLOG_SECONDS);
    budget = step.budget;
    last = now;
    sim.advance(step.ticks);
    sendOutcomes(sim);
    sendRoads(sim);
    sendSignals(sim);
    sendBuildings(sim);
    if (!paused) {
      sendCityStats(sim, now);
    }
    sendFinance(sim, now);
    sendOverview(sim, now, paused);
    sendChunks(sim);
    layerSender.send(sim, reply, CHUNKS_PER_FRAME, now);
    trafficSender.send(sim, reply, now);
  }, FRAME_MS);
  return {
    setPaused: (p) => {
      paused = p;
      sim.set_paused(p);
    },
    setPace: (p) => {
      pace = p;
      reply({ kind: "pace", pace });
    },
  };
}

const ready = (async () => {
  const wasm = await init();
  await initThreadPool(navigator.hardwareConcurrency);
  if (JSON.stringify(JSON.parse(snapshot_layout())) !== JSON.stringify(layout)) {
    throw new Error("三重バッファの配置が WASM と画面で食い違う。WASM をビルドし直す");
  }
  // 新しく始めるか、保存データから作り直すかは画面が決める
  const sim = await startSimulation();
  sim.advance(0);
  // 三重バッファを確保するとメモリが伸びることがある。伸びたあとの SharedArrayBuffer を渡す。
  // 枠の位置はこのあと変わらないので、のちにメモリが伸びても渡した SharedArrayBuffer の範囲に収まる
  const memory: unknown = wasm.memory.buffer;
  if (!(memory instanceof SharedArrayBuffer)) {
    throw new Error("WASM のメモリが SharedArrayBuffer でない");
  }
  const { setPaused, setPace } = run(sim);
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
    seed: (JSON.parse(sim.save_meta()) as { seed: string }).seed,
  });
  return { sim, setPaused, setPace };
})();

ready.catch((error: unknown) => reply({ kind: "error", message: String(error) }));

self.addEventListener("message", async (event: MessageEvent<MainToWorker>) => {
  const message = event.data;
  try {
    const { sim, setPaused, setPace } = await ready;
    switch (message.kind) {
      case "command":
        // 適用はその場で済むので、止めているときも結果と道路をすぐ返す
        submit(sim, message.id, message.command);
        sendOutcomes(sim);
        sendRoads(sim);
        sendSignals(sim);
        sendBuildings(sim);
        sendCityStats(sim, performance.now(), true);
        sendFinance(sim, performance.now(), true);
        sendOverview(sim, performance.now(), false, true);
        layerSender.send(sim, reply, CHUNKS_PER_FRAME);
        trafficSender.send(sim, reply);
        break;
      case "pace":
        setPace(message.pace);
        break;
      case "layers":
        layerSender.setActive(message.active);
        trafficSender.setActive(message.active);
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
      case "inspect": {
        const { query } = message;
        const info =
          query.kind === "tile"
            ? sim.inspect(query.x, query.y)
            : sim.inspect_target(...query.target);
        reply({ kind: "inspect", query, info: [...info] });
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
        setMoverView(sim, message.movers);
        break;
      case "dev-spawn-walks": {
        const { from, to, count } = message;
        reply({
          kind: "dev-spawned",
          count: sim.spawn_dev_walks(from.x, from.y, to.x, to.y, count),
        });
        break;
      }
    }
  } catch (error) {
    console.error(`操作を受け付けなかった：${String(error)}`);
    // 画面は返事を待っているので、失敗しても返す
    if (message.kind === "feedback") {
      reply({ kind: "feedback-error", message: String(error) });
    }
  }
});
