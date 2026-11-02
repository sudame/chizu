import { describe, expect, test } from "vitest";
import {
  CAR_COLORS,
  carColorIndex,
  MOVER_MATCH_TOLERANCE,
  moverColorAt,
  WALKER_COLOR,
} from "./mover-colors.ts";
import {
  GROUND_COLOR,
  SHORE_COLOR,
  UNOWNED_COLOR,
  UNOWNED_HATCH_COLOR,
  UNOWNED_SHORE_COLOR,
  UNOWNED_WATER_COLOR,
  WATER_COLOR,
} from "./palette.ts";
import {
  type Color,
  MARKING_COLORS,
  PLANTING_COLOR,
  ROAD_STYLES,
  SIDEWALK_COLOR,
  SIDEWALK_EDGE_COLOR,
  SIGNAL_BASE_COLOR,
  SIGNAL_COLOR,
  ZEBRA_COLOR,
  ZONE_COLORS,
} from "./roads.ts";

/** 半透明の色 fg を bg の上に重ねた色（0〜255）。 */
const over = (fg: readonly number[], bg: readonly number[]) =>
  [0, 1, 2].map((i) => ((fg[i] ?? 0) * (fg[3] ?? 1) + (bg[i] ?? 0) * (1 - (fg[3] ?? 1))) * 255);

const to255 = (c: readonly number[]) => [0, 1, 2].map((i) => (c[i] ?? 0) * 255);

/** 移動体がいないときに、移動体の位置に描かれうる地面と道路の色（0〜255）。 */
function roadSurfaces(): number[][] {
  const out: number[][] = [];
  for (const c of [
    GROUND_COLOR,
    UNOWNED_COLOR,
    UNOWNED_HATCH_COLOR,
    WATER_COLOR,
    SHORE_COLOR,
    UNOWNED_WATER_COLOR,
    UNOWNED_SHORE_COLOR,
    SIDEWALK_COLOR,
    SIDEWALK_EDGE_COLOR,
    PLANTING_COLOR,
    SIGNAL_BASE_COLOR,
    ...Object.values(SIGNAL_COLOR),
  ]) {
    out.push(to255(c));
  }
  for (const zone of Object.values(ZONE_COLORS)) {
    for (const c of [zone.ground, zone.building, zone.edge]) {
      out.push(to255(c));
    }
  }
  for (const [kind, style] of Object.entries(ROAD_STYLES)) {
    out.push(to255(style.asphalt), to255(style.edge));
    const lines = MARKING_COLORS[Number(kind)];
    for (const line of [lines?.center, lines?.divider, ZEBRA_COLOR] as (Color | undefined)[]) {
      if (line !== undefined) {
        out.push(over(line, style.asphalt));
      }
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
