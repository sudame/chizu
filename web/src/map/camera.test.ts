import { describe, expect, test } from "vitest";
import { Camera, chunksByDistance, MAX_ZOOM, MIN_ZOOM } from "./camera.ts";

const viewport = { width: 800, height: 600 };

describe("Camera", () => {
  test("画面の中心はカメラの位置に、ワールドとの変換は行き来で戻る", () => {
    const camera = new Camera(100, -50, 4);
    expect(camera.screenToWorld(400, 300, viewport)).toEqual({ x: 100, y: -50 });
    expect(camera.screenToWorld(0, 0, viewport)).toEqual({ x: 0, y: -125 });
    const back = camera.worldToScreen(37.5, 12.25, viewport);
    const world = camera.screenToWorld(back.x, back.y, viewport);
    expect(world.x).toBeCloseTo(37.5);
    expect(world.y).toBeCloseTo(12.25);
  });

  test("引きずると、つかんだワールドの点がカーソルに付いてくる", () => {
    const camera = new Camera(0, 0, 8);
    const grabbed = camera.screenToWorld(200, 100, viewport);
    camera.pan(80, -40);
    expect(camera.screenToWorld(280, 60, viewport)).toEqual(grabbed);
  });

  test("拡大縮小しても、カーソルの下のワールドの点は動かない", () => {
    const camera = new Camera(10, 20, 8);
    const under = camera.screenToWorld(650, 120, viewport);
    camera.zoomAt(650, 120, 1.7, viewport);
    expect(camera.zoom).toBeCloseTo(13.6);
    const after = camera.screenToWorld(650, 120, viewport);
    expect(after.x).toBeCloseTo(under.x);
    expect(after.y).toBeCloseTo(under.y);
  });

  test("拡大率は上限と下限で止まり、止まっても点はずれない", () => {
    const camera = new Camera(0, 0, 8);
    const under = camera.screenToWorld(100, 500, viewport);
    camera.zoomAt(100, 500, 1000, viewport);
    expect(camera.zoom).toBe(MAX_ZOOM);
    expect(camera.screenToWorld(100, 500, viewport).x).toBeCloseTo(under.x);
    camera.zoomAt(100, 500, 1e-6, viewport);
    expect(camera.zoom).toBe(MIN_ZOOM);
    expect(camera.screenToWorld(100, 500, viewport).y).toBeCloseTo(under.y);
    expect(new Camera(0, 0, 0).zoom).toBe(MIN_ZOOM);
  });

  test("見えているチャンクの範囲は、負の座標と境目を正しく扱う", () => {
    // 画面は x が -100〜100、y が -75〜75 のワールドを映す
    const camera = new Camera(0, 0, 4);
    expect(camera.visibleChunks(viewport)).toEqual({ minCx: -2, maxCx: 1, minCy: -2, maxCy: 1 });
    // 右端がちょうど x = 128 のときは、チャンク 2 を含めない
    const edge = new Camera(28, 32, 4);
    expect(edge.visibleChunks(viewport)).toEqual({ minCx: -2, maxCx: 1, minCy: -1, maxCy: 1 });
  });
});

describe("chunksByDistance", () => {
  test("中心に近いチャンクから並ぶ", () => {
    const order = chunksByDistance({ minCx: -1, maxCx: 1, minCy: 0, maxCy: 0 }, { x: 100, y: 32 });
    expect(order).toEqual([
      [1, 0],
      [0, 0],
      [-1, 0],
    ]);
  });
});
