// 路線を引く道具の操作をコマンドに直す部分。DOM に触らないので、Vitest で確かめられる。
// CS2 の路線の道具と同じく、駅を 1 つずつ押して足していく。最初の 2 つで路線を作り、そのあとは作った路線を延ばす。

import type { CommandMessage } from "../protocol/messages.ts";
import { MAX_LINE_TRAINS, type RailLineInfo } from "./lines.ts";
import type { RailStation } from "./rail.ts";
import type { Tile } from "./roads.ts";

/** 路線を指す。rail_lines が添える添え字と世代である。 */
export type LineRef = { index: number; generation: number };

/** 路線の道具の途中の状態。 */
export type LineDraft = {
  /** 押した最初の駅（敷地の min のマス）。次の駅を押すと路線ができる。 */
  first: Tile | null;
  /** 路線を作るコマンドを送り、できた路線がまだ届いていない。届いたら延ばす路線にする。 */
  pending: { first: Tile; second: Tile } | null;
  /** 延ばす路線。ここに駅を足していく。 */
  extend: LineRef | null;
  /**
   * 駅を挿す路線。`after` は、どの駅の次に挿すかを選んだ駅の並びの添え字で、選ぶまでは null。
   * 路線の駅を押して位置を選び、そのあと路線にない駅を押すと、そこに挿す。
   */
  insert: { line: LineRef; after: number | null } | null;
};

export const EMPTY_DRAFT: LineDraft = { first: null, pending: null, extend: null, insert: null };

const sameTile = (a: Tile, b: Tile) => a.x === b.x && a.y === b.y;

/** マス `at` を敷地に含む駅。 */
export function stationAt(stations: readonly RailStation[], at: Tile): RailStation | null {
  return (
    stations.find(
      (s) => at.x >= s.min.x && at.x <= s.max.x && at.y >= s.min.y && at.y <= s.max.y,
    ) ?? null
  );
}

export function createLineCommand(from: Tile, to: Tile): CommandMessage {
  return { kind: "CreateLine", fromX: from.x, fromY: from.y, toX: to.x, toY: to.y };
}

export function extendLineCommand(line: LineRef, at: Tile): CommandMessage {
  return {
    kind: "ExtendLine",
    lineIndex: line.index,
    lineGeneration: line.generation,
    atX: at.x,
    atY: at.y,
  };
}

export function insertLineStationCommand(line: LineRef, after: number, at: Tile): CommandMessage {
  return {
    kind: "InsertLineStation",
    lineIndex: line.index,
    lineGeneration: line.generation,
    afterIndex: after,
    atX: at.x,
    atY: at.y,
  };
}

export function removeLineCommand(line: LineRef): CommandMessage {
  return { kind: "RemoveLine", lineIndex: line.index, lineGeneration: line.generation };
}

/** 本数を 0〜MAX_LINE_TRAINS に収めて決める。 */
export function setLineTrainsCommand(line: LineRef, trains: number): CommandMessage {
  return {
    kind: "SetLineTrains",
    lineIndex: line.index,
    lineGeneration: line.generation,
    trains: Math.min(Math.max(Math.round(trains), 0), MAX_LINE_TRAINS),
  };
}

/**
 * 駅 `station`（敷地の min のマス）を押したときの、コマンドと次の状態。
 * 延ばす路線があればそこへ足す。なければ、最初の駅を覚え、2 つ目の駅で路線を作る。
 * 駅を挿す路線があれば、その路線の駅（`insertStations`、路線の並びの順）を押して位置を選び、
 * 路線にない駅を押して挿す。駅でない所を押したときは何もしない。
 */
