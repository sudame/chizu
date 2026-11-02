// 収支のパネル。区画の種類ごとの税収、道路と施設の維持費、1 日あたりの差し引きを出し、税率を上げ下げできる。
// DOM に触るのは setupFinance だけで、数の読み方と文言は Vitest で確かめられる。

import { FINANCE_LENGTH, type ZoneName } from "./protocol/messages.ts";

export type TaxZone = Exclude<ZoneName, "Clear">;

/** 税率をかける区画の種類。chizu-wasm の finance の並びと同じ。 */
export const TAX_ZONES: { zone: TaxZone; label: string }[] = [
  { zone: "Residential", label: "住宅" },
  { zone: "Commercial", label: "商業" },
  { zone: "Industrial", label: "工業" },
  { zone: "Office", label: "オフィス" },
];

/** ボタン 1 回で動かす税率（‰）。1% である。 */
export const RATE_STEP_PERMILLE = 10;

/** chizu-wasm の finance を読んだもの。金額はどれも 1 日あたり。 */
export type Finance = {
  /** 税率の上限（‰）。 */
  maxPermille: number;
  /** 住宅・商業・工業・オフィスの順の税率（‰）。 */
  permille: number[];
  /** 同じ順の税収。 */
  taxes: number[];
  roadUpkeep: number;
  powerUpkeep: number;
  waterUpkeep: number;
  income: number;
  expenses: number;
  /** 収入から支出を引いた額。負なら赤字。 */
  net: number;
};

export function parseFinance(values: ArrayLike<number>): Finance | null {
  if (values.length !== FINANCE_LENGTH) {
    return null;
  }
  const at = (i: number) => values[i] ?? 0;
  const taxes = [at(5), at(6), at(7), at(8)];
  const income = taxes.reduce((a, b) => a + b, 0);
  const expenses = at(9) + at(10) + at(11);
  return {
    maxPermille: at(0),
    permille: [at(1), at(2), at(3), at(4)],
    taxes,
    roadUpkeep: at(9),
    powerUpkeep: at(10),
    waterUpkeep: at(11),
    income,
    expenses,
    net: income - expenses,
  };
}

export const isDeficit = (finance: Finance): boolean => finance.net < 0;

/** 桁区切りの金額。 */
export function formatMoney(value: number): string {
  return value.toLocaleString("ja-JP");
}

/** 符号つきの金額。0 は符号をつけない。 */
export function formatSigned(value: number): string {
  if (value === 0) {
    return "0";
  }
  return value > 0 ? `+${formatMoney(value)}` : formatMoney(value);
}

/** 税率（‰）を % で。1% 刻みなら整数、そうでなければ小数 1 桁。 */
export function formatRate(permille: number): string {
  return permille % 10 === 0 ? `${permille / 10}%` : `${(permille / 10).toFixed(1)}%`;
}

/** 開け閉めの見出しに出す 1 行。赤字のときはそれが分かる文を添える。 */
export function summaryText(finance: Finance | null): string {
  if (finance === null) {
    return "収支";
  }
  const text = `収支 ${formatSigned(finance.net)} / 日`;
  return isDeficit(finance) ? `${text}（赤字）` : text;
}

/** 所持金の横に添える文。赤字でなければ空。 */
export function deficitNote(finance: Finance | null): string {
  return finance !== null && isDeficit(finance) ? `赤字 ${formatSigned(finance.net)} / 日` : "";
}

/** 税率を 1 刻み動かした値。0 と上限のあいだに収める。 */
export function steppedRate(current: number, direction: 1 | -1, max: number): number {
  return Math.max(0, Math.min(max, current + direction * RATE_STEP_PERMILLE));
}

export function taxCommand(
  zone: TaxZone,
  permille: number,
): { kind: "SetTaxRate"; zone: TaxZone; permille: number } {
  return { kind: "SetTaxRate", zone, permille };
}

export type FinanceRow = { label: string; value: string };

/** 税収、維持費、差し引きの行。税率の欄は含めない。 */
export function financeRows(finance: Finance): {
  taxes: FinanceRow[];
  expenses: FinanceRow[];
  net: FinanceRow;
} {
  return {
    taxes: TAX_ZONES.map(({ label }, i) => ({
      label,
      value: formatSigned(finance.taxes[i] ?? 0),
    })),
    expenses: [
      { label: "道路の維持費", value: formatSigned(-finance.roadUpkeep) },
      { label: "発電所の維持費", value: formatSigned(-finance.powerUpkeep) },
      { label: "浄水場の維持費", value: formatSigned(-finance.waterUpkeep) },
    ],
    net: { label: "差し引き", value: formatSigned(finance.net) },
  };
}

/**
 * 押した先の税率を、中核が反映するまで覚えておく。
 * 反映前の finance が遅れて届いても表示を戻さず、続けて押したときも古い値から数え直さない。
 */
export class PendingRates {
  private readonly wanted = new Map<TaxZone, number>();

  /** 表示する税率。押した先があればそれ、なければ中核の値。 */
  shown(zone: TaxZone, received: number): number {
    return this.wanted.get(zone) ?? received;
  }

