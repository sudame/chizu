import { parseCityStats, renderCityStats } from "./city-panel.ts";
import { DEV_WALK_TOOL, setupDevWalks } from "./dev-walks.ts";
import { setupFeedback } from "./feedback.ts";
import {
  deficitNote,
  type Finance,
  type FinancePanel,
  parseFinance,
  setupFinance,
} from "./finance.ts";
import { parseGrowthBlocks, parseStalls, setupGrowthNotice } from "./growth-notice.ts";
import { setupHudPanel, trackHeight } from "./hud-panel.ts";
import { applyInspectReply, setupInspector } from "./inspector.ts";
import { renderLegend } from "./legend.ts";
import { Camera, type ChunkRange, chunksByDistance, type Viewport } from "./map/camera.ts";
import { ChunkStore } from "./map/chunk-sync.ts";
import { setupLayerPanel } from "./map/layers.ts";
import { KIND_CAR, KIND_WALK, moverArea } from "./map/movers.ts";
import { MapRenderer } from "./map/renderer.ts";
import {
  buildingGeometry,
  type Color,
  type CrosswalkArm,
  crosswalkArms,
  FACILITY_COLOR,
  Geometry,
  MIN_PX,
  ROAD_VISUAL_HALF,
  type RoadMeshes,
  roadFootprint,
  roadMeshes,
  signalGeometry,
  unsuppliedCount,
} from "./map/roads.ts";
import {
  buildRoadIndex,
  DISCONNECTED_NOTICE,
  EMPTY_ROAD_INDEX,
  isDisconnected,
  type RoadIndex,
  roadEndpoints,
} from "./map/snap.ts";
import {
  buildRoadCommand,
  bulldozeCommand,
  buyChunkCommand,
  chooseRotation,
  clampArea,
  crosswalkCommand,
  MAX_ZONE_SIDE,
  paintZoneCommand,
  parseRoadCostTable,
  parseServiceTable,
  pickCrosswalk,
  placeServiceCommand,
  type RoadCostTable,
  roadCost,
  type ServiceInfo,
  type ServiceProbe,
  screenToTile,
  serviceFootprint,
  signalCommand,
  type Tile,
  type Tool,
  upgradePlan,
  upgradeRoadCommand,
} from "./map/tools.ts";
import { parseOverview, renderOverview } from "./overview.ts";
import {
  CHUNK_SIDE,
  type CommandMessage,
  FACILITY_STRIDE,
  type InspectQuery,
  isWorkerToMain,
  type MainToWorker,
  type Outcome,
  type RoadKindName,
  SEGMENT_STRIDE,
  type ServiceName,
  SIGNAL_STRIDE,
  type ZoneName,
} from "./protocol/messages.ts";
import { type SnapshotHeader, SnapshotReader } from "./protocol/snapshot.ts";
import { setupSaves } from "./save/main-side.ts";
import { describeSpeed, shownSpeed, splitSpeed } from "./speed.ts";

function element<T extends Element>(selector: string): T {
  const found = document.querySelector<T>(selector);
  if (found === null) {
    throw new Error(`${selector} が見つからない`);
  }
  return found;
}

const status = element<HTMLElement>("#status");
const hud = element<HTMLElement>("#hud");
const hudClock = element<HTMLElement>("#hud-clock");
const hudMoney = element<HTMLElement>("#hud-money");
const hudDeficit = element<HTMLElement>("#hud-deficit");
const hudSpeed = element<HTMLElement>("#hud-speed");
const mapCanvas = element<HTMLCanvasElement>("#map");
renderLegend(element<HTMLElement>("#legend-body"));
const pauseButton = element<HTMLButtonElement>("#pause");
const toolInfo = element<HTMLElement>("#tool-info");
const toolMessage = element<HTMLElement>("#tool-message");
const toolButtons = [...document.querySelectorAll<HTMLButtonElement>("button[data-tool]")];

const worker = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
setupSaves(worker, element<HTMLElement>("#controls"));

function send(message: MainToWorker): void {
  worker.postMessage(message);
}

// 送ったコマンドの種類を、画面が振った番号で引く。結果の通知はこの番号で突き合わせる
let nextCommandId = 0;
const sentCommands = new Map<number, CommandMessage["kind"]>();
function sendCommand(command: CommandMessage): void {
  const id = nextCommandId++;
  sentCommands.set(id, command.kind);
  send({ kind: "command", id, command });
}

// 最後に届いた収支の見込み。所持金の横の赤字の表示に使う
let finance: Finance | null = null;
const financePanel: FinancePanel = setupFinance(
  element<HTMLDetailsElement>("#finance"),
  sendCommand,
);

// 1 倍より遅く進めているときの倍率。中核の速度は 1 のままなので、Worker だけが持つ。表示は Worker が返した値を使う
let workerPace = 1;

const pad = (n: number) => String(n).padStart(2, "0");

