import { describe, expect, it } from "vitest";
import {
  balanceNote,
  deficitNote,
  financeRows,
  formatFee,
  formatMoney,
  formatRate,
  formatSigned,
  isDeficit,
  PendingFee,
  PendingRates,
  parkingFeeCommand,
  parseFinance,
  steppedFee,
  steppedRate,
  summaryText,
  taxCommand,
} from "./finance.ts";
import { FINANCE_LENGTH, isWorkerToMain } from "./protocol/messages.ts";

// 税率の上限、税率 4 つ、税収 4 つ、道路、発電所、浄水場、駐車場の維持費、駐車場の料金、料金の上限、料金の収入、鉄道の維持費
const SAMPLE = [300, 100, 120, 100, 0, 1_000, 400, 300, 0, 180, 1_200, 0, 96, 10, 50, 120, 24];
const DEFICIT = [300, 100, 100, 100, 100, 0, 0, 0, 0, 180, 1_200, 0, 96, 10, 50, 0, 0];

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
      parkingUpkeep: 96,
      railUpkeep: 24,
      parkingFee: 10,
      maxParkingFee: 50,
      parkingFees: 120,
      income: 1_820,
      expenses: 1_500,
      net: 320,
    });
  });

  it("支出が収入を上回れば赤字", () => {
    const deficit = parseFinance(DEFICIT);
    expect(deficit?.net).toBe(-1_476);
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
    const deficit = parseFinance(DEFICIT);
    expect(summaryText(deficit)).toBe("収支 -1,476 / 日（赤字）");
    expect(deficitNote(deficit)).toBe("赤字 -1,476 / 日");
    // 上の帯には、黒字でも 1 日あたりの収支を出す
    expect(balanceNote(surplus)).toBe("+320 / 日");
    expect(balanceNote(deficit)).toBe("赤字 -1,476 / 日");
    expect(balanceNote(null)).toBe("");
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
      { label: "駐車場の維持費", value: "-96" },
      { label: "鉄道の維持費", value: "-24" },
    ]);
    expect(rows.parkingFees).toEqual({ label: "駐車場の料金", value: "+120" });
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

  it("駐車場の料金を 1 ずつ動かし、0 と上限で止める", () => {
    expect(steppedFee(10, 1, 50)).toBe(11);
    expect(steppedFee(10, -1, 50)).toBe(9);
    expect(steppedFee(0, -1, 50)).toBe(0);
    expect(steppedFee(50, 1, 50)).toBe(50);
    expect(formatFee(10)).toBe("10 / 台");
    expect(parkingFeeCommand(12)).toEqual({ kind: "SetParkingFee", fee: 12 });
  });

  it("押した先の料金を、反映されるまで覚える", () => {
    const p = new PendingFee();
    expect(p.press(1, 10, 50)).toBe(11);
    p.receive(10);
    expect(p.shown(10)).toBe(11);
    expect(p.press(1, 10, 50)).toBe(12);
    p.receive(12);
    expect(p.shown(12)).toBe(12);
    expect(p.press(1, 50, 50)).toBeNull();
    expect(p.press(-1, 10, 50)).toBe(9);
    p.reject();
    expect(p.shown(10)).toBe(10);
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

describe("PendingRates", () => {
  const max = 300;

  it("押した先を表示し、続けて押すと 2 刻み先を求める", () => {
    const p = new PendingRates();
    expect(p.press("Residential", 1, 100, max)).toBe(110);
    // 反映前の finance が届いても、表示は押した先のまま
    p.receive([100, 100, 100, 100]);
    expect(p.shown("Residential", 100)).toBe(110);
    expect(p.press("Residential", 1, 100, max)).toBe(120);
    // 110 までを反映した finance では、まだ 120 を待つ
    p.receive([110, 100, 100, 100]);
    expect(p.shown("Residential", 110)).toBe(120);
    // 120 が反映されたら忘れる
    p.receive([120, 100, 100, 100]);
    expect(p.shown("Residential", 120)).toBe(120);
    expect(p.press("Residential", -1, 120, max)).toBe(110);
  });

  it("区画ごとに別々に覚える", () => {
    const p = new PendingRates();
    p.press("Commercial", -1, 100, max);
    expect(p.shown("Residential", 100)).toBe(100);
    expect(p.shown("Commercial", 100)).toBe(90);
    p.receive([100, 90, 100, 100]);
    expect(p.shown("Commercial", 90)).toBe(90);
  });

  it("端では動かさず、何も覚えない", () => {
    const p = new PendingRates();
    expect(p.press("Office", -1, 0, max)).toBeNull();
    expect(p.press("Office", 1, 300, max)).toBeNull();
    expect(p.shown("Office", 0)).toBe(0);
  });

  it("断られたら押した先を捨てて中核の値に戻す", () => {
    const p = new PendingRates();
    p.press("Industrial", 1, 100, max);
    p.press("Office", 1, 100, max);
    p.reject();
    expect(p.shown("Industrial", 100)).toBe(100);
    expect(p.shown("Office", 100)).toBe(100);
  });
});
