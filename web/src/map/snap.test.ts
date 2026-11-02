import { describe, expect, test } from "vitest";
import { HIGHWAY_CODE } from "./lanes.ts";
import {
  buildRoadIndex,
  DISCONNECTED_NOTICE,
  EMPTY_ROAD_INDEX,
  isDisconnected,
  isRefused,
  isRoadTile,
  roadEndpoints,
  snapRoadEnd,
  snapRoadStart,
} from "./snap.ts";

// 区間は [ax, ay, bx, by, 種類]。(10, 10) から (20, 10) への道路と、(20, 10) から (20, 20) への道路
const roads = buildRoadIndex(
  new Float64Array([10, 10, 20, 10, 0, 20, 10, 20, 20, 0]),
  new Float64Array([10, 10, 20, 10, 20, 20]),
);
const t = (x: number, y: number) => ({ x, y });

describe("buildRoadIndex", () => {
  test("区間の途中のマスも道路として数える", () => {
    expect(isRoadTile(roads, t(15, 10))).toBe(true);
    expect(isRoadTile(roads, t(15, 11))).toBe(false);
    expect(isRoadTile(roads, t(20, 15))).toBe(true);
  });

  test("斜めの区間のマスも数える", () => {
    const diagonal = buildRoadIndex(new Float64Array([0, 0, 4, 4, 0]), new Float64Array());
    expect(isRoadTile(diagonal, t(2, 2))).toBe(true);
    expect(isRoadTile(diagonal, t(2, 3))).toBe(false);
  });
});

describe("snapRoadStart", () => {
  test("1 マス離れた道路の端に吸い付ける", () => {
    expect(snapRoadStart(roads, t(9, 10))).toEqual(t(10, 10));
    expect(snapRoadStart(roads, t(9, 9))).toEqual(t(10, 10));
  });

  test("区間の 1 マス脇なら、区間の上のマスに吸い付ける", () => {
    expect(snapRoadStart(roads, t(15, 11))).toEqual(t(15, 10));
  });

  test("端が範囲にあれば、区間の上のマスより端を選ぶ", () => {
    expect(snapRoadStart(roads, t(11, 11))).toEqual(t(10, 10));
    expect(snapRoadStart(roads, t(19, 11))).toEqual(t(20, 10));
  });

  test("道路の上ならそのまま、2 マス以上離れていれば吸い付けない", () => {
    expect(snapRoadStart(roads, t(12, 10))).toEqual(t(12, 10));
    expect(snapRoadStart(roads, t(8, 10))).toEqual(t(8, 10));
    expect(snapRoadStart(roads, t(15, 12))).toEqual(t(15, 12));
  });

  test("高速道路の途中には吸い付けない", () => {
    const highway = buildRoadIndex(
      new Float64Array([0, 0, 10, 0, HIGHWAY_CODE]),
      new Float64Array([0, 0, 1, 0, 10, 0, 1, 1]),
    );
    expect(snapRoadStart(highway, t(5, 1))).toEqual(t(5, 1));
    // 内側の端には吸い付けるが、外の世界につながる外側の端には吸い付けない
    expect(snapRoadStart(highway, t(-1, 1))).toEqual(t(0, 0));
    expect(snapRoadStart(highway, t(11, 0))).toEqual(t(11, 0));
  });

  test("道路がなければそのまま", () => {
    expect(snapRoadStart(EMPTY_ROAD_INDEX, t(3, 3))).toEqual(t(3, 3));
  });
});

describe("snapRoadEnd", () => {
  test("既存の道路の端の 1 マス手前で離すと、その端につながる", () => {
    expect(snapRoadEnd(roads, t(0, 10), t(9, 10))).toEqual(t(10, 10));
    expect(snapRoadEnd(roads, t(0, 10), t(9, 11))).toEqual(t(10, 10));
  });

  test("吸い付けたあとも、始点から縦横か 45 度の直線になる", () => {
    for (const [sx, sy] of [
      [0, 0],
      [3, 17],
      [30, 4],
      [-5, 12],
    ] as const) {
      for (let cx = 5; cx <= 25; cx++) {
        for (let cy = 5; cy <= 25; cy++) {
          const end = snapRoadEnd(roads, t(sx ?? 0, sy ?? 0), t(cx, cy));
          const dx = Math.abs(end.x - (sx ?? 0));
          const dy = Math.abs(end.y - (sy ?? 0));
          expect(dx === 0 || dy === 0 || dx === dy).toBe(true);
        }
      }
    }
  });

  test("直線にできない道路には吸い付けない", () => {
    // 始点 (0, 2) から斜めに引いた終点 (9, 11) の隣の端 (10, 10) は、始点から直線の外にある
    expect(snapRoadEnd(roads, t(0, 2), t(9, 11))).toEqual(t(9, 11));
  });

  test("斜めに引いても、直線の上の端に吸い付く", () => {
    expect(snapRoadEnd(roads, t(0, 0), t(9, 9))).toEqual(t(10, 10));
  });

  test("始点そのものには吸い付けない", () => {
    expect(snapRoadEnd(roads, t(10, 10), t(9, 10))).toEqual(t(9, 10));
  });

  test("終点がすでに道路の上ならそのまま", () => {
    expect(snapRoadEnd(roads, t(0, 10), t(12, 10))).toEqual(t(12, 10));
  });
});