export function clickStation(
  draft: LineDraft,
  station: Tile | null,
  insertStations: readonly Tile[] = [],
): { command: CommandMessage | null; draft: LineDraft } {
  if (station === null || draft.pending !== null) {
    return { command: null, draft };
  }
  if (draft.insert !== null) {
    const { line, after } = draft.insert;
    const index = insertStations.findIndex((t) => sameTile(t, station));
    if (index >= 0) {
      return { command: null, draft: { ...draft, insert: { line, after: index } } };
    }
    if (after === null) {
      return { command: null, draft };
    }
    return {
      command: insertLineStationCommand(line, after, station),
      draft: { ...draft, insert: { line, after: null } },
    };
  }
  if (draft.extend !== null) {
    return { command: extendLineCommand(draft.extend, station), draft };
  }
  if (draft.first === null) {
    return { command: null, draft: { ...draft, first: station } };
  }
  if (sameTile(draft.first, station)) {
    // 同じ駅をもう一度押したら、選びなおす
    return { command: null, draft: EMPTY_DRAFT };
  }
  return {
    command: createLineCommand(draft.first, station),
    draft: {
      first: null,
      pending: { first: draft.first, second: station },
      extend: null,
      insert: null,
    },
  };
}

/**
 * 路線の一覧が届いたときの状態。路線を作るコマンドを送ったあとなら、できた路線（まだ知らなかった添え字で、
 * 駅の並びが押した 2 つ）を延ばす路線にする。できなかった（断られた）ときは、一覧に増えがなければ最初に戻す。
 */
export function adoptCreated(
  draft: LineDraft,
  lines: readonly RailLineInfo[],
  known: ReadonlySet<string>,
): LineDraft {
  const wait = draft.pending;
  if (wait === null) {
    return draft;
  }
  const made = lines.find(
    (l) =>
      !known.has(lineKey(l)) &&
      l.stations.length === 2 &&
      l.stations[0] !== undefined &&
      l.stations[1] !== undefined &&
      sameTile(l.stations[0], wait.first) &&
      sameTile(l.stations[1], wait.second),
  );
  if (made === undefined) {
    return draft;
  }
  return {
    first: null,
    pending: null,
    extend: { index: made.index, generation: made.generation },
    insert: null,
  };
}

/** 路線を作るコマンドが断られたときの状態。最初の駅を選んだ状態に戻し、別の駅を選びなおせるようにする。 */
export function abandonPending(draft: LineDraft): LineDraft {
  return draft.pending === null
    ? draft
    : { first: draft.pending.first, pending: null, extend: null, insert: null };
}

/** 路線を見分ける鍵。 */
export function lineKey(l: LineRef): string {
  return `${l.index}.${l.generation}`;
}

/** 延ばしている路線が、まだ一覧にあるか。 */
export function extendTarget(
  draft: LineDraft,
  lines: readonly RailLineInfo[],
): RailLineInfo | null {
  const e = draft.extend;
  if (e === null) {
    return null;
  }
  return lines.find((l) => l.index === e.index && l.generation === e.generation) ?? null;
}

/** 駅を挿している路線が、まだ一覧にあるか。 */
export function insertTarget(
  draft: LineDraft,
  lines: readonly RailLineInfo[],
): RailLineInfo | null {
  const e = draft.insert?.line;
  if (e === undefined) {
    return null;
  }
  return lines.find((l) => l.index === e.index && l.generation === e.generation) ?? null;
}

/** 道具の説明の欄に出す、次にすることの案内。 */
export function lineToolHint(
  draft: LineDraft,
  nameOf: (station: Tile) => string,
  lineLabel: (line: LineRef) => string,
): string {
  if (draft.pending !== null) {
    return "路線を作っている";
  }
  if (draft.insert !== null) {
    return draft.insert.after === null
      ? `${lineLabel(draft.insert.line)}に駅を挿す。どの駅の次かを、路線の駅を押して選ぶ`
      : `${lineLabel(draft.insert.line)}に駅を挿す。挿す駅を押す（別の駅を押すと位置を選びなおせる）`;
  }
  if (draft.extend !== null) {
    return `${lineLabel(draft.extend)}を延ばす。次の駅を押す（最初の駅を押すと環状になる）`;
  }
  if (draft.first !== null) {
    return `${nameOf(draft.first)}から。次の駅を押すと路線ができる`;
  }
  return "路線の最初の駅を押す";
}