function describeClock(header: SnapshotHeader): string {
  const hour = Math.floor(header.secondOfDay / 3600);
  const minute = Math.floor(header.secondOfDay / 60) % 60;
  return `${header.day + 1} 日目 ${pad(hour)}:${pad(minute)}`;
}

// 同じ文字なら書き換えない（毎フレーム呼ぶので、レイアウトをやり直させない）
function setText(el: HTMLElement, text: string): void {
  if (el.textContent !== text) {
    el.textContent = text;
  }
}

function draw(header: SnapshotHeader): void {
  setText(hudClock, describeClock(header));
  setText(hudMoney, `お金 ${header.money.toLocaleString("ja-JP")}`);
  // 収支が赤字のあいだは、所持金の横に知らせる
  const note = deficitNote(finance);
  setText(hudDeficit, note);
  hudDeficit.hidden = note === "";
  const speed = shownSpeed(header.speed, workerPace);
  setText(hudSpeed, header.paused ? "一時停止中" : describeSpeed(speed));
  hud.dataset.deficit = String(note !== "");
  // test:wasm が読む
  hud.dataset.tick = String(header.tick);
  hud.dataset.speed = String(speed);
  hud.dataset.paused = String(header.paused);
  hud.dataset.day = String(header.day);
  hud.dataset.secondOfDay = String(header.secondOfDay);
  hud.dataset.money = String(header.money);
  money = header.money;
}

// 手元に置くチャンクの数の上限。超えたら見えていないものから捨てる
const MAX_STORED_CHUNKS = 4096;
// ホイールの 1 ピクセル分の拡大率の変わり方
const WHEEL_ZOOM_RATE = 0.0015;
const WHEEL_LINE_PIXELS = 16;

// 開始地点の買ってあるチャンク（-1〜1）の真ん中から見始める
const camera = new Camera(CHUNK_SIDE / 2, CHUNK_SIDE / 2, 4);
const store = new ChunkStore();
// 建たない理由の知らせを押すと、止まっている区画のマスの真ん中へカメラを動かす。拡大率は変えない
const growthNotice = setupGrowthNotice((x, y) => {
  camera.x = x + 0.5;
  camera.y = y + 0.5;
});

// WebGL2 を使えなくても、時計と操作は動かす
let mapError: string | null = null;
function createRenderer(): MapRenderer | null {
  try {
    const created = new MapRenderer(mapCanvas);
    mapError = null;
    return created;
  } catch (error) {
    mapError = `地図を描けない：${String(error)}`;
    return null;
  }
}
let renderer = createRenderer();
// 最後に届いた道路網と建物。コンテキストを失ったあとに貼り直す
let roads: RoadMeshes | null = null;
let buildings: Geometry | null = null;
let signals: Geometry | null = null;
let activeLayers: number[] = [];
setupHudPanel(element<HTMLButtonElement>("#hud-toggle"), element<HTMLElement>("#hud-details"));
trackHeight(element<HTMLElement>("#toolbar"), "--toolbar-h");
trackHeight(element<HTMLElement>("#hudbar"), "--hudbar-h");
setupLayerPanel(element<HTMLElement>("#layers"), (active) => {
  activeLayers = active;
  renderer?.layers.setActive(active);
  send({ kind: "layers", active });
});

// コンテキストが戻ったら作り直し、手元のチャンクを貼り直す
mapCanvas.addEventListener("webglcontextlost", (event) => {
  event.preventDefault();
  renderer = null;
});
mapCanvas.addEventListener("webglcontextrestored", () => {
  renderer = createRenderer();
  if (renderer !== null) {
    renderer.showZonable = tool.kind === "zone";
  }
  for (const chunk of store.all()) {
    renderer?.upload(chunk);
  }
  if (roads !== null) {
    renderer?.roads.setRoads(roads);
  }
  if (buildings !== null) {
    renderer?.roads.setBuildings(buildings);
  }
  if (signals !== null) {
    renderer?.roads.setSignals(signals);
  }
  renderer?.layers.setActive(activeLayers);
  send({ kind: "layers", active: activeLayers });
  updatePreview();
});

const viewport = (): Viewport => ({
  width: mapCanvas.clientWidth,
  height: mapCanvas.clientHeight,
});
const inRange = (r: ChunkRange, cx: number, cy: number) =>
  cx >= r.minCx && cx <= r.maxCx && cy >= r.minCy && cy <= r.maxCy;

let sentRange = "";
// 見えているチャンクの範囲が変わったときだけ、その範囲と捨てたチャンクを Worker に伝える
function sendView(range: ChunkRange): void {
  const movers = moverArea(range, camera.zoom);
  const key = `${range.minCx},${range.maxCx},${range.minCy},${range.maxCy},${movers !== null}`;
  if (key === sentRange) {
    return;
  }
  sentRange = key;
  const visible = chunksByDistance(range, { x: camera.x, y: camera.y });
  store.touch(visible);
  const removed = store.evict((cx, cy) => inRange(range, cx, cy), MAX_STORED_CHUNKS);
  for (const chunk of removed) {
    renderer?.remove(chunk.cx, chunk.cy);
  }
  send({
    kind: "view",
    visible: visible.map(([cx, cy]) => ({ cx, cy })),
    forgotten: removed.map(({ cx, cy }) => ({ cx, cy })),
    movers,
  });
}

