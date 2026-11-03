import { describe, expect, test } from "vitest";
import {
  badgeText,
  bellLabel,
  dismiss,
  emptyMemory,
  type Notice,
  reconcile,
  toastText,
} from "./notice-center.ts";

const notice = (id: string, key = "k", headline = `${id}の知らせ`): Notice => ({
  id,
  key,
  headline,
  details: [],
  jump: null,
});

describe("reconcile", () => {
  test("初めて出る知らせだけが fresh になり、続くあいだは出し直さない", () => {
    const a = notice("stall");
    const first = reconcile(emptyMemory(), [a]);
    expect(first.visible).toEqual([a]);
    expect(first.fresh).toEqual([a]);
    const again = reconcile(first.memory, [a, notice("lot")]);
    expect(again.visible).toHaveLength(2);
    expect(again.fresh.map((n) => n.id)).toEqual(["lot"]);
    expect(reconcile(again.memory, [a, notice("lot")]).fresh).toEqual([]);
  });

  test("見出しの件数が揺れても、key が同じなら新しい知らせにしない", () => {
    const first = reconcile(emptyMemory(), [notice("stall", "k", "住宅 4 間口")]);
    const next = reconcile(first.memory, [notice("stall", "k", "住宅 9 間口")]);
    expect(next.fresh).toEqual([]);
    expect(next.visible[0]?.headline).toBe("住宅 9 間口");
  });

  test("解けた知らせは一覧から消え、また起きれば新しい知らせとして出る", () => {
    const a = notice("stall");
    const shown = reconcile(emptyMemory(), [a]);
    const solved = reconcile(shown.memory, []);
    expect(solved.visible).toEqual([]);
    expect(reconcile(solved.memory, [a]).fresh).toEqual([a]);
  });

  test("key が変わると、新しい知らせとして出る", () => {
    const shown = reconcile(emptyMemory(), [notice("stall", "power")]);
    const changed = reconcile(shown.memory, [notice("stall", "power+water")]);
    expect(changed.fresh.map((n) => n.key)).toEqual(["power+water"]);
  });
});

describe("閉じた知らせ", () => {
  test("状態が同じあいだは、一覧にも札にも出さない", () => {
    const a = notice("stall");
    const shown = reconcile(emptyMemory(), [a]);
    const closed = reconcile(dismiss(shown.memory, a), [a]);
    expect(closed.visible).toEqual([]);
    expect(closed.fresh).toEqual([]);
    expect(reconcile(closed.memory, [a]).visible).toEqual([]);
  });

  test("状態が変わったら出し直す", () => {
    const a = notice("stall", "power");
    const closed = reconcile(dismiss(reconcile(emptyMemory(), [a]).memory, a), [a]);
    const changed = reconcile(closed.memory, [notice("stall", "power+water")]);
    expect(changed.visible).toHaveLength(1);
    expect(changed.fresh).toHaveLength(1);
  });

  test("解けてからまた起きたら、閉じていても出す", () => {
    const a = notice("stall");
    const closed = reconcile(dismiss(reconcile(emptyMemory(), [a]).memory, a), [a]);
    const solved = reconcile(closed.memory, []);
    expect(reconcile(solved.memory, [a]).fresh).toEqual([a]);
  });

  test("閉じたのは 1 つだけで、ほかの知らせは残る", () => {
    const a = notice("stall");
    const b = notice("lot");
    const shown = reconcile(emptyMemory(), [a, b]);
    const closed = reconcile(dismiss(shown.memory, a), [a, b]);
    expect(closed.visible.map((n) => n.id)).toEqual(["lot"]);
  });
});

describe("文言", () => {
  test("札は 1 件なら見出し、複数なら件数", () => {
    expect(toastText([])).toBe("");
    expect(toastText([notice("a", "k", "電気が足りない")])).toBe("電気が足りない");
    expect(toastText([notice("a"), notice("b")])).toBe("新しい知らせが 2 件ある");
  });

  test("鐘の読み上げと数の印", () => {
    expect(bellLabel(0)).toBe("知らせ");
    expect(bellLabel(3)).toBe("知らせ 3 件");
    expect(badgeText(3)).toBe("3");
    expect(badgeText(12)).toBe("9+");
  });
});
