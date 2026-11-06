// メインスレッドと Worker のあいだでやりとりするメッセージ。

import { validCityStats } from "./city-stats.ts";

export type RoadKindName = "Small" | "Medium" | "Large";
/** 施設の種類。ServiceKind::ALL の順に駅を続けた並びで、service_table の並びと同じである。 */
export type ServiceName = "PowerPlant" | "WaterPump" | "ParkingLot" | "ParkingGarage" | "Station";
/** 施設の種類の順（ServiceKind の値は 1 から数える）。 */
export const SERVICE_NAMES: readonly ServiceName[] = [
  "PowerPlant",
  "WaterPump",
  "ParkingLot",
  "ParkingGarage",
  "Station",
];
/** 共同の駐車場か。 */
export const isParking = (service: ServiceName): boolean =>
  service === "ParkingLot" || service === "ParkingGarage";
/** 施設の種類の値（ServiceKind の値）。 */
export const serviceCode = (service: ServiceName): number => SERVICE_NAMES.indexOf(service) + 1;
/** chizu-wasm の finance の数の個数。 */
export const FINANCE_LENGTH = 17;

/** chizu-wasm の carless の、固定の並びの個数。そのあとに世帯の多い区切りの `[x, y, 世帯の数]` が続く。 */
export const CARLESS_FIXED = 6;

/** chizu-wasm の overview の数の個数。 */
export const OVERVIEW_LENGTH = 22;
/** 区画の種類。Residential は高密度の住宅、Clear は区画を消す。 */
export type ZoneName =
  | "Residential"
  | "ResidentialLow"
  | "Commercial"
  | "Industrial"
  | "Office"
  | "Clear";

/** 画面から送るコマンド。chizu-wasm が Command に直し、検証する。座標はマスの座標。 */
export type CommandMessage =
  | { kind: "SetSimSpeed"; speed: number }
  | {
      kind: "BuildRoad";
      road: RoadKindName;
      fromX: number;
      fromY: number;
      toX: number;
      toY: number;
    }
  /** 直線と同じ 1 歩を通る既存の道路の区間を、区間ごと road の種類に変える。 */
  | {
      kind: "UpgradeRoad";
      road: RoadKindName;
      fromX: number;
      fromY: number;
      toX: number;
      toY: number;
    }
  /** 線路を (fromX, fromY) から (toX, toY) の直線に引く。駅の線路は駅を置くと同時にできる。 */
  | { kind: "BuildRail"; fromX: number; fromY: number; toX: number; toY: number }
  | { kind: "Bulldoze"; minX: number; minY: number; maxX: number; maxY: number }
  | {
      kind: "PaintZone";
      zone: ZoneName;
      minX: number;
      minY: number;
      maxX: number;
      maxY: number;
    }
  | { kind: "BuyChunk"; cx: number; cy: number }
  /** 区画の種類ごとの税率（‰）。上限は chizu-core の MAX_TAX_PERMILLE で、中核が検証する。 */
  | { kind: "SetTaxRate"; zone: Exclude<ZoneName, "Clear" | "ResidentialLow">; permille: number }
  /** 共同の駐車場の料金（借りた枠 1 つあたりの 1 日の額）。上限は chizu-core の MAX_PARKING_FEE で、中核が検証する。 */
  | { kind: "SetParkingFee"; fee: number }
  | { kind: "PlaceBuilding"; service: ServiceName; atX: number; atY: number; rotation: number }
  /** マス (x, y) の交差点に信号を置くか（on が true）、外す。chizu-wasm がマスのノードを引く。 */
  | { kind: "SetSignal"; x: number; y: number; on: boolean }
  /** 駅 (fromX, fromY) から駅 (toX, toY) への路線を作る。駅は敷地のどのマスでも指せる。 */
  | { kind: "CreateLine"; fromX: number; fromY: number; toX: number; toY: number }
  /** 路線の終点の先に駅 (atX, atY) を足す。最初の駅を指せば環状にする。路線は rail_lines が添える添え字と世代で指す。 */
  | { kind: "ExtendLine"; lineIndex: number; lineGeneration: number; atX: number; atY: number }
  /** 路線の afterIndex 番目（0 始まり）の駅の次に駅 (atX, atY) を挿す。環状の路線にも挿せる。 */
  | {
      kind: "InsertLineStation";
      lineIndex: number;
      lineGeneration: number;
      afterIndex: number;
      atX: number;
      atY: number;
    }
  | { kind: "RemoveLine"; lineIndex: number; lineGeneration: number }
  /** 路線を走らせる列車の本数（0〜16）。行き詰まりを防ぐ上限を超える分は、中核が上限に抑える。 */
  | { kind: "SetLineTrains"; lineIndex: number; lineGeneration: number; trains: number }
  /**
   * マス (x, y) の交差点の、(dx, dy) の向きの腕の横断歩道を戻すか（on が true）、外す。
   * 向きは交差点から出ていく 1 歩で、chizu-wasm がマスのノードと腕の向きを引く。
   */
  | { kind: "SetCrosswalk"; x: number; y: number; dx: number; dy: number; on: boolean };

