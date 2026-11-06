import { describe, expect, test } from "vitest";
import { parseRoadProbe, RoadProbeGate, roadProbeText } from "./road-probe.ts";

describe("道路の見積もりの踏切と路線の上限", () => {
  test("空なら何も出さない", () => {
    expect(parseRoadProbe([])).toBeNull();
    expect(roadProbeText(null)).toBe("");
    expect(roadProbeText(parseRoadProbe([0]))).toBe("");
  });

  test("踏切だけができるとき", () => {
    expect(roadProbeText(parseRoadProbe([1]))).toBe("踏切ができる");
    expect(roadProbeText(parseRoadProbe([3]))).toBe("踏切が 3 本できる");
  });

  test("路線の上限が下がるとき、路線ごとに前と後を出す", () => {
    const probe = parseRoadProbe([1, 0, 0, 3, 1, 1, 0, 2, 0]);
    expect(probe?.lowered).toEqual([
      { index: 0, generation: 0, before: 3, after: 1 },
      { index: 1, generation: 0, before: 2, after: 0 },
    ]);
    expect(roadProbeText(probe)).toBe(
      "踏切ができ、青線の本数の上限が 3 から 1 に下がる、緑線の本数の上限が 2 から 0 に下がる",
    );
  });
});

describe("RoadProbeGate", () => {
  const probe = parseRoadProbe([1]);

  test("答えを待つあいだは次を送らず、届いてから最新の 1 件を送る", () => {
    const gate = new RoadProbeGate();
    const a = gate.keyOf({ to: 1 });
    const b = gate.keyOf({ to: 2 });
    expect(gate.shouldSend(a)).toBe(true);
    expect(gate.shouldSend(b)).toBe(false);
    gate.receive(a, probe);
    expect(gate.answer(b)).toBeNull();
    expect(gate.shouldSend(b)).toBe(true);
    expect(gate.shouldSend(b)).toBe(false);
    gate.receive(b, probe);
    expect(gate.answer(b)).toBe(probe);
  });

  test("街が変わったら古い答えを捨て、同じコマンドでも問い直す", () => {
    const gate = new RoadProbeGate();
    const a = gate.keyOf({ to: 1 });
    gate.shouldSend(a);
    gate.receive(a, probe);
    expect(gate.answer(a)).toBe(probe);
    gate.reset();
    const again = gate.keyOf({ to: 1 });
    expect(again).not.toBe(a);
    expect(gate.answer(again)).toBeNull();
    expect(gate.shouldSend(again)).toBe(true);
  });

  test("捨てる前に送った問い合わせの答えは、遅れて届いても見せない", () => {
    const gate = new RoadProbeGate();
    const old = gate.keyOf({ to: 1 });
    gate.shouldSend(old);
    gate.reset();
    gate.receive(old, probe);
    expect(gate.answer(old)).toBeNull();
    expect(gate.answer(gate.keyOf({ to: 1 }))).toBeNull();
    // 答えが届いたので、新しい問い合わせを送れる
    expect(gate.shouldSend(gate.keyOf({ to: 1 }))).toBe(true);
  });
});
