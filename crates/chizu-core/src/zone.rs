//! 区画を塗れるマスの判定。
//!
//! CS2 にならい、区画は道路に面したマスから奥へ [`ZONE_DEPTH`] マスまで塗れる。
//! ここでは道路の区間ごとの区画のセルは作らず、マスごとに「道路のマスからのチェビシェフ距離が
//! `ZONE_DEPTH` 以下か」で判定する。外部接続の高速道路沿いには塗れない。

use std::collections::BTreeSet;

use crate::network::{Network, SegmentKind};
use crate::tile::{CHUNK_SIDE, ChunkCoord, TilePos, TileRect};

/// 道路のマスから区画を塗れる奥行き（マス）。
pub const ZONE_DEPTH: i64 = 6;

/// 1 回のコマンドで塗れる長方形の 1 辺の上限（マス）。
pub const MAX_ZONE_SIDE: u64 = 256;

/// 長方形の幅と高さ。どちらかが `i64` の差に収まらなければ `None`。
pub fn rect_size(r: TileRect) -> Option<(u64, u64)> {
    let w = r
        .max
        .x
        .checked_sub(r.min.x)?
        .unsigned_abs()
        .checked_add(1)?;
    let h = r
        .max
        .y
        .checked_sub(r.min.y)?
        .unsigned_abs()
        .checked_add(1)?;
    Some((w, h))
}

/// 長方形の各マスが、高速道路を除く道路のマスから `ZONE_DEPTH` 以内か。行ごとに並べて返す。
/// 道路のマスそのものも `true` になる。
pub fn near_road(network: &Network, area: TileRect) -> Vec<bool> {
    let (w, h) = rect_size(area).expect("区画の長方形が大きすぎる");
    let (w, h) = (w as usize, h as usize);
    let mut mask = vec![false; w * h];
    let grow = |v: i64, d: i64| v.checked_add(d).expect("区画の長方形があふれた");
    let (lo, hi) = (
        TilePos::new(grow(area.min.x, -ZONE_DEPTH), grow(area.min.y, -ZONE_DEPTH)),
        TilePos::new(grow(area.max.x, ZONE_DEPTH), grow(area.max.y, ZONE_DEPTH)),
    );
    let (Some(c0), Some(c1)) = (lo.chunk(), hi.chunk()) else {
        return mask;
    };
    let mut segments = BTreeSet::new();
    for cy in c0.cy..=c1.cy {
        for cx in c0.cx..=c1.cx {
            segments.extend(network.segments_in_chunk(ChunkCoord::new(cx, cy)));
        }
    }
    for s in segments {
        if network
            .segment(s)
            .is_some_and(|s| s.kind == SegmentKind::Highway)
        {
            continue;
        }
        let line = network.segment_line(s).expect("区間がない");
        for t in line.tiles() {
            if t.x < lo.x || t.x > hi.x || t.y < lo.y || t.y > hi.y {
                continue;
            }
            let x0 = (t.x - ZONE_DEPTH).max(area.min.x);
            let x1 = (t.x + ZONE_DEPTH).min(area.max.x);
            let y0 = (t.y - ZONE_DEPTH).max(area.min.y);
            let y1 = (t.y + ZONE_DEPTH).min(area.max.y);
            for y in y0..=y1 {
                let row = (y - area.min.y) as usize * w;
                for x in x0..=x1 {
                    mask[row + (x - area.min.x) as usize] = true;
                }
            }
        }
    }
    mask
}

/// 直線のまわり `ZONE_DEPTH` マスを含む長方形。道路を消したあとに区画を見直す範囲に使う。
pub fn around(a: TilePos, b: TilePos) -> TileRect {
    let r = TileRect::from_corners(a, b);
    let d = ZONE_DEPTH;
    TileRect {
        min: TilePos::new(r.min.x - d, r.min.y - d),
        max: TilePos::new(r.max.x + d, r.max.y + d),
    }
}

/// 長方形のマスを、行ごとに返す。
pub fn tiles_of(area: TileRect) -> impl Iterator<Item = TilePos> {
    (area.min.y..=area.max.y)
        .flat_map(move |y| (area.min.x..=area.max.x).map(move |x| TilePos::new(x, y)))
}

const _: () = assert!(ZONE_DEPTH < CHUNK_SIDE as i64);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::RoadKind;
    use crate::network::Line;

    #[test]
    fn near_road_reaches_six_tiles_and_skips_highways() {
        let mut n = Network::new();
        let road = Line::between(TilePos::new(0, 0), TilePos::new(10, 0)).unwrap();
        n.build_road(&road, RoadKind::Small).unwrap();
        n.add_highway(TilePos::new(0, 40), TilePos::new(20, 40));
        let area = TileRect::from_corners(TilePos::new(-8, -8), TilePos::new(12, 50));
        let mask = near_road(&n, area);
        let (w, _) = rect_size(area).unwrap();
        let at = |x: i64, y: i64| mask[((y + 8) as u64 * w + (x + 8) as u64) as usize];
        assert!(at(0, 0) && at(5, 6) && at(-6, -6) && at(10, -6));
        assert!(at(12, 0) && !at(5, 7) && !at(-7, 0));
        assert!(!at(5, 40) && !at(5, 37), "高速道路沿いには塗れない");
    }
}
