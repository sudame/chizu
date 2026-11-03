import { demandBar, parseCityStats, renderCityStats, setupDemandFactors } from "./city-panel.ts";
import { DEV_WALK_TOOL, setupDevWalks } from "./dev-walks.ts";
import { handlesEscape, nextEscapeStep } from "./escape.ts";
import { setupFeedback } from "./feedback.ts";
import {
  balanceNote,
  deficitNote,
  type Finance,
  type FinancePanel,
  parseFinance,
  setupFinance,
} from "./finance.ts";
import {
  type LotShortageSince,
  parseGrowthBlocks,
  parseIsolated,
  parseStalls,
  persistedLotShortage,
  setupGrowthNotice,
  trackLotShortage,
} from "./growth-notice.ts";
import { setupHudPanel, trackHeight } from "./hud-panel.ts";
import { applyInspectReply, setupInspector } from "./inspector.ts";
import { renderLegend } from "./legend.ts";
import {
  Camera,
  type ChunkRange,
  chunksByDistance,
  MAX_ZOOM,
  MIN_ZOOM,
  type Viewport,
} from "./map/camera.ts";
import { ChunkStore } from "./map/chunk-sync.ts";
import { DetailCache } from "./map/detail.ts";
import { LabelLayer } from "./map/label-layer.ts";
import { KIND_CAR, KIND_WALK, moverArea } from "./map/movers.ts";
import {
  BULLDOZE_EDGE_COLOR,
  BULLDOZE_FILL_COLOR,
  CLEAR_ZONE_COLOR,
  HINT_EDGE_COLOR,
  HINT_FILL_COLOR,
  PREVIEW_BAD_COLOR,
  PREVIEW_BAD_EDGE_COLOR,
  PREVIEW_LINE_COLOR,
  PREVIEW_OK_COLOR,
  PREVIEW_OK_EDGE_COLOR,
  PREVIEW_ROAD_FILL_ALPHA,
} from "./map/palette.ts";
import { MapRenderer } from "./map/renderer.ts";
import {
  type BuildingMeshes,
  buildingEdgeColor,
  buildingMeshes,
  type Color,
  type CrosswalkArm,
  crosswalkArms,
  facilityCircleColor,
  footprintOutline,
  Geometry,
  MIN_PX,
  ROAD_VISUAL_HALF,
  type RoadMeshes,
  roadFootprint,
  roadMeshes,
  roadStyle,
  signalGeometry,
  unsuppliedCount,
  zoneBlockColor,
} from "./map/roads.ts";
import {
  buildRoadIndex,
  DISCONNECTED_NOTICE,
  EMPTY_ROAD_INDEX,
  isDisconnected,
  type RoadIndex,
  roadEndpoints,
} from "./map/snap.ts";
import { showsFine } from "./map/stages.ts";
import {
  loadTheme,
  renderThemeLegend,
  saveTheme,
  setupThemePanel,
  type ThemeId,
  themeLegend,
  themeOf,
  themeStorage,
} from "./map/themes.ts";
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
import { TrafficLines } from "./map/traffic.ts";
import { parseOverview, renderOverview } from "./overview.ts";
import { createPopovers } from "./popover.ts";
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
import { setupScaleBar } from "./scale-bar.ts";
import { BAR_SPEEDS, describeSpeed, nearestSpeed } from "./speed.ts";

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
const speedButtons = [...document.querySelectorAll<HTMLButtonElement>("button[data-speed]")];
const hudSpeed = element<HTMLElement>("#hud-speed");
const mapCanvas = element<HTMLCanvasElement>("#map");
renderLegend(element<HTMLElement>("#legend-body"));
const pauseButton = element<HTMLButtonElement>("#pause");
const toolInfo = element<HTMLElement>("#tool-info");
const toolMessage = element<HTMLElement>("#tool-message");
const toolButtons = [...document.querySelectorAll<HTMLButtonElement>("button[data-tool]")];

const worker = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
setupSaves(worker, element<HTMLElement>("#controls"));
setupDemandFactors();

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

