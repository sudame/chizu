import { parseCityStats, renderCityStats } from "./city-panel.ts";
import { DEV_WALK_TOOL, setupDevWalks } from "./dev-walks.ts";
import { setupFeedback } from "./feedback.ts";
import { Camera, type ChunkRange, chunksByDistance, type Viewport } from "./map/camera.ts";
import { ChunkStore } from "./map/chunk-sync.ts";
import { moverArea } from "./map/movers.ts";
import { MapRenderer } from "./map/renderer.ts";
import {
  buildingGeometry,
  type Color,
  FACILITY_COLOR,
  Geometry,
  halfWidthOf,
  roadGeometry,
  unsuppliedCount,
} from "./map/roads.ts";
import {
  buildRoadCommand,
  bulldozeCommand,
  buyChunkCommand,
  chooseRotation,
  clampArea,
  MAX_ZONE_SIDE,
  paintZoneCommand,
  parseRoadCostTable,
  parseServiceTable,
  placeServiceCommand,
  type RoadCostTable,
  roadCost,
  type ServiceInfo,
  type ServiceProbe,
  screenToTile,
  serviceFootprint,
  snapToEightDirections,
  type Tile,
  type Tool,
} from "./map/tools.ts";
import {
  CHUNK_SIDE,
  type CommandMessage,
  FACILITY_STRIDE,
  isWorkerToMain,
  type MainToWorker,
  type Outcome,
  type RoadKindName,
  SEGMENT_STRIDE,
  type ServiceName,
  type ZoneName,
} from "./protocol/messages.ts";
import { type SnapshotHeader, SnapshotReader } from "./protocol/snapshot.ts";

function element<T extends Element>(selector: string): T {
  const found = document.querySelector<T>(selector);
  if (found === null) {
    throw new Error(`${selector} が見つからない`);
  }
  return found;
}

const status = element<HTMLElement>("#status");
const canvas = element<HTMLCanvasElement>("#hud");
const mapCanvas = element<HTMLCanvasElement>("#map");
const pauseButton = element<HTMLButtonElement>("#pause");
const toolInfo = element<HTMLElement>("#tool-info");
const toolMessage = element<HTMLElement>("#tool-message");
const toolButtons = [...document.querySelectorAll<HTMLButtonElement>("button[data-tool]")];
const context = canvas.getContext("2d");
if (context === null) {
  throw new Error("canvas の 2D の描画を使えない");
}

const worker = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });

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

const pad = (n: number) => String(n).padStart(2, "0");

function describeClock(header: SnapshotHeader): string {
  const hour = Math.floor(header.secondOfDay / 3600);
  const minute = Math.floor(header.secondOfDay / 60) % 60;
  return `${header.day + 1} 日目 ${pad(hour)}:${pad(minute)}`;
}

function draw(ctx: CanvasRenderingContext2D, header: SnapshotHeader): void {
  ctx.clearRect(0, 0, canvas.width, canvas.height);
  ctx.fillStyle = "#222";
  ctx.font = "20px sans-serif";
  ctx.fillText(describeClock(header), 12, 30);
  ctx.fillText(`お金 ${header.money.toLocaleString("ja-JP")}`, 12, 58);
  ctx.fillText(header.paused ? "一時停止中" : `速度 ${header.speed} 倍`, 12, 86);
  // test:wasm が読む
  canvas.dataset.tick = String(header.tick);
  canvas.dataset.speed = String(header.speed);
  canvas.dataset.paused = String(header.paused);
  canvas.dataset.day = String(header.day);
  canvas.dataset.secondOfDay = String(header.secondOfDay);
  canvas.dataset.money = String(header.money);
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
let roads: Geometry | null = null;
let buildings: Geometry | null = null;

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
      : (renderer?.movers.draw(camera, vp, header, reader.movers(header.moverCount)) ?? 0);
  // test:wasm が読む
  mapCanvas.dataset.drawnMovers = String(movers);
  const first = renderer?.movers.first;
  mapCanvas.dataset.firstMover = first ? `${first.x},${first.y}` : "";
  mapCanvas.dataset.drawnChunks = String(drawn);
  mapCanvas.dataset.visibleChunks = String(
    (range.maxCx - range.minCx + 1) * (range.maxCy - range.minCy + 1),
  );
  mapCanvas.dataset.storedChunks = String(store.size);
  mapCanvas.dataset.cameraX = String(camera.x);
  mapCanvas.dataset.cameraY = String(camera.y);
  mapCanvas.dataset.zoom = String(camera.zoom);
}

function startDrawing(reader: SnapshotReader, ctx: CanvasRenderingContext2D): void {
  const frame = () => {
    const header = reader.acquire();
    if (header !== null) {
      draw(ctx, header);
    }
    drawMap(reader, header);
    requestAnimationFrame(frame);
  };
  requestAnimationFrame(frame);
}

