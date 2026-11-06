//! 道路の側から見た踏切（設計 3.10 の 3 の「踏切」）。
//!
//! 踏切は道路の区間の途中のマスにある。車と歩く人は、閉じた踏切を区間の途中の衝突区間（`crossing` の横切りと
//! 同じ仕組み）として扱い、止まりきれるなら手前で止まる。閉じたあとに入る車と人はいない。
//!
//! 開いていても、踏切の先に車 1 台が抜けきる空きがなければ、車は踏切に入らない。空きは、踏切の向こうの端から、
//! 同じ車線の前の車が減速の上限で止まった所の尾までで測る。前の車がいなければ、リンクの終わりの停止線までで測る。
//! 踏切の中で止まる車を出さないためである。

use std::collections::{BTreeMap, BTreeSet};

use smallvec::SmallVec;

use super::Trips;
use super::car::{CAR_LENGTH, SPACING, braking_distance};
use super::crossing::{JAY_HALF_MM, Occupied};
use crate::network::{DIAGONAL_TILE_MM, Dir, LaneMode, LinkId, Network, SegmentId, TILE_MM};
use crate::tile::TilePos;
use crate::units::{LaneOffset, MmPerTick};

/// 閉じた踏切の手前で待つ歩く人が、踏切の手前の端とのあいだに空ける距離（mm）。
const WALK_GAP_MM: u32 = 500;

/// 区間の上の踏切。位置は区間の `a` のノードの中心から測る。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub tile: TilePos,
    /// 踏切のマスの、`a` の側の端と `b` の側の端。
    pub from: u32,
    pub to: u32,
    pub closed: bool,
}

/// リンクから見た踏切。位置はリンクの始まりから測った手前の端と向こうの端である。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct OnLink {
    pub tile: TilePos,
    pub near: u32,
    pub far: u32,
    pub closed: bool,
}

/// 道路の区間ごとの踏切。tick ごとに、線路の網の踏切の一覧と、閉じた踏切から作る。
#[derive(Clone, Debug, Default)]
pub struct RailCrossings {
    spans: BTreeMap<SegmentId, SmallVec<[Span; 1]>>,
}

/// 真ん中が `z` の衝突区間の手前で止まる車が、手前の端 `near` の踏切の手前で、前の端を止める位置になる値。
/// 衝突区間を抜けたとみなすのは、車の前の端が手前の端に届いたときである（`car::drive`）。
pub(super) fn zone_of(near: u32) -> u32 {
    near.saturating_sub(CAR_LENGTH / 2 + JAY_HALF_MM)
}

impl RailCrossings {
    /// 踏切のマス `tiles` と、そのうち閉じたマス `closed` から作る。道路の区間の途中にないマスは飛ばす。
    pub fn new(
        net: &Network,
        tiles: impl IntoIterator<Item = TilePos>,
        closed: &BTreeSet<TilePos>,
    ) -> RailCrossings {
        let mut spans = BTreeMap::<SegmentId, SmallVec<[Span; 1]>>::new();
        for t in tiles {
            let Some(seg) = net.segments_through(t).first().copied() else {
                continue;
            };
            let Some(line) = net.segment_line(seg) else {
                continue;
            };
            let Some(k) = line.position_of(t) else {
                continue;
            };
            let per = if line.is_diagonal() {
                DIAGONAL_TILE_MM
            } else {
                TILE_MM
            };
            let k = i64::try_from(k).expect("歩数があふれた");
            let center = k * per;
            let mm = |v: i64| u32::try_from(v.max(0)).expect("区間の上の位置が u32 に収まらない");
            spans.entry(seg).or_default().push(Span {
                tile: t,
                from: mm(center - per / 2),
                to: mm(center + per / 2),
                closed: closed.contains(&t),
            });
        }
        for v in spans.values_mut() {
            v.sort_unstable_by_key(|s| s.from);
        }
        RailCrossings { spans }
    }

    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    /// リンク `link`（長さ `len`）の上の踏切。リンクの始まりに近い順に並ぶ。
    pub(super) fn on_link(&self, link: LinkId, len: u32) -> SmallVec<[OnLink; 2]> {
        let Some(list) = self.spans.get(&link.segment) else {
            return SmallVec::new();
        };
        let mut out: SmallVec<[OnLink; 2]> = list
            .iter()
            .map(|s| match link.dir {
                Dir::Forward => OnLink {
                    tile: s.tile,
                    near: s.from,
                    far: s.to,
                    closed: s.closed,
                },
                Dir::Backward => OnLink {
                    tile: s.tile,
                    near: len.saturating_sub(s.to),
                    far: len.saturating_sub(s.from),
                    closed: s.closed,
                },
            })
            .collect();
        out.sort_unstable_by_key(|c| c.near);
        out
    }

    /// 閉じた踏切を、両方の向きの車のリンクの衝突区間として `occupied` に足す。区間の途中を横切る人と同じく、
    /// 止まりきれる車は手前で止まり、ノードを越えてすぐの踏切が閉じていれば、ノードを越えずに止まる。
    pub(super) fn add_closed_zones(&self, net: &Network, occupied: &mut Occupied) {
        for (seg, list) in &self.spans {
            if !list.iter().any(|s| s.closed) {
                continue;
            }
            let Some(len) = net
                .segment_line(*seg)
                .and_then(|l| u32::try_from(l.length().0).ok())
            else {
                continue;
            };
            for dir in [Dir::Forward, Dir::Backward] {
                let link = LinkId {
                    segment: *seg,
                    dir,
                    mode: LaneMode::Car,
                };
                for c in self.on_link(link, len).iter().filter(|c| c.closed) {
                    let z = zone_of(c.near);
                    occupied.zones.entry(link).or_default().push(z);
                    occupied.jay.entry(link).or_default().push(z);
                }
            }
        }
        for v in occupied.jay.values_mut().chain(occupied.zones.values_mut()) {
            v.sort_unstable();
        }
    }

