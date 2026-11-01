// WebGL2 でチャンクを描く。チャンクごとに 64×64 の整数テクスチャを 1 枚持ち、1 枚の四角に貼る。
// 位置はカメラからの相対で CPU が求めて渡す。遠くの座標でも f32 の桁が足りるようにするためである。

import { CHUNK_SIDE, TILE_WATER, TILE_ZONABLE, TILE_ZONE_SHIFT } from "../protocol/messages.ts";
import type { Camera, ChunkRange, Viewport } from "./camera.ts";
import { chunkKey, type StoredChunk } from "./chunk-sync.ts";
import { MoverLayer } from "./movers.ts";
import { RoadLayer } from "./roads.ts";

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
in vec2 v_uv;
out vec4 color;
// 区画の色は CS2 にならう。住宅は緑、商業は青、工業は黄、オフィスは水色
const vec3 ZONE[4] = vec3[4](
  vec3(0.15, 0.95, 0.15),
  vec3(0.25, 0.45, 0.95),
  vec3(0.95, 0.8, 0.2),
  vec3(0.3, 0.85, 0.95)
);
void main() {
  uint tile = texture(u_tiles, v_uv).r;
  vec3 c = (tile & ${TILE_WATER}u) != 0u ? vec3(0.24, 0.48, 0.72) : vec3(0.56, 0.72, 0.42);
  uint zone = (tile >> ${TILE_ZONE_SHIFT}u) & 7u;
  if (zone >= 1u && zone <= 4u) {
    c = mix(c, ZONE[zone - 1u], 0.55);
  } else if (u_zonable && (tile & ${TILE_ZONABLE}u) != 0u) {
    c = mix(c, vec3(1.0), 0.3);
  }
  color = vec4(u_owned ? c : c * 0.45, 1.0);
}`;

const BACKGROUND = [0.1, 0.1, 0.12] as const;

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
  /** 移動体。道路の上に重ねて描く。 */
  readonly movers: MoverLayer;
  readonly #textures = new Map<string, Texture>();
  readonly #u: Record<
    "offset" | "size" | "viewport" | "tiles" | "owned" | "zonable",
    WebGLUniformLocation
  >;
  /** 区画を塗れるマスを明るく示すか。区画の道具を選んでいるあいだだけ true にする。 */
  showZonable = false;

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
    };
    gl.uniform1i(this.#u.tiles, 0);

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
    this.roads = new RoadLayer(gl, (v, f) => link(gl, v, f));
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
    gl.clearColor(...BACKGROUND, 1);
    gl.clear(gl.COLOR_BUFFER_BIT);
    gl.useProgram(this.#program);
    gl.bindVertexArray(this.#vao);
    gl.uniform1i(this.#u.zonable, this.showZonable ? 1 : 0);
    gl.uniform2f(this.#u.viewport, viewport.width, viewport.height);
    const size = CHUNK_SIDE * camera.zoom;
    gl.uniform1f(this.#u.size, size);
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
        gl.bindTexture(gl.TEXTURE_2D, entry.texture);
        gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
        drawn++;
      }
    }
    gl.bindVertexArray(null);
    this.roads.draw(camera, viewport);
    return drawn;
  }
}
