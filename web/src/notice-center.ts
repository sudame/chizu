import { pickStall } from "./growth-notice.ts";

// 通知センター。警告の知らせを、画面の隅の鐘の印と数の印、押すと開く一覧にまとめる。
// 新しい知らせは、小さな札で 1 回だけ数秒知らせる。解けた知らせは一覧から消え、閉じた知らせは状態が変わるまで出し直さない。

/** 移る先のマス。 */
export type Target = { x: number; y: number };

/**
 * 知らせ 1 つ。id は知らせの種類で、key は「状態」を表す。
 * 件数のように揺れる値は key に入れない。key が変わると、閉じた知らせも出し直す。
 */
export type Notice = {
  id: string;
  key: string;
  headline: string;
  details: string[];
  jump: { label: string; targets: Target[] } | null;
};

/** 覚えている知らせ。key と、最後に見えたゲームの tick。 */
type Seen = { key: string; seen: number };

/** 出した知らせと、閉じた知らせを覚えておく。 */
export type Memory = { announced: Map<string, Seen>; dismissed: Map<string, Seen> };

export function emptyMemory(): Memory {
  return { announced: new Map(), dismissed: new Map() };
}

/**
 * 解けた知らせを覚えておくゲームの中の長さ（tick）。ゲーム内の半日である（1 日は 7200 tick）。
 * 区画の知らせは、建つたびに出入りしてちらつく。そのあいだに同じ状態で戻った知らせは、
 * 新しい知らせとして札を出さず、閉じた印も残す。
 */
export const RESOLVED_MEMORY_TICKS = 3600;

export type Reconciled = {
  memory: Memory;
  /** 一覧に出す知らせ（閉じていないもの） */
  visible: Notice[];
  /** 今回初めて出る知らせ。札で知らせる */
  fresh: Notice[];
};

/** 解けた知らせの覚えを、RESOLVED_MEMORY_TICKS のあいだ引き継ぐ。tick が戻ったら（読み込みや新しい街）捨てる。 */
function carry(
  from: Map<string, Seen>,
  into: Map<string, Seen>,
  present: Set<string>,
  now: number,
) {
  for (const [id, entry] of from) {
    const age = now - entry.seen;
    if (!present.has(id) && age >= 0 && age < RESOLVED_MEMORY_TICKS) {
      into.set(id, entry);
    }
  }
}

/**
 * 今ある知らせと覚えておいたものを突き合わせる。now はゲームの tick。
 * 解けた知らせの覚えは、RESOLVED_MEMORY_TICKS が過ぎると外れ、そのあとまた起きれば新しい知らせとして出る。
 */
export function reconcile(previous: Memory, notices: Notice[], now: number): Reconciled {
  // tick が戻ったら、別の街の覚えなので捨てる
  const future = [...previous.announced.values(), ...previous.dismissed.values()].some(
    (e) => e.seen > now,
  );
  const memory = future ? emptyMemory() : previous;
  const dismissed = new Map<string, Seen>();
  const announced = new Map<string, Seen>();
  const visible: Notice[] = [];
  const fresh: Notice[] = [];
  for (const n of notices) {
    if (memory.dismissed.get(n.id)?.key === n.key) {
      dismissed.set(n.id, { key: n.key, seen: now });
      continue;
    }
    visible.push(n);
    announced.set(n.id, { key: n.key, seen: now });
    if (memory.announced.get(n.id)?.key !== n.key) {
      fresh.push(n);
    }
  }
  const present = new Set(notices.map((n) => n.id));
  carry(memory.announced, announced, present, now);
  carry(memory.dismissed, dismissed, present, now);
  return { memory: { announced, dismissed }, visible, fresh };
}

/** 知らせを閉じる。いま出ている key のままなら、出し直さない。 */
export function dismiss(memory: Memory, notice: Notice, now: number): Memory {
  const announced = new Map(memory.announced);
  announced.delete(notice.id);
  const dismissed = new Map(memory.dismissed);
  dismissed.set(notice.id, { key: notice.key, seen: now });
  return { announced, dismissed };
}

/**
 * 項目を閉じたり解けたりして消えたとき、フォーカスを移す先。
 * 次の項目、なければ前の項目、なければ null（鐘へ戻す）。order は消える前の並び。
 */
export function focusAfterRemoval(order: string[], removed: string): string | null {
  const at = order.indexOf(removed);
  if (at < 0) {
    return null;
  }
  const rest = order.filter((id) => id !== removed);
  return rest[at] ?? rest[at - 1] ?? null;
}

/** 札で知らせる文。1 件ならその見出し、複数なら件数。 */
export function toastText(fresh: Notice[]): string {
  const [first] = fresh;
  if (first === undefined) {
    return "";
  }
  return fresh.length === 1 ? first.headline : `新しい知らせが ${fresh.length} 件ある`;
}

/** 鐘の読み上げの文。 */
export function bellLabel(count: number): string {
  return count === 0 ? "知らせ" : `知らせ ${count} 件`;
}

/** 数の印の文字。大きい数は 9+ にまとめる。 */
export function badgeText(count: number): string {
  return count > 9 ? "9+" : String(count);
}

/** 札を出しておく時間（ミリ秒）。 */
export const TOAST_MS = 6000;

type Item = {
  li: HTMLLIElement;
  head: HTMLElement;
  details: HTMLElement;
  jump: HTMLButtonElement;
  close: HTMLButtonElement;
  last: Target | null;
  targets: Target[];
  notice: Notice;
};

export type NoticeCenter = {
  /** 出どころごとに、いまある知らせを丸ごと渡す。出どころの順に一覧へ並ぶ */
  set(source: string, notices: Notice[]): void;
};

