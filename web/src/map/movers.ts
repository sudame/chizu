// 三重バッファの移動体の枠を読み、WebGL2 で描く。歩く人は赤紫の丸、車は向きに沿った小さな長方形にする。
// 移動体の座標は枠の基準のチャンクの原点からのマスの数なので、カメラからの相対は基準の原点だけを
// CPU で求めて渡す。遠くの座標でも f32 の桁が足りるようにするためで、道路の描き方と同じである。

import { CHUNK_SIDE, type MoverArea } from "../protocol/messages.ts";
import { layout, type SnapshotHeader } from "../protocol/snapshot.ts";
import type { Camera, ChunkRange, Viewport } from "./camera.ts";
import { SIDEWALK_WIDTH } from "./lanes.ts";
import { CAR_COLORS, WALKER_COLOR } from "./mover-colors.ts";
import { MoverInterpolator } from "./mover-interpolation.ts";

/** 種類の番号。chizu-wasm の movers::KIND_WALK などと同じ。 */
export const KIND_WALK = 1;
export const KIND_CAR = 2;

/** これより縮小したら（1 マスがこのピクセル数より小さいと）移動体を描かず、Worker にも求めない。 */
export const MIN_MOVER_ZOOM = 2;

export function showsMovers(zoom: number): boolean {
  return zoom >= MIN_MOVER_ZOOM;
}

const glslVec3 = ([r, g, b]: readonly number[]) => `vec3(${r}, ${g}, ${b})`;

const VERTEX = `#version 300 es
in vec2 a_corner;
in vec2 a_pos;
in uint a_heading_kind;
in uint a_id;
uniform vec2 u_offset;
uniform float u_zoom;
uniform vec2 u_viewport;
out vec2 v_corner;
out vec2 v_half_px;
flat out uint v_kind;
flat out vec3 v_color;
const vec3 CAR[${CAR_COLORS.length}] = vec3[${CAR_COLORS.length}](
  ${CAR_COLORS.map(glslVec3).join(",\n  ")}
);
void main() {
  uint kind = a_heading_kind >> 16u;
  float angle = float(a_heading_kind & 0xffffu) * (6.2831853 / 65536.0);
  vec2 along = vec2(cos(angle), sin(angle));
  vec2 across = vec2(-along.y, along.x);
  // 半分の大きさ（マス）。1 マスは 8m。車はいちばん狭い車線（大きい道路の 1.1m）に収まる幅にし、
  // 長さは幅の 3 倍ほどにして、車線の線と見分けられる形にする。人は、中核が置く歩道の中心から
  // 歩道の幅（0.08 マス）に収まる丸にし、車道やマスの外にはみ出さないようにする。
  // 縮小しても形が分かるよう、画面のピクセルの下限を置く
  vec2 half_px = kind == ${KIND_CAR}u
    ? max(vec2(0.2, 0.065) * u_zoom, vec2(2.0, 1.0))
    : vec2(max(${SIDEWALK_WIDTH / 2} * u_zoom, 1.5));
  vec2 p = a_pos * u_zoom + u_offset + along * a_corner.x * half_px.x + across * a_corner.y * half_px.y;
  p /= u_viewport * 0.5;
  gl_Position = vec4(p.x, -p.y, 0.0, 1.0);
  v_corner = a_corner;
  v_half_px = half_px;
  v_kind = kind;
  // mover-colors.ts の carColorIndex と同じ計算で、ID から車の色を選ぶ
  uint h = a_id * 0x9e3779b1u;
  h = (h ^ (h >> 16u)) * 0x85ebca6bu;
  h ^= h >> 13u;
  v_color = kind == ${KIND_CAR}u ? CAR[h % ${CAR_COLORS.length}u] : ${glslVec3(WALKER_COLOR)};
}`;

// 車は少し暗い縁と、前（進む向き）の白い窓を描き、向きが分かるようにする。人は白い縁の丸にする。
// 縁と窓は、画面で描ける大きさのときだけ描く
const FRAGMENT = `#version 300 es
precision mediump float;
in vec2 v_corner;
in vec2 v_half_px;
flat in uint v_kind;
flat in vec3 v_color;
out vec4 color;
void main() {
  if (v_kind == ${KIND_CAR}u) {
    vec2 d = (1.0 - abs(v_corner)) * v_half_px;
    vec3 c = v_color;
    if (v_half_px.y >= 2.0 && min(d.x, d.y) < 0.75) {
      c *= 0.7;
    } else if (v_half_px.x >= 4.0 && v_corner.x > 0.25 && v_corner.x < 0.6) {
      c = mix(c, vec3(1.0), 0.55);
    }
    color = vec4(c, 1.0);
    return;
  }
  float r = length(v_corner);
  if (r > 1.0) {
    discard;
  }
  bool rim = v_half_px.x >= 2.0 && (1.0 - r) * v_half_px.x < 1.0;
  color = vec4(rim ? vec3(1.0) : v_color, 1.0);
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
  readonly #interpolator = new MoverInterpolator();
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
    const id = gl.getAttribLocation(program, "a_id");
    gl.enableVertexAttribArray(id);
    gl.vertexAttribIPointer(id, 1, gl.UNSIGNED_INT, stride, layout.mover.id * 4);
    gl.vertexAttribDivisor(id, 1);
    gl.bindVertexArray(null);
  }

  /**
   * 枠の移動体を描き、描いた数を返す。縮小しすぎているときは描かない。
   * 位置は前の tick と今の tick のあいだを、時刻 `now`（ミリ秒）に合わせて補間する。
   */
  draw(
    camera: Camera,
    viewport: Viewport,
    header: SnapshotHeader,
    words: Uint32Array,
    now: number,
  ): number {
    const count = Math.min(header.moverCount, Math.floor(words.length / layout.moverWords));
    this.firstOf.clear();
    if (!showsMovers(camera.zoom) || count === 0) {
      this.first = null;
      return 0;
    }
    const gl = this.#gl;
    const used = count * layout.moverWords;
    this.#copy.set(words.subarray(0, used));
    this.#interpolator.apply(
      {
        tick: header.tick,
        speed: header.speed,
        paused: header.paused,
        baseCx: header.baseCx,
        baseCy: header.baseCy,
        count,
        words: this.#copy,
        floats: this.#floats,
      },
      now,
      this.#floats,
    );
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
