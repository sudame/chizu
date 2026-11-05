// 画面の側の保存と読み込み。起動したときに続きから始めるか新しく始めるかを選ばせ、
// Worker から届いた保存データを IndexedDB に書く。手動保存の欄もここで作る。

import { resetUtilityTrends } from "../city-panel.ts";
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

const MANUAL_SLOTS = SLOTS.filter((s) => s.startsWith("manual-"));
const AUTO_SAVE_LOCK = "chizu-auto-save";

// 自動保存の枠に書けるタブを 1 つに限る。取れたら、ページを閉じるまで持ち続ける
function holdAutoSaveLock(): Promise<boolean> {
  if (!("locks" in navigator)) {
    return Promise.resolve(true);
  }
  return new Promise((resolve) => {
    navigator.locks
      .request(AUTO_SAVE_LOCK, { ifAvailable: true }, (lock) => {
        resolve(lock !== null);
        return lock === null ? undefined : new Promise<never>(() => {});
      })
      .catch(() => resolve(true));
  });
}

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

/** うまくいった知らせを出しておく長さ。 */
const NOTICE_MS = 8000;

/** 消える知らせ（うまくいったこと）か、閉じるまで残る知らせ（続く状態、失敗、規則の変化）か。 */
type NoticeKind = "transient" | "sticky";

