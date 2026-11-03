import { describe, expect, it } from "vitest";
import { CHUNK_SIDE } from "../protocol/messages.ts";
import { layout } from "../protocol/snapshot.ts";
import {
  angleCode,
  hasMoverPaths,
  type MoverFrame,
  MoverInterpolator,
  MoverPaths,
  pathAt,
  TICKS_PER_SECOND,
  turnHeading,
} from "./mover-interpolation.ts";

type Spec = {
  id: number;
  x: number;
  y: number;
  generation?: number;
  kind?: number;
  heading?: number;
  path?: number;
  via?: number;
  frac?: number;
};

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
    words[at + layout.mover.headingKind] = (((m.kind ?? 2) << 16) | (m.heading ?? 0)) >>> 0;
    words[at + layout.mover.path] = m.path ?? 0;
    words[at + layout.mover.via] = m.via ?? 0;
    floats[at + layout.mover.frac] = m.frac ?? 0;
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
  paths?: MoverPaths,
): Map<number, { x: number; y: number; heading: number }> {
  const { frame: f, out } = frame(tick, movers, opts);
  interp.apply(f, now, out, paths);
  const words = new Uint32Array(out.buffer);
  const result = new Map<number, { x: number; y: number; heading: number }>();
  movers.forEach((m, i) => {
    const at = i * layout.moverWords;
    result.set(m.id, {
      x: out[at + layout.mover.x] ?? Number.NaN,
      y: out[at + layout.mover.y] ?? Number.NaN,
      heading: (words[at + layout.mover.headingKind] ?? 0) & 0xffff,
    });
  });
  return result;
}

/** 位置だけを取り出す。 */
function xy(p: { x: number; y: number } | undefined): { x: number; y: number } | undefined {
  return p === undefined ? undefined : { x: p.x, y: p.y };
}

const TICK_MS = 1000 / TICKS_PER_SECOND;

// tick が進んだ最初の枠を時刻 0 で読んでから、時刻 `later` の枠を読む
function arrive(
  i: MoverInterpolator,
  tick: number,
  movers: Spec[],
  later: number,
  opts?: Parameters<typeof frame>[2],
  paths?: MoverPaths,
): Map<number, { x: number; y: number; heading: number }> {
  run(i, 0, tick, movers, opts, paths);
  return run(i, later, tick, movers, opts, paths);
}

