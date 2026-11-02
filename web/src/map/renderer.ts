// WebGL2 でチャンクを描く。チャンクごとに 64×64 の整数テクスチャを 1 枚持ち、1 枚の四角に貼る。
// 位置はカメラからの相対で CPU が求めて渡す。遠くの座標でも f32 の桁が足りるようにするためである。

import {
  CHUNK_SIDE,
  TILE_BUILDING,
  TILE_WATER,
  TILE_ZONABLE,
  TILE_ZONE_SHIFT,
} from "../protocol/messages.ts";
import type { Camera, ChunkRange, Viewport } from "./camera.ts";
import { chunkKey, type StoredChunk } from "./chunk-sync.ts";
import { LayerOverlay, WASH } from "./layers.ts";
import { MoverLayer } from "./movers.ts";
import {
  GROUND_COLOR,
  OUTSIDE_COLOR,
  SHORE_COLOR,
  UNOWNED_COLOR,
  UNOWNED_HATCH_COLOR,
  UNOWNED_SHORE_COLOR,
  UNOWNED_WATER_COLOR,
  WATER_COLOR,
  ZONABLE_COLOR,
  ZONABLE_MIX,
} from "./palette.ts";
import { buildingEdgeColor, RoadLayer, ZONE_COLORS, zoneGroundColor } from "./roads.ts";
import { stageWeights } from "./stages.ts";
import { demandColor, demandShare } from "./themes.ts";

const vec3 = ([r, g, b]: readonly number[]) => `vec3(${r}, ${g}, ${b})`;

// 区画ごとの色を、区画 1〜4 の順に 4 つずつ並べる。地面（roads.ts と共有する）、引きの段の街区の面の
// 建物のないマスと建物のあるマス（濃い）、寄りの段の空き地の模様（建物の縁の色）の順である。
// シェーダーの定数の配列にすると、ソフトウェアの描画ではマスの画素ごとに配列を作り直して重くなるので、uniform で渡す
const ZONE_PALETTE = new Float32Array(
  [1, 2, 3, 4].flatMap((zone) =>
    [
      zoneGroundColor(zone),
      ZONE_COLORS[zone]?.blockEmpty ?? [0, 0, 0],
      ZONE_COLORS[zone]?.block ?? [0, 0, 0],
      buildingEdgeColor(zone),
    ].flatMap(([r = 0, g = 0, b = 0]) => [r, g, b]),
  ),
);
// 模様は、建物の縁の色を区画の地面に 60% 重ねる
const PATTERN_MIX = 0.6;
// 模様の周期は 0.5 マスにする（商業の点の間隔、オフィスの横線の間隔。工業の斜線は x + y の方向に 1 マス）。
// チャンクの 1 辺は、この周期の数で何個分か
const PATTERN_CELLS = CHUNK_SIDE * 2;

