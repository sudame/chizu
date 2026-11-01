// 三重バッファの移動体の枠を読み、WebGL2 で描く。歩く人は小さな点、車は向きに沿った小さな長方形にする。
// 移動体の座標は枠の基準のチャンクの原点からのマスの数なので、カメラからの相対は基準の原点だけを
// CPU で求めて渡す。遠くの座標でも f32 の桁が足りるようにするためで、道路の描き方と同じである。

import { CHUNK_SIDE, type MoverArea } from "../protocol/messages.ts";
import { layout, type SnapshotHeader } from "../protocol/snapshot.ts";
import type { Camera, ChunkRange, Viewport } from "./camera.ts";

/** 種類の番号。chizu-wasm の movers::KIND_WALK などと同じ。 */
export const KIND_WALK = 1;
export const KIND_CAR = 2;

/** これより縮小したら（1 マスがこのピクセル数より小さいと）移動体を描かず、Worker にも求めない。 */
export const MIN_MOVER_ZOOM = 2;

export function showsMovers(zoom: number): boolean {
  return zoom >= MIN_MOVER_ZOOM;
}

const VERTEX = `#version 300 es
in vec2 a_corner;
in vec2 a_pos;
in uint a_heading_kind;
uniform vec2 u_offset;
uniform float u_zoom;
uniform vec2 u_viewport;
out vec2 v_corner;
flat out uint v_kind;
void main() {
  uint kind = a_heading_kind >> 16u;
  float angle = float(a_heading_kind & 0xffffu) * (6.2831853 / 65536.0);
  vec2 along = vec2(cos(angle), sin(angle));
  vec2 across = vec2(-along.y, along.x);
  // 半分の大きさ（マス）。1 マスは 8m で、車は 4.8m 長さで、いちばん狭い車線（大きい道路の 1.1m）に収まる幅にする。
  // 人は歩道（0.64m）に収まる大きさにする。縮小しても消えないよう下限を置く
  vec2 half_px = kind == ${KIND_CAR}u
    ? max(vec2(0.3, 0.06) * u_zoom, vec2(2.0, 1.0))
    : vec2(max(0.035 * u_zoom, 1.5));
  vec2 p = a_pos * u_zoom + u_offset + along * a_corner.x * half_px.x + across * a_corner.y * half_px.y;
  p /= u_viewport * 0.5;
  gl_Position = vec4(p.x, -p.y, 0.0, 1.0);
  v_corner = a_corner;
  v_kind = kind;
}`;

// 色は CS2 の情報表示のように、地図の上で目立ちすぎない明るい灰色に寄せる
const FRAGMENT = `#version 300 es
precision mediump float;
in vec2 v_corner;
flat in uint v_kind;
out vec4 color;
void main() {
  if (v_kind == ${KIND_CAR}u) {
    color = vec4(0.82, 0.84, 0.9, 0.95);
    return;
  }
  if (dot(v_corner, v_corner) > 1.0) {
    discard;
  }
  color = vec4(0.96, 0.92, 0.78, 0.95);
}`;

export class MoverLayer {
  readonly #gl: WebGL2RenderingContext;
  readonly #program: WebGLProgram;
  readonly #vao: WebGLVertexArrayObject;
  readonly #instances: WebGLBuffer;
  readonly #u: Record<"offset" | "zoom" | "viewport", WebGLUniformLocation | null>;
  // WebGL に SharedArrayBuffer を直に渡せないブラウザがあるので、枠の中身をここに写してから送る
  readonly #copy = new Uint32Array(layout.maxMovers * layout.moverWords);
  readonly #floats = new Float32Array(this.#copy.buffer);
  /** 最後に描いた枠の、最初の移動体のワールド座標（マス）。test:wasm が位置の変化を見るのに使う。 */
  first: { x: number; y: number } | null = null;
  /** 最後に描いた枠の、種類ごとの最初の移動体のワールド座標（マス）。test:wasm が画素を見るのに使う。 */
  firstOf: Map<number, { x: number; y: number }> = new Map();

