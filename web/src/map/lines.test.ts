import { describe, expect, test } from "vitest";
import { capZeroReason, lineRow } from "../line-panel.ts";
import {
  abandonPending,
  adoptCreated,
  clickStation,
  EMPTY_DRAFT,
  lineKey,
  setLineTrainsCommand,
  stationAt,
} from "./line-tool.ts";
import {
  LINE_COLORS,
  LINE_HEADER,
  LINE_MIN_PX,
  laneLayout,
  lineColorIndex,
  lineMeshes,
  lineName,
  MAX_LANES,
  moveMinutes,
  parseLines,
  TRACK_WIDTH,
} from "./lines.ts";

/** 駅 (0,0) と (30,0) を結ぶ路線の rail_lines の並び。 */
function encode(
  index: number,
  opts: {
    stopped?: number;
    trains?: number;
    cap?: number;
    shared?: number[][];
    waitPlaces?: number;
    turnCap?: number;
    /** 折り返せる本数を決めている終点の [ホームの数, 駅の x, y, 分け合う路線の数]。 */
    turnLimit?: [number, number, number, number];
    /** 上限を決めているものの [種類(0 なし、1 待てる所、2 区画の分け前), 区画の枠, 路線の分, 分け合う路線の数]。 */
    capLimit?: [number, number, number, number];
    /** 折り返せる本数を区画の分で抑えているときの [区画の枠, 路線の分, 分け合う路線の数]。 */
    turnZone?: [number, number, number];
  } = {},
) {
  const trains = opts.trains ?? 2;
  const cap = opts.cap ?? 3;
  const turnCap = opts.turnCap ?? 16;
  return [
    ...[
      index,
      0,
      0,
      trains,
      cap,
      Math.min(trains, cap, turnCap),
      opts.stopped ?? 0,
      100,
      600,
      2,
      1,
      opts.waitPlaces ?? 4,
      opts.shared?.length ?? 0,
      turnCap,
      ...(opts.turnLimit ?? [0, 0, 0, 0]),
      ...(opts.capLimit ?? [0, 0, 0, 0]),
      ...(opts.turnZone ? [1, ...opts.turnZone] : [0, 0, 0, 0]),
    ],
    ...[0, 0, 30, 0],
    ...[3, 5, 25, 5],
    ...(opts.shared ?? []).flat(),
  ];
}

