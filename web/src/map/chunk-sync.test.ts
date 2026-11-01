import { describe, expect, test } from "vitest";
import { CHUNK_AREA, type ChunkData } from "../protocol/messages.ts";
import { ChunkStore, ChunkSync } from "./chunk-sync.ts";

function data(cx: number, cy: number, version: number, owned = false): ChunkData {
  return { cx, cy, version, owned, tiles: new ArrayBuffer(CHUNK_AREA) };
}

const at = (...xs: number[]) => xs.map((cx) => ({ cx, cy: 0 }));

describe("ChunkSync", () => {
  const versions = new Map([
    ["0,0", 3],
    ["1,0", 0],
    ["2,0", 5],
  ]);
  const versionOf = (cx: number, cy: number) => versions.get(`${cx},${cy}`) ?? 0;

  test("まだ送っていないチャンクだけを選び、一度送ったものは選ばない", () => {
    const sync = new ChunkSync();
    sync.setView(at(0, 1, 2), []);
    expect(sync.collect(versionOf, 10)).toEqual([
      { cx: 0, cy: 0, version: 3 },
      { cx: 1, cy: 0, version: 0 },
      { cx: 2, cy: 0, version: 5 },
    ]);
    expect(sync.collect(versionOf, 10)).toEqual([]);
  });

  test("中核の側で版が上がったら送り直す", () => {
    const local = new Map(versions);
    const sync = new ChunkSync();
    sync.setView(at(0), []);
    const of = (cx: number, cy: number) => local.get(`${cx},${cy}`) ?? 0;
    expect(sync.collect(of, 10)).toHaveLength(1);
    local.set("0,0", 4);
    expect(sync.collect(of, 10)).toEqual([{ cx: 0, cy: 0, version: 4 }]);
  });

  test("送ったチャンクが届く前に新しい view が来ても、送り直さない", () => {
    const sync = new ChunkSync();
    sync.setView(at(0, 1), []);
    expect(sync.collect(versionOf, 10)).toHaveLength(2);
    // 画面はまだ何も受け取っていないまま、範囲を広げた
    sync.setView(at(0, 1, 2), []);
    expect(sync.collect(versionOf, 10)).toEqual([{ cx: 2, cy: 0, version: 5 }]);
  });

  test("画面が捨てたチャンクは、また見えたら送り直す", () => {
    const sync = new ChunkSync();
    sync.setView(at(0, 1), []);
    sync.collect(versionOf, 10);
    sync.setView(at(2), at(0));
    sync.collect(versionOf, 10);
    sync.setView(at(0, 1), []);
    expect(sync.collect(versionOf, 10)).toEqual([{ cx: 0, cy: 0, version: 3 }]);
  });

  test("view が何度来ても、1 周期に選ぶ数は limit を超えず、残りは次の周期に回る", () => {
    const sync = new ChunkSync();
    const view = at(0, 1, 2, 3, 4);
    const picked: number[] = [];
    for (let cycle = 0; cycle < 3; cycle++) {
      sync.setView(view, []);
      sync.setView(view, []);
      const sent = sync.collect(versionOf, 2);
      expect(sent.length).toBeLessThanOrEqual(2);
      picked.push(...sent.map((c) => c.cx));
    }
    expect(picked).toEqual([0, 1, 2, 3, 4]);
  });
});

describe("ChunkStore", () => {
  test("届いたチャンクを持ち、同じ版は書き換えない", () => {
    const store = new ChunkStore();
    expect(store.receive([data(0, 0, 1, true), data(-1, 2, 0)])).toHaveLength(2);
    expect(store.get(0, 0)?.owned).toBe(true);
    expect(store.get(-1, 2)?.tiles).toHaveLength(CHUNK_AREA);
    expect(store.receive([data(0, 0, 1, true)])).toEqual([]);
    expect(store.receive([data(0, 0, 2, true)]).map((c) => c.version)).toEqual([2]);
    expect(store.size).toBe(2);
  });

  test("数が上限を超えたら、残すもの以外を捨てる", () => {
    const store = new ChunkStore();
    store.receive([data(0, 0, 1), data(1, 0, 1), data(2, 0, 1), data(3, 0, 1)]);
    const removed = store.evict((cx) => cx >= 2, 2);
    expect(removed.map((c) => c.cx)).toEqual([0, 1]);
    expect(store.size).toBe(2);
    expect(store.evict(() => false, 5)).toEqual([]);
  });

  test("最後に使ったのが古いものから捨てる", () => {
    const store = new ChunkStore();
    // 開始地点のチャンク 0 が最初に届き、そのあと遠くを見た
    store.receive([data(0, 0, 1), data(1, 0, 1), data(2, 0, 1), data(3, 0, 1)]);
    // 開始地点に戻ってきた
    store.touch([[0, 0]]);
    // 同じ版を受け取り直したチャンクも、使ったものとして扱う
    store.receive([data(1, 0, 1)]);
    expect(store.evict(() => false, 2).map((c) => c.cx)).toEqual([2, 3]);
    expect(store.get(0, 0)).toBeDefined();
    expect(store.get(1, 0)).toBeDefined();
  });

  test("捨てたチャンクを Worker に伝えると、また見えたときに届き直す", () => {
    const store = new ChunkStore();
    const sync = new ChunkSync();
    const core = new Map([["0,0", 1]]);
    const of = (cx: number, cy: number) => core.get(`${cx},${cy}`) ?? 0;
    const round = () => {
      const picked = sync.collect(of, 10);
      store.receive(picked.map(({ cx, cy, version }) => data(cx, cy, version)));
      return picked.length;
    };
    sync.setView(at(0, 1), []);
    expect(round()).toBe(2);
    expect(round()).toBe(0);
    core.set("0,0", 2);
    expect(round()).toBe(1);
    expect(store.get(0, 0)?.version).toBe(2);
    const removed = store.evict((cx) => cx === 1, 1);
    sync.setView(at(1), removed);
    expect(round()).toBe(0);
    sync.setView(at(0, 1), []);
    expect(round()).toBe(1);
    expect(store.get(0, 0)?.version).toBe(2);
  });
});
