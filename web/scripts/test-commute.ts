// ビルドしたページを vite preview で配信し、ヘッドレスの Chrome で住民の通勤が描かれるかを確かめる。
// chizu-core のゴールデンの通勤の記録（tests/golden/commute.rec）を、2 日目の朝の通勤のさなかの tick までの
// 保存データとして IndexedDB に書き、読み込む。住民が自分で歩きと車で職場へ向かうので、
// 歩く人と車のそれぞれの位置に移動体の画素が描かれることを見る。
// Chrome は mise の tools で入れた chrome-headless-shell を PATH から探す。

import { execFileSync } from "node:child_process";
import { accessSync, constants, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";
import { preview } from "vite";

const TIMEOUT_MS = 30_000;
// chizu-core の clock.rs と world/people.rs の値
const TICKS_PER_DAY = 7_200;
const GAME_SECONDS_PER_TICK = 12;
const WORK_START = TICKS_PER_DAY / 3;
// 2 日目の 8 時 12 分。出発のばらつきの 1 時間の中なので、歩く人も車もいる
const TICK = TICKS_PER_DAY + WORK_START + 60;
const SEED = "42";
// web/src/map/movers.ts の色を足した明るさ（歩く人は約 678、車は約 655）より少し下
const MOVER_BRIGHTNESS = 560;

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
const recPath = join(repo, "crates/chizu-core/tests/golden/commute.rec");
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
  const waitFor = (fn: () => boolean) =>
    page.waitForFunction(fn, undefined, { timeout: TIMEOUT_MS });
  const running = () =>
    page
      .locator("#status")
      .and(page.locator(":is([data-state=running], [data-state=error])"))
      .waitFor({ timeout: TIMEOUT_MS });

  // 一度開いて保存先を作らせてから、通勤の記録を保存 1 に書く
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
  await start.locator('button[data-load="manual-1"]').click();
  await running();
  const saveButton = page.locator("#save-open");
  await waitFor(
    () => document.querySelector<HTMLElement>("#save-open")?.dataset.loadedHash !== undefined,
  );
  check(
    (await saveButton.getAttribute("data-rules-changed")) === "false",
    `通勤の記録を朝まで読み込むと、chizu-cli と同じ状態ハッシュになる（${stateHash}）`,
  );

  // 歩く人と車がどちらも描かれるまで待ち、止めてから画素を読む
  await waitFor(() => {
    const d = document.querySelector<HTMLElement>("#map")?.dataset;
    return (d?.firstWalker ?? "") !== "" && (d?.firstCar ?? "") !== "";
  });
  await page.locator("#pause").click();
  await page
    .locator("#hud")
    .and(page.locator("[data-paused=true]"))
    .waitFor({ timeout: TIMEOUT_MS });
  await page.waitForTimeout(200);
  const map = page.locator("#map");
  const d = await map.evaluate((el) => ({ ...(el as HTMLElement).dataset }));
  const size = page.viewportSize() ?? { width: 1280, height: 720 };
  const zoom = Number(d.zoom);
  const screenOf = (at: string) => {
    const [x, y] = at.split(",").map(Number);
    return {
      x: size.width / 2 + ((x ?? 0) - Number(d.cameraX)) * zoom,
      y: size.height / 2 + ((y ?? 0) - Number(d.cameraY)) * zoom,
    };
  };
  const pixel = (p: { x: number; y: number }) =>
    map.evaluate((el, q) => {
      const canvas = el as HTMLCanvasElement;
      const gl = canvas.getContext("webgl2");
      const out = new Uint8Array(4);
      const dpr = window.devicePixelRatio;
      const [x, y] = [Math.floor(q.x * dpr), canvas.height - 1 - Math.floor(q.y * dpr)];
      gl?.readPixels(x, y, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, out);
      return [...out];
    }, p);
  const brightness = (c: number[]) => (c[0] ?? 0) + (c[1] ?? 0) + (c[2] ?? 0);
  for (const [key, label] of [
    ["firstWalker", "歩く人"],
    ["firstCar", "車"],
  ] as const) {
    const at = d[key] ?? "";
    const c = await pixel(screenOf(at));
    check(
      brightness(c) >= MOVER_BRIGHTNESS,
      `朝の通勤で${label}の画素が描かれる（${at}、${c.join(", ")}、移動体 ${d.drawnMovers}）`,
    );
  }
} finally {
  await browser.close();
  await server.close();
}

if (failures.length > 0) {
  console.error(`test-commute は ${failures.length} 件失敗した`);
  process.exit(1);
}
