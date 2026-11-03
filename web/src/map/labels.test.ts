import { describe, expect, test } from "vitest";
import {
  type Candidate,
  DISTRICT_MIN_BUILDINGS,
  districtCandidates,
  districtLabels,
  FADE_MS,
  facilityCandidates,
  facilityLabels,
  LabelFader,
  type LabelKind,
  labelStyle,
  MAX_CANDIDATES,
  type Measure,
  nextLabelStage,
  parkingCandidates,
  parkingLabels,
  placeLabels,
  type Rect,
  ROAD_LABEL_REPEAT,
  roadCandidates,
  roadRuns,
  STAGE_DETAIL,
  STAGE_FAR,
  STAGE_NEAR,
  STAGE_STANDARD,
} from "./labels.ts";
import { districtName, roadName } from "./names.ts";

// 1 字を字の大きさの正方形とみなす測り方
const measure: Measure = (text, style, vertical) => {
  const n = [...text].length;
  return vertical ? { w: style.size, h: style.size * n } : { w: style.size * n, h: style.size };
};

const box = (x0: number, y0: number, x1: number, y1: number): Rect => ({ x0, y0, x1, y1 });

function candidate(id: string, priority: number, b: Rect): Candidate {
  const style = { size: 12, weight: 500, spacing: 0, color: "#000000", alpha: 1, shield: false };
  return {
    id,
    priority,
    text: id,
    style,
    vertical: false,
    angle: 0,
    wx: 0,
    wy: 0,
    ox: 0,
    oy: 0,
    w: b.x1 - b.x0,
    h: b.y1 - b.y0,
    box: b,
  };
}

const ids = (cs: readonly Candidate[]) => cs.map((c) => c.id);

describe("重なりと優先", () => {
  test("重なるラベルは、優先の高いほうだけを置く", () => {
    const placed = placeLabels(
      [candidate("小", 7, box(0, 0, 40, 12)), candidate("高速", 1, box(20, 4, 60, 16))],
      new Set(),
      [],
    );
    expect(ids(placed)).toEqual(["高速"]);
  });

  test("4px の余白の中に入るものも重なりとみなす", () => {
    const a = candidate("a", 1, box(0, 0, 10, 10));
    expect(ids(placeLabels([a, candidate("b", 2, box(13, 0, 20, 10))], new Set(), []))).toEqual([
      "a",
    ]);
    expect(ids(placeLabels([a, candidate("c", 2, box(14, 0, 20, 10))], new Set(), []))).toEqual([
      "a",
      "c",
    ]);
  });

  test("パネルの下になる所には置かない", () => {
    const placed = placeLabels(
      [candidate("a", 1, box(0, 0, 10, 10)), candidate("b", 1, box(100, 0, 110, 10))],
      new Set(),
      [box(90, 0, 200, 40)],
    );
    expect(ids(placed)).toEqual(["a"]);
  });

  test("同じ優先なら、前に出ていたラベルを先に置く", () => {
    const a = candidate("a", 5, box(0, 0, 40, 12));
    const b = candidate("b", 5, box(10, 0, 50, 12));
    expect(ids(placeLabels([a, b], new Set(), []))).toEqual(["a"]);
    expect(ids(placeLabels([a, b], new Set(["b"]), []))).toEqual(["b"]);
    // 優先が違えば、前に出ていたかによらない
    const c = candidate("c", 4, box(10, 0, 50, 12));
    expect(ids(placeLabels([a, c], new Set(["a"]), []))).toEqual(["c"]);
  });

  test("決まりの優先の順に並ぶ", () => {
    const order: [LabelKind, number][] = [
      ["highway", STAGE_STANDARD],
      ["district", STAGE_FAR],
      ["facility", STAGE_STANDARD],
      ["district", STAGE_STANDARD],
      ["large", STAGE_STANDARD],
      ["medium", STAGE_STANDARD],
      ["small", STAGE_NEAR],
      ["district", STAGE_NEAR],
      ["parking", STAGE_NEAR],
    ];
    const priorities = order.map(([kind, stage]) => labelStyle(kind, stage)?.priority);
    expect(priorities).toEqual([...priorities].sort((a = 0, b = 0) => a - b));
    expect(new Set(priorities).size).toBe(order.length);
  });

  test("調べる候補は 1 画面 MAX_CANDIDATES 個まで", () => {
    const many = Array.from({ length: MAX_CANDIDATES + 50 }, (_, i) =>
      candidate(String(i), 7, box(i * 30, 0, i * 30 + 10, 10)),
    );
    expect(placeLabels(many, new Set(), [])).toHaveLength(MAX_CANDIDATES);
  });
});

