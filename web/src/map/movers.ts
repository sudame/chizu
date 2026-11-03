// 三重バッファの移動体の枠を読み、WebGL2 で描く。歩く人は赤紫の丸、車は向きに沿った小さな長方形にする。
// 移動体の座標は枠の基準のチャンクの原点からのマスの数なので、カメラからの相対は基準の原点だけを
// CPU で求めて渡す。遠くの座標でも f32 の桁が足りるようにするためで、道路の描き方と同じである。

import { CHUNK_SIDE, type MoverArea } from "../protocol/messages.ts";
import { layout, type SnapshotHeader } from "../protocol/snapshot.ts";
import type { Camera, ChunkRange, Viewport } from "./camera.ts";
import { SIDEWALK_WIDTH } from "./lanes.ts";
import { CAR_COLORS, WALKER_COLOR } from "./mover-colors.ts";
import { MoverInterpolator, type MoverPaths } from "./mover-interpolation.ts";
import { hex, hexRgb } from "./palette.ts";
import { showsMovers, stageWeights } from "./stages.ts";

/** 種類の番号。chizu-wasm の movers::KIND_WALK などと同じ。 */
export const KIND_WALK = 1;
export const KIND_CAR = 2;

const glslVec3 = ([r, g, b]: readonly number[]) => `vec3(${r}, ${g}, ${b})`;

/** 車の影。`#3c4043` の 25% を、右下へ画面で 1px ずらす。細部の段でだけ描く。 */
export const CAR_SHADOW_COLOR = hex("#3c4043", 0.25);
const SHADOW_RGB = CAR_SHADOW_COLOR.slice(0, 3);
const SHADOW_SHIFT_PX = 1;
// 細部の段の、車の窓と屋根の位置。車の四角の中の座標（前後と左右に -1〜1、x の正が進む向き）で書く。
// 前の窓を後ろの窓より広くして、どちらへ進むかを示す。屋根は車の色を白へ 15% 寄せる
export const CAR_FRONT_WINDOW = [0.2, 0.6] as const;
export const CAR_REAR_WINDOW = [-0.72, -0.5] as const;
const CAR_WINDOW_SIDE = "0.72";
const CAR_ROOF_LIGHTEN = "0.15";
// 細部の段の、人の肩の楕円の半径（進む向き、横）と、頭の丸の半径。人の四角の中の座標で書く
export const WALKER_SHOULDER = [0.5, 1.0] as const;
export const WALKER_HEAD = 0.38;
/** 細部の段の、人の頭の色。 */
export const WALKER_HEAD_COLOR = hexRgb("#5f3a2e");

const VERTEX = `#version 300 es
in vec2 a_corner;
in vec2 a_pos;
in uint a_heading_kind;
in uint a_id;
uniform vec2 u_offset;
uniform float u_zoom;
uniform vec2 u_viewport;
uniform float u_fine;
// v_local は、移動体の中心からの、進む向きと横の向きの画面のピクセル。v_shadow は、影のずれ（右下へ
// ${SHADOW_SHIFT_PX}px）を同じ向きで表したもの
out vec2 v_local;
out vec2 v_shadow;
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
  // 細部の段では、車の影を同じ四角の中で描くので、四角を影のずれの分だけ広げる
  vec2 quad = kind == ${KIND_CAR}u && u_fine > 0.0 ? half_px + ${SHADOW_SHIFT_PX}.0 : half_px;
  vec2 local = a_corner * quad;
  vec2 p = a_pos * u_zoom + u_offset + along * local.x + across * local.y;
  p /= u_viewport * 0.5;
  gl_Position = vec4(p.x, -p.y, 0.0, 1.0);
  v_local = local;
  v_shadow = vec2(dot(vec2(${SHADOW_SHIFT_PX}.0), along), dot(vec2(${SHADOW_SHIFT_PX}.0), across));
  v_half_px = half_px;
  v_kind = kind;
  // mover-colors.ts の carColorIndex と同じ計算で、ID から車の色を選ぶ
  uint h = a_id * 0x9e3779b1u;
  h = (h ^ (h >> 16u)) * 0x85ebca6bu;
  h ^= h >> 13u;
  v_color = kind == ${KIND_CAR}u ? CAR[h % ${CAR_COLORS.length}u] : ${glslVec3(WALKER_COLOR)};
}`;

