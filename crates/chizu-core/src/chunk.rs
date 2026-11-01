//! チャンクと層。

use crate::command::ZoneKind;
use crate::state_hash::{StateHash, StateHasher};
use crate::terrain::{TerrainTiles, terrain};
use crate::tile::{CHUNK_AREA, ChunkCoord};

/// 高さごとの層。値は [`Chunk`] の層の配列の添え字である。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Layer {
    Ground = 0,
    Elevated = 1,
    Underground = 2,
}

impl Layer {
    pub const ALL: [Layer; 3] = [Layer::Ground, Layer::Elevated, Layer::Underground];

    pub fn index(self) -> usize {
        self as usize
    }
}

/// マスを何が使っているか。値は状態ハッシュに書く番号である。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum TileUse {
    #[default]
    Empty = 0,
    /// 道路か歩道が通っている。
    Road = 1,
}

/// 1 つの層のマスの表。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayerTiles {
    pub occupancy: [TileUse; CHUNK_AREA],
    /// 地上の層だけが意味を持つ。
    pub zone: [Option<ZoneKind>; CHUNK_AREA],
}

impl LayerTiles {
    pub fn empty() -> Box<LayerTiles> {
        Box::new(LayerTiles {
            occupancy: [TileUse::Empty; CHUNK_AREA],
            zone: [None; CHUNK_AREA],
        })
    }
}

impl StateHash for LayerTiles {
    fn write_state(&self, h: &mut StateHasher) {
        // マスごとに書くと遅いので、1 バイトずつの列にまとめて書く。区画のないマスは 0 である。
        let mut bytes = [0_u8; CHUNK_AREA];
        for (b, u) in bytes.iter_mut().zip(&self.occupancy) {
            *b = *u as u8;
        }
        h.write_bytes(&bytes);
        for (b, z) in bytes.iter_mut().zip(&self.zone) {
            *b = z.map_or(0, |z| z as u8);
        }
        h.write_bytes(&bytes);
    }
}

/// 64×64 マスのチャンク。
///
/// 表を書き換えられるのは [`Chunk::edit_layer`] だけで、そこで版を上げる。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    coord: ChunkCoord,
    owned: bool,
    version: u32,
    terrain: TerrainTiles,
    layers: [Option<Box<LayerTiles>>; 3],
}

impl Chunk {
    /// 種から地形を作り、地上の層だけを空で用意したチャンク。買っていない状態で作る。
    pub fn generate(seed: u64, coord: ChunkCoord) -> Chunk {
        Chunk {
            coord,
            owned: false,
            version: 0,
            terrain: terrain(seed, coord),
            layers: [Some(LayerTiles::empty()), None, None],
        }
    }

    pub fn coord(&self) -> ChunkCoord {
        self.coord
    }

    pub fn owned(&self) -> bool {
        self.owned
    }

    /// 中身が変わるたびに増える。描画側のキャッシュの鍵に使う。
    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn terrain(&self) -> &TerrainTiles {
        &self.terrain
    }

    /// 作っていない層は `None` を返す。
    pub fn layer(&self, layer: Layer) -> Option<&LayerTiles> {
        self.layers[layer.index()].as_deref()
    }

    /// 買った土地にする。
    pub fn set_owned(&mut self) {
        if !self.owned {
            self.owned = true;
            self.bump_version();
        }
    }

    /// 層の表を書き換える。作っていない層は空で作ってから渡す。
    pub fn edit_layer<R>(&mut self, layer: Layer, f: impl FnOnce(&mut LayerTiles) -> R) -> R {
        let tiles = self.layers[layer.index()].get_or_insert_with(LayerTiles::empty);
        let r = f(tiles);
        self.bump_version();
        r
    }

    fn bump_version(&mut self) {
        self.version = self.version.wrapping_add(1);
    }
}

impl StateHash for Chunk {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.coord);
        h.write(&self.owned);
        h.write(&self.version);
        h.write(&self.terrain);
        for layer in &self.layers {
            h.write(&layer.as_deref());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state_hash::state_hash_of;

    #[test]
    fn generate_makes_only_the_ground_layer() {
        let c = Chunk::generate(1, ChunkCoord::new(2, 3));
        assert!(!c.owned());
        assert_eq!(c.version(), 0);
        assert!(c.layer(Layer::Ground).is_some());
        assert!(c.layer(Layer::Elevated).is_none());
        assert!(c.layer(Layer::Underground).is_none());
    }

    #[test]
    fn hash_covers_ownership_and_tiles() {
        let base = Chunk::generate(1, ChunkCoord::new(0, 0));
        let mut owned = base.clone();
        owned.set_owned();
        let mut zoned = base.clone();
        zoned.edit_layer(Layer::Ground, |t| t.zone[5] = Some(ZoneKind::Office));
        let mut elevated = base.clone();
        elevated.edit_layer(Layer::Elevated, |_| ());
        let h = state_hash_of(&base);
        for (what, c) in [
            ("買う", &owned),
            ("区画を塗る", &zoned),
            ("高架の層を足す", &elevated),
        ] {
            assert_eq!(c.version(), 1, "{what}");
            assert_ne!(state_hash_of(c), h, "{what}");
        }
        // 版が同じでも、中身が違えばハッシュが違う。
        let mut other_zone = base.clone();
        other_zone.edit_layer(Layer::Ground, |t| t.zone[5] = Some(ZoneKind::Industrial));
        assert_ne!(state_hash_of(&other_zone), state_hash_of(&zoned));
    }

    #[test]
    fn set_owned_twice_bumps_the_version_once() {
        let mut c = Chunk::generate(1, ChunkCoord::new(0, 0));
        c.set_owned();
        c.set_owned();
        assert_eq!(c.version(), 1);
    }
}