describe("段ごとの出し方", () => {
  test("寄るときは境目で段を移し、引くときは 0.5 引いてから戻る", () => {
    expect(nextLabelStage(3.9, STAGE_FAR)).toBe(STAGE_FAR);
    expect(nextLabelStage(4, STAGE_FAR)).toBe(STAGE_STANDARD);
    expect(nextLabelStage(3.6, STAGE_STANDARD)).toBe(STAGE_STANDARD);
    expect(nextLabelStage(3.5, STAGE_STANDARD)).toBe(STAGE_STANDARD);
    expect(nextLabelStage(3.49, STAGE_STANDARD)).toBe(STAGE_FAR);
    expect(nextLabelStage(12, STAGE_STANDARD)).toBe(STAGE_NEAR);
    expect(nextLabelStage(11.6, STAGE_NEAR)).toBe(STAGE_NEAR);
    expect(nextLabelStage(11.4, STAGE_NEAR)).toBe(STAGE_STANDARD);
    // 一度に段を飛ばして動いても、その倍率の段になる
    expect(nextLabelStage(20, STAGE_FAR)).toBe(STAGE_NEAR);
    expect(nextLabelStage(2, STAGE_NEAR)).toBe(STAGE_FAR);
  });

  test("段ごとに出すラベルの種類が決まりの表のとおりになる", () => {
    const shown = (stage: number) =>
      (["highway", "district", "facility", "large", "medium", "small"] as LabelKind[]).filter(
        (k) => labelStyle(k, stage) !== null,
      );
    // 引き：地区は大きく、道路は大と高速だけ、施設の名前は出さない
    expect(shown(STAGE_FAR)).toEqual(["highway", "district", "large"]);
    expect(shown(STAGE_STANDARD)).toEqual(["highway", "district", "facility", "large", "medium"]);
    expect(shown(STAGE_NEAR)).toEqual([
      "highway",
      "district",
      "facility",
      "large",
      "medium",
      "small",
    ]);
    // 地区の名前を消す段
    expect(shown(STAGE_DETAIL)).toEqual(["highway", "facility", "large", "medium", "small"]);
  });

  test("字の大きさと間隔と色が決まりのとおりになる", () => {
    expect(labelStyle("district", STAGE_FAR)?.style).toMatchObject({ size: 17, spacing: 4 });
    expect(labelStyle("district", STAGE_STANDARD)?.style).toMatchObject({ size: 14, spacing: 2 });
    expect(labelStyle("district", STAGE_NEAR)?.style).toMatchObject({
      size: 13,
      alpha: 0.8,
      color: "#70757a",
    });
    expect(labelStyle("large", STAGE_FAR)?.style).toMatchObject({ size: 11, color: "#6b5310" });
    expect(labelStyle("small", STAGE_NEAR)?.style).toMatchObject({ size: 12, color: "#5f6368" });
    expect(labelStyle("highway", STAGE_NEAR)?.style).toMatchObject({ size: 12, shield: true });
    expect(labelStyle("facility", STAGE_STANDARD)?.style).toMatchObject({
      size: 12,
      weight: 600,
    });
  });
});