describe("路線の一覧", () => {
  test("rail_lines の並びを読む", () => {
    const lines = parseLines([...encode(0), ...encode(9)]);
    expect(lines).toHaveLength(2);
    expect(lines[1]).toMatchObject({ index: 9, trains: 2, cap: 3, stopped: false });
    expect(lines[0]?.stations).toEqual([
      { x: 0, y: 0 },
      { x: 30, y: 0 },
    ]);
    expect(lines[0]?.tracks).toEqual([{ a: { x: 3, y: 5 }, b: { x: 25, y: 5 } }]);
    expect(encode(0).length).toBe(LINE_HEADER + 4 + 4);
  });

  test("途中で切れた並びは、読めたところまでにする", () => {
    expect(parseLines(encode(0).slice(0, -2))).toEqual([]);
  });

  test("色は添え字の順に 8 色を回し、名前は同じ色の何本目かを添える", () => {
    expect(LINE_COLORS).toHaveLength(8);
    expect(lineColorIndex(9)).toBe(1);
    expect(lineName(1)).toBe("緑線");
    expect(lineName(9)).toBe("緑線（2）");
  });

  test("運転間隔は移動の分で出す", () => {
    expect(moveMinutes(90)).toBe("1.5 分");
    expect(moveMinutes(1200)).toBe("20 分");
  });

  test("止まっている路線は描かず、同じ線路を通る路線は横に並べる", () => {
    const lines = parseLines([...encode(0), ...encode(1), ...encode(2, { stopped: 1 })]);
    const meshes = lineMeshes(lines);
    const one = lineMeshes(parseLines(encode(0)));
    expect(meshes.far.vertexCount).toBe(one.far.vertexCount * 2);
    expect(lineMeshes([]).far.vertexCount).toBe(0);
  });

  test("同じ線路を何本の路線が通っても、線は有限で線路の幅に収まる", () => {
    for (const n of [1, 2, 3, 4, 5, 8, 9, 10, 64]) {
      const { lanes, half, pitch } = laneLayout(n);
      expect(lanes).toBe(Math.min(n, MAX_LANES));
      expect(half).toBeGreaterThan(0);
      expect(Number.isFinite(pitch)).toBe(true);
      // 端の線の外側までの幅
      expect((lanes - 1) * pitch + 2 * half).toBeLessThanOrEqual(TRACK_WIDTH + 1e-9);
      // 隣の線と重ならない
      expect(pitch).toBeGreaterThan(2 * half);

      const lines = parseLines(Array.from({ length: n }, (_, i) => encode(i)).flat());
      const meshes = lineMeshes(lines);
      expect(meshes.far.vertexCount).toBe(6 * n);
      for (const g of [meshes.far, meshes.near]) {
        const data = g.build();
        expect(data.every(Number.isFinite)).toBe(true);
      }
      // 頂点の横のずれは、線の外側まで含めて線路の幅の半分に収まる
      const far = meshes.far.build();
      const y = 5 + 0.5;
      const stride = far.length / (6 * n);
      for (let v = 0; v < 6 * n; v++) {
        const off = (far[v * stride + 1] ?? 0) + meshes.far.origin.y - y;
        expect(Math.abs(off) + half).toBeLessThanOrEqual(TRACK_WIDTH / 2 + 1e-6);
      }
    }
    expect(LINE_MIN_PX).toBeGreaterThanOrEqual(1);
  });

  test("一覧の行に上限と止まっている印を出す", () => {
    const name = () => "港町駅";
    const [ok] = parseLines(encode(0));
    const [stopped] = parseLines(encode(0, { stopped: 1 }));
    const [zero] = parseLines(encode(0, { cap: 0 }));
    if (!ok || !stopped || !zero) throw new Error("読めない");
    expect(lineRow(ok, name)).toMatchObject({
      trains: "本数 2 / 上限 3",
      canAdd: true,
      warning: "",
    });
    expect(lineRow(stopped, name).warning).toContain("止まっている");
    expect(lineRow(zero, name).warning).toContain("上限が 0");
    expect(lineRow(zero, name).canAdd).toBe(false);
  });

  test("終点で折り返せる本数が上限より少なければ、上限と＋ボタンはその本数に合わせる", () => {
    const [turned] = parseLines(encode(0, { trains: 2, cap: 3, turnCap: 2 }));
    const [over] = parseLines(encode(0, { trains: 3, cap: 3, turnCap: 2 }));
    if (!turned || !over) throw new Error("読めない");
    expect(turned.turnCap).toBe(2);
    expect(lineRow(turned, () => "")).toMatchObject({
      trains: "本数 2 / 上限 2",
      canAdd: false,
      warning: "",
    });
    expect(lineRow(over, () => "").warning).toContain("終点の駅で折り返せるのは 2 本まで");
    expect(over.turnLimit).toBeNull();
  });

  test("走らせる本数を抑えている終点と、そのホームの数と、分け合う路線を出す", () => {
    const name = (s: { x: number; y: number }) => (s.x === 30 ? "東駅" : "西駅");
    const [one] = parseLines(
      encode(0, { trains: 3, cap: 5, turnCap: 2, turnLimit: [1, 30, 0, 1] }),
    );
    const [two] = parseLines(encode(0, { trains: 6, cap: 9, turnCap: 4, turnLimit: [2, 0, 0, 0] }));
    if (!one || !two) throw new Error("読めない");
    expect(one.turnLimit).toEqual({ platforms: 1, station: { x: 30, y: 0 }, sharedLines: 1 });
    const w1 = lineRow(one, name).warning;
    expect(w1).toContain("走らせるのは 2 本");
    expect(w1).toContain("終点の東駅で折り返せるのは 2 本まで");
    expect(w1).toContain("着いたホーム 1 つで折り返す");
    expect(w1).toContain("ほかの路線（1 本）と分け合っている");
    const w2 = lineRow(two, name).warning;
    expect(w2).toContain("終点の西駅で折り返せるのは 4 本まで");
    expect(w2).toContain("ホーム 2 つを交互に使っても足りない");
    expect(w2).not.toContain("分け合って");
  });

  test("上限で抑えているときは、待てる所の数か区画の分け前を理由に出す", () => {
    const [waits] = parseLines(
      encode(0, { trains: 5, cap: 3, waitPlaces: 4, capLimit: [1, 0, 0, 0] }),
    );
    const [zone] = parseLines(encode(0, { trains: 5, cap: 2, capLimit: [2, 3, 1, 2] }));
    const [free] = parseLines(encode(0, { trains: 5, cap: 5 }));
    if (!waits || !zone || !free) throw new Error("読めない");
    expect(waits.capLimit).toEqual({ kind: "waitPlaces" });
    expect(zone.capLimit).toEqual({ kind: "zone", room: 3, share: 1, sharedLines: 2 });
    expect(free.capLimit).toBeNull();
    const w1 = lineRow(waits, () => "").warning;
    expect(w1).toContain("走らせるのは 3 本");
    expect(w1).toContain("待てる所が 4 個なので、走らせられるのは 3 本まで");
    const w2 = lineRow(zone, () => "").warning;
    expect(w2).toContain("枠 3 を 3 本の路線で分け、この路線の分は 1");
    expect(w2).toContain("走らせられるのは 2 本まで");
  });

  test("区画の分け直しで折り返せる本数を抑えているが、終点の理由がないときは、分け前を出す", () => {
    const [line] = parseLines(encode(0, { trains: 6, cap: 5, turnCap: 2, turnZone: [4, 2, 1] }));
    if (!line) throw new Error("読めない");
    expect(line.turnLimit).toBeNull();
    expect(line.turnZone).toEqual({ room: 4, share: 2, sharedLines: 1 });
    const w = lineRow(line, () => "").warning;
    expect(w).toContain("枠 4 を 2 本の路線で分け、この路線の分は 2");
    expect(w).toContain("走らせるのは 2 本まで");
  });

  test("ほかの路線が折り返すホームを通り抜けて抑えられているときは、終点と言わない", () => {
    const [through] = parseLines(
      encode(0, { trains: 7, cap: 9, turnCap: 4, turnLimit: [1, 15, 0, 1] }),
    );
    if (!through) throw new Error("読めない");
    const w = lineRow(through, () => "中駅").warning;
    expect(w).toContain("中駅でほかの路線が折り返すホームを通るので、走らせるのは 4 本まで");
    expect(w).toContain("同じホームを使うほかの路線（1 本）と分け合っている");
    expect(w).not.toContain("終点");
  });

  test("上限が 0 の路線と 0 でない路線が混ざった並びを続けて読む", () => {
    const lines = parseLines([
      ...encode(0),
      ...encode(1, { cap: 0, shared: [[0, 0]] }),
      ...encode(2),
      ...encode(3, {
        cap: 0,
        shared: [
          [0, 0],
          [2, 0],
        ],
      }),
    ]);
    expect(lines.map((l) => l.index)).toEqual([0, 1, 2, 3]);
    expect(lines.map((l) => l.sharedWith.length)).toEqual([0, 1, 0, 2]);
    expect(lines[2]?.tracks).toHaveLength(1);
    expect(lines[3]?.sharedWith[1]).toEqual({ index: 2, generation: 0 });
  });

  test("上限が 0 の理由に、共有している路線の名前を出す", () => {
    const [shared] = parseLines(
      encode(2, {
        cap: 0,
        shared: [
          [0, 0],
          [9, 0],
        ],
      }),
    );
    const [few] = parseLines(encode(2, { cap: 0, waitPlaces: 1 }));
    if (!shared || !few) throw new Error("読めない");
    expect(shared.sharedWith).toEqual([
      { index: 0, generation: 0 },
      { index: 9, generation: 0 },
    ]);
    expect(capZeroReason(shared)).toContain("青線、緑線（2）と進路を共有");
    expect(capZeroReason(shared)).toContain("枠が回ってこない");
    expect(lineRow(shared, () => "").warning).toContain("青線");
    expect(capZeroReason(few)).toContain("待てる所が足りない");
  });
});

