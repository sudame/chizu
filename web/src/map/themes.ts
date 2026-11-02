// 主題図（docs/map-style.md の「主題図」）。地図アプリのレイヤーにならい、なし、交通、電気、水道、需要の
// 1 つだけを選ぶ。初めは交通を選び、選んだものはブラウザの localStorage に覚えさせる。
// 主題図は画面の側だけの設定で、記録にも保存データにも入れない。

import { demandBar } from "../city-panel.ts";
import type { LayerCode } from "../protocol/messages.ts";
import { HIGHWAY_CODE } from "./lanes.ts";
import { THEME_COLORS, TRAFFIC_LAYER, TRAFFIC_LEVELS } from "./layers.ts";
import type { Color } from "./palette.ts";
import { buildingEdgeColor, roadStyle } from "./roads.ts";
import { stageWeights } from "./stages.ts";
import { TRAFFIC_UNCOUNTED_COLOR } from "./traffic.ts";

export type ThemeId = "none" | "traffic" | "power" | "water" | "demand";

export type Theme = {
  id: ThemeId;
  label: string;
  /** Worker に値を頼むデータレイヤーの番号。需要は街の数字の需要で塗るので頼まない。 */
  layer: LayerCode | null;
  /** パネルで名前の横に出す見本の色。 */
  swatch: Color | null;
};

/** パネルに並べる順。 */
export const THEMES: readonly Theme[] = [
  { id: "none", label: "なし", layer: null, swatch: null },
  { id: "traffic", label: "交通", layer: TRAFFIC_LAYER, swatch: TRAFFIC_LEVELS[0]?.color ?? null },
  { id: "power", label: "電気", layer: 1, swatch: THEME_COLORS.suppliedBuilding },
  { id: "water", label: "水道", layer: 2, swatch: THEME_COLORS.enoughRoad },
  { id: "demand", label: "需要", layer: null, swatch: demandColor(1) },
];

/** 需要の主題図で、建物のない区画を塗る色の濃さの下限。需要が 0 以下でも、区画の種類は見分けられるようにする。 */
export const DEMAND_MIN_SHARE = 0.15;

/** 需要の主題図で、需要の高さに応じて白から寄せる区画の色。区画の種類ごとの建物の縁の色である。 */
export function demandColor(zone: number): Color {
  return buildingEdgeColor(zone);
}

/** 需要の高さ（0〜1）での、白から区画の色へ寄せる割合。 */
export function demandShare(level: number): number {
  const l = Math.min(Math.max(level, 0), 1);
  return DEMAND_MIN_SHARE + (1 - DEMAND_MIN_SHARE) * l;
}

/** 初めて開いたときと、覚えた値を読めないときの主題図。 */
export const DEFAULT_THEME: ThemeId = "traffic";
/** 選んだ主題図を覚える localStorage の鍵。 */
export const THEME_STORAGE_KEY = "chizu.theme";

const isThemeId = (v: unknown): v is ThemeId => THEMES.some((t) => t.id === v);

export function themeOf(id: ThemeId): Theme {
  return THEMES.find((t) => t.id === id) ?? (THEMES[0] as Theme);
}