describe("道路の名前", () => {
  const vp = { width: 800, height: 600 };

  test("同じ直線の上で続く同じ種類の区間をつなぎ、離れた区間は分ける", () => {
    const runs = roadRuns(
      0,
      // 横の中の道路 0→10→20 と、離れた 30→40、同じ直線の大の道路
      [0, 5, 10, 5, 2, 20, 5, 10, 5, 2, 30, 5, 40, 5, 2, 40, 5, 50, 5, 3],
    );
    const medium = runs.filter((r) => r.kind === 2).map((r) => [r.ax, r.bx]);
    expect(medium).toEqual([
      [0.5, 20.5],
      [30.5, 40.5],
    ]);
    expect(runs.find((r) => r.kind === 3)?.name).toBe(roadName(0, "H5", 3));
    expect(runs.every((r) => r.orientation === "horizontal")).toBe(true);
  });

  test("縦の道路は縦書き、斜めの道路は字が逆さにならない角度で置く", () => {
    const camera = { x: 20, y: 20, zoom: 10 };
    const vertical = roadCandidates(
      roadRuns(0, [20, 0, 20, 40, 2]),
      camera,
      vp,
      STAGE_STANDARD,
      measure,
    );
    expect(vertical.length).toBeGreaterThan(0);
    expect(vertical.every((c) => c.vertical && c.angle === 0)).toBe(true);
    // 右上がりの斜め。左下から右上へ向かうので -45 度
    const angled = roadCandidates(
      roadRuns(0, [40, 0, 0, 40, 2]),
      camera,
      vp,
      STAGE_STANDARD,
      measure,
    );
    expect(angled.length).toBeGreaterThan(0);
    for (const c of angled) {
      expect(c.vertical).toBe(false);
      expect(Math.abs(c.angle)).toBeCloseTo(Math.PI / 4);
      expect(Math.abs(c.angle)).toBeLessThanOrEqual(Math.PI / 2);
    }
  });

  test("見えている長さが字の長さ + 40px より短い区間には置かない", () => {
    const camera = { x: 0, y: 0, zoom: 10 };
    // 「〇〇通り」は多くて 4 字で 44px、要るのは 84px。6 マス（60px）では置かない
    const short = roadCandidates(roadRuns(0, [0, 0, 6, 0, 2]), camera, vp, STAGE_STANDARD, measure);
    expect(short).toEqual([]);
    const long = roadCandidates(roadRuns(0, [0, 0, 12, 0, 2]), camera, vp, STAGE_STANDARD, measure);
    expect(long).toHaveLength(1);
  });

  test("長い区間は 360px ごとに繰り返し、位置は地図と一緒に動く", () => {
    const runs = roadRuns(0, [-200, 0, 200, 0, 3]);
    const at = (x: number) => roadCandidates(runs, { x, y: 0, zoom: 4 }, vp, STAGE_FAR, measure);
    const a = at(0);
    expect(a.length).toBeGreaterThanOrEqual(2);
    const xs = a.map((c) => c.wx).sort((p, q) => p - q);
    expect((xs[1] ?? 0) - (xs[0] ?? 0)).toBeCloseTo(ROAD_LABEL_REPEAT / 4);
    // 少し動かしても、同じ識別子のラベルは同じワールドの位置にある
    const b = at(3);
    for (const c of b) {
      const same = a.find((d) => d.id === c.id);
      if (same !== undefined) expect(c.wx).toBeCloseTo(same.wx);
    }
  });

  test("道路を延ばしても、同じ位置のラベルは同じ識別子のまま残る", () => {
    const camera = { x: 0, y: 0, zoom: 4 };
    const before = roadCandidates(roadRuns(0, [-200, 0, 100, 0, 3]), camera, vp, 0, measure);
    const after = roadCandidates(
      roadRuns(0, [-200, 0, 100, 0, 3, 100, 0, 300, 0, 3]),
      camera,
      vp,
      0,
      measure,
    );
    expect(before.length).toBeGreaterThan(0);
    for (const c of before) {
      const same = after.find((d) => d.id === c.id);
      expect(same?.wx).toBeCloseTo(c.wx);
    }
  });

  test("画面の端にかかって字が収まらない位置には置かず、見えている部分の真ん中にずらさない", () => {
    const runs = roadRuns(0, [-200, 0, 200, 0, 3]);
    // 倍率 4 では、繰り返す位置は x = (k + 0.5) × 90 マスにある。x = 45 が画面の右端に来るときは出さない
    const camera = { x: 45 - 400 / 4, y: 0, zoom: 4 };
    const xs = roadCandidates(runs, camera, vp, STAGE_FAR, measure).map((c) => c.wx);
    expect(xs.length).toBeGreaterThan(0);
    for (const x of xs) {
      const r = x / 90 - 0.5;
      expect(r - Math.round(r)).toBeCloseTo(0);
    }
    expect(xs.some((x) => Math.abs(x - 45) < 1e-6)).toBe(false);
  });

  test("段で出さない種類の道路には置かない", () => {
    const runs = roadRuns(0, [0, 0, 100, 0, 1]);
    const camera = { x: 50, y: 0, zoom: 6 };
    expect(roadCandidates(runs, camera, vp, STAGE_STANDARD, measure)).toEqual([]);
    expect(roadCandidates(runs, camera, vp, STAGE_NEAR, measure).length).toBeGreaterThan(0);
  });
});

