import { describe, expect, test } from "vitest";
import { moverArea } from "./movers.ts";
import {
  BLOCK_BAND,
  DETAIL_BAND,
  FINE_BAND,
  MIN_MOVER_ZOOM,
  MOVER_BAND,
  showsFine,
  showsMovers,
  smoothstep,
  stageWeights,
  TRAFFIC_FREE_BAND,
} from "./stages.ts";

describe("縮尺の段", () => {
  test("smoothstep は幅の始めで 0、終わりで 1 になり、そのあいだで増える", () => {
    expect(smoothstep(3, 5, 2)).toBe(0);
    expect(smoothstep(3, 5, 3)).toBe(0);
    expect(smoothstep(3, 5, 4)).toBeCloseTo(0.5);
    expect(smoothstep(3, 5, 5)).toBe(1);
    expect(smoothstep(3, 5, 9)).toBe(1);
    expect(smoothstep(3, 5, 3.5)).toBeLessThan(smoothstep(3, 5, 3.6));
  });

  test("引きでは街区の面だけを描き、人と車は描かない", () => {
    const w = stageWeights(2);
    expect(w.blocks).toBe(1);
    expect(w.clusters).toBe(0);
    expect(w.detail).toBe(0);
    expect(w.movers).toBe(0);
    expect(w.marks).toBe(0);
    expect(w.minorRoad).toBeCloseTo(0.55);
  });

  test("標準では建物のまとまりを描き、小の道路も濃く描く", () => {
    const w = stageWeights(8);
    expect(w.blocks).toBe(0);
    expect(w.clusters).toBe(1);
    expect(w.detail).toBe(0);
    expect(w.movers).toBe(0);
    expect(w.minorRoad).toBe(1);
  });

  test("寄りでは 1 軒ずつの描き方と、車と人を描く", () => {
    const w = stageWeights(20);
    expect(w.clusters).toBe(0);
    expect(w.detail).toBe(1);
    expect(w.movers).toBe(1);
  });

  test("段の境目の幅のあいだは、前の段と次の段を薄く重ねる", () => {
    const block = stageWeights((BLOCK_BAND[0] + BLOCK_BAND[1]) / 2);
    expect(block.blocks).toBeCloseTo(0.5);
    expect(block.clusters).toBeCloseTo(0.5);
    const detail = stageWeights((DETAIL_BAND[0] + DETAIL_BAND[1]) / 2);
    expect(detail.detail).toBeCloseTo(0.5);
    expect(detail.clusters).toBeCloseTo(0.5);
    const movers = stageWeights((MOVER_BAND[0] + MOVER_BAND[1]) / 2);
    expect(movers.movers).toBeCloseTo(0.5);
  });

  test("交通の主題図の「空いている」は、引きでは描かず、4〜6 で現れる", () => {
    expect(stageWeights(3).trafficFree).toBe(0);
    expect(stageWeights((TRAFFIC_FREE_BAND[0] + TRAFFIC_FREE_BAND[1]) / 2).trafficFree).toBeCloseTo(
      0.5,
    );
    expect(stageWeights(8).trafficFree).toBe(1);
  });

  test("細部は 32〜48 で現れ、寄りの描き方は消さない", () => {
    expect(FINE_BAND).toEqual([32, 48]);
    expect(stageWeights(20).fine).toBe(0);
    expect(stageWeights(32).fine).toBe(0);
    expect(showsFine(32)).toBe(false);
    expect(showsFine(32.01)).toBe(true);
    expect(stageWeights(40).fine).toBeCloseTo(0.5);
    const top = stageWeights(128);
    expect(top.fine).toBe(1);
    expect(top.detail).toBe(1);
    expect(top.movers).toBe(1);
    expect(top.clusters).toBe(0);
  });

  test("寄りの段より引いたら、Worker に移動体の範囲を頼まない", () => {
    expect(MIN_MOVER_ZOOM).toBe(12);
    expect(showsMovers(MIN_MOVER_ZOOM)).toBe(true);
    expect(showsMovers(MIN_MOVER_ZOOM - 0.01)).toBe(false);
    const range = { minCx: 0, maxCx: 1, minCy: 0, maxCy: 0 };
    expect(moverArea(range, 11.9)).toBeNull();
    expect(moverArea(range, 12)).toEqual({ minX: 0, minY: 0, maxX: 127, maxY: 63 });
  });

  test("列車は引きでは点、標準からは 4 両の形で、入れ替わるあいだは両方が出る", () => {
    expect(stageWeights(2)).toMatchObject({ trainDot: 1, trains: 0 });
    expect(stageWeights(8)).toMatchObject({ trainDot: 0, trains: 1 });
    const mid = stageWeights(4);
    expect(mid.trainDot).toBeCloseTo(0.5);
    expect(mid.trains).toBeCloseTo(0.5);
  });
});
