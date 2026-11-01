// スレッドつきの WASM の中核をヘッドレスの Chrome で回して時間とメモリを測り、同じ条件のネイティブの値と並べる。
// 測る中身は chizu-scenario の perf.rs にあり、WASM は chizu-bench-wasm、ネイティブは perf のベンチから呼ぶ。
// スレッド数は 1、4、navigator.hardwareConcurrency の 3 通りで、WASM はそれぞれ新しい Worker で測る。
// 結果は JSON で標準出力と target/bench-wasm.json に書く。--wasm-only を渡すとネイティブは回さない。
// ほかの処理が重なると数字がぶれるので、測る前とあとの 1 分間の load average を結果に添える。
// --threads=1,4 のように渡すと、そのスレッド数だけを測る。--rounds=3 のように渡すと、全体を繰り返す。

import { execFileSync } from "node:child_process";
import { accessSync, constants, readFileSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import { loadavg } from "node:os";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const TIMEOUT_MS = 30 * 60_000;
const repo = fileURLToPath(new URL("../..", import.meta.url));
const roots: Record<string, string> = {
  "/pkg/": join(repo, "target/bench-wasm/pkg"),
  "/": join(repo, "web/bench"),
};
const types: Record<string, string> = {
  ".html": "text/html",
  ".js": "text/javascript",
  ".wasm": "application/wasm",
};
const wasmOnly = process.argv.includes("--wasm-only");
const onlyThreads = process.argv
  .find((a) => a.startsWith("--threads="))
  ?.slice("--threads=".length)
  .split(",")
  .map(Number);
const rounds = Number(
  process.argv.find((a) => a.startsWith("--rounds="))?.slice("--rounds=".length) ?? 1,
);

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

// SharedArrayBuffer を使うので、COOP と COEP のヘッダーをつけて配る
const server = createServer((req, res) => {
  const path = new URL(req.url ?? "/", "http://localhost").pathname;
  const headers = {
    "Cross-Origin-Opener-Policy": "same-origin",
    "Cross-Origin-Embedder-Policy": "require-corp",
  };
  if (path === "/") {
    res.writeHead(200, { ...headers, "Content-Type": "text/html" }).end("<!doctype html>");
    return;
  }
  const prefix = Object.keys(roots).find((p) => path.startsWith(p)) ?? "/";
  const file = normalize(join(roots[prefix] ?? "", path.slice(prefix.length)));
  try {
    const body = readFileSync(file);
    const type = types[extname(file)] ?? "application/octet-stream";
    res.writeHead(200, { ...headers, "Content-Type": type }).end(body);
  } catch {
    res.writeHead(404, headers).end();
  }
});
await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
const url = `http://127.0.0.1:${(server.address() as AddressInfo).port}/`;
const browser = await chromium.launch({ executablePath: findOnPath("chrome-headless-shell") });

type Run = {
  round: number;
  threads: number;
  results: Record<string, Record<string, number | null>>;
  load_average: number[];
};
type Results = Run["results"];
const load = () => Math.round((loadavg()[0] ?? 0) * 100) / 100;

function measureNative(threads: number): Results {
  const out = execFileSync(
    "cargo",
    ["bench", "--quiet", "-p", "chizu-scenario", "--bench", "perf"],
    {
      cwd: repo,
      encoding: "utf8",
      env: { ...process.env, CHIZU_PERF_THREADS: String(threads) },
      stdio: ["ignore", "pipe", "inherit"],
      timeout: TIMEOUT_MS,
    },
  );
  return JSON.parse(out.trim().split("\n").at(-1) ?? "{}").runs[0].results;
}

// 負荷の揺れが片方にだけ乗らないように、スレッド数ごとに WASM とネイティブを続けて測る
const wasm: Run[] = [];
const native: Run[] = [];
let hardwareConcurrency = 0;
try {
  const page = await browser.newPage();
  page.on("console", (message) => console.error(`[console.${message.type()}] ${message.text()}`));
  await page.goto(url);
  hardwareConcurrency = await page.evaluate(() => navigator.hardwareConcurrency);
  for (let round = 1; round <= rounds; round++) {
    for (const threads of onlyThreads ?? [...new Set([1, 4, hardwareConcurrency])]) {
      console.error(`${round} 回目：WASM をスレッド ${threads} 本で測る（load ${load()}）`);
      let before = load();
      const reply = await page.evaluate(
        (n) =>
          new Promise<{ ok: boolean; results?: Results; error?: string }>((resolve) => {
            const worker = new Worker("/worker.js", { type: "module" });
            worker.onmessage = ({ data }) => {
              worker.terminate();
              resolve(data);
            };
            worker.onerror = (e) => resolve({ ok: false, error: e.message });
            worker.postMessage({ threads: n });
          }),
        threads,
      );
      if (!reply.ok || reply.results === undefined) {
        throw new Error(`WASM のベンチが失敗した（スレッド ${threads} 本）：${reply.error}`);
      }
      wasm.push({ round, threads, results: reply.results, load_average: [before, load()] });
      if (!wasmOnly) {
        console.error(`${round} 回目：ネイティブをスレッド ${threads} 本で測る（load ${load()}）`);
        before = load();
        const results = measureNative(threads);
        native.push({ round, threads, results, load_average: [before, load()] });
      }
    }
  }
} finally {
  await browser.close();
  server.close();
}

// 時間の項目ごとに、WASM がネイティブの何倍かかったか
const timeKeys = ["day_ns", "tick_mean_ns", "tick_worst_ns", "mean_ns"];
const ratio = wasm.flatMap(({ round, threads, results }) => {
  const base = native.find((r) => r.round === round && r.threads === threads)?.results;
  if (base === undefined) {
    return [];
  }
  const items: Record<string, number> = {};
  for (const [name, values] of Object.entries(results)) {
    for (const key of timeKeys) {
      const w = values[key];
      const n = base[name]?.[key];
      if (typeof w === "number" && typeof n === "number" && n > 0) {
        items[`${name}.${key}`] = Math.round((w / n) * 100) / 100;
      }
    }
  }
  return [{ round, threads, wasm_over_native: items }];
});

const report = JSON.stringify(
  { browser: browser.version(), hardware_concurrency: hardwareConcurrency, wasm, native, ratio },
  null,
  2,
);
writeFileSync(join(repo, "target/bench-wasm.json"), `${report}\n`);
console.log(report);
