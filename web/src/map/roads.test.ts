import { describe, expect, test } from "vitest";
import { carriagewayHalfWidth, HIGHWAY_CODE, laneCenter } from "./lanes.ts";
import { hex } from "./palette.ts";
import {
  asphaltHalfWidth,
  buildingColor,
  buildingEdgeColor,
  buildingMeshes,
  CLUSTER_INSET,
  crosswalkArms,
  facilitySiteColor,
  facilitySiteEdgeColor,
  footprintOutline,
  Geometry,
  headingBit,
  MARKING_COLORS,
  MIN_PX,
  markCenter,
  miterTrim,
  ROAD_VISUAL_HALF,
  roadDrawOrder,
  roadFootprint,
  roadGeometry,
  roadLaneGeometry,
  roadMarkingGeometry,
  roadMeshes,
  roadStyle,
  SIDEWALK_COLOR,
  SIGNAL_COLOR,
  signalGeometry,
  signalLampCenter,
  THIN_PX,
  tilesOnLine,
  unsuppliedCount,
  VERTEX_FLOATS,
  ZONE_COLORS,
  zebra,
} from "./roads.ts";

const brightness = (c: readonly number[]) => (c[0] ?? 0) + (c[1] ?? 0) + (c[2] ?? 0);

describe("建物", () => {
  test("建物の色は区画ごとに決まり、区画の系統の色を保つ", () => {
    const [r1, g1, b1] = buildingColor(1);
    expect(g1).toBeGreaterThan(r1);
    expect(g1).toBeGreaterThan(b1);
    const [r2, g2, b2] = buildingColor(2);
    expect(b2).toBeGreaterThan(r2);
    expect(b2).toBeGreaterThan(g2);
    const [r3, g3, b3] = buildingColor(3);
    expect(r3).toBeGreaterThan(b3);
    expect(g3).toBeGreaterThan(b3);
    const [r4, g4, b4] = buildingColor(4);
    expect(g4).toBeGreaterThan(r4);
    expect(b4).toBeGreaterThan(r4);
    // どの区画も、建物は地面より濃く、縁は建物より濃い
    for (const zone of [1, 2, 3, 4, 5]) {
      const colors = ZONE_COLORS[zone];
      expect(brightness(buildingColor(zone))).toBeLessThan(brightness(colors?.ground ?? []));
      expect(brightness(buildingEdgeColor(zone))).toBeLessThan(brightness(buildingColor(zone)));
    }
  });

  test("低密度の住宅（5）は緑の系統のまま、高密度の住宅とも、ほかの区画とも、どの面でも見分けられる", () => {
    const low = ZONE_COLORS[5];
    expect(low).toBeDefined();
    // 緑の系統：緑が最も強く、赤が青より強い（工業の黄、商業とオフィスの青系とは別）
    const [r, g, b] = low?.edge ?? [0, 0, 0];
    expect(g).toBeGreaterThan(r);
    expect(r).toBeGreaterThan(b);
    const dist = (a: readonly number[], c: readonly number[]) =>
      Math.hypot((a[0] ?? 0) - (c[0] ?? 0), (a[1] ?? 0) - (c[1] ?? 0), (a[2] ?? 0) - (c[2] ?? 0));
    for (const other of [1, 2, 3, 4]) {
      for (const face of ["ground", "blockEmpty", "block", "building", "edge"] as const) {
        const d = dist(low?.[face] ?? [], ZONE_COLORS[other]?.[face] ?? []);
        expect(d, `区画 ${other} の ${face}`).toBeGreaterThan(0.04);
      }
    }
  });

  test("寄りの段では、建物ごとに影、縁、本体の 3 つの長方形を、少し内側に縮めて描く", () => {
    const m = buildingMeshes(
      new Float64Array([10, 20, 12, 21, 1, 1, 3, 30, 20, 30, 20, 3, 5, 3]),
      new Float64Array(),
    );
    // 影 2 つと縁 2 つ、本体 2 つ
    expect(m.details.vertexCount).toBe(2 * 2 * 6);
    expect(m.bodies.vertexCount).toBe(2 * 6);
    const data = m.details.build();
    // 影が先に 2 つ並び、本体と同じ位置から、右下へ画面で 2px ずれる（縮尺によらない）
    expect(data[0]).toBeCloseTo(0.15);
    expect([data[2], data[3], data[5]]).toEqual([1, 1, 2]);
    // 1 つめの建物の縁は、影 2 つのあと
    expect(data[12 * VERTEX_FLOATS]).toBeCloseTo(0.15);
    expect(data[13 * VERTEX_FLOATS]).toBeCloseTo(3 - 0.15);
  });

  test("標準の段の建物のまとまりは、縁を付けず、隣との隙間を細くする", () => {
    const m = buildingMeshes(new Float64Array([10, 20, 12, 21, 1, 1, 3]), new Float64Array());
    expect(m.clusters.vertexCount).toBe(6);
    const data = m.clusters.build();
    expect(data[0]).toBeCloseTo(CLUSTER_INSET);
    expect(data[1 * VERTEX_FLOATS]).toBeCloseTo(3 - CLUSTER_INSET);
    expect(CLUSTER_INSET).toBeLessThan(0.15);
    expect(Array.from(data.subarray(6, 9))).toEqual(
      Array.from(new Float32Array(buildingColor(1).slice(0, 3))),
    );
  });

  test("建物の縁は本体より暗く、色は区画の表の値にそろう", () => {
    const m = buildingMeshes(new Float64Array([0, 0, 0, 0, 1, 3, 3]), new Float64Array());
    const colorOf = (g: Geometry, vertex: number) =>
      Array.from(g.build().subarray(vertex * VERTEX_FLOATS + 6, vertex * VERTEX_FLOATS + 9));
    const [edge, body] = [colorOf(m.details, 6), colorOf(m.bodies, 0)];
    expect(brightness(edge)).toBeLessThan(brightness(body));
    expect(body).toEqual(Array.from(new Float32Array(buildingColor(1).slice(0, 3))));
    expect(edge).toEqual(Array.from(new Float32Array(buildingEdgeColor(1).slice(0, 3))));
  });

  test("施設は敷地の縁と面を描き、電気か水道の届かない建物には印を足す", () => {
    const facilities = new Float64Array([0, 0, 3, 3, 1]);
    // 電気も水道も届く、電気だけ届く、どちらも届かない
    const supplies = [3, 1, 0];
    const buildings = new Float64Array(
      supplies.flatMap((supply, i) => [10 + 2 * i, 20, 10 + 2 * i, 20, 1, 1, supply]),
    );
    const m = buildingMeshes(buildings, facilities);
    // 影は施設 1 つと建物 3 つ、縁は建物 3 つ
    expect(m.details.vertexCount).toBe(6 * (4 + 3));
    expect(m.facilities.vertexCount).toBe(6 * 5);
    // 印は水滴 2 つ（9 頂点）と稲妻 1 つ（6 頂点）
    expect(m.marks.vertexCount).toBe(9 * 2 + 6);
    expect(unsuppliedCount(buildings)).toBe(2);
    // 敷地は縁の 4 本と面を重ならないように置く。色は決まりの表の発電所の敷地の縁と面である
    const body = 4 * 6 * VERTEX_FLOATS;
    const facilityData = m.facilities.build();
    expect([...facilityData.subarray(6, 10)]).toEqual(
      [...facilitySiteEdgeColor(1)].map(Math.fround),
    );
    expect([...facilityData.subarray(body + 6, body + 10)]).toEqual(
      [...facilitySiteColor(1)].map(Math.fround),
    );
    expect(facilitySiteColor(1)).toEqual(hex("#f8e3cf"));
    // 印の最初が、電気だけ届く建物の水道の印。基準のマスは最初の建物の左上 (10, 20)
    const water = markCenter({ x: 12, y: 20 }, "water");
    const data = m.marks.build();
    expect(data[0]).toBeCloseTo(water.x - 10);
    expect(data[1]).toBeCloseTo(water.y - 20);
    expect(water).toEqual({ x: 12.75, y: 20.3 });
  });
});

