import { describe, expect, test } from "vitest";
import {
  CHUNK_SIZE,
  chunkOf,
  DISTRICT_SUFFIXES,
  districtName,
  facilityGroupNames,
  facilityName,
  NAME_WORDS,
  normalizeSeed,
  roadLineKey,
  roadName,
  segmentRoadName,
} from "./names.ts";

const p = (x: number, y: number) => ({ x, y });

describe("名前の表", () => {
  test("語は 64 個で、重ならない", () => {
    expect(NAME_WORDS).toHaveLength(64);
    expect(new Set(NAME_WORDS).size).toBe(64);
  });
});

describe("roadLineKey", () => {
  test("向きごとに鍵が決まる", () => {
    expect(roadLineKey(p(3, 7), p(9, 7))).toBe("H7");
    expect(roadLineKey(p(4, 1), p(4, 9))).toBe("V4");
    expect(roadLineKey(p(5, 2), p(8, 5))).toBe("D3");
    expect(roadLineKey(p(5, 8), p(8, 5))).toBe("A13");
  });

  test("負の座標でも鍵が決まる", () => {
    expect(roadLineKey(p(-5, -3), p(2, -3))).toBe("H-3");
    expect(roadLineKey(p(-4, -9), p(-4, 3))).toBe("V-4");
    expect(roadLineKey(p(-3, -5), p(0, -2))).toBe("D2");
    expect(roadLineKey(p(-5, 2), p(-2, -1))).toBe("A-3");
  });

  test("縦と斜めも、端点を逆にしても同じ鍵になる", () => {
    expect(roadLineKey(p(4, 9), p(4, 1))).toBe(roadLineKey(p(4, 1), p(4, 9)));
    expect(roadLineKey(p(8, 5), p(5, 2))).toBe(roadLineKey(p(5, 2), p(8, 5)));
    expect(roadLineKey(p(5, 8), p(8, 5))).toBe(roadLineKey(p(8, 5), p(5, 8)));
    expect(roadLineKey(p(-2, -1), p(-5, 2))).toBe(roadLineKey(p(-5, 2), p(-2, -1)));
  });

  test("区間を分けても、端点の向きを逆にしても同じ鍵になる", () => {
    expect(roadLineKey(p(9, 7), p(3, 7))).toBe(roadLineKey(p(3, 7), p(6, 7)));
    expect(roadLineKey(p(8, 5), p(5, 8))).toBe(roadLineKey(p(6, 7), p(5, 8)));
  });
});

describe("chunkOf", () => {
  test("負のマスも、切り下げでチャンクに入る", () => {
    expect(chunkOf(0)).toBe(0);
    expect(chunkOf(63)).toBe(0);
    expect(chunkOf(64)).toBe(1);
    for (let t = -1; t >= -64; t--) expect(chunkOf(t)).toBe(-1);
    expect(chunkOf(-65)).toBe(-2);
  });
});

describe("roadName", () => {
  test("同じ入力からは同じ名前になる", () => {
    expect(roadName(0, "H10", 1)).toBe(roadName(0, "H10", 1));
  });

  test("区間を分けても、名前は変わらない", () => {
    const whole = segmentRoadName(42, p(0, 12), p(40, 12), 2);
    expect(segmentRoadName(42, p(0, 12), p(17, 12), 2)).toBe(whole);
    expect(segmentRoadName(42, p(17, 12), p(40, 12), 2)).toBe(whole);
    expect(segmentRoadName(42, p(40, 12), p(25, 12), 2)).toBe(whole);
  });

  test("種類を上げたときは、末尾だけが変わる", () => {
    const small = roadName(42, "V9", 1);
    const medium = roadName(42, "V9", 2);
    const large = roadName(42, "V9", 3);
    const highway = roadName(42, "V9", 4);
    expect(medium).toBe(small);
    expect(small.endsWith("通り")).toBe(true);
    const stem = small.slice(0, -"通り".length);
    expect(large).toBe(`${stem}大通り`);
    expect(highway).toBe(`${stem}自動車道`);
  });

  test("種を変えると、名前が変わる", () => {
    const keys = Array.from({ length: 40 }, (_, i) => `H${i}`);
    const changed = keys.filter((k) => roadName(0, k, 1) !== roadName(42, k, 1));
    expect(changed.length).toBeGreaterThan(30);
  });

  test("種は数、bigint、文字列（先頭の 0 つきも）のどれでも同じ名前になる", () => {
    expect(roadName(42, "H1", 1)).toBe(roadName(42n, "H1", 1));
    expect(roadName(42, "H1", 1)).toBe(roadName("42", "H1", 1));
    expect(roadName(42, "H1", 1)).toBe(roadName("042", "H1", 1));
    expect(districtName(42, 3, 4)).toBe(districtName("042", 3, 4));
    expect(districtName(42, 3, 4)).toBe(districtName(42n, 3, 4));
  });

  test("安全な整数でない数と、10 進でない文字列は例外になる", () => {
    expect(() => roadName(2 ** 53, "H1", 1)).toThrow(RangeError);
    expect(() => districtName(1.5, 0, 0)).toThrow(RangeError);
    expect(() => roadName("abc", "H1", 1)).toThrow(RangeError);
    expect(normalizeSeed(Number.MAX_SAFE_INTEGER)).toBe("9007199254740991");
  });

  test("近くの直線の名前は、ほとんど重ならない", () => {
    const names = new Set(Array.from({ length: 12 }, (_, i) => roadName(7, `H${i * 4}`, 1)));
    expect(names.size).toBeGreaterThanOrEqual(10);
  });
});