/** ページの localStorage。使えない（ブロックされている、置き場がない）ときは null。 */
export function themeStorage(): Storage | null {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

/** 覚えた主題図を読む。読めない、知らない値のときは交通にする。 */
export function loadTheme(storage: Pick<Storage, "getItem"> | null): ThemeId {
  try {
    const value = storage?.getItem(THEME_STORAGE_KEY);
    return isThemeId(value) ? value : DEFAULT_THEME;
  } catch {
    return DEFAULT_THEME;
  }
}

/** 選んだ主題図を覚える。書けないときは覚えずに続ける。 */
export function saveTheme(storage: Pick<Storage, "setItem"> | null, id: ThemeId): void {
  try {
    storage?.setItem(THEME_STORAGE_KEY, id);
  } catch {
    // 書けなくても、今の画面の主題図は変える
  }
}

function cssColor([r, g, b]: readonly number[]): string {
  const channel = (v: number | undefined) => Math.round((v ?? 0) * 255);
  return `rgb(${channel(r)} ${channel(g)} ${channel(b)})`;
}

/**
 * 主題図を選ぶパネルの中身を作る。押すと onChange を呼ぶ。選んだものは、パネルの `data-active`（test:wasm が読む）と
 * ボタンの `aria-pressed` に出す。返す関数で、外から選んだものを変えられる。
 */
export function setupThemePanel(
  container: HTMLElement,
  initial: ThemeId,
  onChange: (id: ThemeId) => void,
): (id: ThemeId) => void {
  let selected = initial;
  const buttons = THEMES.map((theme) => {
    const b = document.createElement("button");
    b.type = "button";
    const dot = document.createElement("span");
    dot.className = "layer-dot";
    dot.setAttribute("aria-hidden", "true");
    if (theme.swatch !== null) {
      dot.style.background = cssColor(theme.swatch);
    }
    const text = document.createElement("span");
    text.textContent = theme.label;
    b.append(dot, text);
    b.dataset.layer = theme.id;
    b.addEventListener("click", () => {
      select(theme.id);
      onChange(theme.id);
    });
    return b;
  });
  const select = (id: ThemeId) => {
    selected = id;
    buttons.forEach((b, i) => {
      b.setAttribute("aria-pressed", String(THEMES[i]?.id === selected));
    });
    container.dataset.active = selected;
  };
  container.replaceChildren(...buttons);
  select(initial);
  return select;
}

/**
 * 主題図の凡例の 1 項目。`line` は線の見本、`dash` は破線の見本、`fill` は面の見本、`hatch` は斜線つきの面の見本、
 * `bar` は需要の棒（`value` は -1〜1）である。
 */
export type ThemeLegendItem = {
  label: string;
  kind: "line" | "dash" | "fill" | "hatch" | "bar";
  color: string;
  accent?: string;
  value?: number;
};

const ZONE_LABELS = ["住宅", "商業", "工業", "オフィス"];

/**
 * 主題図の凡例。交通は、引きの段では描かない「空いている」を外す。需要は、区画の種類ごとの需要の棒を出す。
 * `demand` は住宅・商業・工業・オフィスの需要（街の数字の値）である。
 */
export function themeLegend(
  id: ThemeId,
  zoom: number,
  demand: readonly number[] | null,
): ThemeLegendItem[] {
  switch (id) {
    case "none":
      return [];
    case "traffic": {
      const free = stageWeights(zoom).trafficFree >= 0.5;
      const levels = TRAFFIC_LEVELS.filter((_, i) => free || i > 0).map(
        (l): ThemeLegendItem => ({ label: l.label, kind: "line", color: cssColor(l.color) }),
      );
      return [
        ...levels,
        {
          label: "数えない（高速道路）",
          kind: "dash",
          color: cssColor(TRAFFIC_UNCOUNTED_COLOR),
          accent: cssColor(roadStyle(HIGHWAY_CODE).asphalt),
        },
      ];
    }
    case "power":
    case "water":
      return [
        { label: "届いている", kind: "fill", color: cssColor(THEME_COLORS.suppliedBuilding) },
        {
          label: "届いていない",
          kind: "hatch",
          color: cssColor(THEME_COLORS.unsuppliedBuilding),
          accent: cssColor(THEME_COLORS.unsuppliedHatch),
        },
        { label: "網が足りている", kind: "line", color: cssColor(THEME_COLORS.enoughRoad) },
        { label: "網が足りない", kind: "line", color: cssColor(THEME_COLORS.shortRoad) },
        {
          label: "施設につながっていない",
          kind: "dash",
          color: cssColor(THEME_COLORS.unconnectedRoad),
        },
      ];
    case "demand":
      return ZONE_LABELS.map((label, i) => ({
        label,
        kind: "bar",
        color: cssColor(demandColor(i + 1)),
        value: demandBar(demand?.[i] ?? 0),
      }));
  }
}

/** 凡例を描き直す必要があるかを見分ける鍵。倍率は、交通の「空いている」を出すかだけで見る。 */
export function themeLegendKey(
  id: ThemeId,
  zoom: number,
  demand: readonly number[] | null,
): string {
  return JSON.stringify(themeLegend(id, zoom, demand));
}

/** 凡例の中身を container に作る。項目がなければ隠す。 */
export function renderThemeLegend(container: HTMLElement, items: ThemeLegendItem[]): void {
  container.hidden = items.length === 0;
  container.replaceChildren(
    ...items.map((item) => {
      const li = document.createElement("li");
      li.dataset.kind = item.kind;
      const sample = document.createElement("span");
      sample.className = `theme-sample theme-${item.kind}`;
      sample.setAttribute("aria-hidden", "true");
      sample.style.setProperty("--c", item.color);
      if (item.accent !== undefined) {
        sample.style.setProperty("--a", item.accent);
      }
      if (item.kind === "bar") {
        const v = item.value ?? 0;
        // 正の需要は真ん中から右へ、負の需要は左へ伸ばす
        sample.style.setProperty("--v", String(Math.abs(v)));
        sample.dataset.sign = v < 0 ? "neg" : "pos";
      }
      const text = document.createElement("span");
      text.textContent = item.label;
      li.append(sample, text);
      return li;
    }),
  );
}