function drawMap(reader: SnapshotReader, header: SnapshotHeader | null): void {
  const vp = viewport();
  const range = camera.visibleChunks(vp);
  sendView(range);
  const drawn = renderer?.draw(camera, vp, range) ?? 0;
  const movers =
    header === null
      ? 0
      : (renderer?.movers.draw(
          camera,
          vp,
          header,
          reader.movers(header.moverCount),
          performance.now(),
        ) ?? 0);
  // test:wasm が読む
  mapCanvas.dataset.drawnMovers = String(movers);
  const first = renderer?.movers.first;
  mapCanvas.dataset.firstMover = first ? `${first.x},${first.y}` : "";
  const firstOf = (kind: number) => {
    const p = renderer?.movers.firstOf.get(kind);
    return p ? `${p.x},${p.y}` : "";
  };
  mapCanvas.dataset.firstWalker = firstOf(KIND_WALK);
  mapCanvas.dataset.firstCar = firstOf(KIND_CAR);
  mapCanvas.dataset.drawnChunks = String(drawn);
  mapCanvas.dataset.visibleChunks = String(
    (range.maxCx - range.minCx + 1) * (range.maxCy - range.minCy + 1),
  );
  mapCanvas.dataset.storedChunks = String(store.size);
  mapCanvas.dataset.cameraX = String(camera.x);
  mapCanvas.dataset.cameraY = String(camera.y);
  mapCanvas.dataset.zoom = String(camera.zoom);
}

function startDrawing(reader: SnapshotReader): void {
  const frame = () => {
    const header = reader.acquire();
    if (header !== null) {
      draw(header);
    }
    drawMap(reader, header);
    requestAnimationFrame(frame);
  };
  requestAnimationFrame(frame);
}

// 左のボタンは、この距離（CSS ピクセル）より動かしたら地図を引きずる操作とみなす。中のボタンはいつも引きずる
const CLICK_SLOP_PX = 4;
const PREVIEW_OK: Color = [0.3, 0.6, 1, 0.7];
const PREVIEW_BAD: Color = [1, 0.3, 0.3, 0.7];
// 種類を変える道具でなぞった直線。変わる区間の色より薄くする
const PREVIEW_LINE: Color = [1, 1, 1, 0.3];
const BULLDOZE_AREA: Color = [1, 0.3, 0.3, 0.35];
// 区画を塗る長方形の色。renderer.ts の区画の色に合わせる
const ZONE_AREA: Record<ZoneName, Color> = {
  Residential: [0.15, 0.95, 0.15, 0.35],
  Commercial: [0.25, 0.45, 0.95, 0.35],
  Industrial: [0.95, 0.8, 0.2, 0.35],
  Office: [0.3, 0.85, 0.95, 0.35],
  Clear: [1, 1, 1, 0.35],
};
const BUY_AREA: Color = [1, 1, 1, 0.2];
// 横断歩道の道具で変えられる腕と、いま押すと変わる腕の印
const CROSSWALK_SPOT: Color = [0.3, 0.6, 1, 0.45];
const CROSSWALK_HALF = 0.14;
const CROSSWALK_PICKED_HALF = 0.24;

let tool: Tool = { kind: "none" };
let roadCosts: RoadCostTable | null = null;
let services: Record<ServiceName, ServiceInfo> | null = null;
// 施設の向き。近くの道路に向くよう自動で決め、R キーで回すと手で決めた向きを使う
let manualRotation: number | null = null;
// 施設を置けるかの、中核の答え。カーソルのマスが変わるたびに Worker に問い合わせる
let probe: { service: ServiceName; x: number; y: number; results: ServiceProbe } | null = null;
let probeSent = "";
let money = 0;
let roadStart: Tile | null = null;
let roadIndex: RoadIndex = EMPTY_ROAD_INDEX;
// 最後に届いた道路の区間（chizu-wasm の road_segments）。種類を変える道具の見積もりに使う
let roadSegments: ArrayLike<number> = [];
// Alt を押しているあいだは、道路を近くの道路に吸い付けない
let noSnap = false;
// プレビューで最後に見せた道路。押したときは、これと同じ道路を送る
let shownRoad: {
  start: Tile | null;
  cursor: Tile;
  noSnap: boolean;
  from: Tile;
  to: Tile;
} | null = null;
let cursor: Tile | null = null;
// カーソルのワールド座標（マスの単位）。横断歩道の道具は、マスの中のどの腕に近いかで選ぶ
let cursorPoint: { x: number; y: number } | null = null;
// 交差点の、横断歩道を置ける腕。道路網が届くたびに作り直す
let crosswalks: CrosswalkArm[] = [];
// プレビューで最後に見せた横断歩道の腕。押したときは、これと同じ腕を送る
let shownCrosswalk: CrosswalkArm | null = null;
let pointer: {
  id: number;
  x: number;
  y: number;
  mode: "click" | "pan" | "area";
  start: Tile;
} | null = null;