describe("Geometry", () => {
  test("区間は 2 つの三角形で、位置は基準のマスからの相対でマスの中心に置く", () => {
    const g = new Geometry({ x: 100, y: 100 });
    g.segment({ x: 100, y: 100 }, { x: 104, y: 100 }, 0.3, [0, 0, 0, 1]);
    expect(g.vertexCount).toBe(6);
    const data = g.build();
    expect(data[0]).toBe(0.5);
    expect(data[1]).toBe(0.5);
    expect(data[3]).toBe(1);
    expect(data[4]).toBeCloseTo(0.3);
    expect(data[5]).toBe(MIN_PX);
    expect(data[2 * VERTEX_FLOATS]).toBe(4.5);
  });

  test("長さのない区間は描かない", () => {
    const g = new Geometry({ x: 0, y: 0 });
    g.segment({ x: 1, y: 1 }, { x: 1, y: 1 }, 0.3, [0, 0, 0, 1]);
    expect(g.vertexCount).toBe(0);
  });

  test("縮小したときの道路は、縁の色の帯の上に塗りの帯を重ね、ノードも同じ順に埋める", () => {
    const g = roadGeometry(
      new Float64Array([0, 0, 4, 0, 1]),
      new Float64Array([0, 0, 1, 0, 0, 4, 0, 1, 0, 0]),
    );
    // 縁の帯 1、ノードの縁の四角 2、車道の帯 1、ノードの車道の四角 2
    expect(g.vertexCount).toBe(6 * 6);
    const data = g.build();
    // 縁の帯は、塗りの帯と同じ世界の幅（小は 0.62 マスの半分）と下限で、画面で 1px を足す
    expect(data[4]).toBeCloseTo(0.31);
    expect(data[10]).toBe(1);
    expect(data[18 * VERTEX_FLOATS + 4]).toBeCloseTo(0.31);
    expect(data[18 * VERTEX_FLOATS + 10]).toBe(0);
    expect(roadGeometry(new Float64Array(), new Float64Array()).vertexCount).toBe(0);
  });

  test("縮小したときは、どの倍率でも、縁が塗りの両側に 1px 以上出る", () => {
    // 頂点シェーダーと同じ式で、半分の太さ（画面のピクセル）を求める
    const halfPx = (data: Float32Array, vertex: number, zoom: number) =>
      Math.max(
        (data[vertex * VERTEX_FLOATS + 4] ?? 0) * zoom,
        data[vertex * VERTEX_FLOATS + 5] ?? 0,
      ) + (data[vertex * VERTEX_FLOATS + 10] ?? 0);
    for (const kind of [1, 2, 3, HIGHWAY_CODE]) {
      const data = roadGeometry(
        new Float64Array([0, 0, 4, 0, kind]),
        new Float64Array([0, 0, 1, 0, 4, 0, 1, 0]),
      ).build();
      for (const zoom of [1, 2, 3, 4, 5, 5.9, 6]) {
        const [edge, fill] = [halfPx(data, 0, zoom), halfPx(data, 18, zoom)];
        expect(edge - fill).toBeGreaterThanOrEqual(1 - 1e-6);
        // 塗りは、決まりの表の幅と下限（2 倍の値）に従う
        const width = [0, 0.62, 0.8, 0.96, 1][kind] ?? 0;
        const lower = [0, 1.8, 2.2, 3.2, 4][kind] ?? 0;
        expect(fill * 2).toBeCloseTo(Math.max(lower, width * zoom));
      }
    }
  });

  test("縮小したときも、広い道路ほど車道の帯を太く残す", () => {
    const minOf = (kind: number) =>
      roadGeometry(
        new Float64Array([0, 0, 4, 0, kind]),
        new Float64Array([0, 0, 1, 0, 0, 4, 0, 1, 0, 0]),
      ).build()[18 * VERTEX_FLOATS + 5] ?? 0;
    const mins = [1, 2, 3, HIGHWAY_CODE].map(minOf);
    expect(mins).toEqual([...mins].sort((a, b) => a - b));
    expect(new Set(mins).size).toBe(4);
  });
});

