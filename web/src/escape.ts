// Esc を押したとき、1 回につき 1 段だけ戻る順を決める。
// 何度か押せば、どの状態からでも「見る」の道具に着き、最後は開いているパネルも閉じる。

export type EscapeState = {
  /** 道路の始点を置いた、範囲を引いている、など、引きかけの操作がある */
  drawing: boolean;
  /** 道具の 2 段目、レイヤー、収支などの浮いたパネルが開いている */
  popoverOpen: boolean;
  /** 「見る」以外の道具を選んでいる */
  toolSelected: boolean;
  /** 「見る」で押したものの検査の欄が開いている */
  inspectorOpen: boolean;
};

export type EscapeStep =
  | "cancel-drawing"
  | "close-popover"
  | "select-view"
  | "close-inspector"
  | "nothing";

export function nextEscapeStep(state: EscapeState): EscapeStep {
  if (state.drawing) {
    return "cancel-drawing";
  }
  if (state.popoverOpen) {
    return "close-popover";
  }
  if (state.toolSelected) {
    return "select-view";
  }
  if (state.inspectorOpen) {
    return "close-inspector";
  }
  return "nothing";
}
