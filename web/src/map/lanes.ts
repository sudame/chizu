// 道路の断面。車線と歩道の数と幅を、区間の種類の番号から求める。
// 幅は中核の network.rs の SegmentKind::lane_width_mm と sidewalk_width_mm（1 マス = 8,000mm）と同じにする。
// 道路は 1 マスに収める。半分の幅は小 0.3、中 0.4、大 0.5、高速 0.5 マスで、車線の幅は種類ごとに変わる。
// 移動体を車線の上に描くずれは中核が求めるので、ここでは道路の線の組み立てにだけ使う。

/** 歩道 1 本の幅（マス）。 */
export const SIDEWALK_WIDTH = 0.08;

const LANE_WIDTHS: Record<number, number> = { 1: 0.22, 2: 0.16, 3: 0.14, 4: 0.25 };

/** 車線 1 本の幅（マス）。 */
export function laneWidth(kindCode: number): number {
  return LANE_WIDTHS[kindCode] ?? 0.22;
}

/** 区間の種類の番号。chizu-wasm の segment_kind_code と同じ。 */
export const HIGHWAY_CODE = 4;

const LANES_PER_SIDE: Record<number, number> = { 1: 1, 2: 2, 3: 3, [HIGHWAY_CODE]: 2 };

/** 片側の車の車線の数。 */
export function lanesPerSide(kindCode: number): number {
  return LANES_PER_SIDE[kindCode] ?? 1;
}

/** 歩道があるか。高速道路にはない。 */
export function hasSidewalks(kindCode: number): boolean {
  return kindCode !== HIGHWAY_CODE;
}

/** 中心線から道路の端（歩道を含む）までの半分の幅（マス）。 */
export function roadHalfWidth(kindCode: number): number {
  return carriagewayHalfWidth(kindCode) + (hasSidewalks(kindCode) ? SIDEWALK_WIDTH : 0);
}

/** 中心線から車道の端までの半分の幅（マス）。 */
export function carriagewayHalfWidth(kindCode: number): number {
  return lanesPerSide(kindCode) * laneWidth(kindCode);
}

export type StripRole = "asphalt" | "sidewalk" | "center" | "divider";

/** 中心線から `offset` だけ横にずらした、半分の太さ `half` の帯。単位はマス。 */
export type Strip = { role: StripRole; offset: number; half: number };

/**
 * 道路の断面を構成する帯を、下から重ねる順に返す。
 * 車道、両端の歩道、上りと下りのあいだの中央線、同じ向きの車線のあいだの区切りの線である。
 */
export function crossSection(kindCode: number): Strip[] {
  const lanes = lanesPerSide(kindCode);
  const edge = carriagewayHalfWidth(kindCode);
  const strips: Strip[] = [{ role: "asphalt", offset: 0, half: edge }];
  if (hasSidewalks(kindCode)) {
    for (const side of [-1, 1]) {
      strips.push({
        role: "sidewalk",
        offset: side * (edge + SIDEWALK_WIDTH / 2),
        half: SIDEWALK_WIDTH / 2,
      });
    }
  }
  strips.push({ role: "center", offset: 0, half: CENTER_LINE_HALF });
  for (let i = 1; i < lanes; i++) {
    for (const side of [-1, 1]) {
      strips.push({ role: "divider", offset: side * i * laneWidth(kindCode), half: DIVIDER_HALF });
    }
  }
  return strips;
}

const CENTER_LINE_HALF = 0.02;
const DIVIDER_HALF = 0.012;

/**
 * 車線の中心の、中心線からの横のずれ（マス）。進む向きの左を正とし、番号 0 が道路の端にいちばん近い。
 * 中核の lane_shift::lateral_mm と同じ値になる。
 */
export function laneCenter(kindCode: number, index: number): number {
  const lanes = lanesPerSide(kindCode);
  const i = Math.min(Math.max(index, 0), lanes - 1);
  return (lanes - i - 0.5) * laneWidth(kindCode);
}

/** 道路の線をこの倍率（1 マスのピクセル数）以上で車線ごとに描く。これより縮小したら 1 本の線にする。 */
export const LANE_DETAIL_ZOOM = 6;

/** 車線ごとの描き方に切り替える倍率か。 */
export function showsLanes(zoom: number): boolean {
  return zoom >= LANE_DETAIL_ZOOM;
}
