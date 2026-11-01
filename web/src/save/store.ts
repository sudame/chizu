// 保存の枠を IndexedDB に置く。1 枠 1 件で、枠の名前を鍵にする。

import { isSaveRecord, isSlotId, type SaveRecord, type SlotId } from "./protocol.ts";

const DB_NAME = "chizu";
const DB_VERSION = 1;
const STORE = "saves";

function promised<T>(request: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

/** 枠ごとの保存データと、読めなかったか形の崩れた枠。 */
export type SlotListing = { records: Map<SlotId, SaveRecord>; broken: Set<SlotId> };

export class SaveStore {
  private readonly db: IDBDatabase;

  private constructor(db: IDBDatabase) {
    this.db = db;
  }

  static async open(): Promise<SaveStore> {
    const request = indexedDB.open(DB_NAME, DB_VERSION);
    request.onupgradeneeded = () => {
      request.result.createObjectStore(STORE, { keyPath: "slot" });
    };
    return new SaveStore(await promised(request));
  }

  async put(record: SaveRecord): Promise<void> {
    const tx = this.db.transaction(STORE, "readwrite");
    tx.objectStore(STORE).put(record);
    await new Promise<void>((resolve, reject) => {
      tx.oncomplete = () => resolve();
      tx.onerror = () => reject(tx.error);
      tx.onabort = () => reject(tx.error);
    });
  }

  /**
   * 鍵を先に読み、1 枠ずつ読む。1 枠が読めなくても、ほかの枠は返す。
   * 鍵を読むこと自体ができないときは例外を投げる。
   */
  async list(): Promise<SlotListing> {
    const store = () => this.db.transaction(STORE, "readonly").objectStore(STORE);
    const keys = await promised(store().getAllKeys());
    const records = new Map<SlotId, SaveRecord>();
    const broken = new Set<SlotId>();
    for (const key of keys) {
      if (!isSlotId(key)) {
        continue;
      }
      try {
        const row: unknown = await promised(store().get(key));
        if (isSaveRecord(row) && row.slot === key) {
          records.set(key, row);
        } else {
          broken.add(key);
        }
      } catch {
        broken.add(key);
      }
    }
    return { records, broken };
  }
}
