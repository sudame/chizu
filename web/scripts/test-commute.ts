// ビルドしたページを vite preview で配信し、ヘッドレスの Chrome で住民の通勤が描かれるかを確かめる。
// chizu-core のゴールデンの通勤の記録（tests/golden/commute.rec）を、2 日目の朝の通勤のさなかの tick までの
// 保存データとして IndexedDB に書き、読み込む。住民が自分で歩きと車で職場へ向かうので、
// 歩く人と車のそれぞれの位置に移動体の画素が描かれることを見る。
// `node scripts/test-commute.ts errand` では、買い物の記録（tests/golden/errand.rec）を 2 日目の正午まで読み込み、
// 昼にも歩く人か車のどちらかが描かれることを見る。
// Chrome は mise の tools で入れた chrome-headless-shell を PATH から探す。

import { execFileSync } from "node:child_process";
import { accessSync, constants, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";
import { preview } from "vite";
// 移動体の画素は、車か歩く人の色のどれかに近い。道路の色は、どの移動体の色からも離してある
import { moverColorAt } from "../src/map/mover-colors.ts";
import { zoomInTo } from "./ui.ts";

const TIMEOUT_MS = 30_000;
// chizu-core の clock.rs と world/people.rs の値
const TICKS_PER_DAY = 7_200;
const GAME_SECONDS_PER_TICK = 12;
const TICKS_PER_HOUR = TICKS_PER_DAY / 24;
// 工場で働く人が家を出る時刻の山の頂（Shift::of の Industrial）。通勤の記録の職場は工場だけである
const INDUSTRIAL_LEAVE_HOME = 7 * TICKS_PER_HOUR;
const SCENARIO = process.argv[2] ?? "commute";
// commute は 2 日目の 7 時 12 分。家を出る人が最も多い時間帯なので、歩く人も車もいる。
// errand は 2 日目の正午。買い物に出た人がいる
const NOON = TICKS_PER_DAY + TICKS_PER_DAY / 2;
const TICK = SCENARIO === "errand" ? NOON : TICKS_PER_DAY + INDUSTRIAL_LEAVE_HOME + 60;
// 描かれていなければならない移動体。errand ではどちらか一方でよい
const NEED_BOTH = SCENARIO !== "errand";
const WHEN = SCENARIO === "errand" ? "昼の買い物" : "朝の通勤";
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
const recPath = join(repo, `crates/chizu-core/tests/golden/${SCENARIO}.rec`);
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
    `${WHEN}の記録を読み込むと、chizu-cli と同じ状態ハッシュになる（${stateHash}）`,
  );

  // 歩く人と車は寄りの段でだけ描くので、濃さがそろう倍率まで寄る。描かれるまで待ち、止めてから画素を読む
  await zoomInTo(page, 16);
  await page.waitForFunction(
    (both) => {
      const d = document.querySelector<HTMLElement>("#map")?.dataset;
      const [walker, car] = [(d?.firstWalker ?? "") !== "", (d?.firstCar ?? "") !== ""];
      return both ? walker && car : walker || car;
    },
    NEED_BOTH,
    { timeout: TIMEOUT_MS },
  );
  await page.locator("#pause").click();
  await page
    .locator("#hud")
    .and(page.locator("[data-paused=true]"))
    .waitFor({ timeout: TIMEOUT_MS });
  await page.waitForTimeout(200);
  const map = page.locator("#map");
  const dataset = () => map.evaluate((el) => ({ ...(el as HTMLElement).dataset }));
  let d = await dataset();
  const size = page.viewportSize() ?? { width: 1280, height: 720 };
  const zoom = Number(d.zoom);
  const screenOf = (at: string) => {
    const [x, y] = at.split(",").map(Number);
    return {
      x: size.width / 2 + ((x ?? 0) - Number(d.cameraX)) * zoom,
      y: size.height / 2 + ((y ?? 0) - Number(d.cameraY)) * zoom,
    };
  };
  const onScreen = (p: { x: number; y: number }) =>
    p.x >= 10 && p.y >= 10 && p.x <= size.width - 10 && p.y <= size.height - 10;
  // 寄ると画面に入る範囲が狭いので、移動体が画面の外にいれば、中のボタンで地図を引きずって真ん中へ寄せる。
  // 引きずると Worker に頼む範囲が変わり、最初の移動体が入れ替わることがあるので、読み直してから確かめる
  const bringIntoView = async (key: "firstWalker" | "firstCar") => {
    for (let i = 0; i < 10; i++) {
      d = await dataset();
      const at = d[key] ?? "";
      if (at === "" || onScreen(screenOf(at))) {
        return at;
      }
      const p = screenOf(at);
      const limitX = size.width / 2 - 20;
      const limitY = size.height / 2 - 20;
      const dx = Math.max(-limitX, Math.min(limitX, size.width / 2 - p.x));
      const dy = Math.max(-limitY, Math.min(limitY, size.height / 2 - p.y));
      await page.mouse.move(size.width / 2, size.height / 2);
      await page.mouse.down({ button: "middle" });
      await page.mouse.move(size.width / 2 + dx, size.height / 2 + dy, { steps: 4 });
      await page.mouse.up({ button: "middle" });
      await page.waitForTimeout(300);
    }
    return (await dataset())[key] ?? "";
  };
  // 車が走っていれば、網を変えていなくても、車を描く道筋が画面に届いている
  if (NEED_BOTH) {
    check(
      Number(d.moverPaths) > 0,
      `${WHEN}で、網を変えなくても車を描く道筋が画面に届く（${d.moverPaths} 本）`,
    );
  }
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
  for (const [key, label] of [
    ["firstWalker", "歩く人"],
    ["firstCar", "車"],
  ] as const) {
    const at = await bringIntoView(key);
    if (!NEED_BOTH && at === "") {
      continue;
    }
    const c = await pixel(screenOf(at));
    check(
      moverColorAt(c),
      `${WHEN}で${label}の画素が描かれる（${at}、${c.join(", ")}、移動体 ${d.drawnMovers}）`,
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
