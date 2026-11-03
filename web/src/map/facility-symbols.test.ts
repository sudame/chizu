import { describe, expect, test } from "vitest";
import {
  activeSymbols,
  facilityOwner,
  facilitySymbols,
  GLYPH_PATHS,
  layoutSymbols,
  SYMBOL_BORDER,
  SYMBOL_RADIUS,
  SYMBOL_RADIUS_FAR,
  symbolBox,
  symbolLook,
} from "./facility-symbols.ts";
import {
  facilityCandidates,
  facilityLabels,
  type Measure,
  nextLabelStage,
  placeLabels,
  STAGE_NEAR,
  STAGE_STANDARD,
} from "./labels.ts";
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
  });

  test("まとまりの記号は、中心にいちばん近い施設の敷地の中心に置く", () => {
    // 3 棟（敷地の中心の x は 1、5、9）の外接矩形の中心は 5 で、真ん中の敷地の中心に当たる
    const three = groups.find((g) => g.count === 3);
    expect([three?.centerX, three?.centerY]).toEqual([5, 1]);
    // 外接矩形の中心が空き地になる配置（L 字）でも、いずれかの敷地の中心に置く
    const l = facilityLabels(
      0,
      [
        [0, 0, 1, 1, 1],
        [0, 8, 1, 9, 1],
        [8, 8, 9, 9, 1],
      ].flat(),
      () => false,
    )[0];
    expect(l?.members.some((m) => m.x === l.centerX && m.y === l.centerY)).toBe(true);
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
    expect(symbols).toHaveLength(4);
    const alphas = symbols.map((s) => s.alpha);
    expect(alphas[0]).toBeGreaterThan(0);
    expect(alphas[0]).toBeLessThan(1);
    expect(alphas[0]).toBeCloseTo(1 - (alphas[1] ?? 0));
    expect(count(mid, 1).every((s) => s.alpha === 1)).toBe(true);
    expect(count(DETAIL_BAND[0], 3)).toHaveLength(1);
    expect(count(DETAIL_BAND[1], 3)).toHaveLength(3);
  });

  test("名前の段が出している記号だけを数える", () => {
    const mid = (DETAIL_BAND[0] + DETAIL_BAND[1]) / 2;
    const placed = layoutSymbols(groups, { x: 5, y: 1, zoom: mid }, { width: 400, height: 200 });
    expect(activeSymbols(placed, false).some((s) => s.phase === "member")).toBe(false);
    expect(activeSymbols(placed, true).some((s) => s.phase === "group")).toBe(false);
  });

  test("記号の範囲は、縁を含む丸と影を囲む", () => {
    const box = symbolBox({ sx: 100, sy: 50 }, SYMBOL_RADIUS);
    const r = SYMBOL_RADIUS + SYMBOL_BORDER;
    expect(box.x0).toBe(100 - r);
    expect(box.x1).toBe(100 + r);
    expect(box.y0).toBe(50 - r);
    expect(box.y1).toBeGreaterThan(50 + r);
  });

  test("ずらす量は丸の直径までで、収まらない記号は出さない", () => {
    // 2×2 の浄水場を 5 マスおきに 20 個並べ、倍率 2 で重ねる
    const list = facilityLabels(
      0,
      Array.from({ length: 20 }, (_, i) => [i * 5, 0, i * 5 + 1, 1, 2]).flat(),
      () => false,
    );
    expect(list).toHaveLength(20);
    const zoom = 2;
    const placed = layoutSymbols(list, { x: 50, y: 1, zoom }, { width: 400, height: 200 });
    const min = 2 * (symbolLook(zoom).radius + SYMBOL_BORDER) + 1;
    expect(placed.length).toBeGreaterThan(0);
    expect(placed.length).toBeLessThan(20);
    for (const s of placed) {
      const home = { x: (s.x - 50) * zoom + 200, y: (s.y - 1) * zoom + 100 };
      expect(Math.hypot(s.sx - home.x, s.sy - home.y)).toBeLessThanOrEqual(min + 1e-6);
    }
    for (let i = 0; i < placed.length; i++) {
      for (let j = 0; j < i; j++) {
        const [a, b] = [placed[i], placed[j]];
        expect(
          Math.hypot((a?.sx ?? 0) - (b?.sx ?? 0), (a?.sy ?? 0) - (b?.sy ?? 0)),
        ).toBeGreaterThanOrEqual(min - 1e-6);
      }
    }
  });

  test("画面の外の施設は、ずらす計算に入れない", () => {
    const list = facilityLabels(
      0,
      Array.from({ length: 1000 }, (_, i) => [i * 3, 0, i * 3 + 1, 1, 2]).flat(),
      () => false,
    );
    const placed = layoutSymbols(list, { x: 50, y: 1, zoom: 4 }, { width: 400, height: 200 });
    expect(placed.length).toBeLessThan(100);
    // 画面の中は、1000 施設あっても数十ミリ秒かからない
    const start = performance.now();
    layoutSymbols(list, { x: 50, y: 1, zoom: 4 }, { width: 400, height: 200 });
    expect(performance.now() - start).toBeLessThan(15);
  });

  test("丸が重なる記号は、重ならない所までずらす", () => {
    // 2 マスの隙間もない別の種類の施設を、倍率 4 で並べる
    const near = facilityLabels(
      0,
      [
        [0, 0, 1, 1, 1],
        [2, 0, 3, 1, 2],
      ].flat(),
      () => false,
    );
    const placed = layoutSymbols(near, { x: 2, y: 1, zoom: 4 }, { width: 400, height: 200 });
    const [a, b] = placed;
    const min = 2 * (symbolLook(4).radius + SYMBOL_BORDER);
    expect(
      Math.hypot((a?.sx ?? 0) - (b?.sx ?? 0), (a?.sy ?? 0) - (b?.sy ?? 0)),
    ).toBeGreaterThanOrEqual(min);
  });
});

