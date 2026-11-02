// データレイヤー（CS2 の情報表示）。Worker から届いたマスごとの値を、チャンクごとの整数テクスチャに入れ、
// 値ごとの色で地図の上に半透明で重ねる。値の意味は chizu-wasm の layers.rs が決め、ここは色だけを決める。

import { CHUNK_SIDE, type LayerCode } from "../protocol/messages.ts";
import type { Camera, ChunkRange, Viewport } from "./camera.ts";
import { chunkKey } from "./chunk-sync.ts";
import type { Color } from "./roads.ts";

/** 色の表の長さ。値はこれより小さくする。 */
export const PALETTE_SIZE = 8;

/** データレイヤー 1 枚。palette[v] が値 v の色で、0 の値は塗らない。 */
export type DataLayer = { code: LayerCode; label: string; palette: readonly Color[] };

const NONE: Color = [0, 0, 0, 0];
// 電気と水道の値の色。値は chizu-wasm の layers::SUPPLY_BUILDING_ON などと同じ並び。
// 届いている建物は緑、届いていない建物は赤、道路は網の釣り合い（足りる、足りない、施設がない）で塗る
const supplyPalette = (facility: Color): Color[] => [
  NONE,
  [0.2, 0.85, 0.4, 0.9],
  [0.95, 0.2, 0.2, 0.9],
  [0.3, 0.75, 0.95, 0.9],
  [1, 0.55, 0.1, 0.9],
  [0.45, 0.45, 0.5, 0.9],
  facility,
];

/**
 * 交通の値の色と凡例の名前。値は chizu-wasm の layers::TRAFFIC_FREE などと同じ並びで、
 * 空いている、やや混んでいる、混んでいる、詰まっている。緑、黄、橙、赤で、一目で混み具合が分かるようにする。
 */
export const TRAFFIC_LEVELS: readonly { label: string; color: Color }[] = [
  { label: "空いている", color: [0.2, 0.8, 0.35, 0.9] },
  { label: "やや混んでいる", color: [0.98, 0.88, 0.15, 0.9] },
  { label: "混んでいる", color: [1, 0.5, 0.05, 0.9] },
  { label: "詰まっている", color: [0.9, 0.12, 0.12, 0.95] },
];

/** 交通のレイヤーの番号。chizu-wasm の layers::LAYERS の 3 番めである。 */
export const TRAFFIC_LAYER: LayerCode = 3;

export const DATA_LAYERS: readonly DataLayer[] = [
  { code: 1, label: "電気", palette: supplyPalette([1, 0.85, 0.1, 0.95]) },
  { code: 2, label: "水道", palette: supplyPalette([0.2, 0.5, 1, 0.95]) },
  { code: TRAFFIC_LAYER, label: "交通", palette: [NONE, ...TRAFFIC_LEVELS.map((l) => l.color)] },
];

