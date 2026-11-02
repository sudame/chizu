// 地図のラベルを、地図の上に重ねた Canvas 2D に描く。字は 1 つのラベルごとに画像にして覚えておき、
// 描くときは画像を並べるだけにする。置き場所を求め直すのは、データが変わったときとカメラが動いたときだけで、
// 止まっていて出し入れの途中でもないときは何も描き直さない。

import { TILE_WATER } from "../protocol/messages.ts";
import {
  type CameraView,
  type Candidate,
  type DistrictLabel,
  districtCandidates,
  districtLabels,
  type FacilityLabel,
  facilityCandidates,
  facilityLabels,
  LabelFader,
  type LabelSize,
  nextLabelStage,
  placeLabels,
  type Rect,
  type RoadRun,
  roadCandidates,
  roadRuns,
  STAGE_STANDARD,
  styleKey,
  type TextStyle,
  type Viewport,
} from "./labels.ts";

const FONT_FAMILY = '"Hiragino Sans", "Noto Sans JP", system-ui, sans-serif';
const HALO = "#ffffff";
const SHIELD_COLOR = "#1e7b4a";
const SHIELD_RADIUS = 5;
const SHIELD_BORDER = 1.5;
const SHIELD_PAD_X = 5;
const SHIELD_PAD_Y = 3;
// 縦書きの 1 字の送り（字の大きさに掛ける）
const VERTICAL_ADVANCE = 1.05;
// 覚えておく画像の数の上限。超えたら全部捨てて作り直す
const MAX_SPRITES = 2000;
// パネルの下になる所。ここにはラベルを置かない
const OBSTACLE_SELECTORS = [
  "#hudbar",
  "#inspector",
  "#growth-notice",
  "#toolstack",
  "#map-controls",
  "#map-meta",
  "#feedback-open",
];

type Sprite = { image: HTMLCanvasElement; w: number; h: number };

/** 縁取りの太さ。地図の上の文字は 3.5〜4px の白の縁取りを付ける。 */
const haloWidth = (style: TextStyle) => (style.size >= 13 ? 4 : 3.5);
const font = (style: TextStyle) => `${style.weight} ${style.size}px ${FONT_FAMILY}`;

/** チャンクのマスに水があるか。地区の名前の「浜」に使う。 */
export function hasWater(tiles: Uint8Array): boolean {
  for (const t of tiles) {
    if ((t & TILE_WATER) !== 0) return true;
  }
  return false;
}

export class LabelLayer {
  readonly canvas: HTMLCanvasElement;
  readonly #ctx: CanvasRenderingContext2D;
  readonly #measureCtx: CanvasRenderingContext2D;
  readonly #sprites = new Map<string, Sprite>();
  readonly #fader = new LabelFader();
  readonly #water = new Map<string, boolean>();
  #seed: string | null = null;
  #segments: ArrayLike<number> = [];
  #buildings: ArrayLike<number> = [];
  #facilities: ArrayLike<number> = [];
  #runs: RoadRun[] = [];
  #districts: DistrictLabel[] = [];
  #facilityLabels: FacilityLabel[] = [];
  #roadsDirty = false;
  #placesDirty = false;
  #stage = STAGE_STANDARD;
  #viewKey = "";
  #obstacles: Rect[] | null = null;
  #lastFrame: number | null = null;
  #animating = false;
  #dpr = 1;

  constructor(canvas: HTMLCanvasElement) {
    this.canvas = canvas;
    const ctx = canvas.getContext("2d");
    const measure = document.createElement("canvas").getContext("2d");
    if (ctx === null || measure === null) {
      throw new Error("Canvas 2D を使えない");
    }
    this.#ctx = ctx;
    this.#measureCtx = measure;
    // パネルの大きさや出し入れが変わったら、置かない所を測り直す
    const observer = new ResizeObserver(() => {
      this.#obstacles = null;
      this.#viewKey = "";
    });
    for (const selector of OBSTACLE_SELECTORS) {
      const el = document.querySelector(selector);
      if (el !== null) observer.observe(el);
    }
    window.addEventListener("resize", () => {
      this.#obstacles = null;
    });
  }

  /** 街の種。名前はこの種から作る。 */
  setSeed(seed: string): void {
    this.#seed = seed;
    this.#roadsDirty = true;
    this.#placesDirty = true;
  }

  setRoads(segments: ArrayLike<number>): void {
    this.#segments = segments;
    this.#roadsDirty = true;
  }

  setBuildings(buildings: ArrayLike<number>, facilities: ArrayLike<number>): void {
    this.#buildings = buildings;
    this.#facilities = facilities;
    this.#placesDirty = true;
  }

  /** チャンクのマスが届いたときに、水があるかを覚える。変わったときだけ地区の名前を作り直す。 */
  setChunkTiles(cx: number, cy: number, tiles: Uint8Array): void {
    const key = `${cx},${cy}`;
    const water = hasWater(tiles);
    if (this.#water.get(key) !== water) {
      this.#water.set(key, water);
      this.#placesDirty = true;
    }
  }

