import { describe, expect, test } from "vitest";
import { type Lot, lotKey } from "./lots.ts";
import {
  filledSlots,
  PARKED_CAR_LENGTH,
  PARKED_CAR_WIDTH,
  ParkedCache,
  parkedColorIndex,
  parkedGeometry,
  parseIndoorParked,
  parseParked,
  slotsOf,
} from "./parked.ts";
import { roadDrawOrder, VERTEX_FLOATS } from "./roads.ts";

type Strip = Lot["strips"][number];

// 建物 (10, 20) の西の列は、枠が南北に 4 つ並び、車は東から入る。もう 1 本は北の帯で、枠が東西に 2 つ並び、南から入る
const LOT: Lot = {
  surface: 6,
  indoor: 0,
  faces: [],
  strips: [
    { x0: 10, y0: 20, x1: 10.625, y1: 21.25, openX: 1, openY: 0, count: 4 },
    { x0: 11, y0: 22, x1: 11.625, y1: 22.625, openX: 0, openY: 1, count: 2 },
  ],
};
const LOTS = new Map([[lotKey(10, 20), LOT]]);
const VIEW = { minX: 0, minY: 0, maxX: 40, maxY: 40 };
const only = (n: number) => new Map([[lotKey(10, 20), n]]);

/** 長方形（6 頂点）の中心と大きさ。 */
function boxes(g: ReturnType<typeof parkedGeometry>["cars"]) {
  const data = g.build();
  const out: { cx: number; cy: number; w: number; h: number }[] = [];
  for (let i = 0; i + 6 * VERTEX_FLOATS <= data.length; i += 6 * VERTEX_FLOATS) {
    const xs: number[] = [];
    const ys: number[] = [];
    for (let k = 0; k < 6; k++) {
      xs.push((data[i + k * VERTEX_FLOATS] ?? 0) + g.origin.x);
      ys.push((data[i + k * VERTEX_FLOATS + 1] ?? 0) + g.origin.y);
    }
    const [x0, x1] = [Math.min(...xs), Math.max(...xs)];
    const [y0, y1] = [Math.min(...ys), Math.max(...ys)];
    out.push({ cx: (x0 + x1) / 2, cy: (y0 + y1) / 2, w: x1 - x0, h: y1 - y0 });
  }
  return out;
}

describe("停めてある台数", () => {
  test("建物の左上のマスで引ける表に直す", () => {
    const m = parseParked(new Float64Array([10, 20, 3, 0, 30, 40, 1, 0]));
    expect(m.get("10,20")).toBe(3);
    expect(m.get("30,40")).toBe(1);
    expect(parseParked(new Float64Array([1, 2, 3])).size).toBe(0);
  });

  test("建物の中の台数は、1 台以上ある建物だけを引ける", () => {
    const m = parseIndoorParked(new Float64Array([10, 20, 9, 4, 30, 40, 1, 0]));
    expect(m.get("10,20")).toBe(4);
    expect(m.has("30,40")).toBe(false);
  });
});

describe("枠の埋め方", () => {
  test("枠は並びの順に先頭から詰める", () => {
    expect(filledSlots(LOT, 0)).toHaveLength(0);
    expect(filledSlots(LOT, 3).map((f) => f.index)).toEqual([0, 1, 2]);
    expect(filledSlots(LOT, 5).map((f) => f.index)).toEqual([0, 1, 2, 3, 4]);
  });

  test("平面の枠より多い台数は、建物の中にあるので枠の数でとどめる", () => {
    expect(filledSlots(LOT, 99)).toHaveLength(6);
  });

  test("枠の中心は、並びの長い辺に沿って等間隔に並び、車は入る側の反対へ鼻を向ける", () => {
    const slots = slotsOf(LOT.strips[0] as Strip);
    expect(slots.map((s) => s.cy)).toEqual([20.15625, 20.46875, 20.78125, 21.09375]);
    expect(slots.every((s) => s.alongX && s.cx === 10.3125)).toBe(true);
    expect(slots.every((s) => s.forward === -1)).toBe(true);
  });
});

