// 「見る」の道具で押した建物や施設の様子を出す小さなパネル。DOM に触るのは setupInspector だけで、
// 数の読み方と文言は Vitest で確かめられる。

import {
  type InspectQuery,
  type InspectTarget,
  isParking,
  SERVICE_NAMES,
  type ServiceName,
  type ZoneName,
} from "./protocol/messages.ts";

/** chizu-wasm の inspect が返す数の個数。 */
export const INSPECT_LENGTH = 20;

/** 幸福度とその要素の満点。chizu-core の HAPPINESS_MAX と同じ。 */
export const HAPPINESS_MAX = 100;

/** 住宅の住民の様子。 */
export type ResidentsInfo = {
  count: number;
  happiness: number;
  commute: number;
  services: number;
  parking: number;
  wealth: number;
  /** 通勤の所要時間の平均（時計の秒）。職場のある住民がいなければ null。 */
  commuteSeconds: number | null;
};

/** 住宅の車を置く枠の様子。 */
export type CarPlacesInfo = {
  /** 自宅の枠の数。建物の駐車場の台数である。 */
  homeSpaces: number;
  /** 自宅の枠を持つ世帯の数。 */
  home: number;
  /** 共同の駐車場の枠を借りている世帯の数。 */
  shared: number;
  /** 枠を持たない世帯の数。車を持てない。 */
  none: number;
};

export type BuildingView = {
  kind: "building";
  /** 押したものの識別子。以後はマスでなくこれで引く。 */
  target: InspectTarget;
  /** 区画の種類。低密度の住宅は ResidentialLow で、高密度の住宅（Residential）と分かれる。 */
  zone: Exclude<ZoneName, "Clear">;
  level: number;
  power: boolean;
  water: boolean;
  /** 住宅なら世帯の上限、職場なら職場の上限。 */
  capacity: number;
  /** 住宅なら住んでいる世帯の数、職場なら働いている人の数。 */
  occupied: number;
  /** 住宅のときだけ。住民がいなければ null。 */
  residents: ResidentsInfo | null;
  /** 住宅のときだけ。 */
  carPlaces: CarPlacesInfo | null;
};

/** 共同の駐車場の埋まり具合。 */
export type ParkingView = {
  /** 台数。 */
  spaces: number;
  /** 世帯に貸した枠の数。 */
  rented: number;
  /** 停まっている台数。借りた世帯の車と外来の車の和。 */
  parked: number;
  /** 外来の車が停まっているか、向かっていて押さえている台数。 */
  visitors: number;
};

export type FacilityView = {
  kind: "facility";
  target: InspectTarget;
  service: ServiceName;
  /** 電気か水道の供給量。共同の駐車場は 0。 */
  capacity: number;
  upkeepPerDay: number;
  /** 共同の駐車場のときだけ。 */
  parking: ParkingView | null;
};

export type InspectView = BuildingView | FacilityView;

const ZONES: Record<number, BuildingView["zone"]> = {
  1: "Residential",
  2: "Commercial",
  3: "Industrial",
  4: "Office",
  5: "ResidentialLow",
};

/** 住宅（高密度でも低密度でも）か。 */
const isHome = (zone: BuildingView["zone"]) => zone === "Residential" || zone === "ResidentialLow";
const SERVICES: Record<number, ServiceName> = Object.fromEntries(
  SERVICE_NAMES.map((name, i) => [i + 1, name]),
);

