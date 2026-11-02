// Worker の側の保存と読み込み。画面から始め方を受け取って街を作り、そのあとは保存データを求めに応じて返す。
// 自動保存の頃合いもここで決め、データを画面へ送る。IndexedDB に書くのは画面の側である。

import { Restoring, record_format_version, Simulation } from "../../wasm/chizu_wasm.js";
import {
  isSaveToWorker,
  type SaveFromWorker,
  type SaveMeta,
  type SaveToWorker,
  type SlotId,
} from "./protocol.ts";

const NEW_GAME_SEED = 0n;
/** 再生を一続きに回す時間の上限（ミリ秒）。過ぎたら進み具合を送り、ほかのメッセージに譲る。 */
const RESTORE_SLICE_MS = 50;
/** 1 回の step で進める tick 数。 */
const RESTORE_STEP_TICKS = 64;
const AUTO_SAVE_MS = 60_000;
const AUTO_SAVE_CHECK_MS = 500;

function post(message: SaveFromWorker, transfer: Transferable[] = []): void {
  self.postMessage(message, transfer);
}

// setTimeout(0) は入れ子が 5 段を超えると 4 ミリ秒以上待たされ、再生の区切りごとに待ちが積もる。
// MessageChannel のメッセージなら待たされずに、たまったメッセージを先に片づけてから戻ってこられる。
const yieldChannel = new MessageChannel();
const yieldWaiters: (() => void)[] = [];
yieldChannel.port1.onmessage = () => yieldWaiters.shift()?.();
const yieldToEvents = () =>
  new Promise<void>((resolve) => {
    yieldWaiters.push(resolve);
    yieldChannel.port2.postMessage(null);
  });

/** 作り直した街と、状態ハッシュが保存したときと違うか（シミュレーションの規則が変わったか）。 */
async function restore(save: {
  meta: SaveMeta;
  commands: ArrayBuffer;
}): Promise<{ sim: Simulation; rulesChanged: boolean }> {
  const { meta } = save;
  const r = new Restoring(
    meta.formatVersion,
    meta.seed,
    meta.tick,
    meta.commandCount,
    meta.stateHash,
    new Uint8Array(save.commands),
  );
  post({ kind: "save:progress", tick: r.tick(), target: r.target() });
  while (!r.done()) {
    const start = performance.now();
    while (!r.done() && performance.now() - start < RESTORE_SLICE_MS) {
      r.step(RESTORE_STEP_TICKS);
    }
    post({ kind: "save:progress", tick: r.tick(), target: r.target() });
    await yieldToEvents();
  }
  const restored = r.finish();
  try {
    return { sim: restored.take(), rulesChanged: restored.rules_changed() };
  } finally {
    restored.free();
  }
}

// メタデータと記録は同じ時点のものを 1 回で取る
function payloadOf(sim: Simulation): { meta: SaveMeta; commands: ArrayBuffer } {
  const payload = sim.save();
  try {
    const meta = JSON.parse(payload.meta()) as SaveMeta;
    return { meta, commands: payload.take_commands().buffer as ArrayBuffer };
  } finally {
    payload.free();
  }
}

const metaOf = (sim: Simulation) => JSON.parse(sim.save_meta()) as SaveMeta;

function sendSave(sim: Simulation, slot: SlotId): void {
  const { meta, commands } = payloadOf(sim);
  post({ kind: "save:data", slot, meta, commands }, [commands]);
}

// ゲーム内の日が変わったとき、または実時間で 1 分たって何か変わっていたときに自動保存する
function startAutoSave(sim: Simulation): void {
  const clock = () => {
    const [tick = 0, day = 0, commands = 0] = sim.save_clock();
    return { tick, day, commands };
  };
  let last = clock();
  let lastAt = performance.now();
  setInterval(() => {
    const now = clock();
    const changed = now.tick !== last.tick || now.commands !== last.commands;
    const elapsed = performance.now() - lastAt >= AUTO_SAVE_MS;
    if (now.day !== last.day || (elapsed && changed)) {
      sendSave(sim, "auto");
      last = now;
      lastAt = performance.now();
    }
  }, AUTO_SAVE_CHECK_MS);
}

/**
 * 画面が始め方を決めるのを待って、街を作る。読み込めなかったときは画面に伝え、次の指示を待つ。
 * 作ったあとは、保存データを求められたら返し、画面が許したときは自動保存も回す。
 */
export function startSimulation(): Promise<Simulation> {
  let sim: Simulation | null = null;
  let starting = false;
  return new Promise((resolve) => {
    const begin = async (message: Extract<SaveToWorker, { kind: "save:start" }>) => {
      starting = true;
      try {
        const loaded = message.save === null ? null : await restore(message.save);
        const created = loaded?.sim ?? new Simulation(NEW_GAME_SEED);
        sim = created;
        post({
          kind: "save:started",
          loaded:
            loaded === null ? null : { meta: metaOf(created), rulesChanged: loaded.rulesChanged },
        });
        if (message.autoSave) {
          startAutoSave(created);
        }
        resolve(created);
      } catch (error) {
        post({ kind: "save:load-error", message: String(error) });
      } finally {
        starting = false;
      }
    };
    self.addEventListener("message", (event: MessageEvent<unknown>) => {
      const message = event.data;
      if (!isSaveToWorker(message)) {
        return;
      }
      if (message.kind === "save:start") {
        if (sim === null && !starting) {
          void begin(message);
        }
      } else if (sim !== null) {
        sendSave(sim, message.slot);
      }
    });
    post({ kind: "save:hello", formatVersion: record_format_version() });
  });
}