/** レイヤーを出しているあいだ、下の地図をこの割合だけ暗くして、重ねた色を見やすくする。 */
export const DIM = 0.45;

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
uniform float u_dim;
// 重ねたレイヤーの数と、このレイヤーの順番。各マスを縦の帯に等分し、このレイヤーは u_slot 番目の帯だけを塗る
uniform int u_slots;
uniform int u_slot;
in vec2 v_uv;
out vec4 color;
void main() {
  uint v = texture(u_values, v_uv).r;
  int band = int(floor(fract(v_uv.x * ${CHUNK_SIDE}.0) * float(u_slots)));
  if (v == 0u || v >= ${PALETTE_SIZE}u || min(band, u_slots - 1) != u_slot) {
    if (u_dim == 0.0) {
      discard;
    }
    color = vec4(0.0, 0.0, 0.0, u_dim);
  } else {
    color = u_palette[v];
  }
}`;

/** 色の表を uniform に渡す並びにする。足りない分は透明で埋める。 */
export function paletteArray(palette: readonly Color[]): Float32Array {
  const out = new Float32Array(PALETTE_SIZE * 4);
  palette.slice(0, PALETTE_SIZE).forEach((c, i) => {
    out.set(c, i * 4);
  });
  return out;
}

type Entry = { texture: WebGLTexture; version: number };

export class LayerOverlay {
  readonly #gl: WebGL2RenderingContext;
  readonly #program: WebGLProgram;
  readonly #vao: WebGLVertexArrayObject;
  readonly #u: Record<
    "offset" | "size" | "viewport" | "values" | "palette" | "dim" | "slots" | "slot",
    WebGLUniformLocation | null
  >;
  /** レイヤーの番号ごとの、チャンクのテクスチャ。 */
  readonly #textures = new Map<LayerCode, Map<string, Entry>>();
  /** 出すレイヤー。並びの順に重ねる。 */
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
      dim: u("u_dim"),
      slots: u("u_slots"),
      slot: u("u_slot"),
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
   * 出しているレイヤーを、範囲の中のチャンクについて重ねて描き、描いたテクスチャの数を返す。
   * 2 枚以上を出しているときは、どのレイヤーも読めるよう、各マスを縦の帯に分けて 1 枚ずつ塗る。
   */
  draw(camera: Camera, viewport: Viewport, range: ChunkRange): number {
    const gl = this.#gl;
    const layers = DATA_LAYERS.filter((l) => this.active.includes(l.code));
    if (layers.length === 0) {
      return 0;
    }
    gl.useProgram(this.#program);
    gl.bindVertexArray(this.#vao);
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
    gl.uniform1i(this.#u.values, 0);
    gl.uniform2f(this.#u.viewport, viewport.width, viewport.height);
    gl.uniform1f(this.#u.size, CHUNK_SIDE * camera.zoom);
    gl.uniform1i(this.#u.slots, layers.length);
    let drawn = 0;
    for (const [slot, layer] of layers.entries()) {
      gl.uniform1i(this.#u.slot, slot);
      const chunks = this.#textures.get(layer.code);
      if (chunks === undefined) {
        continue;
      }
      gl.uniform4fv(this.#u.palette, paletteArray(layer.palette));
      // 下の地図を暗くするのは、最初に重ねるレイヤーだけにする
      gl.uniform1f(this.#u.dim, drawn === 0 ? DIM : 0);
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
    }
    gl.disable(gl.BLEND);
    gl.bindVertexArray(null);
    return drawn;
  }
}

/**
 * 画面の端に、レイヤーを選ぶボタンを並べる。CS2 の情報表示にならい、「なし」とレイヤーごとのボタンを置く。
 * レイヤーのボタンはそれぞれ押すたびにオンとオフが変わり、いくつでも重ねられる。「なし」はすべて消す。
 */
export function setupLayerPanel(
  container: HTMLElement,
  onChange: (active: LayerCode[]) => void,
): void {
  let active: LayerCode[] = [];
  const entries: [LayerCode | null, string][] = [
    [null, "なし"],
    ...DATA_LAYERS.map((l): [LayerCode, string] => [l.code, l.label]),
  ];
  const buttons = entries.map(([code, label]) => {
    const b = document.createElement("button");
    b.type = "button";
    b.textContent = label;
    b.dataset.layer = code === null ? "none" : String(code);
    b.addEventListener("click", () => {
      active =
        code === null
          ? []
          : active.includes(code)
            ? active.filter((c) => c !== code)
            : [...active, code];
      show();
      onChange(active);
    });
    return b;
  });
  const show = () => {
    buttons.forEach((b, i) => {
      const code = entries[i]?.[0] ?? null;
      const pressed = code === null ? active.length === 0 : active.includes(code);
      b.setAttribute("aria-pressed", String(pressed));
    });
    // test:wasm が読む
    container.dataset.active = active.join(",");
  };
  container.replaceChildren(...buttons);
  show();
}
