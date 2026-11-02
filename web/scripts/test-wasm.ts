// ビルドしたページを vite preview で配信し、ヘッドレスの Chrome で開く。
// preview を使うのは、本番と同じくビルドしたものを COOP/COEP のヘッダーつきで配信して確かめるためである。
// スレッドつきの WASM が Worker で動き、正しい和を返すかを確かめる。
// 三重バッファから読んだ時計が実時間に合わせて進み、速度と一時停止のボタンに従うかも確かめる。
// チャンクが Worker から届いて描かれること、ドラッグとホイールでカメラが動くことも確かめる。
// 道路の道具で道路を引くと描かれて所持金が減ること、断られると理由が出ること、ブルドーザで消せることも確かめる。
// 電気のデータレイヤーで、届いていない建物と届いている建物が色分けされることも確かめる。
// フィードバックを送ると、保存した記録を chizu-cli で再生して同じ状態ハッシュになることも確かめる。
// Chrome は mise の tools で入れた chrome-headless-shell を PATH から探す。

import { execFileSync } from "node:child_process";
import { accessSync, constants, existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium, type Page } from "playwright-core";
import { preview } from "vite";
import { stageWeights } from "../src/map/stages.ts";
import { chooseLayer, pickSpeed, pickTool, zoomInTo } from "./ui.ts";

const TIMEOUT_MS = 30_000;
const MIN_THREADS = 2;
const TICKS_PER_SECOND = 30;
const FAST_SPEED = 48;
// chizu-core の clock.rs と world.rs の値
const TICKS_PER_DAY = 7_200;
const GAME_SECONDS_PER_TICK = 12;
const INITIAL_MONEY = 500_000;
// web/src/map/camera.ts の値
// 買った土地の地面の色（docs/map-style.md の色の表）
const GROUND_HEX = "#f4f2ec";

/** 色の表の 2 つの値（`#rrggbb`）を、b を t の割合で重ねた色にする。 */
function mixHex(a: string, b: string, t: number): string {
  const ch = (code: string, at: number) => Number.parseInt(code.slice(at, at + 2), 16);
  return `#${[1, 3, 5]
    .map((at) =>
      Math.round(ch(a, at) * (1 - t) + ch(b, at) * t)
        .toString(16)
        .padStart(2, "0"),
    )
    .join("")}`;
}
// 地面に買える土地の仮の表示（#1a73e8 の 10%）が重なった色
const GROUND_ON_HINT_HEX = "#dee5ec";
const MIN_ZOOM = 1;
const MAX_ZOOM = 32;

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
// フィードバックは一時ディレクトリに書かせる。preview はこの値を読んでから立ち上がる
const feedbackDir = mkdtempSync(join(tmpdir(), "chizu-feedback-"));
process.env.CHIZU_FEEDBACK_DIR = feedbackDir;
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
// 上の帯：日時、お金と収支、人口、速度はたたんでも見え、たためる欄は閉じて始まり、開閉が覚えられる。
// 収支の欄はお金を押して開く。幅 1280 と 500 で、帯と欄が地図の広い範囲を覆わないことも見る
async function checkHudPanel(page: Page, at: string): Promise<void> {
  await page.goto(at);
  await page.locator("#status[data-state=running]").waitFor({ timeout: TIMEOUT_MS });
  const hud = page.locator("#hud");
  await hud.and(page.locator("[data-tick]")).waitFor({ timeout: TIMEOUT_MS });
  const details = page.locator("#hud-details");
  const toggle = page.locator("#hud-toggle");
  const finance = page.locator("#finance");
  const financeToggle = page.locator("#finance-toggle");
  check(
    (await hud.isVisible()) &&
      (await page.locator("#pause").isVisible()) &&
      (await page.locator("#hud-population").isVisible()) &&
      !(await details.isVisible()) &&
      !(await finance.isVisible()),
    "日時、お金、人口、速度の帯は見え、たためる欄と収支の欄は閉じて始まる",
  );
  const speedLabels = await page.locator("#speeds button[data-speed]").allTextContents();
  check(
    speedLabels.join(",") === "1 倍,4 倍,16 倍" &&
      (await page.locator('#speeds button[data-speed="1"]').getAttribute("aria-pressed")) ===
        "true",
    `帯の速度は 1、4、16 倍で、いまの速度が押した状態で出る（${speedLabels.join(",")}）`,
  );
  const covered = async () =>
    page.evaluate(() => {
      const r = document.querySelector("#overlay")?.getBoundingClientRect();
      return r === undefined ? 0 : r.width * r.height;
    });
  const closedArea = await covered();
  await toggle.click();
  check(
    (await details.isVisible()) &&
      (await page.locator("#city-numbers").isVisible()) &&
      (await page.locator("#demand").isVisible()) &&
      (await page.locator("#overview").isVisible()) &&
      (await page.locator('#controls button[data-speed="48"]').isVisible()) &&
      (await page.locator("#save-open").isVisible()),
    "「⋯」を押すと、建物、電気と水道、需要、人口と幸福度、ほかの速度と保存が出る",
  );
  await page.reload();
  await page.locator("#status[data-state=running]").waitFor({ timeout: TIMEOUT_MS });
  check(await details.isVisible(), "開いたことが覚えられ、読み直しても開いている");
  await toggle.click();
  await page.reload();
  await page.locator("#status[data-state=running]").waitFor({ timeout: TIMEOUT_MS });
  check(!(await details.isVisible()), "閉じたことが覚えられ、読み直しても閉じている");
  const size = page.viewportSize() ?? { width: 1280, height: 720 };
  const share = closedArea / (size.width * size.height);
  check(share < 0.06, `たたんだ帯が覆う広さは画面の 6% に満たない（${(share * 100).toFixed(1)}%）`);

  // お金を押すと収支の欄が開き、もう一度押すか、欄の外を押すか、Esc で閉じる
  await financeToggle.click();
  check(
    (await finance.isVisible()) &&
      (await financeToggle.getAttribute("aria-expanded")) === "true" &&
      (await page.locator("#finance .finance-net").isVisible()),
    "お金を押すと収支の欄が開く",
  );
  await financeToggle.click();
  check(!(await finance.isVisible()), "お金をもう一度押すと収支の欄が閉じる");
  await financeToggle.click();
  await page.mouse.click(size.width / 2, size.height / 2);
  check(!(await finance.isVisible()), "欄の外（地図）を押すと収支の欄が閉じる");
  await financeToggle.click();
  await page.keyboard.press("Escape");
  check(!(await finance.isVisible()), "Esc で収支の欄が閉じる");

  // どの幅でも、帯と地図の部品のボタンは覆われず、帯と欄、知らせ、右上と四隅の部品が重ならない。
  // 欄は、詳細を開いたときと、収支を開いたときに分けて見る
  const showInspector = (visible: boolean) =>
    page.evaluate((on) => {
      const el = document.querySelector<HTMLElement>("#inspector");
      if (el === null) {
        return;
      }
      el.hidden = !on;
      const title = el.querySelector("[data-inspector-title]");
      const rows = el.querySelector("[data-inspector-rows]");
      if (on && title !== null && rows !== null) {
        title.textContent = "確かめ用の建物";
        rows.innerHTML = "<dt>人口</dt><dd>4</dd><dt>電気</dt><dd>届いている</dd>";
      }
    }, visible);
  const roadToggle = page.locator('button[data-menu="road"]');
  const zoneToggle = page.locator('button[data-menu="zone"]');
  const layerToggle = page.locator("#layers-open");
  const states: { open: () => Promise<void>; close: () => Promise<void> }[] = [
    { open: () => toggle.click(), close: () => toggle.click() },
    { open: () => financeToggle.click(), close: () => financeToggle.click() },
    // 道具の 2 段目とレイヤーのパネルは、開いているあいだ隅の部品と重ならない
    { open: () => roadToggle.click(), close: () => page.keyboard.press("Escape") },
    { open: () => zoneToggle.click(), close: () => page.keyboard.press("Escape") },
    // 信号の 2 段目には、横断歩道を外す、戻す道具も入るので、いちばん幅が広い
    {
      open: () => page.locator('button[data-menu="signal"]').click(),
      close: () => page.keyboard.press("Escape"),
    },
    { open: () => layerToggle.click(), close: () => page.keyboard.press("Escape") },
    // 建物の情報は、詳細や収支を開いたときも含めて、ほかの欄と重ならない
    { open: () => showInspector(true), close: () => showInspector(false) },
    {
      open: async () => {
        await toggle.click();
        await showInspector(true);
      },
      close: async () => {
        await showInspector(false);
        await toggle.click();
      },
    },
    {
      open: async () => {
        await financeToggle.click();
        await showInspector(true);
      },
      close: async () => {
        await showInspector(false);
        await financeToggle.click();
      },
    },
  ];
  for (const [index, { open, close }] of states.entries()) {
    await open();
    for (const width of [1280, 1000, 900, 860, 780, 640, 500, 390]) {
      await page.setViewportSize({ width, height: 800 });
      await page.waitForTimeout(300);
      const layout = await page.evaluate(() => {
        const notice = document.querySelector<HTMLElement>("#save-notice");
        if (notice !== null) {
          notice.textContent = "確かめ用の知らせ";
          notice.hidden = false;
        }
        const box = (sel: string) => {
          const el = document.querySelector(sel);
          const r = el?.getBoundingClientRect();
          // hidden の部品は大きさが 0 なので、重なりの確かめから外す
          return r === undefined ||
            r.width === 0 ||
            r.height === 0 ||
            (el !== null && getComputedStyle(el).visibility === "hidden")
            ? null
            : r;
        };
        const covered = [
          ...document.querySelectorAll<HTMLElement>(
            "#hudbar button, #toolbar button, #map-controls button, #feedback-open, .tool-menu button, #layer-panel button",
          ),
        ]
          .filter((b) => {
            const r = b.getBoundingClientRect();
            // 隠した部品は、押せない代わりに覆いも起こさない
            if (r.width === 0 || getComputedStyle(b).visibility === "hidden") {
              return false;
            }
            const hit = document.elementFromPoint(r.x + r.width / 2, r.y + r.height / 2);
            return hit !== b && !b.contains(hit);
          })
          .map((b) => b.id || b.dataset.tool || b.dataset.menu || b.textContent);
        const overlaps = (a: DOMRect, b: DOMRect) =>
          a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom;
        const parts = [
          "#hudbar",
          "#hud-details",
          "#finance",
          "#inspector",
          "#layer-panel",
          "#notices",
          "#feedback-open",
          "#toolstack",
          "#map-controls",
          "#map-meta",
        ];
        const pairs: string[] = [];
        for (const [i, x] of parts.entries()) {
          for (const y of parts.slice(i + 1)) {
            const rx = box(x);
            const ry = box(y);
            if (rx !== null && ry !== null && overlaps(rx, ry)) {
              pairs.push(`${x}と${y}`);
            }
          }
        }
        const bar = box("#hudbar");
        return { covered, pairs, right: bar?.right ?? 0, bottom: box("#toolstack")?.bottom ?? 0 };
      });
      check(
        layout.covered.length === 0 && layout.pairs.length === 0,
        `幅 ${width}（状態 ${index}）で、帯と部品のボタンは覆われず、部品どうしが重ならない（覆われた ${layout.covered.join(",")}、重なり ${layout.pairs.join(",")}）`,
      );
      check(layout.right <= width, `幅 ${width} で帯は画面に収まる（右 ${layout.right}）`);
      check(layout.bottom <= 800, `幅 ${width} で道具の帯は画面に収まる（下 ${layout.bottom}）`);
    }
    await page.setViewportSize({ width: 1280, height: 720 });
    await close();
  }
  await page.close();
}

