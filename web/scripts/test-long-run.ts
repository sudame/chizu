// ビルドしたページを vite preview で配信し、ヘッドレスの Chrome で、何日も回した街が WASM とネイティブで一致するかを確かめる。
// chizu-core のゴールデンの記録（tests/golden/long_run.rec）を、7 日目の始まりまでの保存データとして IndexedDB に書いて読み込む。
// 読み込むと Worker が記録を種から再生し直すので、画面の確かめを挟まずに、Worker だけを最高速で回したことになる。
// 再生した街の状態ハッシュが、chizu-cli で同じ記録を再生したものと一致するかを見る。
// この記録には、通勤、日をまたぐ処理、建物の段階の上がり、家のない世帯が出ていく処理、電気と水道、取り壊しが入る。
// Chrome は mise の tools で入れた chrome-headless-shell を PATH から探す。

import { execFileSync } from "node:child_process";
import { accessSync, constants, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";
import { preview } from "vite";

// 読み込みは街を 6 日ぶん再生するので、ほかの確かめより長く待つ
const TIMEOUT_MS = 120_000;
// chizu-core の clock.rs と golden.rs の値
const TICKS_PER_DAY = 7_200;
const GAME_SECONDS_PER_TICK = 12;
const TICK = 6 * TICKS_PER_DAY;
const SEED = "42";

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

const repo = fileURLToPath(new URL("../..", import.meta.url));
const recPath = join(repo, "crates/chizu-core/tests/golden/long_run.rec");
const rec = readFileSync(recPath);
// 記録の先頭は b"CHIZUREC"、版 u32、件数 u64 である（crates/chizu-core/src/record.rs）
const formatVersion = rec.readUInt32LE(8);
const commandCount = Number(rec.readBigUInt64LE(12));
const stateHash = execFileSync(
  "cargo",
  ["run", "--quiet", "-p", "chizu-cli", "--", recPath, SEED, String(TICK)],
  {
    cwd: repo,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "inherit"],
  },
).trim();

const server = await preview({
  root: fileURLToPath(new URL("..", import.meta.url)),
  logLevel: "warn",
  preview: { host: "127.0.0.1", port: 0 },
});
const url = server.resolvedUrls?.local[0];
if (url === undefined) {
  throw new Error("preview の URL が分からない");
}
const browser = await chromium.launch({ executablePath: findOnPath("chrome-headless-shell") });

try {
  const page = await browser.newPage();
  page.on("pageerror", (error) => console.log(`[pageerror] ${error.message}`));
  const running = () =>
    page
      .locator("#status")
      .and(page.locator(":is([data-state=running], [data-state=error])"))
      .waitFor({ timeout: TIMEOUT_MS });

  // 一度開いて保存先を作らせてから、記録を保存 1 に書く
  await page.goto(url);
  await running();
  await page.evaluate(
    ({ meta, bytes }) =>
      new Promise<void>((resolve, reject) => {
        const open = indexedDB.open("chizu");
        open.onsuccess = () => {
          const tx = open.result.transaction("saves", "readwrite");
          const commands = new Uint8Array(bytes).buffer;
          tx.objectStore("saves").put({ slot: "manual-1", savedAt: Date.now(), meta, commands });
          tx.oncomplete = () => resolve();
          tx.onerror = () => reject(tx.error);
        };
        open.onerror = () => reject(open.error);
      }),
    {
      meta: {
        formatVersion,
        seed: SEED,
        tick: TICK,
        commandCount,
        day: Math.floor(TICK / TICKS_PER_DAY),
        secondOfDay: (TICK % TICKS_PER_DAY) * GAME_SECONDS_PER_TICK,
        money: "0",
        stateHash,
      },
      bytes: [...rec],
    },
  );
  await page.reload();
  const start = page.locator("#save-start");
  await start.and(page.locator("[open]")).waitFor({ timeout: TIMEOUT_MS });
  const startedAt = performance.now();
  await start.locator('button[data-load="manual-1"]').click();
  await running();
  const saveButton = page.locator("#save-open");
  await page.waitForFunction(
    () => document.querySelector<HTMLElement>("#save-open")?.dataset.loadedHash !== undefined,
    undefined,
    { timeout: TIMEOUT_MS },
  );
  const seconds = ((performance.now() - startedAt) / 1000).toFixed(1);
  const loaded = await saveButton.getAttribute("data-loaded-hash");
  check(
    (await saveButton.getAttribute("data-from-state")) === "false" &&
      loaded === stateHash &&
      (await saveButton.getAttribute("data-rules-changed")) === "false",
    `${TICK} tick（6 日）の記録を Worker で再生すると、chizu-cli と同じ状態ハッシュになる（${loaded} と ${stateHash}、再生 ${seconds} 秒）`,
  );
} finally {
  await browser.close();
  await server.close();
}

if (failures.length > 0) {
  console.error(`test-long-run は ${failures.length} 件失敗した`);
  process.exit(1);
}
