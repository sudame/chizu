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
    await page.locator(`button[data-speed="${speed}"]`).click();
    await hud.and(page.locator(`[data-speed="${speed}"]`)).waitFor({ timeout: TIMEOUT_MS });
  };
  const pause = async (paused: boolean) => {
    await page.locator("#pause").click();
    await hud.and(page.locator(`[data-paused=${paused}]`)).waitFor({ timeout: TIMEOUT_MS });
  };
  // 外部接続の高速道路が届いてから、引く前の区間の数を読む
  await waitMap("roadSegments", "0", "gt");
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
  const brightness = (c: number[]) => (c[0] ?? 0) + (c[1] ?? 0) + (c[2] ?? 0);

  await setSpeed(1);
  await pause(true);
  const segments = d0.roadSegments ?? "0";
  await page.locator('button[data-tool="road-Small"]').click();
  const [from, to] = [tileCenter(12, 32), tileCenter(52, 32)];
  await page.mouse.click(from.x, from.y);
  await page.mouse.click(to.x, to.y);
  await page.keyboard.press("Escape");
  await waitMap("roadSegments", segments, "gt");
  await page.waitForTimeout(100);
  const walkFrom = tileCenter(14, 32);
  const walkTo = tileCenter(50, 32);
  const roadColor = await pixel(walkFrom);
  await page.locator('button[data-tool="dev-walks"]').click();
  await page.mouse.click(walkFrom.x, walkFrom.y);
  await page.mouse.click(walkTo.x, walkTo.y);
  await waitMap("devWalks", "0", "gt");
  await waitMap("drawnMovers", "0", "gt");
  const spawned = await map.evaluate((el) => ({ ...(el as HTMLElement).dataset }));
  const walkerColor = await pixel(walkFrom);
  check(
    brightness(walkerColor) > brightness(roadColor) + 200,
    `歩く人を出すと移動体の点が描かれる（${spawned.drawnMovers} 人、${roadColor.join(", ")} → ${walkerColor.join(", ")}）`,
  );
  await pause(false);
  await waitMap("firstMover", spawned.firstMover ?? "", "ne");
  const moved = await map.getAttribute("data-first-mover");
  check(true, `時間を進めると移動体の位置が変わる（${spawned.firstMover} → ${moved}）`);
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
    initialSegments > 0,
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
  await page.locator('button[data-tool="road-Small"]').click();
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
  // 道路とノードは灰色、地面は緑が強い
  const isRoad = (c: number[]) =>
    Math.abs((c[1] ?? 0) - (c[0] ?? 0)) < 20 && Math.abs((c[2] ?? 0) - (c[1] ?? 0)) < 20;
  const isGround = (c: number[]) => (c[1] ?? 0) > (c[0] ?? 0) + 30;
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

  // 土地を買う道具。地図を左へ 400 ピクセル引きずり、右隣のチャンクが見えるようにする
  await page.locator('button[data-tool="buy"]').click();
  await page.mouse.move(cx + 200, cy);
  await page.mouse.down();
  await page.mouse.move(cx - 200, cy, { steps: 5 });
  await page.mouse.up();
  const view = await mapState();
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
  const eastBefore = await pixel(east.x, east.y);
  const tickBeforeBuy = await tick();
  const moneyBeforeBuy = await moneyNow();
  const bought = await buy(2, 0);
  check(
    bought.command === "BuyChunk" && bought.outcome === "Applied",
    `隣のチャンクを買える（${bought.text}）`,
  );
  await waitMoney(moneyBeforeBuy - 50_000, tickBeforeBuy);
  check(true, `所持金が値段の分だけ減る（${moneyBeforeBuy} → ${await moneyNow()}）`);
  await page.waitForFunction(
    ({ x, y, before }) => {
      const canvas = document.querySelector<HTMLCanvasElement>("#map");
      const gl = canvas?.getContext("webgl2");
      if (canvas === null || canvas === undefined || gl === null || gl === undefined) {
        return false;
      }
      const out = new Uint8Array(4);
      const dpr = window.devicePixelRatio;
      gl.readPixels(
        Math.floor(x * dpr),
        canvas.height - 1 - Math.floor(y * dpr),
        1,
        1,
        gl.RGBA,
        gl.UNSIGNED_BYTE,
        out,
      );
      return (out[0] ?? 0) + (out[1] ?? 0) + (out[2] ?? 0) > before * 1.5;
    },
    { x: east.x, y: east.y, before: brightness(eastBefore) },
    { timeout: TIMEOUT_MS },
  );
  const eastAfter = await pixel(east.x, east.y);
  check(
    brightness(eastAfter) > brightness(eastBefore) * 1.5,
    `買ったチャンクが明るく描き直される（${eastBefore.join(", ")} → ${eastAfter.join(", ")}）`,
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
  await page.locator('button[data-tool="zone-Commercial"]').click();
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
  check(
    brightness(zonable) > brightness(tooFar) + 30,
    `区画の道具を選ぶと、道路から 6 マス以内のマスが明るく示される（${zonable.join(", ")} と ${tooFar.join(", ")}）`,
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
      return (out[2] ?? 0) > (out[0] ?? 0) + 50;
    },
    tilePoint(45, 34),
    { timeout: TIMEOUT_MS },
  );
  const commercial = await colorOfTile(45, 34);
  check(
    (commercial[2] ?? 0) > (commercial[0] ?? 0) + 50 && (commercial[2] ?? 0) > (commercial[1] ?? 0),
    `塗ったマスが商業の青で描かれる（${commercial.join(", ")}）`,
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
  await page.locator('button[data-tool="zone-Residential"]').click();
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
  await page.waitForTimeout(100);
  // 印と重ならない、左上のマスの下寄りを読む
  const house = await pixel(
    cx + (bx + 0.5 - view.x) * view.zoom,
    cy + (by + 0.8 - view.y) * view.zoom,
  );
  check(
    (house[1] ?? 0) > (house[0] ?? 0) + 50 && brightness(house) < 250,
    `建物が住宅の濃い緑で描かれる（マス (${bx}, ${by})、${house.join(", ")}）`,
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
  const isYellow = (c: number[]) =>
    (c[0] ?? 0) > 200 && (c[1] ?? 0) > 180 && (c[2] ?? 0) < 100;
  const anyYellow = (cs: number[][]) => cs.some(isYellow);
  const markBefore = await markPixels();
  check(
    (await unsupplied()) > 0 && anyYellow(markBefore),
    `電気の届かない建物に黄色の印が出る（${await unsupplied()} 棟、${markBefore.map((c) => c.join(",")).join(" ")}）`,
  );
  const shortage = page.locator("#city-shortage");
  check(
    (await shortage.isVisible()) && ((await shortage.textContent()) ?? "").includes("届いていない"),
    `届いていない建物の数と理由が欄に出る（${await shortage.textContent()}）`,
  );
  check(
    (await page.locator("#legend-body li").count()) >= 10,
    "凡例に区画・建物の段階・印・施設が並ぶ",
  );
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
  await page.locator('#layers button[data-layer="1"]').click();
  const layerOff = await waitColor(isRed);
  check(
    isRed(layerOff) && (await page.locator("#layers").getAttribute("data-active")) === "1",
    `電気のレイヤーでは、届いていない建物が赤で塗られる（${layerOff.join(", ")}）`,
  );
  // 道路を (52, 32) から (60, 32) へ延ばし、その下に発電所と浄水場を置く
  await page.locator('button[data-tool="road-Small"]').click();
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
    await page.locator(`button[data-tool="service-${service}"]`).click();
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
  const layerOn = await waitColor(isGreen);
  check(isGreen(layerOn), `発電所を置くと、届いている緑に変わる（${layerOn.join(", ")}）`);
  // 水道のレイヤーも重ねると、各マスの左半分が電気、右半分が水道になる。発電所は電気のレイヤーだけで黄に塗る
  await page.locator('#layers button[data-layer="2"]').click();
  const halfOf = (x: number, y: number, dx: number) => {
    const p = tilePoint(x, y);
    return pixel(p.x + (dx - 0.5) * view.zoom, p.y);
  };
  let plantLeft = await halfOf(54, 34, 0.25);
  let plantRight = await halfOf(54, 34, 0.75);
  for (let i = 0; i < 100 && (!isYellow(plantLeft) || isYellow(plantRight)); i++) {
    await page.waitForTimeout(100);
    plantLeft = await halfOf(54, 34, 0.25);
    plantRight = await halfOf(54, 34, 0.75);
  }
  check(
    isYellow(plantLeft) && !isYellow(plantRight),
    `電気と水道を重ねると、マスの左右に分けて塗る（左 ${plantLeft.join(", ")}、右 ${plantRight.join(", ")}）`,
  );
  await page.locator('#layers button[data-layer="none"]').click();
  await page.waitForTimeout(100);
  const markAfter = await markPixels();
  const plantColor = await colorOfTile(54, 34);
  check(
    !anyYellow(markAfter) && (plantColor[0] ?? 0) > 200 && (plantColor[2] ?? 0) < 100,
    `施設が橙で描かれ、建物に供給が届いて印が消える（施設 ${plantColor.join(", ")}、印の場所 ${markAfter.map((c) => c.join(",")).join(" ")}）`,
  );
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