/** コマンドを適用した結果。Rejected は、Worker が受け付けずに記録しなかったときも含む。 */
export type Outcome = "Applied" | "AppliedPartly" | "Denied" | "Rejected" | "NotImplemented";

/** 区間 1 本の数の個数。chizu-wasm の road_segments と同じ。 */
export const SEGMENT_STRIDE = 5;
/** 線路 1 本の数の個数。chizu-wasm の rail_tracks と同じ。 */
export const RAIL_TRACK_STRIDE = 5;
/** 踏切 1 つの数の個数。chizu-wasm の rail_crossings と同じ。 */
export const RAIL_CROSSING_STRIDE = 7;
/** 駅 1 つの数の個数。chizu-wasm の rail_stations と同じ。 */
export const RAIL_STATION_STRIDE = 6;
/** 建物 1 つの数の個数。chizu-wasm の buildings と同じ。 */
export const BUILDING_STRIDE = 7;

/**
 * 駐車場の形の 1 件の見出しの数（建物の min の x と y、平面の台数、建物の中の台数、長方形の数、枠の並びの数、
 * 私道の数、家の数）。
 */
export const LOT_HEADER = 8;
/** 駐車場の長方形 1 つの数の個数（x0、y0、x1、y1）。私道と家の長方形も同じ個数である。 */
export const LOT_FACE_STRIDE = 4;
/** 駐車場の枠の並び 1 本の数の個数（x0、y0、x1、y1、車の入る側の向きの x と y、枠の数）。 */
export const LOT_STRIP_STRIDE = 7;

/** 駐車場の形の 1 件の長さ。見出しが崩れていれば NaN になる。 */
export function lotRecordLength(faces: number, strips: number, lanes: number, houses: number) {
  return LOT_HEADER + (faces + lanes + houses) * LOT_FACE_STRIDE + strips * LOT_STRIP_STRIDE;
}

/** 駐車場の形の並びが、見出しの数のとおりに過不足なく読めるか。 */
export function lotsWellFormed(data: Float64Array): boolean {
  let i = 0;
  while (i < data.length) {
    if (i + LOT_HEADER > data.length) {
      return false;
    }
    const counts = [4, 5, 6, 7].map((k) => data[i + k] ?? -1);
    if (counts.some((c) => !Number.isInteger(c) || c < 0)) {
      return false;
    }
    const [faces = 0, strips = 0, lanes = 0, houses = 0] = counts;
    i += lotRecordLength(faces, strips, lanes, houses);
  }
  return i === data.length;
}
/** 停めてある台数 1 件の数の個数（建物か共同の駐車場の min の x と y、停めてある台数、そのうち建物の中の台数）。chizu-wasm の parked と同じ。 */
export const PARKED_STRIDE = 4;
/** 建物の供給の値のうち、電気が届いていることを表す値。水道は 2 を足す。 */
export const SUPPLY_POWER = 1;
export const SUPPLY_WATER = 2;
/** 施設 1 つの数の個数。chizu-wasm の facilities と同じ。 */
export const FACILITY_STRIDE = 5;
/** ノード 1 つの数の個数。chizu-wasm の road_nodes と同じ。 */
export const NODE_STRIDE = 5;
/** 信号の腕 1 本の数の個数。chizu-wasm の signal_lights と同じ。 */
export const SIGNAL_STRIDE = 5;

