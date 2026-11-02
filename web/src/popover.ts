// ボタンを押すと開く、浮いたパネル（道具の 2 段目、レイヤーの選択、収支）。
// 開いているのは同時に 1 つだけで、パネルの外を押すか Esc を押すと閉じる。

export type Popover = {
  open(): void;
  close(): void;
  isOpen(): boolean;
};

export type Popovers = {
  /** toggle を押すと panel を出し入れするようにつなぐ。 */
  add(toggle: HTMLElement, panel: HTMLElement): Popover;
  closeAll(): void;
};

export function createPopovers(doc: Document = document): Popovers {
  const entries: { toggle: HTMLElement; panel: HTMLElement; popover: Popover }[] = [];

  const closeAll = () => {
    for (const e of entries) {
      e.popover.close();
    }
  };

  doc.addEventListener("pointerdown", (event) => {
    const target = event.target;
    if (!(target instanceof Node)) {
      return;
    }
    for (const e of entries) {
      if (e.popover.isOpen() && !e.panel.contains(target) && !e.toggle.contains(target)) {
        e.popover.close();
      }
    }
  });

  doc.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") {
      return;
    }
    for (const e of entries) {
      if (e.popover.isOpen()) {
        // パネルの中にいたときは、押したボタンへ戻す。外にいたときは動かさない
        const inside = e.panel.contains(doc.activeElement);
        e.popover.close();
        if (inside) {
          e.toggle.focus();
        }
      }
    }
  });

  return {
    closeAll,
    add(toggle, panel) {
      const popover: Popover = {
        open() {
          for (const e of entries) {
            if (e.popover !== popover) {
              e.popover.close();
            }
          }
          panel.hidden = false;
          toggle.setAttribute("aria-expanded", "true");
        },
        close() {
          panel.hidden = true;
          toggle.setAttribute("aria-expanded", "false");
        },
        isOpen: () => !panel.hidden,
      };
      entries.push({ toggle, panel, popover });
      popover.close();
      toggle.addEventListener("click", (event) => {
        if (popover.isOpen()) {
          popover.close();
          return;
        }
        popover.open();
        // キーボードで開いたときは、Tab で中へ入れるよう、最初のボタンへ動かす
        if (event.detail === 0) {
          panel.querySelector<HTMLElement>("button:not([disabled])")?.focus();
        }
      });
      return popover;
    },
  };
}