const tileAt = (event: PointerEvent): Tile => {
  const rect = mapCanvas.getBoundingClientRect();
  return screenToTile(camera, event.clientX - rect.left, event.clientY - rect.top, viewport());
};
const pointAt = (event: PointerEvent): { x: number; y: number } => {
  const rect = mapCanvas.getBoundingClientRect();
  return camera.screenToWorld(event.clientX - rect.left, event.clientY - rect.top, viewport());
};

// 道具の仮の表示と、道路の長さと費用の見積もりを書き直す
function updatePreview(): void {
  let geometry: Geometry | null = null;
  let info = "";
  let infoText = "";
  if (tool.kind === "upgrade" && cursor !== null) {
    const { from, to } = roadEndpoints(roadIndex, roadStart, cursor, noSnap);
    shownRoad = { start: roadStart, cursor, noSnap, from, to };
    geometry = new Geometry(from);
    if (roadStart === null) {
      info = "変えたい道路の端を押し、なぞってもう一度押す";
    } else if (roadCosts !== null) {
      // 変わるのは直線が通る区間の全体なので、区間ごと塗る
      const plan = upgradePlan(roadSegments, roadCosts, tool.road, from, to);
      const short = plan.cost > 0 && plan.cost > money;
      const color = short || plan.segments.length === 0 ? PREVIEW_BAD : PREVIEW_OK;
      roadFootprint(geometry, from, to, PREVIEW_LINE, MIN_PX);
      for (const s of plan.segments) {
        roadFootprint(geometry, s.from, s.to, color, MIN_PX);
      }
      info =
        plan.roads === 0
          ? "変えられる道路がない（高速道路は変えない）"
          : plan.segments.length === 0
            ? "道路はもうその種類になっている"
            : `区間 ${plan.segments.length} 本・費用 ${plan.cost.toLocaleString("ja-JP")}`;
      mapCanvas.dataset.previewCost = String(plan.cost);
    }
    geometry.square(from, ROAD_VISUAL_HALF, PREVIEW_OK, MIN_PX);
  } else if (tool.kind === "road" && cursor !== null) {
    const { from, to } = roadEndpoints(roadIndex, roadStart, cursor, noSnap);
    shownRoad = { start: roadStart, cursor, noSnap, from, to };
    geometry = new Geometry(from);
    // 置いたあとの道路と同じく、通るマスをマスいっぱいに塗る
    if (roadStart !== null && roadCosts !== null) {
      const cost = roadCost(roadCosts, tool.road, from, to);
      const length = Math.max(Math.abs(to.x - from.x), Math.abs(to.y - from.y));
      const color = cost > money ? PREVIEW_BAD : PREVIEW_OK;
      roadFootprint(geometry, from, to, color, MIN_PX);
      info = `長さ ${length} マス・費用 ${cost.toLocaleString("ja-JP")}`;
      if (isDisconnected(roadIndex, from, to)) {
        info += `・${DISCONNECTED_NOTICE}`;
      }
      mapCanvas.dataset.previewCost = String(cost);
    }
    geometry.square(from, ROAD_VISUAL_HALF, PREVIEW_OK, MIN_PX);
  } else if ((tool.kind === "bulldoze" || tool.kind === "zone") && cursor !== null) {
    const start = pointer?.mode === "area" ? pointer.start : cursor;
    const end = tool.kind === "zone" ? clampArea(start, cursor, MAX_ZONE_SIDE) : cursor;
    geometry = new Geometry(start);
    const color = tool.kind === "zone" ? ZONE_AREA[tool.zone] : BULLDOZE_AREA;
    geometry.rect(
      { x: Math.min(start.x, end.x), y: Math.min(start.y, end.y) },
      { x: Math.max(start.x, end.x), y: Math.max(start.y, end.y) },
      color,
    );
  } else if (tool.kind === "service" && cursor !== null && services !== null) {
    const info = services[tool.service];
    requestProbe(tool.service, cursor);
    const choice = serviceChoice(tool.service, cursor);
    const { min, max } = serviceFootprint(cursor, choice.rotation, info.width, info.depth);
    geometry = new Geometry(min);
    const base = FACILITY_COLOR[tool.service === "PowerPlant" ? 1 : 2] ?? PREVIEW_OK;
    const color: Color = choice.placeable ? [base[0], base[1], base[2], 0.6] : PREVIEW_BAD;
    geometry.rect(min, max, color);
    const cost = `費用 ${info.cost.toLocaleString("ja-JP")}・R で回す`;
    infoText = choice.placeable ? cost : `置けない：${choice.reason}・${cost}`;
    // test:wasm が読む
    mapCanvas.dataset.serviceRotation = String(choice.rotation);
    mapCanvas.dataset.servicePlaceable = String(choice.placeable);
  } else if (tool.kind === "crosswalk") {
    // 変えられる腕に印を付け、カーソルに近い腕を大きく示す
    const on = tool.on;
    const candidates = crosswalks.filter((arm) => arm.removed === on);
    geometry = new Geometry(candidates[0] ?? { x: 0, y: 0 });
    for (const arm of candidates) {
      geometry.dot(arm.cx, arm.cy, CROSSWALK_HALF, CROSSWALK_SPOT, MIN_PX);
    }
    shownCrosswalk = cursorPoint === null ? null : pickCrosswalk(crosswalks, cursorPoint, on);
    if (shownCrosswalk !== null) {
      geometry.dot(shownCrosswalk.cx, shownCrosswalk.cy, CROSSWALK_PICKED_HALF, PREVIEW_OK, MIN_PX);
    }
    info = on
      ? candidates.length === 0
        ? "横断歩道を外した腕がない"
        : "印の腕を押して横断歩道を戻す。歩く人は近道でき、曲がる車はまた歩く人を待つ"
      : candidates.length === 0
        ? "横断歩道のある交差点がない"
        : "印の腕を押して横断歩道を外す。曲がる車は止まらなくなり、歩く人は交差点を回り込む";
  } else if (tool.kind === "signal" && cursor !== null) {
    geometry = new Geometry(cursor);
    geometry.rect(cursor, cursor, BUY_AREA);
    info = tool.on
      ? "3 本以上の道路が集まる交差点を押して信号を置く"
      : "信号のある交差点を押して外す";
  } else if (tool.kind === "buy" && cursor !== null) {
    const c = buyChunkCommand(cursor);
    if (c.kind === "BuyChunk") {
      const min = { x: c.cx * CHUNK_SIDE, y: c.cy * CHUNK_SIDE };
      geometry = new Geometry(min);
      geometry.rect(min, { x: min.x + CHUNK_SIDE - 1, y: min.y + CHUNK_SIDE - 1 }, BUY_AREA);
      info = `チャンク (${c.cx}, ${c.cy})`;
    }
  }
  renderer?.roads.setPreview(geometry);
  toolInfo.textContent = infoText === "" ? info : infoText;
}

