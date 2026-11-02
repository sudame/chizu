// 地図の色と印の凡例。色と形は描画で使う定数（map/roads.ts）から取る。主題図の凡例は map/themes.ts が作る。

import { HIGHWAY_CODE } from "./map/lanes.ts";
import { CAR_COLORS, WALKER_COLOR } from "./map/mover-colors.ts";
import {
  BOLT_SHAPE,
  buildingColor,
  buildingEdgeColor,
  DROP_SHAPE,
  FACILITY_COLOR,
  NO_POWER_COLOR,
  NO_WATER_COLOR,
  roadStyle,
  type Triangle,
  zoneGroundColor,
} from "./map/roads.ts";

/** 凡例の 1 項目。swatch は色の四角で、border があれば縁をその色で描く。icon は印の形（三角形の組み合わせ）である。 */
export type LegendItem = {
  label: string;
  color: string;
  border?: string;
  icon?: { triangles: readonly Triangle[] };
};

export type LegendSection = { title: string; items: LegendItem[] };

/** [0, 1] の色を CSS の色に直す。 */
export function cssColor(c: readonly number[]): string {
  const ch = (i: number) => Math.round(Math.min(Math.max(c[i] ?? 0, 0), 1) * 255);
  return `rgb(${ch(0)} ${ch(1)} ${ch(2)})`;
}

const ZONES: [number, string][] = [
  [1, "住宅"],
  [2, "商業"],
  [3, "工業"],
  [4, "オフィス"],
];

const ROADS: [number, string][] = [
  [1, "小さい道路"],
  [2, "中くらいの道路"],
  [3, "大きい道路"],
  [HIGHWAY_CODE, "高速道路"],
];

export function legendSections(): LegendSection[] {
  return [
    {
      title: "区画（地面の色）",
      items: ZONES.map(([zone, label]) => ({
        label,
        color: cssColor(zoneGroundColor(zone)),
        border: cssColor(buildingEdgeColor(zone)),
      })),
    },
    {
      title: "建物（区画と同じ系統の色）",
      items: ZONES.map(([zone, label]) => ({
        label,
        color: cssColor(buildingColor(zone)),
        border: cssColor(buildingEdgeColor(zone)),
      })),
    },
    {
      title: "届いていない印（建物の左上）",
      items: [
        {
          label: "電気が届いていない",
          color: cssColor(NO_POWER_COLOR),
          icon: { triangles: BOLT_SHAPE },
        },
        {
          label: "水道が届いていない",
          color: cssColor(NO_WATER_COLOR),
          icon: { triangles: DROP_SHAPE },
        },
      ],
    },
    {
      title: "公共施設",
      items: [
        { label: "発電所", color: cssColor(FACILITY_COLOR[1] ?? [0.6, 0.6, 0.6, 1]) },
        { label: "浄水場", color: cssColor(FACILITY_COLOR[2] ?? [0.6, 0.6, 0.6, 1]) },
      ],
    },
    {
      title: "道路（広いほど車道が太い。高速道路は橙、大きい道路は黄）",
      items: ROADS.map(([kind, label]) => {
        const style = roadStyle(kind);
        return { label, color: cssColor(style.asphalt), border: cssColor(style.edge) };
      }),
    },
    {
      title: "移動体",
      items: [
        { label: "車（色はいろいろ）", color: cssColor(CAR_COLORS[3] ?? [1, 1, 1]) },
        { label: "歩く人", color: cssColor(WALKER_COLOR) },
      ],
    },
  ];
}

/** SVG の points 属性。形の座標 [-1, 1] を 0〜size の範囲に直す。 */
export function trianglePoints(t: Triangle, size: number): string {
  return t.map(([x, y]) => `${((x + 1) * size) / 2},${((y + 1) * size) / 2}`).join(" ");
}

const SVG_NS = "http://www.w3.org/2000/svg";
const ICON_PX = 16;

function swatch(item: LegendItem): Element {
  if (item.icon === undefined) {
    const box = document.createElement("span");
    box.className = "swatch";
    box.style.background = item.color;
    if (item.border !== undefined) {
      box.style.boxShadow = `inset 0 0 0 3px ${item.border}`;
    }
    return box;
  }
  const svg = document.createElementNS(SVG_NS, "svg");
  svg.setAttribute("class", "swatch-icon");
  svg.setAttribute("width", String(ICON_PX));
  svg.setAttribute("height", String(ICON_PX));
  svg.setAttribute("viewBox", `0 0 ${ICON_PX} ${ICON_PX}`);
  svg.setAttribute("aria-hidden", "true");
  for (const t of item.icon.triangles) {
    const polygon = document.createElementNS(SVG_NS, "polygon");
    polygon.setAttribute("points", trianglePoints(t, ICON_PX));
    polygon.setAttribute("fill", item.color);
    svg.append(polygon);
  }
  return svg;
}

/** 凡例の中身を container に作る。 */
export function renderLegend(container: HTMLElement): void {
  container.replaceChildren();
  for (const section of legendSections()) {
    const block = document.createElement("section");
    const title = document.createElement("h3");
    title.textContent = section.title;
    const list = document.createElement("ul");
    for (const item of section.items) {
      const li = document.createElement("li");
      const text = document.createElement("span");
      text.textContent = item.label;
      li.append(swatch(item), text);
      list.append(li);
    }
    block.append(title, list);
    container.append(block);
  }
}
