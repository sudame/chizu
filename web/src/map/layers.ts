// データレイヤー（CS2 の情報表示）。Worker から届いたマスごとの値を、チャンクごとの整数テクスチャに入れ、
// 下の地図を白で薄めてから、値ごとの色で塗る。値の意味は chizu-wasm の layers.rs が決め、ここは色だけを決める。
// 交通は道路に沿った線で描くので、ここでは塗らない（traffic.ts）。

import { CHUNK_SIDE, type LayerCode } from "../protocol/messages.ts";
import type { Camera, ChunkRange, Viewport } from "./camera.ts";
import { chunkKey } from "./chunk-sync.ts";
import { type Color, hex } from "./palette.ts";

/** 色の表の長さ。値はこれより小さくする。 */
export const PALETTE_SIZE = 8;

/**
 * 値の塗り方。`fill` は色で塗る、`hatch` は色の上に `accent` の斜線を引く、`dash` は色と下地の薄めた地図を
 * 斜めの縞で交互にする（破線の代わり）、`through` は薄めずに下の地図をそのまま見せる（施設の記号）。
 */
export type ValueStyle =
  | { kind: "none" }
  | { kind: "fill"; color: Color }
  | { kind: "hatch"; color: Color; accent: Color }
  | { kind: "dash"; color: Color }
  | { kind: "through" };

/** データレイヤー 1 枚。styles[v] が値 v の塗り方で、0 の値は下の地図を薄めるだけにする。 */
export type DataLayer = {
  code: LayerCode;
  label: string;
  styles: readonly ValueStyle[];
};

/** 主題図の色（docs/map-style.md の色の表の「主題図」）。 */
export const THEME_COLORS = {
  suppliedBuilding: hex("#81c995"),
  unsuppliedBuilding: hex("#f28b82"),
  unsuppliedHatch: hex("#d93025"),
  enoughRoad: hex("#1a73e8"),
  shortRoad: hex("#f29900"),
  unconnectedRoad: hex("#9aa0a6"),
  // 駐車場の主題図。車を持てない世帯の家は赤の面に斜線、共同の駐車場は埋まり具合を青の濃さの 4 段で示す
  carlessHome: hex("#f28b82"),
  carlessHatch: hex("#d93025"),
  lotFree: hex("#a8c7fa"),
  lotBusy: hex("#669df6"),
  lotNearlyFull: hex("#1a73e8"),
  lotFull: hex("#174ea6"),
} as const;

/** 電気、水道、需要の主題図を出すあいだ、下の地図をこの割合だけ白で薄める。 */
export const WASH = 0.5;

// 電気と水道の値の塗り方。値は chizu-wasm の layers::SUPPLY_BUILDING_ON などと同じ並び。
// 届いている建物は緑、届いていない建物は赤に斜線、道路は網の釣り合い（足りる、足りない、施設がない）で塗る。
// そのレイヤーの施設は、薄めずにそのまま出す
const SUPPLY_STYLES: readonly ValueStyle[] = [
  { kind: "none" },
  { kind: "fill", color: THEME_COLORS.suppliedBuilding },
  {
    kind: "hatch",
    color: THEME_COLORS.unsuppliedBuilding,
    accent: THEME_COLORS.unsuppliedHatch,
  },
  { kind: "fill", color: THEME_COLORS.enoughRoad },
  { kind: "fill", color: THEME_COLORS.shortRoad },
  { kind: "dash", color: THEME_COLORS.unconnectedRoad },
  { kind: "through" },
];

/**
 * 交通の値の色と凡例の名前。値は chizu-wasm の layers::TRAFFIC_FREE などと同じ並びで、
 * 空いている、やや混んでいる、混んでいる、詰まっている。色相に加えて明るさも段々に暗くし、明るさの順でも読めるようにする。
 */
export const TRAFFIC_LEVELS: readonly { label: string; color: Color }[] = [
  { label: "空いている", color: hex("#63d668") },
  { label: "やや混んでいる", color: hex("#ff974d") },
  { label: "混んでいる", color: hex("#f23c32") },
  { label: "詰まっている", color: hex("#811f1f") },
];

/** 交通のレイヤーの番号。chizu-wasm の layers::LAYERS の 3 番めである。交通は線で描くので、ここの面では塗らない。 */
export const TRAFFIC_LAYER: LayerCode = 3;

/** 駐車場の値の塗り方。値は chizu-wasm の layers::PARKING_CARLESS_HOME などと同じ並び。 */
const PARKING_STYLES: readonly ValueStyle[] = [
  { kind: "none" },
  { kind: "hatch", color: THEME_COLORS.carlessHome, accent: THEME_COLORS.carlessHatch },
  { kind: "fill", color: THEME_COLORS.lotFree },
  { kind: "fill", color: THEME_COLORS.lotBusy },
  { kind: "fill", color: THEME_COLORS.lotNearlyFull },
  { kind: "fill", color: THEME_COLORS.lotFull },
];

/**
 * 共同の駐車場の埋まり具合の段階と凡例の名前。値は chizu-wasm の layers::PARKING_LOT_FREE などと同じ並びで、
 * 台数に対する、借りられた枠と外来の車の割合である。
 */