// 読み込んだ記録の速さが段にないとき（前の帯の 4 倍や 16 倍など）、最初の枠で近い段にそろえる。
// SetSimSpeed のコマンドで変えるので、記録にも残る
let speedAligned = false;
function alignSpeed(speed: number): void {
  if (speedAligned || speed < 1) {
    return;
  }
  speedAligned = true;
  const next = nearestSpeed(speed);
  if (next !== speed) {
    sendCommand({ kind: "SetSimSpeed", speed: next });
  }
}

function draw(header: SnapshotHeader): void {
  alignSpeed(header.speed);
  setText(hudClock, describeClock(header));
  setText(hudMoney, `お金 ${header.money.toLocaleString("ja-JP")}`);
  // 収支が赤字のあいだは、所持金の横に知らせる
  const note = deficitNote(finance);
  const balance = balanceNote(finance);
  setText(hudDeficit, balance);
  hudDeficit.hidden = balance === "";
  hudDeficit.dataset.deficit = String(note !== "");
  const speed = header.speed;
  setText(hudSpeed, header.paused ? "一時停止中" : describeSpeed(speed));
  // 帯のボタンに出ていない速度（288 倍）のときと、止めているときだけ、速度の文字を出す
  hudSpeed.hidden = !header.paused && (BAR_SPEEDS as readonly number[]).includes(speed);
  for (const b of speedButtons) {
    b.setAttribute("aria-pressed", String(!header.paused && Number(b.dataset.speed) === speed));
  }
  pauseButton.setAttribute("aria-pressed", String(header.paused));
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
// 道路名、地区名、施設名。WebGL2 とは別の Canvas 2D に描くので、地図を描けなくても作る
const labels = new LabelLayer(element<HTMLCanvasElement>("#labels"));
// 最後に届いた道路網と建物。コンテキストを失ったあとに貼り直す
let roads: RoadMeshes | null = null;
let buildings: BuildingMeshes | null = null;
let signals: Geometry | null = null;
// 細部の段で描き足すもの。細部の段に入っているときだけ、見えている範囲のぶんを作る
const detail = new DetailCache();
// 主題図。選んだものはブラウザに覚えさせ、次に開いたときも同じものにする
let theme: ThemeId = loadTheme(themeStorage());
const layersOf = (id: ThemeId): number[] => {
  const layer = themeOf(id).layer;
  return layer === null ? [] : [layer];
};
let activeLayers: number[] = layersOf(theme);
// 交通の主題図の線。区間ごとの値か道路網が変わったときだけ作り直す
const trafficLines = new TrafficLines();
let trafficBuilds = 0;
// 需要の主題図と凡例に使う、住宅・商業・工業・オフィスの需要（街の数字の値）
let demand: number[] | null = null;
// 電気か水道が届いていない建物の数。街の数字だけを送り直されたときの表示に使う
let unsupplied = 0;
// 主題図の凡例。レイヤーのボタンの左に浮かべるものと、レイヤーのパネルの中のもの
const themeLegendElements = [
  element<HTMLElement>("#theme-legend"),
  element<HTMLElement>("#layer-theme-legend"),
];
let themeLegendShown = "";
setupHudPanel(element<HTMLButtonElement>("#hud-toggle"), element<HTMLElement>("#hud-details"));
// 道具の帯と、その上の見積もりと 2 段目の高さ。狭い画面で、下の知らせをその上へ積むのに使う
trackHeight(element<HTMLElement>("#toolstack"), "--toolbar-h");
// 道具の帯だけの高さ。狭い画面で、左下と右下の部品を帯の上へ上げるのに使う
trackHeight(element<HTMLElement>("#toolbar"), "--bar-h");
trackHeight(element<HTMLElement>("#hudbar"), "--hudbar-h");
trackHeight(element<HTMLElement>("#map-meta"), "--meta-h");
const updateScale = setupScaleBar(element<HTMLElement>("#scale"));
const popovers = createPopovers();
for (const toggle of document.querySelectorAll<HTMLButtonElement>("button[data-menu]")) {
  popovers.add(toggle, element<HTMLElement>(`#menu-${toggle.dataset.menu}`));
}
popovers.add(element<HTMLElement>("#layers-open"), element<HTMLElement>("#layer-panel"));
popovers.add(element<HTMLElement>("#finance-toggle"), element<HTMLElement>("#finance"));
const toolGroups = [...document.querySelectorAll<HTMLButtonElement>("button[data-menu]")].map(
  (toggle) => ({ toggle, menu: element<HTMLElement>(`#menu-${toggle.dataset.menu}`) }),
);
// 道具の 2 段目を開いているあいだは、狭い画面で下の隅の部品が 2 段目に重なるので、CSS で隠す
const syncMenuOpen = () => {
  document.body.toggleAttribute(
    "data-menu-open",
    toolGroups.some(({ menu }) => !menu.hidden),
  );
};
for (const { menu } of toolGroups) {
  new MutationObserver(syncMenuOpen).observe(menu, {
    attributes: true,
    attributeFilter: ["hidden"],
  });
}
// レイヤーのパネルを開いているあいだも、狭い画面で左下の部品と知らせが重なるので、CSS で隠す
const layerPanel = element<HTMLElement>("#layer-panel");
new MutationObserver(() => {
  document.body.toggleAttribute("data-layers-open", !layerPanel.hidden);
}).observe(layerPanel, { attributes: true, attributeFilter: ["hidden"] });
const zoomInButton = element<HTMLButtonElement>("#zoom-in");
const zoomOutButton = element<HTMLButtonElement>("#zoom-out");
/** 主題図を画面と Worker に伝える。 */
function applyTheme(): void {
  activeLayers = layersOf(theme);
  renderer?.layers.setActive(activeLayers);
  send({ kind: "layers", active: activeLayers });
  trafficLines.clear();
  updateDemandTheme();
  updateThemeLegend();
}
function updateDemandTheme(): void {
  if (renderer !== null) {
    renderer.demand = theme === "demand" ? (demand ?? [0, 0, 0, 0]).map(demandBar) : null;
  }
}
setupThemePanel(element<HTMLElement>("#layers"), theme, (id) => {
  theme = id;
  saveTheme(themeStorage(), id);
  applyTheme();
});
applyTheme();

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
  detail.invalidate();
  applyTheme();
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
  rebuildTraffic();
  rebuildDetail(vp);
  updateThemeLegend();
  const drawn = renderer?.draw(camera, vp, range) ?? 0;
  labels.frame(camera, vp, performance.now());
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
  updateScale(camera.zoom);
  zoomInButton.disabled = camera.zoom >= MAX_ZOOM;
  zoomOutButton.disabled = camera.zoom <= MIN_ZOOM;
}