describe("地区と施設の名前", () => {
  const building = (x: number, y: number) => [x, y, x, y, 1, 1, 3];

  test("建物が 12 棟以上あるチャンクだけに、建物の重心へ地区の名前を置く", () => {
    const many = Array.from({ length: DISTRICT_MIN_BUILDINGS }, (_, i) => building(i * 2, 10));
    const few = Array.from({ length: DISTRICT_MIN_BUILDINGS - 1 }, (_, i) => building(70 + i, 10));
    const labels = districtLabels(7, [...many, ...few].flat(), () => false);
    expect(labels).toHaveLength(1);
    const [d] = labels;
    expect(d?.name).toBe(districtName(7, 0, 0));
    expect(d?.x).toBeCloseTo(11.5);
    expect(d?.y).toBeCloseTo(10.5);
  });

  test("水辺のあるチャンクの地区は「浜」になる", () => {
    const many = Array.from({ length: DISTRICT_MIN_BUILDINGS }, (_, i) => building(i, 3));
    const [d] = districtLabels(7, many.flat(), (cx, cy) => cx === 0 && cy === 0);
    expect(d?.name.endsWith("浜")).toBe(true);
  });

  test("地区の名前は段に合わせた大きさで出し、地区の名前を消す段では出さない", () => {
    const d = [{ cx: 0, cy: 0, name: "楓町", x: 10, y: 10 }];
    const camera = { x: 10, y: 10, zoom: 3 };
    const vp = { width: 400, height: 300 };
    expect(districtCandidates(d, camera, vp, STAGE_FAR, measure)[0]?.style.size).toBe(17);
    expect(districtCandidates(d, camera, vp, STAGE_DETAIL, measure)).toEqual([]);
  });

  test("同じ種類で 2 マス以内の施設は 1 つのまとまりにし、標準では棟の数を添える", () => {
    // 浄水場 3 つ（2 マスの隙間で並ぶ）と、離れた浄水場、隣の発電所
    const facilities = [
      [0, 0, 1, 1, 2],
      [4, 0, 5, 1, 2],
      [8, 0, 9, 1, 2],
      [20, 0, 21, 1, 2],
      [10, 0, 11, 1, 1],
    ].flat();
    const groups = facilityLabels(0, facilities, () => false);
    const water = groups.filter((g) => g.kind === 2);
    expect(water.map((g) => g.count)).toEqual([3, 1]);
    expect(groups.filter((g) => g.kind === 1).map((g) => g.count)).toEqual([1]);
    // 同じ地区の同じ種類の 2 つ目は「第二」
    expect(water[1]?.name).toContain("第二");
    const camera = { x: 10, y: 1, zoom: 8 };
    const vp = { width: 800, height: 300 };
    const std = facilityCandidates(groups, camera, vp, STAGE_STANDARD, measure);
    expect(std.find((c) => c.text.endsWith("（3 棟）"))).toBeDefined();
    const near = facilityCandidates(groups, camera, vp, STAGE_NEAR, measure);
    expect(near.some((c) => c.text.includes("棟"))).toBe(false);
    expect(facilityCandidates(groups, camera, vp, STAGE_FAR, measure)).toEqual([]);
    // 名前は記号の右 15px に置き、施設の種類の色にする
    const first = std[0];
    expect(first?.ox).toBe(15);
    expect(near.find((c) => c.text.endsWith("発電所"))?.style.color).toBe("#a8500a");
  });

  test("寄りの名前は、まとまりのいちばん右の記号から試し、標準はまとまりの記号の右から試す", () => {
    const facilities = [
      [0, 0, 1, 1, 2],
      [4, 0, 5, 1, 2],
      [8, 0, 9, 1, 2],
    ].flat();
    const groups = facilityLabels(0, facilities, () => false);
    expect(groups[0]?.members.map((m) => m.x)).toEqual([1, 5, 9]);
    const vp = { width: 800, height: 300 };
    const std = facilityCandidates(groups, { x: 5, y: 1, zoom: 8 }, vp, STAGE_STANDARD, measure)[0];
    const near = facilityCandidates(groups, { x: 5, y: 1, zoom: 16 }, vp, STAGE_NEAR, measure)[0];
    expect(std?.wx).toBe(groups[0]?.centerX);
    expect(near?.wx).toBe(9);
    expect(near?.ox).toBe(15);
    // 置き方の違う候補は、右、左、下、上の順に並ぶ
    const all = facilityCandidates(groups, { x: 5, y: 1, zoom: 8 }, vp, STAGE_STANDARD, measure);
    // 1 つのまとまりは 1 つの候補で、置き方の違いは alternatives に持つ
    expect(all).toHaveLength(1);
    const options = [all[0], ...(all[0]?.alternatives ?? [])];
    expect(options.map((c) => c?.id.split("|").at(-3))).toEqual(["r", "l", "b", "t"]);
    // いちばん右の記号の右が画面の外にあるときは、収まる置き方を出す
    const narrow = { width: 300, height: 300 };
    const fallback = facilityCandidates(
      groups,
      { x: 0, y: 1, zoom: 16 },
      narrow,
      STAGE_NEAR,
      measure,
    );
    expect(fallback.length).toBeGreaterThan(0);
    const boxes = fallback.flatMap((c) => [c, ...(c.alternatives ?? [])]).map((c) => c.box);
    expect(boxes.every((b) => b.x1 <= narrow.width && b.x0 >= 0)).toBe(true);
  });
});