describe("路線の道具", () => {
  const a = { x: 3, y: 7 };
  const b = { x: 53, y: 7 };

  test("駅を押す順に、最初の駅を覚え、2 つ目で路線を作る", () => {
    const one = clickStation(EMPTY_DRAFT, a);
    expect(one.command).toBeNull();
    const two = clickStation(one.draft, b);
    expect(two.command).toEqual({ kind: "CreateLine", fromX: 3, fromY: 7, toX: 53, toY: 7 });
    expect(two.draft.pending).toEqual({ first: a, second: b });
    // 作っているあいだの押しは受け付けない
    expect(clickStation(two.draft, a).command).toBeNull();
  });

  test("同じ駅をもう一度押すと選びなおし、駅でない所は何もしない", () => {
    const one = clickStation(EMPTY_DRAFT, a).draft;
    expect(clickStation(one, a).draft).toEqual(EMPTY_DRAFT);
    expect(clickStation(one, null)).toEqual({ command: null, draft: one });
  });

  test("できた路線を延ばす路線にし、以後の駅は足す", () => {
    const pending = clickStation(clickStation(EMPTY_DRAFT, a).draft, b).draft;
    const made = parseLines(encode(4));
    const [line] = made;
    if (!line) throw new Error("読めない");
    line.stations = [a, b];
    // 前から知っていた路線は、延ばす路線に選ばない
    expect(adoptCreated(pending, made, new Set([lineKey(line)]))).toBe(pending);
    const next = adoptCreated(pending, made, new Set());
    expect(next.extend).toEqual({ index: 4, generation: 0 });
    const c = { x: 80, y: 7 };
    expect(clickStation(next, c).command).toEqual({
      kind: "ExtendLine",
      lineIndex: 4,
      lineGeneration: 0,
      atX: 80,
      atY: 7,
    });
  });

  test("駅を挿すときは、路線の駅で位置を選び、路線にない駅を押して挿す", () => {
    const line = { index: 2, generation: 1 };
    const stations = [a, b];
    const c = { x: 80, y: 7 };
    const start = { ...EMPTY_DRAFT, insert: { line, after: null } };
    // 位置を選ぶ前に路線にない駅を押しても何もしない
    expect(clickStation(start, c, stations)).toEqual({ command: null, draft: start });
    const chosen = clickStation(start, a, stations);
    expect(chosen.command).toBeNull();
    expect(chosen.draft.insert).toEqual({ line, after: 0 });
    const done = clickStation(chosen.draft, c, stations);
    expect(done.command).toEqual({
      kind: "InsertLineStation",
      lineIndex: 2,
      lineGeneration: 1,
      afterIndex: 0,
      atX: 80,
      atY: 7,
    });
    // 挿したあとは位置を選びなおす
    expect(done.draft.insert).toEqual({ line, after: null });
  });

  test("路線を作れなかったら最初の駅を選んだ状態に戻す", () => {
    const pending = clickStation(clickStation(EMPTY_DRAFT, a).draft, b).draft;
    expect(abandonPending(pending)).toEqual({
      first: a,
      pending: null,
      extend: null,
      insert: null,
    });
  });

  test("本数は 0〜16 に収める", () => {
    const ref = { index: 1, generation: 2 };
    expect(setLineTrainsCommand(ref, 99)).toMatchObject({ trains: 16 });
    expect(setLineTrainsCommand(ref, -1)).toMatchObject({ trains: 0 });
  });

  test("敷地のどのマスでも駅を指せる", () => {
    const st = { min: { x: 3, y: 7 }, max: { x: 12, y: 9 }, toward: { x: 0, y: 1 } };
    expect(stationAt([st], { x: 12, y: 9 })).toBe(st);
    expect(stationAt([st], { x: 13, y: 9 })).toBeNull();
  });
});
