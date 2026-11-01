//! 種からの地形の生成。
//!
//! 地形は種とマスのワールド座標だけで決まる。チャンクの境目のマスも隣のチャンクを作らずに
//! 決まるので、水辺は境目で途切れない。
//!
//! 水辺は整数だけの値ノイズで作る。格子点に種から引いた値を置き、そのあいだを
//! smoothstep（3t² − 2t³）で補間する。格子の間隔は 2 の累乗のマス数にして、
//! 格子点の添え字を算術右シフト（負の座標でも切り下げになる）で求める。
//!
//! - 海：間隔の広いノイズが低いところ。
//! - 川：中くらいの間隔のノイズが真ん中の値をとる等高線のまわりの細い帯。滑らかなノイズの
//!   等高線は途切れずにつながるので、蛇行する川になる。
//!
//! ここを変えると、同じ種から別の地形が生まれ、ゴールデンのハッシュがすべて変わる。

use crate::rng::{RngPurpose, mix_words};
use crate::state_hash::{StateHash, StateHasher};
use crate::tile::{CHUNK_AREA, CHUNK_SIDE, ChunkCoord, TilePos, local_index};

/// ノイズの値の上限。ノイズは `0..=NOISE_MAX` の値をとる。
const NOISE_MAX: u64 = 0xFFFF;

/// 1 つのノイズの層。`id` は層ごとに別の値を引くための番号で、変えると地形が変わる。
struct Octave {
    id: u64,
    /// 格子の間隔は `1 << shift` マス。
    shift: u32,
    weight: u64,
}

/// 海を決めるノイズ。間隔 1,024 マス（16 チャンク）に、海岸線を崩す 128 マスの層を重ねる。
const SEA_OCTAVES: [Octave; 2] = [
    Octave {
        id: 1,
        shift: 10,
        weight: 3,
    },
    Octave {
        id: 2,
        shift: 7,
        weight: 1,
    },
];

/// 川を決めるノイズ。間隔 256 マス（4 チャンク）に、曲がりを足す 64 マスの層を重ねる。
const RIVER_OCTAVES: [Octave; 2] = [
    Octave {
        id: 3,
        shift: 8,
        weight: 3,
    },
    Octave {
        id: 4,
        shift: 6,
        weight: 1,
    },
];

/// 海のノイズがこれより低いマスは海になる。
const SEA_LEVEL: u64 = 18_000;

/// 川のノイズと真ん中の値との差がこれより小さいマスは川になる。帯の幅は 2〜4 マスほどになる。
const RIVER_HALF_WIDTH: u64 = 600;

/// 格子点 `(ix, iy)` に置く値。
fn lattice(seed: u64, id: u64, ix: i64, iy: i64) -> u64 {
    mix_words(seed, RngPurpose::Terrain, &[id, ix as u64, iy as u64]) >> 48
}

/// `a` から `b` へ、`s / d` の割合だけ進んだ値。
fn lerp(a: u64, b: u64, s: u64, d: u64) -> u64 {
    (a * (d - s) + b * s) / d
}

/// 1 つの層の値ノイズ。`0..=NOISE_MAX` の値を返す。
fn value_noise(seed: u64, octave: &Octave, x: i64, y: i64) -> u64 {
    let size = 1_u64 << octave.shift;
    let mask = (1_i64 << octave.shift) - 1;
    let (ix, iy) = (x >> octave.shift, y >> octave.shift);
    // smoothstep を size³ を分母とする分数で表す。
    let d = size * size * size;
    let smooth = |t: u64| t * t * (3 * size - 2 * t);
    let sx = smooth((x & mask) as u64);
    let sy = smooth((y & mask) as u64);
    let v = |dx: i64, dy: i64| lattice(seed, octave.id, ix.wrapping_add(dx), iy.wrapping_add(dy));
    let top = lerp(v(0, 0), v(1, 0), sx, d);
    let bottom = lerp(v(0, 1), v(1, 1), sx, d);
    lerp(top, bottom, sy, d)
}

/// 層を重みで混ぜたノイズ。`0..=NOISE_MAX` の値を返す。
fn layered_noise(seed: u64, octaves: &[Octave], x: i64, y: i64) -> u64 {
    let total: u64 = octaves.iter().map(|o| o.weight).sum();
    let sum: u64 = octaves
        .iter()
        .map(|o| o.weight * value_noise(seed, o, x, y))
        .sum();
    sum / total
}

/// マスが水辺（海か川）か。種とワールド座標だけで決まる。
pub fn is_water(seed: u64, tile: TilePos) -> bool {
    let (x, y) = (tile.x, tile.y);
    if layered_noise(seed, &SEA_OCTAVES, x, y) < SEA_LEVEL {
        return true;
    }
    let mid = NOISE_MAX.div_ceil(2);
    layered_noise(seed, &RIVER_OCTAVES, x, y).abs_diff(mid) < RIVER_HALF_WIDTH
}

/// 1 つのチャンクの地形。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerrainTiles {
    /// マスごとの標高。いまは平らな地図なので、すべて 0 である。
    elevation: Box<[u16; CHUNK_AREA]>,
    /// 行 `ly` のビット `lx` が、マス `(lx, ly)` が水辺かどうかを表す。
    water: [u64; CHUNK_SIDE],
}

impl TerrainTiles {
    pub fn elevation(&self, lx: usize, ly: usize) -> u16 {
        self.elevation[local_index(lx, ly)]
    }

