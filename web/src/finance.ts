// 収支のパネル。区画の種類ごとの税収、共同の駐車場の料金の収入、鉄道の運賃の収入、道路と施設と鉄道の維持費、
// 1 日あたりの差し引きを出し、税率と駐車場の料金と運賃を上げ下げできる。
// DOM に触るのは setupFinance だけで、数の読み方と文言は Vitest で確かめられる。

import { FINANCE_LENGTH, type ZoneName } from "./protocol/messages.ts";

/** 税率は区画の種類ごとに決め、密度では分けない。 */
export type TaxZone = Exclude<ZoneName, "Clear" | "ResidentialLow">;

/** 税率をかける区画の種類。chizu-wasm の finance の並びと同じ。 */
export const TAX_ZONES: { zone: TaxZone; label: string }[] = [
  { zone: "Residential", label: "住宅" },
  { zone: "Commercial", label: "商業" },
  { zone: "Industrial", label: "工業" },
  { zone: "Office", label: "オフィス" },
];

/** ボタン 1 回で動かす税率（‰）。1% である。 */
export const RATE_STEP_PERMILLE = 10;

/** ボタン 1 回で動かす駐車場の料金。 */
export const FEE_STEP = 1;

/** 維持費の行の数。道路、発電所、浄水場、駐車場、鉄道。 */
const EXPENSE_ROWS = 5;

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
  /** 共同の駐車場の維持費。 */
  parkingUpkeep: number;
  /** 鉄道の維持費。駅と、駅の外の線路の分である。 */
  railUpkeep: number;
  /** 共同の駐車場の料金（借りた枠 1 つあたりの 1 日の額）。 */
  parkingFee: number;
  /** 料金の上限。 */
  maxParkingFee: number;
  /** 料金の収入。 */
  parkingFees: number;
  /** 鉄道の運賃（1 回の乗車の額）。 */
  fare: number;
  /** 運賃の上限。 */
  maxFare: number;
  /** 前の日の運賃の収入。 */
  fares: number;
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
  const income = taxes.reduce((a, b) => a + b, 0) + at(15) + at(19);
  const expenses = at(9) + at(10) + at(11) + at(12) + at(16);
  return {
    maxPermille: at(0),
    permille: [at(1), at(2), at(3), at(4)],
    taxes,
    roadUpkeep: at(9),
    powerUpkeep: at(10),
    waterUpkeep: at(11),
    parkingUpkeep: at(12),
    railUpkeep: at(16),
    parkingFee: at(13),
    maxParkingFee: at(14),
    parkingFees: at(15),
    fare: at(17),
    maxFare: at(18),
    fares: at(19),
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

/** 上の帯の所持金の横に出す、1 日あたりの収支。赤字のときは赤字と分かる文にする。 */
export function balanceNote(finance: Finance | null): string {
  if (finance === null) {
    return "";
  }
  return isDeficit(finance) ? deficitNote(finance) : `${formatSigned(finance.net)} / 日`;
}

/** 税率を 1 刻み動かした値。0 と上限のあいだに収める。 */
export function steppedRate(current: number, direction: 1 | -1, max: number): number {
  return Math.max(0, Math.min(max, current + direction * RATE_STEP_PERMILLE));
}

/** 駐車場の料金を 1 刻み動かした値。0 と上限のあいだに収める。 */
export function steppedFee(current: number, direction: 1 | -1, max: number): number {
  return Math.max(0, Math.min(max, current + direction * FEE_STEP));
}

/** 料金を出す文。 */
export function formatFee(fee: number): string {
  return `${fee} / 台`;
}

export function parkingFeeCommand(fee: number): { kind: "SetParkingFee"; fee: number } {
  return { kind: "SetParkingFee", fee };
}

/** 運賃を出す文。 */
export function formatFare(fare: number): string {
  return `${fare} / 回`;
}

export function fareCommand(fare: number): { kind: "SetFare"; fare: number } {
  return { kind: "SetFare", fare };
}

export function taxCommand(
  zone: TaxZone,
  permille: number,
): { kind: "SetTaxRate"; zone: TaxZone; permille: number } {
  return { kind: "SetTaxRate", zone, permille };
}

export type FinanceRow = { label: string; value: string };

/** 税収、駐車場の料金の収入、運賃の収入、維持費、差し引きの行。税率と料金と運賃の欄は含めない。 */
export function financeRows(finance: Finance): {
  taxes: FinanceRow[];
  parkingFees: FinanceRow;
  fares: FinanceRow;
  expenses: FinanceRow[];
  net: FinanceRow;
} {
  return {
    taxes: TAX_ZONES.map(({ label }, i) => ({
      label,
      value: formatSigned(finance.taxes[i] ?? 0),
    })),
    parkingFees: { label: "駐車場の料金", value: formatSigned(finance.parkingFees) },
    fares: { label: "鉄道の運賃", value: formatSigned(finance.fares) },
    expenses: [
      { label: "道路の維持費", value: formatSigned(-finance.roadUpkeep) },
      { label: "発電所の維持費", value: formatSigned(-finance.powerUpkeep) },
      { label: "浄水場の維持費", value: formatSigned(-finance.waterUpkeep) },
      { label: "駐車場の維持費", value: formatSigned(-finance.parkingUpkeep) },
      { label: "鉄道の維持費", value: formatSigned(-finance.railUpkeep) },
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

/** 押した先の駐車場の料金か運賃を、中核が反映するまで覚えておく。考え方は PendingRates と同じ。 */
export class PendingFee {
  private wanted: number | null = null;

  shown(received: number): number {
    return this.wanted ?? received;
  }

  /** 1 刻み動かした先を覚えて返す。動かせない（端）ときは null。 */
  press(direction: 1 | -1, received: number, max: number): number | null {
    const current = this.shown(received);
    const next = steppedFee(current, direction, max);
    if (next === current) {
      return null;
    }
    this.wanted = next;
    return next;
  }

  receive(fee: number): void {
    if (this.wanted === fee) {
      this.wanted = null;
    }
  }

  reject(): void {
    this.wanted = null;
  }
}

export type FinanceCommand =
  | ReturnType<typeof taxCommand>
  | ReturnType<typeof parkingFeeCommand>
  | ReturnType<typeof fareCommand>;

export type FinancePanel = {
  update(finance: Finance): void;
  /** SetTaxRate、SetParkingFee、SetFare が断られたときに呼ぶ。 */
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
 * 開け閉めするパネルを組む。税率のボタンは SetTaxRate を、駐車場の料金のボタンは SetParkingFee を、運賃のボタンは
 * SetFare を send に渡す。
 * 返事が届くまでに続けて押しても 1 つ飛ばさないよう、押した先の値を覚えておき、次の finance が届いたら捨てる。
 */
export function setupFinance(
  root: HTMLElement,
  send: (command: FinanceCommand) => void,
): FinancePanel {
  const summary = root.querySelector<HTMLElement>("[data-finance-summary]");
  const body = root.querySelector<HTMLElement>("[data-finance-body]");
  if (summary === null || body === null) {
    throw new Error("収支のパネルの要素が足りない");
  }
  let latest: Finance | null = null;
  const pending = new PendingRates();
  const pendingFee = new PendingFee();
  const pendingFare = new PendingFee();

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
  // 欄の高さを抑えるため、料金は税率と同じ並びの最後の行に置く
  const feeRow = make("div", "", "finance-row");
  feeRow.dataset.parkingFee = "";
  feeRow.title =
    "共同の駐車場の、借りた枠 1 台あたりの 1 日の料金。世帯は蓄えに応じて 10〜49 まで払い、超えると借りない。借りている世帯も 1 日 1 回の見直しで返す。料金が高いほど、借りている世帯の幸福度が下がる（0 なら下がらない）。";
  const feeMinus = make("button", "−");
  const feePlus = make("button", "＋");
  feeMinus.type = "button";
  feePlus.type = "button";
  feeMinus.setAttribute("aria-label", "駐車場の料金を下げる");
  feePlus.setAttribute("aria-label", "駐車場の料金を上げる");
  const feeLabel = make("span", "", "finance-rate");
  const feeAmount = make("span", "", "finance-amount");
  const changeFee = (direction: 1 | -1) => {
    if (latest === null) {
      return;
    }
    const next = pendingFee.press(direction, latest.parkingFee, latest.maxParkingFee);
    if (next === null) {
      return;
    }
    feeLabel.textContent = formatFee(next);
    send(parkingFeeCommand(next));
  };
  feeMinus.addEventListener("click", () => changeFee(-1));
  feePlus.addEventListener("click", () => changeFee(1));
  feeRow.append(make("span", "駐車場"), feeMinus, feeLabel, feePlus, feeAmount);
  taxList.append(feeRow);
  // 運賃も駐車場の料金と同じ形の行にする
  const fareRow = make("div", "", "finance-row");
  fareRow.dataset.fare = "";
  fareRow.title =
    "鉄道の 1 回の乗車の運賃。乗ったときに所持金に入る。住民は運賃 1 につき 5 秒の遠回りと同じに見て、歩きや車と比べて鉄道を選ぶので、高いほど乗る人が減る。";
  const fareMinus = make("button", "−");
  const farePlus = make("button", "＋");
  fareMinus.type = "button";
  farePlus.type = "button";
  fareMinus.setAttribute("aria-label", "運賃を下げる");
  farePlus.setAttribute("aria-label", "運賃を上げる");
  const fareLabel = make("span", "", "finance-rate");
  const fareAmount = make("span", "", "finance-amount");
  const changeFare = (direction: 1 | -1) => {
    if (latest === null) {
      return;
    }
    const next = pendingFare.press(direction, latest.fare, latest.maxFare);
    if (next === null) {
      return;
    }
    fareLabel.textContent = formatFare(next);
    send(fareCommand(next));
  };
  fareMinus.addEventListener("click", () => changeFare(-1));
  farePlus.addEventListener("click", () => changeFare(1));
  fareRow.append(make("span", "鉄道"), fareMinus, fareLabel, farePlus, fareAmount);
  taxList.append(fareRow);

  for (let i = 0; i < EXPENSE_ROWS; i++) {
    const row = make("div", "", "finance-row");
    const label = make("span");
    const value = make("span", "", "finance-amount");
    row.append(label, value);
    expenseList.append(row);
    expenseValues.push(value);
  }
  body.append(
    make("h3", "収入（税率と駐車場の料金と運賃）"),
    taxList,
    make("h3", "維持費"),
    expenseList,
    netRow,
    make(
      "p",
      "金額は 1 日あたりの見込み。駐車場の料金は 1 台 1 日の額。運賃は 1 回の乗車の額で、収入は前の日の分。",
      "finance-note",
    ),
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
    feeLabel.textContent = formatFee(pendingFee.shown(finance.parkingFee));
    feeAmount.textContent = rows.parkingFees.value;
    feeRow.dataset.parkingFee = String(finance.parkingFee);
    feeRow.dataset.income = String(finance.parkingFees);
    fareLabel.textContent = formatFare(pendingFare.shown(finance.fare));
    fareAmount.textContent = rows.fares.value;
    fareRow.dataset.fare = String(finance.fare);
    fareRow.dataset.income = String(finance.fares);
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
      pendingFee.receive(finance.parkingFee);
      pendingFare.receive(finance.fare);
      render(finance);
    },
    rejected() {
      pending.reject();
      pendingFee.reject();
      pendingFare.reject();
      if (latest !== null) {
        render(latest);
      }
    },
  };
}