/** チャンクの 1 辺のマス数。chizu-core の CHUNK_SIDE と同じ。 */
export const CHUNK_SIDE = 64;
export const CHUNK_AREA = CHUNK_SIDE * CHUNK_SIDE;
// マスのバイトの意味。chizu-wasm の sim::TILE_WATER などと同じ
/** 水辺。 */
export const TILE_WATER = 1;
/** 道路が通っている。 */
export const TILE_ROAD = 1 << 1;
/** 区画を塗れる。 */
export const TILE_ZONABLE = 1 << 2;
/** 区画の種類（1 住宅（高密度）、2 商業、3 工業、4 オフィス、5 住宅（低密度）、なければ 0）を置くビットの位置。3 ビットを使う。 */
export const TILE_ZONE_SHIFT = 3;
/** 区画に建った建物がある。施設のマスには立たない。 */
export const TILE_BUILDING = 1 << 6;

export type ChunkCoordMessage = { cx: number; cy: number };
export type TileMessage = { x: number; y: number };
export type MoverArea = { minX: number; minY: number; maxX: number; maxY: number };

/** Worker から送るチャンクの中身。tiles は 1 マス 1 バイトを行ごとに並べたもので、移譲して送る。 */
export type ChunkData = {
  cx: number;
  cy: number;
  version: number;
  owned: boolean;
  tiles: ArrayBuffer;
};

/** データレイヤーの番号。chizu-wasm の layers::LAYERS の code と同じ。 */
export type LayerCode = number;

/** データレイヤーの、チャンク 1 つの値。values は 1 マス 1 バイトを行ごとに並べたもので、移譲して送る。 */
export type LayerChunkData = { cx: number; cy: number; version: number; values: ArrayBuffer };

/** 押したものの識別子 `[種類, a, b]`。chizu-wasm の inspect_target に渡す。建物は添え字と世代、施設は min のマス。 */
export type InspectTarget = [number, number, number];

/** 建物や施設の問い合わせ。最初はマスで、押したものが決まってからは識別子で引く。 */
export type InspectQuery =
  | { kind: "tile"; x: number; y: number }
  | { kind: "target"; target: InspectTarget };

function isInspectQuery(value: unknown): value is InspectQuery {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const v = value as Record<string, unknown>;
  if (v.kind === "tile") {
    return Number.isInteger(v.x) && Number.isInteger(v.y);
  }
  return (
    v.kind === "target" &&
    Array.isArray(v.target) &&
    v.target.length === 3 &&
    v.target.every((n) => Number.isInteger(n))
  );
}

