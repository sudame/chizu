// 遊びながらフィードバックを残す欄。送ると、書いた文、地図のスクリーンショット、状態、記録を
// 開発サーバー（feedback-plugin.ts）へ送り、feedback/ に保存させる。

export type FeedbackDeps = {
  /** ゲームを一時停止する。 */
  pause: () => void;
  /** Worker に状態と記録を求める。 */
  requestState: () => Promise<{ state: string; commands: ArrayBuffer }>;
  /** 地図の canvas。描画の結果は preserveDrawingBuffer で残っている。 */
  map: HTMLCanvasElement;
  camera: () => { x: number; y: number; zoom: number };
};

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

  open.addEventListener("click", () => {
    deps.pause();
    result.textContent = "";
    dialog.showModal();
    note.focus();
  });

  sendButton.addEventListener("click", async (event) => {
    event.preventDefault();
    if (note.value.trim() === "") {
      result.textContent = "気づいたことを書いてから送る";
      return;
    }
    sendButton.disabled = true;
    try {
      const { state, commands } = await deps.requestState();
      const screenshot = deps.map.toDataURL("image/png").replace(/^data:image\/png;base64,/, "");
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
