// 三重バッファの列車の車両の欄を読み、WebGL2 で描く（docs/map-style.md の「移動体」の列車）。
// 車両は移動体と同じ語の並びで届く。座標は枠の車両の基準のチャンクの原点からのマスの数で、
// 補間は移動体と同じ MoverInterpolator に任せる（車両ごとに、前の tick と今の tick のあいだを動かす）。
//
// 段ごとの見せ方：
// - 引き：編成ごとに 1 つの点（路線の色の丸に白い縁）
// - 標準：1 両ずつの角丸の長方形（20m × 2.9m）を路線の色で
// - 寄り：窓の帯と、先頭の運転席の窓
// - 細部：窓を 1 両 4 つに分け、屋根を少し明るくする
// 点と 4 両の形は、倍率 3〜5 のあいだで入れ替える（stages.ts の TRAIN_BAND）。

import { CHUNK_SIDE } from "../protocol/messages.ts";
import { layout, type SnapshotHeader } from "../protocol/snapshot.ts";
import type { Camera, Viewport } from "./camera.ts";
import { LINE_COLORS } from "./lines.ts";
import { MoverInterpolator } from "./mover-interpolation.ts";
import { hexRgb } from "./palette.ts";
import { stageWeights } from "./stages.ts";

/** 種類の番号。chizu-wasm の trains::KIND_TRAIN と同じ。種類の上位 16 ビットの下位 2 ビットが種類で、残りが路線の色の番号。 */
export const KIND_TRAIN = 3;
/**
 * 混み具合の段階（0〜3）を置く位置。種類の上位 16 ビットのうち、路線の色の番号（3 ビット）の上の 2 ビットである。
 * chizu-wasm の trains::LOAD_SHIFT と同じ。窓の色が段階とともに車体の色へ寄る（空いた列車は明るく、混んだ列車は濃い窓）。
 */
export const LOAD_SHIFT = 5;

/** 1 編成の両数。chizu-wasm の trains::CARS_PER_TRAIN と同じ。 */
export const CARS_PER_TRAIN = 4;

/** 1 両の描く大きさの半分（マス）。20m の車両の長さから、両のあいだに隙間を作る分だけ短くする。幅は 2.9m。 */
export const CAR_HALF = [1.18, 0.18] as const;
/** 画面の半分の大きさの下限（ピクセル）。 */
const CAR_MIN_PX = [2, 1.2] as const;
/** 引きの点の半径（ピクセル）。 */
export const DOT_RADIUS_PX = 3.2;
/** 窓の帯と、先頭の窓（車両の四角の中の座標で、-1〜1。x の正が進む向き）。 */
const WINDOW_BAND = [0.8, 0.55] as const;
const CAB_WINDOW = [0.78, 0.62] as const;
const WINDOW_MIX = "0.55";
/** 混み具合の段階 3 で、窓の明るさを車体の色へ寄せる割合。 */
const LOAD_DARKEN = "0.6";
const ROOF_LIGHTEN = "0.12";

const glslVec3 = ([r, g, b]: readonly number[]) => `vec3(${r}, ${g}, ${b})`;

