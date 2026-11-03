import { describe, expect, test } from "vitest";
import {
  badgeText,
  bellLabel,
  dismiss,
  emptyMemory,
  focusAfterRemoval,
  type Notice,
  RESOLVED_MEMORY_TICKS,
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
    const first = reconcile(emptyMemory(), [a], 0);
    expect(first.visible).toEqual([a]);
    expect(first.fresh).toEqual([a]);
    const again = reconcile(first.memory, [a, notice("lot")], 0);
    expect(again.visible).toHaveLength(2);
    expect(again.fresh.map((n) => n.id)).toEqual(["lot"]);
    expect(reconcile(again.memory, [a, notice("lot")], 0).fresh).toEqual([]);
  });

  test("見出しの件数が揺れても、key が同じなら新しい知らせにしない", () => {
    const first = reconcile(emptyMemory(), [notice("stall", "k", "住宅 4 間口")], 0);
    const next = reconcile(first.memory, [notice("stall", "k", "住宅 9 間口")], 0);
    expect(next.fresh).toEqual([]);
    expect(next.visible[0]?.headline).toBe("住宅 9 間口");
  });

  test("解けた知らせは一覧から消え、しばらくのうちに同じ状態で戻っても札は出さない", () => {
    const a = notice("stall");
    const shown = reconcile(emptyMemory(), [a], 0);
    const solved = reconcile(shown.memory, [], 100);
    expect(solved.visible).toEqual([]);
    const back = reconcile(solved.memory, [a], 200);
    expect(back.visible).toEqual([a]);
    expect(back.fresh).toEqual([]);
  });

  test("覚えておく長さを過ぎてからまた起きたら、新しい知らせとして出る", () => {
    const a = notice("stall");
    const shown = reconcile(emptyMemory(), [a], 0);
    const solved = reconcile(shown.memory, [], 10);
    const later = reconcile(solved.memory, [], 10 + RESOLVED_MEMORY_TICKS);
    expect(reconcile(later.memory, [a], 10 + RESOLVED_MEMORY_TICKS + 1).fresh).toEqual([a]);
  });

  test("tick が戻ったら（読み込み）、覚えを捨てる", () => {
    const a = notice("stall");
    const shown = reconcile(emptyMemory(), [a], 5000);
    const solved = reconcile(shown.memory, [], 5010);
    expect(reconcile(solved.memory, [a], 100).fresh).toEqual([a]);
  });

  test("key が変わると、新しい知らせとして出る", () => {
    const shown = reconcile(emptyMemory(), [notice("stall", "power")], 0);
    const changed = reconcile(shown.memory, [notice("stall", "power+water")], 0);
    expect(changed.fresh.map((n) => n.key)).toEqual(["power+water"]);
  });
});

describe("閉じた知らせ", () => {
  test("状態が同じあいだは、一覧にも札にも出さない", () => {
    const a = notice("stall");
    const shown = reconcile(emptyMemory(), [a], 0);
    const closed = reconcile(dismiss(shown.memory, a, 0), [a], 0);
    expect(closed.visible).toEqual([]);
    expect(closed.fresh).toEqual([]);
    expect(reconcile(closed.memory, [a], 0).visible).toEqual([]);
  });

  test("状態が変わったら出し直す", () => {
    const a = notice("stall", "power");
    const closed = reconcile(dismiss(reconcile(emptyMemory(), [a], 0).memory, a, 0), [a], 0);
    const changed = reconcile(closed.memory, [notice("stall", "power+water")], 0);
    expect(changed.visible).toHaveLength(1);
    expect(changed.fresh).toHaveLength(1);
  });

  test("解けてもしばらくは閉じた印を残し、過ぎてからまた起きたら出す", () => {
    const a = notice("stall");
    const closed = reconcile(dismiss(reconcile(emptyMemory(), [a], 0).memory, a, 0), [a], 0);
    const solved = reconcile(closed.memory, [], 100);
    expect(reconcile(solved.memory, [a], 200).visible).toEqual([]);
    const gone = reconcile(solved.memory, [], 100 + RESOLVED_MEMORY_TICKS);
    expect(reconcile(gone.memory, [a], 100 + RESOLVED_MEMORY_TICKS + 1).fresh).toEqual([a]);
  });

  test("電気の知らせを閉じたあとに水道も足りなくなったら、出し直す", () => {
    const power = notice("utility", "short:電気");
    const closed = reconcile(
      dismiss(reconcile(emptyMemory(), [power], 0).memory, power, 0),
      [power],
      0,
    );
    const both = reconcile(closed.memory, [notice("utility", "short:電気|水道")], 10);
    expect(both.visible).toHaveLength(1);
    expect(both.fresh).toHaveLength(1);
  });

  test("閉じたのは 1 つだけで、ほかの知らせは残る", () => {
    const a = notice("stall");
    const b = notice("lot");
    const shown = reconcile(emptyMemory(), [a, b], 0);
    const closed = reconcile(dismiss(shown.memory, a, 0), [a, b], 0);
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

describe("focusAfterRemoval", () => {
  test("消えた項目の次、なければ前、なければ鐘（null）へ移す", () => {
    expect(focusAfterRemoval(["a", "b", "c"], "b")).toBe("c");
    expect(focusAfterRemoval(["a", "b", "c"], "c")).toBe("b");
    expect(focusAfterRemoval(["a"], "a")).toBeNull();
    expect(focusAfterRemoval(["a"], "x")).toBeNull();
  });
});
