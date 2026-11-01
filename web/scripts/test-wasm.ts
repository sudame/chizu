// ビルドしたページを vite preview で配信し、ヘッドレスの Chrome で開く。
// preview を使うのは、本番と同じくビルドしたものを COOP/COEP のヘッダーつきで配信して確かめるためである。
// スレッドつきの WASM が Worker で動き、正しい和を返すかを確かめる。
// 三重バッファから読んだ時計が実時間に合わせて進み、速度と一時停止のボタンに従うかも確かめる。
// チャンクが Worker から届いて描かれること、ドラッグとホイールでカメラが動くことも確かめる。
// 道路の道具で道路を引くと描かれて所持金が減ること、断られると理由が出ること、ブルドーザで消せることも確かめる。
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
// web/src/map/camera.ts の値
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
  const brightness = (c: number[]) => (c[0] ?? 0) + (c[1] ?? 0) + (c[2] ?? 0);

  await waitAllDrawn();
  const first = await mapState();
  check(first.drawn > 0, `起動するとチャンクが届いて描かれる（${first.drawn} 枚）`);
  const size = page.viewportSize() ?? { width: 1280, height: 720 };
  const cx = size.width / 2;
  const cy = size.height / 2;
  // 画面の中心は開始地点の買ってある土地で、水辺はない
  const owned = await pixel(cx, cy);
  check(
    (owned[1] ?? 0) > (owned[0] ?? 0) &&
      (owned[1] ?? 0) > (owned[2] ?? 0) &&
      brightness(owned) > 300,
    `画面の中心に買ってある陸が描かれる（${owned.join(", ")}）`,
  );

  const initialSegments = Number(await map.getAttribute("data-road-segments"));
  check(
    (await map.getAttribute("data-road-revision")) === "0" && initialSegments > 0,
    `起動すると、外部接続の高速道路のある道路網が届く（${initialSegments} 本）`,
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

  await page.locator('button[data-tool="road-Small"]').click();
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
  await page.waitForFunction(
    (expected) => Number(document.querySelector<HTMLElement>("#hud")?.dataset.money) === expected,
    moneyBefore - built.cost,
    { timeout: TIMEOUT_MS },
  );
  check(true, `所持金が見積もりの分だけ減る（${moneyBefore} → ${await moneyNow()}）`);
  // 道路はマスの中心を通るので、中心の行のマスの真ん中（2 ピクセル下）に描かれる
  await page.waitForTimeout(100);
  const road = await pixel(cx, cy + 2);
  check(
    Math.abs((road[1] ?? 0) - (road[0] ?? 0)) < 30 && brightness(road) < brightness(owned),
    `道路が灰色で描かれる（${road.join(", ")}）`,
  );

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
  await page.locator('button[data-tool="road-Small"]').click();
  const pausedAt = await tick();
  const pausedBuild = await drawRoad();
  await waitSegments(initialSegments + 1);
  check(
    pausedBuild.result.outcome === "Applied" && (await tick()) === pausedAt,
    `一時停止したままでも道路を引ける（${pausedBuild.result.text}、tick ${pausedAt}）`,
  );
  await page.locator("#pause").click();
  await hud.and(page.locator("[data-paused=false]")).waitFor({ timeout: TIMEOUT_MS });
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
  check(
    out.drawn > first.drawn,
    `縮小すると、まわりのチャンクも届いて描かれる（${first.drawn} 枚から ${out.drawn} 枚）`,
  );
  // 中心から 300 マス右は、買っていない土地
  const unowned = await pixel(cx + 300, cy);
  check(
    brightness(unowned) < brightness(owned) * 0.6,
    `買っていない土地は暗く描かれる（${unowned.join(", ")}）`,
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
} finally {
  await browser.close();
  await server?.close();
}

if (failures.length > 0) {
  console.error(`test:wasm は ${failures.length} 件失敗した`);
  process.exit(1);
}
