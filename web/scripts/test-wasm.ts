// ビルドしたページを vite preview で配信し、ヘッドレスの Chrome で開く。
// preview を使うのは、本番と同じくビルドしたものを COOP/COEP のヘッダーつきで配信して確かめるためである。
// スレッドつきの WASM が Worker で動き、正しい和を返すかを確かめる。
// 三重バッファから読んだ時計が実時間に合わせて進み、速度と一時停止のボタンに従うかも確かめる。
// Chrome は mise の tools で入れた chrome-headless-shell を PATH から探す。

import { accessSync, constants } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";
import { preview } from "vite";

const TIMEOUT_MS = 30_000;
const MIN_THREADS = 2;
const TICKS_PER_SECOND = 30;
const FAST_SPEED = 48;
// chizu-core の clock.rs と world.rs の値
const TICKS_PER_DAY = 7_200;
const GAME_SECONDS_PER_TICK = 12;
const INITIAL_MONEY = 500_000;

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
  await status.and(page.locator(":is([data-state=running], [data-state=error])")).waitFor({
    timeout: TIMEOUT_MS,
  });

  const state = await status.getAttribute("data-state");
  check(state === "running", `Worker が動き出した（${await status.textContent()}）`);

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

  const hud = page.locator("#hud");
  await hud.and(page.locator("[data-tick]")).waitFor({ timeout: TIMEOUT_MS });
  const tick = async () => Number(await hud.getAttribute("data-tick"));
  const ticksIn = async (ms: number) => {
    const before = await tick();
    await page.waitForTimeout(ms);
    return (await tick()) - before;
  };

  const normal = await ticksIn(2_000);
  check(
    normal >= TICKS_PER_SECOND && normal <= TICKS_PER_SECOND * 4,
    `等速で 2 秒待つと時計が 60 tick ほど進む（${normal} tick）`,
  );

  await page.locator(`button[data-speed="${FAST_SPEED}"]`).click();
  await hud.and(page.locator(`[data-speed="${FAST_SPEED}"]`)).waitFor({ timeout: TIMEOUT_MS });
  const fast = await ticksIn(2_000);
  check(fast >= normal * 10, `${FAST_SPEED} 倍にすると等速より速く進む（${fast} tick）`);

  await page.locator("#pause").click();
  await hud.and(page.locator("[data-paused=true]")).waitFor({ timeout: TIMEOUT_MS });
  const paused = await ticksIn(1_000);
  check(paused === 0, `一時停止すると時計が止まる（${paused} tick）`);

  // 一時停止のあいだは枠が変わらないので、1 枠の値をまとめて読める
  const shown = await hud.evaluate((el) => ({ ...(el as HTMLElement).dataset }));
  const shownTick = Number(shown.tick);
  const expectedClock = {
    day: Math.floor(shownTick / TICKS_PER_DAY),
    secondOfDay: (shownTick % TICKS_PER_DAY) * GAME_SECONDS_PER_TICK,
  };
  check(
    Number(shown.day) === expectedClock.day &&
      Number(shown.secondOfDay) === expectedClock.secondOfDay,
    `時計の日と時刻が tick ${shownTick} と合う（${shown.day} 日、${shown.secondOfDay} 秒）`,
  );
  check(Number(shown.money) === INITIAL_MONEY, `お金が初期値と合う（${shown.money}）`);
  check(Number(shown.speed) === FAST_SPEED, `速度が ${FAST_SPEED} 倍と読める（${shown.speed}）`);

  await page.locator("#pause").click();
  await hud.and(page.locator("[data-paused=false]")).waitFor({ timeout: TIMEOUT_MS });
  const resumed = await ticksIn(500);
  check(resumed > 0, `再開すると時計がまた進む（${resumed} tick）`);
} finally {
  await browser.close();
  await server?.close();
}

if (failures.length > 0) {
  console.error(`test:wasm は ${failures.length} 件失敗した`);
  process.exit(1);
}