describe("MoverInterpolator", () => {
  it("最初の枠はその場に描く", () => {
    const i = new MoverInterpolator();
    const r = run(i, 1000, 10, [{ id: 1, x: 5, y: 7 }]);
    expect(xy(r.get(1))).toEqual({ x: 5, y: 7 });
  });

  it("tick が進むと、前の位置から今の位置へ時刻に合わせて動く", () => {
    const i = new MoverInterpolator();
    run(i, 1000, 10, [{ id: 1, x: 0, y: 0 }]);
    const at = (dt: number) => xy(run(i, 1000 + dt, 11, [{ id: 1, x: 10, y: 20 }]).get(1));
    expect(at(0)).toEqual({ x: 0, y: 0 });
    expect(at(TICK_MS / 2)?.x).toBeCloseTo(5, 4);
    expect(at(TICK_MS / 2)?.y).toBeCloseTo(10, 4);
    // 次の tick が来るはずの時刻を過ぎても、今の位置で止まって行き過ぎない
    expect(at(TICK_MS * 3)).toEqual({ x: 10, y: 20 });
  });

  it("速度の値ではなく、tick が届いた時刻の差から区間を測る", () => {
    const i = new MoverInterpolator();
    // header.speed は 6 倍（公称 33ms）だが、重くて 133ms ごとにしか tick が届かない
    for (let k = 0; k < 6; k++) {
      run(i, k * 133, 10 + k, [{ id: 1, x: k * 10, y: 0 }], { speed: 6 });
    }
    const mid =
      run(i, 5 * 133 + 133 / 2, 15, [{ id: 1, x: 50, y: 0 }], { speed: 6 }).get(1)?.x ?? 0;
    expect(mid).toBeGreaterThan(44);
    expect(mid).toBeLessThan(46);
  });

  it("6 倍から 1 倍へ落としたら、測り直して追いつく", () => {
    const i = new MoverInterpolator();
    for (let k = 0; k < 6; k++) {
      run(i, k * 33.3, 10 + k, [{ id: 1, x: k * 10, y: 0 }], { speed: 6 });
    }
    let t = 5 * 33.3;
    let x = 50;
    for (let k = 6; k < 14; k++) {
      t += 200;
      x += 10;
      run(i, t, 10 + k, [{ id: 1, x, y: 0 }]);
    }
    const mid = run(i, t + 100, 23, [{ id: 1, x, y: 0 }]).get(1)?.x ?? 0;
    expect(mid).toBeGreaterThan(x - 6);
    expect(mid).toBeLessThan(x - 4);
  });

  // 1 フレーム 16.7ms の画面で、tick が interval ごとに届くとき、位置が止まらず跳ばずに進むか
  describe.each([
    ["1 倍（200ms ごと）", 200],
    ["間隔の揺れる 1 倍", 200],
    ["6 倍（33ms ごと）", 33.3],
    ["間隔の揺れる 6 倍", 33.3],
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
    expect(xy(r.get(1))).toEqual({ x: 10, y: 0 });
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
    expect(xy(r.get(3))).toEqual({ x: 50, y: 50 });
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

  it("向きも、前の tick の向きから近いほうへ回して補間する", () => {
    const i = new MoverInterpolator();
    const quarter = 16384;
    // 東（0）から北（3/4 周）へ曲がる。近いほうは 0 から負の向き
    run(i, 0, 10, [{ id: 1, x: 0, y: 0, heading: 0 }]);
    const { frame: f, out } = frame(11, [{ id: 1, x: 1, y: 0, heading: 3 * quarter }]);
    i.apply(f, 0, out);
    const { frame: g, out: half } = frame(11, [{ id: 1, x: 1, y: 0, heading: 3 * quarter }]);
    i.apply(g, TICK_MS / 2, half);
    const word = new Uint32Array(half.buffer)[layout.mover.headingKind] ?? 0;
    expect(word >>> 16).toBe(2);
    expect(word & 0xffff).toBe(4 * quarter - quarter / 2);
  });
});

describe("MoverInterpolator の折り返し", () => {
  it("補間の途中で次の tick が来ても、折り返した向きが残らない", () => {
    const i = new MoverInterpolator();
    const half = 32768;
    const headingAt = (now: number, tick: number, heading: number) => {
      const { frame: f, out } = frame(tick, [{ id: 1, x: tick, y: 0, heading }]);
      i.apply(f, now, out);
      return (new Uint32Array(out.buffer)[layout.mover.headingKind] ?? 0) & 0xffff;
    };
    headingAt(0, 10, 0);
    headingAt(TICK_MS, 11, 0);
    // 折り返した tick の補間の途中で、次の tick が届く
    expect(headingAt(TICK_MS * 2, 12, half)).toBe(half);
    expect(headingAt(TICK_MS * 2.3, 12, half)).toBe(half);
    expect(headingAt(TICK_MS * 2.5, 13, half)).toBe(half);
    expect(headingAt(TICK_MS * 2.6, 13, half)).toBe(half);
  });
});

describe("turnHeading", () => {
  const quarter = 16384;
  it("近いほうへ回す", () => {
    expect(turnHeading(0, quarter, 0.5)).toBe(quarter / 2);
    expect(turnHeading(0, 3 * quarter, 0.5)).toBe(65536 - quarter / 2);
    expect(turnHeading(3 * quarter, 0, 0.5)).toBe(65536 - quarter / 2);
    expect(turnHeading(100, 200, 0)).toBe(100);
    expect(turnHeading(100, 200, 1)).toBe(200);
  });

  it("折り返しは回さず、すぐに新しい向きにする", () => {
    expect(turnHeading(0, 2 * quarter, 0)).toBe(2 * quarter);
    expect(turnHeading(0, 2 * quarter, 0.5)).toBe(2 * quarter);
  });
});

/**
 * 道筋の並び（chizu-wasm の take_mover_paths の形）。点は [x, y, 向き] で、長さは点のあいだの距離を足す。
 * `dropped` は忘れさせる道筋の番号である。
 */
function pathData(
  reset: boolean,
  paths: [number, [number, number, number][]][],
  dropped: number[] = [],
): Float64Array {
  const out = [reset ? 1 : 0, dropped.length, ...dropped, paths.length];
  for (const [id, points] of paths) {
    out.push(id, points.length);
    let s = 0;
    points.forEach(([x, y, a], k) => {
      const prev = points[k - 1];
      if (prev) {
        s += Math.hypot(x - prev[0], y - prev[1]);
      }
      out.push(x, y, s, a);
    });
  }
  return new Float64Array(out);
}

const EAST = 0;
const SOUTH = Math.PI / 2;

describe("pathAt", () => {
  it("点のあいだは位置も向きもまっすぐ移し、両端の外は端の点にする", () => {
    const paths = new MoverPaths();
    paths.receive(
      pathData(true, [
        [
          3,
          [
            [0, 0, EAST],
            [1, 0, EAST],
            [1, 1, SOUTH],
          ],
        ],
      ]),
    );
    const p = paths.get(3);
    if (!p) {
      throw new Error("道筋がない");
    }
    expect(pathAt(p, 0.5)).toEqual({ x: 0.5, y: 0, angle: EAST });
    const q = pathAt(p, 1.5);
    expect(q.x).toBeCloseTo(1);
    expect(q.y).toBeCloseTo(0.5);
    expect(q.angle).toBeCloseTo(Math.PI / 4);
    expect(pathAt(p, -1)).toEqual({ x: 0, y: 0, angle: EAST });
    expect(pathAt(p, 9)).toEqual({ x: 1, y: 1, angle: SOUTH });
  });
});

describe("MoverPaths", () => {
  it("届いた道筋を足し、忘れさせる番号の道筋を忘れ、忘れる印があれば前の道筋をすべて忘れる", () => {
    const paths = new MoverPaths();
    paths.receive(
      pathData(true, [
        [
          1,
          [
            [0, 0, 0],
            [1, 0, 0],
          ],
        ],
      ]),
    );
    paths.receive(
      pathData(false, [
        [
          2,
          [
            [0, 0, 0],
            [0, 1, 0],
          ],
        ],
      ]),
    );
    expect(paths.size).toBe(2);
    paths.receive(pathData(false, [], [1]));
    expect(paths.get(1)).toBeUndefined();
    expect(paths.get(2)?.length).toBe(8);
    paths.receive(
      pathData(true, [
        [
          3,
          [
            [0, 0, 0],
            [2, 0, 0],
          ],
        ],
      ]),
    );
    expect(paths.get(1)).toBeUndefined();
    expect(paths.get(2)).toBeUndefined();
    expect(paths.get(3)?.length).toBe(8);
    expect(paths.get(0)).toBeUndefined();
  });
});

describe("MoverInterpolator の道筋に沿う補間", () => {
  // 東へ 1 マス進み、南へ曲がって 1 マス進む道筋。弦は (0, 0) から (1, 1) へ角を削る
  const corner = () => {
    const paths = new MoverPaths();
    paths.receive(
      pathData(true, [
        [
          7,
          [
            [0, 0, EAST],
            [1, 0, EAST],
            [1, 1, SOUTH],
          ],
        ],
      ]),
    );
    return paths;
  };

  it("前の tick と今の tick が同じ道筋の上なら、道筋に沿って長さに比例して動かす", () => {
    const paths = corner();
    const i = new MoverInterpolator();
    run(i, 0, 10, [{ id: 1, x: 0, y: 0, path: 7, frac: 0 }], {}, paths);
    const mid = arrive(i, 11, [{ id: 1, x: 1, y: 1, path: 7, frac: 1 }], TICK_MS / 2, {}, paths);
    // 弦の真ん中 (0.5, 0.5) ではなく、道筋の角にいる
    expect(mid.get(1)?.x).toBeCloseTo(1, 4);
    expect(mid.get(1)?.y).toBeCloseTo(0, 4);
    const quarter = run(
      i,
      TICK_MS * 0.75,
      11,
      [{ id: 1, x: 1, y: 1, path: 7, frac: 1 }],
      {},
      paths,
    );
    expect(quarter.get(1)?.x).toBeCloseTo(1, 4);
    expect(quarter.get(1)?.y).toBeCloseTo(0.5, 4);
    // 向きは道筋の向き。東から南へ半分回っている
    expect(quarter.get(1)?.heading).toBe(angleCode(Math.PI / 4));
  });

  it("前の道筋の終わりと今の道筋の始まりがつながっていれば、2 つをつないでたどる", () => {
    const paths = new MoverPaths();
    paths.receive(
      pathData(true, [
        [
          1,
          [
            [0, 0, EAST],
            [1, 0, EAST],
          ],
        ],
        [
          2,
          [
            [1, 0, SOUTH],
            [1, 1, SOUTH],
          ],
        ],
      ]),
    );
    const i = new MoverInterpolator();
    run(i, 0, 10, [{ id: 1, x: 0.5, y: 0, path: 1, frac: 0.5 }], {}, paths);
    const r = arrive(i, 11, [{ id: 1, x: 1, y: 0.5, path: 2, frac: 0.5 }], TICK_MS / 2, {}, paths);
    expect(r.get(1)?.x).toBeCloseTo(1, 4);
    expect(r.get(1)?.y).toBeCloseTo(0, 4);
  });

  it("あいだの道筋を丸ごと通り抜けたなら、その道筋もたどる", () => {
    const paths = new MoverPaths();
    paths.receive(
      pathData(true, [
        [
          1,
          [
            [0, 0, EAST],
            [1, 0, EAST],
          ],
        ],
        [
          2,
          [
            [1, 0, SOUTH],
            [1, 1, SOUTH],
          ],
        ],
        [
          3,
          [
            [1, 1, EAST],
            [2, 1, EAST],
          ],
        ],
      ]),
    );
    const i = new MoverInterpolator();
    run(i, 0, 10, [{ id: 1, x: 0.5, y: 0, path: 1, frac: 0.5 }], {}, paths);
    const at = (t: number) =>
      run(i, TICK_MS * t, 11, [{ id: 1, x: 1.5, y: 1, path: 3, via: 2, frac: 0.5 }], {}, paths).get(
        1,
      );
    at(0);
    // 全部で 0.5 + 1 + 0.5 = 2 マス。半分の所は、通り抜けた道筋の真ん中
    expect(at(0.5)?.x).toBeCloseTo(1, 4);
    expect(at(0.5)?.y).toBeCloseTo(0.5, 4);
    expect(at(0.5)?.heading).toBe(angleCode(SOUTH));
  });

  it("つながらない道筋や、知らない道筋のあいだは、まっすぐ動かす", () => {
    const paths = new MoverPaths();
    paths.receive(
      pathData(true, [
        [
          1,
          [
            [0, 0, EAST],
            [1, 0, EAST],
          ],
        ],
        [
          2,
          [
            [5, 5, SOUTH],
            [5, 6, SOUTH],
          ],
        ],
      ]),
    );
    const i = new MoverInterpolator();
    run(i, 0, 10, [{ id: 1, x: 0, y: 0, path: 1, frac: 0 }], {}, paths);
    const r = arrive(i, 11, [{ id: 1, x: 5, y: 5, path: 2, frac: 0 }], TICK_MS / 2, {}, paths);
    expect(r.get(1)?.x).toBeCloseTo(2.5, 4);
    expect(r.get(1)?.y).toBeCloseTo(2.5, 4);
    const j = new MoverInterpolator();
    run(j, 0, 10, [{ id: 1, x: 0, y: 0, path: 1, frac: 0 }], {}, paths);
    const u = arrive(j, 11, [{ id: 1, x: 1, y: 1, path: 99, frac: 1 }], TICK_MS / 2, {}, paths);
    expect(u.get(1)?.x).toBeCloseTo(0.5, 4);
    expect(u.get(1)?.y).toBeCloseTo(0.5, 4);
  });

  it("網が変わって前の道筋を忘れたら、前の道筋は使わない", () => {
    const paths = corner();
    const i = new MoverInterpolator();
    run(i, 0, 10, [{ id: 1, x: 0, y: 0, path: 7, frac: 0 }], {}, paths);
    run(i, 0, 11, [{ id: 1, x: 1, y: 1, path: 7, frac: 1 }], {}, paths);
    // 道筋 7 を忘れ、同じ所を通る別の番号の道筋だけが届いた
    paths.receive(
      pathData(true, [
        [
          8,
          [
            [0, 0, EAST],
            [0, 1, SOUTH],
            [1, 1, EAST],
          ],
        ],
      ]),
    );
    const r = run(i, TICK_MS / 2, 11, [{ id: 1, x: 1, y: 1, path: 7, frac: 1 }], {}, paths);
    expect(r.get(1)?.x).toBeCloseTo(0.5, 4);
    expect(r.get(1)?.y).toBeCloseTo(0.5, 4);
  });

  it("道筋なしの枠のあとに道筋つきの枠が続けば、2 つめの枠からは道筋に沿って動かす", () => {
    const paths = corner();
    const i = new MoverInterpolator();
    run(i, 0, 10, [{ id: 1, x: 0, y: 0 }], {}, paths);
    // 道筋 7 の s = 0.5 の所。前が道筋なしなので、この区間はまっすぐ動く
    run(i, TICK_MS, 11, [{ id: 1, x: 0.5, y: 0, path: 7, frac: 0.25 }], {}, paths);
    run(i, TICK_MS * 2, 11, [{ id: 1, x: 0.5, y: 0, path: 7, frac: 0.25 }], {}, paths);
    run(i, TICK_MS * 2, 12, [{ id: 1, x: 1, y: 1, path: 7, frac: 1 }], {}, paths);
    const r = run(i, TICK_MS * 2.5, 12, [{ id: 1, x: 1, y: 1, path: 7, frac: 1 }], {}, paths);
    // s は 0.5 から 2 へ。半分は 1.25 で、道筋の角を曲がった所。弦なら (0.75, 0.5)
    expect(r.get(1)?.x).toBeCloseTo(1, 4);
    expect(r.get(1)?.y).toBeCloseTo(0.25, 4);
  });

  it("道筋の知らせが枠より遅れて届いても、次の tick で道筋に戻る", () => {
    const paths = new MoverPaths();
    const i = new MoverInterpolator();
    run(i, 0, 10, [{ id: 1, x: 0, y: 0, path: 7, frac: 0 }], {}, paths);
    // 枠は道筋 7 を指すが、道筋はまだ届いていない
    run(i, TICK_MS, 11, [{ id: 1, x: 0.5, y: 0, path: 7, frac: 0.25 }], {}, paths);
    const late = run(
      i,
      TICK_MS * 1.5,
      11,
      [{ id: 1, x: 0.5, y: 0, path: 7, frac: 0.25 }],
      {},
      paths,
    );
    // 道筋がないので、前の位置 (0, 0) と今の位置 (0.5, 0) のあいだをまっすぐ動く
    expect(late.get(1)?.x).toBeCloseTo(0.25, 4);
    paths.receive(
      pathData(true, [
        [
          7,
          [
            [0, 0, EAST],
            [1, 0, EAST],
            [1, 1, SOUTH],
          ],
        ],
      ]),
    );
    run(i, TICK_MS * 2, 12, [{ id: 1, x: 1, y: 1, path: 7, frac: 1 }], {}, paths);
    const r = run(i, TICK_MS * 2.5, 12, [{ id: 1, x: 1, y: 1, path: 7, frac: 1 }], {}, paths);
    expect(r.get(1)?.x).toBeCloseTo(1, 4);
    expect(r.get(1)?.y).toBeCloseTo(0.25, 4);
  });

  it("道筋どうしがつながらない tick だけまっすぐ動かし、次の tick で道筋に戻る", () => {
    const paths = new MoverPaths();
    paths.receive(
      pathData(true, [
        [
          1,
          [
            [0, 0, EAST],
            [1, 0, EAST],
          ],
        ],
        [
          2,
          [
            [5, 5, SOUTH],
            [5, 7, SOUTH],
          ],
        ],
      ]),
    );
    const i = new MoverInterpolator();
    run(i, 0, 10, [{ id: 1, x: 0, y: 0, path: 1, frac: 0 }], {}, paths);
    run(i, TICK_MS, 11, [{ id: 1, x: 5, y: 5, path: 2, frac: 0 }], {}, paths);
    const straight = run(
      i,
      TICK_MS * 1.5,
      11,
      [{ id: 1, x: 5, y: 5, path: 2, frac: 0 }],
      {},
      paths,
    );
    expect(straight.get(1)?.x).toBeCloseTo(2.5, 4);
    run(i, TICK_MS * 2, 12, [{ id: 1, x: 5, y: 6, path: 2, frac: 0.5 }], {}, paths);
    const r = run(i, TICK_MS * 2.5, 12, [{ id: 1, x: 5, y: 6, path: 2, frac: 0.5 }], {}, paths);
    expect(r.get(1)?.x).toBeCloseTo(5, 4);
    expect(r.get(1)?.y).toBeCloseTo(5.5, 4);
  });

  it("補間の途中で次の tick が来ても、道筋の上の見えている所から続けて動かす", () => {
    const paths = corner();
    const i = new MoverInterpolator();
    run(i, 0, 10, [{ id: 1, x: 0, y: 0, path: 7, frac: 0 }], {}, paths);
    run(i, 0, 11, [{ id: 1, x: 1, y: 0, path: 7, frac: 0.5 }], {}, paths);
    run(i, TICK_MS, 11, [{ id: 1, x: 1, y: 0, path: 7, frac: 0.5 }], {}, paths);
    // 次の tick が、前の区間の半分の所で届く
    run(i, TICK_MS * 1.5, 12, [{ id: 1, x: 1, y: 1, path: 7, frac: 1 }], {}, paths);
    const r = run(i, TICK_MS * 2, 12, [{ id: 1, x: 1, y: 1, path: 7, frac: 1 }], {}, paths);
    expect(r.get(1)?.x).toBeCloseTo(1, 4);
    expect(r.get(1)?.y).toBeGreaterThan(0);
    expect(r.get(1)?.y).toBeLessThan(1);
  });
});

describe("hasMoverPaths", () => {
  it("忘れる番号がなくても、新しい道筋があれば送る", () => {
    // [すべて忘れるか, 忘れる数, 新しい数, 番号, 点の数, 点…]
    expect(hasMoverPaths(new Float64Array([0, 0, 1, 7, 0]))).toBe(true);
  });

  it("忘れる道筋があれば、新しい道筋がなくても送る", () => {
    expect(hasMoverPaths(new Float64Array([0, 2, 3, 4, 0]))).toBe(true);
  });

  it("すべて忘れるときは送る", () => {
    expect(hasMoverPaths(new Float64Array([1, 0, 0]))).toBe(true);
  });

  it("忘れるものも新しい道筋もないときは送らない", () => {
    expect(hasMoverPaths(new Float64Array([0, 0, 0]))).toBe(false);
  });
});
