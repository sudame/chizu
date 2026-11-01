// メインスレッドと Worker のあいだでやりとりするメッセージ。

export type MainToWorker = { kind: "sum"; n: number };

export type WorkerToMain =
  | {
      kind: "sum";
      n: number;
      sum: bigint;
      threads: number;
      crossOriginIsolated: boolean;
    }
  | { kind: "error"; message: string };

export function isWorkerToMain(value: unknown): value is WorkerToMain {
  if (typeof value !== "object" || value === null || !("kind" in value)) {
    return false;
  }
  const v = value as Record<string, unknown>;
  switch (v.kind) {
    case "sum":
      return (
        typeof v.n === "number" &&
        typeof v.sum === "bigint" &&
        typeof v.threads === "number" &&
        typeof v.crossOriginIsolated === "boolean"
      );
    case "error":
      return typeof v.message === "string";
    default:
      return false;
  }
}

export function describeMessage(message: WorkerToMain): string {
  switch (message.kind) {
    case "sum":
      return `1 から ${message.n} までの和は ${message.sum}（スレッド ${message.threads} 本）`;
    case "error":
      return `WASM を動かせなかった：${message.message}`;
  }
}
