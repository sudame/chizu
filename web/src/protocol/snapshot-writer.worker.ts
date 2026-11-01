// 別のスレッドで書き手を回す。枠の中の語をすべて通し番号で埋めて渡すことを count 回くり返す。

import { parentPort, workerData } from "node:worker_threads";
import { fillAll, TestWriter } from "./snapshot-writer.testing.ts";

const { buffer, count } = workerData as { buffer: SharedArrayBuffer; count: number };
const writer = new TestWriter(buffer);
for (let seq = 1; seq <= count; seq++) {
  writer.publish(fillAll(seq));
}
parentPort?.postMessage("done");