// 車は少し暗い縁と、前（進む向き）の白い窓を描き、向きが分かるようにする。人は白い縁の丸にする。
// 縁と窓は、画面で描ける大きさのときだけ描く。
// 細部の段（u_fine）では、車に後ろの窓と、少し明るい屋根を描き足す。窓は前を広くする。
// 人は、白い縁の丸から、進む向きに直角な肩の楕円と、その上の頭の丸の形に移す。
// 車の影は、車の四角の外の、右下へずらした四角に入る所だけを影の色で塗る。
// 影のために描く回を分けると、ソフトウェアの描画で 1 フレームが数 ms 重くなるので、同じ回で描く
const FRAGMENT = `#version 300 es
precision mediump float;
in vec2 v_local;
in vec2 v_shadow;
in vec2 v_half_px;
flat in uint v_kind;
flat in vec3 v_color;
uniform float u_alpha;
// 頂点のシェーダーと同じ精度にそろえる（そろえないと、つなぐときに断られる）
uniform highp float u_fine;
out vec4 color;
void main() {
  vec2 v_corner = v_local / v_half_px;
  if (v_kind == ${KIND_CAR}u) {
    if (abs(v_corner.x) > 1.0 || abs(v_corner.y) > 1.0) {
      vec2 s = abs(v_local - v_shadow);
      if (s.x > v_half_px.x || s.y > v_half_px.y) {
        discard;
      }
      color = vec4(${glslVec3(SHADOW_RGB)}, ${CAR_SHADOW_COLOR[3]} * u_fine * u_alpha);
      return;
    }
    vec2 d = (1.0 - abs(v_corner)) * v_half_px;
    vec3 c = v_color;
    vec3 coarse = c;
    vec3 fine = c;
    if (v_half_px.y >= 2.0 && min(d.x, d.y) < 0.75) {
      coarse = c * 0.7;
      fine = coarse;
    } else {
      if (v_half_px.x >= 4.0 && v_corner.x > 0.25 && v_corner.x < 0.6) {
        coarse = mix(c, vec3(1.0), 0.55);
      }
      float side = abs(v_corner.y);
      if (side < ${CAR_WINDOW_SIDE}) {
        float x = v_corner.x;
        if (x > ${CAR_FRONT_WINDOW[0]} && x < ${CAR_FRONT_WINDOW[1]}) {
          fine = mix(c, vec3(1.0), 0.55);
        } else if (x > ${CAR_REAR_WINDOW[0]} && x < ${CAR_REAR_WINDOW[1]}) {
          fine = mix(c, vec3(1.0), 0.55);
        } else if (x >= ${CAR_REAR_WINDOW[1]} && x <= ${CAR_FRONT_WINDOW[0]}) {
          fine = mix(c, vec3(1.0), ${CAR_ROOF_LIGHTEN});
        }
      }
    }
    color = vec4(mix(coarse, fine, u_fine), u_alpha);
    return;
  }
  float r = length(v_corner);
  bool rim = v_half_px.x >= 2.0 && (1.0 - r) * v_half_px.x < 1.0;
  vec4 coarse = r > 1.0 ? vec4(0.0) : vec4(rim ? vec3(1.0) : v_color, 1.0);
  vec4 fine = vec4(0.0);
  if (u_fine > 0.0) {
    // 進む向き（x）に薄く、横（y）に広い肩の楕円と、真ん中の頭の丸
    vec2 q = v_corner / vec2(${WALKER_SHOULDER[0]}, ${WALKER_SHOULDER[1]});
    float shoulder = length(q);
    float head = length(v_corner) / ${WALKER_HEAD};
    // 楕円の縁までの距離を画面のピクセルで近似し（楕円の式の値を、画面のピクセルあたりの変わり方で割る）、
    // どの向きでも 1px の縁にする
    vec2 slope = q / (vec2(${WALKER_SHOULDER[0]}, ${WALKER_SHOULDER[1]}) * v_half_px.x);
    float perPx = length(slope) / max(shoulder, 1e-4);
    if (head <= 1.0) {
      fine = vec4(${glslVec3(WALKER_HEAD_COLOR)}, 1.0);
    } else if (shoulder <= 1.0) {
      fine = vec4(1.0 - shoulder < perPx ? vec3(1.0) : v_color, 1.0);
    }
  }
  vec4 c = mix(coarse, fine, u_fine);
  if (c.a <= 0.0) {
    discard;
  }
  color = vec4(c.rgb / c.a, c.a * u_alpha);
}`;

export class MoverLayer {
  readonly #gl: WebGL2RenderingContext;
  readonly #program: WebGLProgram;
  readonly #vao: WebGLVertexArrayObject;
  readonly #instances: WebGLBuffer;
  readonly #u: Record<
    "offset" | "zoom" | "viewport" | "alpha" | "fine",
    WebGLUniformLocation | null
  >;
  // WebGL に SharedArrayBuffer を直に渡せないブラウザがあるので、枠の中身をここに写してから送る
  readonly #copy = new Uint32Array(layout.maxMovers * layout.moverWords);
  readonly #floats = new Float32Array(this.#copy.buffer);
  readonly #interpolator = new MoverInterpolator();
  /** 描いたすべての枠で、道筋に沿って補間した移動体と、届いている道筋を指す移動体の延べ数。test:wasm が読む。 */
  get alongPaths(): { along: number; routed: number } {
    return { along: this.#interpolator.along, routed: this.#interpolator.routed };
  }
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
      alpha: gl.getUniformLocation(program, "u_alpha"),
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
   * 枠の移動体を描き、描いた数を返す。寄りの段より引いているときは描かない。
   * 寄りの段に入るあいだ（stages.ts の MOVER_BAND）は、薄く出す。
   * 位置は前の tick と今の tick のあいだを、時刻 `now`（ミリ秒）に合わせて補間する。車は `paths` の道筋に沿わせる。
   */
  draw(
    camera: Camera,
    viewport: Viewport,
    header: SnapshotHeader,
    words: Uint32Array,
    now: number,
    paths?: MoverPaths,
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
      paths,
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
    const weights = stageWeights(camera.zoom);
    gl.uniform2f(this.#u.viewport, viewport.width, viewport.height);
    gl.uniform2f(
      this.#u.offset,
      (header.baseCx * CHUNK_SIDE - camera.x) * camera.zoom,
      (header.baseCy * CHUNK_SIDE - camera.y) * camera.zoom,
    );
    gl.enable(gl.BLEND);
    // 色だけを重ね、フレームバッファのアルファは 1 のまま保つ（キャンバスがページの背景と混ざって白く飛ばないように）
    gl.blendFuncSeparate(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA, gl.ZERO, gl.ONE);
    gl.bindVertexArray(this.#vao);
    gl.uniform1f(this.#u.fine, weights.fine);
    gl.uniform1f(this.#u.alpha, weights.movers);
    gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, count);
    gl.bindVertexArray(null);
    gl.disable(gl.BLEND);
    return count;
  }
}

/** 移動体を求めるマスの範囲。見えているチャンクの範囲をマスに直す。寄りの段より引いているときは null。 */
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