/**
 * 押した位置の道路の始点と終点。プレビューで最後に見せた道路と同じ条件ならそれをそのまま使い、
 * 道路網が届いて索引が変わっていても、プレビューと違う道路を送らない。
 */
function roadAt(at: Tile): { from: Tile; to: Tile } {
  const shown = shownRoad;
  if (
    shown !== null &&
    shown.noSnap === noSnap &&
    shown.cursor.x === at.x &&
    shown.cursor.y === at.y &&
    shown.start?.x === roadStart?.x &&
    shown.start?.y === roadStart?.y
  ) {
    return shown;
  }
  return roadEndpoints(roadIndex, roadStart, at, noSnap);
}

function requestProbe(service: ServiceName, at: Tile): void {
  const key = `${service},${at.x},${at.y}`;
  if (key !== probeSent) {
    probeSent = key;
    send({ kind: "probe-service", service, x: at.x, y: at.y });
  }
}

/** 置く向き。中核の答えがまだ届いていないマスでは、置けないものとして扱う。 */
function serviceChoice(service: ServiceName, at: Tile) {
  const fresh = probe?.service === service && probe.x === at.x && probe.y === at.y;
  return chooseRotation(fresh && probe !== null ? probe.results : [], manualRotation);
}

// 「見る」で押したマス。パネルを出しているあいだ、数百 ms ごとに中核へ問い合わせ直す
const INSPECT_INTERVAL_MS = 400;
// 最初はマスで引き、答えが届いたら押したものの識別子で引く。消えたら閉じる
let inspected: InspectQuery | null = null;
const inspector = setupInspector(element<HTMLElement>("#inspector"), () => {
  inspected = null;
});
function closeInspector(): void {
  inspected = null;
  inspector.close();
}
function requestInspect(): void {
  if (inspected !== null) {
    send({ kind: "inspect", query: inspected });
  }
}
setInterval(requestInspect, INSPECT_INTERVAL_MS);

function cancelTool(): void {
  roadStart = null;
  if (pointer?.mode === "area") {
    pointer = null;
  }
  devWalks.cancel();
  updatePreview();
}

function selectTool(next: Tool, button: HTMLButtonElement): void {
  tool = next;
  for (const b of toolButtons) {
    b.setAttribute("aria-pressed", String(b === button));
  }
  mapCanvas.dataset.tool = button.dataset.tool;
  manualRotation = null;
  if (renderer !== null) {
    renderer.showZonable = next.kind === "zone";
  }
  closeInspector();
  cancelTool();
}