export const PARKING_LOT_LEVELS: readonly { label: string; color: Color }[] = [
  { label: "半分に満たない", color: THEME_COLORS.lotFree },
  { label: "半分以上", color: THEME_COLORS.lotBusy },
  { label: "8 割以上", color: THEME_COLORS.lotNearlyFull },
  { label: "満車", color: THEME_COLORS.lotFull },
];

/** 駐車場のレイヤーの番号。chizu-wasm の layers::LAYERS の 4 番めである。 */
export const PARKING_LAYER: LayerCode = 4;

/** マスの面で塗るデータレイヤー（電気、水道、駐車場）。 */
export const DATA_LAYERS: readonly DataLayer[] = [
  { code: 1, label: "電気", styles: SUPPLY_STYLES },
  { code: 2, label: "水道", styles: SUPPLY_STYLES },
  { code: PARKING_LAYER, label: "駐車場", styles: PARKING_STYLES },
];

// 塗り方の番号。シェーダーの u_style と同じ
const STYLE_CODE: Record<ValueStyle["kind"], number> = {
  none: 0,
  fill: 1,
  hatch: 2,
  dash: 3,
  through: 4,
};
// 斜線と縞の周期と太さ（画面のピクセル、x + y の方向）
const HATCH_PERIOD_PX = 6;
const HATCH_WIDTH_PX = 2;
const DASH_PERIOD_PX = 8;

const VERTEX = `#version 300 es
in vec2 a_corner;
uniform vec2 u_offset;
uniform float u_size;
uniform vec2 u_viewport;
out vec2 v_uv;
void main() {
  vec2 p = (u_offset + a_corner * u_size) / (u_viewport * 0.5);
  gl_Position = vec4(p.x, -p.y, 0.0, 1.0);
  v_uv = a_corner;
}`;

const FRAGMENT = `#version 300 es
precision mediump float;
uniform mediump usampler2D u_values;
uniform vec4 u_palette[${PALETTE_SIZE}];
uniform vec4 u_accent[${PALETTE_SIZE}];
uniform int u_style[${PALETTE_SIZE}];
uniform float u_wash;
in vec2 v_uv;
out vec4 color;
void main() {
  uint v = texture(u_values, v_uv).r;
  int style = v < ${PALETTE_SIZE}u ? u_style[v] : 0;
  vec4 wash = vec4(1.0, 1.0, 1.0, u_wash);
  highp float diagonal = gl_FragCoord.x + gl_FragCoord.y;
  if (style == 4) {
    discard;
  } else if (style == 1) {
    color = u_palette[v];
  } else if (style == 2) {
    color = mod(diagonal, ${HATCH_PERIOD_PX}.0) < ${HATCH_WIDTH_PX}.0 ? u_accent[v] : u_palette[v];
  } else if (style == 3) {
    color = mod(diagonal, ${DASH_PERIOD_PX}.0) < ${DASH_PERIOD_PX / 2}.0 ? u_palette[v] : wash;
  } else {
    color = wash;
  }
}`;

/** 塗り方の並びを、uniform に渡す色、斜線の色、塗り方の番号の並びにする。足りない分は塗らない。 */
export function styleArrays(styles: readonly ValueStyle[]): {
  palette: Float32Array;
  accent: Float32Array;
  style: Int32Array;
} {
  const palette = new Float32Array(PALETTE_SIZE * 4);
  const accent = new Float32Array(PALETTE_SIZE * 4);
  const style = new Int32Array(PALETTE_SIZE);
  styles.slice(0, PALETTE_SIZE).forEach((s, i) => {
    style[i] = STYLE_CODE[s.kind];
    if (s.kind === "fill" || s.kind === "hatch" || s.kind === "dash") {
      palette.set(s.color, i * 4);
    }
    if (s.kind === "hatch") {
      accent.set(s.accent, i * 4);
    }
  });
  return { palette, accent, style };
}

type Entry = { texture: WebGLTexture; version: number };

export class LayerOverlay {
  readonly #gl: WebGL2RenderingContext;
  readonly #program: WebGLProgram;
  readonly #vao: WebGLVertexArrayObject;
  readonly #u: Record<
    "offset" | "size" | "viewport" | "values" | "palette" | "accent" | "style" | "wash",
    WebGLUniformLocation | null
  >;
  /** レイヤーの番号ごとの、チャンクのテクスチャ。 */
  readonly #textures = new Map<LayerCode, Map<string, Entry>>();
  /** 出すレイヤー。面で塗るのは一度に 1 つだけである。 */
  active: LayerCode[] = [];

