import { describe, expect, it } from "vitest";
import {
  applyInspectReply,
  describeInspect,
  formatDuration,
  INSPECT_LENGTH,
  parseInspect,
  sameQuery,
  setupInspector,
} from "./inspector.ts";
import type { InspectQuery } from "./protocol/messages.ts";

const pad = (head: number[]) => [...head, ...Array(INSPECT_LENGTH - head.length).fill(-1)];

describe("parseInspect", () => {
  it("空の配列や長さの違う配列は何もないものとして扱う", () => {
    expect(parseInspect([])).toBeNull();
    expect(parseInspect([1, 1, 1])).toBeNull();
  });

  it("住宅を読む", () => {
    expect(
      parseInspect([1, 1, 2, 1, 0, 8, 3, 9, 62, 80, 50, 55, 3960, 4, 1, 0, 2, 44, 5, 2]),
    ).toEqual({
      kind: "building",
      target: [1, 5, 2],
      zone: "Residential",
      level: 2,
      power: true,
      water: false,
      capacity: 8,
      occupied: 3,
      residents: {
        count: 9,
        happiness: 62,
        commute: 80,
        services: 50,
        parking: 44,
        wealth: 55,
        commuteSeconds: 3960,
      },
      carPlaces: { homeSpaces: 4, home: 1, shared: 0, none: 2 },
    });
  });

  it("住民のいない住宅は住民の様子が null になり、職のない住民の通勤は null になる", () => {
    const empty = parseInspect(pad([1, 1, 1, 1, 1, 2, 0, 0]));
    expect(empty).toMatchObject({ kind: "building", residents: null });
    const jobless = parseInspect([
      1, 1, 1, 1, 1, 2, 1, 2, 40, 100, 100, 0, -1, -1, -1, -1, -1, 100, 6, 1,
    ]);
    expect(jobless).toMatchObject({ residents: { commuteSeconds: null } });
  });

  it("職場と施設を読む", () => {
    expect(parseInspect(pad([1, 3, 4, 1, 1, 12, 5, -1]))).toMatchObject({
      zone: "Industrial",
      capacity: 12,
      occupied: 5,
      residents: null,
    });
    expect(parseInspect(pad([2, 2, 1000, 600]))).toEqual({
      kind: "facility",
      target: [2, -1, -1],
      service: "WaterPump",
      capacity: 1000,
      upkeepPerDay: 600,
      parking: null,
    });
  });

  it("知らない区画や施設は読まない", () => {
    expect(parseInspect(pad([1, 9, 1, 1, 1, 1, 1]))).toBeNull();
    expect(parseInspect(pad([2, 9, 1, 1]))).toBeNull();
    expect(parseInspect(pad([7]))).toBeNull();
  });
});