describe("出し入れ", () => {
  test("ラベルは 200ms かけて出し、置かなくなったら 200ms かけて消す", () => {
    const fader = new LabelFader();
    const a = candidate("a", 1, box(0, 0, 10, 10));
    fader.update([a]);
    expect(fader.step(FADE_MS / 2)).toBe(true);
    expect([...fader.items()][0]?.alpha).toBeCloseTo(0.5);
    expect(fader.step(FADE_MS / 2)).toBe(false);
    expect([...fader.items()][0]?.alpha).toBe(1);
    expect(fader.shown()).toEqual(new Set(["a"]));
    fader.update([]);
    expect(fader.shown()).toEqual(new Set());
    expect(fader.step(FADE_MS / 4)).toBe(true);
    expect([...fader.items()][0]?.alpha).toBeCloseTo(0.75);
    fader.step(FADE_MS);
    expect([...fader.items()]).toEqual([]);
  });
});

describe("建物の中の駐車場の台数", () => {
  // 建物 (10, 20)-(11, 21) と、建物 (30, 40)-(30, 40)。供給などの欄は使わない
  const buildings = [10, 20, 11, 21, 1, 1, 3, 30, 40, 30, 40, 1, 1, 3];
  const camera = { x: 11, y: 21, zoom: 16 };
  const vp = { width: 400, height: 300 };

  test("建物の中に停めてある建物だけを、敷地の中心に結ぶ", () => {
    const labels = parkingLabels(buildings, new Map([["10,20", 12]]));
    expect(labels).toEqual([{ key: "10,20", x: 11, y: 21, count: 12 }]);
    expect(parkingLabels(buildings, new Map())).toEqual([]);
    expect(parkingLabels(buildings, new Map([["99,99", 3]]))).toEqual([]);
  });

  test("数字は「P 台数」で出し、台数が変わっても同じラベルのまま字だけ変わる", () => {
    const at = (n: number) =>
      parkingCandidates(
        parkingLabels(buildings, new Map([["10,20", n]])),
        camera,
        vp,
        STAGE_NEAR,
        measure,
      );
    const [a] = at(12);
    const [b] = at(13);
    expect(a?.text).toBe("P 12");
    expect(b?.text).toBe("P 13");
    expect(a?.id).toBe(b?.id);
    expect(a?.style).toMatchObject({ shield: true, badge: "#1a73e8", color: "#ffffff" });
  });

  test("寄りと細部の段だけ出す", () => {
    const labels = parkingLabels(buildings, new Map([["10,20", 5]]));
    const count = (stage: number) => parkingCandidates(labels, camera, vp, stage, measure).length;
    expect(count(STAGE_FAR)).toBe(0);
    expect(count(STAGE_STANDARD)).toBe(0);
    expect(count(STAGE_NEAR)).toBe(1);
    expect(count(STAGE_DETAIL)).toBe(1);
  });

  test("画面の外の建物は出さない", () => {
    const labels = parkingLabels(
      buildings,
      new Map([
        ["10,20", 5],
        ["30,40", 2],
      ]),
    );
    const out = parkingCandidates(labels, camera, vp, STAGE_NEAR, measure);
    expect(out.map((c) => c.text)).toEqual(["P 5"]);
  });

  test("ほかのラベルと重なるときは、優先の低い台数のほうを置かない", () => {
    const labels = parkingLabels(buildings, new Map([["10,20", 5]]));
    const [p] = parkingCandidates(labels, camera, vp, STAGE_NEAR, measure);
    if (p === undefined) throw new Error("候補がない");
    const road = candidate("road", 7, p.box);
    expect(placeLabels([p, road], new Set(), []).map((c) => c.id)).toEqual(["road"]);
  });
});