describe("isDisconnected", () => {
  test("どの道路にも触れない道路はつながらない", () => {
    expect(isDisconnected(roads, t(10, 12), t(19, 12))).toBe(true);
    // 1 マス離れた平行な道路もつながらない
    expect(isDisconnected(roads, t(10, 11), t(19, 11))).toBe(true);
  });

  test("端や途中で既存の道路に触れればつながる", () => {
    expect(isDisconnected(roads, t(0, 10), t(10, 10))).toBe(false);
    expect(isDisconnected(roads, t(15, 5), t(15, 15))).toBe(false);
  });

  test("道路が 1 本もない街では知らせない", () => {
    expect(isDisconnected(EMPTY_ROAD_INDEX, t(0, 0), t(5, 0))).toBe(false);
  });

  test("高速道路の途中を横切る道路は、つながるとは数えない", () => {
    const highway = buildRoadIndex(
      new Float64Array([0, 0, 10, 0, HIGHWAY_CODE]),
      new Float64Array([0, 0, 1, 0, 10, 0, 1, 1]),
    );
    expect(isDisconnected(highway, t(5, -3), t(5, 3))).toBe(true);
  });
});

describe("知らせの文言", () => {
  test("電気と水道が届かないことと、吸い付きを切る方法を伝える", () => {
    expect(DISCONNECTED_NOTICE).toContain("つながらない");
    expect(DISCONNECTED_NOTICE).toContain("電気と水道");
    expect(DISCONNECTED_NOTICE).toContain("Alt");
  });
});

describe("既存の道路と重なる吸い付けはしない", () => {
  test("道路の上の始点から 1 マス脇へ出す枝は、元の道路に沿って吸い付かない", () => {
    expect(snapRoadEnd(roads, t(15, 10), t(15, 11))).toEqual(t(15, 11));
    expect(snapRoadEnd(roads, t(14, 10), t(14, 11))).toEqual(t(14, 11));
    expect(snapRoadEnd(roads, t(16, 10), t(16, 9))).toEqual(t(16, 9));
  });

  test("同じ道路の端の手前でも、沿ってしまうなら吸い付かない", () => {
    // (11, 10) から (10, 10) へは、(10, 10)–(20, 10) に沿った1歩になる
    expect(snapRoadEnd(roads, t(11, 10), t(10, 11))).toEqual(t(10, 11));
  });

  test("沿わない吸い付けは今までどおりする", () => {
    expect(snapRoadEnd(roads, t(15, 5), t(15, 9))).toEqual(t(15, 10));
  });

  test("斜めの道路の角で交わる吸い付けもしない", () => {
    const diagonal = buildRoadIndex(
      new Float64Array([0, 0, 4, 4, 0, 4, 0, 0, 4, 0]),
      new Float64Array([0, 0, 1, 0, 4, 4, 1, 0, 4, 0, 1, 0, 0, 4, 1, 0]),
    );
    expect(isRefused(diagonal, t(1, 0), t(0, 1))).toBe(true);
    expect(isRefused(diagonal, t(10, 10), t(12, 10))).toBe(false);
  });

  test("高速道路の途中を通る道路は断られると数える", () => {
    const highway = buildRoadIndex(
      new Float64Array([0, 0, 10, 0, HIGHWAY_CODE]),
      new Float64Array([0, 0, 1, 0, 10, 0, 1, 1]),
    );
    expect(isRefused(highway, t(5, -2), t(5, 2))).toBe(true);
    expect(isRefused(highway, t(0, -2), t(0, 2))).toBe(false);
  });
});

describe("roadEndpoints", () => {
  test("始点がなければ、カーソルを吸い付けた始点を返す", () => {
    expect(roadEndpoints(roads, null, t(9, 10), false)).toEqual({ from: t(10, 10), to: t(10, 10) });
  });

  test("始点があれば、終点を吸い付ける", () => {
    expect(roadEndpoints(roads, t(0, 10), t(9, 10), false)).toEqual({
      from: t(0, 10),
      to: t(10, 10),
    });
  });

  test("Alt を押しているあいだは吸い付けず、8 方向に寄せるだけにする", () => {
    expect(roadEndpoints(roads, null, t(9, 10), true)).toEqual({ from: t(9, 10), to: t(9, 10) });
    expect(roadEndpoints(roads, t(0, 10), t(9, 10), true)).toEqual({
      from: t(0, 10),
      to: t(9, 10),
    });
  });
});
