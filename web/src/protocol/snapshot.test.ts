import { readFileSync } from "node:fs";
import { Worker } from "node:worker_threads";
import { describe, expect, test } from "vitest";
import { layout, SnapshotReader, slotBase } from "./snapshot.ts";
import { allocateSnapshotBuffer, CHECKED_WORDS, TestWriter } from "./snapshot-writer.testing.ts";

function setHeader(tick: number, money: number) {
  return (words: Uint32Array, base: number) => {
    const h = layout.header;
    words[base + h.tickLo] = tick % 2 ** 32;
    words[base + h.tickHi] = Math.floor(tick / 2 ** 32);
    words[base + h.moneyLo] = money >>> 0;
    words[base + h.moneyHi] = Math.floor(money / 2 ** 32) >>> 0;
    words[base + h.speed] = 4;
    words[base + h.paused] = 1;
  };
}

describe("三重バッファ", () => {
  test("書かれる前は null を返す", () => {
    const reader = new SnapshotReader(allocateSnapshotBuffer(), 0);
    expect(reader.acquire()).toBeNull();
  });

  test("配置の版が違えば読まない", () => {
    const buffer = allocateSnapshotBuffer();
    new Int32Array(buffer)[layout.control.version] = layout.version + 1;
    expect(() => new SnapshotReader(buffer, 0)).toThrow();
  });

  test("先頭の数値を読む", () => {
    const buffer = allocateSnapshotBuffer();
    const writer = new TestWriter(buffer);
    const reader = new SnapshotReader(buffer, 0);
    writer.publish(setHeader(2 ** 40 + 3, -1_234_567));
    expect(reader.acquire()).toMatchObject({
      tick: 2 ** 40 + 3,
      money: -1_234_567,
      speed: 4,
      paused: true,
    });
  });

  test("Rust の書き手が書いた枠を読む", () => {
    // crates/chizu-wasm/src/snapshot.rs の fixture_bytes が書き出したもの。
    // 制御の語のあとに、各枠の先頭の数値だけが並んでいる
    const bytes = readFileSync(new URL("./snapshot-fixture.bin", import.meta.url));
    const fixture = new Uint32Array(bytes.buffer, bytes.byteOffset, bytes.byteLength / 4);
    expect(fixture.length).toBe(layout.controlWords + layout.slotCount * layout.slotHeaderWords);
    const buffer = new SharedArrayBuffer(layout.totalWords * 4);
    const words = new Uint32Array(buffer);
    words.set(fixture.subarray(0, layout.controlWords));
    for (let slot = 0; slot < layout.slotCount; slot++) {
      const from = layout.controlWords + slot * layout.slotHeaderWords;
      words.set(fixture.subarray(from, from + layout.slotHeaderWords), slotBase(slot));
    }

    const reader = new SnapshotReader(buffer, 0);
    expect(reader.acquire()).toEqual({
      tick: 2 ** 32 + 7,
      day: 3,
      secondOfDay: 45_672,
      money: -1_234_567_890_123,
      population: 42,
      speed: 48,
      paused: true,
      moverCount: 0,
      baseCx: -3,
      baseCy: 5,
      trainCarCount: 0,
      trainBaseCx: 2,
      trainBaseCy: -7,
      closedCrossingCount: 0,
    });
  });

  test("列車の車両は移動体のあとに、移動体と同じ語の並びで置かれる", () => {
    const buffer = allocateSnapshotBuffer();
    const writer = new TestWriter(buffer);
    const reader = new SnapshotReader(buffer, 0);
    writer.publish((words, base) => {
      words[base + layout.header.trainCarCount] = 2;
      for (let i = 0; i < 2; i++) {
        const at = base + layout.carsStart + i * layout.moverWords;
        words[at + layout.mover.id] = 40 + i;
      }
    });
    const header = reader.acquire();
    expect(header?.trainCarCount).toBe(2);
    const cars = reader.cars(header?.trainCarCount ?? 0);
    expect(cars.length).toBe(2 * layout.moverWords);
    expect(cars[layout.mover.id]).toBe(40);
    expect(cars[layout.moverWords + layout.mover.id]).toBe(41);
    // 移動体の欄とは重ならない
    expect(layout.carsStart).toBe(layout.slotHeaderWords + layout.maxMovers * layout.moverWords);
    expect(reader.cars(10 ** 6).length).toBe(layout.maxTrainCars * layout.moverWords);
  });

  test("閉じている踏切のマスは車両のあとに、x と y の組で置かれる", () => {
    const buffer = allocateSnapshotBuffer();
    const writer = new TestWriter(buffer);
    const reader = new SnapshotReader(buffer, 0);
    writer.publish((words, base) => {
      words[base + layout.header.closedCrossingCount] = 2;
      const at = base + layout.crossingsStart;
      words[at] = 30;
      words[at + 1] = (-8 >>> 0) as number;
      words[at + layout.crossingWords] = 5;
      words[at + layout.crossingWords + 1] = 6;
    });
    const header = reader.acquire();
    expect(header?.closedCrossingCount).toBe(2);
    expect(Array.from(reader.closedCrossings(header?.closedCrossingCount ?? 0))).toEqual([
      30, -8, 5, 6,
    ]);
    // 車両の欄とは重ならない
    expect(layout.crossingsStart).toBe(layout.carsStart + layout.maxTrainCars * layout.moverWords);
    expect(reader.closedCrossings(10 ** 6).length).toBe(
      layout.maxClosedCrossings * layout.crossingWords,
    );
  });

  test("読み手は最新の枠を読み、書き手の枠とは重ならない", () => {
    const buffer = allocateSnapshotBuffer();
    const writer = new TestWriter(buffer);
    const reader = new SnapshotReader(buffer, 0);
    for (let tick = 1; tick <= 20; tick++) {
      writer.publish(setHeader(tick, 0));
      if (tick % 3 === 0) {
        writer.publish(setHeader(tick + 1000, 0));
        expect(reader.acquire()?.tick).toBe(tick + 1000);
      } else {
        expect(reader.acquire()?.tick).toBe(tick);
      }
      expect(reader.reading).not.toBe(writer.writing);
      // 新しく書かれていなければ、同じ枠を読み続ける
      const slot = reader.reading;
      reader.acquire();
      expect(reader.reading).toBe(slot);
    }
  });

  test("別のスレッドの書き手と並べて回しても、読む枠が書きかけにならない", async () => {
    const buffer = allocateSnapshotBuffer();
    const reader = new SnapshotReader(buffer, 0);
    const words = new Uint32Array(buffer);
    const count = 20_000;
    const worker = new Worker(new URL("./snapshot-writer.worker.ts", import.meta.url), {
      workerData: { buffer, count },
    });
    let done = false;
    const finished = new Promise<void>((resolve, reject) => {
      worker.once("message", () => {
        done = true;
        resolve();
      });
      worker.once("error", reject);
    });

    let previous = 0;
    let reads = 0;
    let torn = 0;
    let backwards = 0;
    while (!done || previous < count) {
      const header = reader.acquire();
      if (header !== null) {
        const base = slotBase(reader.reading);
        const seq = words[base] ?? 0;
        for (let i = 1; i < CHECKED_WORDS; i++) {
          if (words[base + i] !== seq) {
            torn++;
            break;
          }
        }
        if (seq < previous) {
          backwards++;
        }
        previous = seq;
        reads++;
      }
      // 書き手の終わりを受け取るため、ときどきイベントループに戻る
      if (reads % 256 === 0) {
        await new Promise((resolve) => setImmediate(resolve));
      }
    }
    await finished;
    await worker.terminate();

    expect(torn).toBe(0);
    expect(backwards).toBe(0);
    expect(previous).toBe(count);
  });
});
