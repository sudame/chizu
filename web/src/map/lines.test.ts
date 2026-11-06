import { describe, expect, test } from "vitest";
import { lineRow } from "../line-panel.ts";
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
  lineColorIndex,
  lineMeshes,
  lineName,
  moveMinutes,
  parseLines,
} from "./lines.ts";

/** 駅 (0,0) と (30,0) を結ぶ路線の rail_lines の並び。 */
function encode(index: number, opts: { stopped?: number; trains?: number; cap?: number } = {}) {
  const trains = opts.trains ?? 2;
  const cap = opts.cap ?? 3;
  return [
    ...[index, 0, 0, trains, cap, Math.min(trains, cap), opts.stopped ?? 0, 100, 600, 2, 1],
    ...[0, 0, 30, 0],
    ...[3, 5, 25, 5],
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

  test("路線を作れなかったら最初の駅を選んだ状態に戻す", () => {
    const pending = clickStation(clickStation(EMPTY_DRAFT, a).draft, b).draft;
    expect(abandonPending(pending)).toEqual({ first: a, pending: null, extend: null });
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
