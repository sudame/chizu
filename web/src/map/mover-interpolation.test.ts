import { describe, expect, it } from "vitest";
import { CHUNK_SIDE } from "../protocol/messages.ts";
import { layout } from "../protocol/snapshot.ts";
import { type MoverFrame, MoverInterpolator, TICKS_PER_SECOND } from "./mover-interpolation.ts";

type Spec = { id: number; x: number; y: number };

function frame(
  tick: number,
  movers: Spec[],
  opts: { speed?: number; paused?: boolean; baseCx?: number; baseCy?: number } = {},
): { frame: MoverFrame; out: Float32Array } {
  const words = new Uint32Array(movers.length * layout.moverWords);
  const floats = new Float32Array(words.buffer);
  movers.forEach((m, i) => {
    const at = i * layout.moverWords;
    floats[at + layout.mover.x] = m.x;
    floats[at + layout.mover.y] = m.y;
    words[at + layout.mover.id] = m.id;
  });
  return {
    frame: {
      tick,
      speed: opts.speed ?? 1,
      paused: opts.paused ?? false,
      baseCx: opts.baseCx ?? 0,
      baseCy: opts.baseCy ?? 0,
      count: movers.length,
      words,
      floats,
    },
    out: floats,
  };
}

function run(
  interp: MoverInterpolator,
  now: number,
  tick: number,
  movers: Spec[],
  opts?: Parameters<typeof frame>[2],
): Map<number, { x: number; y: number }> {
  const { frame: f, out } = frame(tick, movers, opts);
  interp.apply(f, now, out);
  const result = new Map<number, { x: number; y: number }>();
  movers.forEach((m, i) => {
    const at = i * layout.moverWords;
    result.set(m.id, {
      x: out[at + layout.mover.x] ?? Number.NaN,
      y: out[at + layout.mover.y] ?? Number.NaN,
    });
  });
  return result;
}

const TICK_MS = 1000 / TICKS_PER_SECOND;

// tick が進んだ最初の枠を時刻 0 で読んでから、時刻 `later` の枠を読む
function arrive(
  i: MoverInterpolator,
  tick: number,
  movers: Spec[],
  later: number,
  opts?: Parameters<typeof frame>[2],
): Map<number, { x: number; y: number }> {
  run(i, 0, tick, movers, opts);
  return run(i, later, tick, movers, opts);
}

