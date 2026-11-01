// メインスレッドと Worker のあいだでやりとりするメッセージ。

/** 画面から送るコマンド。chizu-wasm が Command に直し、検証する。 */
export type CommandMessage = { kind: "SetSimSpeed"; speed: number };

/** チャンクの 1 辺のマス数。chizu-core の CHUNK_SIDE と同じ。 */
export const CHUNK_SIDE = 64;
export const CHUNK_AREA = CHUNK_SIDE * CHUNK_SIDE;
/** マスのバイトのうち、水辺を表すビット。chizu-wasm の sim::TILE_WATER と同じ。 */
export const TILE_WATER = 1;

export type ChunkCoordMessage = { cx: number; cy: number };

/** Worker から送るチャンクの中身。tiles は 1 マス 1 バイトを行ごとに並べたもので、移譲して送る。 */
export type ChunkData = {
  cx: number;
  cy: number;
  version: number;
  owned: boolean;
  tiles: ArrayBuffer;
};

export type MainToWorker =
  | { kind: "command"; command: CommandMessage }
  | { kind: "pause"; paused: boolean }
  /**
   * visible はいま見えているチャンクを先に送ってほしい順に並べたもの。
   * forgotten は前の view のあとに画面が捨てたチャンクで、Worker は次に見えたときに送り直す。
   */
  | { kind: "view"; visible: ChunkCoordMessage[]; forgotten: ChunkCoordMessage[] };

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
    }
  | { kind: "chunks"; chunks: ChunkData[] }
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
        typeof v.sum === "bigint"
      );
    case "chunks":
      return Array.isArray(v.chunks) && v.chunks.every(isChunkData);
    case "error":
      return typeof v.message === "string";
    default:
      return false;
  }
}
