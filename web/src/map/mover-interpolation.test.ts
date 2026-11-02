import { describe, expect, it } from "vitest";
import { CHUNK_SIDE } from "../protocol/messages.ts";
import { layout } from "../protocol/snapshot.ts";
import { type MoverFrame, MoverInterpolator, TICKS_PER_SECOND } from "./mover-interpolation.ts";

type Spec = { id: number; x: number; y: number; generation?: number; kind?: number };

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
    words[at + layout.mover.generation] = m.generation ?? 0;
    words[at + layout.mover.headingKind] = (m.kind ?? 2) << 16;
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

  it("速度の値ではなく、tick が届いた時刻の差から区間を測る", () => {
    const i = new MoverInterpolator();
    // header.speed は 1 のまま、Worker が間引いて 133ms ごとに tick が届く（#102 の 0.25 倍）
    for (let k = 0; k < 6; k++) {
      run(i, k * 133, 10 + k, [{ id: 1, x: k * 10, y: 0 }]);
    }
    const mid = run(i, 5 * 133 + 133 / 2, 15, [{ id: 1, x: 50, y: 0 }]).get(1)?.x ?? 0;
    expect(mid).toBeGreaterThan(44);
    expect(mid).toBeLessThan(46);
  });

  it("速度が切り替わったら、測り直して追いつく", () => {
    const i = new MoverInterpolator();
    for (let k = 0; k < 6; k++) {
      run(i, k * 33.3, 10 + k, [{ id: 1, x: k * 10, y: 0 }]);
    }
    let t = 5 * 33.3;
    let x = 50;
    for (let k = 6; k < 14; k++) {
      t += 133;
      x += 10;
      run(i, t, 10 + k, [{ id: 1, x, y: 0 }]);
    }
    const mid = run(i, t + 66.5, 23, [{ id: 1, x, y: 0 }]).get(1)?.x ?? 0;
    expect(mid).toBeGreaterThan(x - 6);
    expect(mid).toBeLessThan(x - 4);
  });

  // 1 フレーム 16.7ms の画面で、tick が interval ごとに届くとき、位置が止まらず跳ばずに進むか
  describe.each([
    ["0.25 倍（133ms ごと）", 133],
    ["0.5 倍（67ms ごと）", 67],
    ["1 倍（33ms ごと）", 33.3],
    ["間隔の揺れる 1 倍", 33.3],
  ])("%s", (name, interval) => {
    it("位置が止まらず、跳ばずに進む", () => {
      const frameMs = 1000 / 60;
      const jitter = name.startsWith("間隔の揺れる");
      // 到着はフレームの時刻に切り上がる。揺れるときは到着を最大 12ms ずらす
      const arrivals: number[] = [];
      for (let k = 0; k < 60; k++) {
        const wobble = jitter ? ((k * 7) % 5) * 3 : 0;
        arrivals.push(k * interval + wobble);
      }
      const i = new MoverInterpolator();
      const xs: number[] = [];
      for (let f = 0; f < 100; f++) {
        const now = f * frameMs;
        const tick = arrivals.filter((a) => a <= now).length;
        xs.push(run(i, now, tick, [{ id: 1, x: tick * 10, y: 0 }]).get(1)?.x ?? 0);
      }
      const steps = xs.slice(1).map((x, k) => x - (xs[k] ?? 0));
      const warm = steps.slice(40);
      const ideal = (10 * frameMs) / interval;
      expect(Math.min(...warm)).toBeGreaterThanOrEqual(0);
      expect(Math.max(...warm)).toBeLessThan(ideal * 2.5);
      // 止まるフレームが続かない
      let run0 = 0;
      let longest = 0;
      for (const d of warm) {
        run0 = d < ideal * 0.2 ? run0 + 1 : 0;
        longest = Math.max(longest, run0);
      }
      expect(longest).toBeLessThanOrEqual(1);
    });
  });

  it("世代の違う移動は、添え字が同じでも補間でつながない", () => {
    const i = new MoverInterpolator();
    run(i, 0, 10, [{ id: 5, generation: 1, x: 100, y: 0 }]);
    const r = arrive(i, 11, [{ id: 5, generation: 2, x: 0, y: 0 }], TICK_MS / 2);
    expect(r.get(5)?.x).toBe(0);
  });

  it("種類の違う移動は、添え字と世代が同じでも補間でつながない", () => {
    const i = new MoverInterpolator();
    run(i, 0, 10, [{ id: 5, kind: 1, x: 100, y: 0 }]);
    const r = arrive(i, 11, [{ id: 5, kind: 2, x: 0, y: 0 }], TICK_MS / 2);
    expect(r.get(5)?.x).toBe(0);
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
    run(i, TICK_MS, 11, [{ id: 1, x: 10, y: 0 }]);
    const a = run(i, TICK_MS + TICK_MS / 4, 11, [{ id: 1, x: 10, y: 0 }]).get(1)?.x ?? 0;
    const b = run(i, TICK_MS + TICK_MS / 2, 11, [{ id: 1, x: 10, y: 0 }]).get(1)?.x ?? 0;
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
});
