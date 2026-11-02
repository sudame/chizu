// 道路、地区、施設の名前の規則。名前は状態に持たず、種と形から決まった規則で作る（docs/map-style.md「名前の付け方」）。
// 表と規則を変えると、既存の街の名前も変わる。変えるときは PR に「名前が変わる」と書く。

/** 種。中核の種は u64 なので、数、bigint、10 進の文字列のどれでも受ける。 */
export type Seed = number | bigint | string;

/** 名前の前半に使う和語の地名風の語。道路と地区で共有する。 */
export const NAME_WORDS: readonly string[] = [
  "桜",
  "楓",
  "柳",
  "欅",
  "若葉",
  "瑞穂",
  "日向",
  "松",
  "梅",
  "桃",
  "杉",
  "檜",
  "椿",
  "藤",
  "萩",
  "菊",
  "蓮",
  "葵",
  "橘",
  "榊",
  "栗",
  "栃",
  "樫",
  "楠",
  "柏",
  "桐",
  "竹",
  "稲穂",
  "大和",
  "旭",
  "朝日",
  "夕日",
  "緑",
  "青葉",
  "白河",
  "黒川",
  "赤坂",
  "清水",
  "宝",
  "星",
  "月見",
  "雲雀",
  "鶴",
  "亀",
  "鳩",
  "燕",
  "鷹",
  "富士",
  "筑波",
  "霞",
  "朝霧",
  "春日",
  "秋津",
  "千歳",
  "常盤",
  "栄",
  "幸",
  "弥生",
  "睦",
  "三島",
  "八幡",
  "芦原",
  "紅葉",
  "水無月",
];

/** 地区の名前の後半。チャンクに水辺があるときは WATERSIDE_SUFFIX にする。 */
export const DISTRICT_SUFFIXES: readonly string[] = ["町", "台", "丘", "原", "野", "本町"];
export const WATERSIDE_SUFFIX = "浜";

/** 地区の単位。チャンクの一辺のマス数。 */
export const CHUNK_SIZE = 64;

/** 区間の種類の番号（chizu-wasm の segment_kind_code と同じ）。 */
const KIND_LARGE = 3;
const KIND_HIGHWAY = 4;

/** 施設の種類の番号（ServiceKind の値）と、名前の後半。 */
const FACILITY_NAMES: Record<number, string> = { 1: "発電所", 2: "浄水場" };
const FACILITY_FALLBACK = "施設";

const ORDINALS = ["", "第二", "第三", "第四", "第五", "第六", "第七", "第八", "第九", "第十"];

const encoder = new TextEncoder();