describe("道路の見た目", () => {
  test("道路の塗りは小と中が白、大が黄、高速道路が橙で、縁は種類ごとに違う", () => {
    expect(roadStyle(1).asphalt).toEqual(roadStyle(2).asphalt);
    const [rl, gl, bl] = roadStyle(3).asphalt;
    expect(rl).toBeGreaterThan(bl);
    expect(gl).toBeGreaterThan(bl);
    const [rh, gh, bh] = roadStyle(HIGHWAY_CODE).asphalt;
    expect(rh).toBeGreaterThan(gh);
    expect(gh).toBeGreaterThan(bh);
    const edges = [1, 2, 3, HIGHWAY_CODE].map((kind) => roadStyle(kind).edge.join());
    expect(new Set(edges).size).toBe(4);
    // 高速道路の縁は歩道と違う色
    expect(roadStyle(HIGHWAY_CODE).edge).not.toEqual(SIDEWALK_COLOR);
  });

  test("小の道路には中央線も車線の区切りもない", () => {
    expect(MARKING_COLORS[1]).toBeUndefined();
    for (const kind of [2, 3, HIGHWAY_CODE]) {
      expect(MARKING_COLORS[kind]).toBeDefined();
    }
  });

  test("車道は車線の幅に合わせ、高速道路の路肩は外側の車線の車にかからない", () => {
    expect(asphaltHalfWidth(2)).toBeCloseTo(carriagewayHalfWidth(2));
    // 車の半分の幅は 0.065 マス（movers.ts）
    const outer = laneCenter(HIGHWAY_CODE, 0) + 0.065;
    expect(outer).toBeLessThanOrEqual(asphaltHalfWidth(HIGHWAY_CODE));
    expect(asphaltHalfWidth(HIGHWAY_CODE)).toBeLessThan(ROAD_VISUAL_HALF);
  });

  test("段ごとに描くものを替え、段の境目の幅でだけ両方を描く", () => {
    const drawn = (zoom: number) =>
      new Map(
        roadDrawOrder(zoom)
          .filter(([, alpha]) => alpha > 0)
          .map(([name, alpha]) => [name, alpha]),
      );
    const fadeOf = (zoom: number, name: string) =>
      roadDrawOrder(zoom).find(([n]) => n === name)?.[2];
    // 引き：線の道路だけ。小の道路は不透明のまま、色を地面に寄せて薄くする。
    // 建物は街区の面（シェーダー）で塗るので、まとまりも描かない
    const hiki = drawn(2);
    expect(hiki.get("minor")).toBe(1);
    expect(fadeOf(2, "minor")).toBeCloseTo(0.55);
    expect(fadeOf(8, "minor")).toBe(1);
    expect(fadeOf(2, "major")).toBe(1);
    expect(hiki.get("major")).toBe(1);
    for (const name of ["clusters", "bodies", "details", "lanes", "markings", "signals", "marks"]) {
      expect(hiki.has(name as never)).toBe(false);
    }
    // 標準：建物のまとまりと線の道路
    const hyojun = drawn(8);
    expect(hyojun.get("clusters")).toBe(1);
    expect(hyojun.get("minor")).toBe(1);
    for (const name of ["bodies", "details", "lanes", "markings", "signals"]) {
      expect(hyojun.has(name as never)).toBe(false);
    }
    // 寄り：1 軒ずつの建物と車線ごとの道路、路面の線と信号
    const yori = drawn(20);
    for (const name of ["bodies", "details", "lanes", "markings", "signals"]) {
      expect(yori.get(name as never)).toBe(1);
    }
    for (const name of ["clusters", "minor", "major"]) {
      expect(yori.has(name as never)).toBe(false);
    }
    // 標準と寄りのあいだは、両方を薄く重ねる
    const between = drawn(12);
    expect(between.get("major")).toBe(1);
    expect(between.get("lanes")).toBeGreaterThan(0);
    expect(between.get("lanes")).toBeLessThan(1);
    expect(between.get("clusters")).toBeGreaterThan(0);
    expect(between.get("bodies")).toBe(1);
    // 細部の段：寄りの描き方はそのまま描き、屋根の形、街路樹と停止線、3 つ並びの信号を描き足す
    expect(yori.has("roofs")).toBe(false);
    const saibu = drawn(128);
    for (const name of ["bodies", "details", "lanes", "markings", "roofs", "streets", "lamps"]) {
      expect(saibu.get(name as never)).toBe(1);
    }
    expect(saibu.has("signals")).toBe(false);
    // 32〜48 で溶け込む。1 つの灯火は 3 つの並びに入れ替わる
    const fading = drawn(40);
    expect(fading.get("roofs")).toBeCloseTo(0.5);
    expect(fading.get("signals")).toBeCloseTo(0.5);
    expect(fading.get("lamps")).toBeCloseTo(0.5);
    expect(fading.get("lanes")).toBe(1);
    const fineOrder = roadDrawOrder(128).map(([name]) => name);
    expect(fineOrder.indexOf("bodies")).toBeLessThan(fineOrder.indexOf("roofs"));
    expect(fineOrder.indexOf("roofs")).toBeLessThan(fineOrder.indexOf("marks"));
    expect(fineOrder.indexOf("markings")).toBeLessThan(fineOrder.indexOf("streets"));
    // 下から順に、建物、道路、路面の線、信号、仮の表示
    const order = roadDrawOrder(12).map(([name]) => name);
    expect(order.indexOf("bodies")).toBeLessThan(order.indexOf("major"));
    expect(order.indexOf("major")).toBeLessThan(order.indexOf("lanes"));
    expect(order.at(-1)).toBe("preview");
  });
});

