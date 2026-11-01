// 保存と読み込みのために、メインスレッドと Worker のあいだでやりとりするメッセージ。
// protocol/messages.ts とは別の kind（"save:" で始まる）にして、それぞれの側で別に受ける。

export const SLOTS = ["auto", "manual-1", "manual-2", "manual-3"] as const;
export type SlotId = (typeof SLOTS)[number];

/** chizu-wasm の SavePayload の meta。 */
export type SaveMeta = {
  formatVersion: number;
  seed: string;
  tick: number;
  /** 記録の件数。読み込むときに記録と突き合わせる。 */
  commandCount: number;
  day: number;
  secondOfDay: number;
  money: string;
  stateHash: string;
};

/** IndexedDB の 1 枠に書くもの。commands は記録を encode したもの。 */
export type SaveRecord = {
  slot: SlotId;
  /** 保存した日時（Date.now()）。 */
  savedAt: number;
  meta: SaveMeta;
  commands: ArrayBuffer;
};

export type SaveToWorker =
  /** save が null なら新しく始める。 */
  | { kind: "save:start"; save: { meta: SaveMeta; commands: ArrayBuffer } | null }
  | { kind: "save:request"; slot: SlotId };

export type SaveFromWorker =
  /** Worker が WASM を読み込み、始め方を待っている。 */
  | { kind: "save:hello"; formatVersion: number }
  | { kind: "save:progress"; tick: number; target: number }
  /** 読み込めなかった。Worker は次の save:start を待つ。 */
  | { kind: "save:load-error"; message: string }
  /**
   * 街ができた。読み込んだときは、作り直した街の meta を添える。rulesChanged は、作り直した街の
   * 状態ハッシュが保存したときと違う（シミュレーションの規則が変わった）こと。
   */
  | { kind: "save:started"; loaded: { meta: SaveMeta; rulesChanged: boolean } | null }
  | { kind: "save:data"; slot: SlotId; meta: SaveMeta; commands: ArrayBuffer };

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

export function isSlotId(value: unknown): value is SlotId {
  return SLOTS.some((s) => s === value);
}

export function isSaveMeta(value: unknown): value is SaveMeta {
  return (
    isObject(value) &&
    Number.isInteger(value.formatVersion) &&
    typeof value.seed === "string" &&
    Number.isInteger(value.tick) &&
    Number.isInteger(value.commandCount) &&
    Number.isInteger(value.day) &&
    Number.isInteger(value.secondOfDay) &&
    typeof value.money === "string" &&
    typeof value.stateHash === "string"
  );
}

export function isSaveRecord(value: unknown): value is SaveRecord {
  return (
    isObject(value) &&
    isSlotId(value.slot) &&
    typeof value.savedAt === "number" &&
    isSaveMeta(value.meta) &&
    value.commands instanceof ArrayBuffer
  );
}

export function isSaveToWorker(value: unknown): value is SaveToWorker {
  if (!isObject(value)) {
    return false;
  }
  switch (value.kind) {
    case "save:start": {
      const s = value.save;
      return s === null || (isObject(s) && isSaveMeta(s.meta) && s.commands instanceof ArrayBuffer);
    }
    case "save:request":
      return isSlotId(value.slot);
    default:
      return false;
  }
}

export function isSaveFromWorker(value: unknown): value is SaveFromWorker {
  if (!isObject(value)) {
    return false;
  }
  switch (value.kind) {
    case "save:hello":
      return Number.isInteger(value.formatVersion);
    case "save:progress":
      return typeof value.tick === "number" && typeof value.target === "number";
    case "save:load-error":
      return typeof value.message === "string";
    case "save:started":
      return (
        value.loaded === null ||
        (isObject(value.loaded) &&
          isSaveMeta(value.loaded.meta) &&
          typeof value.loaded.rulesChanged === "boolean")
      );
    case "save:data":
      return (
        isSlotId(value.slot) && isSaveMeta(value.meta) && value.commands instanceof ArrayBuffer
      );
    default:
      return false;
  }
}

const pad = (n: number) => String(n).padStart(2, "0");

/** 枠に並べる説明。保存した日時、ゲーム内の日付、所持金。 */
export function describeSave(record: SaveRecord): string {
  const at = new Date(record.savedAt);
  const saved = `${at.getFullYear()}/${pad(at.getMonth() + 1)}/${pad(at.getDate())} ${pad(at.getHours())}:${pad(at.getMinutes())}`;
  const { day, secondOfDay, money } = record.meta;
  const clock = `${day + 1} 日目 ${pad(Math.floor(secondOfDay / 3600))}:${pad(Math.floor(secondOfDay / 60) % 60)}`;
  return `${saved} 保存・${clock}・お金 ${BigInt(money).toLocaleString("ja-JP")}`;
}

/** 読み込めない理由。読み込めるなら null。 */
export function unreadableReason(record: SaveRecord, formatVersion: number): string | null {
  if (record.meta.formatVersion !== formatVersion) {
    return `読めない（記録の形式の版 ${record.meta.formatVersion}、読めるのは ${formatVersion}）`;
  }
  return null;
}

export const SLOT_NAMES: Record<SlotId, string> = {
  auto: "自動保存",
  "manual-1": "保存 1",
  "manual-2": "保存 2",
  "manual-3": "保存 3",
};
