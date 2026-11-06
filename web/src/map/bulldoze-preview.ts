// 取り壊しの範囲をドラッグしているあいだ、実際に消えるものを示す部分。DOM に触らないので、Vitest で確かめられる。

import type { Tile } from "./roads.ts";

/** 長方形の両端のマス。 */
export type TileBox = { min: Tile; max: Tile };

/** 中核の `bulldoze_preview` の答え。 */
export type BulldozePreview = {
  /** 消える道路のマスの数。 */
  roadTiles: number;
  /** 消える線路のマスの数。駅の中を含む。 */
  railTiles: number;
  /** 家をなくす世帯の数。 */
  displacedHouseholds: number;
  /** 消える道路の区間（交差点から交差点まで）の両端のマス。 */
  segments: { from: Tile; to: Tile }[];
  facilities: TileBox[];
  buildings: TileBox[];
  stations: TileBox[];
  /** 消える線路（ノードからノードまで）の両端のマス。 */
  tracks: { from: Tile; to: Tile }[];
};

/** 先頭の個数の欄の数。chizu-wasm の `Sim::bulldoze_preview` と同じ。 */
const HEADER = 8;

/** `bulldoze_preview` の数の並びを読む。空や形の合わないものは、消えるものがないとして `null` を返す。 */
export function parseBulldozePreview(info: ArrayLike<number>): BulldozePreview | null {
  if (info.length < HEADER) {
    return null;
  }
  const at0 = (i: number): number => info[i] ?? 0;
  const [roadTiles, facilities, buildings, displacedHouseholds, segments] = [
    at0(0),
    at0(1),
    at0(2),
    at0(3),
    at0(4),
  ] as const;
  const [railTiles, stations, tracks] = [at0(5), at0(6), at0(7)] as const;
  if (info.length !== HEADER + 4 * (segments + facilities + buildings + stations + tracks)) {
    return null;
  }
  let at = HEADER;
  const next = (): [number, number, number, number] => {
    const v: [number, number, number, number] = [at0(at), at0(at + 1), at0(at + 2), at0(at + 3)];
    at += 4;
    return v;
  };
  const lines = (n: number): { from: Tile; to: Tile }[] =>
    Array.from({ length: n }, () => {
      const [x0, y0, x1, y1] = next();
      return { from: { x: x0, y: y0 }, to: { x: x1, y: y1 } };
    });
  const segs = lines(segments);
  const boxes = (n: number): TileBox[] =>
    Array.from({ length: n }, () => {
      const [x0, y0, x1, y1] = next();
      return { min: { x: x0, y: y0 }, max: { x: x1, y: y1 } };
    });
  const facilityBoxes = boxes(facilities);
  const buildingBoxes = boxes(buildings);
  const stationBoxes = boxes(stations);
  return {
    roadTiles,
    railTiles,
    displacedHouseholds,
    segments: segs,
    facilities: facilityBoxes,
    buildings: buildingBoxes,
    stations: stationBoxes,
    tracks: lines(tracks),
  };
}

/** 消えるものがあるか。 */
export function isEmptyPreview(p: BulldozePreview): boolean {
  return (
    p.segments.length === 0 &&
    p.facilities.length === 0 &&
    p.buildings.length === 0 &&
    p.stations.length === 0 &&
    p.tracks.length === 0
  );
}

/** 道具の説明の欄に出す、消える数。消えるものがなければ空の文字列。 */
export function bulldozeSummary(p: BulldozePreview): string {
  const parts: string[] = [];
  if (p.roadTiles > 0) {
    parts.push(`道路 ${p.roadTiles.toLocaleString("ja-JP")} マス`);
  }
  if (p.railTiles > 0) {
    parts.push(`線路 ${p.railTiles.toLocaleString("ja-JP")} マス`);
  }
  if (p.buildings.length > 0) {
    parts.push(`建物 ${p.buildings.length.toLocaleString("ja-JP")} 棟`);
  }
  if (p.facilities.length > 0) {
    parts.push(`施設 ${p.facilities.length.toLocaleString("ja-JP")}`);
  }
  if (p.stations.length > 0) {
    parts.push(`駅 ${p.stations.length.toLocaleString("ja-JP")}`);
  }
  if (p.displacedHouseholds > 0) {
    parts.push(`家をなくす世帯 ${p.displacedHouseholds.toLocaleString("ja-JP")}`);
  }
  return parts.length === 0 ? "" : `消える：${parts.join("・")}`;
}

/** 区間の直線 a〜b が通るマス（縦横と 45 度）。 */
function tilesOnSegment(a: Tile, b: Tile): Tile[] {
  const sx = Math.sign(b.x - a.x);
  const sy = Math.sign(b.y - a.y);
  const len = Math.max(Math.abs(b.x - a.x), Math.abs(b.y - a.y));
  return Array.from({ length: len + 1 }, (_, k) => ({ x: a.x + sx * k, y: a.y + sy * k }));
}

/** 消える区間が通るマス。交差点のマスは 1 度だけ数える。 */
export function removedRoadTiles(p: BulldozePreview): Tile[] {
  const seen = new Set<string>();
  const out: Tile[] = [];
  for (const s of p.segments) {
    for (const t of tilesOnSegment(s.from, s.to)) {
      const key = `${t.x},${t.y}`;
      if (!seen.has(key)) {
        seen.add(key);
        out.push(t);
      }
    }
  }
  return out;
}