// 開発用の道具で歩く人を出し、描かれること、時間を進めると動くこと、着くと消えることを確かめる。
// 起動したばかりのページで、開始地点の真ん中の行に道路を引き、その左寄りと右寄りのマスのあいだに出す
async function checkDevWalks(page: Page, at: string): Promise<void> {
  await page.goto(at);
  const map = page.locator("#map");
  const hud = page.locator("#hud");
  await page.locator("#status[data-state=running]").waitFor({ timeout: TIMEOUT_MS });
  await hud.and(page.locator("[data-tick]")).waitFor({ timeout: TIMEOUT_MS });
  const waitMap = (key: string, expected: string, op: "eq" | "gt" | "ne") =>
    page.waitForFunction(
      ([k, v, o]) => {
        const d = document.querySelector<HTMLElement>("#map")?.dataset[k] ?? "";
        return o === "eq" ? d === v : o === "gt" ? Number(d) > Number(v) : d !== v;
      },
      [key, expected, op] as const,
      { timeout: TIMEOUT_MS },
    );
  const setSpeed = async (speed: number) => {
    await pickSpeed(page, speed);
    await hud.and(page.locator(`[data-speed="${speed}"]`)).waitFor({ timeout: TIMEOUT_MS });
  };
  const pause = async (paused: boolean) => {
    await page.locator("#pause").click();
    await hud.and(page.locator(`[data-paused=${paused}]`)).waitFor({ timeout: TIMEOUT_MS });
  };
  // 外部接続の高速道路が届いてから、引く前の区間の数を読む
  await waitMap("roadSegments", "0", "gt");
  // 歩く人は寄りの段でだけ描くので、濃さがそろう倍率まで寄ってから引く
  await zoomInTo(page, 16);
  const size = page.viewportSize() ?? { width: 1280, height: 720 };
  const d0 = await map.evaluate((el) => ({ ...(el as HTMLElement).dataset }));
  const zoom = Number(d0.zoom);
  const tileCenter = (x: number, y: number) => ({
    x: size.width / 2 + (x + 0.5 - Number(d0.cameraX)) * zoom,
    y: size.height / 2 + (y + 0.5 - Number(d0.cameraY)) * zoom,
  });
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

  await setSpeed(1);
  await pause(true);
  const segments = d0.roadSegments ?? "0";
  await pickTool(page, "road-Small");
  const [from, to] = [tileCenter(12, 32), tileCenter(52, 32)];
  await page.mouse.click(from.x, from.y);
  await page.mouse.click(to.x, to.y);
  await page.keyboard.press("Escape");
  await waitMap("roadSegments", segments, "gt");
  await page.waitForTimeout(100);
  const walkFrom = tileCenter(14, 32);
  const walkTo = tileCenter(50, 32);
  // 歩く人は歩道の上にいる。小さい道路の歩道は中心線から 0.26 マス、東へ歩く人の右（画面の下）にある
  const sidewalk = { x: walkFrom.x, y: walkFrom.y + 0.26 * zoom };
  const roadColor = await pixel(sidewalk);
  await page.locator('button[data-tool="dev-walks"]').click();
  await page.mouse.click(walkFrom.x, walkFrom.y);
  await page.mouse.click(walkTo.x, walkTo.y);
  await waitMap("devWalks", "0", "gt");
  await waitMap("drawnMovers", "0", "gt");
  const spawned = await map.evaluate((el) => ({ ...(el as HTMLElement).dataset }));
  const walkerColor = await pixel(sidewalk);
  // 歩く人は明るい歩道の上で赤紫の丸に描くので、明るさではなく色の違いで見る
  const difference = walkerColor.reduce((sum, v, i) => sum + Math.abs(v - (roadColor[i] ?? 0)), 0);
  check(
    difference > 150,
    `歩く人を出すと移動体の点が描かれる（${spawned.drawnMovers} 人、${roadColor.join(", ")} → ${walkerColor.join(", ")}）`,
  );
  await pause(false);
  await waitMap("firstMover", spawned.firstMover ?? "", "ne");
  const moved = await map.getAttribute("data-first-mover");
  check(true, `時間を進めると移動体の位置が変わる（${spawned.firstMover} → ${moved}）`);
  // 補間そのものは Vitest（mover-interpolation.test.ts）で確かめる。ここでは、画面に補間が効いていることだけを見る。
  // tick が変わらない続きの 2 枠のあいだで位置が変わっていれば、tick に頼らず補間で動いている。
  // 枠の回数や間隔には頼らず、見つかるまで待つ。寄りの段は描画が重く、CI では 1 枠のあいだに tick が
  // 進みきって同じ tick の枠が続かないことがあるので、0.25 倍に落として 1 tick を長くしてから見る
  await setSpeed(0.25);
  const interpolated = await page.evaluate(
    (timeoutMs) =>
      new Promise<boolean>((resolve) => {
        const end = performance.now() + timeoutMs;
        let last: { tick: string; position: string } | null = null;
        const step = () => {
          const tick = document.querySelector<HTMLElement>("#hud")?.dataset.tick ?? "";
          const position = document.querySelector<HTMLElement>("#map")?.dataset.firstMover ?? "";
          if (last !== null && last.tick === tick && last.position !== position) {
            resolve(true);
            return;
          }
          last = { tick, position };
          if (performance.now() < end) {
            requestAnimationFrame(step);
          } else {
            resolve(false);
          }
        };
        requestAnimationFrame(step);
      }),
    TIMEOUT_MS,
  );
  check(
    interpolated,
    "移動体は 1 tick のあいだにも少しずつ動く（tick が同じ枠の間で位置が変わる）",
  );
  check(
    await page.locator("#dev-flag").isVisible(),
    "開発用の道具を使うと、再現できないことの印が出る",
  );
  // 48 倍で着くまで進めたあと、等速に戻しても描かれないことで、送られなくなったのではなく着いて消えたと分かる
  await setSpeed(48);
  await waitMap("drawnMovers", "0", "eq");
  await setSpeed(1);
  const tickAtNormal = Number(await hud.getAttribute("data-tick"));
  await page.waitForFunction(
    (t) => Number(document.querySelector<HTMLElement>("#hud")?.dataset.tick) > t + 10,
    tickAtNormal,
    { timeout: TIMEOUT_MS },
  );
  check(
    (await map.getAttribute("data-drawn-movers")) === "0",
    "行き先に着いた移動体は消え、等速に戻しても描かれない",
  );
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
  check(
    (await page.locator("#layers").getAttribute("data-active")) === "",
    "レイヤーは初めに「なし」を選んでいる",
  );
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

  await pickSpeed(page, 0.5);
  await hud.and(page.locator('[data-speed="0.5"]')).waitFor({ timeout: TIMEOUT_MS });
  const half = await ticksIn(2_000);
  check(
    half >= normal * 0.3 && half <= normal * 0.7,
    `0.5 倍にすると等速の半分ほど進む（${half} tick、等速は ${normal} tick）`,
  );

  await pickSpeed(page, 0.25);
  await hud.and(page.locator('[data-speed="0.25"]')).waitFor({ timeout: TIMEOUT_MS });
  const quarter = await ticksIn(2_000);
  check(
    quarter >= normal * 0.15 && quarter <= normal * 0.35,
    `0.25 倍にすると等速の 4 分の 1 ほど進む（${quarter} tick、等速は ${normal} tick）`,
  );

  await pickSpeed(page, FAST_SPEED);
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

  const map = page.locator("#map");
  const mapState = async () => {
    const d = await map.evaluate((el) => ({ ...(el as HTMLElement).dataset }));
    return {
      drawn: Number(d.drawnChunks),
      visible: Number(d.visibleChunks),
      x: Number(d.cameraX),
      y: Number(d.cameraY),
      zoom: Number(d.zoom),
    };
  };
  // 見えているチャンクがすべて届いて描かれるまで待つ
  const waitAllDrawn = () =>
    page.waitForFunction(
      () => {
        const d = document.querySelector<HTMLElement>("#map")?.dataset;
        return d !== undefined && Number(d.drawnChunks) > 0 && d.drawnChunks === d.visibleChunks;
      },
      undefined,
      { timeout: TIMEOUT_MS },
    );
  // 画面の点 (x, y)（CSS ピクセル）の色。描画の結果は preserveDrawingBuffer で残してある
  const pixel = (x: number, y: number) =>
    map.evaluate(
      (el, at) => {
        const canvas = el as HTMLCanvasElement;
        const gl = canvas.getContext("webgl2");
        const out = new Uint8Array(4);
        const dpr = window.devicePixelRatio;
        gl?.readPixels(
          Math.floor(at.x * dpr),
          canvas.height - 1 - Math.floor(at.y * dpr),
          1,
          1,
          gl.RGBA,
          gl.UNSIGNED_BYTE,
          out,
        );
        return [...out];
      },
      { x, y },
    );
  // 画素の色（0〜255）が、色の表の値（`#rrggbb`）に近いか。キャンバスのアルファが 1 を切ると、
  // ページの背景と混ざって白く飛ぶので、アルファが 255 のままであることも確かめる
  const near = (c: number[], code: string, tolerance = 6) =>
    c[3] === 255 &&
    [1, 3, 5].every(
      (at, i) => Math.abs((c[i] ?? 0) - Number.parseInt(code.slice(at, at + 2), 16)) <= tolerance,
    );

  await waitAllDrawn();
  const first = await mapState();
  check(first.drawn > 0, `起動するとチャンクが届いて描かれる（${first.drawn} 枚）`);
  const size = page.viewportSize() ?? { width: 1280, height: 720 };
  const cx = size.width / 2;
  const cy = size.height / 2;
  // 画面の中心は開始地点の買ってある土地で、水辺はない
  const owned = await pixel(cx, cy);
  check(
    near(owned, GROUND_HEX),
    `画面の中心に買ってある陸が淡い地面の色で描かれる（${owned.join(", ")}）`,
  );

  const initialSegments = Number(await map.getAttribute("data-road-segments"));
  check(
    initialSegments > 0,
    `起動すると、外部接続の高速道路のある道路網が届く（${initialSegments} 本）`,
  );
  // 高速道路の名前が、地図の上に重ねたラベルの canvas に盾で出る
  await page.waitForFunction(
    () =>
      (document.querySelector<HTMLElement>("#labels")?.dataset.labels ?? "").includes("自動車道"),
    undefined,
    { timeout: TIMEOUT_MS },
  );
  // ラベルは 200ms かけて現れる
  await page.waitForTimeout(400);
  const labelInk = await page.evaluate(() => {
    const canvas = document.querySelector<HTMLCanvasElement>("#labels");
    const ctx = canvas?.getContext("2d");
    if (canvas === null || canvas === undefined || ctx === null || ctx === undefined) return 0;
    const data = ctx.getImageData(0, 0, canvas.width, canvas.height).data;
    let n = 0;
    for (let i = 3; i < data.length; i += 4) if ((data[i] ?? 0) > 0) n++;
    return n;
  });
  check(
    labelInk > 0,
    `高速道路の名前のラベルが描かれる（${await page.locator("#labels").getAttribute("data-labels")}）`,
  );

  // 道路を引く道具で、画面の中心の行に横の道路を引く
  const message = page.locator("#tool-message");
  const outcomeCount = async () => Number((await message.getAttribute("data-count")) ?? 0);
  const nextOutcome = async (before: number) => {
    await page.waitForFunction(
      (n) => Number(document.querySelector<HTMLElement>("#tool-message")?.dataset.count ?? 0) > n,
      before,
      { timeout: TIMEOUT_MS },
    );
    return {
      command: await message.getAttribute("data-command"),
      outcome: await message.getAttribute("data-outcome"),
      text: await message.textContent(),
    };
  };
  const roadSegments = async () => Number((await map.getAttribute("data-road-segments")) ?? 0);
  const moneyNow = async () => Number(await hud.getAttribute("data-money"));
  const drawRoad = async () => {
    await page.mouse.click(cx - 80, cy);
    await page.mouse.move(cx + 80, cy + 6, { steps: 4 });
    const cost = Number(await map.getAttribute("data-preview-cost"));
    const before = await outcomeCount();
    await page.mouse.click(cx + 80, cy + 6);
    await page.keyboard.press("Escape");
    return { cost, result: await nextOutcome(before) };
  };

  // ゲーム内の 1 時間（300 tick）ごとに収支を精算する。この時点の収支は 40 マスの小さい道路 1 本の維持費
  // （建設費 10,000 の 1%/日）だけなので、`since` からの精算 1 回ごとに、1 日 100 の 1/24 を切り上げた額まで減ってよい
  const upkeepPerHour = Math.ceil((40 * 250) / 100 / 24);
  const waitMoney = (expected: number, since: number) =>
    page.waitForFunction(
      ({ expected, since, upkeepPerHour }) => {
        const hud = document.querySelector<HTMLElement>("#hud");
        const money = Number(hud?.dataset.money);
        const settled = Math.ceil(Number(hud?.dataset.tick) / 300) - Math.ceil(since / 300);
        return money <= expected && money >= expected - settled * upkeepPerHour;
      },
      { expected, since, upkeepPerHour },
      { timeout: TIMEOUT_MS },
    );
  await pickTool(page, "road-Small");
  const tickBefore = await tick();
  const moneyBefore = await moneyNow();
  const built = await drawRoad();
  check(
    built.result.outcome === "Applied",
    `道路を引くと受け付けられる（${built.result.outcome}：${built.result.text}）`,
  );
  // 縦に 1.5 マスずらした終点は横に寄せるので、40 マスの小さい道路になる
  check(built.cost === 40 * 250, `引く前に費用が見積もられる（${built.cost}）`);
  const waitSegments = (n: number) =>
    page.waitForFunction(
      (expected) =>
        Number(document.querySelector<HTMLElement>("#map")?.dataset.roadSegments) === expected,
      n,
      { timeout: TIMEOUT_MS },
    );
  await waitSegments(initialSegments + 1);
  check(
    (await roadSegments()) === initialSegments + 1,
    `引いた道路の区間が届く（${await roadSegments()} 本）`,
  );
  await waitMoney(moneyBefore - built.cost, tickBefore);
  check(true, `所持金が見積もりの分だけ減る（${moneyBefore} → ${await moneyNow()}）`);
  // 道路はマス (12, 32) から (52, 32) へ、マスの中心を通る。カメラは (32, 32) を画面の中心に、
  // 1 マスを 4 ピクセルで描くので、マス (x, y) の中心は画面の (cx + (x - 31.5) × 4, cy + (y - 31.5) × 4) にある
  await page.waitForTimeout(100);
  const zoom = first.zoom;
  const tileCenter = (x: number, y: number) => ({
    x: cx + (x + 0.5 - first.x) * zoom,
    y: cy + (y + 0.5 - first.y) * zoom,
  });
  // カーソルのある道路には始点の印が重なるので、道路から離れたところへ動かしてから色を読む
  await page.mouse.move(cx, cy - 120, { steps: 2 });
  await page.waitForTimeout(100);
  // 小さい道路は白の塗り、地面は淡い地面の色（#f4f2ec）で、白とは青の強さで見分けられる。
  // 引きの段では、小さい道路の色を地面の色に寄せて薄くする（stages.ts）。区間の途中もノードも同じ色になる
  const minorRoadHex = mixHex(GROUND_HEX, "#ffffff", stageWeights(zoom).minorRoad);
  const isRoad = (c: number[]) => near(c, minorRoadHex, 3);
  const isGround = (c: number[]) => near(c, GROUND_HEX, 4);
  const colorAt = async (x: number, y: number) => {
    const p = tileCenter(x, y);
    return pixel(p.x, p.y);
  };
  for (const [x, y, road, label] of [
    [32, 32, true, "道路の途中のマスの中心"],
    [12, 32, true, "始点のマスの中心"],
    [52, 32, true, "終点のマスの中心"],
    [32, 31, false, "1 つ上のマスの中心"],
    [32, 33, false, "1 つ下のマスの中心"],
    [10, 32, false, "始点の 2 つ左のマスの中心"],
    [54, 32, false, "終点の 2 つ右のマスの中心"],
  ] as const) {
    const c = await colorAt(x, y);
    check(
      road ? isRoad(c) : isGround(c),
      `${label} (${x}, ${y}) が${road ? "道路" : "地面"}の色で描かれる（${c.join(", ")}）`,
    );
  }

  // 道路の吸い付き。既存の道路（行 32）の 1 マス手前（行 31）で離すと、行 32 まで延びてつながる
  const toolInfo = page.locator("#tool-info");
  const snapSegments = await roadSegments();
  const [snapFrom, snapNear, snapFar] = [
    tileCenter(30, 20),
    tileCenter(30, 31),
    tileCenter(40, 20),
  ];
  await page.mouse.click(snapFrom.x, snapFrom.y);
  await page.mouse.move(snapFar.x, snapFar.y, { steps: 3 });
  const lonely = (await toolInfo.textContent()) ?? "";
  check(
    lonely.includes("つながらない"),
    `どの道路ともつながらない道路は、プレビューで知らせる（${lonely}）`,
  );
  await page.mouse.move(snapNear.x, snapNear.y, { steps: 3 });
  const joined = (await toolInfo.textContent()) ?? "";
  check(
    !joined.includes("つながらない") && joined.includes("長さ 12 マス"),
    `道路の 1 マス手前までなら、つながる道路として長さ 12 マスと見積もる（${joined}）`,
  );
  const snapBeforeOutcome = await outcomeCount();
  await page.mouse.click(snapNear.x, snapNear.y);
  await page.keyboard.press("Escape");
  const snapped = await nextOutcome(snapBeforeOutcome);
  check(snapped.outcome === "Applied", `吸い付けた道路は受け付けられる（${snapped.text}）`);
  // 新しい道路 1 本と、つながった既存の道路が分かれた 1 本で、区間が 2 本増える
  await waitSegments(snapSegments + 2);
  check(
    (await roadSegments()) === snapSegments + 2,
    `1 マス手前で離した道路は既存の道路につながる（${snapSegments} → ${await roadSegments()} 本）`,
  );

  // 信号の道具。吸い付けた道路でできた T 字路 (30, 32) に置いて、腕 3 本の灯火が届くかを見る
  const waitSignalArms = (n: number) =>
    page.waitForFunction(
      (expected) =>
        Number(document.querySelector<HTMLElement>("#map")?.dataset.signalArms ?? 0) === expected,
      n,
      { timeout: TIMEOUT_MS },
    );
  const junction = tileCenter(30, 32);
  await pickTool(page, "signal-on");
  const beforeSignal = await outcomeCount();
  await page.mouse.click(junction.x, junction.y);
  const signalled = await nextOutcome(beforeSignal);
  check(
    signalled.command === "SetSignal" && signalled.outcome === "Applied",
    `交差点に信号を置ける（${signalled.text}）`,
  );
  await waitSignalArms(3);
  check(true, "置いた信号の、腕 3 本の灯火が届く");
  const beforeMidway = await outcomeCount();
  const midway = tileCenter(40, 32);
  await page.mouse.click(midway.x, midway.y);
  const refused = await nextOutcome(beforeMidway);
  check(
    refused.outcome === "Rejected" && (refused.text ?? "").includes("交差点がない"),
    `交差点のないマスには置けない（${refused.text}）`,
  );
  await pickTool(page, "signal-off");
  const beforeOff = await outcomeCount();
  await page.mouse.click(junction.x, junction.y);
  const removed = await nextOutcome(beforeOff);
  check(removed.outcome === "Applied", `信号を外せる（${removed.text}）`);
  await waitSignalArms(0);
  check(true, "外した信号は描かれなくなる");

  // 横断歩道の道具。T 字路 (30, 32) の北の腕の縞の上を押して外し、外した腕が道路網に載って縞が描かれなくなるのを見る。
  // 縞の真ん中は、交差点のマスの中心から北へ約 0.38 マス（小さい道路の半分の幅と縞の長さの半分）にある
  const waitRemovedCrosswalks = (n: number) =>
    page.waitForFunction(
      (expected) =>
        Number(document.querySelector<HTMLElement>("#map")?.dataset.removedCrosswalks ?? -1) ===
        expected,
      n,
      { timeout: TIMEOUT_MS },
    );
  const above = tileCenter(30, 31);
  const zebraAt = {
    x: junction.x + (above.x - junction.x) * 0.38,
    y: junction.y + (above.y - junction.y) * 0.38,
  };
  await pickTool(page, "crosswalk-off");
  await page.mouse.move(zebraAt.x, zebraAt.y, { steps: 2 });
  const beforeCrosswalk = await outcomeCount();
  await page.mouse.click(zebraAt.x, zebraAt.y);
  const crosswalkOff = await nextOutcome(beforeCrosswalk);
  check(
    crosswalkOff.command === "SetCrosswalk" && crosswalkOff.outcome === "Applied",
    `交差点の腕の横断歩道を外せる（${crosswalkOff.text}）`,
  );
  await waitRemovedCrosswalks(1);
  check(true, "外した腕は道路網に載り、縞が描かれなくなる");
  await pickTool(page, "crosswalk-on");
  const beforeBack = await outcomeCount();
  await page.mouse.click(zebraAt.x, zebraAt.y);
  const crosswalkOn = await nextOutcome(beforeBack);
  check(crosswalkOn.outcome === "Applied", `外した横断歩道を戻せる（${crosswalkOn.text}）`);
  await waitRemovedCrosswalks(0);
  check(true, "戻した腕には縞がまた描かれる");
  await pickTool(page, "road-Small");

  const again = await drawRoad();
  check(
    again.result.outcome === "Denied" && (again.result.text ?? "").includes("重なる"),
    `同じ場所に引くと理由つきで断られる（${again.result.text}）`,
  );

  await page.locator('button[data-tool="bulldoze"]').click();
  const beforeBulldoze = await outcomeCount();
  await page.mouse.move(cx - 100, cy - 20);
  await page.mouse.down();
  await page.mouse.move(cx + 100, cy + 20, { steps: 4 });
  await page.mouse.up();
  const bulldozed = await nextOutcome(beforeBulldoze);
  check(
    bulldozed.command === "Bulldoze" && bulldozed.outcome === "Applied",
    `ブルドーザで長方形の中の道路を消せる（${bulldozed.text}）`,
  );
  await waitSegments(initialSegments);
  check((await roadSegments()) === initialSegments, "消した道路は描かれなくなる");

  // CS2 と同じく、一時停止したままでも建てられる
  await page.locator("#pause").click();
  await hud.and(page.locator("[data-paused=true]")).waitFor({ timeout: TIMEOUT_MS });
  await pickTool(page, "road-Small");
  const pausedAt = await tick();
  const pausedBuild = await drawRoad();
  await waitSegments(initialSegments + 1);
  check(
    pausedBuild.result.outcome === "Applied" && (await tick()) === pausedAt,
    `一時停止したままでも道路を引ける（${pausedBuild.result.text}、tick ${pausedAt}）`,
  );
  await page.locator("#pause").click();
  await hud.and(page.locator("[data-paused=false]")).waitFor({ timeout: TIMEOUT_MS });

  // 土地を買う道具。地図を左へ 400 ピクセル引きずり、右隣のチャンクが見えるようにする
  await page.locator('button[data-tool="buy"]').click();
  await page.mouse.move(cx + 200, cy);
  await page.mouse.down();
  await page.mouse.move(cx - 200, cy, { steps: 5 });
  await page.mouse.up();
  const view = await mapState();
  // 見る道具で地図を引きずり、カメラをワールドの (wx, wy) だけ動かす
  const panCameraBy = async (wx: number, wy: number) => {
    await page.locator('button[data-tool="none"]').click();
    const limitX = size.width / 2 - 20;
    const limitY = size.height / 2 - 20;
    for (let i = 0; i < 40 && (Math.abs(wx) > 0.05 || Math.abs(wy) > 0.05); i++) {
      const zoom = (await mapState()).zoom;
      const dx = Math.max(-limitX, Math.min(limitX, -wx * zoom));
      const dy = Math.max(-limitY, Math.min(limitY, -wy * zoom));
      await page.mouse.move(cx, cy);
      await page.mouse.down();
      await page.mouse.move(cx + dx, cy + dy, { steps: 4 });
      await page.mouse.up();
      wx += dx / zoom;
      wy += dy / zoom;
    }
    await page.waitForTimeout(150);
  };
  // チャンク (ccx, ccy) の真ん中のマスの、画面の位置
  const chunkPoint = (ccx: number, ccy: number) => ({
    x: cx + (ccx * 64 + 32.5 - view.x) * view.zoom,
    y: cy + (ccy * 64 + 32.5 - view.y) * view.zoom,
  });
  const buy = async (ccx: number, ccy: number) => {
    const p = chunkPoint(ccx, ccy);
    const before = await outcomeCount();
    await page.mouse.click(p.x, p.y);
    return nextOutcome(before);
  };
  const east = chunkPoint(2, 0);
  // 水辺のマスは買っても色が変わらないので、チャンクの真ん中のまわりの 7×7 点のうち、
  // 買った土地の地面の色（#f4f2ec）の点を数える。買っていない土地は #e6e3db と斜線の #d9d5ca で塗る。
  // 買う道具のカーソルが載っていれば、買える土地の仮の表示（青の 10%）が重なって #dee5ec になる
  const ownedPixels = async () => {
    let n = 0;
    for (let i = -3; i <= 3; i++) {
      for (let j = -3; j <= 3; j++) {
        const c = await pixel(east.x + i * 0.1 * 64 * view.zoom, east.y + j * 0.1 * 64 * view.zoom);
        n += near(c, GROUND_HEX, 2) || near(c, GROUND_ON_HINT_HEX, 2) ? 1 : 0;
      }
    }
    return n;
  };
  const eastBefore = await ownedPixels();
  const tickBeforeBuy = await tick();
  const moneyBeforeBuy = await moneyNow();
  const bought = await buy(2, 0);
  check(
    bought.command === "BuyChunk" && bought.outcome === "Applied",
    `隣のチャンクを買える（${bought.text}）`,
  );
  await waitMoney(moneyBeforeBuy - 50_000, tickBeforeBuy);
  check(true, `所持金が値段の分だけ減る（${moneyBeforeBuy} → ${await moneyNow()}）`);
  const untilOwned = Date.now() + TIMEOUT_MS;
  let eastAfter = await ownedPixels();
  while (eastAfter === 0 && Date.now() < untilOwned) {
    await page.waitForTimeout(100);
    eastAfter = await ownedPixels();
  }
  check(
    eastBefore === 0 && eastAfter > 0,
    `買ったチャンクが買った土地の色に描き直される（地面の色の点 ${eastBefore} → ${eastAfter}）`,
  );
  const far = await buy(3, 1);
  check(
    far.outcome === "Denied" && (far.text ?? "").includes("接していない"),
    `買った土地に接していないチャンクは理由つきで断られる（${far.text}）`,
  );
  // 75,000、100,000、125,000 で買い進めると、残りは次の 150,000 に届かない
  for (const [ccx, ccy] of [
    [3, 0],
    [2, 1],
    [3, 1],
  ] as const) {
    await buy(ccx, ccy);
  }
  const poor = await buy(4, 0);
  check(
    poor.outcome === "Denied" && (poor.text ?? "").includes("所持金"),
    `所持金が足りないと理由つきで断られる（${poor.text}、所持金 ${await moneyNow()}）`,
  );

  // 区画の道具。(12, 32) から (52, 32) の道路の下側に商業の区画を塗る
  await pickTool(page, "zone-Commercial");
  const tilePoint = (x: number, y: number) => ({
    x: cx + (x + 0.5 - view.x) * view.zoom,
    y: cy + (y + 0.5 - view.y) * view.zoom,
  });
  const colorOfTile = (x: number, y: number) => {
    const p = tilePoint(x, y);
    return pixel(p.x, p.y);
  };
  await page.waitForTimeout(100);
  const zonable = await colorOfTile(45, 37);
  const tooFar = await colorOfTile(45, 45);
  // 塗れるマスは、地面に青（#1a73e8）を 10% 重ねる。赤が下がり、青の強さが赤を上回る
  check(
    near(tooFar, GROUND_HEX, 2) &&
      (tooFar[0] ?? 0) - (zonable[0] ?? 0) > 12 &&
      (zonable[2] ?? 0) > (zonable[0] ?? 0) + 8,
    `区画の道具を選ぶと、道路から 6 マス以内のマスが青みがかって示される（${zonable.join(", ")} と ${tooFar.join(", ")}）`,
  );
  const beforeZone = await outcomeCount();
  const z0 = tilePoint(40, 33);
  const z1 = tilePoint(50, 36);
  await page.mouse.move(z0.x, z0.y);
  await page.mouse.down();
  await page.mouse.move(z1.x, z1.y, { steps: 4 });
  await page.mouse.up();
  const zoned = await nextOutcome(beforeZone);
  check(
    zoned.command === "PaintZone" && zoned.outcome === "Applied",
    `区画を塗れる（${zoned.text}）`,
  );
  await page.waitForFunction(
    (at) => {
      const canvas = document.querySelector<HTMLCanvasElement>("#map");
      const gl = canvas?.getContext("webgl2");
      if (canvas === null || canvas === undefined || gl === null || gl === undefined) {
        return false;
      }
      const out = new Uint8Array(4);
      gl.readPixels(
        Math.floor(at.x * window.devicePixelRatio),
        canvas.height - 1 - Math.floor(at.y * window.devicePixelRatio),
        1,
        1,
        gl.RGBA,
        gl.UNSIGNED_BYTE,
        out,
      );
      // 商業の区画の地面（#eaf0f8）は、買った土地の地面（#f4f2ec）と違って青が赤を上回る
      return (out[2] ?? 0) > (out[0] ?? 0) + 8;
    },
    tilePoint(45, 34),
    { timeout: TIMEOUT_MS },
  );
  const commercial = await colorOfTile(45, 34);
  // 引きと標準のあいだでは、区画の地面に街区の面（空き）を重ねる
  const blocks = stageWeights(view.zoom).blocks;
  check(
    near(commercial, mixHex("#eaf0f8", "#dde6f3", blocks), 4),
    `塗ったマスが商業の淡い青の地面で描かれる（${commercial.join(", ")}）`,
  );
  const farZone = await (async () => {
    const before = await outcomeCount();
    const a = tilePoint(45, 45);
    await page.mouse.move(a.x, a.y);
    await page.mouse.down();
    await page.mouse.move(a.x + 8, a.y + 8, { steps: 2 });
    await page.mouse.up();
    return nextOutcome(before);
  })();
  check(
    farZone.outcome === "Denied" && (farZone.text ?? "").includes("塗れるマスがない"),
    `道路から離れた場所は理由つきで断られる（${farZone.text}）`,
  );

  // 道路の上側に住宅の区画を塗り、時間を進めて建物が建つのを待つ
  await pickTool(page, "zone-Residential");
  const r0 = tilePoint(14, 26);
  const r1 = tilePoint(30, 31);
  await page.mouse.move(r0.x, r0.y);
  await page.mouse.down();
  await page.mouse.move(r1.x, r1.y, { steps: 4 });
  await page.mouse.up();
  const cityNumbers = page.locator("#city-numbers");
  await page.waitForFunction(
    () => Number(document.querySelector<HTMLElement>("#city-numbers")?.dataset.buildings ?? 0) > 0,
    undefined,
    { timeout: TIMEOUT_MS },
  );
  const firstBuilding = ((await map.getAttribute("data-first-building")) ?? "")
    .split(",")
    .map(Number);
  const [bx = 0, by = 0] = firstBuilding;
  check(
    Number(await cityNumbers.getAttribute("data-buildings")) > 0,
    `時間を進めると住宅が建ち、数が出る（${await cityNumbers.textContent()}）`,
  );
  await page.waitForFunction(
    () => document.querySelector<HTMLElement>("#overview")?.dataset.population !== undefined,
    undefined,
    { timeout: TIMEOUT_MS },
  );
  check(
    ((await page.locator("#overview-happiness").textContent()) ?? "").startsWith("幸福度"),
    "人口と幸福度が出る",
  );
  await page.waitForTimeout(100);
  // 印と重ならない、左上のマスの下寄りを読む
  const house = await pixel(
    cx + (bx + 0.5 - view.x) * view.zoom,
    cy + (by + 0.8 - view.y) * view.zoom,
  );
  // 引きと標準のあいだでは、街区の面（建物）の上に、建物のまとまりを薄く重ねる
  const houseHex = mixHex(
    mixHex("#eef4e6", "#cde0b9", stageWeights(view.zoom).blocks),
    "#d3e3c0",
    stageWeights(view.zoom).clusters,
  );
  check(
    near(house, houseHex, 4),
    `建物が住宅の緑の色で描かれる（マス (${bx}, ${by})、${house.join(", ")}）`,
  );
  const residentialDemand = await page.locator("#demand .bar").first().getAttribute("data-demand");
  check(residentialDemand !== null, `住宅の需要の棒が出る（${residentialDemand}）`);

  // 電気と水道。施設がないので、建物には届いていない印が出る
  const unsupplied = async () => Number((await map.getAttribute("data-unsupplied")) ?? 0);
  const waitUnsupplied = (test: "zero" | "some") =>
    page.waitForFunction(
      (t) => {
        const n = Number(document.querySelector<HTMLElement>("#map")?.dataset.unsupplied ?? 0);
        return t === "zero" ? n === 0 : n > 0;
      },
      test,
      { timeout: TIMEOUT_MS },
    );
  // 電気の印（稲妻）は建物の左上のマスの (0.25, 0.3) を中心に置く（roads.ts の markCenter）。
  // 縮小すると印は数ピクセルになるので、中心のまわりの 5×5 ピクセルを一度に読む
  const markPixels = () =>
    map.evaluate(
      (el, at) => {
        const canvas = el as HTMLCanvasElement;
        const gl = canvas.getContext("webgl2");
        const dpr = window.devicePixelRatio;
        const out: number[][] = [];
        for (let dy = -2; dy <= 2; dy++) {
          for (let dx = -2; dx <= 2; dx++) {
            const px = new Uint8Array(4);
            gl?.readPixels(
              Math.floor(at.x * dpr) + dx,
              canvas.height - 1 - Math.floor(at.y * dpr) - dy,
              1,
              1,
              gl.RGBA,
              gl.UNSIGNED_BYTE,
              px,
            );
            out.push([...px]);
          }
        }
        return out;
      },
      {
        x: cx + (bx + 0.25 - view.x) * view.zoom,
        y: cy + (by + 0.3 - view.y) * view.zoom,
      },
    );
  const isYellow = (c: number[]) => (c[0] ?? 0) > 200 && (c[1] ?? 0) > 180 && (c[2] ?? 0) < 100;
  const anyYellow = (cs: number[][]) => cs.some(isYellow);
  const markBefore = await markPixels();
  check(
    (await unsupplied()) > 0 && anyYellow(markBefore),
    `電気の届かない建物に黄色の印が出る（${await unsupplied()} 棟、${markBefore.map((c) => c.join(",")).join(" ")}）`,
  );
  const shortage = page.locator("#city-shortage");
  check(
    (await shortage.isVisible()) &&
      /足りない|つながっていない/.test((await shortage.textContent()) ?? ""),
    `届いていない建物の数と理由が欄に出る（${await shortage.textContent()}）`,
  );
  // 施設のない網に塗った区画が建たない理由は、500 ミリ秒ごとの overview で届く
  const growth = page.locator("#growth-notice");
  await page
    .waitForFunction(
      () => document.querySelector<HTMLElement>("#growth-notice")?.hidden === false,
      undefined,
      {
        timeout: TIMEOUT_MS,
      },
    )
    .catch(() => undefined);
  check(
    (await growth.isVisible()) &&
      /余裕がなく.*新築が止まっている/.test((await growth.textContent()) ?? ""),
    `施設のない網の区画が建たない理由が出る（${await growth.textContent()}）`,
  );
  // 区画が画面の外にあっても、知らせを押せばそこが映る。キーボードで押す
  const stallButton = growth.locator("button");
  check(
    (await stallButton.count()) === 1 && (await stallButton.isVisible()),
    "知らせに、止まっている区画へ移るボタンが出る",
  );
  const homeCamera = await mapState();
  await panCameraBy(400, 300);
  const awayCamera = await mapState();
  check(
    Math.abs(awayCamera.x - 32) > 200 && Math.abs(awayCamera.y - 32) > 100,
    `区画から遠く離れた（カメラ ${awayCamera.x.toFixed(1)}, ${awayCamera.y.toFixed(1)}）`,
  );
  await stallButton.focus();
  await page.keyboard.press("Enter");
  await page.waitForFunction(
    () => {
      const d = document.querySelector<HTMLElement>("#map")?.dataset;
      return Math.abs(Number(d?.cameraX) - 32) < 25 && Math.abs(Number(d?.cameraY) - 36) < 12;
    },
    undefined,
    { timeout: TIMEOUT_MS },
  );
  const jumped = await mapState();
  check(
    jumped.x >= 12 && jumped.x <= 53 && jumped.y >= 31 && jumped.y <= 45,
    `知らせを押すと、施設のない網の区画へカメラが動く（カメラ ${jumped.x.toFixed(1)}, ${jumped.y.toFixed(1)}）`,
  );
  // 以降の検査は最初の位置で見る。カメラを元へ戻す
  await panCameraBy(homeCamera.x - jumped.x, homeCamera.y - jumped.y);
  const restored = await mapState();
  check(
    Math.abs(restored.x - homeCamera.x) < 0.2 && Math.abs(restored.y - homeCamera.y) < 0.2,
    `カメラを元の位置へ戻した（${restored.x.toFixed(2)}, ${restored.y.toFixed(2)}）`,
  );
  check((await page.locator("#legend-body li").count()) >= 10, "凡例に区画・建物・印・施設が並ぶ");
  // 電気のデータレイヤーに切り替えると、建物が届いているかどうかの色で塗られる
  const layerPixel = () =>
    pixel(cx + (bx + 0.5 - view.x) * view.zoom, cy + (by + 0.8 - view.y) * view.zoom);
  const isRed = (c: number[]) => (c[0] ?? 0) > 180 && (c[1] ?? 0) < 100 && (c[2] ?? 0) < 100;
  const isGreen = (c: number[]) => (c[1] ?? 0) > (c[0] ?? 0) + 80 && (c[1] ?? 0) > 150;
  const waitColor = async (test: (c: number[]) => boolean) => {
    const until = Date.now() + TIMEOUT_MS;
    let c = await layerPixel();
    while (!test(c) && Date.now() < until) {
      await page.waitForTimeout(100);
      c = await layerPixel();
    }
    return c;
  };
  await chooseLayer(page, "1");
  const layerOff = await waitColor(isRed);
  check(
    isRed(layerOff) && (await page.locator("#layers").getAttribute("data-active")) === "1",
    `電気のレイヤーでは、届いていない建物が赤で塗られる（${layerOff.join(", ")}）`,
  );
  // 道路を (52, 32) から (60, 32) へ延ばし、その下に発電所と浄水場を置く
  await pickTool(page, "road-Small");
  for (const [x, y] of [
    [52, 32],
    [60, 32],
  ] as const) {
    const p = tilePoint(x, y);
    await page.mouse.click(p.x, p.y);
  }
  await page.keyboard.press("Escape");
  // カーソルのマスで中核に置けるかを問い合わせ、答えが届くまで待つ
  const hoverService = async (service: string, x: number, y: number) => {
    await pickTool(page, `service-${service}`);
    const p = tilePoint(x, y);
    await page.mouse.move(p.x - 8, p.y);
    await page.mouse.move(p.x, p.y);
    await page.waitForFunction(
      () => document.querySelector<HTMLElement>("#tool-info")?.textContent?.includes("費用"),
      undefined,
      { timeout: TIMEOUT_MS },
    );
    await page.waitForTimeout(200);
    return {
      placeable: await map.getAttribute("data-service-placeable"),
      rotation: await map.getAttribute("data-service-rotation"),
      info: await page.locator("#tool-info").textContent(),
    };
  };
  const away = await hoverService("WaterPump", 45, 45);
  check(
    away.placeable === "false" && (away.info ?? "").includes("置けない"),
    `道路に面していない場所では、置けないと示す（${away.info}）`,
  );
  const place = async (service: string, x: number, y: number) => {
    const hovered = await hoverService(service, x, y);
    check(
      hovered.placeable === "true" && hovered.rotation === "0",
      `${service} は道路に向く向きが自動で選ばれる（向き ${hovered.rotation}）`,
    );
    const p = tilePoint(x, y);
    const before = await outcomeCount();
    await page.mouse.click(p.x, p.y);
    return nextOutcome(before);
  };
  const plant = await place("PowerPlant", 53, 33);
  const pump = await place("WaterPump", 57, 33);
  check(
    plant.outcome === "Applied" && pump.outcome === "Applied",
    `発電所と浄水場を置ける（${plant.text}、${pump.text}）`,
  );
  await waitUnsupplied("zero");
  // 施設を置いて余裕が戻ると、建たない理由の知らせは消える
  await page
    .waitForFunction(
      () => document.querySelector<HTMLElement>("#growth-notice")?.hidden === true,
      undefined,
      {
        timeout: TIMEOUT_MS,
      },
    )
    .catch(() => undefined);
  check(
    !(await growth.isVisible()),
    `施設を置いて余裕が戻ると、建たない理由の知らせが消える（${await growth.textContent()}）`,
  );
  const layerOn = await waitColor(isGreen);
  check(isGreen(layerOn), `発電所を置くと、届いている緑に変わる（${layerOn.join(", ")}）`);
  // 発電所は電気のレイヤーで黄に塗る。水道を選ぶと電気は外れ（一度に 1 つだけ）、発電所の黄が消える
  const plantPixel = () => {
    const p = tilePoint(54, 34);
    return pixel(p.x, p.y);
  };
  let plantElectric = await plantPixel();
  for (let i = 0; i < 100 && !isYellow(plantElectric); i++) {
    await page.waitForTimeout(100);
    plantElectric = await plantPixel();
  }
  check(
    isYellow(plantElectric),
    `電気のレイヤーでは、発電所が黄に塗られる（${plantElectric.join(", ")}）`,
  );
  await chooseLayer(page, "2");
  let plantWater = await plantPixel();
  for (let i = 0; i < 100 && isYellow(plantWater); i++) {
    await page.waitForTimeout(100);
    plantWater = await plantPixel();
  }
  check(
    !isYellow(plantWater) &&
      (await page.locator("#layers").getAttribute("data-active")) === "2" &&
      (await page.locator('#layers button[data-layer="1"]').getAttribute("aria-pressed")) ===
        "false",
    `水道を選ぶと電気は外れ、一度に 1 つだけ出る（発電所 ${plantWater.join(", ")}）`,
  );
  // 交通のレイヤーを選ぶと、車のいない道路は「空いている」の緑に塗られる
  await chooseLayer(page, "none");
  const roadPoint = tilePoint(56, 32);
  const roadPixel = () => pixel(roadPoint.x, roadPoint.y);
  // 電気のレイヤーの青い道路と見分けるため、青が弱い緑だけを数える
  const isTrafficGreen = (c: number[]) =>
    isGreen(c) && (c[1] ?? 0) > (c[0] ?? 0) + 100 && (c[1] ?? 0) > (c[2] ?? 0) + 60;
  await chooseLayer(page, "3");
  const untilRoad = Date.now() + TIMEOUT_MS;
  let roadColor = await roadPixel();
  while (!isTrafficGreen(roadColor) && Date.now() < untilRoad) {
    await page.waitForTimeout(100);
    roadColor = await roadPixel();
  }
  check(
    isTrafficGreen(roadColor) &&
      (await page.locator("#layers").getAttribute("data-active")) === "3",
    `交通のレイヤーでは、道路が混み具合の色（空いていれば緑）で塗られる（${roadColor.join(", ")}）`,
  );
  await chooseLayer(page, "none");
  await page.waitForTimeout(100);
  const markAfter = await markPixels();
  const plantColor = await colorOfTile(54, 34);
  check(
    !anyYellow(markAfter) && (plantColor[0] ?? 0) > 200 && (plantColor[2] ?? 0) < 100,
    `施設が橙で描かれ、建物に供給が届いて印が消える（施設 ${plantColor.join(", ")}、印の場所 ${markAfter.map((c) => c.join(",")).join(" ")}）`,
  );
  // 「見る」で建物を押すと様子のパネルが出て、何もないところを押すと閉じる
  await page.locator('button[data-tool="none"]').click();
  const inspector = page.locator("#inspector");
  const building = tilePoint(bx, by);
  await page.mouse.click(building.x, building.y);
  await page.waitForFunction(
    () => document.querySelector<HTMLElement>("#inspector")?.dataset.title?.startsWith("住宅"),
    undefined,
    { timeout: TIMEOUT_MS },
  );
  const homeRows = (await inspector.locator("dt").allTextContents()).join(",");
  check(
    homeRows.includes("世帯") && homeRows.includes("電気") && homeRows.includes("水道"),
    `住宅を押すと、世帯と電気と水道が出る（${homeRows}）`,
  );
  const plantTile = tilePoint(54, 34);
  await page.mouse.click(plantTile.x, plantTile.y);
  await page.waitForFunction(
    () => document.querySelector<HTMLElement>("#inspector")?.dataset.title === "発電所",
    undefined,
    { timeout: TIMEOUT_MS },
  );
  await page.locator("[data-inspector-close]").click();
  check(await inspector.isHidden(), "閉じるボタンでパネルが閉じる");
  await page.mouse.click(building.x, building.y);
  await inspector.waitFor({ state: "visible", timeout: TIMEOUT_MS });
  const empty = tilePoint(45, 45);
  await page.mouse.click(empty.x, empty.y);
  await inspector.waitFor({ state: "hidden", timeout: TIMEOUT_MS });
  check(true, "何もないところを押すとパネルが閉じる");
  // 施設の前の道路を消すと、つながりが切れて印がまた出る
  await page.locator('button[data-tool="bulldoze"]').click();
  const cut = tilePoint(59, 32);
  const beforeCut = await outcomeCount();
  await page.mouse.click(cut.x, cut.y);
  const cutResult = await nextOutcome(beforeCut);
  await waitUnsupplied("some");
  check(
    cutResult.outcome === "Applied" && (await unsupplied()) > 0,
    `施設の前の道路を消すと印がまた出る（${await unsupplied()} 棟）`,
  );
  await page.locator('button[data-tool="none"]').click();

  await page.mouse.move(cx, cy);
  for (let i = 0; i < 20; i++) {
    await page.mouse.wheel(0, 400);
  }
  await page.waitForFunction(
    (min) => Number(document.querySelector<HTMLElement>("#map")?.dataset.zoom) === min,
    MIN_ZOOM,
    { timeout: TIMEOUT_MS },
  );
  await waitAllDrawn();
  const out = await mapState();
  check(out.zoom === MIN_ZOOM, `縮小は下限 ${MIN_ZOOM} で止まる（${out.zoom}）`);
  // 倍率 1 では 1 ピクセルが 8m なので、140px に収まる 1、2、5 の長さは 1 km になる
  const scale = page.locator("#scale");
  check(
    (await scale.getAttribute("data-meters")) === "1000" &&
      (await scale.getAttribute("data-px")) === "125" &&
      ((await scale.locator("[data-scale-label]").textContent()) ?? "") === "1 km" &&
      (await page.locator("#zoom-out").isDisabled()) &&
      !(await page.locator("#zoom-in").isDisabled()),
    "下限では、縮尺の物差しが 1 km（125px）を示し、縮小のボタンだけが押せない",
  );
  check(
    out.drawn > first.drawn,
    `縮小すると、まわりのチャンクも届いて描かれる（${first.drawn} 枚から ${out.drawn} 枚）`,
  );
  // 中心から 300 マス右は、買っていない土地
  const unowned = await pixel(cx + 300, cy);
  // 買っていない土地は、#e6e3db の地に #d9d5ca の斜線を引く。斜線の上でも地の上でも、買った土地の色とは違う
  check(
    (near(unowned, "#e6e3db", 2) || near(unowned, "#d9d5ca", 2)) && !near(unowned, GROUND_HEX, 2),
    `買っていない土地は斜線のある別の色で描かれる（${unowned.join(", ")}）`,
  );

  const anchor = { x: cx + 200, y: cy + 100 };
  const worldAt = (s: Awaited<ReturnType<typeof mapState>>) => ({
    x: s.x + (anchor.x - cx) / s.zoom,
    y: s.y + (anchor.y - cy) / s.zoom,
  });
  await page.mouse.move(anchor.x, anchor.y);
  await page.mouse.wheel(0, -300);
  await page.waitForFunction(
    (min) => Number(document.querySelector<HTMLElement>("#map")?.dataset.zoom) > min,
    MIN_ZOOM,
    { timeout: TIMEOUT_MS },
  );
  const zoomed = await mapState();
  const before = worldAt(out);
  const after = worldAt(zoomed);
  check(
    Math.abs(before.x - after.x) < 0.01 && Math.abs(before.y - after.y) < 0.01,
    `拡大してもカーソルの下の点は動かない（${before.x.toFixed(2)}, ${before.y.toFixed(2)} → ${after.x.toFixed(2)}, ${after.y.toFixed(2)}）`,
  );

  await page.mouse.move(cx, cy);
  await page.mouse.down();
  await page.mouse.move(cx + 100, cy + 50, { steps: 5 });
  await page.mouse.up();
  await page.waitForFunction(
    (x) => Number(document.querySelector<HTMLElement>("#map")?.dataset.cameraX) !== x,
    zoomed.x,
    { timeout: TIMEOUT_MS },
  );
  const dragged = await mapState();
  check(
    Math.abs(zoomed.x - dragged.x - 100 / zoomed.zoom) < 0.01 &&
      Math.abs(zoomed.y - dragged.y - 50 / zoomed.zoom) < 0.01,
    `ドラッグした分だけ地図が動く（${(zoomed.x - dragged.x).toFixed(2)}, ${(zoomed.y - dragged.y).toFixed(2)} マス）`,
  );

  for (let i = 0; i < 20; i++) {
    await page.mouse.wheel(0, -400);
  }
  await page.waitForFunction(
    (max) => Number(document.querySelector<HTMLElement>("#map")?.dataset.zoom) === max,
    MAX_ZOOM,
    { timeout: TIMEOUT_MS },
  );
  check((await mapState()).zoom === MAX_ZOOM, `拡大は上限 ${MAX_ZOOM} で止まる`);
  // 拡大縮小のボタンは、画面の真ん中を動かさずに 1.5 倍ずつ変え、物差しも追う
  await page.waitForFunction(
    () => document.querySelector<HTMLElement>("#scale")?.dataset.meters === "20",
    undefined,
    { timeout: TIMEOUT_MS },
  );
  const atMax = await mapState();
  check(
    (await page.locator("#zoom-in").isDisabled()) &&
      !(await page.locator("#zoom-out").isDisabled()) &&
      (await scale.getAttribute("data-px")) === "80",
    "上限では、拡大のボタンだけが押せず、物差しは 20 m（80px）を示す",
  );
  await page.locator("#zoom-out").click();
  await page.waitForFunction(
    (max) => Number(document.querySelector<HTMLElement>("#map")?.dataset.zoom) < max,
    MAX_ZOOM,
    { timeout: TIMEOUT_MS },
  );
  const smaller = await mapState();
  check(
    Math.abs(smaller.zoom - MAX_ZOOM / 1.5) < 0.01 &&
      Math.abs(smaller.x - atMax.x) < 0.01 &&
      Math.abs(smaller.y - atMax.y) < 0.01,
    `縮小のボタンで画面の真ん中を動かさずに 1.5 分の 1 になる（${smaller.zoom.toFixed(2)}）`,
  );
  await page.waitForFunction(
    () => document.querySelector<HTMLElement>("#scale")?.dataset.meters === "50",
    undefined,
    { timeout: TIMEOUT_MS },
  );
  check(
    (await scale.getAttribute("data-px")) === "133" &&
      ((await scale.locator("[data-scale-label]").textContent()) ?? "") === "50 m",
    "倍率が下がると、物差しは 50 m（133px）に変わる",
  );
  await page.locator("#zoom-in").click();
  await page.waitForFunction(
    (max) => Number(document.querySelector<HTMLElement>("#map")?.dataset.zoom) === max,
    MAX_ZOOM,
    { timeout: TIMEOUT_MS },
  );
  check(true, "拡大のボタンで上限まで戻る");

  // 収支のパネルは閉じていて、開くと税率を変えられる。変えた税率は記録に入り、下の再生でも同じ状態になる
  const finance = page.locator("#finance");
  await finance.and(page.locator("[data-net]")).waitFor({ state: "attached", timeout: TIMEOUT_MS });
  check(!(await finance.isVisible()), "収支のパネルは閉じて始まる");
  await page.locator("#finance-toggle").click();
  const resRow = finance.locator('[data-zone="Residential"]');
  check((await resRow.getAttribute("data-permille")) === "100", "住宅の税率は 10% で始まる");
  const netBefore = Number(await finance.getAttribute("data-net"));
  check(
    (await finance.getAttribute("data-deficit")) === String(netBefore < 0) &&
      (await hud.getAttribute("data-deficit")) === String(netBefore < 0),
    `赤字かどうかがパネルと所持金の横に出る（差し引き ${netBefore}）`,
  );
  await resRow.getByLabel("住宅の税率を上げる").click();
  await resRow.getByLabel("住宅の税率を上げる").click();
  await page.waitForFunction(
    () =>
      document.querySelector<HTMLElement>('#finance [data-zone="Residential"]')?.dataset
        .permille === "120",
    undefined,
    { timeout: TIMEOUT_MS },
  );
  check(
    (await resRow.locator(".finance-rate").textContent()) === "12%" &&
      (await page.locator("#tool-message").getAttribute("data-command")) === "SetTaxRate" &&
      (await page.locator("#tool-message").getAttribute("data-outcome")) === "Applied",
    "税率を 1% ずつ上げると中核が受け付け、パネルの税率が変わる",
  );
  await resRow.getByLabel("住宅の税率を下げる").click();
  await page.waitForFunction(
    () =>
      document.querySelector<HTMLElement>('#finance [data-zone="Residential"]')?.dataset
        .permille === "110",
    undefined,
    { timeout: TIMEOUT_MS },
  );
  check(true, "税率を下げると 11% に戻る");

  // 道具の帯は 7 つのボタンで、道路などの種類は押すと開く 2 段目で選ぶ
  const toolLabels = await page.locator("#toolbar .tool").allTextContents();
  check(
    toolLabels.join(",") === "見る,道路,区画,施設,信号,土地,取り壊す",
    `道具の帯は 7 つのボタンが並ぶ（${toolLabels.join(",")}）`,
  );
  const roadMenu = page.locator("#menu-road");
  check(!(await roadMenu.isVisible()), "道路の 2 段目は閉じて始まる");
  await page.locator('button[data-menu="road"]').click();
  check(
    (await roadMenu.isVisible()) &&
      (await page.locator('button[data-menu="road"]').getAttribute("aria-expanded")) === "true" &&
      (await page.locator('#menu-road button[data-tool^="road-"]').count()) === 3 &&
      (await page.locator('#menu-road button[data-tool^="upgrade-"]').count()) === 3,
    "「道路」を押すと、道路の種類と、種類を変える道具の 2 段目が開く",
  );
  await page.locator('button[data-tool="upgrade-Large"]').click();
  check(
    (await map.getAttribute("data-tool")) === "upgrade-Large" &&
      (await page.locator('button[data-menu="road"]').getAttribute("aria-pressed")) === "true" &&
      (await page.locator('button[data-tool="none"]').getAttribute("aria-pressed")) === "false",
    "2 段目で選ぶと、帯の「道路」が押した状態になる",
  );
  await page.keyboard.press("Escape");
  check(!(await roadMenu.isVisible()), "Esc で 2 段目が閉じる");
  await page.locator('button[data-tool="none"]').click();
  check(
    (await page.locator('button[data-menu="road"]').getAttribute("aria-pressed")) === "false",
    "「見る」に戻すと、帯の「道路」の押した状態が外れる",
  );
  // キーボードで開くと、フォーカスが 2 段目の中へ動き、Esc で入口のボタンへ戻る
  await page.locator('button[data-menu="zone"]').focus();
  await page.keyboard.press("Enter");
  check(
    await page.evaluate(() => document.activeElement?.closest("#menu-zone") !== null),
    "キーボードで 2 段目を開くと、フォーカスが中のボタンへ動く",
  );
  await page.keyboard.press("Escape");
  check(
    (await page.evaluate(() => document.activeElement?.getAttribute("data-menu"))) === "zone",
    "Esc で閉じると、フォーカスが入口のボタンへ戻る",
  );
  // レイヤーのパネルも、ボタンで開いて Esc か外を押して閉じる
  await page.locator("#layers-open").click();
  check(
    (await page.locator("#layer-panel").isVisible()) &&
      (await page.locator("#layers button").count()) === 4 &&
      (await page.locator("#layer-panel #legend").count()) === 1,
    "レイヤーのボタンで、なし、電気、水道、交通を選ぶパネルが開き、凡例も中にある",
  );
  await page.mouse.click(cx, cy);
  check(!(await page.locator("#layer-panel").isVisible()), "パネルの外（地図）を押すと閉じる");

  // フィードバックを送り、保存した記録を chizu-cli で再生する
  await page.locator("#feedback-open").click();
  await hud.and(page.locator("[data-paused=true]")).waitFor({ timeout: TIMEOUT_MS });
  await page.locator("#feedback-note").fill("道路の色が見分けにくい");
  await page.locator("#feedback-send").click();
  const result = page.locator("#feedback-result");
  await result.and(page.locator("[data-saved]")).waitFor({ timeout: TIMEOUT_MS });
  const saved = (await result.getAttribute("data-saved")) ?? "";
  const files = ["note.md", "screenshot.png", "state.json", "commands.rec"];
  check(
    saved.startsWith(feedbackDir) && files.every((f) => existsSync(join(saved, f))),
    `フィードバックが 4 つのファイルとして保存される（${saved}）`,
  );
  const fb: {
    seed: string;
    tick: number;
    stateHash: string;
    camera?: { zoom?: unknown };
  } = JSON.parse(readFileSync(join(saved, "state.json"), "utf8"));
  const png = readFileSync(join(saved, "screenshot.png"));
  check(
    png.subarray(1, 4).toString() === "PNG" && png.length > 1000,
    `スクリーンショットが PNG で保存される（${png.length} バイト）`,
  );
  check(
    readFileSync(join(saved, "note.md"), "utf8").includes("道路の色が見分けにくい") &&
      typeof fb.camera?.zoom === "number",
    "書いた文とカメラの位置が保存される",
  );
  const replayed = execFileSync(
    "cargo",
    [
      "run",
      "--quiet",
      "-p",
      "chizu-cli",
      "--",
      join(saved, "commands.rec"),
      fb.seed,
      String(fb.tick),
    ],
    {
      cwd: fileURLToPath(new URL("../..", import.meta.url)),
      encoding: "utf8",
      stdio: ["ignore", "pipe", "inherit"],
    },
  ).trim();
  check(
    replayed === fb.stateHash,
    `保存した記録を chizu-cli で再生すると同じ状態ハッシュになる（${replayed} と ${fb.stateHash}、tick ${fb.tick}）`,
  );
  // 開く前は動いていたので、閉じると再開する
  await page.locator('#feedback button[value="close"]').click();
  await hud.and(page.locator("[data-paused=false]")).waitFor({ timeout: TIMEOUT_MS });
  const afterClose = await ticksIn(500);
  check(afterClose > 0, `フィードバックの欄を閉じると再開する（${afterClose} tick）`);

  // 歩く人を出す道具は記録に入らず、上の再生の確かめと食い違うので、新しいページで確かめる
  await page.close();
  await checkHudPanel(await browser.newPage(), url);
  await checkDevWalks(await browser.newPage(), url);
} finally {
  await browser.close();
  await server?.close();
  rmSync(feedbackDir, { recursive: true, force: true });
}

if (failures.length > 0) {
  console.error(`test:wasm は ${failures.length} 件失敗した`);
  process.exit(1);
}
