// 開発用の「歩く人を出す」道具。地図で 2 つのマス（建物か道路）を選ぶと、そのあいだに歩く人を出す。
// 出すのはコマンドではなく、記録に入らない。そのため、使ったあとのフィードバックは記録を再生しても
// 同じ状態にならない。住民の予定から移動を作るまでの、描画を確かめるための入口である。

import type { Tile } from "./map/tools.ts";
import type { MainToWorker } from "./protocol/messages.ts";

/** 1 回に出す人数。 */
export const DEV_WALK_COUNT = 40;

export const DEV_WALK_TOOL = "dev-walks";

const NOTICE =
  "開発用：記録に入らないので、このあと送るフィードバックは記録を再生しても同じ状態にならない";

export type DevWalks = {
  /** 道具を選んでいるあいだに地図のマスを押したときに呼ぶ。 */
  click: (at: Tile) => void;
  /** Worker が出した人数を返したときに呼ぶ。 */
  spawned: (count: number) => void;
  /** 道具を切り替えたときや取り消したときに呼ぶ。選んだ出発のマスを忘れる。 */
  cancel: () => void;
};

export function setupDevWalks(deps: {
  button: HTMLButtonElement;
  info: HTMLElement;
  /** 一度でも使ったら出し続ける、再現できないことの印。 */
  flag: HTMLElement;
  map: HTMLCanvasElement;
  send: (message: MainToWorker) => void;
}): DevWalks {
  const { button, info, flag, map, send } = deps;
  // フィードバックを受けるサーバー（開発サーバーと preview）のときだけボタンを出す
  fetch("/__feedback")
    .then((response) => {
      button.hidden = response.status !== 204;
    })
    .catch(() => {});

  let start: Tile | null = null;
  const active = () => map.dataset.tool === DEV_WALK_TOOL;
  return {
    click(at) {
      if (!active()) {
        return;
      }
      if (start === null) {
        start = at;
        info.textContent = `行き先のマスを選ぶ。${NOTICE}`;
        return;
      }
      send({ kind: "dev-spawn-walks", from: start, to: at, count: DEV_WALK_COUNT });
      start = null;
    },
    cancel() {
      start = null;
      info.textContent = active() ? `出発のマスを選ぶ。${NOTICE}` : "";
    },
    spawned(count) {
      info.textContent = `歩く人を ${count} 人出した。${NOTICE}`;
      flag.hidden = false;
      // test:wasm が読む
      map.dataset.devWalks = String(Number(map.dataset.devWalks ?? 0) + count);
    },
  };
}
