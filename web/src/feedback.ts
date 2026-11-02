// 遊びながらフィードバックを残す欄。送ると、書いた文、地図のスクリーンショット、状態、記録を
// 開発サーバー（feedback-plugin.ts）へ送り、feedback/ に保存させる。

/** Worker の返事を待つ時間の上限（ミリ秒）。 */
const STATE_TIMEOUT_MS = 5_000;

export type FeedbackDeps = {
  isPaused: () => boolean;
  setPaused: (paused: boolean) => void;
  /** Worker に状態と記録を求める。 */
  requestState: () => Promise<{ state: string; commands: ArrayBuffer }>;
  /** 地図の canvas。描画の結果は preserveDrawingBuffer で残っている。 */
  map: HTMLCanvasElement;
  /** 地図の上に重ねて描いた canvas（ラベルなど）。スクリーンショットに重ねる。 */
  overlays: HTMLCanvasElement[];
  camera: () => { x: number; y: number; zoom: number };
};

/** 地図の canvas に、上に重ねた canvas を重ねた絵を PNG の base64 にする。 */
function screenshotOf(map: HTMLCanvasElement, overlays: HTMLCanvasElement[]): string {
  const out = document.createElement("canvas");
  out.width = map.width;
  out.height = map.height;
  const ctx = out.getContext("2d");
  if (ctx === null) {
    return map.toDataURL("image/png").replace(/^data:image\/png;base64,/, "");
  }
  ctx.drawImage(map, 0, 0);
  for (const overlay of overlays) {
    if (overlay.width > 0 && overlay.height > 0) {
      ctx.drawImage(overlay, 0, 0, out.width, out.height);
    }
  }
  return out.toDataURL("image/png").replace(/^data:image\/png;base64,/, "");
}

function toBase64(bytes: Uint8Array): string {
  let text = "";
  for (const b of bytes) {
    text += String.fromCharCode(b);
  }
  return btoa(text);
}

export function setupFeedback(deps: FeedbackDeps): void {
  const find = <T extends Element>(selector: string): T => {
    const found = document.querySelector<T>(selector);
    if (found === null) {
      throw new Error(`${selector} が見つからない`);
    }
    return found;
  };
  const open = find<HTMLButtonElement>("#feedback-open");
  const dialog = find<HTMLDialogElement>("#feedback");
  const note = find<HTMLTextAreaElement>("#feedback-note");
  const sendButton = find<HTMLButtonElement>("#feedback-send");
  const result = find<HTMLElement>("#feedback-result");

  // フィードバックを受けるサーバー（開発サーバーと preview）のときだけボタンを出す
  fetch("/__feedback")
    .then((response) => {
      open.hidden = response.status !== 204;
    })
    .catch(() => {});

  // 開く前に止まっていたかを覚えておき、閉じたら（Esc で閉じても）元に戻す
  let wasPaused = false;
  open.addEventListener("click", () => {
    wasPaused = deps.isPaused();
    deps.setPaused(true);
    result.textContent = "";
    delete result.dataset.saved;
    dialog.showModal();
    note.focus();
  });
  dialog.addEventListener("close", () => deps.setPaused(wasPaused));

  const requestState = () =>
    Promise.race([
      deps.requestState(),
      new Promise<never>((_, reject) =>
        setTimeout(() => reject(new Error("WASM から返事がない")), STATE_TIMEOUT_MS),
      ),
    ]);

  sendButton.addEventListener("click", async (event) => {
    event.preventDefault();
    if (note.value.trim() === "") {
      result.textContent = "気づいたことを書いてから送る";
      return;
    }
    sendButton.disabled = true;
    try {
      const { state, commands } = await requestState();
      const screenshot = screenshotOf(deps.map, deps.overlays);
      const response = await fetch("/__feedback", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          note: note.value,
          screenshot,
          state: { ...JSON.parse(state), camera: deps.camera() },
          commands: toBase64(new Uint8Array(commands)),
        }),
      });
      const saved: unknown = await response.json();
      if (!response.ok || typeof saved !== "object" || saved === null || !("dir" in saved)) {
        throw new Error(JSON.stringify(saved));
      }
      result.textContent = `保存した：${String(saved.dir)}`;
      // test:wasm が読む
      result.dataset.saved = String(saved.dir);
      note.value = "";
    } catch (error) {
      result.textContent = `保存できなかった（mise run dev で開いているか確かめる）：${String(error)}`;
    } finally {
      sendButton.disabled = false;
    }
  });
}
