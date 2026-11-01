import { describe, expect, test } from "vitest";
import { CHUNK_AREA, type ChunkData } from "../protocol/messages.ts";
import { ChunkStore, ChunkSync } from "./chunk-sync.ts";

function data(cx: number, cy: number, version: number, owned = false): ChunkData {
  return { cx, cy, version, owned, tiles: new ArrayBuffer(CHUNK_AREA) };
}

describe("ChunkSync", () => {
  const versions = new Map([
    ["0,0", 3],
    ["1,0", 0],
    ["2,0", 5],
  ]);
  const versionOf = (cx: number, cy: number) => versions.get(`${cx},${cy}`) ?? 0;

  test("手元と版の違うチャンクだけを選び、一度送ったものは選ばない", () => {
    const sync = new ChunkSync();
    sync.setView([
      { cx: 0, cy: 0, version: 3 },
      { cx: 1, cy: 0, version: null },
      { cx: 2, cy: 0, version: 4 },
    ]);
    expect(sync.collect(versionOf, 10)).toEqual([
      { cx: 1, cy: 0, version: 0 },
      { cx: 2, cy: 0, version: 5 },
    ]);
    expect(sync.collect(versionOf, 10)).toEqual([]);
  });

  test("中核の側で版が上がったら送り直す", () => {
    const local = new Map(versions);
    const sync = new ChunkSync();
    sync.setView([{ cx: 0, cy: 0, version: null }]);
    const of = (cx: number, cy: number) => local.get(`${cx},${cy}`) ?? 0;
    expect(sync.collect(of, 10)).toHaveLength(1);
    local.set("0,0", 4);
    expect(sync.collect(of, 10)).toEqual([{ cx: 0, cy: 0, version: 4 }]);
  });

  test("一度に選ぶ数を limit で抑え、残りは次に回す", () => {
    const sync = new ChunkSync();
    sync.setView([
      { cx: 0, cy: 0, version: null },
      { cx: 1, cy: 0, version: null },
      { cx: 2, cy: 0, version: null },
    ]);
    expect(sync.collect(versionOf, 2).map((c) => c.cx)).toEqual([0, 1]);
    expect(sync.collect(versionOf, 2).map((c) => c.cx)).toEqual([2]);
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

  test("手元の版を渡した順で伝え、ないものは null にする", () => {
    const store = new ChunkStore();
    store.receive([data(3, 4, 7)]);
    expect(
      store.known([
        [5, 5],
        [3, 4],
      ]),
    ).toEqual([
      { cx: 5, cy: 5, version: null },
      { cx: 3, cy: 4, version: 7 },
    ]);
  });

  test("数が上限を超えたら、残すもの以外を捨てる", () => {
    const store = new ChunkStore();
    store.receive([data(0, 0, 1), data(1, 0, 1), data(2, 0, 1), data(3, 0, 1)]);
    const removed = store.evict((cx) => cx >= 2, 2);
    expect(removed.map((c) => c.cx)).toEqual([0, 1]);
    expect(store.size).toBe(2);
    expect(store.evict(() => false, 5)).toEqual([]);
  });

  test("送り直したチャンクを受け取ると、Worker の側も手元の版に追いつく", () => {
    const store = new ChunkStore();
    const sync = new ChunkSync();
    const core = new Map([["0,0", 1]]);
    const of = (cx: number, cy: number) => core.get(`${cx},${cy}`) ?? 0;
    const round = () => {
      const picked = sync.collect(of, 10);
      store.receive(picked.map(({ cx, cy, version }) => data(cx, cy, version)));
      return picked.length;
    };
    sync.setView(store.known([[0, 0]]));
    expect(round()).toBe(1);
    expect(round()).toBe(0);
    core.set("0,0", 2);
    expect(round()).toBe(1);
    sync.setView(store.known([[0, 0]]));
    expect(round()).toBe(0);
    expect(store.get(0, 0)?.version).toBe(2);
  });
});