/** 細部の段に入っていて、画面が前に作った範囲を出たか、形が変わっていれば、細部の頂点を作り直す。 */
function rebuildDetail(vp: Viewport): void {
  const min = camera.screenToWorld(0, 0, vp);
  const max = camera.screenToWorld(vp.width, vp.height, vp);
  const meshes = detail.update(camera.zoom, {
    minX: min.x,
    minY: min.y,
    maxX: max.x,
    maxY: max.y,
  });
  if (meshes === null) {
    return;
  }
  renderer?.roads.setDetail(meshes);
  // test:wasm が読む
  mapCanvas.dataset.detailBuilds = String(detail.builds);
  if (meshes.roofs !== undefined) {
    mapCanvas.dataset.detailRoofs = String(meshes.roofs.vertexCount);
  }
  if (meshes.streets !== undefined) {
    mapCanvas.dataset.detailStreets = String(meshes.streets.vertexCount);
  }
  if (meshes.lamps !== undefined) {
    mapCanvas.dataset.detailLamps = String(meshes.lamps.vertexCount);
  }
}

/** 交通の値か道路網が変わっていれば、交通の主題図の線を作り直す。毎フレームは作らない。 */
function rebuildTraffic(): void {
  const meshes = trafficLines.take(showsFine(camera.zoom));
  if (meshes === undefined) {
    return;
  }
  renderer?.roads.setTraffic(meshes);
  if (meshes !== null) {
    trafficBuilds++;
    // test:wasm が読む
    mapCanvas.dataset.trafficBuilds = String(trafficBuilds);
  }
}