describe("MoverInterpolator", () => {
  it("最初の枠はその場に描く", () => {
    const i = new MoverInterpolator();
    const r = run(i, 1000, 10, [{ id: 1, x: 5, y: 7 }]);
    expect(r.get(1)).toEqual({ x: 5, y: 7 });
  });

  it("tick が進むと、前の位置から今の位置へ時刻に合わせて動く", () => {
    const i = new MoverInterpolator();
    run(i, 1000, 10, [{ id: 1, x: 0, y: 0 }]);
    const at = (dt: number) => run(i, 1000 + dt, 11, [{ id: 1, x: 10, y: 20 }]).get(1);
    expect(at(0)).toEqual({ x: 0, y: 0 });
    expect(at(TICK_MS / 2)?.x).toBeCloseTo(5, 4);
    expect(at(TICK_MS / 2)?.y).toBeCloseTo(10, 4);
    // 次の tick が来るはずの時刻を過ぎても、今の位置で止まって行き過ぎない
    expect(at(TICK_MS * 3)).toEqual({ x: 10, y: 20 });
  });

  it("速度が上がると 1 tick の間隔が縮み、速くつながる", () => {
    const i = new MoverInterpolator();
    run(i, 0, 10, [{ id: 1, x: 0, y: 0 }], { speed: 8 });
    const r = arrive(i, 11, [{ id: 1, x: 8, y: 0 }], TICK_MS / 8 / 2, { speed: 8 });
    expect(r.get(1)?.x).toBeCloseTo(4, 4);
    // 0.25 倍では 4 倍の間隔をかけて動く
    const j = new MoverInterpolator();
    run(j, 0, 10, [{ id: 1, x: 0, y: 0 }], { speed: 0.25 });
    const s = arrive(j, 11, [{ id: 1, x: 8, y: 0 }], TICK_MS * 4 * 0.25, { speed: 0.25 });
    expect(s.get(1)?.x).toBeCloseTo(2, 4);
  });

  it("一時停止中は今の位置に置いて動かさない", () => {
    const i = new MoverInterpolator();
    run(i, 0, 10, [{ id: 1, x: 0, y: 0 }]);
    run(i, 1, 11, [{ id: 1, x: 10, y: 0 }]);
    const r = run(i, 2, 11, [{ id: 1, x: 10, y: 0 }], { paused: true });
    expect(r.get(1)).toEqual({ x: 10, y: 0 });
  });

  it("同じ tick の枠を何度読んでも、時刻だけで進む", () => {
    const i = new MoverInterpolator();
    run(i, 0, 10, [{ id: 1, x: 0, y: 0 }]);
    run(i, 100, 11, [{ id: 1, x: 10, y: 0 }]);
    const a = run(i, 100 + TICK_MS / 4, 11, [{ id: 1, x: 10, y: 0 }]).get(1)?.x ?? 0;
    const b = run(i, 100 + TICK_MS / 2, 11, [{ id: 1, x: 10, y: 0 }]).get(1)?.x ?? 0;
    expect(a).toBeCloseTo(2.5, 4);
    expect(b).toBeCloseTo(5, 4);
  });

  it("消えた移動体は描かれず、新しく出た移動体はその場に描く", () => {
    const i = new MoverInterpolator();
    run(i, 0, 10, [
      { id: 1, x: 0, y: 0 },
      { id: 2, x: 100, y: 0 },
    ]);
    const r = arrive(
      i,
      11,
      [
        { id: 2, x: 110, y: 0 },
        { id: 3, x: 50, y: 50 },
      ],
      TICK_MS / 2,
    );
    expect(r.has(1)).toBe(false);
    expect(r.get(2)?.x).toBeCloseTo(105, 4);
    expect(r.get(3)).toEqual({ x: 50, y: 50 });
  });

  it("並びが入れ替わっても ID で照合し、別の移動体と取り違えない", () => {
    const i = new MoverInterpolator();
    run(i, 0, 10, [
      { id: 1, x: 0, y: 0 },
      { id: 2, x: 100, y: 0 },
      { id: 3, x: 200, y: 0 },
    ]);
    // 上限で切られて 1 が落ち、並びも変わった
    const r = arrive(
      i,
      11,
      [
        { id: 3, x: 210, y: 0 },
        { id: 4, x: 300, y: 0 },
        { id: 2, x: 110, y: 0 },
      ],
      TICK_MS / 2,
    );
    expect(r.get(3)?.x).toBeCloseTo(205, 4);
    expect(r.get(2)?.x).toBeCloseTo(105, 4);
    expect(r.get(4)?.x).toBe(300);
  });

  it("同じ tick のうちに並びが変わっても、前の位置を保つ", () => {
    const i = new MoverInterpolator();
    run(i, 0, 10, [
      { id: 1, x: 0, y: 0 },
      { id: 2, x: 100, y: 0 },
    ]);
    run(i, 0, 11, [
      { id: 1, x: 10, y: 0 },
      { id: 2, x: 110, y: 0 },
    ]);
    const r = run(i, TICK_MS / 2, 11, [
      { id: 2, x: 110, y: 0 },
      { id: 1, x: 10, y: 0 },
    ]);
    expect(r.get(1)?.x).toBeCloseTo(5, 4);
    expect(r.get(2)?.x).toBeCloseTo(105, 4);
  });

  it("基準のチャンクが変わっても、ワールドの位置で補間する", () => {
    const i = new MoverInterpolator();
    run(i, 0, 10, [{ id: 1, x: 10, y: 0 }], { baseCx: 0 });
    // 基準が 1 チャンク東へ動いた。ワールドの x は 10 から CHUNK_SIDE + 2 - CHUNK_SIDE ではなく 2 + CHUNK_SIDE
    const r = arrive(i, 11, [{ id: 1, x: 2, y: 0 }], TICK_MS / 2, { baseCx: 1 });
    const world = (10 + (CHUNK_SIDE + 2)) / 2;
    expect((r.get(1)?.x ?? 0) + CHUNK_SIDE).toBeCloseTo(world, 3);
  });

  it("tick が巻き戻る、または長く空くと補間せずその場に置く", () => {
    const i = new MoverInterpolator();
    run(i, 0, 100, [{ id: 1, x: 0, y: 0 }]);
    expect(run(i, 1, 5, [{ id: 1, x: 50, y: 0 }]).get(1)?.x).toBe(50);
    expect(run(i, 2, 5 + TICKS_PER_SECOND, [{ id: 1, x: 80, y: 0 }]).get(1)?.x).toBe(80);
  });

  it("次の tick が区間の終わり頃に届くと、区間をつなげて途切れない", () => {
    const i = new MoverInterpolator();
    run(i, 0, 10, [{ id: 1, x: 0, y: 0 }]);
    run(i, 0, 11, [{ id: 1, x: 10, y: 0 }]);
    // 区間の終わり（TICK_MS）の少し後に次の tick が届いても、割合は 0 からではなくつながった値から始まる
    const r = run(i, TICK_MS + 5, 12, [{ id: 1, x: 20, y: 0 }]);
    expect(r.get(1)?.x).toBeGreaterThan(10);
  });
});