/** chizu-wasm の inspect の数を読む。何もなければ（空の配列なら）null。読めない形でも null。 */
export function parseInspect(numbers: ArrayLike<number>): InspectView | null {
  if (numbers.length !== INSPECT_LENGTH) {
    return null;
  }
  const at = (i: number) => numbers[i] ?? -1;
  const target: InspectTarget = [at(0), at(INSPECT_LENGTH - 2), at(INSPECT_LENGTH - 1)];
  if (at(0) === 2) {
    const service = SERVICES[at(1)];
    if (service === undefined) {
      return null;
    }
    return {
      kind: "facility",
      target,
      service,
      capacity: at(2),
      upkeepPerDay: at(3),
      parking: at(4) >= 0 ? { spaces: at(4), rented: at(5), parked: at(6), visitors: at(7) } : null,
    };
  }
  const zone = ZONES[at(1)];
  if (at(0) !== 1 || zone === undefined) {
    return null;
  }
  const residents = isHome(zone) && at(7) > 0;
  return {
    kind: "building",
    target,
    zone,
    level: at(2),
    power: at(3) === 1,
    water: at(4) === 1,
    capacity: at(5),
    occupied: at(6),
    residents: residents
      ? {
          count: at(7),
          happiness: at(8),
          commute: at(9),
          services: at(10),
          parking: at(17),
          wealth: at(11),
          commuteSeconds: at(12) >= 0 ? at(12) : null,
        }
      : null,
    carPlaces:
      isHome(zone) && at(13) >= 0
        ? { homeSpaces: at(13), home: at(14), shared: at(15), none: at(16) }
        : null,
  };
}

const ZONE_LABEL: Record<BuildingView["zone"], string> = {
  Residential: "住宅（高密度）",
  ResidentialLow: "住宅（低密度）",
  Commercial: "商業",
  Industrial: "工業",
  Office: "オフィス",
};
const SERVICE_LABEL: Record<ServiceName, string> = {
  PowerPlant: "発電所",
  WaterPump: "浄水場",
  ParkingLot: "平面の駐車場",
  ParkingGarage: "駐車場ビル",
};

const n = (v: number) => v.toLocaleString("ja-JP");

/** 通勤の所要時間。1 分に満たなければ秒、そのあとは分で丸める。 */
export function formatDuration(seconds: number): string {
  return seconds < 60 ? `${n(seconds)} 秒` : `約 ${n(Math.round(seconds / 60))} 分`;
}

/** パネルの 1 行。 */
export type InspectRow = { label: string; value: string };

export type InspectText = { title: string; rows: InspectRow[] };

const reach = (ok: boolean) => (ok ? "届いている" : "届いていない");

/** パネルに出す題と行。 */
export function describeInspect(view: InspectView): InspectText {
  if (view.kind === "facility") {
    const upkeep = { label: "維持費", value: `${n(view.upkeepPerDay)} / 日` };
    if (isParking(view.service) && view.parking !== null) {
      const p = view.parking;
      // 供給量は電気と水道の施設の欄で、駐車場には出さない
      return {
        title: SERVICE_LABEL[view.service],
        rows: [
          { label: "台数", value: `${n(p.spaces)} 台` },
          { label: "貸した枠", value: `${n(p.rented)} / ${n(p.spaces)}` },
          { label: "停まっている", value: `${n(p.parked)} 台` },
          { label: "外来の車", value: `${n(p.visitors)} 台` },
          upkeep,
        ],
      };
    }
    return {
      title: SERVICE_LABEL[view.service],
      rows: [{ label: "供給量", value: n(view.capacity) }, upkeep],
    };
  }
  const supply = [
    { label: "電気", value: reach(view.power) },
    { label: "水道", value: reach(view.water) },
  ];
  if (!isHome(view.zone)) {
    return {
      title: `${ZONE_LABEL[view.zone]}の職場（段階 ${view.level}）`,
      rows: [{ label: "働く人", value: `${n(view.occupied)} / ${n(view.capacity)} 人` }, ...supply],
    };
  }
  const rows: InspectRow[] = [
    { label: "世帯", value: `${n(view.occupied)} / ${n(view.capacity)}` },
    { label: "住民", value: `${n(view.residents?.count ?? 0)} 人` },
  ];
  const r = view.residents;
  if (r !== null) {
    rows.push(
      { label: "幸福度", value: `${r.happiness} / ${HAPPINESS_MAX}` },
      { label: "　通勤", value: String(r.commute) },
      { label: "　生活サービス", value: String(r.services) },
      { label: "　駐車場", value: String(r.parking) },
      { label: "　暮らし向き", value: String(r.wealth) },
    );
  }
  rows.push(...supply);
  if (r !== null && r.commuteSeconds !== null) {
    rows.push({ label: "通勤の所要時間", value: formatDuration(r.commuteSeconds) });
  }
  const c = view.carPlaces;
  if (c !== null) {
    rows.push(
      { label: "自宅の駐車場", value: `${n(c.homeSpaces)} 台` },
      { label: "　自宅の枠の世帯", value: `${n(c.home)} 世帯` },
      { label: "　借りている世帯", value: `${n(c.shared)} 世帯` },
      { label: "　車を持てない世帯", value: `${n(c.none)} 世帯` },
    );
  }
  const density = view.zone === "ResidentialLow" ? "低密度" : "高密度";
  return { title: `住宅（${density}、段階 ${view.level}）`, rows };
}

