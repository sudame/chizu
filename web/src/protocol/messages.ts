// メインスレッドと Worker のあいだでやりとりするメッセージ。

export type RoadKindName = "Small" | "Medium" | "Large";
/** 施設の種類。 */
export type ServiceName = "PowerPlant" | "WaterPump";
/** chizu-wasm の city_stats の数の個数。 */
export const CITY_STATS_LENGTH = 16;
/** chizu-wasm の finance の数の個数。 */
export const FINANCE_LENGTH = 12;

/** chizu-wasm の overview の数の個数。 */
export const OVERVIEW_LENGTH = 21;
/** 区画の種類。Clear は区画を消す。 */
export type ZoneName = "Residential" | "Commercial" | "Industrial" | "Office" | "Clear";

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
  | { kind: "SetTaxRate"; zone: Exclude<ZoneName, "Clear">; permille: number }
  | { kind: "PlaceBuilding"; service: ServiceName; atX: number; atY: number; rotation: number }
  /** マス (x, y) の交差点に信号を置くか（on が true）、外す。chizu-wasm がマスのノードを引く。 */
  | { kind: "SetSignal"; x: number; y: number; on: boolean };

/** コマンドを適用した結果。Rejected は、Worker が受け付けずに記録しなかったときも含む。 */
export type Outcome = "Applied" | "Denied" | "Rejected" | "NotImplemented";

/** 区間 1 本の数の個数。chizu-wasm の road_segments と同じ。 */
export const SEGMENT_STRIDE = 5;
/** 建物 1 つの数の個数。chizu-wasm の buildings と同じ。 */
export const BUILDING_STRIDE = 7;
/** 建物の供給の値のうち、電気が届いていることを表す値。水道は 2 を足す。 */
export const SUPPLY_POWER = 1;
export const SUPPLY_WATER = 2;
/** 施設 1 つの数の個数。chizu-wasm の facilities と同じ。 */
export const FACILITY_STRIDE = 5;
/** ノード 1 つの数の個数。chizu-wasm の road_nodes と同じ。 */
export const NODE_STRIDE = 4;
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
/** 区画の種類（1 住宅、2 商業、3 工業、4 オフィス、なければ 0）を置くビットの位置。 */
export const TILE_ZONE_SHIFT = 3;

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
  /** 1 倍より遅く進めるときの倍率（0 より大きく 1 以下）。中核の速度は変えず、tick を進める頻度だけを落とす。 */
  | { kind: "pace"; pace: number }
  /** 施設を間口のマス (x, y) に置けるかを、向きごとに中核に問い合わせる。 */
  | { kind: "probe-service"; service: ServiceName; x: number; y: number }
  /** マス (x, y) にある建物か施設の様子を中核に問い合わせる。 */
  | { kind: "inspect"; query: InspectQuery }
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
    }
  | { kind: "chunks"; chunks: ChunkData[] }
  /** データレイヤーの値。版が変わったチャンクだけを送る。 */
  | { kind: "layer-chunks"; layer: LayerCode; chunks: LayerChunkData[] }
  /** state は chizu-wasm の feedback_state の JSON、commands は記録を encode したもの。 */
  | { kind: "feedback"; state: string; commands: ArrayBuffer }
  /** probe-service の答え。results は向き 0〜3 の順に、置けるなら null、置けないなら理由の文。 */
  | {
      kind: "service-probe";
      service: ServiceName;
      x: number;
      y: number;
      results: (string | null)[];
    }
  /** inspect の答え。query は問い合わせの写しで、info は chizu-wasm の inspect の数。何もなければ空。 */
  | { kind: "inspect"; query: InspectQuery; info: number[] }
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
   * 建物と街の数字。建物の表が変わったときだけ送る。stats は chizu-wasm の city_stats で、
   * 建物の数、世帯の上限、職場の上限、住宅・商業・工業・オフィスの需要の順に並ぶ。
   */
  | { kind: "buildings"; buildings: Float64Array; facilities: Float64Array; stats: number[] }
  /**
   * 人口と幸福度とマイルストーン。overview は chizu-wasm の overview で、数の並びは overview.ts にある。
   * stalls は chizu-wasm の growth_stalls で、新築が止まっている道路網ごとに [x, y, 足りない資源] が並ぶ。
   */
  | { kind: "overview"; overview: number[]; stalls: number[] }
  /**
   * id は画面が振ったコマンドの番号、seq は記録の通し番号。
   * 記録する前に受け付けなかったコマンドには seq がない。
   */
  | { kind: "outcome"; id: number; seq?: number; outcome: Outcome; reason?: string }
  /**
   * 収支の見込みと税率。chizu-wasm の finance で、Worker が一定の間隔で送る。
   * 並びは税率の上限、区画の種類ごとの税率、税収、道路の維持費、発電所と浄水場の維持費。
   */
  | { kind: "finance"; values: number[] }
  /** dev-spawn-walks で出せた人数。 */
  | { kind: "dev-spawned"; count: number }
  /** Worker が受け取って使っている倍率。画面の表示はこの値を使う。 */
  | { kind: "pace"; pace: number }
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
        Array.isArray(v.serviceTable)
      );
    case "chunks":
      return Array.isArray(v.chunks) && v.chunks.every(isChunkData);
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
    case "signals":
      return v.lights instanceof Float64Array && v.lights.length % SIGNAL_STRIDE === 0;
    case "buildings":
      return (
        v.buildings instanceof Float64Array &&
        v.buildings.length % BUILDING_STRIDE === 0 &&
        Array.isArray(v.stats) &&
        v.facilities instanceof Float64Array &&
        v.facilities.length % FACILITY_STRIDE === 0 &&
        v.stats.length === CITY_STATS_LENGTH
      );
    case "overview":
      return (
        Array.isArray(v.overview) &&
        v.overview.length === OVERVIEW_LENGTH &&
        v.overview.every((x) => typeof x === "number") &&
        Array.isArray(v.stalls) &&
        v.stalls.length % 3 === 0 &&
        v.stalls.every((x) => typeof x === "number")
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
    case "pace":
      return typeof v.pace === "number" && v.pace > 0 && v.pace <= 1;
    case "service-probe":
      return (
        typeof v.service === "string" &&
        Number.isInteger(v.x) &&
        Number.isInteger(v.y) &&
        Array.isArray(v.results) &&
        v.results.length === 4 &&
        v.results.every((r) => r === null || typeof r === "string")
      );
    case "inspect":
      return (
        isInspectQuery(v.query) &&
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
