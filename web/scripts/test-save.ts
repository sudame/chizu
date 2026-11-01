// ビルドしたページを vite preview で配信し、ヘッドレスの Chrome で保存と読み込みを確かめる。
// 道路と区画を作って手動で保存し、ページを読み直してから読み込むと、同じ状態ハッシュと所持金になるか。
// ゲーム内の日が変わると自動保存されるか。記録の形式の版が違う保存データは読めないと示されるか。
// Chrome は mise の tools で入れた chrome-headless-shell を PATH から探す。

import { accessSync, constants } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";
import { preview } from "vite";

const TIMEOUT_MS = 30_000;
const FAST_SPEED = 48;

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
  page.on("console", (message) => console.log(`[console.${message.type()}] ${message.text()}`));
  page.on("pageerror", (error) => console.log(`[pageerror] ${error.message}`));
  const waitFor = (fn: () => boolean) =>
    page.waitForFunction(fn, undefined, { timeout: TIMEOUT_MS });
  const running = () =>
    page
      .locator("#status")
      .and(page.locator(":is([data-state=running], [data-state=error])"))
      .waitFor({ timeout: TIMEOUT_MS });
  const saveButton = page.locator("#save-open");
  const hud = page.locator("#hud");

  await page.goto(url);
  await running();
  check(
    (await page.locator("#status").getAttribute("data-state")) === "running" &&
      !(await page.locator("#save-start").evaluate((el) => (el as HTMLDialogElement).open)),
    "保存データがないときは、選ぶ画面を出さずに新しく始める",
  );

  // 道路を引き、その上側に住宅の区画を塗る。カメラはマス (32, 32) を中心に 1 マス 4 ピクセルで描く
  const size = page.viewportSize() ?? { width: 1280, height: 720 };
  const tile = (x: number, y: number) => ({
    x: size.width / 2 + (x + 0.5 - 32) * 4,
    y: size.height / 2 + (y + 0.5 - 32) * 4,
  });
  await page.locator('button[data-tool="road-Small"]').click();
  await page.mouse.click(tile(12, 32).x, tile(12, 32).y);
  await page.mouse.click(tile(52, 32).x, tile(52, 32).y);
  await page.keyboard.press("Escape");
  await waitFor(() => document.querySelector<HTMLElement>("#tool-message")?.dataset.count === "1");
  await page.locator('button[data-tool="zone-Residential"]').click();
  const r0 = tile(14, 26);
  const r1 = tile(30, 31);
  await page.mouse.move(r0.x, r0.y);
  await page.mouse.down();
  await page.mouse.move(r1.x, r1.y, { steps: 4 });
  await page.mouse.up();
  await waitFor(() => document.querySelector<HTMLElement>("#tool-message")?.dataset.count === "2");
  check(
    (await page.locator("#tool-message").getAttribute("data-outcome")) === "Applied",
    "道路を引き、区画を塗れる",
  );

  // 速くして日をまたがせ、自動保存と建物が建つのを待つ
  await page.locator(`button[data-speed="${FAST_SPEED}"]`).click();
  await waitFor(
    () => Number(document.querySelector<HTMLElement>("#save-open")?.dataset.autoSaves ?? 0) > 0,
  );
  check(
    (await saveButton.getAttribute("data-last-slot")) === "auto",
    `ゲーム内の日が変わると自動保存される（${await saveButton.getAttribute("data-auto-saves")} 回）`,
  );
  await waitFor(
    () => Number(document.querySelector<HTMLElement>("#city-numbers")?.dataset.buildings ?? 0) > 0,
  );

  // 止めてから保存 1 に保存する
  await page.locator("#pause").click();
  await hud.and(page.locator("[data-paused=true]")).waitFor({ timeout: TIMEOUT_MS });
  await saveButton.click();
  await page.locator('#save-dialog button[data-save="manual-1"]').click();
  await waitFor(
    () => document.querySelector<HTMLElement>("#save-open")?.dataset.lastSlot === "manual-1",
  );
  const savedHash = await saveButton.getAttribute("data-last-hash");
  const savedMoney = await saveButton.getAttribute("data-last-money");
  const savedTick = Number(await hud.getAttribute("data-tick"));
  check(
    savedMoney === (await hud.getAttribute("data-money")),
    `保存した所持金が画面の所持金と同じ（${savedMoney}）`,
  );
  await page.locator("#save-dialog button", { hasText: "閉じる" }).click();

  // 記録の形式の版が違う保存データを、保存 2 に書いておく
  await page.evaluate(
    () =>
      new Promise<void>((resolve, reject) => {
        const open = indexedDB.open("chizu");
        open.onsuccess = () => {
          const tx = open.result.transaction("saves", "readwrite");
          tx.objectStore("saves").put({
            slot: "manual-2",
            savedAt: Date.now(),
            meta: {
              formatVersion: 999,
              seed: "0",
              tick: 0,
              commandCount: 0,
              day: 0,
              secondOfDay: 0,
              money: "0",
              stateHash: "0",
            },
            commands: new ArrayBuffer(0),
          });
          tx.oncomplete = () => resolve();
          tx.onerror = () => reject(tx.error);
        };
        open.onerror = () => reject(open.error);
      }),
  );

  await page.reload();
  const start = page.locator("#save-start");
  await start.and(page.locator("[open]")).waitFor({ timeout: TIMEOUT_MS });
  const enabled = async (slot: string) =>
    start.locator(`button[data-load="${slot}"]`).isEnabled({ timeout: TIMEOUT_MS });
  check(
    (await enabled("auto")) && (await enabled("manual-1")),
    "読み直すと、自動保存と保存 1 を続きから始められる",
  );
  check(
    !(await enabled("manual-2")) && ((await start.textContent()) ?? "").includes("読めない"),
    "記録の形式の版が違う保存データは、読めないと示される",
  );
  check(((await start.textContent()) ?? "").includes("日目"), "枠ごとにゲーム内の日付が並ぶ");

  await start.locator('button[data-load="manual-1"]').click();
  await running();
  await waitFor(
    () => document.querySelector<HTMLElement>("#save-open")?.dataset.loadedHash !== undefined,
  );
  const loadedHash = await saveButton.getAttribute("data-loaded-hash");
  const loadedMoney = await saveButton.getAttribute("data-loaded-money");
  check(
    loadedHash === savedHash && loadedMoney === savedMoney,
    `読み込むと同じ状態ハッシュと所持金になる（${loadedHash} と ${savedHash}、${loadedMoney} と ${savedMoney}）`,
  );
  const reports = Number(await page.locator("#save-progress").getAttribute("data-reports"));
  check(reports >= 2, `読み込みのあいだ進み具合が届く（${reports} 回）`);
  await hud.and(page.locator("[data-tick]")).waitFor({ timeout: TIMEOUT_MS });
  const tickAfter = Number(await hud.getAttribute("data-tick"));
  check(tickAfter >= savedTick, `読み込んだ tick から続く（${savedTick} → ${tickAfter}）`);
  check(
    (await saveButton.getAttribute("data-rules-changed")) === "false" &&
      ((await page.locator("#save-notice").textContent()) ?? "").includes("読み込んだ"),
    "読み込んだと示され、規則が変わったとは示されない",
  );

  // 状態ハッシュだけを変えた保存 1 の写しを保存 3 に書き、規則が変わったときの表示を確かめる
  await page.evaluate(
    () =>
      new Promise<void>((resolve, reject) => {
        const open = indexedDB.open("chizu");
        open.onsuccess = () => {
          const store = open.result.transaction("saves", "readwrite").objectStore("saves");
          const get = store.get("manual-1");
          get.onsuccess = () => {
            const copy = get.result;
            copy.slot = "manual-3";
            copy.meta.stateHash = "0000000000000000";
            const put = store.put(copy);
            put.onsuccess = () => resolve();
            put.onerror = () => reject(put.error);
          };
        };
        open.onerror = () => reject(open.error);
      }),
  );
  await page.reload();
  await start.and(page.locator("[open]")).waitFor({ timeout: TIMEOUT_MS });
  await start.locator('button[data-load="manual-3"]').click();
  await running();
  await waitFor(
    () => document.querySelector<HTMLElement>("#save-open")?.dataset.rulesChanged !== undefined,
  );
  check(
    (await saveButton.getAttribute("data-rules-changed")) === "true" &&
      (await saveButton.getAttribute("data-loaded-hash")) === savedHash &&
      ((await page.locator("#save-notice").textContent()) ?? "").includes("規則が変わった"),
    "状態ハッシュが食い違っても読み込み、規則が変わったと示す",
  );
} finally {
  await browser.close();
  await server.close();
}

if (failures.length > 0) {
  console.error(`test-save は ${failures.length} 件失敗した`);
  process.exit(1);
}