describe("describeInspect", () => {
  it("住宅の世帯、住民、幸福度とその要素、電気と水道、通勤を出す", () => {
    const view = parseInspect([1, 1, 2, 1, 0, 8, 3, 9, 62, 80, 50, 55, 3960, 4, 1, 0, 2, 44, 5, 2]);
    expect(view).not.toBeNull();
    const text = describeInspect(view as NonNullable<typeof view>);
    expect(text.title).toBe("住宅（高密度、段階 2）");
    expect(text.rows).toEqual([
      { label: "世帯", value: "3 / 8" },
      { label: "住民", value: "9 人" },
      { label: "幸福度", value: "62 / 100" },
      { label: "　通勤", value: "80" },
      { label: "　生活サービス", value: "50" },
      { label: "　駐車場", value: "44" },
      { label: "　暮らし向き", value: "55" },
      { label: "電気", value: "届いている" },
      { label: "水道", value: "届いていない" },
      { label: "通勤の所要時間", value: "約 66 分" },
      { label: "自宅の駐車場", value: "4 台" },
      { label: "　自宅の枠の世帯", value: "1 世帯" },
      { label: "　借りている世帯", value: "0 世帯" },
      { label: "　車を持てない世帯", value: "2 世帯" },
    ]);
  });

  it("共同の駐車場は、供給量でなく、台数と貸した枠、停まっている台数、外来の車を出す", () => {
    const view = parseInspect(pad([2, 3, 0, 96, 12, 5, 4, 2]));
    expect(view).toMatchObject({
      kind: "facility",
      service: "ParkingLot",
      capacity: 0,
      parking: { spaces: 12, rented: 5, parked: 4, visitors: 2 },
    });
    expect(describeInspect(view as NonNullable<typeof view>)).toEqual({
      title: "平面の駐車場",
      rows: [
        { label: "台数", value: "12 台" },
        { label: "貸した枠", value: "5 / 12" },
        { label: "停まっている", value: "4 台" },
        { label: "外来の車", value: "2 台" },
        { label: "維持費", value: "96 / 日" },
      ],
    });
    const garage = parseInspect(pad([2, 4, 0, 2880, 240, 0, 0, 0]));
    expect(describeInspect(garage as NonNullable<typeof garage>).title).toBe("駐車場ビル");
    // 供給量の行は、発電所と浄水場だけに出す
    const labels = describeInspect(garage as NonNullable<typeof garage>).rows.map((r) => r.label);
    expect(labels).not.toContain("供給量");
  });

  it("低密度の住宅（区画の番号 5）は高密度と分けて読み、住宅の欄を出す", () => {
    const view = parseInspect([
      1, 5, 2, 1, 1, 3, 1, 3, 62, 80, 50, 55, 3960, 3, 1, 0, 0, 100, 5, 2,
    ]);
    expect(view).toMatchObject({ kind: "building", zone: "ResidentialLow" });
    const text = describeInspect(view as NonNullable<typeof view>);
    expect(text.title).toBe("住宅（低密度、段階 2）");
    expect(text.rows.map((r) => r.label)).toContain("幸福度");
    expect(text.rows.map((r) => r.label)).toContain("自宅の駐車場");
    expect(text.rows[0]).toEqual({ label: "世帯", value: "1 / 3" });
  });

  it("住民のいない住宅は幸福度と通勤を出さない", () => {
    const view = parseInspect(pad([1, 1, 1, 0, 0, 2, 0, 0]));
    const text = describeInspect(view as NonNullable<typeof view>);
    expect(text.rows.map((r) => r.label)).toEqual(["世帯", "住民", "電気", "水道"]);
    expect(text.rows[1]?.value).toBe("0 人");
  });

  it("職場は種類と段階、働く人の数と上限、電気と水道を出す", () => {
    const view = parseInspect(pad([1, 2, 3, 1, 1, 1200, 1100]));
    const text = describeInspect(view as NonNullable<typeof view>);
    expect(text.title).toBe("商業の職場（段階 3）");
    expect(text.rows).toEqual([
      { label: "働く人", value: "1,100 / 1,200 人" },
      { label: "電気", value: "届いている" },
      { label: "水道", value: "届いている" },
    ]);
  });

  it("施設は種類、供給量、維持費を出す", () => {
    const view = parseInspect(pad([2, 1, 1200, 1200]));
    const text = describeInspect(view as NonNullable<typeof view>);
    expect(text).toEqual({
      title: "発電所",
      rows: [
        { label: "供給量", value: "1,200" },
        { label: "維持費", value: "1,200 / 日" },
      ],
    });
  });
});

describe("formatDuration", () => {
  it("1 分に満たなければ秒、それ以上は分で丸める", () => {
    expect(formatDuration(0)).toBe("0 秒");
    expect(formatDuration(59)).toBe("59 秒");
    expect(formatDuration(60)).toBe("約 1 分");
    expect(formatDuration(89)).toBe("約 1 分");
    expect(formatDuration(90)).toBe("約 2 分");
  });
});

