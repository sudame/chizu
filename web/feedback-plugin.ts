// 遊びながら残したフィードバックを、開発サーバーで受けてリポジトリの feedback/ に書く。
// 1 件ごとに feedback/<日時>-<番号>/ を作り、note.md、screenshot.png、state.json、commands.rec を置く。
// 置き場所は環境変数 CHIZU_FEEDBACK_DIR で変えられる。test:wasm は一時ディレクトリを渡す。

import { mkdir, readdir, writeFile } from "node:fs/promises";
import type { IncomingMessage, ServerResponse } from "node:http";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import type { Connect, Plugin } from "vite";

export const FEEDBACK_PATH = "/__feedback";

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

async function readBody(req: IncomingMessage): Promise<string> {
  const chunks: Buffer[] = [];
  for await (const chunk of req) {
    chunks.push(chunk as Buffer);
  }
  return Buffer.concat(chunks).toString("utf8");
}

async function save(root: string, body: FeedbackBody): Promise<string> {
  await mkdir(root, { recursive: true });
  const count = (await readdir(root)).length;
  const name = `${stamp(new Date())}-${pad(count + 1, 3)}`;
  const dir = join(root, name);
  await mkdir(dir);
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

export function feedbackPlugin(): Plugin {
  const root =
    process.env.CHIZU_FEEDBACK_DIR ?? fileURLToPath(new URL("../feedback", import.meta.url));
  const handler: Connect.NextHandleFunction = (req, res, next) => {
    if (req.method !== "POST") {
      next();
      return;
    }
    readBody(req)
      .then((text) => {
        const body: unknown = JSON.parse(text);
        if (!isFeedbackBody(body)) {
          reply(res, 400, { error: "フィードバックの形が違う" });
          return;
        }
        return save(root, body).then((dir) => reply(res, 200, { dir }));
      })
      .catch((error: unknown) => reply(res, 500, { error: String(error) }));
  };
  return {
    name: "chizu-feedback",
    configureServer(server) {
      server.middlewares.use(FEEDBACK_PATH, handler);
    },
    configurePreviewServer(server) {
      server.middlewares.use(FEEDBACK_PATH, handler);
    },
  };
}