  constructor(
    gl: WebGL2RenderingContext,
    link: (vertex: string, fragment: string) => WebGLProgram,
  ) {
    this.#gl = gl;
    this.#program = link(VERTEX, FRAGMENT);
    const u = (name: string) => gl.getUniformLocation(this.#program, name);
    this.#u = {
      offset: u("u_offset"),
      size: u("u_size"),
      viewport: u("u_viewport"),
      values: u("u_values"),
      palette: u("u_palette"),
      accent: u("u_accent"),
      style: u("u_style"),
      wash: u("u_wash"),
    };
    this.#vao = gl.createVertexArray();
    gl.bindVertexArray(this.#vao);
    gl.bindBuffer(gl.ARRAY_BUFFER, gl.createBuffer());
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([0, 0, 1, 0, 0, 1, 1, 1]), gl.STATIC_DRAW);
    const corner = gl.getAttribLocation(this.#program, "a_corner");
    gl.enableVertexAttribArray(corner);
    gl.vertexAttribPointer(corner, 2, gl.FLOAT, false, 0, 0);
    gl.bindVertexArray(null);
  }

  upload(layer: LayerCode, cx: number, cy: number, version: number, values: Uint8Array): void {
    const gl = this.#gl;
    let chunks = this.#textures.get(layer);
    if (chunks === undefined) {
      chunks = new Map();
      this.#textures.set(layer, chunks);
    }
    const key = chunkKey(cx, cy);
    let entry = chunks.get(key);
    if (entry === undefined) {
      const texture = gl.createTexture();
      gl.bindTexture(gl.TEXTURE_2D, texture);
      gl.texStorage2D(gl.TEXTURE_2D, 1, gl.R8UI, CHUNK_SIDE, CHUNK_SIDE);
      for (const p of [gl.TEXTURE_MIN_FILTER, gl.TEXTURE_MAG_FILTER]) {
        gl.texParameteri(gl.TEXTURE_2D, p, gl.NEAREST);
      }
      for (const p of [gl.TEXTURE_WRAP_S, gl.TEXTURE_WRAP_T]) {
        gl.texParameteri(gl.TEXTURE_2D, p, gl.CLAMP_TO_EDGE);
      }
      entry = { texture, version };
      chunks.set(key, entry);
    }
    entry.version = version;
    gl.bindTexture(gl.TEXTURE_2D, entry.texture);
    gl.texSubImage2D(
      gl.TEXTURE_2D,
      0,
      0,
      0,
      CHUNK_SIDE,
      CHUNK_SIDE,
      gl.RED_INTEGER,
      gl.UNSIGNED_BYTE,
      values,
    );
  }

  /** 画面が捨てたチャンクのテクスチャを、どのレイヤーからも消す。 */
  remove(cx: number, cy: number): void {
    const key = chunkKey(cx, cy);
    for (const chunks of this.#textures.values()) {
      const entry = chunks.get(key);
      if (entry !== undefined) {
        this.#gl.deleteTexture(entry.texture);
        chunks.delete(key);
      }
    }
  }

  /** 出していないレイヤーのテクスチャを消す。出し直すときは Worker が送り直す。 */
  setActive(active: LayerCode[]): void {
    this.active = active;
    for (const [code, chunks] of this.#textures) {
      if (!active.includes(code)) {
        for (const entry of chunks.values()) {
          this.#gl.deleteTexture(entry.texture);
        }
        this.#textures.delete(code);
      }
    }
  }

  /**
   * 出しているレイヤーを、範囲の中のチャンクについて描き、描いたテクスチャの数を返す。
   * 値のないマスは下の地図を白で薄め、値のあるマスは値の塗り方で塗る。
   */
  draw(camera: Camera, viewport: Viewport, range: ChunkRange): number {
    const gl = this.#gl;
    const layer = DATA_LAYERS.find((l) => this.active.includes(l.code));
    const chunks = layer === undefined ? undefined : this.#textures.get(layer.code);
    if (layer === undefined || chunks === undefined) {
      return 0;
    }
    gl.useProgram(this.#program);
    gl.bindVertexArray(this.#vao);
    gl.enable(gl.BLEND);
    // 色だけを重ね、フレームバッファのアルファは 1 のまま保つ（キャンバスがページの背景と混ざって白く飛ばないように）
    gl.blendFuncSeparate(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA, gl.ZERO, gl.ONE);
    gl.uniform1i(this.#u.values, 0);
    gl.uniform2f(this.#u.viewport, viewport.width, viewport.height);
    gl.uniform1f(this.#u.size, CHUNK_SIDE * camera.zoom);
    gl.uniform1f(this.#u.wash, WASH);
    const arrays = styleArrays(layer.styles);
    gl.uniform4fv(this.#u.palette, arrays.palette);
    gl.uniform4fv(this.#u.accent, arrays.accent);
    gl.uniform1iv(this.#u.style, arrays.style);
    let drawn = 0;
    for (let cy = range.minCy; cy <= range.maxCy; cy++) {
      for (let cx = range.minCx; cx <= range.maxCx; cx++) {
        const entry = chunks.get(chunkKey(cx, cy));
        if (entry === undefined) {
          continue;
        }
        gl.uniform2f(
          this.#u.offset,
          (cx * CHUNK_SIDE - camera.x) * camera.zoom,
          (cy * CHUNK_SIDE - camera.y) * camera.zoom,
        );
        gl.bindTexture(gl.TEXTURE_2D, entry.texture);
        gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
        drawn++;
      }
    }
    gl.disable(gl.BLEND);
    gl.bindVertexArray(null);
    return drawn;
  }
}