  readonly #isWaterside = (cx: number, cy: number) => this.#water.get(`${cx},${cy}`) === true;

  /** 毎フレーム呼ぶ。要るときだけ置き場所を求め直し、描き直す。 */
  frame(camera: CameraView, viewport: Viewport, now: number): void {
    const dt = this.#lastFrame === null ? 0 : now - this.#lastFrame;
    this.#lastFrame = now;
    const dpr = window.devicePixelRatio || 1;
    let changed = false;
    if (this.#seed !== null && this.#roadsDirty) {
      this.#runs = roadRuns(this.#seed, this.#segments);
      this.#roadsDirty = false;
      changed = true;
    }
    if (this.#seed !== null && this.#placesDirty) {
      this.#districts = districtLabels(this.#seed, this.#buildings, this.#isWaterside);
      this.#facilityLabels = facilityLabels(this.#seed, this.#facilities, this.#isWaterside);
      this.#placesDirty = false;
      changed = true;
    }
    const key = `${camera.x},${camera.y},${camera.zoom},${viewport.width},${viewport.height},${dpr}`;
    if (key !== this.#viewKey) {
      this.#viewKey = key;
      changed = true;
    }
    if (dpr !== this.#dpr) {
      this.#dpr = dpr;
      this.#sprites.clear();
    }
    if (changed) {
      this.#place(camera, viewport);
    }
    const moving = this.#fader.step(dt);
    if (changed || moving || this.#animating) {
      this.#draw(camera, viewport);
    }
    this.#animating = moving;
  }

  #place(camera: CameraView, viewport: Viewport): void {
    const start = performance.now();
    this.#stage = nextLabelStage(camera.zoom, this.#stage);
    const measure = (text: string, style: TextStyle, vertical: boolean): LabelSize => {
      const s = this.#sprite(text, style, vertical);
      return { w: s.w, h: s.h };
    };
    const candidates: Candidate[] = [
      ...roadCandidates(this.#runs, camera, viewport, this.#stage, measure),
      ...districtCandidates(this.#districts, camera, viewport, this.#stage, measure),
      ...facilityCandidates(this.#facilityLabels, camera, viewport, this.#stage, measure),
    ];
    const placed = placeLabels(candidates, this.#fader.shown(), this.#readObstacles());
    this.#fader.update(placed);
    // test:wasm が読む
    const texts = placed.map((c) => c.text).join("|");
    if (this.canvas.dataset.labels !== texts) {
      this.canvas.dataset.labels = texts;
    }
    this.canvas.dataset.labelStage = String(this.#stage);
    this.canvas.dataset.placeMs = (performance.now() - start).toFixed(3);
  }

  #readObstacles(): Rect[] {
    if (this.#obstacles === null) {
      const map = this.canvas.getBoundingClientRect();
      this.#obstacles = [];
      for (const selector of OBSTACLE_SELECTORS) {
        const el = document.querySelector<HTMLElement>(selector);
        if (el === null) continue;
        const r = el.getBoundingClientRect();
        if (r.width === 0 || r.height === 0) continue;
        this.#obstacles.push({
          x0: r.left - map.left,
          y0: r.top - map.top,
          x1: r.right - map.left,
          y1: r.bottom - map.top,
        });
      }
    }
    return this.#obstacles;
  }

  #draw(camera: CameraView, viewport: Viewport): void {
    const ctx = this.#ctx;
    const dpr = this.#dpr;
    const width = Math.round(viewport.width * dpr);
    const height = Math.round(viewport.height * dpr);
    if (this.canvas.width !== width || this.canvas.height !== height) {
      this.canvas.width = width;
      this.canvas.height = height;
    } else {
      ctx.setTransform(1, 0, 0, 1, 0, 0);
      ctx.clearRect(0, 0, width, height);
    }
    for (const { label, alpha } of this.#fader.items()) {
      const sprite = this.#sprite(label.text, label.style, label.vertical);
      const sx = (label.wx - camera.x) * camera.zoom + viewport.width / 2;
      const sy = (label.wy - camera.y) * camera.zoom + viewport.height / 2;
      ctx.globalAlpha = alpha * label.style.alpha;
      if (label.angle === 0) {
        // 画素の境目にそろえて、字をにじませない
        const x = Math.round((sx + label.ox) * dpr);
        const y = Math.round((sy + label.oy) * dpr);
        ctx.setTransform(1, 0, 0, 1, 0, 0);
        ctx.drawImage(sprite.image, x, y);
      } else {
        const c = Math.cos(label.angle) * dpr;
        const s = Math.sin(label.angle) * dpr;
        ctx.setTransform(c, s, -s, c, sx * dpr, sy * dpr);
        ctx.drawImage(sprite.image, label.ox, label.oy, sprite.w, sprite.h);
      }
    }
    ctx.globalAlpha = 1;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
  }

