// 保存の枠を IndexedDB に置く。1 枠 1 件で、枠の名前を鍵にする。

import { isSaveRecord, type SaveRecord, type SlotId } from "./protocol.ts";

const DB_NAME = "chizu";
const DB_VERSION = 1;
const STORE = "saves";

function promised<T>(request: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

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

  /** 枠ごとの保存データ。形の崩れたものは除く。 */
  async all(): Promise<Map<SlotId, SaveRecord>> {
    const rows: unknown[] = await promised(
      this.db.transaction(STORE, "readonly").objectStore(STORE).getAll(),
    );
    return new Map(rows.filter(isSaveRecord).map((r) => [r.slot, r]));
  }
}