describe("車線ごとの道路", () => {
  const nodesOf = (...tiles: [number, number][]) =>
    new Float64Array(tiles.flatMap(([x, y]) => [x, y, 1, 0, 0]));
  // 6 頂点ごとの塊の、i 番めの頂点の x と y
  const at = (g: Geometry, block: number, vertex: number) => {
    const data = g.build();
    const i = (block * 6 + vertex) * VERTEX_FLOATS;
    return { x: data[i] ?? 0, y: data[i + 1] ?? 0, half: data[i + 4] ?? 0, min: data[i + 5] ?? 0 };
  };

  test("中くらいの道路 1 本は、両端の四角、植え込み 2、歩道と外の縁の線 4、行き止まりの 3 辺ずつ、車道、縁石の線 2、両端の車道の四角と、線 3 本になる", () => {
    const segments = new Float64Array([0, 0, 4, 0, 2]);
    const nodes = nodesOf([0, 0], [4, 0]);
    expect(roadLaneGeometry(segments, nodes).vertexCount).toBe(6 * (2 + 2 + 4 + 2 * 3 + 1 + 2 + 2));
    expect(roadMarkingGeometry(segments, nodes).vertexCount).toBe(6 * 3);
  });

  test("歩道は車道の外に 0.08 マス、植え込みは歩道の外からマスの縁まで埋め、線は中心線の左右へ車線の幅ごとに並ぶ", () => {
    const segments = new Float64Array([0, 0, 4, 0, 2]);
    const nodes = nodesOf([0, 0], [4, 0]);
    const lanes = roadLaneGeometry(segments, nodes);
    // 両端の四角のあとが植え込み。中くらいの道路の車道の端は 0.32 で、歩道は 0.32〜0.40、植え込みは 0.40〜0.5 にある
    for (const block of [2, 3]) {
      const s = at(lanes, block, 0);
      expect(Math.abs(s.y - 0.5) - s.half).toBeCloseTo(0.4);
      expect(Math.abs(s.y - 0.5) + s.half).toBeCloseTo(ROAD_VISUAL_HALF);
    }
    for (const block of [4, 6]) {
      const s = at(lanes, block, 0);
      expect(Math.abs(s.y - 0.5) - s.half).toBeCloseTo(0.32);
      expect(Math.abs(s.y - 0.5) + s.half).toBeCloseTo(0.4);
    }
    // 歩道の外の縁の線は、歩道の外の端に沿う
    expect(Math.abs(at(lanes, 5, 0).y - 0.5)).toBeCloseTo(0.4);
    const markings = roadMarkingGeometry(segments, nodes);
    const ys = [0, 1, 2].map((block) => at(markings, block, 0).y.toFixed(2)).sort();
    expect(ys).toEqual(["0.34", "0.50", "0.66"]);
  });

  test("車道は実際の幅で描き、線は細くても画面で消えない", () => {
    const segments = new Float64Array([0, 0, 4, 0, 3]);
    const nodes = nodesOf([0, 0], [4, 0]);
    const lanes = roadLaneGeometry(segments, nodes);
    // 四角 2、植え込み 2、歩道と縁の線 4、行き止まりの縁の線 6 のあとが車道で、半分の太さは車線 3 本分（0.14 × 3）
    expect(at(lanes, 14, 0).half).toBeCloseTo(0.42);
    expect(at(lanes, 14, 0).min).toBe(0);
    expect(at(lanes, 5, 0).min).toBe(THIN_PX);
    expect(at(roadMarkingGeometry(segments, nodes), 0, 0).min).toBe(THIN_PX);
  });

  test("3 本以上が集まる交差点では、線を交差点の中を横切らないよう縮める", () => {
    // (4, 0) に 3 本が集まる
    const segments = new Float64Array([0, 0, 4, 0, 1, 4, 0, 8, 0, 1, 4, 0, 4, 4, 1]);
    const nodes = nodesOf([0, 0], [4, 0], [8, 0], [4, 4]);
    const lanes = roadLaneGeometry(segments, nodes);
    // 四角 4、植え込み 6、歩道と縁の線 12、交差点の腕のない北の辺 1 と行き止まり 3 つの 3 辺ずつ、車道 3、縁石の線 6、車道の四角 4
    expect(lanes.vertexCount).toBe(6 * (4 + 6 + 12 + 1 + 3 * 3 + 3 + 6 + 4));
    // 交差点のノードの北の辺の縁の線は、マスの上の縁に沿う。行き止まり (0,0) の 3 辺のあとにある
    const north = at(lanes, 4 + 6 + 12 + 3, 0);
    expect(north.y).toBeCloseTo(0);
    expect(north.x).toBeCloseTo(4);
    // 区間 (0,0)-(4,0) の歩道は、終点側がマスの縁で終わる
    expect(at(lanes, 10, 2).x).toBeCloseTo(4.5 - ROAD_VISUAL_HALF);
    // 小の道路には中央線がなく、交差点の 3 つの腕の横断歩道の縞（3 本ずつ）だけが描かれる
    expect(roadMarkingGeometry(segments, nodes).vertexCount).toBe(6 * 3 * 3);
    // 中くらいの道路なら、中央線と区切り 2 本が、終点側で交差点の道路の半分の幅（0.4）だけ手前で終わる
    const medium = new Float64Array([0, 0, 4, 0, 2, 4, 0, 8, 0, 2, 4, 0, 4, 4, 2]);
    const markings = roadMarkingGeometry(medium, nodes);
    // 区間ごとに中央線と区切り 2 本、交差点の 3 つの腕の横断歩道の縞（4 本ずつ）
    expect(markings.vertexCount).toBe(6 * (3 * 3 + 3 * 4));
    expect(at(markings, 0, 2).x).toBeCloseTo(4.5 - 0.4);
  });

  test("直角の曲がり角のマスは歩道の色で埋め、帯はそのマスの縁で止まる", () => {
    // 東へ行って、(4, 0) で南へ折れる
    const segments = new Float64Array([0, 0, 4, 0, 1, 4, 0, 4, 4, 1]);
    const g = roadLaneGeometry(segments, nodesOf([0, 0], [4, 0], [4, 4]));
    // 角のノードには、マスいっぱいの歩道の色の四角がある
    expect(at(g, 1, 0).x).toBeCloseTo(4.5);
    expect(at(g, 1, 0).half).toBeCloseTo(ROAD_VISUAL_HALF);
    expect(Array.from(g.build().subarray(VERTEX_FLOATS + 6, VERTEX_FLOATS + 9))).toEqual(
      Array.from(new Float32Array(SIDEWALK_COLOR.slice(0, 3))),
    );
    // 四角 3 のあとに、区間 0 の植え込み（北、南）が続き、終点側は角のマスの縁で終わる
    expect(at(g, 3, 2).x).toBeCloseTo(4.5 - ROAD_VISUAL_HALF);
    // 区間 1 の植え込み（東、西）は、始点側が角のマスの縁から始まる
    expect(at(g, 5, 0).y).toBeCloseTo(0.5 + ROAD_VISUAL_HALF);
  });

  test("まっすぐ通り抜けるノードのマスは埋めず、帯をつなげる", () => {
    // (4, 0) で種類が変わるだけの道路
    const segments = new Float64Array([0, 0, 4, 0, 1, 4, 0, 8, 0, 2]);
    const g = roadLaneGeometry(segments, nodesOf([0, 0], [4, 0], [8, 0]));
    // 四角は両端の 2 つだけ（通り抜けるノードの四角は描かない）
    expect(at(g, 0, 0).x).toBeCloseTo(0.5);
    expect(at(g, 1, 0).x).toBeCloseTo(8.5);
    // 区間 0 の植え込みは、終点側で縮めずノードの中心まで届く
    expect(at(g, 2, 2).x).toBeCloseTo(4.5);
  });

  test("縮小用と拡大用と路面の線の頂点を作る", () => {
    const meshes = roadMeshes(new Float64Array([0, 0, 4, 0, 1]), nodesOf([0, 0], [4, 0]));
    // 小の道路は、縮小用の小の道路の側にだけ入る
    expect(meshes.minor.vertexCount).toBe(36);
    expect(meshes.major.vertexCount).toBe(0);
    // 小と中が集まるノードは、中の側に入れ、薄い小の道路の上に濃く描く
    const mixed = roadMeshes(
      new Float64Array([0, 0, 4, 0, 1, 4, 0, 8, 0, 2]),
      nodesOf([0, 0], [4, 0], [8, 0]),
    );
    // 小：縁と塗りの帯 2、ノード (0, 0) の四角 2。中：帯 2、ノード (4, 0) と (8, 0) の四角 4
    expect(mixed.minor.vertexCount).toBe(6 * 4);
    expect(mixed.major.vertexCount).toBe(6 * 6);
    expect(meshes.lanes.vertexCount).toBeGreaterThan(36);
    // 小の道路には路面の線がない
    expect(meshes.markings.vertexCount).toBe(0);
    expect(
      roadMeshes(new Float64Array([0, 0, 4, 0, 2]), nodesOf([0, 0], [4, 0])).markings.vertexCount,
    ).toBe(18);
  });
});

