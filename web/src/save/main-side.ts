// 画面の側の保存と読み込み。起動したときに続きから始めるか新しく始めるかを選ばせ、
// Worker から届いた保存データを IndexedDB に書く。手動保存の欄もここで作る。

import {
  describeSave,
  isSaveFromWorker,
  type SaveFromWorker,
  type SaveRecord,
  type SaveToWorker,
  SLOT_NAMES,
  SLOTS,
  type SlotId,
  unreadableReason,
} from "./protocol.ts";
import { SaveStore } from "./store.ts";

const MANUAL_SLOTS = SLOTS.filter((s) => s !== "auto");

function make<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  text = "",
  attrs: Record<string, string> = {},
): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  el.textContent = text;
  for (const [k, v] of Object.entries(attrs)) {
    el.setAttribute(k, v);
  }
  return el;
}

export function setupSaves(worker: Worker, controls: HTMLElement): void {
  const send = (message: SaveToWorker, transfer: Transferable[] = []) =>
    worker.postMessage(message, transfer);
  const storePromise = SaveStore.open();
  let records = new Map<SlotId, SaveRecord>();
  let formatVersion = 0;
  // 読み込みを頼んだ枠。読み込んだあと、作り直した状態ハッシュと比べる
  let loading: SaveRecord | null = null;

  const startDialog = make("dialog", "", { id: "save-start" });
  const startList = make("div");
  const startMessage = make("p", "", { "aria-live": "polite" });
  const progress = make("progress", "", { id: "save-progress", max: "1", value: "0" });
  progress.hidden = true;
  const newButton = make("button", "新しく始める", { type: "button", id: "save-new" });
  startDialog.append(make("p", "続きから始めるか、新しく始めるかを選ぶ。"), startList, newButton);
  startDialog.append(progress, startMessage);
  // Esc で閉じると始め方が決まらないので、閉じさせない
  startDialog.addEventListener("cancel", (event) => event.preventDefault());

  const saveButton = make("button", "保存", { type: "button", id: "save-open" });
  const saveDialog = make("dialog", "", { id: "save-dialog" });
  const saveList = make("div");
  const saveResult = make("p", "", { "aria-live": "polite" });
  const closeButton = make("button", "閉じる", { type: "button" });
  saveDialog.append(make("p", "保存する枠を選ぶ。"), saveList, saveResult, closeButton);
  closeButton.addEventListener("click", () => saveDialog.close());
  saveButton.disabled = true;
  saveButton.addEventListener("click", () => {
    renderSaveList();
    saveResult.textContent = "";
    saveDialog.showModal();
  });
  controls.append(saveButton);
  const notice = make("p", "", { id: "save-notice", "aria-live": "polite" });
  controls.after(notice);
  document.body.append(startDialog, saveDialog);

  const describe = (slot: SlotId) => {
    const r = records.get(slot);
    return `${SLOT_NAMES[slot]}：${r === undefined ? "空き" : describeSave(r)}`;
  };

  function renderStartList(): void {
    startList.replaceChildren(
      ...SLOTS.filter((s) => records.has(s)).map((slot) => {
        const record = records.get(slot) as SaveRecord;
        const reason = unreadableReason(record, formatVersion);
        const button = make("button", "続きから", { type: "button", "data-load": slot });
        button.disabled = reason !== null;
        if (reason !== null) {
          button.dataset.unreadable = "true";
        }
        button.addEventListener("click", () => load(record));
        const row = make("p", `${describe(slot)} `);
        row.append(button);
        if (reason !== null) {
          row.append(` ${reason}`);
        }
        return row;
      }),
    );
  }

  function renderSaveList(): void {
    saveList.replaceChildren(
      ...MANUAL_SLOTS.map((slot) => {
        const button = make("button", "ここに保存", { type: "button", "data-save": slot });
        button.addEventListener("click", () => {
          saveResult.textContent = "保存している";
          send({ kind: "save:request", slot });
        });
        const row = make("p", `${describe(slot)} `);
        row.append(button);
        return row;
      }),
    );
  }

  const setStartBusy = (busy: boolean) => {
    for (const b of startDialog.querySelectorAll("button")) {
      b.disabled = busy || b.dataset.unreadable === "true";
    }
  };

  function load(record: SaveRecord): void {
    loading = record;
    setStartBusy(true);
    startMessage.textContent = `${SLOT_NAMES[record.slot]}を読み込んでいる`;
    progress.hidden = false;
    progress.value = 0;
    // 枠の記録は IndexedDB に残すので、複製を送る
    send({ kind: "save:start", save: { meta: record.meta, commands: record.commands.slice(0) } });
  }

  newButton.addEventListener("click", () => {
    loading = null;
    setStartBusy(true);
    send({ kind: "save:start", save: null });
  });

  async function hello(): Promise<void> {
    try {
      records = await (await storePromise).all();
    } catch (error) {
      notice.textContent = `保存データを開けなかった：${String(error)}`;
    }
    if (records.size === 0) {
      send({ kind: "save:start", save: null });
      return;
    }
    renderStartList();
    startDialog.showModal();
  }

  async function store(message: Extract<SaveFromWorker, { kind: "save:data" }>): Promise<void> {
    const record: SaveRecord = {
      slot: message.slot,
      savedAt: Date.now(),
      meta: message.meta,
      commands: message.commands,
    };
    try {
      await (await storePromise).put(record);
    } catch (error) {
      const text = `${SLOT_NAMES[message.slot]}に保存できなかった：${String(error)}`;
      notice.textContent = text;
      saveResult.textContent = text;
      return;
    }
    records.set(record.slot, record);
    // test:wasm が読む
    saveButton.dataset.lastSlot = record.slot;
    saveButton.dataset.lastHash = record.meta.stateHash;
    saveButton.dataset.lastMoney = record.meta.money;
    if (record.slot === "auto") {
      saveButton.dataset.autoSaves = String(Number(saveButton.dataset.autoSaves ?? 0) + 1);
    } else {
      saveResult.textContent = `${SLOT_NAMES[record.slot]}に保存した`;
      renderSaveList();
    }
  }

  worker.addEventListener("message", (event: MessageEvent<unknown>) => {
    const message = event.data;
    if (!isSaveFromWorker(message)) {
      return;
    }
    switch (message.kind) {
      case "save:hello":
        formatVersion = message.formatVersion;
        void hello();
        break;
      case "save:progress":
        progress.max = Math.max(message.target, 1);
        progress.value = message.tick;
        // test:wasm が読む
        progress.dataset.reports = String(Number(progress.dataset.reports ?? 0) + 1);
        startMessage.textContent = `読み込んでいる（tick ${message.tick} / ${message.target}）`;
        break;
      case "save:load-error":
        startMessage.textContent = `読み込めなかった：${message.message}`;
        progress.hidden = true;
        setStartBusy(false);
        break;
      case "save:started": {
        startDialog.close();
        saveButton.disabled = false;
        const loaded = message.loaded;
        if (loaded !== null && loading !== null) {
          // test:wasm が読む
          saveButton.dataset.rulesChanged = String(loaded.rulesChanged);
          saveButton.dataset.loadedHash = loaded.meta.stateHash;
          saveButton.dataset.loadedMoney = loaded.meta.money;
          notice.textContent = loaded.rulesChanged
            ? `${SLOT_NAMES[loading.slot]}を読み込んだが、シミュレーションの規則が変わったので、保存したときと違う街になった`
            : `${SLOT_NAMES[loading.slot]}を読み込んだ`;
        }
        break;
      }
      case "save:data":
        void store(message);
        break;
    }
  });
}