describe("施設の名前と記号", () => {
  const measure: Measure = (text, style) => ({ w: style.size * [...text].length, h: style.size });
  const vp = { width: 900, height: 300 };

  /** 名前を置き、置けた名前を返す。描いている記号は、薄いものも含めて置けない範囲にする。 */
  const placeNames = (
    list: typeof groups,
    camera: { x: number; y: number; zoom: number },
    stage: number,
  ) => {
    const look = symbolLook(camera.zoom);
    const symbols = layoutSymbols(list, camera, vp);
    const obstacles = symbols.map((s) => ({ ...symbolBox(s, look.radius), owner: s.key }));
    const names = placeLabels(
      facilityCandidates(list, camera, vp, stage, measure, symbols),
      new Set(),
      obstacles,
    );
    return { names, symbols, look };
  };

  test("右にほかの施設の記号があっても、左、下、上の順で置き場所を探して名前を出す", () => {
    // 浄水場 3 棟の右隣に、発電所がある（名前の右 15px は発電所の記号に当たる）
    const list = facilityLabels(
      0,
      [
        [0, 0, 1, 1, 2],
        [4, 0, 5, 1, 2],
        [8, 0, 9, 1, 2],
        [10, 0, 13, 3, 1],
      ].flat(),
      () => false,
    );
    const { names: placed } = placeNames(list, { x: 7, y: 2, zoom: 8 }, STAGE_STANDARD);
    expect(placed).toHaveLength(2);
    // 浄水場の名前は、右でなく別の側に置かれる
    const water = placed.find((c) => c.text.includes("浄水場"));
    expect(water?.ox).not.toBe(15);
  });

  test("名前を置けなくても、記号は出る", () => {
    const list = facilityLabels(0, [[0, 0, 1, 1, 2]].flat(), () => false);
    const camera = { x: 1, y: 1, zoom: 8 };
    expect(placeNames(list, { ...camera }, STAGE_STANDARD).names.length).toBeGreaterThan(0);
    expect(layoutSymbols(list, camera, { width: 20, height: 20 })).toHaveLength(1);
    expect(
      facilityCandidates(list, camera, { width: 20, height: 20 }, STAGE_STANDARD, measure),
    ).toEqual([]);
  });

  test("倍率 9〜13 を細かく動かしても、まとまりの名前は消えない", () => {
    const list = facilityLabels(
      0,
      [
        [0, 0, 1, 1, 2],
        [4, 0, 5, 1, 2],
        [8, 0, 9, 1, 2],
      ].flat(),
      () => false,
    );
    for (const direction of [1, -1]) {
      let stage = direction > 0 ? STAGE_STANDARD : STAGE_NEAR;
      for (let i = 0; i <= 16; i++) {
        const zoom = direction > 0 ? 9 + i / 4 : 13 - i / 4;
        stage = nextLabelStage(zoom, stage);
        const { names } = placeNames(list, { x: 5, y: 1, zoom }, stage);
        expect(names, `倍率 ${zoom}`).toHaveLength(1);
      }
    }
  });
  test("入れ替わりの途中の薄い記号にも、名前を重ねない", () => {
    const list = facilityLabels(
      0,
      [
        [0, 0, 1, 1, 2],
        [4, 0, 5, 1, 2],
        [8, 0, 9, 1, 2],
      ].flat(),
      () => false,
    );
    for (const zoom of [10.5, 11, 11.5, 12, 13]) {
      const stage = nextLabelStage(zoom, zoom >= 12 ? STAGE_NEAR : STAGE_STANDARD);
      const { names, symbols, look } = placeNames(list, { x: 5, y: 1, zoom }, stage);
      expect(names, `倍率 ${zoom}`).toHaveLength(1);
      const box = names[0]?.box;
      for (const s of symbols.filter((s) => s.key !== names[0]?.owner)) {
        const b = symbolBox(s, look.radius);
        const hit = box && box.x0 < b.x1 && b.x0 < box.x1 && box.y0 < b.y1 && b.y0 < box.y1;
        expect(hit, `倍率 ${zoom}、記号 ${s.key}`).toBe(false);
      }
    }
  });

  test("施設が 100 まとまり見えても、道路の名前を置く", () => {
    const facilities = Array.from({ length: 100 }, (_, i) => [
      (i % 10) * 30,
      Math.floor(i / 10) * 30,
      (i % 10) * 30 + 1,
      Math.floor(i / 10) * 30 + 1,
      2,
    ]).flat();
    const list = facilityLabels(0, facilities, () => false);
    expect(list).toHaveLength(100);
    const camera = { x: 135, y: 135, zoom: 5 };
    const wide = { width: 1600, height: 1600 };
    const facility = facilityCandidates(list, camera, wide, STAGE_STANDARD, measure);
    // 1 つのまとまりは 1 つの候補と数える
    expect(facility).toHaveLength(100);
    const road = {
      id: "road",
      priority: 5,
      text: "道",
      style: { size: 11, weight: 500, spacing: 0, color: "#000", alpha: 1, shield: false },
      vertical: false,
      angle: 0,
      wx: 0,
      wy: 0,
      ox: 0,
      oy: 0,
      w: 20,
      h: 11,
      box: { x0: 5, y0: 5, x1: 25, y1: 16 },
    };
    const placed = placeLabels([...facility, road], new Set(), []);
    expect(placed.some((c) => c.id === "road")).toBe(true);
  });
});