describe("miterTrim", () => {
  const east = { x: 1, y: 0 };
  const south = { x: 0, y: 1 };
  const north = { x: 0, y: -1 };

  test("まっすぐなら縮めない", () => {
    expect(miterTrim(east, east, 0.26)).toBeCloseTo(0);
  });

  test("曲がる側の内側は縮み、外側は伸びる", () => {
    // 東から南へ曲がるのは、画面で右へ曲がる。右（南）の側が内側である
    expect(miterTrim(east, south, 0.26)).toBeCloseTo(0.26);
    expect(miterTrim(east, south, -0.26)).toBeCloseTo(-0.26);
    expect(miterTrim(east, north, 0.26)).toBeCloseTo(-0.26);
  });

  test("折り返しでは縮めも伸ばしもしない", () => {
    expect(miterTrim(east, { x: -1, y: 0 }, 0.26)).toBe(0);
  });

  test("45 度の折れ曲がりは tan(22.5 度) 倍", () => {
    const d = { x: Math.SQRT1_2, y: Math.SQRT1_2 };
    expect(miterTrim(east, d, 0.2)).toBeCloseTo(0.2 * Math.tan(Math.PI / 8));
  });
});

describe("信号", () => {
  test("灯火は腕の側の、入ってくる車線の側に置く", () => {
    // 東の腕から入る車は西へ進むので、左側通行では南（y の大きい側）の車線を走る
    const east = signalLampCenter(10, 10, 1, 0);
    expect(east.x).toBeGreaterThan(10.5);
    expect(east.y).toBeGreaterThan(10.5);
    // 北の腕から入る車は南へ進み、東（x の大きい側）の車線を走る
    const north = signalLampCenter(10, 10, 0, -1);
    expect(north.y).toBeLessThan(10.5);
    expect(north.x).toBeGreaterThan(10.5);
  });

  test("腕ごとに台と灯火の 2 つの四角を、灯火の色で描く", () => {
    const lights = new Float64Array([4, 5, 1, 0, 0, 4, 5, 0, 1, 2]);
    const g = signalGeometry(lights);
    expect(g.vertexCount).toBe(2 * 2 * 6);
    const data = g.build();
    const colorAt = (vertex: number) =>
      Array.from(data.subarray(vertex * VERTEX_FLOATS + 6, vertex * VERTEX_FLOATS + 10));
    // 1 本めの灯火（7 番めの頂点から）は青、2 本めは赤
    expect(colorAt(6)).toEqual(Array.from(new Float32Array(SIGNAL_COLOR[0] ?? [])));
    expect(colorAt(18)).toEqual(Array.from(new Float32Array(SIGNAL_COLOR[2] ?? [])));
  });
});