describe("applyInspectReply", () => {
  const tile: InspectQuery = { kind: "tile", x: 3, y: 4 };
  const facility = [2, 1, 1200, 1200, ...Array(14).fill(-1), 8, 9];

  it("マスで押した最初の答えが届いたら、以後は識別子で引く", () => {
    const result = applyInspectReply(tile, tile, facility);
    expect(result).toMatchObject({
      stale: false,
      query: { kind: "target", target: [2, 8, 9] },
      view: { kind: "facility" },
    });
  });

  it("押し直したあとに届いた前の問い合わせの答えは捨てる", () => {
    const other: InspectQuery = { kind: "tile", x: 5, y: 4 };
    expect(applyInspectReply(other, tile, facility)).toEqual({ stale: true });
    const target: InspectQuery = { kind: "target", target: [2, 8, 9] };
    // 識別子で引いている最中に押し直した（マスで待っている）ときも、識別子の答えは捨てる
    expect(applyInspectReply(tile, target, facility)).toEqual({ stale: true });
    expect(applyInspectReply(null, tile, facility)).toEqual({ stale: true });
  });

  it("識別子で引いて空の答えが返ったら（押したものが消えたら）閉じる", () => {
    const target: InspectQuery = { kind: "target", target: [1, 5, 2] };
    expect(applyInspectReply(target, target, [])).toEqual({
      stale: false,
      query: null,
      view: null,
    });
  });

  it("同じ識別子の答えは次も識別子で引く", () => {
    const target: InspectQuery = { kind: "target", target: [2, 8, 9] };
    expect(applyInspectReply(target, target, facility)).toMatchObject({
      stale: false,
      query: target,
    });
  });
});

describe("sameQuery", () => {
  it("種類と値がそろったときだけ同じ", () => {
    expect(sameQuery({ kind: "tile", x: 1, y: 2 }, { kind: "tile", x: 1, y: 2 })).toBe(true);
    expect(sameQuery({ kind: "tile", x: 1, y: 2 }, { kind: "tile", x: 1, y: 3 })).toBe(false);
    expect(
      sameQuery({ kind: "target", target: [1, 2, 3] }, { kind: "target", target: [1, 2, 4] }),
    ).toBe(false);
    expect(sameQuery({ kind: "tile", x: 1, y: 2 }, { kind: "target", target: [1, 2, 3] })).toBe(
      false,
    );
  });
});

describe("setupInspector", () => {
  const panel = () => {
    const el = {
      hidden: true,
      dataset: {} as Record<string, string>,
      querySelector: (selector: string) => parts[selector] ?? null,
    };
    return el;
  };
  type FakeEl = {
    textContent: string;
    writes: number;
    children: FakeEl[];
    addEventListener?: () => void;
    replaceChildren?: (...c: FakeEl[]) => void;
  };
  const parts: Record<string, FakeEl> = {};

  it("値が変わった行だけ書き換え、変わらなければ要素にも文にも触れない", () => {
    const made: FakeEl[] = [];
    const make = (): FakeEl => {
      let text = "";
      const el: FakeEl = {
        get textContent() {
          return text;
        },
        set textContent(v: string) {
          text = v;
          el.writes++;
        },
        writes: 0,
        children: [],
      };
      made.push(el);
      return el;
    };
    const title = make();
    const list = make();
    list.replaceChildren = (...c) => {
      list.children = c;
    };
    const close = make();
    close.addEventListener = () => {};
    parts["[data-inspector-title]"] = title;
    parts["[data-inspector-rows]"] = list;
    parts["[data-inspector-close]"] = close;
    (globalThis as { document?: unknown }).document = { createElement: make };
    const inspector = setupInspector(panel() as unknown as HTMLElement, () => {});
    const at = (n: number) =>
      parseInspect([1, 3, 1, 1, 1, 10, n, ...Array(11).fill(-1), 1, 1]) as NonNullable<
        ReturnType<typeof parseInspect>
      >;
    inspector.show(at(1));
    const rows = [...list.children];
    const writes = made.map((m) => m.writes);
    inspector.show(at(1));
    expect(made.map((m) => m.writes)).toEqual(writes);
    expect(list.children.every((c, i) => c === rows[i])).toBe(true);
    inspector.show(at(2));
    expect(list.children.every((c, i) => c === rows[i])).toBe(true);
    expect(rows[1]?.textContent).toBe("2 / 10 人");
  });
});