function clickTile(at: Tile): void {
  if (tool.kind === "road" || tool.kind === "upgrade") {
    if (roadStart === null) {
      roadStart = roadAt(at).from;
    } else {
      const end = roadAt(at).to;
      if (end.x !== roadStart.x || end.y !== roadStart.y) {
        const make = tool.kind === "road" ? buildRoadCommand : upgradeRoadCommand;
        sendCommand(make(tool.road, roadStart, end));
        // CS2 と同じく、引いた終点から続けて引ける
        roadStart = end;
      }
    }
  } else if (tool.kind === "buy") {
    sendCommand(buyChunkCommand(at));
  } else if (tool.kind === "signal") {
    sendCommand(signalCommand(at, tool.on));
  } else if (tool.kind === "crosswalk") {
    // 触って操作する画面では押す前にカーソルが動かないので、押した位置から選び直す
    const arm = cursorPoint === null ? null : pickCrosswalk(crosswalks, cursorPoint, tool.on);
    if (arm !== null) {
      sendCommand(crosswalkCommand(arm, tool.on));
    }
  } else if (mapCanvas.dataset.tool === DEV_WALK_TOOL) {
    devWalks.click(at);
  } else if (tool.kind === "service") {
    // 答えの届いていないマスでも送り、置けるかは中核に決めさせる
    sendCommand(placeServiceCommand(tool.service, at, serviceChoice(tool.service, at).rotation));
  } else if (tool.kind === "none") {
    // 何もないところを押すと、答えが空で返ってきてパネルが閉じる
    inspected = { kind: "tile", x: at.x, y: at.y };
    requestInspect();
  }
}

mapCanvas.addEventListener("contextmenu", (event) => event.preventDefault());
window.addEventListener("keydown", (event) => {
  // 文字を打っているあいだは、道具のキーとして扱わない
  const target = event.target;
  if (
    target instanceof HTMLInputElement ||
    target instanceof HTMLTextAreaElement ||
    (target instanceof HTMLElement && target.isContentEditable)
  ) {
    return;
  }
  if (event.key === "Escape") {
    closeInspector();
    cancelTool();
  } else if ((event.key === "r" || event.key === "R") && tool.kind === "service") {
    const current = cursor !== null ? serviceChoice(tool.service, cursor).rotation : 0;
    manualRotation = (current + 1) % 4;
    updatePreview();
  }
});
// 押している指やボタンのポインター。2 つ目が来たら、どの操作も確定させずに取り消す
const activePointers = new Set<number>();
mapCanvas.addEventListener("pointerdown", (event) => {
  activePointers.add(event.pointerId);
  if (activePointers.size > 1) {
    pointer = null;
    updatePreview();
    return;
  }
  if (event.button === 2) {
    cancelTool();
    return;
  }
  if (event.button !== 0 && event.button !== 1) {
    return;
  }
  const area = tool.kind === "bulldoze" || tool.kind === "zone";
  const mode = event.button === 1 ? "pan" : area ? "area" : "click";
  pointer = { id: event.pointerId, x: event.clientX, y: event.clientY, mode, start: tileAt(event) };
  cursorPoint = pointAt(event);
  mapCanvas.setPointerCapture(event.pointerId);
  updatePreview();
});
mapCanvas.addEventListener("pointermove", (event) => {
  const at = tileAt(event);
  cursorPoint = pointAt(event);
  const picked =
    tool.kind === "crosswalk" && pickCrosswalk(crosswalks, cursorPoint, tool.on) !== shownCrosswalk;
  if (cursor?.x !== at.x || cursor?.y !== at.y || noSnap !== event.altKey || picked) {
    noSnap = event.altKey;
    cursor = at;
    updatePreview();
  }
  if (pointer?.id !== event.pointerId) {
    return;
  }
  const dx = event.clientX - pointer.x;
  const dy = event.clientY - pointer.y;
  if (pointer.mode === "click" && Math.hypot(dx, dy) > CLICK_SLOP_PX) {
    pointer.mode = "pan";
  }
  if (pointer.mode === "pan") {
    camera.pan(dx, dy);
    pointer.x = event.clientX;
    pointer.y = event.clientY;
  }
});
mapCanvas.addEventListener("pointerup", (event) => {
  activePointers.delete(event.pointerId);
  if (pointer?.id !== event.pointerId) {
    return;
  }
  const done = pointer;
  pointer = null;
  noSnap = event.altKey;
  if (done.mode === "click") {
    cursorPoint = pointAt(event);
    clickTile(tileAt(event));
  } else if (done.mode === "area") {
    const end = tileAt(event);
    sendCommand(
      tool.kind === "zone"
        ? paintZoneCommand(tool.zone, done.start, clampArea(done.start, end, MAX_ZONE_SIDE))
        : bulldozeCommand(done.start, end),
    );
  }
  updatePreview();
});
mapCanvas.addEventListener("pointercancel", (event) => {
  activePointers.delete(event.pointerId);
  if (pointer?.id === event.pointerId) {
    pointer = null;
    updatePreview();
  }
});
mapCanvas.addEventListener(
  "wheel",
  (event) => {
    event.preventDefault();
    const pixels =
      event.deltaMode === WheelEvent.DOM_DELTA_LINE
        ? WHEEL_LINE_PIXELS
        : event.deltaMode === WheelEvent.DOM_DELTA_PAGE
          ? mapCanvas.clientHeight
          : 1;
    const rect = mapCanvas.getBoundingClientRect();
    camera.zoomAt(
      event.clientX - rect.left,
      event.clientY - rect.top,
      Math.exp(-event.deltaY * pixels * WHEEL_ZOOM_RATE),
      viewport(),
    );
  },
  { passive: false },
);

