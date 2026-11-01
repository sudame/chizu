// ビルドしたページを vite preview で配信し、ヘッドレスの Chrome で開く。
// preview を使うのは、本番と同じくビルドしたものを COOP/COEP のヘッダーつきで配信して確かめるためである。
// スレッドつきの WASM が Worker で動き、正しい和を返すかを確かめる。
// Chrome は mise の tools で入れた chrome-headless-shell を PATH から探す。

import { accessSync, constants } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";
import { preview } from "vite";

const TIMEOUT_MS = 30_000;
const MIN_THREADS = 2;

function findOnPath(name: string): string {
  for (const dir of (process.env.PATH ?? "").split(":")) {
    const candidate = join(dir, name);
    try {
      accessSync(candidate, constants.X_OK);
      return candidate;
    } catch {}
  }
  throw new Error(`${name} が PATH にない。mise install で入れる`);
}

const failures: string[] = [];
function check(ok: boolean, label: string): void {
  console.log(`${ok ? "ok" : "NG"}: ${label}`);
  if (!ok) {
    failures.push(label);
  }
}

// 引数で URL を渡すと、preview を立てずにその URL を開く。開発サーバーを確かめるときに使う
const givenUrl = process.argv[2];
const server =
  givenUrl === undefined
    ? await preview({
        root: fileURLToPath(new URL("..", import.meta.url)),
        logLevel: "warn",
        preview: { host: "127.0.0.1", port: 0 },
      })
    : undefined;
const url = givenUrl ?? server?.resolvedUrls?.local[0];
if (url === undefined) {
  throw new Error("preview の URL が分からない");
}
const browser = await chromium.launch({ executablePath: findOnPath("chrome-headless-shell") });

try {
  const page = await browser.newPage();
  page.on("console", (message) => console.log(`[console.${message.type()}] ${message.text()}`));
  page.on("pageerror", (error) => console.log(`[pageerror] ${error.message}`));

  await page.goto(url);
  const status = page.locator("#status");
  await status.and(page.locator(":is([data-state=done], [data-state=error])")).waitFor({
    timeout: TIMEOUT_MS,
  });

  const state = await status.getAttribute("data-state");
  check(state === "done", `Worker が結果を返した（${await status.textContent()}）`);

  check(
    await page.evaluate(() => self.crossOriginIsolated),
    "ページの crossOriginIsolated が true",
  );
  check(
    (await status.getAttribute("data-worker-isolated")) === "true",
    "Worker の crossOriginIsolated が true",
  );

  const threads = Number(await status.getAttribute("data-threads"));
  check(threads >= MIN_THREADS, `rayon のスレッドが ${MIN_THREADS} 本以上ある（${threads} 本）`);

  const n = BigInt((await status.getAttribute("data-n")) ?? "0");
  const sum = await status.getAttribute("data-sum");
  const expected = (n * (n + 1n)) / 2n;
  check(n > 0n && sum === String(expected), `1 から ${n} までの和が ${expected}（${sum}）`);
} finally {
  await browser.close();
  await server?.close();
}

if (failures.length > 0) {
  console.error(`test:wasm は ${failures.length} 件失敗した`);
  process.exit(1);
}