  constructor(
    gl: WebGL2RenderingContext,
    link: (vertex: string, fragment: string) => WebGLProgram,
  ) {
    this.#gl = gl;
    this.#program = link(VERTEX, FRAGMENT);
    const program = this.#program;
    this.#u = {
      offset: gl.getUniformLocation(program, "u_offset"),
      zoom: gl.getUniformLocation(program, "u_zoom"),
      viewport: gl.getUniformLocation(program, "u_viewport"),
    };
    this.#vao = gl.createVertexArray();
    gl.bindVertexArray(this.#vao);
    gl.bindBuffer(gl.ARRAY_BUFFER, gl.createBuffer());
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1, 1, 1]), gl.STATIC_DRAW);
    const corner = gl.getAttribLocation(program, "a_corner");
    gl.enableVertexAttribArray(corner);
    gl.vertexAttribPointer(corner, 2, gl.FLOAT, false, 0, 0);

    this.#instances = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, this.#instances);
    gl.bufferData(gl.ARRAY_BUFFER, this.#copy.byteLength, gl.DYNAMIC_DRAW);
    const stride = layout.moverWords * 4;
    const pos = gl.getAttribLocation(program, "a_pos");
    gl.enableVertexAttribArray(pos);
    gl.vertexAttribPointer(pos, 2, gl.FLOAT, false, stride, layout.mover.x * 4);
    gl.vertexAttribDivisor(pos, 1);
    const headingKind = gl.getAttribLocation(program, "a_heading_kind");
    gl.enableVertexAttribArray(headingKind);
    gl.vertexAttribIPointer(headingKind, 1, gl.UNSIGNED_INT, stride, layout.mover.headingKind * 4);
    gl.vertexAttribDivisor(headingKind, 1);
    gl.bindVertexArray(null);
  }

  /** 枠の移動体を描き、描いた数を返す。縮小しすぎているときは描かない。 */
  draw(camera: Camera, viewport: Viewport, header: SnapshotHeader, words: Uint32Array): number {
    const count = Math.min(header.moverCount, Math.floor(words.length / layout.moverWords));
    this.firstOf.clear();
    if (!showsMovers(camera.zoom) || count === 0) {
      this.first = null;
      return 0;
    }
    const gl = this.#gl;
    const used = count * layout.moverWords;
    this.#copy.set(words.subarray(0, used));
    const at = (i: number) => ({
      x: header.baseCx * CHUNK_SIDE + (this.#floats[i * layout.moverWords + layout.mover.x] ?? 0),
      y: header.baseCy * CHUNK_SIDE + (this.#floats[i * layout.moverWords + layout.mover.y] ?? 0),
    });
    this.first = at(0);
    for (let i = 0; i < count && this.firstOf.size < 2; i++) {
      const kind = (this.#copy[i * layout.moverWords + layout.mover.headingKind] ?? 0) >>> 16;
      if (!this.firstOf.has(kind)) {
        this.firstOf.set(kind, at(i));
      }
    }
    gl.bindBuffer(gl.ARRAY_BUFFER, this.#instances);
    gl.bufferSubData(gl.ARRAY_BUFFER, 0, this.#copy, 0, used);
    gl.useProgram(this.#program);
    gl.uniform1f(this.#u.zoom, camera.zoom);
    gl.uniform2f(this.#u.viewport, viewport.width, viewport.height);
    gl.uniform2f(
      this.#u.offset,
      (header.baseCx * CHUNK_SIDE - camera.x) * camera.zoom,
      (header.baseCy * CHUNK_SIDE - camera.y) * camera.zoom,
    );
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
    gl.bindVertexArray(this.#vao);
    gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, count);
    gl.bindVertexArray(null);
    gl.disable(gl.BLEND);
    return count;
  }
}

/** 移動体を求めるマスの範囲。見えているチャンクの範囲をマスに直す。縮小しすぎているときは null。 */
export function moverArea(range: ChunkRange, zoom: number): MoverArea | null {
  if (!showsMovers(zoom)) {
    return null;
  }
  return {
    minX: range.minCx * CHUNK_SIDE,
    minY: range.minCy * CHUNK_SIDE,
    maxX: (range.maxCx + 1) * CHUNK_SIDE - 1,
    maxY: (range.maxCy + 1) * CHUNK_SIDE - 1,
  };
}