/** 主題図の凡例を、中身が変わったときだけ描き直す。 */
function updateThemeLegend(): void {
  const items = themeLegend(theme, camera.zoom, demand);
  const key = `${theme}:${JSON.stringify(items)}`;
  if (key === themeLegendShown) {
    return;
  }
  themeLegendShown = key;
  for (const container of themeLegendElements) {
    renderThemeLegend(container, items);
    // test:wasm が読む
    container.dataset.theme = theme;
  }
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
// 区画を塗る長方形の色。街区の面・建物の色の 70% に、建物の縁の色の縁を付ける
const ZONE_CODE: Partial<Record<ZoneName, number>> = {
  Residential: 1,
  Commercial: 2,
  Industrial: 3,
  Office: 4,
};
const zoneArea = (zone: ZoneName): { fill: Color; edge: Color } => {
  const code = ZONE_CODE[zone];
  return code === undefined
    ? { fill: CLEAR_ZONE_COLOR, edge: CLEAR_ZONE_COLOR }
    : { fill: zoneBlockColor(code), edge: buildingEdgeColor(code) };
};
// 縁の太さと、破線の周期（画面のピクセル）
const AREA_EDGE_PX = 1.5;
const BUY_EDGE_PX = 2;
const DASH_PX = 8;

const ROAD_CODE: Record<RoadKindName, number> = { Small: 1, Medium: 2, Large: 3 };

/**
 * 道路を引くときの仮の表示。置けるときは、選んだ種類の塗りの 70% に青の縁、
 * 置けないときは、赤の 35% の塗りに赤の破線の縁にする。
 */
function roadPreview(geometry: Geometry, from: Tile, to: Tile, road: RoadKindName, ok: boolean) {
  const [r, g, b] = roadStyle(ROAD_CODE[road]).asphalt;
  roadFootprint(geometry, from, to, ok ? [r, g, b, PREVIEW_ROAD_FILL_ALPHA] : PREVIEW_BAD_COLOR, 0);
  footprintOutline(
    geometry,
    from,
    to,
    ok ? PREVIEW_OK_EDGE_COLOR : PREVIEW_BAD_EDGE_COLOR,
    AREA_EDGE_PX,
    ok ? 0 : DASH_PX,
  );
}
// 横断歩道の道具で変えられる腕の目印と、いま押すと変わる腕の印。置ける仮の表示の色を使う
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
// 空き区画がない状態が続いた長さは、Worker の tick で数える（描画は背景のタブで止まる）
let lotShortageSince: LotShortageSince = [];
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
      roadFootprint(geometry, from, to, PREVIEW_LINE_COLOR, MIN_PX);
      const ok = !(short || plan.segments.length === 0);
      for (const s of plan.segments) {
        roadPreview(geometry, s.from, s.to, tool.road, ok);
      }
      info =
        plan.roads === 0
          ? "変えられる道路がない（高速道路は変えない）"
          : plan.segments.length === 0
            ? "道路はもうその種類になっている"
            : `区間 ${plan.segments.length} 本・費用 ${plan.cost.toLocaleString("ja-JP")}`;
      mapCanvas.dataset.previewCost = String(plan.cost);
    }
    geometry.square(from, ROAD_VISUAL_HALF, PREVIEW_OK_COLOR, MIN_PX);
  } else if (tool.kind === "road" && cursor !== null) {
    const { from, to } = roadEndpoints(roadIndex, roadStart, cursor, noSnap);
    shownRoad = { start: roadStart, cursor, noSnap, from, to };
    geometry = new Geometry(from);
    // 置いたあとの道路と同じく、通るマスをマスいっぱいに塗る
    if (roadStart !== null && roadCosts !== null) {
      const cost = roadCost(roadCosts, tool.road, from, to);
      const length = Math.max(Math.abs(to.x - from.x), Math.abs(to.y - from.y));
      roadPreview(geometry, from, to, tool.road, cost <= money);
      info = `長さ ${length} マス・費用 ${cost.toLocaleString("ja-JP")}`;
      if (isDisconnected(roadIndex, from, to)) {
        info += `・${DISCONNECTED_NOTICE}`;
      }
      mapCanvas.dataset.previewCost = String(cost);
    }
    geometry.square(from, ROAD_VISUAL_HALF, PREVIEW_OK_COLOR, MIN_PX);
  } else if ((tool.kind === "bulldoze" || tool.kind === "zone") && cursor !== null) {
    const start = pointer?.mode === "area" ? pointer.start : cursor;
    const end = tool.kind === "zone" ? clampArea(start, cursor, MAX_ZONE_SIDE) : cursor;
    geometry = new Geometry(start);
    const area = tool.kind === "zone" ? zoneArea(tool.zone) : null;
    const min = { x: Math.min(start.x, end.x), y: Math.min(start.y, end.y) };
    const max = { x: Math.max(start.x, end.x), y: Math.max(start.y, end.y) };
    geometry.rect(min, max, area?.fill ?? BULLDOZE_FILL_COLOR);
    geometry.outline(
      min.x,
      min.y,
      max.x + 1,
      max.y + 1,
      area?.edge ?? BULLDOZE_EDGE_COLOR,
      AREA_EDGE_PX,
    );
  } else if (tool.kind === "service" && cursor !== null && services !== null) {
    const info = services[tool.service];
    requestProbe(tool.service, cursor);
    const choice = serviceChoice(tool.service, cursor);
    const { min, max } = serviceFootprint(cursor, choice.rotation, info.width, info.depth);
    geometry = new Geometry(min);
    const base = facilityCircleColor(tool.service === "PowerPlant" ? 1 : 2);
    const color: Color = choice.placeable ? [base[0], base[1], base[2], 0.7] : PREVIEW_BAD_COLOR;
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
      geometry.dot(arm.cx, arm.cy, CROSSWALK_HALF, PREVIEW_OK_COLOR, MIN_PX);
    }
    shownCrosswalk = cursorPoint === null ? null : pickCrosswalk(crosswalks, cursorPoint, on);
    if (shownCrosswalk !== null) {
      const { cx, cy } = shownCrosswalk;
      geometry.dot(cx, cy, CROSSWALK_PICKED_HALF, PREVIEW_OK_EDGE_COLOR, MIN_PX);
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
    geometry.rect(cursor, cursor, HINT_FILL_COLOR);
    info = tool.on
      ? "3 本以上の道路が集まる交差点を押して信号を置く"
      : "信号のある交差点を押して外す";
  } else if (tool.kind === "buy" && cursor !== null) {
    const c = buyChunkCommand(cursor);
    if (c.kind === "BuyChunk") {
      const min = { x: c.cx * CHUNK_SIDE, y: c.cy * CHUNK_SIDE };
      geometry = new Geometry(min);
      geometry.rect(min, { x: min.x + CHUNK_SIDE - 1, y: min.y + CHUNK_SIDE - 1 }, HINT_FILL_COLOR);
      geometry.outline(
        min.x,
        min.y,
        min.x + CHUNK_SIDE,
        min.y + CHUNK_SIDE,
        HINT_EDGE_COLOR,
        BUY_EDGE_PX,
        DASH_PX,
      );
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
  // 2 段目の中の道具を選んでいるあいだは、その入口の帯のボタンを押した状態にする
  for (const { toggle, menu } of toolGroups) {
    toggle.setAttribute("aria-pressed", String(menu.contains(button)));
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

/** Esc 1 回ぶん。順は escape.ts が決める */
function escapeOnce(): void {
  const step = nextEscapeStep({
    drawing: roadStart !== null || pointer?.mode === "area" || devWalks.pending(),
    popoverOpen: popovers.anyOpen(),
    toolSelected: mapCanvas.dataset.tool !== "none",
    inspectorOpen: inspected !== null,
  });
  if (step === "cancel-drawing") {
    cancelTool();
  } else if (step === "close-popover") {
    popovers.closeOnEscape();
  } else if (step === "select-view") {
    const view = toolButtons.find((b) => b.dataset.tool === "none");
    if (view !== undefined) {
      selectTool({ kind: "none" }, view);
    }
  } else if (step === "close-inspector") {
    closeInspector();
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
    // 保存やフィードバックのダイアログは、ブラウザが Esc で閉じる。裏の道具は戻さない
    const dialogOpen =
      document.querySelector("dialog[open]") !== null ||
      (target instanceof Element && target.closest("dialog") !== null);
    if (handlesEscape({ repeat: event.repeat, dialogOpen })) {
      escapeOnce();
    }
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
  pauseButton.setAttribute("aria-pressed", String(paused));
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
  overlays: [labels.canvas],
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

const ZOOM_BUTTON_FACTOR = 1.5;
function zoomByButton(factor: number): void {
  const vp = viewport();
  camera.zoomAt(vp.width / 2, vp.height / 2, factor, vp);
}
zoomInButton.addEventListener("click", () => zoomByButton(ZOOM_BUTTON_FACTOR));
zoomOutButton.addEventListener("click", () => zoomByButton(1 / ZOOM_BUTTON_FACTOR));

for (const button of speedButtons) {
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
      labels.setChunkTiles(chunk.cx, chunk.cy, chunk.tiles);
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
  if (message.kind === "traffic") {
    if (theme === "traffic") {
      trafficLines.setValues(message.revision, new Uint8Array(message.values));
    }
    return;
  }
  if (message.kind === "roads") {
    roads = roadMeshes(message.segments, message.nodes);
    roadIndex = buildRoadIndex(message.segments, message.nodes);
    roadSegments = message.segments;
    // 道路網が変わったら、交通の主題図の線も作り直す。新しい網の値が届くまでは描かない
    trafficLines.setRoads(message.revision, message.segments, message.nodes);
    detail.setRoads(message.segments, message.nodes);
    crosswalks = crosswalkArms(message.segments, message.nodes);
    // test:wasm が読む
    mapCanvas.dataset.removedCrosswalks = String(crosswalks.filter((arm) => arm.removed).length);
    updatePreview();
    renderer?.roads.setRoads(roads);
    labels.setRoads(message.segments);
    // test:wasm が読む
    mapCanvas.dataset.roadRevision = String(message.revision);
    mapCanvas.dataset.roadSegments = String(message.segments.length / SEGMENT_STRIDE);
    return;
  }
  if (message.kind === "signals") {
    signals = signalGeometry(message.lights);
    detail.setSignals(message.lights);
    renderer?.roads.setSignals(signals);
    // test:wasm が読む
    mapCanvas.dataset.signalArms = String(message.lights.length / SIGNAL_STRIDE);
    return;
  }
  if (message.kind === "buildings") {
    buildings = buildingMeshes(message.buildings, message.facilities);
    detail.setBuildings(message.buildings);
    renderer?.roads.setBuildings(buildings);
    labels.setBuildings(message.buildings, message.facilities);
    const stats = parseCityStats(message.stats);
    unsupplied = unsuppliedCount(message.buildings);
    renderCityStats(stats, unsupplied);
    demand = [...stats.demand];
    updateDemandTheme();
    // test:wasm が読む
    mapCanvas.dataset.firstBuilding = Array.from(message.buildings.subarray(0, 4)).join(",");
    mapCanvas.dataset.unsupplied = String(unsuppliedCount(message.buildings));
    mapCanvas.dataset.facilities = String(message.facilities.length / FACILITY_STRIDE);
    return;
  }
  if (message.kind === "cityStats") {
    // 建物の表が変わらないまま動いた需要と内訳
    const stats = parseCityStats(message.stats);
    renderCityStats(stats, unsupplied);
    demand = [...stats.demand];
    updateDemandTheme();
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
    lotShortageSince = trackLotShortage(lotShortageSince, message.lotShortage, message.tick);
    growthNotice.render(
      parseGrowthBlocks(message.overview),
      parseStalls(message.stalls),
      parseIsolated(message.isolated),
      persistedLotShortage(lotShortageSince, message.tick),
    );
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
  labels.setSeed(message.seed);
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
