//! マスとチャンクの座標。
//!
//! 1 マスは 8m 四方、1 チャンクは 64×64 マスである。

use serde::{Deserialize, Serialize};

/// チャンクの 1 辺のマス数。
pub const CHUNK_SIDE: usize = 64;

/// チャンクのマス数。チャンクの中のマスの表は、この長さの配列を行ごとに並べて持つ。
pub const CHUNK_AREA: usize = CHUNK_SIDE * CHUNK_SIDE;

/// チャンクの中のマス `(lx, ly)` の、マスの表での添え字。
#[track_caller]
pub fn local_index(lx: usize, ly: usize) -> usize {
    assert!(
        lx < CHUNK_SIDE && ly < CHUNK_SIDE,
        "チャンクの外のマス ({lx}, {ly})"
    );
    ly * CHUNK_SIDE + lx
}

/// コマンドで指せるマスの座標の絶対値の上限。x、y ともに `-WORLD_LIMIT..=WORLD_LIMIT` に収める。
/// 範囲を決めておくと、座標の足し引きがコマンドの処理の中であふれない。
pub const WORLD_LIMIT: i64 = 1 << 31;

/// マスの座標。チャンク座標（`i32`）に 64 を掛けた範囲を表せるよう `i64` で持つ。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TilePos {
    pub x: i64,
    pub y: i64,
}

impl crate::state_hash::StateHash for TilePos {
    fn write_state(&self, h: &mut crate::state_hash::StateHasher) {
        h.write(&self.x);
        h.write(&self.y);
    }
}

impl TilePos {
    pub fn new(x: i64, y: i64) -> TilePos {
        TilePos { x, y }
    }

    /// コマンドで指せる範囲（[`WORLD_LIMIT`]）の中か。
    pub fn in_world(self) -> bool {
        (-WORLD_LIMIT..=WORLD_LIMIT).contains(&self.x)
            && (-WORLD_LIMIT..=WORLD_LIMIT).contains(&self.y)
    }

    /// このマスを含むチャンク。チャンクの座標が `i32` に収まらないときは `None`。
    pub fn chunk(self) -> Option<ChunkCoord> {
        let side = CHUNK_SIDE as i64;
        let c = |v: i64| i32::try_from(crate::round::div_floor(v, side)).ok();
        Some(ChunkCoord::new(c(self.x)?, c(self.y)?))
    }

    /// チャンクの中での位置 `(lx, ly)`。
    pub fn local(self) -> (usize, usize) {
        let side = CHUNK_SIDE as u64;
        // 2 の補数の下位ビットは、負の座標でも床の剰余と一致する。
        (
            (self.x as u64 % side) as usize,
            (self.y as u64 % side) as usize,
        )
    }
}

/// マスの長方形。`min` と `max` の両端を含む。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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

    /// 両方の角がコマンドで指せる範囲の中か。
    pub fn in_world(self) -> bool {
        self.min.in_world() && self.max.in_world()
    }

    /// `min` が `max` を x、y のどちらでも超えていないか。
    pub fn is_ordered(self) -> bool {
        self.min.x <= self.max.x && self.min.y <= self.max.y
    }
}

/// チャンクの座標。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ChunkCoord {
    pub cx: i32,
    pub cy: i32,
}

impl ChunkCoord {
    pub fn new(cx: i32, cy: i32) -> ChunkCoord {
        ChunkCoord { cx, cy }
    }

    /// チャンクのマスがすべてコマンドで指せる範囲の中か。
    pub fn in_world(self) -> bool {
        let last = CHUNK_SIDE as i64 - 1;
        let o = self.origin();
        o.in_world() && TilePos::new(o.x + last, o.y + last).in_world()
    }

    /// 左上（x、y が最小）のマス。
    pub fn origin(self) -> TilePos {
        let side = CHUNK_SIDE as i64;
        TilePos::new(i64::from(self.cx) * side, i64::from(self.cy) * side)
    }

    /// 辺を接する 4 つのチャンク。座標が `i32` に収まらない方向は含めない。
    pub fn edge_neighbors(self) -> impl Iterator<Item = ChunkCoord> {
        let ChunkCoord { cx, cy } = self;
        [
            cx.checked_sub(1).map(|x| ChunkCoord::new(x, cy)),
            cx.checked_add(1).map(|x| ChunkCoord::new(x, cy)),
            cy.checked_sub(1).map(|y| ChunkCoord::new(cx, y)),
            cy.checked_add(1).map(|y| ChunkCoord::new(cx, y)),
        ]
        .into_iter()
        .flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_covers_the_whole_i32_range() {
        assert_eq!(ChunkCoord::new(-1, 2).origin(), TilePos::new(-64, 128));
        let far = ChunkCoord::new(i32::MIN, i32::MAX).origin();
        assert_eq!(far, TilePos::new(-(1_i64 << 37), ((1_i64 << 31) - 1) * 64));
    }

    #[test]
    fn chunk_and_local_floor_negative_coordinates() {
        let t = TilePos::new(-1, 64);
        assert_eq!(t.chunk(), Some(ChunkCoord::new(-1, 1)));
        assert_eq!(t.local(), (63, 0));
        assert_eq!(TilePos::new(i64::MIN, 0).chunk(), None);
    }

    #[test]
    fn edge_neighbors_skip_overflow() {
        let n: Vec<_> = ChunkCoord::new(0, 0).edge_neighbors().collect();
        assert_eq!(
            n,
            [
                ChunkCoord::new(-1, 0),
                ChunkCoord::new(1, 0),
                ChunkCoord::new(0, -1),
                ChunkCoord::new(0, 1),
            ]
        );
        let corner = ChunkCoord::new(i32::MAX, i32::MIN);
        assert_eq!(corner.edge_neighbors().count(), 2);
    }
}
