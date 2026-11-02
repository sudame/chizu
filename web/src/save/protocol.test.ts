import { describe, expect, it } from "vitest";
import {
  describeSave,
  isSaveFromWorker,
  isSaveRecord,
  isSaveToWorker,
  type SaveRecord,
  unreadableReason,
} from "./protocol.ts";

const record: SaveRecord = {
  slot: "manual-1",
  savedAt: new Date(2026, 10, 1, 9, 5).getTime(),
  meta: {
    formatVersion: 1,
    seed: "0",
    tick: 7_500,
    commandCount: 3,
    day: 1,
    secondOfDay: 3_600 * 13 + 60 * 7,
    money: "1234567",
    stateHash: "00000000000000ff",
  },
  commands: new ArrayBuffer(4),
};

describe("保存データ", () => {
  it("保存した日時、ゲーム内の日付、所持金を並べる", () => {
    expect(describeSave(record)).toBe("2026/11/01 09:05 保存・2 日目 13:07・お金 1,234,567");
  });

  it("記録の形式の版が違えば読めない", () => {
    expect(unreadableReason(record, 1)).toBeNull();
    expect(unreadableReason(record, 2)).toContain("読めない");
  });

  it("値の壊れたものを受け付けない", () => {
    const broken: [string, Record<string, unknown>][] = [
      ["所持金が整数でない", { money: "abc" }],
      ["所持金が小数", { money: "1.5" }],
      ["種が負", { seed: "-1" }],
      ["状態ハッシュが 16 進でない", { stateHash: "zzzzzzzzzzzzzzzz" }],
      ["tick が負", { tick: -1 }],
      ["時刻が 1 日を超える", { secondOfDay: 86_400 }],
    ];
    for (const [label, change] of broken) {
      expect(isSaveRecord({ ...record, meta: { ...record.meta, ...change } }), label).toBe(false);
    }
    expect(isSaveRecord({ ...record, savedAt: Number.NaN })).toBe(false);
    expect(isSaveRecord({ ...record, savedAt: Number.POSITIVE_INFINITY })).toBe(false);
    expect(isSaveRecord({ ...record, meta: { ...record.meta, money: "-20" } })).toBe(true);
  });

  it("形の崩れたものを受け付けない", () => {
    expect(isSaveRecord(record)).toBe(true);
    expect(isSaveRecord({ ...record, slot: "manual-9" })).toBe(false);
    expect(isSaveRecord({ ...record, meta: { ...record.meta, tick: 1.5 } })).toBe(false);
    expect(isSaveFromWorker({ kind: "save:progress", tick: 1, target: 2 })).toBe(true);
    expect(isSaveToWorker({ kind: "save:start", save: null, autoSave: true })).toBe(true);
    expect(isSaveToWorker({ kind: "save:start", save: null })).toBe(false);
    expect(isSaveFromWorker({ kind: "ready" })).toBe(false);
  });

  it("街の状態は、ないか ArrayBuffer のときだけ受け付ける", () => {
    expect(isSaveRecord({ ...record, state: new ArrayBuffer(8) })).toBe(true);
    expect(isSaveRecord({ ...record, state: "abc" })).toBe(false);
    expect(isSaveRecord({ ...record, state: null })).toBe(false);
    const save = { meta: record.meta, commands: new ArrayBuffer(4) };
    for (const state of [null, new ArrayBuffer(8)]) {
      expect(isSaveToWorker({ kind: "save:start", save: { ...save, state }, autoSave: true })).toBe(
        true,
      );
    }
    expect(isSaveToWorker({ kind: "save:start", save, autoSave: true })).toBe(false);
    const data = { kind: "save:data", slot: "auto", ...save };
    expect(isSaveFromWorker({ ...data, state: new ArrayBuffer(8) })).toBe(true);
    expect(isSaveFromWorker(data)).toBe(false);
  });

  it("読み込んだ街には、状態から開いたかを添える", () => {
    const loaded = { meta: record.meta, rulesChanged: false };
    expect(isSaveFromWorker({ kind: "save:started", loaded: { ...loaded, fromState: true } })).toBe(
      true,
    );
    expect(isSaveFromWorker({ kind: "save:started", loaded })).toBe(false);
  });
});