describe("districtName", () => {
  test("同じ入力からは同じ名前になる", () => {
    expect(districtName(0, 3, -2)).toBe(districtName(0, 3, -2));
  });

  test("隣り合うチャンクの名前は、8 近傍で重ならない", () => {
    for (const seed of [0, 1, 42, 123456789, 18446744073709551615n]) {
      for (let cx = -6; cx <= 6; cx++) {
        for (let cy = -6; cy <= 6; cy++) {
          const here = districtName(seed, cx, cy);
          for (let dx = -1; dx <= 1; dx++) {
            for (let dy = -1; dy <= 1; dy++) {
              if (dx === 0 && dy === 0) continue;
              expect(districtName(seed, cx + dx, cy + dy)).not.toBe(here);
            }
          }
        }
      }
    }
  });

  test("16 × 16 チャンクの範囲で、地区名がそっくり同じにならない", () => {
    for (const seed of [0, 1, 42, 99999, 18446744073709551615n]) {
      for (const [ox, oy] of [
        [0, 0],
        [-8, -8],
        [-16, 5],
        [37, -41],
      ] as const) {
        const names = new Set<string>();
        for (let cx = 0; cx < 16; cx++) {
          for (let cy = 0; cy < 16; cy++) names.add(districtName(seed, ox + cx, oy + cy));
        }
        expect(names.size).toBe(256);
      }
    }
  });

  test("種を変えると、名前が変わる", () => {
    const cells = Array.from({ length: 16 }, (_, i) => [i % 4, Math.floor(i / 4)] as const);
    const changed = cells.filter(([x, y]) => districtName(0, x, y) !== districtName(42, x, y));
    expect(changed.length).toBeGreaterThan(12);
  });

  test("水辺のチャンクは「浜」で終わり、そうでなければ決まった表の後半になる", () => {
    expect(districtName(0, 1, 1, true).endsWith("浜")).toBe(true);
    const name = districtName(0, 1, 1, false);
    expect(DISTRICT_SUFFIXES.some((s) => name.endsWith(s))).toBe(true);
  });
});

describe("facilityName", () => {
  test("地区の名前と種類から作る", () => {
    expect(facilityName("大和町", 2)).toBe("大和町浄水場");
    expect(facilityName("大和町", 1)).toBe("大和町発電所");
  });

  test("同じ地区の同じ種類は、左上の座標の順に第二、第三を付ける", () => {
    const names = facilityGroupNames(0, [
      { x: 30, y: 5, kind: 2 },
      { x: 10, y: 5, kind: 2 },
      { x: 20, y: 40, kind: 2 },
      { x: 12, y: 12, kind: 1 },
    ]);
    const district = districtName(0, 0, 0);
    expect(names).toEqual([
      `${district}第二浄水場`,
      `${district}浄水場`,
      `${district}第三浄水場`,
      `${district}発電所`,
    ]);
  });

  test("同じ行では左が先になる", () => {
    const [right, left] = facilityGroupNames(0, [
      { x: 30, y: 5, kind: 2 },
      { x: 10, y: 5, kind: 2 },
    ]);
    expect(left).toBe(`${districtName(0, 0, 0)}浄水場`);
    expect(right).toBe(`${districtName(0, 0, 0)}第二浄水場`);
  });

  test("11 番目からも漢数字になる", () => {
    expect(facilityName("大和町", 2, 10)).toBe("大和町第十一浄水場");
    expect(facilityName("大和町", 2, 19)).toBe("大和町第二十浄水場");
    expect(facilityName("大和町", 2, 20)).toBe("大和町第二十一浄水場");
    expect(facilityName("大和町", 2, 9)).toBe("大和町第十浄水場");
  });

  test("別の地区の同じ種類は、第二を付けない", () => {
    const [a, b] = facilityGroupNames(0, [
      { x: 1, y: 1, kind: 2 },
      { x: CHUNK_SIZE + 1, y: 1, kind: 2 },
    ]);
    expect(a).toBe(`${districtName(0, 0, 0)}浄水場`);
    expect(b).toBe(`${districtName(0, 1, 0)}浄水場`);
  });

  test("右に足したまとまりは、先にあったまとまりの名前を変えない", () => {
    const before = facilityGroupNames(0, [{ x: 10, y: 10, kind: 2 }]);
    const after = facilityGroupNames(0, [
      { x: 10, y: 10, kind: 2 },
      { x: 30, y: 10, kind: 2 },
    ]);
    expect(after[0]).toBe(before[0]);
  });
});