// 買っていない土地の斜線。画面で、線と線の間隔が 10px、線の太さが 1px の 45 度の線にする。
// x + y の方向では、間隔が 10 × √2、太さが √2 になる
const HATCH_PERIOD = 10 * Math.SQRT2;
const HATCH_WIDTH = Math.SQRT2;

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
uniform mediump usampler2D u_tiles;
uniform bool u_owned;
uniform bool u_zonable;
uniform highp float u_size;
uniform highp float u_hatch;
// 街区の面の濃さ（引きで 1）と、空き地の模様の濃さ（寄りで 1）
uniform float u_block;
uniform float u_pattern;
in highp vec2 v_uv;
out vec4 color;
// 区画の色（ZONE_PALETTE）。区画の地面は淡い色で塗る。色は CS2 にならう（住宅は緑、商業は青、工業は黄、オフィスは水色）
uniform vec3 u_zone[16];
// 空き地の模様（住宅は無地、商業は点、工業は斜線、オフィスは横線）が、この画素にかかるか。
// 線と点の太さは画面で 1px ほどにする。ソフトウェアの描画では分岐の両方を計算するので、安い計算にとどめる
bool pattern(uint zone) {
  highp vec2 cell = fract(v_uv * ${PATTERN_CELLS}.0);
  float cellPx = u_size / ${PATTERN_CELLS}.0;
  if (zone == 2u) {
    vec2 d = (cell - 0.5) * cellPx;
    float r = max(1.0, 0.08 * cellPx);
    return dot(d, d) < r * r;
  }
  if (zone == 3u) {
    return fract((v_uv.x + v_uv.y) * ${CHUNK_SIDE}.0) * cellPx * 2.0 < 1.4;
  }
  return cell.y * cellPx < 1.0;
}
bool isWater(ivec2 t) {
  if (t.x < 0 || t.y < 0 || t.x >= ${CHUNK_SIDE} || t.y >= ${CHUNK_SIDE}) {
    return true;
  }
  return (texelFetch(u_tiles, t, 0).r & ${TILE_WATER}u) != 0u;
}
void main() {
  ivec2 t = min(ivec2(floor(v_uv * ${CHUNK_SIDE}.0)), ivec2(${CHUNK_SIDE - 1}));
  uint tile = texelFetch(u_tiles, t, 0).r;
  vec3 c;
  if ((tile & ${TILE_WATER}u) != 0u) {
    c = ${vec3(WATER_COLOR)};
    // 買っていない水は、買っていない土地と同じく少し暗くして斜線を引く
    if (!u_owned) {
      c = ${vec3(UNOWNED_WATER_COLOR)};
      highp float sw = u_hatch + (v_uv.x + v_uv.y) * u_size;
      if (mod(sw, ${HATCH_PERIOD}) < ${HATCH_WIDTH}) {
        c = ${vec3(UNOWNED_SHORE_COLOR)};
      }
    }
    // 水と陸の境目のマスの辺に、1 ピクセルの岸の線を引く（チャンクの端は陸があるかを調べない）
    float px = u_size / ${CHUNK_SIDE}.0;
    highp vec2 f = fract(v_uv * ${CHUNK_SIDE}.0) * px;
    highp vec2 g = px - f;
    bool shore = (f.x < 1.0 && !isWater(t + ivec2(-1, 0)))
      || (g.x < 1.0 && !isWater(t + ivec2(1, 0)))
      || (f.y < 1.0 && !isWater(t + ivec2(0, -1)))
      || (g.y < 1.0 && !isWater(t + ivec2(0, 1)));
    if (shore) {
      c = u_owned ? ${vec3(SHORE_COLOR)} : ${vec3(UNOWNED_SHORE_COLOR)};
    }
  } else if (!u_owned) {
    // 買っていない土地は、色の差がなくても分かるよう斜線を引く
    c = ${vec3(UNOWNED_COLOR)};
    // 斜線は世界の座標に付ける。u_hatch は、チャンクの左上の世界の位置（画面のピクセル）を周期で割った余りである
    highp float s = u_hatch + (v_uv.x + v_uv.y) * u_size;
    if (mod(s, ${HATCH_PERIOD}) < ${HATCH_WIDTH}) {
      c = ${vec3(UNOWNED_HATCH_COLOR)};
    }
  } else {
    c = ${vec3(GROUND_COLOR)};
    uint zone = (tile >> ${TILE_ZONE_SHIFT}u) & 7u;
    if (zone >= 1u && zone <= 4u) {
      int i = int(zone - 1u) * 4;
      bool built = (tile & ${TILE_BUILDING}u) != 0u;
      c = u_zone[i];
      // 引きでは街区の面で塗り、建物のあるマスを濃くする
      if (u_block > 0.0) {
        c = mix(c, u_zone[i + (built ? 2 : 1)], u_block);
      }
      // 寄りでは、建物のない区画に模様を付ける。住宅は無地
      if (u_pattern > 0.0 && !built && zone != 1u) {
        if (pattern(zone)) {
          c = mix(c, u_zone[i + 3], ${PATTERN_MIX} * u_pattern);
        }
      }
    } else if (u_zonable && (tile & ${TILE_ZONABLE}u) != 0u) {
      c = mix(c, ${vec3(ZONABLE_COLOR)}, ${ZONABLE_MIX});
    }
  }
  color = vec4(c, 1.0);
}`;

const DEMAND_PALETTE = new Float32Array(
  [1, 2, 3, 4].flatMap((zone) => {
    const [r = 0, g = 0, b = 0] = demandColor(zone);
    return [r, g, b];
  }),
);

// 需要の主題図。チャンクのマスの区画と建物のビットを読み、建物のない区画をその種類の需要の高さで濃く塗る。
// ほかのマスは白で薄める
const DEMAND_FRAGMENT = `#version 300 es
precision mediump float;
uniform mediump usampler2D u_tiles;
uniform bool u_owned;
uniform vec3 u_deep[4];
uniform float u_share[4];
uniform float u_wash;
in highp vec2 v_uv;
out vec4 color;
void main() {
  ivec2 t = min(ivec2(floor(v_uv * ${CHUNK_SIDE}.0)), ivec2(${CHUNK_SIDE - 1}));
  uint tile = texelFetch(u_tiles, t, 0).r;
  uint zone = (tile >> ${TILE_ZONE_SHIFT}u) & 7u;
  bool empty = (tile & (${TILE_BUILDING}u | ${TILE_WATER}u)) == 0u;
  if (u_owned && empty && zone >= 1u && zone <= 4u) {
    int i = int(zone - 1u);
    color = vec4(mix(vec3(1.0), u_deep[i], u_share[i]), 1.0);
  } else {
    color = vec4(1.0, 1.0, 1.0, u_wash);
  }
}`;

function compile(gl: WebGL2RenderingContext, type: GLenum, source: string): WebGLShader {
  const shader = gl.createShader(type);
  if (shader === null) {
    throw new Error("シェーダーを作れない");
  }
  gl.shaderSource(shader, source);
  gl.compileShader(shader);
  if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
    throw new Error(`シェーダーをコンパイルできない：${gl.getShaderInfoLog(shader)}`);
  }
  return shader;
}

function link(gl: WebGL2RenderingContext, vertex: string, fragment: string): WebGLProgram {
  const program = gl.createProgram();
  gl.attachShader(program, compile(gl, gl.VERTEX_SHADER, vertex));
  gl.attachShader(program, compile(gl, gl.FRAGMENT_SHADER, fragment));
  gl.linkProgram(program);
  if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
    throw new Error(`シェーダーをつなげない：${gl.getProgramInfoLog(program)}`);
  }
  return program;
}

function uniform(gl: WebGL2RenderingContext, program: WebGLProgram, name: string) {
  const location = gl.getUniformLocation(program, name);
  if (location === null) {
    throw new Error(`uniform ${name} がない`);
  }
  return location;
}

type Texture = { texture: WebGLTexture; owned: boolean };

export class MapRenderer {
  readonly canvas: HTMLCanvasElement;
  readonly #gl: WebGL2RenderingContext;
  readonly #program: WebGLProgram;
  readonly #vao: WebGLVertexArrayObject;
  /** 道路網と道具の仮の表示。チャンクの上に重ねて描く。 */
  readonly roads: RoadLayer;
  /** データレイヤー。道路と建物の上に重ねて描く。 */
  readonly layers: LayerOverlay;
  /** 移動体。道路の上に重ねて描く。 */
  readonly movers: MoverLayer;
  readonly #textures = new Map<string, Texture>();
  readonly #u: Record<
    "offset" | "size" | "viewport" | "tiles" | "owned" | "zonable" | "hatch" | "block" | "pattern",
    WebGLUniformLocation
  >;
  /** 区画を塗れるマスを明るく示すか。区画の道具を選んでいるあいだだけ true にする。 */
  showZonable = false;
  /** 需要の主題図の、住宅・商業・工業・オフィスの需要の高さ（0〜1）。null なら需要の主題図を出さない。 */
  demand: readonly number[] | null = null;
  readonly #demandProgram: WebGLProgram;
  readonly #du: Record<
    "offset" | "size" | "viewport" | "tiles" | "owned" | "share" | "wash",
    WebGLUniformLocation
  >;

  constructor(canvas: HTMLCanvasElement) {
    this.canvas = canvas;
    // test:wasm が描いたあとの画素を読めるよう、描画の結果を残す
    const gl = canvas.getContext("webgl2", { antialias: false, preserveDrawingBuffer: true });
    if (gl === null) {
      throw new Error("WebGL2 を使えない");
    }
    this.#gl = gl;
    const program = link(gl, VERTEX, FRAGMENT);
    this.#program = program;
    gl.useProgram(program);
    this.#u = {
      offset: uniform(gl, program, "u_offset"),
      size: uniform(gl, program, "u_size"),
      viewport: uniform(gl, program, "u_viewport"),
      tiles: uniform(gl, program, "u_tiles"),
      owned: uniform(gl, program, "u_owned"),
      zonable: uniform(gl, program, "u_zonable"),
      hatch: uniform(gl, program, "u_hatch"),
      block: uniform(gl, program, "u_block"),
      pattern: uniform(gl, program, "u_pattern"),
    };
    gl.uniform1i(this.#u.tiles, 0);
    gl.uniform3fv(uniform(gl, program, "u_zone"), ZONE_PALETTE);

    this.#vao = gl.createVertexArray();
    gl.bindVertexArray(this.#vao);
    gl.bindBuffer(gl.ARRAY_BUFFER, gl.createBuffer());
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([0, 0, 1, 0, 0, 1, 1, 1]), gl.STATIC_DRAW);
    const corner = gl.getAttribLocation(program, "a_corner");
    gl.enableVertexAttribArray(corner);
    gl.vertexAttribPointer(corner, 2, gl.FLOAT, false, 0, 0);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindVertexArray(null);
    const demand = link(gl, VERTEX, DEMAND_FRAGMENT);
    this.#demandProgram = demand;
    gl.useProgram(demand);
    this.#du = {
      offset: uniform(gl, demand, "u_offset"),
      size: uniform(gl, demand, "u_size"),
      viewport: uniform(gl, demand, "u_viewport"),
      tiles: uniform(gl, demand, "u_tiles"),
      owned: uniform(gl, demand, "u_owned"),
      share: uniform(gl, demand, "u_share"),
      wash: uniform(gl, demand, "u_wash"),
    };
    gl.uniform1i(this.#du.tiles, 0);
    gl.uniform3fv(uniform(gl, demand, "u_deep"), DEMAND_PALETTE);
    this.roads = new RoadLayer(gl, (v, f) => link(gl, v, f));
    this.layers = new LayerOverlay(gl, (v, f) => link(gl, v, f));
    this.movers = new MoverLayer(gl, (v, f) => link(gl, v, f));
  }

  /** チャンクのテクスチャを作るか、中身を書き換える。 */
  upload(chunk: StoredChunk): void {
    const gl = this.#gl;
    const key = chunkKey(chunk.cx, chunk.cy);
    let entry = this.#textures.get(key);
    if (entry === undefined) {
      const texture = gl.createTexture();
      gl.bindTexture(gl.TEXTURE_2D, texture);
      gl.texStorage2D(gl.TEXTURE_2D, 1, gl.R8UI, CHUNK_SIDE, CHUNK_SIDE);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
      entry = { texture, owned: chunk.owned };
      this.#textures.set(key, entry);
    }
    entry.owned = chunk.owned;
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
      chunk.tiles,
    );
  }

  remove(cx: number, cy: number): void {
    const key = chunkKey(cx, cy);
    const entry = this.#textures.get(key);
    if (entry !== undefined) {
      this.#gl.deleteTexture(entry.texture);
      this.#textures.delete(key);
    }
    this.layers.remove(cx, cy);
  }

  /** 範囲の中の、テクスチャのあるチャンクを描き、描いた数を返す。 */
  draw(camera: Camera, viewport: Viewport, range: ChunkRange): number {
    const gl = this.#gl;
    const dpr = window.devicePixelRatio || 1;
    const width = Math.round(viewport.width * dpr);
    const height = Math.round(viewport.height * dpr);
    if (this.canvas.width !== width || this.canvas.height !== height) {
      this.canvas.width = width;
      this.canvas.height = height;
    }
    gl.viewport(0, 0, width, height);
    gl.clearColor(...OUTSIDE_COLOR, 1);
    gl.clear(gl.COLOR_BUFFER_BIT);
    gl.useProgram(this.#program);
    gl.bindVertexArray(this.#vao);
    gl.uniform1i(this.#u.zonable, this.showZonable ? 1 : 0);
    gl.uniform2f(this.#u.viewport, viewport.width, viewport.height);
    const size = CHUNK_SIDE * camera.zoom;
    gl.uniform1f(this.#u.size, size);
    const weights = stageWeights(camera.zoom);
    gl.uniform1f(this.#u.block, weights.blocks);
    gl.uniform1f(this.#u.pattern, weights.detail);
    let drawn = 0;
    for (let cy = range.minCy; cy <= range.maxCy; cy++) {
      for (let cx = range.minCx; cx <= range.maxCx; cx++) {
        const entry = this.#textures.get(chunkKey(cx, cy));
        if (entry === undefined) {
          continue;
        }
        gl.uniform2f(
          this.#u.offset,
          (cx * CHUNK_SIDE - camera.x) * camera.zoom,
          (cy * CHUNK_SIDE - camera.y) * camera.zoom,
        );
        gl.uniform1i(this.#u.owned, entry.owned ? 1 : 0);
        gl.uniform1f(this.#u.hatch, ((cx + cy) * CHUNK_SIDE * camera.zoom) % HATCH_PERIOD);
        gl.bindTexture(gl.TEXTURE_2D, entry.texture);
        gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
        drawn++;
      }
    }
    gl.bindVertexArray(null);
    this.roads.draw(camera, viewport);
    this.layers.draw(camera, viewport, range);
    if (this.demand !== null) {
      this.#drawDemand(camera, viewport, range, this.demand);
    }
    // 道具の仮の表示は、主題図に隠れないよう最後に描く
    this.roads.draw(camera, viewport, "preview");
    return drawn;
  }

  #drawDemand(
    camera: Camera,
    viewport: Viewport,
    range: ChunkRange,
    demand: readonly number[],
  ): void {
    const gl = this.#gl;
    gl.useProgram(this.#demandProgram);
    gl.bindVertexArray(this.#vao);
    gl.enable(gl.BLEND);
    gl.blendFuncSeparate(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA, gl.ZERO, gl.ONE);
    gl.uniform2f(this.#du.viewport, viewport.width, viewport.height);
    gl.uniform1f(this.#du.size, CHUNK_SIDE * camera.zoom);
    gl.uniform1f(this.#du.wash, WASH);
    gl.uniform1fv(
      this.#du.share,
      [0, 1, 2, 3].map((i) => demandShare(demand[i] ?? 0)),
    );
    for (let cy = range.minCy; cy <= range.maxCy; cy++) {
      for (let cx = range.minCx; cx <= range.maxCx; cx++) {
        const entry = this.#textures.get(chunkKey(cx, cy));
        if (entry === undefined) {
          continue;
        }
        gl.uniform2f(
          this.#du.offset,
          (cx * CHUNK_SIDE - camera.x) * camera.zoom,
          (cy * CHUNK_SIDE - camera.y) * camera.zoom,
        );
        gl.uniform1i(this.#du.owned, entry.owned ? 1 : 0);
        gl.bindTexture(gl.TEXTURE_2D, entry.texture);
        gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
      }
    }
    gl.disable(gl.BLEND);
    gl.bindVertexArray(null);
  }
}
