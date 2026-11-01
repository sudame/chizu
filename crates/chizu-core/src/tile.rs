//! マスとチャンクの座標。
//!
//! 1 マスは 8m 四方、1 チャンクは 64×64 マスである。

/// マスの座標。チャンク座標（`i32`）に 64 を掛けた範囲を表せるよう `i64` で持つ。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct TilePos {
    pub x: i64,
    pub y: i64,
}

impl TilePos {
    pub fn new(x: i64, y: i64) -> TilePos {
        TilePos { x, y }
    }
}

/// マスの長方形。`min` と `max` の両端を含む。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TileRect {
    pub min: TilePos,
    pub max: TilePos,
}

impl TileRect {
    /// 2 つの角から作る。角の順番は問わない。
    pub fn from_corners(a: TilePos, b: TilePos) -> TileRect {
        TileRect {
            min: TilePos::new(a.x.min(b.x), a.y.min(b.y)),
            max: TilePos::new(a.x.max(b.x), a.y.max(b.y)),
        }
    }

    /// `min` が `max` を x、y のどちらでも超えていないか。
    pub fn is_ordered(self) -> bool {
        self.min.x <= self.max.x && self.min.y <= self.max.y
    }
}

/// チャンクの座標。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct ChunkCoord {
    pub cx: i32,
    pub cy: i32,
}

impl ChunkCoord {
    pub fn new(cx: i32, cy: i32) -> ChunkCoord {
        ChunkCoord { cx, cy }
    }
}