  /** ラベルの画像。文字と見た目と向きごとに 1 度だけ作る。 */
  #sprite(text: string, style: TextStyle, vertical: boolean): Sprite {
    const key = `${vertical ? "v" : "h"}|${styleKey(style)}|${text}`;
    const cached = this.#sprites.get(key);
    if (cached !== undefined) return cached;
    if (this.#sprites.size >= MAX_SPRITES) this.#sprites.clear();
    const sprite = style.shield
      ? this.#shieldSprite(text, style)
      : vertical
        ? this.#verticalSprite(text, style)
        : this.#horizontalSprite(text, style);
    this.#sprites.set(key, sprite);
    return sprite;
  }

  #canvas(w: number, h: number): { image: HTMLCanvasElement; ctx: CanvasRenderingContext2D } {
    const image = document.createElement("canvas");
    image.width = Math.max(1, Math.ceil(w * this.#dpr));
    image.height = Math.max(1, Math.ceil(h * this.#dpr));
    const ctx = image.getContext("2d");
    if (ctx === null) throw new Error("Canvas 2D を使えない");
    ctx.scale(this.#dpr, this.#dpr);
    return { image, ctx };
  }

  /** 字ごとの幅。字の間隔を空けるときは 1 字ずつ置く。 */
  #glyphWidths(
    text: string,
    style: TextStyle,
  ): { chars: string[]; widths: number[]; total: number } {
    const m = this.#measureCtx;
    m.font = font(style);
    const chars = [...text];
    if (style.spacing === 0) {
      return {
        chars: [text],
        widths: [m.measureText(text).width],
        total: m.measureText(text).width,
      };
    }
    const widths = chars.map((ch) => m.measureText(ch).width);
    const total = widths.reduce((a, b) => a + b, 0) + style.spacing * (chars.length - 1);
    return { chars, widths, total };
  }

  #horizontalSprite(text: string, style: TextStyle): Sprite {
    const halo = haloWidth(style);
    const pad = Math.ceil(halo / 2) + 1;
    const { chars, widths, total } = this.#glyphWidths(text, style);
    const w = Math.ceil(total) + pad * 2;
    const h = Math.ceil(style.size * 1.25) + pad * 2;
    const { image, ctx } = this.#canvas(w, h);
    ctx.font = font(style);
    ctx.textBaseline = "middle";
    ctx.lineJoin = "round";
    ctx.lineWidth = halo;
    ctx.strokeStyle = HALO;
    ctx.fillStyle = style.color;
    for (const pass of ["stroke", "fill"] as const) {
      let x = pad;
      chars.forEach((ch, i) => {
        if (pass === "stroke") ctx.strokeText(ch, x, h / 2);
        else ctx.fillText(ch, x, h / 2);
        x += (widths[i] ?? 0) + style.spacing;
      });
    }
    return { image, w, h };
  }

  /** 縦書き。1 字ずつ縦に積み、長音（ー）は 90 度回す。 */
  #verticalSprite(text: string, style: TextStyle): Sprite {
    const halo = haloWidth(style);
    const pad = Math.ceil(halo / 2) + 1;
    const chars = [...text];
    const step = style.size * VERTICAL_ADVANCE;
    const w = Math.ceil(style.size) + pad * 2;
    const h = Math.ceil(step * chars.length) + pad * 2;
    const { image, ctx } = this.#canvas(w, h);
    ctx.font = font(style);
    ctx.textBaseline = "middle";
    ctx.textAlign = "center";
    ctx.lineJoin = "round";
    ctx.lineWidth = halo;
    ctx.strokeStyle = HALO;
    ctx.fillStyle = style.color;
    for (const pass of ["stroke", "fill"] as const) {
      chars.forEach((ch, i) => {
        ctx.save();
        ctx.translate(w / 2, pad + step * (i + 0.5));
        if (ch === "ー" || ch === "〜" || ch === "～") ctx.rotate(Math.PI / 2);
        if (pass === "stroke") ctx.strokeText(ch, 0, 0);
        else ctx.fillText(ch, 0, 0);
        ctx.restore();
      });
    }
    return { image, w, h };
  }

  /** 高速道路の盾。緑の角丸に白い字を置き、外に白い縁を付ける。 */
  #shieldSprite(text: string, style: TextStyle): Sprite {
    const { total } = this.#glyphWidths(text, style);
    const innerW = Math.ceil(total) + SHIELD_PAD_X * 2;
    const innerH = Math.ceil(style.size * 1.2) + SHIELD_PAD_Y * 2;
    const b = SHIELD_BORDER;
    const w = innerW + b * 2 + 2;
    const h = innerH + b * 2 + 2;
    const { image, ctx } = this.#canvas(w, h);
    ctx.fillStyle = HALO;
    ctx.beginPath();
    ctx.roundRect(1, 1, innerW + b * 2, innerH + b * 2, SHIELD_RADIUS + b);
    ctx.fill();
    ctx.fillStyle = SHIELD_COLOR;
    ctx.beginPath();
    ctx.roundRect(1 + b, 1 + b, innerW, innerH, SHIELD_RADIUS);
    ctx.fill();
    ctx.font = font(style);
    ctx.textBaseline = "middle";
    ctx.textAlign = "center";
    ctx.fillStyle = style.color;
    ctx.fillText(text, w / 2, h / 2);
    return { image, w, h };
  }
}
