import { describe, expect, test } from "vitest";
import { CAR_COLORS, carColorIndex, WALKER_COLOR } from "./mover-colors.ts";

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

  test("歩く人の色は、どの車の色とも違う", () => {
    for (const c of CAR_COLORS) {
      const d = c.reduce((sum, v, i) => sum + Math.abs(v - (WALKER_COLOR[i] ?? 0)), 0);
      expect(d).toBeGreaterThan(0.4);
    }
  });
});