const VERTEX = `#version 300 es
in vec2 a_corner;
in vec2 a_pos;
in uint a_heading_kind;
in uint a_id;
uniform vec2 u_offset;
uniform float u_zoom;
uniform vec2 u_viewport;
uniform float u_dot;
out vec2 v_local;
out vec2 v_half_px;
flat out vec3 v_color;
flat out float v_car;
flat out float v_load;
const vec3 LINE[${LINE_COLORS.length}] = vec3[${LINE_COLORS.length}](
  ${LINE_COLORS.map((c) => glslVec3(hexRgb(c))).join(",\n  ")}
);
void main() {
  uint raw = a_heading_kind >> 16u;
  uint color = (raw >> 2u) & 7u;
  uint load = (raw >> ${LOAD_SHIFT}u) & 3u;
  uint car = a_id & 3u;
  // 点は編成ごとに 1 つ（前から 2 両目）だけ描く。ほかの車両は画面の外へ追い出す
  if (u_dot > 0.5 && car != 1u) {
    gl_Position = vec4(2.0, 2.0, 2.0, 1.0);
    v_local = vec2(0.0);
    v_half_px = vec2(1.0);
    v_color = vec3(0.0);
    v_car = 0.0;
    v_load = 0.0;
    return;
  }
  float angle = float(a_heading_kind & 0xffffu) * (6.2831853 / 65536.0);
  vec2 along = vec2(cos(angle), sin(angle));
  vec2 across = vec2(-along.y, along.x);
  vec2 half_px = u_dot > 0.5
    ? vec2(${DOT_RADIUS_PX})
    : max(vec2(${CAR_HALF[0]}, ${CAR_HALF[1]}) * u_zoom, vec2(${CAR_MIN_PX[0]}, ${CAR_MIN_PX[1]}));
  vec2 local = a_corner * (half_px + 1.0);
  vec2 p = a_pos * u_zoom + u_offset + along * local.x + across * local.y;
  p /= u_viewport * 0.5;
  gl_Position = vec4(p.x, -p.y, 0.0, 1.0);
  v_local = local;
  v_half_px = half_px;
  v_color = LINE[color % ${LINE_COLORS.length}u];
  v_car = float(car);
  v_load = float(load);
}`;

// 縁は 1px の暗い線。四角は 1px 広げて描き、縁のなめらかな端を入れる。
// 寄り（u_detail）で窓の帯と先頭の窓を、細部（u_fine）で窓を 4 つに分けて屋根を明るくする。
const FRAGMENT = `#version 300 es
precision mediump float;
in vec2 v_local;
in vec2 v_half_px;
flat in vec3 v_color;
flat in float v_car;
flat in float v_load;
uniform float u_alpha;
uniform float u_detail;
uniform float u_fine;
// 頂点のシェーダーと同じ精度にそろえる（そろえないと、つなぐときに断られる）
uniform highp float u_dot;
out vec4 color;
void main() {
  vec3 c = v_color;
  if (u_dot > 0.5) {
    float r = length(v_local);
    float edge = clamp(v_half_px.x + 0.5 - r, 0.0, 1.0);
    if (edge <= 0.0) {
      discard;
    }
    vec3 rim = r > v_half_px.x - 1.2 ? vec3(1.0) : c;
    color = vec4(rim, edge * u_alpha);
    return;
  }
  // 角丸の長方形までの符号つきの距離（ピクセル）
  float radius = min(v_half_px.y * 0.7, 3.0);
  vec2 q = abs(v_local) - (v_half_px - radius);
  float dist = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - radius;
  float edge = clamp(0.5 - dist, 0.0, 1.0);
  if (edge <= 0.0) {
    discard;
  }
  vec3 body = dist > -1.0 && v_half_px.y >= 2.0 ? c * 0.7 : c;
  vec2 k = v_local / v_half_px;
  if (dist <= -1.0 && v_half_px.y >= 2.0) {
    // 窓の帯。細部では 4 つに分ける
    float band = step(abs(k.x), ${WINDOW_BAND[0]}) * step(abs(k.y), ${WINDOW_BAND[1]});
    float t = fract((k.x * 0.5 + 0.5) * 4.0);
    float segments = band * step(0.14, t) * step(t, 0.86);
    float window = mix(band, segments, u_fine) * u_detail;
    // 先頭の車両の、進む向きの端の運転席の窓
    float cab = (v_car < 0.5 ? 1.0 : 0.0) * step(${CAB_WINDOW[0]}, k.x) * step(abs(k.y), ${CAB_WINDOW[1]}) * u_detail;
    // 窓は、混んだ列車ほど車体の色へ寄せて濃くする（段階 3 で窓の明るさの差が 0.4 倍ほどになる）
    float windowMix = ${WINDOW_MIX} * (1.0 - ${LOAD_DARKEN} * v_load / 3.0);
    body = mix(body, mix(c, vec3(1.0), windowMix), max(window, cab));
    float roof = (1.0 - band) * (1.0 - step(${CAB_WINDOW[0]}, k.x)) * u_fine;
    body = mix(body, mix(c, vec3(1.0), ${ROOF_LIGHTEN}), roof);
  }
  color = vec4(body, edge * u_alpha);
}`;

