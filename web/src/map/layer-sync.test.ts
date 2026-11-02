import { describe, expect, test } from "vitest";
import { CHUNK_AREA, type WorkerToMain } from "../protocol/messages.ts";
import { LAYER_CHECK_MS, LayerSender, type LayerSource, TrafficSender } from "./layer-sync.ts";
import {
  DATA_LAYERS,
  PALETTE_SIZE,
  styleArrays,
  TRAFFIC_LAYER,
  TRAFFIC_LEVELS,
  WASH,
} from "./layers.ts";
import { hex } from "./palette.ts";

function fakeSource() {
  const calls: string[] = [];
  const source: LayerSource & { version: number; calls: string[] } = {
    version: 1,
    calls,
    layer_version(code) {
      calls.push(`version ${code}`);
      return this.version * 10 + code;
    },
    layer_chunk(code, cx, cy) {
      calls.push(`chunk ${code} ${cx},${cy}`);
      return new Uint8Array(CHUNK_AREA);
    },
  };
  return source;
}

function collect(sender: LayerSender, source: LayerSource, now?: number) {
  const sent: WorkerToMain[] = [];
  sender.send(source, (m) => sent.push(m), 32, now);
  return sent.flatMap((m) =>
    m.kind === "layer-chunks" ? m.chunks.map((c) => `${m.layer}:${c.cx},${c.cy}@${c.version}`) : [],
  );
}

const view = [
  { cx: 0, cy: 0 },
  { cx: 1, cy: 0 },
];

describe("LayerSender", () => {
  test("出しているレイヤーだけを、版が変わったときだけ送る", () => {
    const sender = new LayerSender();
    const source = fakeSource();
    sender.setView(view, []);
    expect(collect(sender, source)).toEqual([]);
    sender.setActive([1]);
    expect(collect(sender, source)).toEqual(["1:0,0@11", "1:1,0@11"]);
    expect(collect(sender, source)).toEqual([]);
    source.version = 2;
    expect(collect(sender, source)).toEqual(["1:0,0@21", "1:1,0@21"]);
    // 重ねたレイヤーは、それぞれ送る
    sender.setActive([1, 2]);
    expect(collect(sender, source)).toEqual(["1:0,0@21", "1:1,0@21", "2:0,0@22", "2:1,0@22"]);
  });

  test("時刻を渡すと、版を確かめ直すのは間隔をおいてからにする", () => {
    const sender = new LayerSender();
    const source = fakeSource();
    sender.setView(view, []);
    sender.setActive([1]);
    expect(collect(sender, source, 1000)).toHaveLength(2);
    source.version = 2;
    expect(collect(sender, source, 1000 + LAYER_CHECK_MS - 1)).toEqual([]);
    expect(collect(sender, source, 1000 + LAYER_CHECK_MS)).toHaveLength(2);
  });

  test("画面が捨てたチャンクは、次に見えたときに送り直す", () => {
    const sender = new LayerSender();
    const source = fakeSource();
    sender.setActive([1]);
    sender.setView(view, []);
    expect(collect(sender, source)).toHaveLength(2);
    sender.setView([{ cx: 1, cy: 0 }], [{ cx: 0, cy: 0 }]);
    sender.setView(view, []);
    expect(collect(sender, source)).toEqual(["1:0,0@11"]);
  });
});