export type MainToWorker =
  /** id は画面が振る番号で、結果の通知で返ってくる。 */
  | { kind: "command"; id: number; command: CommandMessage }
  | { kind: "pause"; paused: boolean }
  /** 施設を間口のマス (x, y) に置けるかを、向きごとに中核に問い合わせる。 */
  | { kind: "probe-service"; service: ServiceName; x: number; y: number }
  /** マス (x, y) にある建物か施設の様子を中核に問い合わせる。 */
  | { kind: "inspect"; query: InspectQuery }
  /** 線路を (x0, y0) から (x1, y1) に引けるかと建設費を、状態を変えずに中核に問い合わせる。 */
  | { kind: "probe-rail"; x0: number; y0: number; x1: number; y1: number }
  /** 長方形 (x0, y0)-(x1, y1) を取り壊したときに消えるものを、状態を変えずに中核に問い合わせる。 */
  | { kind: "bulldoze-preview"; x0: number; y0: number; x1: number; y1: number }
  /** フィードバックに添える状態と記録を求める。 */
  | { kind: "feedback" }
  /**
   * visible はいま見えているチャンクを先に送ってほしい順に並べたもの。
   * forgotten は前の view のあとに画面が捨てたチャンクで、Worker は次に見えたときに送り直す。
   */
  | {
      kind: "view";
      visible: ChunkCoordMessage[];
      forgotten: ChunkCoordMessage[];
      /** 移動体を求めるマスの範囲（両端を含む）。null なら移動体を書かない。 */
      movers: MoverArea | null;
    }
  /** 開発用に、記録に入れずに歩く人を出す。from と to はマスの座標。 */
  | { kind: "dev-spawn-walks"; from: TileMessage; to: TileMessage; count: number }
  /** 画面に出すデータレイヤー。送るたびに、見えているチャンクの値を送り直してもらう。 */
  | { kind: "layers"; active: LayerCode[] };