/** FNV-1a（32 ビット）。 */
export function fnv1a(text: string): number {
  let hash = 0x811c9dc5;
  for (const byte of encoder.encode(text)) {
    hash ^= byte;
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return hash >>> 0;
}

function pick<T>(items: readonly T[], hash: number): T {
  return items[hash % items.length] as T;
}

export type Point = { x: number; y: number };

/**
 * 区間の端点から、直線の鍵を求める。同じ直線の上の区間は、分けても、端点の向きを逆にしても同じ鍵になる。
 * 横は `H{y}`、縦は `V{x}`、右下がりの斜めは `D{x - y}`、右上がりの斜めは `A{x + y}`。
 * 45 度でない向きは道路の線として作れないが、念のため端点の組を鍵にする。
 */
export function roadLineKey(a: Point, b: Point): string {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  if (dy === 0) return `H${a.y}`;
  if (dx === 0) return `V${a.x}`;
  if (dx === dy) return `D${a.x - a.y}`;
  if (dx === -dy) return `A${a.x + a.y}`;
  const forward = a.x < b.x || (a.x === b.x && a.y <= b.y);
  const [p, q] = forward ? [a, b] : [b, a];
  return `X${p.x},${p.y},${q.x},${q.y}`;
}

/** 区間の種類による名前の後半。小と中は「通り」、大は「大通り」、高速は「自動車道」。 */
export function roadSuffix(kindCode: number): string {
  if (kindCode === KIND_HIGHWAY) return "自動車道";
  if (kindCode === KIND_LARGE) return "大通り";
  return "通り";
}

/** 道路の名前。前半は直線の鍵と種で決まり、種類を上げたときは末尾だけが変わる。 */
export function roadName(seed: Seed, lineKey: string, kindCode: number): string {
  const word = pick(NAME_WORDS, fnv1a(`${seed}/road/${lineKey}`));
  return `${word}${roadSuffix(kindCode)}`;
}

/** 区間の端点と種類から、道路の名前を求める。 */
export function segmentRoadName(seed: Seed, a: Point, b: Point, kindCode: number): string {
  return roadName(seed, roadLineKey(a, b), kindCode);
}

/** マスの座標が属するチャンクの座標。 */
export function chunkOf(tile: number): number {
  return Math.floor(tile / CHUNK_SIZE);
}

const permutations = new Map<string, readonly number[]>();

/** 種ごとの語の並べ替え。Fisher-Yates を、種から決まるハッシュで回す。 */
function wordPermutation(seed: Seed): readonly number[] {
  const key = String(seed);
  const cached = permutations.get(key);
  if (cached) return cached;
  const perm = NAME_WORDS.map((_, i) => i);
  for (let i = perm.length - 1; i > 0; i--) {
    const j = fnv1a(`${key}/perm/${i}`) % (i + 1);
    [perm[i], perm[j]] = [perm[j] as number, perm[i] as number];
  }
  if (permutations.size > 16) permutations.clear();
  permutations.set(key, perm);
  return perm;
}

const mod = (n: number, m: number) => ((n % m) + m) % m;

// チャンクが 1 つ進むごとに、並べ替えの表を進める幅。8 近傍の差（±1、±9、±8、±10）が、どれも 64 の倍数にならない。
const STEP_X = 1;
const STEP_Y = 9;

/**
 * 地区の名前。地区はチャンク（64 × 64 マス）である。
 * 前半の語は、種から決まる並べ替えの表を、チャンクの座標に応じて進んで選ぶ。8 近傍のチャンクは必ず別の語になる。
 * 後半は、種とチャンクの座標のハッシュで選ぶ。`waterside` は、チャンクに水辺があるとき。後半が「浜」になる。
 */
export function districtName(seed: Seed, cx: number, cy: number, waterside = false): string {
  const offset = fnv1a(`${seed}/district-offset`);
  const index = mod(offset + cx * STEP_X + cy * STEP_Y, NAME_WORDS.length);
  const word = NAME_WORDS[wordPermutation(seed)[index] as number] as string;
  if (waterside) return `${word}${WATERSIDE_SUFFIX}`;
  return `${word}${pick(DISTRICT_SUFFIXES, fnv1a(`${seed}/district/${cx},${cy}`))}`;
}

/** 施設の種類の名前。 */
export function facilityKindName(kind: number): string {
  return FACILITY_NAMES[kind] ?? FACILITY_FALLBACK;
}

/** 同じ地区の同じ種類の中での順番（0 から）による、種類の前に付ける語。0 番目は付けない。 */
export function ordinalPrefix(ordinal: number): string {
  return ORDINALS[ordinal] ?? `第${ordinal + 1}`;
}

/** 施設の名前。`ordinal` は、同じ地区の同じ種類のまとまりの中での順番（0 から）。 */
export function facilityName(district: string, kind: number, ordinal = 0): string {
  return `${district}${ordinalPrefix(ordinal)}${facilityKindName(kind)}`;
}

/** 施設のまとまり。座標は左上のマス。 */
export type FacilityGroup = { x: number; y: number; kind: number };

/**
 * 施設のまとまりの名前を、入力と同じ順で返す。
 * 地区は、まとまりの左上のマスのチャンクで決める。同じ地区の同じ種類が 2 つ以上あるときは、
 * 左上の座標の順（x、同じなら y）に「第二」「第三」を付ける。
 */
export function facilityGroupNames(
  seed: Seed,
  groups: readonly FacilityGroup[],
  isWaterside: (cx: number, cy: number) => boolean = () => false,
): string[] {
  const buckets = new Map<string, number[]>();
  groups.forEach((g, i) => {
    const key = `${chunkOf(g.x)},${chunkOf(g.y)}/${g.kind}`;
    const list = buckets.get(key);
    if (list) list.push(i);
    else buckets.set(key, [i]);
  });
  const names = new Array<string>(groups.length);
  for (const list of buckets.values()) {
    list.sort((i, j) => {
      const a = groups[i] as FacilityGroup;
      const b = groups[j] as FacilityGroup;
      return a.x - b.x || a.y - b.y;
    });
    list.forEach((index, ordinal) => {
      const g = groups[index] as FacilityGroup;
      const cx = chunkOf(g.x);
      const cy = chunkOf(g.y);
      names[index] = facilityName(districtName(seed, cx, cy, isWaterside(cx, cy)), g.kind, ordinal);
    });
  }
  return names;
}