    pub fn is_water(&self, lx: usize, ly: usize) -> bool {
        local_index(lx, ly);
        self.water[ly] >> lx & 1 == 1
    }

    /// 水辺のマスの数。
    pub fn water_count(&self) -> u32 {
        self.water.iter().map(|row| row.count_ones()).sum()
    }
}

/// チャンクの地形を種から作る。同じ種と座標からは必ず同じ地形になる。
pub fn terrain(seed: u64, coord: ChunkCoord) -> TerrainTiles {
    let origin = coord.origin();
    let mut water = [0_u64; CHUNK_SIDE];
    for (ly, row) in water.iter_mut().enumerate() {
        for lx in 0..CHUNK_SIDE {
            let tile = TilePos::new(origin.x + lx as i64, origin.y + ly as i64);
            if is_water(seed, tile) {
                *row |= 1 << lx;
            }
        }
    }
    TerrainTiles {
        elevation: Box::new([0; CHUNK_AREA]),
        water,
    }
}

impl StateHash for TerrainTiles {
    fn write_state(&self, h: &mut StateHasher) {
        let mut bytes = [0_u8; CHUNK_AREA * 2];
        for (out, e) in bytes
            .as_chunks_mut::<2>()
            .0
            .iter_mut()
            .zip(self.elevation.iter())
        {
            *out = e.to_le_bytes();
        }
        h.write_bytes(&bytes);
        h.write(&self.water);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn same_seed_and_coord_give_the_same_terrain() {
        for coord in [
            ChunkCoord::new(0, 0),
            ChunkCoord::new(-3, 7),
            ChunkCoord::new(i32::MIN, i32::MAX),
        ] {
            assert_eq!(terrain(42, coord), terrain(42, coord));
        }
    }

    #[test]
    fn seed_changes_the_terrain() {
        let coords = (-4..4).flat_map(|cx| (-4..4).map(move |cy| ChunkCoord::new(cx, cy)));
        let differs = coords
            .into_iter()
            .any(|c| terrain(1, c).water != terrain(2, c).water);
        assert!(differs);
    }

    #[test]
    fn chunk_tiles_match_the_world_function() {
        let seed = 7;
        let coord = ChunkCoord::new(-1, 2);
        let t = terrain(seed, coord);
        let origin = coord.origin();
        for ly in 0..CHUNK_SIDE {
            for lx in 0..CHUNK_SIDE {
                let tile = TilePos::new(origin.x + lx as i64, origin.y + ly as i64);
                assert_eq!(t.is_water(lx, ly), is_water(seed, tile), "({lx}, {ly})");
                assert_eq!(t.elevation(lx, ly), 0);
            }
        }
    }

    /// 隣り合うマスのノイズの差の最大を、チャンクの中と境目をまたぐ所とで分けて求める。
    fn max_steps(seed: u64, octaves: &[Octave], chunks: i64) -> (u64, u64) {
        let side = CHUNK_SIDE as i64;
        let (mut inside, mut across) = (0, 0);
        for y in -chunks * side..chunks * side {
            for x in -chunks * side..chunks * side {
                let n = layered_noise(seed, octaves, x, y);
                for (nx, ny) in [(x + 1, y), (x, y + 1)] {
                    let step = n.abs_diff(layered_noise(seed, octaves, nx, ny));
                    let crosses = (nx != x && nx.rem_euclid(side) == 0)
                        || (ny != y && ny.rem_euclid(side) == 0);
                    let max = if crosses { &mut across } else { &mut inside };
                    *max = (*max).max(step);
                }
            }
        }
        (inside, across)
    }

    #[test]
    fn noise_has_no_seam_at_chunk_edges() {
        for octaves in [&SEA_OCTAVES[..], &RIVER_OCTAVES[..]] {
            let (inside, across) = max_steps(3, octaves, 2);
            assert!(inside > 0);
            assert!(across <= inside, "境目で {across}、チャンクの中で {inside}");
        }
    }

    #[test]
    fn water_crosses_chunk_edges() {
        let seed = 42;
        let last = CHUNK_SIDE - 1;
        let mut crossings = 0;
        for cx in -4..4 {
            for cy in -4..4 {
                let a = terrain(seed, ChunkCoord::new(cx, cy));
                let right = terrain(seed, ChunkCoord::new(cx + 1, cy));
                let below = terrain(seed, ChunkCoord::new(cx, cy + 1));
                for i in 0..CHUNK_SIDE {
                    crossings += u32::from(a.is_water(last, i) && right.is_water(0, i));
                    crossings += u32::from(a.is_water(i, last) && below.is_water(i, 0));
                }
            }
        }
        assert!(crossings > 0, "境目を越える水辺が 1 つもない");
    }

    #[test]
    fn some_tiles_are_water_and_most_are_land() {
        let seed = 42;
        let mut water = 0_u64;
        let mut chunks = 0_u64;
        for cx in -8..8 {
            for cy in -8..8 {
                water += u64::from(terrain(seed, ChunkCoord::new(cx, cy)).water_count());
                chunks += 1;
            }
        }
        let total = chunks * CHUNK_AREA as u64;
        assert!(water > 0);
        assert!(water * 2 < total, "水辺が多すぎる：{water} / {total}");
    }

    proptest! {
        #[test]
        fn is_water_is_pure(seed in any::<u64>(), x in any::<i64>(), y in any::<i64>()) {
            let tile = TilePos::new(x, y);
            prop_assert_eq!(is_water(seed, tile), is_water(seed, tile));
        }
    }
}