export type WorkerToMain =
  | {
      kind: "ready";
      /** WASM のメモリ。三重バッファはこの中の snapshotOffset の位置にある。 */
      memory: SharedArrayBuffer;
      snapshotOffset: number;
      threads: number;
      crossOriginIsolated: boolean;
      n: number;
      sum: bigint;
      /** chizu-wasm の road_cost_table。 */
      roadCosts: number[];
      /** chizu-wasm の service_table。 */
      serviceTable: number[];
      /** 街の種（10 進の文字列）。地図の名前を作るのに使う。 */
      seed: string;
    }
  | { kind: "chunks"; chunks: ChunkData[] }
  /** データレイヤーの値。版が変わったチャンクだけを送る。 */
  | { kind: "layer-chunks"; layer: LayerCode; chunks: LayerChunkData[] }
  /**
   * 交通の、区間ごとの値（chizu-wasm の traffic_segments）。並びは revision の道路網の road_segments と同じ。
   * 値か道路網が変わったときだけ送る。
   */
  | { kind: "traffic"; revision: number; values: ArrayBuffer }
  /** state は chizu-wasm の feedback_state の JSON、commands は記録を encode したもの。 */
  | { kind: "feedback"; state: string; commands: ArrayBuffer }
  /** probe-service の答え。results は向き 0〜3 の順に、置けるなら null、置けないなら理由の文。 */
  | {
      kind: "service-probe";
      service: ServiceName;
      x: number;
      y: number;
      results: (string | null)[];
      /** 共同の駐車場の届く範囲の中の、車を持てない世帯の数。ほかの施設は 0。 */
      carless: number;
    }
  /**
   * probe-rail の答え。x0〜y1 は問い合わせの写しで、reason は引けない理由の文（引けるなら null）、
   * cost は建設費（直線の形が正しくないときは 0）。
   */
  | {
      kind: "rail-probe";
      x0: number;
      y0: number;
      x1: number;
      y1: number;
      reason: string | null;
      cost: number;
    }
  /**
   * 線路と駅の形。線路や駅が変わったときだけ、全体を送り直す。配列は移譲して送る。
   * tracks は chizu-wasm の rail_tracks、stations は rail_stations、lines は rail_lines、crossings は rail_crossings である。
   * 路線のコマンドも版を上げるので、路線の一覧も同じメッセージで送る。踏切の一覧は道路を引いても変わるので、
   * 道路網の版が変わって踏切の一覧が前と違うときも送り直す。踏切が閉じているかは、三重バッファの枠で送る。
   */
  | {
      kind: "rail";
      revision: number;
      tracks: Float64Array;
      stations: Float64Array;
      lines: Float64Array;
      crossings: Float64Array;
    }
  /** inspect の答え。query は問い合わせの写しで、info は chizu-wasm の inspect の数。何もなければ空。 */
  | { kind: "inspect"; query: InspectQuery; info: number[] }
  /** bulldoze-preview の答え。x0〜y1 は問い合わせの写しで、info は chizu-wasm の bulldoze_preview の数。 */
  | { kind: "bulldoze-preview"; x0: number; y0: number; x1: number; y1: number; info: number[] }
  /** フィードバックの状態を返せなかった。 */
  | { kind: "feedback-error"; message: string }
  /**
   * 信号の腕ごとの灯火。chizu-wasm の signal_lights で、変わったときだけ送る。配列は移譲して送る。
   * 腕 1 本は [ノードの x, ノードの y, 腕の向きの x, 腕の向きの y, 灯火（0 青、1 黄、2 赤）] である。
   */
  | { kind: "signals"; lights: Float64Array }
  /** 道路網の形。網が変わったときだけ、全体を送り直す。配列は移譲して送る。 */
  | { kind: "roads"; revision: number; segments: Float64Array; nodes: Float64Array }
  /**
   * 車を描く道筋（chizu-wasm の take_mover_paths）。新しく求めた道筋と、忘れた道筋の番号だけを、枠より先に送る。
   * 先頭は前の道筋をすべて忘れるか（1 なら忘れる）、忘れる道筋の数とその番号、新しい道筋の数で、新しい道筋
   * ごとに、番号、点の数、点ごとの x、y（マス）、始まりからの長さ（マス）、向き（ラジアン）が続く。
   * 配列は移譲して送る。
   */
  | { kind: "mover-paths"; data: Float64Array }
  /**
   * 建物と街の数字。建物の表か道路網が変わったときだけ送る。stats は chizu-wasm の city_stats で、
   * 建物の数、世帯の上限、職場の上限、住宅・商業・工業・オフィスの需要の順に並び、最後に需要の内訳が続く。
   * lots は chizu-wasm の lots で、敷地を切り欠いた駐車場の形が、切り欠く建物の分だけ並び、続けて共同の駐車場の枠の並びが並ぶ（並びは map/lots.ts にある）。
   * どちらの端に寄せるかが道路網で変わるので、道路網が変わったときにも送り直す。
   */
  | {
      kind: "buildings";
      buildings: Float64Array;
      facilities: Float64Array;
      lots: Float64Array;
      stats: number[];
      /** stats を求めた時点の Worker の tick。需要の控えに使う。 */
      tick: number;
    }
  /**
   * 駐車場に停まっている車の台数。chizu-wasm の parked で、Worker がおよそ 1 秒に 1 回、前と違うときだけ送る。
   * 駐車場のある建物のうち、1 台以上停めている建物ごとに [建物の min の x, min の y, 停めてある台数, そのうち建物の中の台数] が並ぶ。
   * 毎 tick 変わる値なので三重バッファには入れない。配列は移譲して送る。
   */
  | { kind: "parked"; data: Float64Array }
  /**
   * 人口と幸福度とマイルストーン。overview は chizu-wasm の overview で、数の並びは overview.ts にある。
   * stalls は chizu-wasm の growth_stalls で、新築が止まっている道路網ごとに [x, y, 足りない資源] が並ぶ。
   * isolated は chizu-wasm の isolated_facilities で、街の道路網につながっていない施設ごとに [x, y, 種類] が並ぶ。
   * parkingShortage は chizu-wasm の parking_shortage で、駐車場が足りない建物ごとに [x, y, 区画の種類, 満車で車をやめた数] が並ぶ。
   * carless は chizu-wasm の carless で、車を持てない世帯の数、理由ごとの数、世帯の多い区切りの並びである（CARLESS_FIXED が固定の個数）。
   * lotShortage は chizu-wasm の lot_shortage で、住宅・商業・工業・オフィスの順に、需要が正なのに空き区画がなければ 1、そうでなければ 0 の 4 個。tick は overview を作った時点の Worker の tick で、空き区画がない状態の続いた長さを数える。
   */
  | {
      kind: "overview";
      overview: number[];
      stalls: number[];
      isolated: number[];
      lotShortage: number[];
      parkingShortage: number[];
      carless: number[];
      tick: number;
    }
  /**
   * id は画面が振ったコマンドの番号、seq は記録の通し番号。
   * 記録する前に受け付けなかったコマンドには seq がない。
   */
  | { kind: "outcome"; id: number; seq?: number; outcome: Outcome; reason?: string }
  /**
   * 収支の見込みと税率。chizu-wasm の finance で、Worker が一定の間隔で送る。
   * 並びは税率の上限、区画の種類ごとの税率、税収、道路の維持費、発電所と浄水場の維持費、共同の駐車場の維持費。
   */
  | { kind: "finance"; values: number[] }
  /**
   * 街の数字だけの送り直し。需要は税率、幸福度、人口、職のない住民で、建物の表が変わらなくても動くため、
   * Worker が一定の間隔で確かめ、前と数が違うときだけ送る。stats は buildings と同じ並びである。
   */
  | { kind: "cityStats"; stats: number[]; tick: number }
  /** dev-spawn-walks で出せた人数。 */
  | { kind: "dev-spawned"; count: number }
  | { kind: "error"; message: string };