describe("横断歩道", () => {
  test("交差点の四角の外に、車道の幅に縞を並べる", () => {
    const g = new Geometry({ x: 0, y: 0 });
    zebra(g, { x: 0, y: 0 }, { dx: 1, dy: 0 }, 0.3, 0.2);
    // 車道の幅 0.4 マスに、0.14 マスおきの縞が 2 本
    expect(g.vertexCount).toBe(2 * 6);
    const data = g.build();
    for (let v = 0; v < g.vertexCount; v++) {
      const x = data[v * VERTEX_FLOATS] ?? 0;
      expect(x).toBeGreaterThanOrEqual(0.5 + 0.3 - 1e-6);
      expect(x).toBeLessThanOrEqual(0.5 + 0.3 + 0.16 + 1e-6);
    }
  });

  test("縞は路面の線と同じく、拡大したときだけ描く", () => {
    const segments = new Float64Array([0, 0, 4, 0, 1, 4, 0, 8, 0, 1, 4, 0, 4, 4, 1]);
    const nodes = new Float64Array([0, 0, 1, 0, 0, 4, 0, 1, 0, 0, 8, 0, 1, 0, 0, 4, 4, 1, 0, 0]);
    // 車線ごとの道路には縞を入れず、路面の線の頂点に入れる
    expect(roadLaneGeometry(segments, nodes).vertexCount).toBe(
      6 * (4 + 6 + 12 + 1 + 9 + 3 + 6 + 4),
    );
    const last = roadMarkingGeometry(segments, nodes).build();
    const v = last.length / VERTEX_FLOATS - 1;
    expect(last[v * VERTEX_FLOATS + 5]).toBe(THIN_PX);
  });

  // (4, 0) に西、東、南の 3 本が集まる
  const tee = new Float64Array([0, 0, 4, 0, 1, 4, 0, 8, 0, 1, 4, 0, 4, 4, 1]);
  const teeNodes = (mask: number) =>
    new Float64Array([0, 0, 1, 0, 0, 4, 0, 3, 0, mask, 8, 0, 1, 0, 0, 4, 4, 1, 0, 0]);
  // 南の腕（Heading の S は 2）
  const SOUTH = 1 << 2;

  test("横断歩道を外した腕には縞を描かない", () => {
    const all = roadMarkingGeometry(tee, teeNodes(0)).vertexCount;
    const without = roadMarkingGeometry(tee, teeNodes(SOUTH)).vertexCount;
    // 小さい道路の縞は 3 本
    expect(all - without).toBe(3 * 6);
  });

  test("腕の向きのビットは chizu-core の Heading の順に並ぶ", () => {
    expect(headingBit({ dx: 1, dy: 0 })).toBe(1);
    expect(headingBit({ dx: Math.SQRT1_2, dy: Math.SQRT1_2 })).toBe(2);
    expect(headingBit({ dx: 0, dy: 1 })).toBe(SOUTH);
    expect(headingBit({ dx: 0, dy: -1 })).toBe(1 << 6);
    expect(headingBit({ dx: Math.SQRT1_2, dy: -Math.SQRT1_2 })).toBe(1 << 7);
  });

  test("横断歩道を置ける腕を、外したかと縞の真ん中とともに並べる", () => {
    const arms = crosswalkArms(tee, teeNodes(SOUTH));
    expect(arms.map((a) => [a.x, a.y, a.dx, a.dy, a.removed])).toEqual([
      [4, 0, -1, 0, false],
      [4, 0, 1, 0, false],
      [4, 0, 0, 1, true],
    ]);
    const south = arms[2];
    expect(south?.cx).toBeCloseTo(4.5);
    expect(south?.cy).toBeGreaterThan(0.5 + 0.3);
    expect(south?.cy).toBeLessThan(0.5 + 0.3 + 0.16);
  });
});

