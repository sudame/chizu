import { describe, expect, test } from "vitest";
import { DATA_LAYERS, PALETTE_SIZE, PARKING_LAYER, TRAFFIC_LAYER } from "./layers.ts";
import {
  DEFAULT_THEME,
  DEMAND_MIN_SHARE,
  demandShare,
  loadTheme,
  saveTheme,
  THEME_STORAGE_KEY,
  THEMES,
  themeLegend,
  themeOf,
} from "./themes.ts";

/** localStorage の代わり。読み書きのたびに投げるようにもできる。 */
function memoryStorage(throws = false) {
  const values = new Map<string, string>();
  return {
    values,
    getItem(key: string): string | null {
      if (throws) {
        throw new Error("読めない");
      }
      return values.get(key) ?? null;
    },
    setItem(key: string, value: string): void {
      if (throws) {
        throw new Error("書けない");
      }
      values.set(key, value);
    },
  };
}

describe("主題図の選択", () => {
  test("なし、交通、電気、水道、駐車場、需要の順に並び、交通だけが交通のレイヤーを Worker に頼む", () => {
    expect(THEMES.map((t) => t.label)).toEqual(["なし", "交通", "電気", "水道", "駐車場", "需要"]);
    expect(themeOf("parking").layer).toBe(PARKING_LAYER);
    expect(themeOf("traffic").layer).toBe(TRAFFIC_LAYER);
    expect(themeOf("none").layer).toBeNull();
    expect(themeOf("demand").layer).toBeNull();
    expect(themeOf("power").layer).toBe(1);
    expect(themeOf("water").layer).toBe(2);
  });

  test("初めて開いたときは交通を選ぶ", () => {
    expect(DEFAULT_THEME).toBe("traffic");
    expect(loadTheme(memoryStorage())).toBe("traffic");
    expect(loadTheme(null)).toBe("traffic");
  });

  test("選んだ主題図を覚え、次に読むとその主題図になる", () => {
    const storage = memoryStorage();
    saveTheme(storage, "none");
    expect(storage.values.get(THEME_STORAGE_KEY)).toBe("none");
    expect(loadTheme(storage)).toBe("none");
    saveTheme(storage, "demand");
    expect(loadTheme(storage)).toBe("demand");
  });

  test("読めないときと、知らない値のときは交通にする。書けなくても止まらない", () => {
    const broken = memoryStorage(true);
    expect(loadTheme(broken)).toBe("traffic");
    expect(() => saveTheme(broken, "power")).not.toThrow();
    const old = memoryStorage();
    old.values.set(THEME_STORAGE_KEY, "3");
    expect(loadTheme(old)).toBe("traffic");
  });
});

describe("主題図の凡例", () => {
  test("なしでは凡例を出さない", () => {
    expect(themeLegend("none", 8, null)).toEqual([]);
  });

  test("交通は、引きでは「空いている」を外し、標準からは 4 段をすべて出す。高速道路は破線", () => {
    const labels = (zoom: number) => themeLegend("traffic", zoom, null).map((i) => i.label);
    expect(labels(3)).toEqual([
      "やや混んでいる",
      "混んでいる",
      "詰まっている",
      "数えない（高速道路）",
    ]);
    expect(labels(8)).toEqual([
      "空いている",
      "やや混んでいる",
      "混んでいる",
      "詰まっている",
      "数えない（高速道路）",
    ]);
    const items = themeLegend("traffic", 8, null);
    expect(items[0]?.color).toBe("rgb(99 214 104)");
    expect(items.at(-1)?.kind).toBe("dash");
  });

  test("電気と水道は、建物の届き方と道路の網の釣り合いを出す", () => {
    const items = themeLegend("power", 8, null);
    expect(items.map((i) => i.kind)).toEqual(["fill", "hatch", "line", "line", "dash"]);
    expect(items[1]?.accent).toBe("rgb(217 48 37)");
    expect(themeLegend("water", 8, null)).toEqual(items);
  });

  test("需要は、区画の種類ごとの需要を -1〜1 の棒で出す", () => {
    const items = themeLegend("demand", 8, [50, -200, 0, 100]);
    expect(items.map((i) => i.label)).toEqual(["住宅", "商業", "工業", "オフィス"]);
    expect(items.map((i) => i.value)).toEqual([0.5, -1, 0, 1]);
    expect(items.every((i) => i.kind === "bar")).toBe(true);
  });

  test("需要の塗りは、需要が高いほど濃く、0 以下でも区画の種類が分かる濃さを残す", () => {
    expect(demandShare(-1)).toBe(DEMAND_MIN_SHARE);
    expect(demandShare(0)).toBe(DEMAND_MIN_SHARE);
    expect(demandShare(1)).toBe(1);
    expect(demandShare(0.5)).toBeGreaterThan(demandShare(0.2));
  });
});

describe("駐車場の主題図", () => {
  test("車を持てない世帯の家と、共同の駐車場の埋まり具合の 4 段を凡例に出す", () => {
    const items = themeLegend("parking", 8, null);
    expect(items.map((i) => i.label)).toEqual([
      "車を持てない世帯の家",
      "共同の駐車場：半分に満たない",
      "共同の駐車場：半分以上",
      "共同の駐車場：8 割以上",
      "共同の駐車場：満車",
    ]);
    expect(items[0]?.kind).toBe("hatch");
    expect(items.slice(1).every((i) => i.kind === "fill")).toBe(true);
    // 埋まり具合は色の違いだけでなく、段ごとに濃くなる
    const colors = items.slice(1).map((i) => i.color);
    expect(new Set(colors).size).toBe(4);
  });

  test("駐車場の値の塗り方は chizu-wasm の値の並びと同じ", () => {
    const layer = DATA_LAYERS.find((l) => l.code === PARKING_LAYER);
    expect(layer?.styles.map((s) => s.kind)).toEqual([
      "none",
      "hatch",
      "fill",
      "fill",
      "fill",
      "fill",
    ]);
    // 色の表の長さに収まる
    expect(layer?.styles.length ?? 0).toBeLessThanOrEqual(PALETTE_SIZE);
  });
});
