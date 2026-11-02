import { describe, expect, test } from "vitest";
import { HIGHWAY_CODE } from "./lanes.ts";
import {
  CAR_COLORS,
  carColorIndex,
  MOVER_MATCH_TOLERANCE,
  moverColorAt,
  WALKER_COLOR,
} from "./mover-colors.ts";
import {
  CENTER_LINE_COLOR,
  type Color,
  CURB_COLOR,
  DIVIDER_COLOR,
  GRASS_COLOR,
  roadStyle,
  SHOULDER_COLOR,
  SIDEWALK_COLOR,
  ZEBRA_COLOR,
} from "./roads.ts";

/** 半透明の色 fg を bg の上に重ねた色（0〜255）。 */
const over = (fg: Color, bg: readonly number[]) =>
  [0, 1, 2].map((i) => ((fg[i] ?? 0) * fg[3] + (bg[i] ?? 0) * (1 - fg[3])) * 255);

/** 移動体がいないときに、移動体の位置に描かれうる道路の色（0〜255）。 */
function roadSurfaces(): number[][] {
  const out: number[][] = [];
  for (const c of [GRASS_COLOR, SIDEWALK_COLOR, SHOULDER_COLOR, CURB_COLOR]) {
    out.push([0, 1, 2].map((i) => (c[i] ?? 0) * 255));
  }
  for (const kind of [1, 2, 3, HIGHWAY_CODE]) {
    const asphalt = roadStyle(kind).asphalt;
    out.push(over(asphalt, asphalt));
    for (const line of [CENTER_LINE_COLOR, DIVIDER_COLOR, ZEBRA_COLOR]) {
      out.push(over(line, asphalt));
    }
  }
  return out;
}

describe("移動体の色", () => {
  test("車の色は ID で決まり、続いた ID でもばらつく", () => {
    expect(carColorIndex(7)).toBe(carColorIndex(7));
    const used = new Set(Array.from({ length: 64 }, (_, id) => carColorIndex(id)));
    expect(used.size).toBe(CAR_COLORS.length);
    for (const id of [0, 1, 2 ** 31, 2 ** 32 - 1]) {
      expect(carColorIndex(id)).toBeGreaterThanOrEqual(0);
      expect(carColorIndex(id)).toBeLessThan(CAR_COLORS.length);
    }
  });

  test("シェーダーと同じく、32 ビットで桁あふれさせた積で選ぶ", () => {
    // 64 ビットに広げずに 32 ビットで丸めた計算を、BigInt で確かめる
    const mul = (a: bigint, b: bigint) => (a * b) & 0xffffffffn;
    for (const id of [1, 12345, 0xfffffff1]) {
      let h = mul(BigInt(id), 0x9e3779b1n);
      h = mul(h ^ (h >> 16n), 0x85ebca6bn);
      h ^= h >> 13n;
      expect(carColorIndex(id)).toBe(Number(h % BigInt(CAR_COLORS.length)));
    }
  });

  test("移動体がいなければ、道路のどの色も移動体の色と見なされない", () => {
    for (const bg of roadSurfaces()) {
      expect(moverColorAt(bg.map(Math.round))).toBe(false);
    }
    // 道路の色は、どの移動体の色とも、許す差の 2 倍より離れている
    for (const m of [...CAR_COLORS, WALKER_COLOR]) {
      for (const bg of roadSurfaces()) {
        const far = Math.max(...m.map((v, i) => Math.abs(v * 255 - (bg[i] ?? 0))));
        expect(far).toBeGreaterThan(2 * MOVER_MATCH_TOLERANCE);
      }
    }
  });

  test("移動体の色の画素は、移動体の色と見なされる", () => {
    for (const m of [...CAR_COLORS, WALKER_COLOR]) {
      expect(moverColorAt(m.map((v) => Math.round(v * 255)))).toBe(true);
    }
  });

  test("歩く人の色は、どの車の色とも違う", () => {
    for (const c of CAR_COLORS) {
      const d = c.reduce((sum, v, i) => sum + Math.abs(v - (WALKER_COLOR[i] ?? 0)), 0);
      expect(d).toBeGreaterThan(0.4);
    }
  });
});
