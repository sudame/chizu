// 保存と読み込みのために、メインスレッドと Worker のあいだでやりとりするメッセージ。
// protocol/messages.ts とは別の kind（"save:" で始まる）にして、それぞれの側で別に受ける。

/** auto-previous は、自動保存があるときに新しく始めたら、前の街の自動保存を退避しておく枠。 */
export const SLOTS = ["auto", "auto-previous", "manual-1", "manual-2", "manual-3"] as const;
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

/**
 * IndexedDB の 1 枠に書くもの。commands は記録を encode したもの。state は街の状態を書き出したもので、
 * 街の状態を入れる前の保存データにはない。読み込むときは state があればそこから開き、なければ記録を再生する。
 */
export type SaveRecord = {
  slot: SlotId;
  /** 保存した日時（Date.now()）。 */
  savedAt: number;
  meta: SaveMeta;
  commands: ArrayBuffer;
  state?: ArrayBuffer;
};

/** Worker に渡して読み込む保存データ。 */
export type SaveBody = { meta: SaveMeta; commands: ArrayBuffer; state: ArrayBuffer | null };

export type SaveToWorker =
  /** save が null なら新しく始める。autoSave が false なら自動保存しない。 */
  | {
      kind: "save:start";
      save: SaveBody | null;
      autoSave: boolean;
    }
  | { kind: "save:request"; slot: SlotId };

export type SaveFromWorker =
  /** Worker が WASM を読み込み、始め方を待っている。 */
  | { kind: "save:hello"; formatVersion: number }
  | { kind: "save:progress"; tick: number; target: number }
  /** 読み込めなかった。Worker は次の save:start を待つ。 */
  | { kind: "save:load-error"; message: string }
  /**
   * 街ができた。読み込んだときは、作り直した街の meta を添える。rulesChanged は、作り直した街の
   * 状態ハッシュが保存したときと違う（シミュレーションの規則が変わった）こと。fromState は、
   * 記録を再生せずに街の状態から開いたこと。
   */
  | {
      kind: "save:started";
      loaded: { meta: SaveMeta; rulesChanged: boolean; fromState: boolean } | null;
    }
  | { kind: "save:data"; slot: SlotId; meta: SaveMeta; commands: ArrayBuffer; state: ArrayBuffer };

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

export function isSlotId(value: unknown): value is SlotId {
  return SLOTS.some((s) => s === value);
}

const count = (v: unknown) => Number.isSafeInteger(v) && (v as number) >= 0;
const matches = (v: unknown, pattern: RegExp) => typeof v === "string" && pattern.test(v);

/** 値まで確かめる。種は 10 進の u64、所持金は整数、状態ハッシュは 16 進の 16 桁の文字列である。 */
export function isSaveMeta(value: unknown): value is SaveMeta {
  return (
    isObject(value) &&
    count(value.formatVersion) &&
    matches(value.seed, /^\d{1,20}$/) &&
    count(value.tick) &&
    count(value.commandCount) &&
    count(value.day) &&
    count(value.secondOfDay) &&
    (value.secondOfDay as number) < 86_400 &&
    matches(value.money, /^-?\d{1,20}$/) &&
    matches(value.stateHash, /^[0-9a-f]{16}$/)
  );
}

export function isSaveRecord(value: unknown): value is SaveRecord {
  return (
    isObject(value) &&
    isSlotId(value.slot) &&
    Number.isFinite(value.savedAt) &&
    isSaveMeta(value.meta) &&
    value.commands instanceof ArrayBuffer &&
    (value.state === undefined || value.state instanceof ArrayBuffer)
  );
}

export function isSaveToWorker(value: unknown): value is SaveToWorker {
  if (!isObject(value)) {
    return false;
  }
  switch (value.kind) {
    case "save:start": {
      const s = value.save;
      return (
        typeof value.autoSave === "boolean" &&
        (s === null ||
          (isObject(s) &&
            isSaveMeta(s.meta) &&
            s.commands instanceof ArrayBuffer &&
            (s.state === null || s.state instanceof ArrayBuffer)))
      );
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
          typeof value.loaded.rulesChanged === "boolean" &&
          typeof value.loaded.fromState === "boolean")
      );
    case "save:data":
      return (
        isSlotId(value.slot) &&
        isSaveMeta(value.meta) &&
        value.commands instanceof ArrayBuffer &&
        value.state instanceof ArrayBuffer
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
  "auto-previous": "自動保存（前の街）",
  "manual-1": "保存 1",
  "manual-2": "保存 2",
  "manual-3": "保存 3",
};
