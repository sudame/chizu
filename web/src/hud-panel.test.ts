import { describe, expect, it } from "vitest";
import { HUD_COLLAPSED_KEY, readCollapsed, writeCollapsed } from "./hud-panel.ts";

const memory = () => {
  const map = new Map<string, string>();
  return {
    map,
    getItem: (k: string) => map.get(k) ?? null,
    setItem: (k: string, v: string) => void map.set(k, v),
  };
};

describe("たたんだかどうかの記憶", () => {
  it("何も覚えていなければ、たたんだ状態にする", () => {
    expect(readCollapsed(memory())).toBe(true);
  });

  it("開いたことを覚えて読み戻せる", () => {
    const s = memory();
    writeCollapsed(s, false);
    expect(s.map.get(HUD_COLLAPSED_KEY)).toBe("0");
    expect(readCollapsed(s)).toBe(false);
    writeCollapsed(s, true);
    expect(readCollapsed(s)).toBe(true);
  });

  it("保存先がない、または読み書きで例外になっても動く", () => {
    const broken = {
      getItem: () => {
        throw new Error("denied");
      },
      setItem: () => {
        throw new Error("denied");
      },
    };
    expect(readCollapsed(null)).toBe(true);
    expect(readCollapsed(broken)).toBe(true);
    expect(() => writeCollapsed(null, false)).not.toThrow();
    expect(() => writeCollapsed(broken, false)).not.toThrow();
  });
});