let paused = false;
function setPaused(next: boolean): void {
  paused = next;
  pauseButton.textContent = paused ? "再開" : "一時停止";
  send({ kind: "pause", paused });
}
pauseButton.addEventListener("click", () => setPaused(!paused));

// Worker に求めたフィードバックの状態。届いたら解決し、失敗の返事なら断る
type FeedbackState = { state: string; commands: ArrayBuffer };
let feedbackWaiter: { resolve: (v: FeedbackState) => void; reject: (e: Error) => void } | null =
  null;
setupFeedback({
  isPaused: () => paused,
  setPaused,
  requestState: () =>
    new Promise((resolve, reject) => {
      feedbackWaiter = { resolve, reject };
      send({ kind: "feedback" });
    }),
  map: mapCanvas,
  camera: () => ({ x: camera.x, y: camera.y, zoom: camera.zoom }),
});

const devWalks = setupDevWalks({
  button: element<HTMLButtonElement>(`button[data-tool="${DEV_WALK_TOOL}"]`),
  info: element<HTMLElement>("#dev-info"),
  flag: element<HTMLElement>("#dev-flag"),
  map: mapCanvas,
  send,
});

for (const button of toolButtons) {
  button.addEventListener("click", () => {
    const name = button.dataset.tool ?? "none";
    const road = name.startsWith("road-") ? (name.slice(5) as RoadKindName) : null;
    const upgrade = name.startsWith("upgrade-") ? (name.slice(8) as RoadKindName) : null;
    const zone = name.startsWith("zone-") ? (name.slice(5) as ZoneName) : null;
    const service = name.startsWith("service-") ? (name.slice(8) as ServiceName) : null;
    const next: Tool =
      road !== null
        ? { kind: "road", road }
        : upgrade !== null
          ? { kind: "upgrade", road: upgrade }
          : zone !== null
            ? { kind: "zone", zone }
            : service !== null
              ? { kind: "service", service }
              : name === "signal-on" || name === "signal-off"
                ? { kind: "signal", on: name === "signal-on" }
                : name === "crosswalk-on" || name === "crosswalk-off"
                  ? { kind: "crosswalk", on: name === "crosswalk-on" }
                  : name === "bulldoze" || name === "buy"
                    ? { kind: name }
                    : { kind: "none" };
    selectTool(next, button);
  });
}

const DONE: Record<CommandMessage["kind"], string> = {
  BuildRoad: "道路を引いた",
  UpgradeRoad: "道路の種類を変えた",
  Bulldoze: "取り壊した",
  BuyChunk: "土地を買った",
  PaintZone: "区画を塗った",
  PlaceBuilding: "施設を置いた",
  SetSignal: "信号を切り替えた",
  SetCrosswalk: "横断歩道を切り替えた",
  SetSimSpeed: "速度を変えた",
  SetTaxRate: "税率を変えた",
};
function showOutcome(command: CommandMessage["kind"], outcome: Outcome, reason?: string): void {
  const text: Record<Outcome, string> = {
    Applied: DONE[command],
    Denied: `できなかった：${reason ?? ""}`,
    Rejected: `受け付けなかった：${reason ?? ""}`,
    NotImplemented: "まだ使えない",
  };
  toolMessage.textContent = text[outcome];
  // test:wasm が読む
  toolMessage.dataset.command = command;
  toolMessage.dataset.outcome = outcome;
  toolMessage.dataset.count = String(Number(toolMessage.dataset.count ?? 0) + 1);
}

for (const button of document.querySelectorAll<HTMLButtonElement>("button[data-speed]")) {
  button.addEventListener("click", () => {
    const { core, pace: next } = splitSpeed(Number(button.dataset.speed));
    send({ kind: "pace", pace: next });
    sendCommand({ kind: "SetSimSpeed", speed: core });
  });
}