function isChunkData(value: unknown): value is ChunkData {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const v = value as Record<string, unknown>;
  return (
    Number.isInteger(v.cx) &&
    Number.isInteger(v.cy) &&
    Number.isInteger(v.version) &&
    typeof v.owned === "boolean" &&
    v.tiles instanceof ArrayBuffer &&
    v.tiles.byteLength === CHUNK_AREA
  );
}

export function isWorkerToMain(value: unknown): value is WorkerToMain {
  if (typeof value !== "object" || value === null || !("kind" in value)) {
    return false;
  }
  const v = value as Record<string, unknown>;
  switch (v.kind) {
    case "ready":
      return (
        v.memory instanceof SharedArrayBuffer &&
        typeof v.snapshotOffset === "number" &&
        typeof v.threads === "number" &&
        typeof v.crossOriginIsolated === "boolean" &&
        typeof v.n === "number" &&
        typeof v.sum === "bigint" &&
        Array.isArray(v.roadCosts) &&
        Array.isArray(v.serviceTable) &&
        typeof v.seed === "string"
      );
    case "chunks":
      return Array.isArray(v.chunks) && v.chunks.every(isChunkData);
    case "traffic":
      return typeof v.revision === "number" && v.values instanceof ArrayBuffer;
    case "layer-chunks":
      return (
        Number.isInteger(v.layer) &&
        Array.isArray(v.chunks) &&
        v.chunks.every(
          (c: Record<string, unknown>) =>
            Number.isInteger(c.cx) &&
            Number.isInteger(c.cy) &&
            typeof c.version === "number" &&
            c.values instanceof ArrayBuffer &&
            c.values.byteLength === CHUNK_AREA,
        )
      );
    case "roads":
      return (
        typeof v.revision === "number" &&
        v.segments instanceof Float64Array &&
        v.segments.length % SEGMENT_STRIDE === 0 &&
        v.nodes instanceof Float64Array &&
        v.nodes.length % NODE_STRIDE === 0
      );
    case "mover-paths":
      return v.data instanceof Float64Array && v.data.length >= 3;
    case "signals":
      return v.lights instanceof Float64Array && v.lights.length % SIGNAL_STRIDE === 0;
    case "buildings":
      return (
        v.buildings instanceof Float64Array &&
        v.buildings.length % BUILDING_STRIDE === 0 &&
        Array.isArray(v.stats) &&
        typeof v.tick === "number" &&
        v.facilities instanceof Float64Array &&
        v.facilities.length % FACILITY_STRIDE === 0 &&
        v.lots instanceof Float64Array &&
        lotsWellFormed(v.lots) &&
        v.stats.every((x) => typeof x === "number") &&
        validCityStats(v.stats)
      );
    case "parked":
      return v.data instanceof Float64Array && v.data.length % PARKED_STRIDE === 0;
    case "cityStats":
      return (
        typeof v.tick === "number" &&
        Array.isArray(v.stats) &&
        v.stats.every((x) => typeof x === "number") &&
        validCityStats(v.stats)
      );
    case "overview":
      return (
        Array.isArray(v.overview) &&
        v.overview.length === OVERVIEW_LENGTH &&
        v.overview.every((x) => typeof x === "number") &&
        Array.isArray(v.stalls) &&
        v.stalls.length % 3 === 0 &&
        v.stalls.every((x) => typeof x === "number") &&
        Array.isArray(v.isolated) &&
        v.isolated.length % 3 === 0 &&
        v.isolated.every((x) => typeof x === "number") &&
        Array.isArray(v.lotShortage) &&
        v.lotShortage.length === 4 &&
        v.lotShortage.every((x) => typeof x === "number") &&
        Array.isArray(v.parkingShortage) &&
        v.parkingShortage.length % 4 === 0 &&
        v.parkingShortage.every((x) => typeof x === "number") &&
        Array.isArray(v.carless) &&
        v.carless.length >= CARLESS_FIXED &&
        (v.carless.length - CARLESS_FIXED) % 3 === 0 &&
        v.carless.every((x) => typeof x === "number") &&
        typeof v.tick === "number"
      );
    case "outcome":
      return (
        Number.isInteger(v.id) &&
        (v.seq === undefined || Number.isInteger(v.seq)) &&
        typeof v.outcome === "string" &&
        (v.reason === undefined || typeof v.reason === "string")
      );
    case "feedback":
      return typeof v.state === "string" && v.commands instanceof ArrayBuffer;
    case "finance":
      return (
        Array.isArray(v.values) &&
        v.values.length === FINANCE_LENGTH &&
        v.values.every((n) => typeof n === "number")
      );
    case "dev-spawned":
      return Number.isInteger(v.count);
    case "service-probe":
      return (
        typeof v.service === "string" &&
        Number.isInteger(v.x) &&
        Number.isInteger(v.y) &&
        Array.isArray(v.results) &&
        v.results.length === 4 &&
        v.results.every((r) => r === null || typeof r === "string") &&
        typeof v.carless === "number"
      );
    case "inspect":
      return (
        isInspectQuery(v.query) &&
        Array.isArray(v.info) &&
        v.info.every((n) => typeof n === "number")
      );
    case "rail":
      return (
        typeof v.revision === "number" &&
        v.tracks instanceof Float64Array &&
        v.tracks.length % RAIL_TRACK_STRIDE === 0 &&
        v.stations instanceof Float64Array &&
        v.stations.length % RAIL_STATION_STRIDE === 0 &&
        v.lines instanceof Float64Array &&
        v.crossings instanceof Float64Array &&
        v.crossings.length % RAIL_CROSSING_STRIDE === 0
      );
    case "rail-probe":
      return (
        Number.isInteger(v.x0) &&
        Number.isInteger(v.y0) &&
        Number.isInteger(v.x1) &&
        Number.isInteger(v.y1) &&
        (v.reason === null || typeof v.reason === "string") &&
        typeof v.cost === "number"
      );
    case "bulldoze-preview":
      return (
        Number.isInteger(v.x0) &&
        Number.isInteger(v.y0) &&
        Number.isInteger(v.x1) &&
        Number.isInteger(v.y1) &&
        Array.isArray(v.info) &&
        v.info.every((n) => typeof n === "number")
      );
    case "feedback-error":
    case "error":
      return typeof v.message === "string";
    default:
      return false;
  }
}
