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
  /** 消える道路のマス。ほかの区間が残って通るマスは入らず、個数は `roadTiles` と同じ。 */
  roadTileList: Tile[];
  /** 消える線路のマス。ほかの線路が残って通るマスは入らず、個数は `railTiles` と同じ。 */
  railTileList: Tile[];
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
  if (
    info.length !==
    HEADER +
      4 * (segments + facilities + buildings + stations + tracks) +
      2 * (roadTiles + railTiles)
  ) {
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
  const trackLines = lines(tracks);
  const tileList = (n: number): Tile[] =>
    Array.from({ length: n }, () => {
      const x = at0(at);
      const y = at0(at + 1);
      at += 2;
      return { x, y };
    });
  const roadTileList = tileList(roadTiles);
  const railTileList = tileList(railTiles);
  return {
    roadTiles,
    railTiles,
    displacedHouseholds,
    segments: segs,
    facilities: facilityBoxes,
    buildings: buildingBoxes,
    stations: stationBoxes,
    tracks: trackLines,
    roadTileList,
    railTileList,
  };
}

/** 消える線路のマス。中核が数えたマスと同じで、ほかの線路が残って通るマスは入らない。 */
export function removedRailTiles(p: BulldozePreview): Tile[] {
  return p.railTileList;
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
  if (parts.length === 0) {
    return "";
  }
  const note = removesCrossing(p) ? "（踏切のマスを含むので線路も消える）" : "";
  return `消える：${parts.join("・")}${note}`;
}

/** 消える道路のマスと線路のマスが重なるか。重なるのは踏切のマスで、道路の区間と線路が一緒に消える。 */
export function removesCrossing(p: BulldozePreview): boolean {
  const rail = new Set(p.railTileList.map((t) => `${t.x},${t.y}`));
  return p.roadTileList.some((t) => rail.has(`${t.x},${t.y}`));
}

/** 消える道路のマス。中核が数えたマスと同じで、ほかの区間が残って通るマスは入らない。 */
export function removedRoadTiles(p: BulldozePreview): Tile[] {
  return p.roadTileList;
}
