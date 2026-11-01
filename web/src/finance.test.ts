import { describe, expect, it } from "vitest";
import {
  deficitNote,
  financeRows,
  formatMoney,
  formatRate,
  formatSigned,
  isDeficit,
  parseFinance,
  steppedRate,
  summaryText,
  taxCommand,
} from "./finance.ts";
import { FINANCE_LENGTH, isWorkerToMain } from "./protocol/messages.ts";

// 税率の上限、税率 4 つ、税収 4 つ、道路、発電所、浄水場
const SAMPLE = [300, 100, 120, 100, 0, 1_000, 400, 300, 0, 180, 1_200, 0];

describe("parseFinance", () => {
  it("長さの違う配列は読まない", () => {
    expect(parseFinance([])).toBeNull();
    expect(parseFinance(SAMPLE.slice(1))).toBeNull();
  });

  it("収入、支出、差し引きを足し上げる", () => {
    const f = parseFinance(SAMPLE);
    expect(f).toEqual({
      maxPermille: 300,
      permille: [100, 120, 100, 0],
      taxes: [1_000, 400, 300, 0],
      roadUpkeep: 180,
      powerUpkeep: 1_200,
      waterUpkeep: 0,
      income: 1_700,
      expenses: 1_380,
      net: 320,
    });
  });

  it("支出が収入を上回れば赤字", () => {
    const deficit = parseFinance([300, 100, 100, 100, 100, 0, 0, 0, 0, 180, 1_200, 0]);
    expect(deficit?.net).toBe(-1_380);
    expect(deficit !== null && isDeficit(deficit)).toBe(true);
    const f = parseFinance(SAMPLE);
    expect(f !== null && isDeficit(f)).toBe(false);
  });
});

describe("文言と数字の整形", () => {
  it("金額に桁区切りと符号をつける", () => {
    expect(formatMoney(1_234_567)).toBe("1,234,567");
    expect(formatSigned(1_380)).toBe("+1,380");
    expect(formatSigned(-1_380)).toBe("-1,380");
    expect(formatSigned(0)).toBe("0");
  });

  it("税率を % で出す", () => {
    expect(formatRate(100)).toBe("10%");
    expect(formatRate(0)).toBe("0%");
    expect(formatRate(300)).toBe("30%");
    expect(formatRate(125)).toBe("12.5%");
  });

  it("見出しと所持金の横の文は、赤字のときだけそれを示す", () => {
    expect(summaryText(null)).toBe("収支");
    const surplus = parseFinance(SAMPLE);
    expect(summaryText(surplus)).toBe("収支 +320 / 日");
    expect(deficitNote(surplus)).toBe("");
    expect(deficitNote(null)).toBe("");
    const deficit = parseFinance([300, 100, 100, 100, 100, 0, 0, 0, 0, 180, 1_200, 0]);
    expect(summaryText(deficit)).toBe("収支 -1,380 / 日（赤字）");
    expect(deficitNote(deficit)).toBe("赤字 -1,380 / 日");
  });

  it("税収と維持費と差し引きの行を作る", () => {
    const f = parseFinance(SAMPLE);
    expect(f).not.toBeNull();
    if (f === null) {
      return;
    }
    const rows = financeRows(f);
    expect(rows.taxes).toEqual([
      { label: "住宅", value: "+1,000" },
      { label: "商業", value: "+400" },
      { label: "工業", value: "+300" },
      { label: "オフィス", value: "0" },
    ]);
    expect(rows.expenses).toEqual([
      { label: "道路の維持費", value: "-180" },
      { label: "発電所の維持費", value: "-1,200" },
      { label: "浄水場の維持費", value: "0" },
    ]);
    expect(rows.net).toEqual({ label: "差し引き", value: "+320" });
  });
});

describe("税率の操作", () => {
  it("1% ずつ動かし、0 と上限で止める", () => {
    expect(steppedRate(100, 1, 300)).toBe(110);
    expect(steppedRate(100, -1, 300)).toBe(90);
    expect(steppedRate(0, -1, 300)).toBe(0);
    expect(steppedRate(300, 1, 300)).toBe(300);
    expect(steppedRate(295, 1, 300)).toBe(300);
    expect(steppedRate(5, -1, 300)).toBe(0);
  });

  it("SetTaxRate のコマンドを作る", () => {
    expect(taxCommand("Commercial", 130)).toEqual({
      kind: "SetTaxRate",
      zone: "Commercial",
      permille: 130,
    });
  });
});

describe("finance のメッセージ", () => {
  it("数の個数が合うときだけ受け取る", () => {
    expect(isWorkerToMain({ kind: "finance", values: SAMPLE })).toBe(true);
    expect(SAMPLE).toHaveLength(FINANCE_LENGTH);
    expect(isWorkerToMain({ kind: "finance", values: SAMPLE.slice(1) })).toBe(false);
    expect(isWorkerToMain({ kind: "finance", values: SAMPLE.map(String) })).toBe(false);
  });
});