export class TrainLayer {
  readonly #gl: WebGL2RenderingContext;
  readonly #program: WebGLProgram;
  readonly #vao: WebGLVertexArrayObject;
  readonly #instances: WebGLBuffer;
  readonly #u: Record<
    "offset" | "zoom" | "viewport" | "alpha" | "dot" | "detail" | "fine",
    WebGLUniformLocation | null
  >;
  // WebGL に SharedArrayBuffer を直に渡せないブラウザがあるので、枠の中身をここに写してから送る
  readonly #copy = new Uint32Array(layout.maxTrainCars * layout.moverWords);
  readonly #floats = new Float32Array(this.#copy.buffer);
  readonly #interpolator = new MoverInterpolator();
  /** 最後に描いた枠の、最初の車両のワールド座標（マス）。test:wasm が位置の変化を見るのに使う。 */
  first: { x: number; y: number } | null = null;
  /** 最後に描いた枠の車両の数。 */
  cars = 0;

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
      alpha: gl.getUniformLocation(program, "u_alpha"),
      dot: gl.getUniformLocation(program, "u_dot"),
      detail: gl.getUniformLocation(program, "u_detail"),
      fine: gl.getUniformLocation(program, "u_fine"),
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
   * 枠の車両を描き、描いた車両の数を返す。列車は範囲によらず全部が届くので、画面に入るかは見ない。
   * 位置は前の tick と今の tick のあいだを、時刻 `now`（ミリ秒）に合わせて補間する。
   */
  draw(
    camera: Camera,
    viewport: Viewport,
    header: SnapshotHeader,
    words: Uint32Array,
    now: number,
  ): number {
    const count = Math.min(header.trainCarCount, Math.floor(words.length / layout.moverWords));
    this.cars = count;
    if (count === 0) {
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
        baseCx: header.trainBaseCx,
        baseCy: header.trainBaseCy,
        count,
        words: this.#copy,
        floats: this.#floats,
      },
      now,
      this.#floats,
    );
    this.first = {
      x: header.trainBaseCx * CHUNK_SIDE + (this.#floats[layout.mover.x] ?? 0),
      y: header.trainBaseCy * CHUNK_SIDE + (this.#floats[layout.mover.y] ?? 0),
    };
    const weights = stageWeights(camera.zoom);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.#instances);
    gl.bufferSubData(gl.ARRAY_BUFFER, 0, this.#copy, 0, used);
    gl.useProgram(this.#program);
    gl.uniform1f(this.#u.zoom, camera.zoom);
    gl.uniform2f(this.#u.viewport, viewport.width, viewport.height);
    gl.uniform2f(
      this.#u.offset,
      (header.trainBaseCx * CHUNK_SIDE - camera.x) * camera.zoom,
      (header.trainBaseCy * CHUNK_SIDE - camera.y) * camera.zoom,
    );
    gl.uniform1f(this.#u.detail, weights.detail);
    gl.uniform1f(this.#u.fine, weights.fine);
    gl.enable(gl.BLEND);
    // 色だけを重ね、フレームバッファのアルファは 1 のまま保つ（キャンバスがページの背景と混ざって白く飛ばないように）
    gl.blendFuncSeparate(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA, gl.ZERO, gl.ONE);
    gl.bindVertexArray(this.#vao);
    for (const [dot, alpha] of [
      [1, weights.trainDot],
      [0, weights.trains],
    ] as const) {
      if (alpha <= 0) {
        continue;
      }
      gl.uniform1f(this.#u.dot, dot);
      gl.uniform1f(this.#u.alpha, alpha);
      gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, count);
    }
    gl.bindVertexArray(null);
    gl.disable(gl.BLEND);
    return count;
  }
}