describe("停まっている車の頂点", () => {
  test("停めてある台数ぶんの車が、それぞれの枠の中に描かれる", () => {
    for (const n of [1, 3, 6]) {
      const g = parkedGeometry(LOTS, only(n), VIEW, false);
      // 1 台は縁と面の 2 つの長方形
      const all = boxes(g.cars);
      expect(all).toHaveLength(n * 2);
      const slots = filledSlots(LOT, n);
      for (const [i, b] of all.entries()) {
        const { slot } = slots[Math.floor(i / 2)] as (typeof slots)[number];
        expect(b.cx).toBeCloseTo(slot.cx, 5);
        expect(b.cy).toBeCloseTo(slot.cy, 5);
        const s = LOT.strips.find(
          (t) => slot.cx >= t.x0 && slot.cx <= t.x1 && slot.cy >= t.y0 && slot.cy <= t.y1,
        ) as Strip;
        expect(b.cx - b.w / 2).toBeGreaterThanOrEqual(s.x0 - 1e-6);
        expect(b.cx + b.w / 2).toBeLessThanOrEqual(s.x1 + 1e-6);
        expect(b.cy - b.h / 2).toBeGreaterThanOrEqual(s.y0 - 1e-6);
        expect(b.cy + b.h / 2).toBeLessThanOrEqual(s.y1 + 1e-6);
      }
    }
  });

  test("車の長い辺は、車の入る側に沿う", () => {
    const all = boxes(parkedGeometry(LOTS, only(5), VIEW, false).cars);
    // 西の列（東から入る）は横長、北の帯（南から入る）は縦長
    expect(all[0]?.w).toBeCloseTo(PARKED_CAR_LENGTH, 5);
    expect(all[0]?.h).toBeCloseTo(PARKED_CAR_WIDTH, 5);
    expect(all[8]?.w).toBeCloseTo(PARKED_CAR_WIDTH, 5);
    expect(all[8]?.h).toBeCloseTo(PARKED_CAR_LENGTH, 5);
  });

  test("停めていない建物と、形のない建物には描かない", () => {
    const parked = new Map([
      [lotKey(10, 20), 0],
      [lotKey(50, 50), 4],
    ]);
    const g = parkedGeometry(LOTS, parked, VIEW, true);
    expect(g.cars.vertexCount).toBe(0);
    expect(g.fine.vertexCount).toBe(0);
  });

  test("見えている範囲の外の駐車場は作らない", () => {
    const far = { minX: 100, minY: 100, maxX: 120, maxY: 120 };
    expect(parkedGeometry(LOTS, only(4), far, true).cars.vertexCount).toBe(0);
  });

  test("台数が増えても、すでに停まっている枠の車の色と位置は変わらない", () => {
    const colors = (n: number) => Array.from({ length: n }, (_, i) => parkedColorIndex(10, 20, i));
    expect(colors(5).slice(0, 3)).toEqual(colors(3));
    const a = parkedGeometry(LOTS, only(3), VIEW, false).cars.build();
    const b = parkedGeometry(LOTS, only(5), VIEW, false).cars.build();
    expect(Array.from(b.subarray(0, a.length))).toEqual(Array.from(a));
  });

  test("窓と屋根は細部の段のときだけ作り、1 台に 3 つの長方形を足す", () => {
    const off = parkedGeometry(LOTS, only(4), VIEW, false);
    const on = parkedGeometry(LOTS, only(4), VIEW, true);
    expect(off.fine.vertexCount).toBe(0);
    expect(on.fine.vertexCount).toBe(4 * 3 * 6);
    expect(on.cars.vertexCount).toBe(off.cars.vertexCount);
  });
});