// 左のボタンは、この距離（CSS ピクセル）より動かしたら地図を引きずる操作とみなす。中のボタンはいつも引きずる
const CLICK_SLOP_PX = 4;
const ROAD_CODES: Record<RoadKindName, number> = { Small: 1, Medium: 2, Large: 3 };
const PREVIEW_OK: Color = [0.3, 0.6, 1, 0.7];
const PREVIEW_BAD: Color = [1, 0.3, 0.3, 0.7];
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
let cursor: Tile | null = null;
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

// 道具の仮の表示と、道路の長さと費用の見積もりを書き直す
function updatePreview(): void {
  let geometry: Geometry | null = null;
  let info = "";
  let infoText = "";
  if (tool.kind === "road" && cursor !== null) {
    const from = roadStart ?? cursor;
    const to = roadStart === null ? cursor : snapToEightDirections(roadStart, cursor);
    geometry = new Geometry(from);
    const half = halfWidthOf(ROAD_CODES[tool.road]);
    if (roadStart !== null && roadCosts !== null) {
      const cost = roadCost(roadCosts, tool.road, from, to);
      const length = Math.max(Math.abs(to.x - from.x), Math.abs(to.y - from.y));
      const color = cost > money ? PREVIEW_BAD : PREVIEW_OK;
      geometry.segment(from, to, half, color);
      info = `長さ ${length} マス・費用 ${cost.toLocaleString("ja-JP")}`;
      mapCanvas.dataset.previewCost = String(cost);
    }
    geometry.square(from, half, PREVIEW_OK);
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
  cancelTool();
}

function clickTile(at: Tile): void {
  if (tool.kind === "road") {
    if (roadStart === null) {
      roadStart = at;
    } else {
      const end = snapToEightDirections(roadStart, at);
      if (end.x !== roadStart.x || end.y !== roadStart.y) {
        sendCommand(buildRoadCommand(tool.road, roadStart, end));
        // CS2 と同じく、引いた終点から続けて引ける
        roadStart = end;
      }
    }
  } else if (tool.kind === "buy") {
    sendCommand(buyChunkCommand(at));
  } else if (mapCanvas.dataset.tool === DEV_WALK_TOOL) {
    devWalks.click(at);
  } else if (tool.kind === "service") {
    // 答えの届いていないマスでも送り、置けるかは中核に決めさせる
    sendCommand(placeServiceCommand(tool.service, at, serviceChoice(tool.service, at).rotation));
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
  mapCanvas.setPointerCapture(event.pointerId);
  updatePreview();
});
mapCanvas.addEventListener("pointermove", (event) => {
  const at = tileAt(event);
  if (cursor?.x !== at.x || cursor?.y !== at.y) {
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
  if (done.mode === "click") {
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
    const zone = name.startsWith("zone-") ? (name.slice(5) as ZoneName) : null;
    const service = name.startsWith("service-") ? (name.slice(8) as ServiceName) : null;
    const next: Tool =
      road !== null
        ? { kind: "road", road }
        : zone !== null
          ? { kind: "zone", zone }
          : service !== null
            ? { kind: "service", service }
            : name === "bulldoze" || name === "buy"
              ? { kind: name }
              : { kind: "none" };
    selectTool(next, button);
  });
}

const DONE: Record<CommandMessage["kind"], string> = {
  BuildRoad: "道路を引いた",
  Bulldoze: "取り壊した",
  BuyChunk: "土地を買った",
  PaintZone: "区画を塗った",
  PlaceBuilding: "施設を置いた",
  SetSimSpeed: "速度を変えた",
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
    sendCommand({ kind: "SetSimSpeed", speed: Number(button.dataset.speed) });
  });
}

worker.addEventListener("message", (event: MessageEvent<unknown>) => {
  const message = event.data;
  if (!isWorkerToMain(message)) {
    return;
  }
  if (message.kind === "chunks") {
    for (const chunk of store.receive(message.chunks)) {
      renderer?.upload(chunk);
    }
    return;
  }
  if (message.kind === "roads") {
    roads = roadGeometry(message.segments, message.nodes);
    renderer?.roads.setRoads(roads);
    // test:wasm が読む
    mapCanvas.dataset.roadRevision = String(message.revision);
    mapCanvas.dataset.roadSegments = String(message.segments.length / SEGMENT_STRIDE);
    return;
  }
  if (message.kind === "buildings") {
    buildings = buildingGeometry(message.buildings, message.facilities);
    renderer?.roads.setBuildings(buildings);
    renderCityStats(parseCityStats(message.stats));
    // test:wasm が読む
    mapCanvas.dataset.firstBuilding = Array.from(message.buildings.subarray(0, 4)).join(",");
    mapCanvas.dataset.unsupplied = String(unsuppliedCount(message.buildings));
    mapCanvas.dataset.facilities = String(message.facilities.length / FACILITY_STRIDE);
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
  startDrawing(reader, context);
});

worker.addEventListener("error", (event) => {
  status.textContent = `Worker を起こせなかった：${event.message}`;
  status.dataset.state = "error";
});