/** 2 つの問い合わせが同じか。 */
export function sameQuery(a: InspectQuery, b: InspectQuery): boolean {
  if (a.kind === "tile" && b.kind === "tile") {
    return a.x === b.x && a.y === b.y;
  }
  if (a.kind === "target" && b.kind === "target") {
    return a.target.every((v, i) => v === b.target[i]);
  }
  return false;
}

/** 答えを受けた結果。stale なら捨てる。そうでなければ、次の問い合わせ（null なら閉じる）と出す中身を返す。 */
export type InspectOutcome =
  | { stale: true }
  | { stale: false; query: InspectQuery | null; view: InspectView | null };

/**
 * 答えが、いま出している（または待っている）問い合わせへのものかを見分けて受ける。
 * 押し直したあとに届いた前の答えは捨てる。マスで押した最初の答えが届いたら、以後は識別子で引く。
 * 答えが空（押したものが消えた）なら閉じる。
 */
export function applyInspectReply(
  current: InspectQuery | null,
  replied: InspectQuery,
  info: ArrayLike<number>,
): InspectOutcome {
  if (current === null || !sameQuery(current, replied)) {
    return { stale: true };
  }
  const view = parseInspect(info);
  if (view === null) {
    return { stale: false, query: null, view: null };
  }
  return { stale: false, query: { kind: "target", target: view.target }, view };
}

export type Inspector = {
  /** 届いた様子で書き直す。null ならパネルを閉じる。 */
  show: (view: InspectView | null) => void;
  close: () => void;
};

/**
 * パネルの中身を作る。閉じるボタンを押したら onClose を呼ぶ。
 * 行の要素は題が変わるか行の並びが変わるときだけ作り直し、そのほかは値が変わった文だけ書き換える。
 * 読み上げ（aria-live）は題に付け、パネルを開いたときと題が変わったときだけ読まれるようにする。
 */
export function setupInspector(panel: HTMLElement, onClose: () => void): Inspector {
  const title = panel.querySelector<HTMLElement>("[data-inspector-title]");
  const list = panel.querySelector<HTMLElement>("[data-inspector-rows]");
  const closeButton = panel.querySelector<HTMLButtonElement>("[data-inspector-close]");
  if (title === null || list === null || closeButton === null) {
    throw new Error("建物のパネルの要素が足りない");
  }
  let labels: string[] = [];
  let values: HTMLElement[] = [];
  const close = () => {
    panel.hidden = true;
    title.textContent = "";
    list.replaceChildren();
    labels = [];
    values = [];
  };
  closeButton.addEventListener("click", () => {
    close();
    onClose();
  });
  return {
    close,
    show(view) {
      if (view === null) {
        close();
        return;
      }
      const text = describeInspect(view);
      if (title.textContent !== text.title) {
        title.textContent = text.title;
      }
      const next = text.rows.map((r) => r.label);
      if (next.length !== labels.length || next.some((l, i) => l !== labels[i])) {
        values = [];
        list.replaceChildren(
          ...text.rows.flatMap(({ label }) => {
            const dt = document.createElement("dt");
            dt.textContent = label;
            const dd = document.createElement("dd");
            values.push(dd);
            return [dt, dd];
          }),
        );
        labels = next;
      }
      text.rows.forEach(({ value }, i) => {
        const dd = values[i];
        if (dd !== undefined && dd.textContent !== value) {
          dd.textContent = value;
        }
      });
      panel.hidden = false;
      // test:wasm が読む
      panel.dataset.title = text.title;
    },
  };
}
