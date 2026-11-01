import { describe, expect, test } from "vitest";
import { CHUNK_AREA, type WorkerToMain } from "../protocol/messages.ts";
import { LAYER_CHECK_MS, LayerSender, type LayerSource } from "./layer-sync.ts";
import { DATA_LAYERS, PALETTE_SIZE, paletteArray } from "./layers.ts";

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

describe("データレイヤーの色", () => {
  test("色の表は uniform の長さにそろえ、値 0 は塗らない", () => {
    for (const layer of DATA_LAYERS) {
      const a = paletteArray(layer.palette);
      expect(a).toHaveLength(PALETTE_SIZE * 4);
      expect(a[3]).toBe(0);
      expect(layer.palette.length).toBeLessThanOrEqual(PALETTE_SIZE);
    }
  });
});