export function setupSaves(worker: Worker, controls: HTMLElement): void {
  const send = (message: SaveToWorker, transfer: Transferable[] = []) =>
    worker.postMessage(message, transfer);
  const storePromise = SaveStore.open();
  const lockPromise = holdAutoSaveLock();
  let records = new Map<SlotId, SaveRecord>();
  let broken = new Set<SlotId>();
  let formatVersion = 0;
  // 保存データを読めないときは false にして、保存を使わずに遊ぶ
  let storage = true;
  let autoSave = true;
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
  // うまくいったことの知らせ（保存した、読み込んだ）は、地図を覆い続けないよう数秒で消す。要素は残すので、文面は消えるまで読める
  const noticeEl = make("p", "", { id: "save-notice", "aria-live": "polite", role: "status" });
  noticeEl.hidden = true;
  // 続いている状態や、気づかないと困る失敗の知らせは、閉じるまで残す
  const alertsEl = make("div", "", { id: "save-alerts" });
  const noticeHost = document.querySelector("#notices") ?? controls;
  // 地図のラベルは、この 2 つの下に置かない
  noticeEl.dataset.mapPanel = "";
  alertsEl.dataset.mapPanel = "";
  noticeHost.append(noticeEl, alertsEl);
  let noticeTimer: ReturnType<typeof setTimeout> | undefined;
  const showNotice = (text: string, kind: NoticeKind = "transient") => {
    if (kind === "sticky") {
      for (const item of alertsEl.children) {
        if (item.firstElementChild?.textContent === text) {
          return;
        }
      }
      const item = make("div", "", { class: "save-alert", role: "status" });
      const close = make("button", "×", { type: "button", "aria-label": "閉じる" });
      close.addEventListener("click", () => item.remove());
      item.append(make("span", text), close);
      alertsEl.append(item);
      return;
    }
    noticeEl.textContent = text;
    noticeEl.hidden = false;
    clearTimeout(noticeTimer);
    noticeTimer = setTimeout(() => {
      noticeEl.hidden = true;
    }, NOTICE_MS);
  };
  document.body.append(startDialog, saveDialog);

  const BROKEN = "壊れている";
  const describe = (slot: SlotId) => {
    const r = records.get(slot);
    if (broken.has(slot)) {
      return `${SLOT_NAMES[slot]}：${BROKEN}`;
    }
    return `${SLOT_NAMES[slot]}：${r === undefined ? "空き" : describeSave(r)}`;
  };

  function startRow(slot: SlotId): HTMLElement {
    const record = records.get(slot);
    const button = make("button", "続きから", { type: "button", "data-load": slot });
    let text: string;
    let reason: string | null;
    try {
      text = describe(slot);
      reason = record === undefined ? BROKEN : unreadableReason(record, formatVersion);
    } catch {
      text = `${SLOT_NAMES[slot]}：${BROKEN}`;
      reason = BROKEN;
    }
    button.disabled = reason !== null;
    if (reason !== null) {
      button.dataset.unreadable = "true";
    }
    if (record !== undefined) {
      button.addEventListener("click", () => load(record));
    }
    const row = make("p", `${text} `);
    row.append(button);
    if (reason !== null && reason !== BROKEN) {
      row.append(` ${reason}`);
    }
    return row;
  }

  function renderStartList(): void {
    startList.replaceChildren(
      ...SLOTS.filter((s) => records.has(s) || broken.has(s)).map(startRow),
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
    // 枠の記録と街の状態は IndexedDB に残すので、複製を送る
    send({
      kind: "save:start",
      save: {
        meta: record.meta,
        commands: record.commands.slice(0),
        state: record.state?.slice(0) ?? null,
      },
      autoSave: storage && autoSave,
    });
  }

  // 自動保存があれば、新しい街の自動保存で消えないように「自動保存（前の街）」へ退避してから始める
  async function startNew(): Promise<void> {
    loading = null;
    setStartBusy(true);
    const previous = records.get("auto");
    if (storage && autoSave && previous !== undefined) {
      const moved: SaveRecord = { ...previous, slot: "auto-previous" };
      try {
        await (await storePromise).put(moved);
        records.set(moved.slot, moved);
      } catch (error) {
        startMessage.textContent = `前の自動保存を退避できなかったので、始めなかった：${String(error)}`;
        setStartBusy(false);
        return;
      }
    }
    send({ kind: "save:start", save: null, autoSave: storage && autoSave });
  }
  newButton.addEventListener("click", () => void startNew());

  // どこで失敗しても、選ぶ画面と始めるボタンは出す
  async function hello(): Promise<void> {
    try {
      autoSave = await lockPromise;
      if (!autoSave) {
        showNotice("別のタブで遊んでいるので、このタブでは自動保存しない", "sticky");
      }
      try {
        ({ records, broken } = await (await storePromise).list());
      } catch (error) {
        // 読めないまま新しく始めると、自動保存で前の街を上書きしかねない
        storage = false;
        startMessage.textContent = `保存データを読めなかった：${String(error)}`;
        newButton.textContent = "保存を使わずに始める（自動保存は切る）";
        startDialog.showModal();
        return;
      }
      if (records.size === 0 && broken.size === 0) {
        await startNew();
        return;
      }
      renderStartList();
    } catch (error) {
      startMessage.textContent = `保存データを並べられなかった：${String(error)}`;
      setStartBusy(false);
    }
    if (!startDialog.open) {
      startDialog.showModal();
    }
  }

  async function store(message: Extract<SaveFromWorker, { kind: "save:data" }>): Promise<void> {
    if (!storage || (message.slot === "auto" && !autoSave)) {
      return;
    }
    const record: SaveRecord = {
      slot: message.slot,
      savedAt: Date.now(),
      meta: message.meta,
      commands: message.commands,
      state: message.state,
    };
    try {
      await (await storePromise).put(record);
    } catch (error) {
      const text = `${SLOT_NAMES[message.slot]}に保存できなかった：${String(error)}`;
      showNotice(text, "sticky");
      saveResult.textContent = text;
      return;
    }
    records.set(record.slot, record);
    broken.delete(record.slot);
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
        // 別の街になるので、余りの減り方の控えは捨てる
        resetUtilityTrends();
        saveButton.disabled = !storage;
        const loaded = message.loaded;
        if (loaded !== null && loading !== null) {
          // test:wasm が読む
          saveButton.dataset.rulesChanged = String(loaded.rulesChanged);
          saveButton.dataset.fromState = String(loaded.fromState);
          saveButton.dataset.loadedHash = loaded.meta.stateHash;
          saveButton.dataset.loadedMoney = loaded.meta.money;
          if (loaded.rulesChanged) {
            showNotice(
              `${SLOT_NAMES[loading.slot]}を読み込んだが、シミュレーションの規則が変わったので、保存したときと違う街になった`,
              "sticky",
            );
          } else {
            showNotice(`${SLOT_NAMES[loading.slot]}を読み込んだ`);
          }
        }
        break;
      }
      case "save:data":
        void store(message);
        break;
    }
  });
}
