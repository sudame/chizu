import { describe, expect, test } from "vitest";
import {
  facilitySymbols,
  GLYPH_PATHS,
  SYMBOL_BORDER,
  SYMBOL_RADIUS,
  SYMBOL_RADIUS_FAR,
  symbolBox,
  symbolLook,
} from "./facility-symbols.ts";
import { facilityLabels, facilityOwner, placeLabels } from "./labels.ts";
import { FACILITY_STYLES } from "./palette.ts";
import { BLOCK_BAND, DETAIL_BAND } from "./stages.ts";

// 浄水場 3 つ（隙間 2 マスで並ぶ）と、離れた浄水場 1 つ、隣の発電所 1 つ
const FACILITIES = [
  [0, 0, 1, 1, 2],
  [4, 0, 5, 1, 2],
  [8, 0, 9, 1, 2],
  [20, 0, 21, 1, 2],
  [10, 0, 11, 1, 1],
].flat();
const groups = facilityLabels(0, FACILITIES, () => false);

describe("施設の記号の見た目", () => {
  test("引きは小さな丸だけで絵がなく、標準からは絵のある丸になる", () => {
    expect(symbolLook(1)).toEqual({ radius: SYMBOL_RADIUS_FAR, glyphAlpha: 0 });
    expect(symbolLook(BLOCK_BAND[0])).toEqual({ radius: SYMBOL_RADIUS_FAR, glyphAlpha: 0 });
    expect(symbolLook(8)).toEqual({ radius: SYMBOL_RADIUS, glyphAlpha: 1 });
    expect(symbolLook(32)).toEqual({ radius: SYMBOL_RADIUS, glyphAlpha: 1 });
    // 溶け込むあいだは、丸が大きくなり、絵が濃くなる
    const mid = symbolLook((BLOCK_BAND[0] + BLOCK_BAND[1]) / 2);
    expect(mid.radius).toBeGreaterThan(SYMBOL_RADIUS_FAR);
    expect(mid.radius).toBeLessThan(SYMBOL_RADIUS);
    expect(mid.glyphAlpha).toBeGreaterThan(0);
    expect(mid.glyphAlpha).toBeLessThan(1);
  });

  test("決まりの表の色と、種類ごとの絵を持つ", () => {
    expect(FACILITY_STYLES[1]).toMatchObject({ circle: "#e8710a", glyph: "bolt", name: "#a8500a" });
    expect(FACILITY_STYLES[2]).toMatchObject({ circle: "#12a4af", glyph: "drop", name: "#0b6f78" });
    expect(Object.keys(GLYPH_PATHS).sort()).toEqual(["bolt", "drop"]);
  });
});

describe("出す記号", () => {
  const count = (zoom: number, count: number) =>
    facilitySymbols(groups, zoom).filter((s) =>
      groups.some((g) => g.count === count && facilityOwner(g) === s.owner),
    );

  test("引きと標準は、まとまりごとに記号を 1 つ出す", () => {
    for (const zoom of [3, 8]) {
      const symbols = facilitySymbols(groups, zoom);
      expect(symbols).toHaveLength(groups.length);
      expect(symbols.every((s) => s.alpha === 1)).toBe(true);
    }
    // 3 棟のまとまりの記号は、まとまりの中心に出す
    const three = groups.find((g) => g.count === 3);
    const symbol = facilitySymbols(groups, 8).find(
      (s) => s.owner === (three && facilityOwner(three)),
    );
    expect([symbol?.x, symbol?.y]).toEqual([three?.centerX, three?.centerY]);
  });

  test("寄りは、施設 1 つずつに記号を出す", () => {
    const symbols = facilitySymbols(groups, 20);
    expect(symbols).toHaveLength(FACILITIES.length / 5);
    expect(symbols.every((s) => s.alpha === 1)).toBe(true);
    expect(count(20, 3)).toHaveLength(3);
  });

  test("標準から寄りへ移るあいだは、まとまりの記号を薄くし、1 つずつの記号を濃くする", () => {
    const mid = (DETAIL_BAND[0] + DETAIL_BAND[1]) / 2;
    const symbols = count(mid, 3);
    // まとまりの記号 1 つと、3 棟の記号
    expect(symbols).toHaveLength(4);
    const alphas = symbols.map((s) => s.alpha);
    expect(alphas[0]).toBeGreaterThan(0);
    expect(alphas[0]).toBeLessThan(1);
    expect(alphas[0]).toBeCloseTo(1 - (alphas[1] ?? 0));
    // 1 棟だけのまとまりは、入れ替えずに同じ位置で出し続ける
    expect(count(mid, 1).every((s) => s.alpha === 1)).toBe(true);
    // 境目の外では、どちらか片方だけを描く
    expect(count(DETAIL_BAND[0], 3)).toHaveLength(1);
    expect(count(DETAIL_BAND[1], 3)).toHaveLength(3);
  });

  test("記号の範囲は、縁を含む丸と影を囲む", () => {
    const s = { owner: "a", kind: 1, x: 10, y: 5, alpha: 1 };
    const box = symbolBox(s, { x: 10, y: 5, zoom: 8 }, { width: 200, height: 100 }, SYMBOL_RADIUS);
    const r = SYMBOL_RADIUS + SYMBOL_BORDER;
    expect(box.x0).toBe(100 - r);
    expect(box.x1).toBe(100 + r);
    expect(box.y0).toBe(50 - r);
    expect(box.y1).toBeGreaterThan(50 + r);
  });
});

describe("ラベルとの重なり", () => {
  const camera = { x: 5, y: 1, zoom: 8 };
  const viewport = { width: 400, height: 200 };

  test("施設の名前は自分の記号の右に置け、ほかのラベルは記号の上に置かない", () => {
    const three = groups.find((g) => g.count === 3);
    // 隣の発電所の記号が名前にかぶらないよう、3 棟のまとまりだけを出す
    const symbols = facilitySymbols(three ? [three] : [], camera.zoom);
    const obstacles = symbols.map((s) => ({
      ...symbolBox(s, camera, viewport, SYMBOL_RADIUS),
      owner: s.owner,
    }));
    const center = {
      x: ((three?.centerX ?? 0) - camera.x) * camera.zoom + viewport.width / 2,
      y: ((three?.centerY ?? 0) - camera.y) * camera.zoom + viewport.height / 2,
    };
    const candidate = (owner: string | undefined, x0: number) => ({
      id: `${owner}|${x0}`,
      priority: 1,
      text: "名",
      style: { size: 12, weight: 600, spacing: 0, color: "#000", alpha: 1, shield: false },
      vertical: false,
      angle: 0,
      wx: 0,
      wy: 0,
      ox: 0,
      oy: 0,
      w: 40,
      h: 16,
      box: { x0, y0: center.y - 8, x1: x0 + 40, y1: center.y + 8 },
      owner,
    });
    // 記号の右 15px から始まる名前は、自分の記号には余白をとらずに調べるので置ける
    expect(
      placeLabels([candidate(three && facilityOwner(three), center.x + 15)], new Set(), obstacles),
    ).toHaveLength(1);
    // 持ち主のないラベルは、記号の縁に重なる所には置けない
    expect(placeLabels([candidate(undefined, center.x + 15)], new Set(), obstacles)).toHaveLength(
      0,
    );
    expect(placeLabels([candidate(undefined, center.x - 5)], new Set(), obstacles)).toHaveLength(0);
  });
});