    /// 歩く人がリンク `link`（長さ `len`）の上を、位置 `from` から進んでよい所。閉じた踏切の手前の端より
    /// 手前にいる人は、その手前で止まる。中にいる人と、もう抜けた人は止めない。
    pub(super) fn walk_limit(&self, link: LinkId, len: u32, from: u32) -> Option<u32> {
        self.on_link(link, len)
            .into_iter()
            .filter(|c| c.closed && from < c.near)
            .map(|c| c.near.saturating_sub(WALK_GAP_MM).max(from))
            .next()
    }

    /// 開いた踏切の先に、車 1 台が抜けきる空きがないときに、手前で止まる衝突区間の真ん中の位置（いまのリンクの
    /// 始まりから測る）。`bound` は、前の車が減速の上限で止まった所から車間を空けた所か、前の車がなければ
    /// リンクの終わりの停止線で、その位置まで車の真ん中が進める。`next` は、ノードの先の次のリンクと長さと、
    /// そのリンクの踏切に使う `bound`（いまのリンクの始まりから測る）で、次のリンクの始まりの近くの踏切も見るときに渡す。
    pub(super) fn no_room_zones(
        &self,
        here: (LinkId, u32, i64),
        next: Option<(LinkId, u32, i64)>,
        front: i64,
    ) -> SmallVec<[u32; 2]> {
        let mut out = SmallVec::new();
        let (link, len, bound) = here;
        let mut look = |link: LinkId, len: u32, shift: i64, bound: i64| {
            for c in self.on_link(link, len) {
                let near = shift + i64::from(c.near);
                let far = shift + i64::from(c.far);
                if c.closed || front > near {
                    continue;
                }
                if bound < far + i64::from(CAR_LENGTH / 2) {
                    let z = zone_of(u32::try_from(near.max(0)).unwrap_or(u32::MAX));
                    out.push(z);
                }
            }
        };
        look(link, len, 0, bound);
        if let Some((n, n_len, n_bound)) = next {
            look(n, n_len, i64::from(len), n_bound);
        }
        out
    }

    /// 進めたあとの車と歩く人から、中に車か人がいる踏切と、閉じた踏切の手前で止まっている車の数を求める。
    pub fn traffic(&self, trips: &Trips) -> (BTreeSet<TilePos>, u64) {
        let mut inside = BTreeSet::new();
        let mut waits = 0_u64;
        if self.is_empty() {
            return (inside, waits);
        }
        let half = i64::from(CAR_LENGTH / 2);
        for (_, t) in trips.iter() {
            let link = t.link();
            if !self.spans.contains_key(&link.segment) {
                continue;
            }
            let len = t.link_len.0;
            let at = i64::from(t.offset.0);
            for c in self.on_link(link, len) {
                let (near, far) = (i64::from(c.near), i64::from(c.far));
                match t.mode {
                    LaneMode::Sidewalk => {
                        if t.walk.jay.is_none() && near < at && at < far {
                            inside.insert(c.tile);
                        }
                    }
                    LaneMode::Car => {
                        if t.waiting {
                            continue;
                        }
                        if at - half < far && near < at + half {
                            inside.insert(c.tile);
                        } else if c.closed && at + half <= near && t.speed == MmPerTick::ZERO {
                            waits += 1;
                        }
                    }
                }
            }
        }
        (inside, waits)
    }

    /// 体が踏切にかかったまま止まっている車のいる踏切と、開いた踏切の手前で車間ぶんの内に止まっている車の数。
    /// 後者は、踏切の先に抜けきる空きがないか、前の車の列が踏切の手前まで延びて止まった車である。
    #[cfg(test)]
    pub(crate) fn stopped_cars(&self, trips: &Trips) -> (BTreeSet<TilePos>, u64) {
        let mut inside = BTreeSet::new();
        let mut held = 0_u64;
        let half = i64::from(CAR_LENGTH / 2);
        for (_, t) in trips.iter() {
            if t.mode != LaneMode::Car || t.waiting || t.speed != MmPerTick::ZERO {
                continue;
            }
            let at = i64::from(t.offset.0);
            for c in self.on_link(t.link(), t.link_len.0) {
                let (near, far) = (i64::from(c.near), i64::from(c.far));
                if at - half < far && near < at + half {
                    inside.insert(c.tile);
                } else if !c.closed && at + half <= near && near - (at + half) < i64::from(SPACING)
                {
                    held += 1;
                }
            }
        }
        (inside, held)
    }
}

/// 前の車（いまのリンクの始まりから数えた位置と速さ）が減速の上限で止まった所から、車間を空けた所。
/// 自分の車の真ん中がそこまで進める。
pub(super) fn bound_behind(leader: (LaneOffset, MmPerTick)) -> i64 {
    let (at, v) = leader;
    i64::from(at.0) + braking_distance(i64::from(v.0.max(0))) - i64::from(SPACING)
}
