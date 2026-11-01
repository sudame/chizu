// メインスレッドと Worker のあいだでやりとりするメッセージ。

export type RoadKindName = "Small" | "Medium" | "Large";
/** 施設の種類。 */
export type ServiceName = "PowerPlant" | "WaterPump";
/** chizu-wasm の city_stats の数の個数。 */
export const CITY_STATS_LENGTH = 16;
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
  | { kind: "PlaceBuilding"; service: ServiceName; atX: number; atY: number; rotation: number };

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
export const NODE_STRIDE = 3;

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

export type MainToWorker =
  /** id は画面が振る番号で、結果の通知で返ってくる。 */
  | { kind: "command"; id: number; command: CommandMessage }
  | { kind: "pause"; paused: boolean }
  /** 施設を間口のマス (x, y) に置けるかを、向きごとに中核に問い合わせる。 */
  | { kind: "probe-service"; service: ServiceName; x: number; y: number }
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
  /** フィードバックの状態を返せなかった。 */
  | { kind: "feedback-error"; message: string }
  /** 道路網の形。網が変わったときだけ、全体を送り直す。配列は移譲して送る。 */
  | { kind: "roads"; revision: number; segments: Float64Array; nodes: Float64Array }
  /**
   * 建物と街の数字。建物の表が変わったときだけ送る。stats は chizu-wasm の city_stats で、
   * 建物の数、世帯の上限、職場の上限、住宅・商業・工業・オフィスの需要の順に並ぶ。
   */
  | { kind: "buildings"; buildings: Float64Array; facilities: Float64Array; stats: number[] }
  /**
   * id は画面が振ったコマンドの番号、seq は記録の通し番号。
   * 記録する前に受け付けなかったコマンドには seq がない。
   */
  | { kind: "outcome"; id: number; seq?: number; outcome: Outcome; reason?: string }
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
    case "buildings":
      return (
        v.buildings instanceof Float64Array &&
        v.buildings.length % BUILDING_STRIDE === 0 &&
        Array.isArray(v.stats) &&
        v.facilities instanceof Float64Array &&
        v.facilities.length % FACILITY_STRIDE === 0 &&
        v.stats.length === CITY_STATS_LENGTH
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
    case "dev-spawned":
      return Number.isInteger(v.count);
    case "service-probe":
      return (
        typeof v.service === "string" &&
        Number.isInteger(v.x) &&
        Number.isInteger(v.y) &&
        Array.isArray(v.results) &&
        v.results.length === 4 &&
        v.results.every((r) => r === null || typeof r === "string")
      );
    case "feedback-error":
    case "error":
      return typeof v.message === "string";
    default:
      return false;
  }
}
