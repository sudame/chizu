// 遊びながら残したフィードバックを、開発サーバーで受けてリポジトリの feedback/ に書く。
// 1 件ごとに feedback/<日時>-<番号>/ を作り、note.md、screenshot.png、state.json、commands.rec を置く。
// 置き場所は環境変数 CHIZU_FEEDBACK_DIR で変えられる。test:wasm は一時ディレクトリを渡す。
//
// 開いているほかのページから書き込まれないよう、同じ localhost のページからの JSON の POST だけを受ける。
// GET には 204 を返す。画面はこれでフィードバックを受けるサーバーかを見分け、ボタンを出す。

import { mkdir, readdir, writeFile } from "node:fs/promises";
import type { IncomingMessage, ServerResponse } from "node:http";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import type { Connect, Plugin } from "vite";

export const FEEDBACK_PATH = "/__feedback";

/** 本文の上限（バイト）。スクリーンショットの PNG を base64 にしても収まる大きさにする。 */
export const MAX_BODY_BYTES = 20 * 1024 * 1024;

const LOCAL_HOSTS = new Set(["localhost", "127.0.0.1", "[::1]"]);

/** 画面から送る 1 件。screenshot と commands は base64 で送る。 */
export type FeedbackBody = {
  note: string;
  screenshot: string;
  state: Record<string, unknown>;
  commands: string;
};

function isFeedbackBody(v: unknown): v is FeedbackBody {
  if (typeof v !== "object" || v === null) {
    return false;
  }
  const b = v as Record<string, unknown>;
  return (
    typeof b.note === "string" &&
    typeof b.screenshot === "string" &&
    typeof b.state === "object" &&
    b.state !== null &&
    typeof b.commands === "string"
  );
}

const pad = (n: number, width = 2) => String(n).padStart(width, "0");

function stamp(d: Date): string {
  return `${d.getFullYear()}${pad(d.getMonth() + 1)}${pad(d.getDate())}-${pad(d.getHours())}${pad(d.getMinutes())}${pad(d.getSeconds())}`;
}

/** 本文を読む。上限を超えたら読むのをやめて null を返す。 */
function readBody(req: IncomingMessage, max: number): Promise<string | null> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = [];
    let size = 0;
    const onData = (chunk: Buffer) => {
      size += chunk.length;
      if (size > max) {
        req.off("data", onData);
        req.pause();
        resolve(null);
        return;
      }
      chunks.push(chunk);
    };
    req.on("data", onData);
    req.on("end", () => resolve(Buffer.concat(chunks).toString("utf8")));
    req.on("error", reject);
  });
}

/** 同じ秒に何件来ても重ならないよう、作れなかった番号は飛ばして次の番号で作り直す。 */
async function makeEntryDir(root: string): Promise<string> {
  await mkdir(root, { recursive: true });
  const base = stamp(new Date());
  for (let n = (await readdir(root)).length + 1; ; n++) {
    const dir = join(root, `${base}-${pad(n, 3)}`);
    try {
      await mkdir(dir);
      return dir;
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "EEXIST") {
        throw error;
      }
    }
  }
}

async function save(root: string, body: FeedbackBody): Promise<string> {
  const dir = await makeEntryDir(root);
  await writeFile(join(dir, "note.md"), `# フィードバック\n\n${body.note.trim()}\n`);
  await writeFile(join(dir, "screenshot.png"), Buffer.from(body.screenshot, "base64"));
  await writeFile(join(dir, "state.json"), `${JSON.stringify(body.state, null, 2)}\n`);
  await writeFile(join(dir, "commands.rec"), Buffer.from(body.commands, "base64"));
  return dir;
}

function reply(res: ServerResponse, status: number, value: unknown): void {
  res.statusCode = status;
  res.setHeader("Content-Type", "application/json");
  res.end(JSON.stringify(value));
}

/** Origin が、このサーバー自身（localhost の同じポート）か。 */
function isSameLocalOrigin(req: IncomingMessage): boolean {
  const origin = req.headers.origin;
  const host = req.headers.host;
  if (origin === undefined || host === undefined) {
    return false;
  }
  try {
    const url = new URL(origin);
    return url.protocol === "http:" && url.host === host && LOCAL_HOSTS.has(url.hostname);
  } catch {
    return false;
  }
}

export function feedbackHandler(
  root: string,
  maxBytes = MAX_BODY_BYTES,
): Connect.NextHandleFunction {
  return (req, res, next) => {
    if (req.url !== FEEDBACK_PATH) {
      next();
      return;
    }
    if (req.method === "GET") {
      res.statusCode = 204;
      res.end();
      return;
    }
    if (req.method !== "POST") {
      reply(res, 405, { error: "POST で送る" });
      return;
    }
    if (!isSameLocalOrigin(req)) {
      reply(res, 403, { error: "このページからしか送れない" });
      return;
    }
    if (!(req.headers["content-type"] ?? "").startsWith("application/json")) {
      reply(res, 415, { error: "application/json で送る" });
      return;
    }
    // 返事を送り終えてから接続を切り、残りの本文は読まない
    const tooLarge = () => {
      res.setHeader("Connection", "close");
      res.on("finish", () => req.destroy());
      reply(res, 413, { error: `本文が ${maxBytes} バイトを超えている` });
    };
    if (Number(req.headers["content-length"] ?? 0) > maxBytes) {
      tooLarge();
      return;
    }
    readBody(req, maxBytes)
      .then((text) => {
        if (text === null) {
          tooLarge();
          return;
        }
        let body: unknown;
        try {
          body = JSON.parse(text);
        } catch {
          body = null;
        }
        if (!isFeedbackBody(body)) {
          reply(res, 400, { error: "フィードバックの形が違う" });
          return;
        }
        return save(root, body).then((dir) => reply(res, 200, { dir }));
      })
      .catch((error: unknown) => reply(res, 500, { error: String(error) }));
  };
}

export function feedbackPlugin(): Plugin {
  const root =
    process.env.CHIZU_FEEDBACK_DIR ?? fileURLToPath(new URL("../feedback", import.meta.url));
  const handler = feedbackHandler(root);
  return {
    name: "chizu-feedback",
    configureServer(server) {
      server.middlewares.use(handler);
    },
    configurePreviewServer(server) {
      server.middlewares.use(handler);
    },
  };
}