describe("段ごとに描く・描かない", () => {
  const alpha = (zoom: number, name: string) =>
    roadDrawOrder(zoom).find(([n]) => n === name)?.[1] ?? 0;

  test("停まっている車は、車と人が現れる倍率 12〜16 で溶け込む", () => {
    expect(alpha(8, "parked")).toBe(0);
    expect(alpha(11.9, "parked")).toBe(0);
    expect(alpha(14, "parked")).toBeGreaterThan(0);
    expect(alpha(14, "parked")).toBeLessThan(1);
    expect(alpha(16, "parked")).toBe(1);
    expect(alpha(14, "parked")).toBe(alpha(14, "lotLines"));
  });

  test("窓と屋根は細部の段（32〜48）でだけ出る", () => {
    expect(alpha(16, "parkedFine")).toBe(0);
    expect(alpha(32, "parkedFine")).toBe(0);
    expect(alpha(40, "parkedFine")).toBeGreaterThan(0);
    expect(alpha(48, "parkedFine")).toBe(1);
  });
});

describe("ParkedCache", () => {
  const fresh = () => {
    const c = new ParkedCache();
    c.setLots(LOTS);
    c.setParked(only(4));
    return c;
  };

  test("寄りの段より引いているあいだは作らない", () => {
    const c = fresh();
    expect(c.update(8, VIEW)).toBeNull();
    expect(c.builds).toBe(0);
    expect(c.update(16, VIEW)?.cars.vertexCount).toBe(4 * 2 * 6);
  });

  test("何も変わらないあいだは作り直さず、台数が変わると作り直す", () => {
    const c = fresh();
    expect(c.update(16, VIEW)).not.toBeNull();
    expect(c.update(16, VIEW)).toBeNull();
    expect(c.builds).toBe(1);
    c.setParked(only(2));
    expect(c.update(16, VIEW)?.cars.vertexCount).toBe(2 * 2 * 6);
    expect(c.builds).toBe(2);
  });

  test("作った範囲の外の台数だけが変わっても、作り直さない", () => {
    const lots = new Map([
      [lotKey(10, 20), LOT],
      [
        lotKey(200, 200),
        {
          ...LOT,
          strips: [{ x0: 200, y0: 200, x1: 200.6, y1: 201, openX: 1, openY: 0, count: 3 }],
        },
      ],
    ]);
    const c = new ParkedCache();
    c.setLots(lots);
    c.setParked(
      new Map([
        [lotKey(10, 20), 4],
        [lotKey(200, 200), 1],
      ]),
    );
    expect(c.update(16, VIEW)).not.toBeNull();
    c.setParked(
      new Map([
        [lotKey(10, 20), 4],
        [lotKey(200, 200), 3],
      ]),
    );
    expect(c.update(16, VIEW)).toBeNull();
    c.setParked(new Map([[lotKey(10, 20), 4]]));
    expect(c.update(16, VIEW)).toBeNull();
    c.setParked(new Map([[lotKey(10, 20), 3]]));
    expect(c.update(16, VIEW)?.cars.vertexCount).toBe(3 * 2 * 6);
  });

  test("細部の段に出入りすると窓と屋根を作り直し、駐車場の形が変わると作り直す", () => {
    const c = fresh();
    expect(c.update(16, VIEW)?.fine.vertexCount).toBe(0);
    expect(c.update(40, VIEW)?.fine.vertexCount).toBe(4 * 3 * 6);
    expect(c.update(40, VIEW)).toBeNull();
    expect(c.update(16, VIEW)?.fine.vertexCount).toBe(0);
    c.setLots(new Map());
    expect(c.update(16, VIEW)?.cars.vertexCount).toBe(0);
  });

  test("画面が作った範囲を出たら作り直す", () => {
    const c = fresh();
    c.update(16, { minX: 0, minY: 0, maxX: 20, maxY: 20 });
    expect(c.update(16, { minX: 2, minY: 2, maxX: 18, maxY: 18 })).toBeNull();
    expect(c.update(16, { minX: 30, minY: 30, maxX: 40, maxY: 40 })).not.toBeNull();
  });
});