describe("交通の区間ごとの値の送り方", () => {
  const trafficSource = () => ({
    version: 1,
    network: 7,
    sent: 0,
    layer_version(code: number) {
      return this.version * 10 + code;
    },
    network_version() {
      return this.network;
    },
    traffic_segments() {
      this.sent++;
      return new Uint8Array([1, 4]);
    },
  });

  test("交通を出しているあいだだけ、版が変わったときに道路網の版と一緒に送る", () => {
    const sender = new TrafficSender();
    const source = trafficSource();
    const sent: WorkerToMain[] = [];
    const reply = (m: WorkerToMain) => sent.push(m);
    sender.send(source, reply);
    expect(sent).toHaveLength(0);
    sender.setActive([TRAFFIC_LAYER]);
    sender.send(source, reply, 1000);
    expect(sent).toEqual([{ kind: "traffic", revision: 7, values: new Uint8Array([1, 4]).buffer }]);
    // 間隔の中では確かめず、版が同じなら送らない
    source.version = 2;
    sender.send(source, reply, 1000 + LAYER_CHECK_MS - 1);
    expect(sent).toHaveLength(1);
    sender.send(source, reply, 1000 + LAYER_CHECK_MS);
    expect(sent).toHaveLength(2);
    sender.send(source, reply, 1000 + 2 * LAYER_CHECK_MS);
    expect(sent).toHaveLength(2);
    sender.setActive([1]);
    sender.send(source, reply);
    expect(sent).toHaveLength(2);
  });

  test("交通はマスの値では送らない", () => {
    const sender = new LayerSender();
    const source = fakeSource();
    sender.setActive([TRAFFIC_LAYER]);
    sender.setView([{ cx: 0, cy: 0 }], []);
    const sent: WorkerToMain[] = [];
    sender.send(source, (m) => sent.push(m), 10);
    expect(sent).toHaveLength(0);
  });
});

describe("データレイヤーの色", () => {
  test("塗り方の並びは uniform の長さにそろえ、値 0 は塗らずに下の地図を薄めるだけにする", () => {
    for (const layer of DATA_LAYERS) {
      const { palette, accent, style } = styleArrays(layer.styles);
      expect(palette).toHaveLength(PALETTE_SIZE * 4);
      expect(accent).toHaveLength(PALETTE_SIZE * 4);
      expect(style).toHaveLength(PALETTE_SIZE);
      expect(style[0]).toBe(0);
      expect(layer.styles.length).toBeLessThanOrEqual(PALETTE_SIZE);
    }
  });

  test("電気と水道は、決まりの色で建物と道路を塗り、施設は薄めずに見せる", () => {
    const power = DATA_LAYERS.find((l) => l.code === 1);
    const { palette, accent, style } = styleArrays(power?.styles ?? []);
    const color = (i: number) => Array.from(palette.subarray(i * 4, i * 4 + 4));
    const f32 = (c: readonly number[]) => Array.from(new Float32Array(c));
    // 値は chizu-wasm の layers::SUPPLY_BUILDING_ON などの並び
    expect(color(1)).toEqual(f32(hex("#81c995")));
    expect(style[2]).toBe(2);
    expect(color(2)).toEqual(f32(hex("#f28b82")));
    expect(Array.from(accent.subarray(8, 12))).toEqual(f32(hex("#d93025")));
    expect(color(3)).toEqual(f32(hex("#1a73e8")));
    expect(color(4)).toEqual(f32(hex("#f29900")));
    expect(style[5]).toBe(3);
    expect(color(5)).toEqual(f32(hex("#9aa0a6")));
    expect(style[6]).toBe(4);
    expect(WASH).toBe(0.5);
  });

  test("交通は面で塗らず、4 段の色は決まりの色で、明るさも段々に暗くなる", () => {
    expect(DATA_LAYERS.some((l) => l.code === TRAFFIC_LAYER)).toBe(false);
    expect(TRAFFIC_LEVELS.map((l) => l.label)).toEqual([
      "空いている",
      "やや混んでいる",
      "混んでいる",
      "詰まっている",
    ]);
    expect(TRAFFIC_LEVELS.map((l) => l.color)).toEqual(
      ["#63d668", "#ff974d", "#f23c32", "#811f1f"].map((c) => hex(c)),
    );
    const luma = ([r, g, b]: readonly number[]) =>
      0.2126 * (r ?? 0) + 0.7152 * (g ?? 0) + 0.0722 * (b ?? 0);
    const lumas = TRAFFIC_LEVELS.map((l) => luma(l.color));
    for (let i = 1; i < lumas.length; i++) {
      expect(lumas[i]).toBeLessThan(lumas[i - 1] ?? 0);
    }
  });
});