/**
 * 要素は index.html に置いてある。ここでは文言と出し入れだけを書き換える。
 * 一覧を開閉するのは呼び出し側で、open は札を押したときに一覧を開くのに使う。
 */
export function setupNoticeCenter(
  jump: (x: number, y: number) => void,
  open: () => void,
  isOpen: () => boolean,
  now: () => number,
): NoticeCenter {
  const bell = document.querySelector<HTMLElement>("#notice-bell");
  const badge = document.querySelector<HTMLElement>("#notice-count");
  const list = document.querySelector<HTMLElement>("#notice-list");
  const empty = document.querySelector<HTMLElement>("#notice-empty");
  const toast = document.querySelector<HTMLButtonElement>("#notice-toast");
  const sources = new Map<string, Notice[]>();
  const items = new Map<string, Item>();
  let memory = emptyMemory();
  let toastTimer: ReturnType<typeof setTimeout> | undefined;

  toast?.addEventListener("click", (event) => {
    toast.hidden = true;
    open();
    // キーボードで押したときは、一覧の最初の項目へフォーカスを入れる
    if (event.detail === 0) {
      list?.querySelector<HTMLElement>("button:not([hidden])")?.focus();
    }
  });

  const make = (notice: Notice): Item => {
    const li = document.createElement("li");
    li.className = "notice-item";
    const head = document.createElement("strong");
    const details = document.createElement("div");
    const row = document.createElement("div");
    row.className = "notice-actions";
    const jumpButton = document.createElement("button");
    jumpButton.type = "button";
    jumpButton.className = "chip";
    const close = document.createElement("button");
    close.type = "button";
    close.className = "chip";
    close.textContent = "閉じる";
    row.append(jumpButton, close);
    li.append(head, details, row);
    const item: Item = {
      li,
      head,
      details,
      jump: jumpButton,
      close,
      last: null,
      targets: [],
      notice,
    };
    jumpButton.addEventListener("click", () => {
      const picked = pickStall(item.targets, item.last);
      if (picked === null) {
        return;
      }
      item.last = { x: picked.x, y: picked.y };
      jump(picked.x, picked.y);
    });
    close.addEventListener("click", () => {
      memory = dismiss(memory, item.notice, now());
      refresh();
    });
    return item;
  };

  const refresh = () => {
    const all = [...sources.values()].flat();
    const result = reconcile(memory, all, now());
    memory = result.memory;
    const ids = new Set(result.visible.map((n) => n.id));
    const before =
      list === null ? [] : [...list.children].map((c) => (c as HTMLElement).dataset.notice ?? "");
    // 項目が消えるとき、その中にあったフォーカスは、次の項目か鐘へ移す
    let refocus: { id: string | null } | null = null;
    for (const [id, item] of items) {
      if (!ids.has(id)) {
        if (item.li.contains(document.activeElement)) {
          refocus = {
            id: focusAfterRemoval(
              before.filter((b) => ids.has(b) || b === id),
              id,
            ),
          };
        }
        item.li.remove();
        items.delete(id);
      }
    }
    for (const notice of result.visible) {
      let item = items.get(notice.id);
      if (item === undefined) {
        item = make(notice);
        items.set(notice.id, item);
      }
      item.notice = notice;
      // 同じ文なら DOM に触らない。触ると読み上げが繰り返され、フォーカスも外れる
      if (item.head.textContent !== notice.headline) {
        item.head.textContent = notice.headline;
      }
      const detailText = notice.details.join("\n");
      if (item.details.dataset.text !== detailText) {
        item.details.dataset.text = detailText;
        item.details.replaceChildren(
          ...notice.details.map((line) => {
            const div = document.createElement("div");
            div.textContent = line;
            return div;
          }),
        );
      }
      item.targets = notice.jump?.targets ?? [];
      if (item.targets.length === 0) {
        item.last = null;
      }
      item.jump.hidden = notice.jump === null;
      const label = notice.jump?.label ?? "";
      if (item.jump.textContent !== label) {
        item.jump.textContent = label;
      }
      item.li.dataset.notice = notice.id;
    }
    if (list !== null) {
      const want = result.visible.map((n) => items.get(n.id)?.li).filter((e) => e !== undefined);
      // 残る項目はそのまま残し、足りない所だけ差し込む。作り直すとフォーカスが外れる
      want.forEach((li, i) => {
        if (list.children[i] !== li) {
          list.insertBefore(li, list.children[i] ?? null);
        }
      });
    }
    if (refocus !== null) {
      const next = refocus.id === null ? undefined : items.get(refocus.id);
      const target = next?.li.querySelector<HTMLElement>("button:not([hidden])") ?? bell;
      target?.focus();
    }
    const count = result.visible.length;
    if (empty !== null) {
      empty.hidden = count > 0;
    }
    if (badge !== null) {
      badge.hidden = count === 0;
      badge.textContent = badgeText(count);
    }
    if (bell !== null) {
      bell.setAttribute("aria-label", bellLabel(count));
      bell.dataset.count = String(count);
    }
    if (toast !== null && result.fresh.length > 0 && !isOpen()) {
      toast.textContent = toastText(result.fresh);
      toast.hidden = false;
      toast.dataset.count = String(Number(toast.dataset.count ?? 0) + 1);
      clearTimeout(toastTimer);
      toastTimer = setTimeout(() => {
        toast.hidden = true;
      }, TOAST_MS);
    } else if (toast !== null && count === 0) {
      toast.hidden = true;
    }
  };

  return {
    set(source, notices) {
      sources.set(source, notices);
      refresh();
    },
  };
}
