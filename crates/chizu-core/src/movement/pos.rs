//! 歩道の上の位置。

use crate::network::{DIAGONAL_TILE_MM, Dir, LaneMode, LinkId, Network, TILE_MM};
use crate::state_hash::{StateHash, StateHasher};
use crate::tile::TilePos;
use crate::units::{LaneOffset, Mm, WorldPos};

/// 歩道の上の位置。網の区間を分けたり消したりしても意味が変わらないよう、リンクではなく
/// マスと歩く向きで持つ。
///
/// 区間の片側の歩道は、その区間を一方の向きに歩く。マス `tile` から `step` の向きに 1 歩進む
/// あいだの、`along` だけ進んだ所を指す。歩く向きが区間の `a` から `b` なら `Forward` の歩道、
/// 逆なら `Backward` の歩道の上である。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SidewalkPos {
    pub tile: TilePos,
    pub step: (i8, i8),
    /// 1 歩の中で進んだ距離。0 以上、1 歩の長さ以下である。
    pub along: LaneOffset,
}

pub(super) fn step_mm(step: (i8, i8)) -> u32 {
    let mm = if step.0 != 0 && step.1 != 0 {
        DIAGONAL_TILE_MM
    } else {
        TILE_MM
    };
    u32::try_from(mm).expect("1 歩の長さが u32 に収まらない")
}

fn shift(t: TilePos, step: (i8, i8), k: u32) -> TilePos {
    let at = |v: i64, d: i8| {
        i64::from(d)
            .checked_mul(i64::from(k))
            .and_then(|d| v.checked_add(d))
            .expect("マスの座標があふれた")
    };
    TilePos::new(at(t.x, step.0), at(t.y, step.1))
}

/// リンクの始まりのマス、1 歩の向き、長さ。リンクがなければ `None`。
pub(super) fn link_geometry(
    net: &Network,
    link: LinkId,
) -> Option<(TilePos, (i8, i8), LaneOffset)> {
    let line = net.segment_line(link.segment)?;
    let (sx, sy) = (line.step.0 as i8, line.step.1 as i8);
    let (start, step) = match link.dir {
        Dir::Forward => (line.start, (sx, sy)),
        Dir::Backward => (line.end(), (-sx, -sy)),
    };
    let len = u32::try_from(line.length().0).expect("リンクの長さが u32 に収まらない");
    Some((start, step, LaneOffset(len)))
}

impl SidewalkPos {
    /// 始まりのマス `start` から `step` の向きに `offset` だけ進んだ位置。
    pub fn along(start: TilePos, step: (i8, i8), offset: LaneOffset) -> SidewalkPos {
        let per = step_mm(step);
        // 1 歩の終わりちょうどは、次の 1 歩の始まりではなくその 1 歩の終わりとして表す。
        let k = offset.0.saturating_sub(1) / per;
        SidewalkPos {
            tile: shift(start, step, k),
            step,
            along: LaneOffset(offset.0 - k * per),
        }
    }

    /// 歩道のリンク `link` の、始まりから `offset` の位置。リンクがなければ `None`。
    pub fn on_link(net: &Network, link: LinkId, offset: LaneOffset) -> Option<SidewalkPos> {
        let sidewalk = net.segment(link.segment)?.kind.has_sidewalks();
        (link.mode == LaneMode::Sidewalk && sidewalk)
            .then(|| Self::on_any_link(net, link, offset))
            .flatten()
    }

    /// 歩道か車のリンク `link` の、始まりから `offset` の位置。車のリンクの位置は、同じ区間の
    /// 同じ向きの歩道の位置と同じ表し方になる。
    pub fn on_any_link(net: &Network, link: LinkId, offset: LaneOffset) -> Option<SidewalkPos> {
        let (start, step, len) = link_geometry(net, link)?;
        (offset <= len).then(|| Self::along(start, step, offset))
    }

    /// いまの網の上で、この位置を通る歩道のリンクと、その始まりからの距離。歩道がなければ `None`。
    pub fn resolve(&self, net: &Network) -> Option<(LinkId, LaneOffset)> {
        self.resolve_on(net, LaneMode::Sidewalk)
    }

    /// いまの網の上で、この位置を通る `mode` のリンクと、その始まりからの距離。
    pub fn resolve_on(&self, net: &Network, mode: LaneMode) -> Option<(LinkId, LaneOffset)> {
        let next = shift(self.tile, self.step, 1);
        let per = step_mm(self.step);
        if self.along.0 > per {
            return None;
        }
        net.segments_in_chunk(self.tile.chunk()?)
            .iter()
            .find_map(|s| {
                let seg = net.segment(*s)?;
                let line = net.segment_line(*s)?;
                let (k, n) = (line.position_of(self.tile)?, line.position_of(next)?);
                let dir = if n == k + 1 {
                    Dir::Forward
                } else if k == n + 1 {
                    Dir::Backward
                } else {
                    return None;
                };
                let link = LinkId {
                    segment: *s,
                    dir,
                    mode,
                };
                (mode == LaneMode::Car || seg.kind.has_sidewalks()).then_some(())?;
                let steps = match dir {
                    Dir::Forward => k,
                    Dir::Backward => line.len - k,
                };
                let steps = u32::try_from(steps).ok()?;
                let offset = steps.checked_mul(per)?.checked_add(self.along.0)?;
                Some((link, LaneOffset(offset)))
            })
    }

    /// ワールド座標。斜めの 1 歩の中の距離は、縦横の成分に直して切り捨てる。
    pub fn world(&self) -> WorldPos {
        let per = u64::from(step_mm(self.step));
        let d = u64::from(self.along.0) * TILE_MM as u64 / per;
        let d = i64::try_from(d).expect("距離が i64 に収まらない");
        let c = |t: i64, s: i8| Mm(t * TILE_MM + TILE_MM / 2 + i64::from(s) * d);
        WorldPos::new(c(self.tile.x, self.step.0), c(self.tile.y, self.step.1))
    }
}

impl StateHash for SidewalkPos {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&(self.tile.x, self.tile.y));
        h.write(&self.step);
        h.write(&self.along);
    }
}
