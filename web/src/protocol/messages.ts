// メインスレッドと Worker のあいだでやりとりするメッセージ。

/** 画面から送るコマンド。chizu-wasm が Command に直し、検証する。 */
export type CommandMessage = { kind: "SetSimSpeed"; speed: number };

export type MainToWorker =
  | { kind: "command"; command: CommandMessage }
  | { kind: "pause"; paused: boolean };

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
  | { kind: "error"; message: string };

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
    case "error":
      return typeof v.message === "string";
    default:
      return false;
  }
}