  /** 1 刻み動かした先を覚えて返す。動かせない（端）ときは null。 */
  press(zone: TaxZone, direction: 1 | -1, received: number, max: number): number | null {
    const current = this.shown(zone, received);
    const next = steppedRate(current, direction, max);
    if (next === current) {
      return null;
    }
    this.wanted.set(zone, next);
    return next;
  }

  /** finance が届いた。押した先と一致した区画だけ、反映済みとして忘れる。 */
  receive(permille: readonly number[]): void {
    TAX_ZONES.forEach(({ zone }, i) => {
      if (this.wanted.get(zone) === permille[i]) {
        this.wanted.delete(zone);
      }
    });
  }

  /** 断られたので、押した先をすべて忘れて中核の値に戻す。 */
  reject(): void {
    this.wanted.clear();
  }
}

export type FinancePanel = {
  update(finance: Finance): void;
  /** SetTaxRate が断られたときに呼ぶ。 */
  rejected(): void;
};

function make<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  text = "",
  className = "",
): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  el.textContent = text;
  if (className !== "") {
    el.className = className;
  }
  return el;
}

/**
 * 開け閉めするパネルを組む。税率のボタンは SetTaxRate を send に渡す。
 * 返事が届くまでに続けて押しても 1 つ飛ばさないよう、押した先の税率を覚えておき、次の finance が届いたら捨てる。
 */
export function setupFinance(
  root: HTMLDetailsElement,
  send: (command: ReturnType<typeof taxCommand>) => void,
): FinancePanel {
  const summary = root.querySelector<HTMLElement>("summary");
  const body = root.querySelector<HTMLElement>("[data-finance-body]");
  if (summary === null || body === null) {
    throw new Error("収支のパネルの要素が足りない");
  }
  let latest: Finance | null = null;
  const pending = new PendingRates();

  const taxList = make("div", "", "finance-list");
  const expenseList = make("div", "", "finance-list");
  const netRow = make("div", "", "finance-row finance-net");
  const netLabel = make("span");
  const netValue = make("span");
  netRow.append(netLabel, netValue);
  const rateLabels: HTMLElement[] = [];
  const amounts: HTMLElement[] = [];
  const expenseValues: HTMLElement[] = [];

  TAX_ZONES.forEach(({ zone, label }, i) => {
    const row = make("div", "", "finance-row");
    row.dataset.zone = zone;
    const name = make("span", label);
    const minus = make("button", "−");
    const plus = make("button", "＋");
    minus.type = "button";
    plus.type = "button";
    minus.setAttribute("aria-label", `${label}の税率を下げる`);
    plus.setAttribute("aria-label", `${label}の税率を上げる`);
    const rate = make("span", "", "finance-rate");
    const amount = make("span", "", "finance-amount");
    const change = (direction: 1 | -1) => {
      if (latest === null) {
        return;
      }
      const next = pending.press(zone, direction, latest.permille[i] ?? 0, latest.maxPermille);
      if (next === null) {
        return;
      }
      rate.textContent = formatRate(next);
      send(taxCommand(zone, next));
    };
    minus.addEventListener("click", () => change(-1));
    plus.addEventListener("click", () => change(1));
    row.append(name, minus, rate, plus, amount);
    taxList.append(row);
    rateLabels.push(rate);
    amounts.push(amount);
  });
  for (let i = 0; i < 3; i++) {
    const row = make("div", "", "finance-row");
    const label = make("span");
    const value = make("span", "", "finance-amount");
    row.append(label, value);
    expenseList.append(row);
    expenseValues.push(value);
  }
  body.append(
    make("h3", "税収（税率）"),
    taxList,
    make("h3", "維持費"),
    expenseList,
    netRow,
    make("p", "金額は 1 日あたりの見込み。", "finance-note"),
  );

  const render = (finance: Finance) => {
    const rows = financeRows(finance);
    summary.textContent = summaryText(finance);
    root.classList.toggle("deficit", isDeficit(finance));
    // test:wasm が読む
    root.dataset.net = String(finance.net);
    root.dataset.income = String(finance.income);
    root.dataset.deficit = String(isDeficit(finance));
    TAX_ZONES.forEach(({ zone }, i) => {
      const rate = rateLabels[i];
      if (rate !== undefined) {
        rate.textContent = formatRate(pending.shown(zone, finance.permille[i] ?? 0));
      }
      const amount = amounts[i];
      if (amount !== undefined) {
        amount.textContent = rows.taxes[i]?.value ?? "";
      }
      const row = taxList.querySelector<HTMLElement>(`[data-zone="${zone}"]`);
      if (row !== null) {
        row.dataset.permille = String(finance.permille[i] ?? 0);
        row.dataset.tax = String(finance.taxes[i] ?? 0);
      }
    });
    rows.expenses.forEach((r, i) => {
      const value = expenseValues[i];
      const label = value?.previousElementSibling;
      if (value !== undefined && label) {
        label.textContent = r.label;
        value.textContent = r.value;
      }
    });
    netLabel.textContent = rows.net.label;
    netValue.textContent = rows.net.value;
  };

  return {
    update(finance) {
      latest = finance;
      pending.receive(finance.permille);
      render(finance);
    },
    rejected() {
      pending.reject();
      if (latest !== null) {
        render(latest);
      }
    },
  };
}