worker.addEventListener("message", (event: MessageEvent<unknown>) => {
  const message = event.data;
  if (!isWorkerToMain(message)) {
    return;
  }
  if (message.kind === "pace") {
    workerPace = message.pace;
    return;
  }
  if (message.kind === "chunks") {
    for (const chunk of store.receive(message.chunks)) {
      renderer?.upload(chunk);
    }
    return;
  }
  if (message.kind === "layer-chunks") {
    if (activeLayers.includes(message.layer)) {
      // 画面が捨てた直後に届いたチャンクは、手元の地形と一緒に捨てる
      for (const c of message.chunks.filter((c) => store.get(c.cx, c.cy) !== undefined)) {
        renderer?.layers.upload(message.layer, c.cx, c.cy, c.version, new Uint8Array(c.values));
      }
    }
    return;
  }
  if (message.kind === "roads") {
    roads = roadMeshes(message.segments, message.nodes);
    roadIndex = buildRoadIndex(message.segments, message.nodes);
    roadSegments = message.segments;
    crosswalks = crosswalkArms(message.segments, message.nodes);
    // test:wasm が読む
    mapCanvas.dataset.removedCrosswalks = String(crosswalks.filter((arm) => arm.removed).length);
    updatePreview();
    renderer?.roads.setRoads(roads);
    // test:wasm が読む
    mapCanvas.dataset.roadRevision = String(message.revision);
    mapCanvas.dataset.roadSegments = String(message.segments.length / SEGMENT_STRIDE);
    return;
  }
  if (message.kind === "signals") {
    signals = signalGeometry(message.lights);
    renderer?.roads.setSignals(signals);
    // test:wasm が読む
    mapCanvas.dataset.signalArms = String(message.lights.length / SIGNAL_STRIDE);
    return;
  }
  if (message.kind === "buildings") {
    buildings = buildingGeometry(message.buildings, message.facilities);
    renderer?.roads.setBuildings(buildings);
    renderCityStats(parseCityStats(message.stats), unsuppliedCount(message.buildings));
    // test:wasm が読む
    mapCanvas.dataset.firstBuilding = Array.from(message.buildings.subarray(0, 4)).join(",");
    mapCanvas.dataset.unsupplied = String(unsuppliedCount(message.buildings));
    mapCanvas.dataset.facilities = String(message.facilities.length / FACILITY_STRIDE);
    return;
  }
  if (message.kind === "finance") {
    finance = parseFinance(message.values);
    if (finance !== null) {
      financePanel.update(finance);
    }
    return;
  }
  if (message.kind === "overview") {
    renderOverview(parseOverview(message.overview));
    growthNotice.render(parseGrowthBlocks(message.overview), parseStalls(message.stalls));
    return;
  }
  if (message.kind === "outcome") {
    const command = sentCommands.get(message.id);
    sentCommands.delete(message.id);
    // 街が変わったので、施設を置けるかを問い合わせ直す
    probeSent = "";
    if (tool.kind === "service") {
      updatePreview();
    }
    if (command === "SetTaxRate" && message.outcome !== "Applied") {
      financePanel.rejected();
    }
    if (command !== undefined && command !== "SetSimSpeed") {
      showOutcome(command, message.outcome, message.reason);
    }
    return;
  }
  if (message.kind === "feedback") {
    feedbackWaiter?.resolve({ state: message.state, commands: message.commands });
    feedbackWaiter = null;
    return;
  }
  if (message.kind === "service-probe") {
    probe = message;
    updatePreview();
    return;
  }
  if (message.kind === "inspect") {
    // 押し直したあとに、前の問い合わせの答えが遅れて届くことがある
    const result = applyInspectReply(inspected, message.query, message.info);
    if (!result.stale) {
      inspected = result.query;
      inspector.show(result.view);
    }
    return;
  }
  if (message.kind === "feedback-error") {
    feedbackWaiter?.reject(new Error(message.message));
    feedbackWaiter = null;
    return;
  }
  if (message.kind === "dev-spawned") {
    devWalks.spawned(message.count);
    return;
  }
  if (message.kind === "error") {
    status.textContent = `WASM を動かせなかった：${message.message}`;
    status.dataset.state = "error";
    return;
  }
  roadCosts = parseRoadCostTable(message.roadCosts);
  services = parseServiceTable(message.serviceTable);
  let reader: SnapshotReader;
  try {
    reader = new SnapshotReader(message.memory, message.snapshotOffset);
  } catch (error) {
    status.textContent = `三重バッファを読めない：${String(error)}`;
    status.dataset.state = "error";
    return;
  }
  status.textContent = `動いている（スレッド ${message.threads} 本）${mapError === null ? "" : `。${mapError}`}`;
  status.dataset.state = "running";
  status.dataset.n = String(message.n);
  status.dataset.sum = String(message.sum);
  status.dataset.threads = String(message.threads);
  status.dataset.workerIsolated = String(message.crossOriginIsolated);
  startDrawing(reader);
});

worker.addEventListener("error", (event) => {
  status.textContent = `Worker を起こせなかった：${event.message}`;
  status.dataset.state = "error";
});