describe("斜めの道路", () => {
  const segments = new Float64Array([0, 0, 4, 4, 3]);
  const nodes = new Float64Array([0, 0, 1, 0, 0, 4, 4, 1, 0, 0]);
  // 頂点を広げたあとの位置（マス）
  const corners = (g: Geometry) => {
    const d = g.build();
    const out: { x: number; y: number }[] = [];
    for (let v = 0; v < g.vertexCount; v++) {
      const o = v * VERTEX_FLOATS;
      const half = d[o + 4] ?? 0;
      out.push({
        x: (d[o] ?? 0) + (d[o + 2] ?? 0) * half,
        y: (d[o + 1] ?? 0) + (d[o + 3] ?? 0) * half,
      });
    }
    return out;
  };

  test("通るマスは縦横と斜めの線の上のマスである", () => {
    expect(tilesOnLine({ x: 0, y: 0 }, { x: 3, y: -3 })).toEqual([
      { x: 0, y: 0 },
      { x: 1, y: -1 },
      { x: 2, y: -2 },
      { x: 3, y: -3 },
    ]);
    expect(tilesOnLine({ x: 2, y: 5 }, { x: 0, y: 5 })).toHaveLength(3);
  });

  test("脇のマスの建物（角から 0.15 マス内側）に、道路の帯がかからない", () => {
    // 三角形のどれかが点 p を覆うか
    const covers = (g: Geometry, p: { x: number; y: number }) => {
      const c = corners(g);
      const cross = (a: typeof p, b: typeof p) =>
        (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
      for (let i = 0; i + 2 < c.length; i += 3) {
        const [a, b, d] = [c[i], c[i + 1], c[i + 2]] as [typeof p, typeof p, typeof p];
        const [s1, s2, s3] = [cross(a, b), cross(b, d), cross(d, a)];
        if ((s1 > 0 && s2 > 0 && s3 > 0) || (s1 < 0 && s2 < 0 && s3 < 0)) {
          return true;
        }
      }
      return false;
    };
    for (const g of [roadLaneGeometry(segments, nodes), roadGeometry(segments, nodes)]) {
      for (let k = 0; k < 4; k++) {
        // マス (k+1, k) と (k, k+1) の建物の、道路にいちばん近い角
        expect(covers(g, { x: k + 1.15, y: k + 0.85 })).toBe(false);
        expect(covers(g, { x: k + 0.85, y: k + 1.15 })).toBe(false);
      }
    }
    // 拡大したときは、道路のマスの外側の角まで塗る
    expect(covers(roadLaneGeometry(segments, nodes), { x: 1.05, y: 1.9 })).toBe(true);
  });

  test("歩道は道路のマスを階段状に塗る", () => {
    const g = new Geometry({ x: 0, y: 0 });
    roadFootprint(g, { x: 0, y: 0 }, { x: 2, y: 2 }, [1, 1, 1, 1]);
    expect(g.vertexCount).toBe(3 * 6);
    const xs = corners(g).map((p) => `${p.x},${p.y}`);
    expect(xs).toContain("0,0");
    expect(xs).toContain("3,3");
    expect(xs).not.toContain("2,0");
  });
});

describe("縁", () => {
  test("長方形の縁は 4 辺で、破線の周期は頂点に入り、辺に沿った位置が付く", () => {
    const g = new Geometry({ x: 0, y: 0 });
    g.outline(1, 2, 4, 3, [1, 0, 0, 1], 1.5, 8);
    expect(g.vertexCount).toBe(4 * 6);
    const data = g.build();
    // 上の辺は横なので、辺に沿った位置は x
    expect(data[11]).toBe(1);
    expect(data[12]).toBe(8);
    // 太さは画面のピクセルで決まる（半分の太さの下限が 0.75）
    expect(data[5]).toBeCloseTo(0.75);
    const solid = new Geometry({ x: 0, y: 0 });
    solid.outline(0, 0, 1, 1, [1, 0, 0, 1], 2);
    expect(solid.build()[12]).toBe(0);
  });

  test("縦横の線の縁は 1 つの長方形、斜めの線の縁は通るマスごとになる", () => {
    const straight = new Geometry({ x: 0, y: 0 });
    footprintOutline(straight, { x: 0, y: 0 }, { x: 5, y: 0 }, [0, 0, 1, 1], 1.5);
    expect(straight.vertexCount).toBe(24);
    const diagonal = new Geometry({ x: 0, y: 0 });
    footprintOutline(diagonal, { x: 0, y: 0 }, { x: 2, y: 2 }, [0, 0, 1, 1], 1.5);
    expect(diagonal.vertexCount).toBe(3 * 24);
  });
});
