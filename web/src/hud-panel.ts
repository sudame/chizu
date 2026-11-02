// 左上の帯の下にある、たためる欄。たたんだかどうかは localStorage に覚える。
// localStorage は使えないことがある（プライベートウィンドウ、サイトデータの遮断）ので、読み書きは握りつぶして動き続ける。

/** 保存先の鍵。 */
export const HUD_COLLAPSED_KEY = "chizu.hud.collapsed";

/** 覚えているものを読む。覚えていない、読めないときは、たたんだ状態（true）にする。 */
export function readCollapsed(storage: Pick<Storage, "getItem"> | null): boolean {
  try {
    return storage?.getItem(HUD_COLLAPSED_KEY) !== "0";
  } catch {
    return true;
  }
}

/** たたんだかどうかを覚える。書けなくても何もしない。 */
export function writeCollapsed(storage: Pick<Storage, "setItem"> | null, collapsed: boolean): void {
  try {
    storage?.setItem(HUD_COLLAPSED_KEY, collapsed ? "1" : "0");
  } catch {
    // 覚えられないだけで、欄は動く
  }
}

/** window.localStorage を取る。アクセスだけで例外になることがあるので、取れなければ null にする。 */
function browserStorage(): Storage | null {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

/** 欄を出し入れするボタンと、欄の中身をつなぐ。 */
export function setupHudPanel(toggle: HTMLButtonElement, panel: HTMLElement): void {
  const storage = browserStorage();
  const apply = (collapsed: boolean) => {
    panel.hidden = collapsed;
    toggle.setAttribute("aria-expanded", String(!collapsed));
    toggle.textContent = collapsed ? "詳細 ▸" : "詳細 ▾";
  };
  apply(readCollapsed(storage));
  toggle.addEventListener("click", () => {
    const collapsed = !panel.hidden;
    apply(collapsed);
    writeCollapsed(storage, collapsed);
  });
}

/**
 * 下の道具の帯の高さを CSS の変数 --toolbar-h に入れる。
 * 帯は幅が狭いと折り返して高くなるので、その上に置くパネルが帯に重ならないよう、この変数を基準に置く。
 */
export function trackToolbarHeight(toolbar: HTMLElement): void {
  const set = () => {
    document.documentElement.style.setProperty("--toolbar-h", `${toolbar.offsetHeight}px`);
  };
  set();
  if (typeof ResizeObserver !== "undefined") {
    new ResizeObserver(set).observe(toolbar);
  }
}
