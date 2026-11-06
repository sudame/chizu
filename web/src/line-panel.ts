// 路線の一覧。鉄道の道具の帯の中に出し、路線ごとに色、駅の並び、本数と上限、止まっている印を見せる。
// 本数は 1 ずつ上げ下げできる。路線を延ばす、駅を挿す、消すこともここから行う。

import { type LineRef, removeLineCommand, setLineTrainsCommand } from "./map/line-tool.ts";
import {
  lineColorCode,
  lineLimit,
  lineName,
  MAX_LINE_TRAINS,
  moveMinutes,
  type RailLineInfo,
  stationChain,
} from "./map/lines.ts";
import type { Tile } from "./map/roads.ts";
import type { CommandMessage } from "./protocol/messages.ts";

export type LineRow = {
  name: string;
  /** 駅の並び。 */
  stops: string;
  /** 本数と上限の文。 */
  trains: string;
  /** 運転間隔の文。走る列車がなければ空。 */
  headway: string;
  /** 止まっている、走れない、などの注意。なければ空。 */
  warning: string;
  /** 本数を 1 つ上げられるか。 */
  canAdd: boolean;
  /** 本数を 1 つ下げられるか。 */
  canSubtract: boolean;
};

/**
 * 上限が 0 の理由の文。待てる所が 2 つ未満か、ほかの路線と共有する区画の枠の分け前がないかを言う。
 */
export function capZeroReason(line: RailLineInfo): string {
  const names = line.sharedWith.map((l) => lineName(l.index)).join("、");
  const shared = names !== "" ? `この路線は${names}と進路を共有していて、` : "";
  if (line.waitPlaces < 2 || names === "") {
    return `${shared}待てる所が足りないので走れない（待てる所 ${line.waitPlaces}、2 つ以上いる）`;
  }
  return `${shared}共有する所に止まれる列車の枠が回ってこないので走れない（同じ所を使う路線を減らすと走れる）`;
}

/** 路線 1 本の一覧の行に出す文。 */
export function lineRow(line: RailLineInfo, nameOf: (station: Tile) => string): LineRow {
  let warning = "";
  if (line.stopped) {
    warning = "止まっている（線路がつながっていない）";
  } else if (line.cap === 0) {
    warning = `列車を走らせられない（本数の上限が 0）。${capZeroReason(line)}`;
  } else if (line.trains > line.allowed) {
    const why = line.turnCap < line.cap ? "（終点の駅で折り返せる本数まで）" : "";
    warning = `決めた本数 ${line.trains} のうち、走らせるのは ${line.allowed} 本${why}`;
  }
  return {
    name: lineName(line.index),
    stops: stationChain(line, nameOf),
    trains: `本数 ${line.trains} / 上限 ${lineLimit(line)}`,
    headway:
      line.stopped || line.allowed === 0 || line.headway === 0
        ? ""
        : `運転間隔 ${moveMinutes(line.headway)}・1 周 ${moveMinutes(line.roundTrip)}`,
    warning,
    canAdd: line.trains < Math.min(lineLimit(line), MAX_LINE_TRAINS),
    canSubtract: line.trains > 0,
  };
}

export type LinePanelHandlers = {
  send: (command: CommandMessage) => void;
  /** 路線を延ばす道具にする。 */
  extend: (line: LineRef) => void;
  /** 路線の途中に駅を挿す道具にする。 */
  insert: (line: LineRef) => void;
};

function element<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className?: string,
  text?: string,
): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  if (className !== undefined) {
    el.className = className;
  }
  if (text !== undefined) {
    el.textContent = text;
  }
  return el;
}

/** 一覧を書き直す。`extending` は、いま延ばしている路線。 */
export function renderLines(
  list: HTMLElement,
  lines: readonly RailLineInfo[],
  nameOf: (station: Tile) => string,
  extending: LineRef | null,
  inserting: LineRef | null,
  handlers: LinePanelHandlers,
): void {
  list.replaceChildren();
  list.dataset.count = String(lines.length);
  for (const line of lines) {
    const row = lineRow(line, nameOf);
    const ref: LineRef = { index: line.index, generation: line.generation };
    const item = element("li");
    item.dataset.line = `${line.index}.${line.generation}`;
    item.dataset.stopped = String(line.stopped);
    item.dataset.trains = String(line.trains);
    item.dataset.cap = String(line.cap);
    const swatch = element("span", "line-swatch");
    swatch.style.background = lineColorCode(line.index);
    item.append(swatch, element("span", "line-name", row.name), element("span", "", row.trains));
    const minus = element("button", "chip", "−");
    minus.type = "button";
    minus.ariaLabel = `${row.name}の本数を減らす`;
    minus.disabled = !row.canSubtract;
    minus.addEventListener("click", () =>
      handlers.send(setLineTrainsCommand(ref, line.trains - 1)),
    );
    const plus = element("button", "chip", "＋");
    plus.type = "button";
    plus.ariaLabel = `${row.name}の本数を増やす`;
    plus.disabled = !row.canAdd;
    plus.addEventListener("click", () => handlers.send(setLineTrainsCommand(ref, line.trains + 1)));
    const extend = element("button", "chip", "駅を足す");
    extend.type = "button";
    extend.setAttribute(
      "aria-pressed",
      String(extending?.index === ref.index && extending.generation === ref.generation),
    );
    extend.addEventListener("click", () => handlers.extend(ref));
    const insert = element("button", "chip", "駅を挿す");
    insert.type = "button";
    insert.setAttribute(
      "aria-pressed",
      String(inserting?.index === ref.index && inserting.generation === ref.generation),
    );
    insert.addEventListener("click", () => handlers.insert(ref));
    const remove = element("button", "chip", "消す");
    remove.type = "button";
    remove.ariaLabel = `${row.name}を消す`;
    remove.addEventListener("click", () => handlers.send(removeLineCommand(ref)));
    // 環状にした路線は延ばせない
    if (line.looped) {
      item.append(minus, plus, insert, remove);
    } else {
      item.append(minus, plus, extend, insert, remove);
    }
    item.append(element("span", "line-stops", row.stops));
    if (row.headway !== "") {
      item.append(element("span", "line-note", row.headway));
    }
    if (row.warning !== "") {
      const warning = element("span", "line-warn", row.warning);
      warning.dataset.warning = line.stopped ? "stopped" : "limit";
      item.append(warning);
    }
    list.append(item);
  }
}
