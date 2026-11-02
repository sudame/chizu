//! 横断歩道と、道路を横切る歩行者の衝突区間。
//!
//! 歩道は区間の両側にあり、どちらの歩道もどちらの向きにも歩ける。歩道のリンクは歩く向きを表し、
//! 歩く人はそのリンクの進む向きの右（`far` なら左）の歩道にいる。建物の出入口は、建物の側の歩道の
//! 上にある（`lane_shift` と `growth` の側の決め方と同じ）。
//!
//! 横断歩道は、CS2 と同じく 3 本以上の区間が集まる交差点のすべての腕に、自動で置く。交差点で
//! 腕 A の歩道から腕 B の歩道へ移る人は、交差点のまわりを時計回りか反時計回りに回り、通る腕の
//! 横断歩道を順に渡る。回る向きは渡る腕の少ないほうで、同じなら時計回り（マスの y は下向きなので、
//! 向きの角度の増える向き）にする。腕 B のどちらの側へ出るかは、渡る腕の少ないほう（同じなら右）にする。
//! ただし経路の最後のリンクへ出るときは、行き先の建物の側へ出る。3 本より少ない区間のノードでは、
//! 側を変えても車道を渡らないものとする。
//!
//! 出発と行き先が同じリンクの上にあり、建物が道路の反対の側どうしなら、出発の位置で道路を横切る。
//! 経路を選ぶときは、横切りに [`JAYWALK_DETOUR_MM`] を足して比べるので、近くの横断歩道までの
//! 遠回りがそれより短ければ、遠回りして横断歩道を使う（`plan`）。
//!
//! どちらも、渡り始めてから渡り終えるまでのあいだ、渡る区間の車線の上に衝突区間を作る。
//! 衝突区間は、その tick の歩く人の位置から作り直し、状態には持たない。
//! - 車は、交差点で入ってくる腕か出ていく腕の横断歩道に人がいれば、ノードの手前で止まる（`junction`）。
//!   区間の途中の衝突区間は、止まった前の車とみなし、その手前で止まる（`car`）。どちらも、止まりきれない
//!   車は止まらない。
//! - 歩く人は、止まりきれない車が来ているか、交差点の中に車がいるあいだは渡り始めない。
//!   人が途切れずに渡り続けて車がいつまでも進めなくならないよう、[`PED_PATIENCE`] tick より長く待った車の
//!   前の横断歩道では、新しく渡り始めない。渡っている人が渡り終えれば、車が進める。
//! - 信号のある交差点では、歩く人は信号に従う。腕の横断歩道は、その腕の車が赤で、ほかの軸が青に
//!   なってから [`signal::WALK_CLEAR`] tick 前までのあいだにだけ渡り始める。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use super::{junction, signal};
use crate::command::NodeId;
use crate::network::{Dir, LinkId, Network, SegmentId};
use crate::units::Tick;

/// 車線 1 本を渡る距離（mm）。
pub const LANE_CROSS_MM: u32 = 3_000;

/// 区間の途中で横切る代わりに、横断歩道まで遠回りしてもよい距離（mm）。経路を選ぶときに、横切りに
/// この長さを足して比べる。
pub const JAYWALK_DETOUR_MM: u64 = 100_000;

/// この tick 数より長く交差点の手前で待った車の前では、歩く人は新しく渡り始めない。
pub const PED_PATIENCE: u32 = 15;

/// 区間の途中の衝突区間の、渡る人の位置から前後への幅（mm）。
pub const JAY_HALF_MM: u32 = 1_000;

/// 区間の途中で横切る人が、車の手前に空けておく距離（mm）。車が止まるまでの距離に足す。
const JAY_MARGIN_MM: i64 = 5_000;

/// 区間 `segment` の車道を渡る距離。
pub fn width(net: &Network, segment: SegmentId) -> u32 {
    net.segment(segment).map_or(0, |s| {
        u32::from(s.kind.car_lanes_per_side()) * 2 * LANE_CROSS_MM
    })
}

/// 歩く人の、歩道の側と、区間の途中の横切り。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Walk {
    /// いまのリンクの進む向きの左の歩道にいるか。
    pub far: bool,
    /// 歩き出す前に区間の途中で横切るなら、横切った距離。横切り終えたら `None` にし、`far` を変える。
    pub jay: Option<u32>,
    /// 行き先の建物が、経路の最後のリンクの進む向きの左にあるか。
    pub far_end: bool,
}

/// 1 歩の向きの角度。x の正の向きを 0 とし、y の正の向きへ 45 度ごとに 1 ずつ増える。
fn heading(step: (i64, i64)) -> u8 {
    match step {
        (1, 0) => 0,
        (1, 1) => 1,
        (0, 1) => 2,
        (-1, 1) => 3,
        (-1, 0) => 4,
        (-1, -1) => 5,
        (0, -1) => 6,
        (1, -1) => 7,
        _ => panic!("8 方向でない向き {step:?}"),
    }
}

/// ノード `node` から区間 `segment` へ出ていく向きの角度。
fn arm_heading(net: &Network, node: NodeId, segment: SegmentId) -> Option<u8> {
    let s = net.segment(segment)?;
    let other = if s.a == node { s.b } else { s.a };
    let (p, q) = (net.node(node)?.tile(), net.node(other)?.tile());
    Some(heading(((q.x - p.x).signum(), (q.y - p.y).signum())))
}

/// 交差点のまわりを回るときに渡る腕。渡る順に並べる。
pub type Arms = SmallVec<[SegmentId; 4]>;

/// ノード `node` のまわりの歩道の角の位置。腕を向きの角度の順に並べ、腕 i の反時計回りの側を 2i、
/// 時計回りの側を 2i+1 とする。2i から 2i+1 へ回るとき腕 i を渡り、2i+1 から次の腕の 2(i+1) へは
/// 渡らずに角を回る。
struct Corners {
    arms: SmallVec<[SegmentId; 8]>,
}

impl Corners {
    fn of(net: &Network, node: NodeId) -> Option<Corners> {
        let n = net.node(node)?;
        let mut arms: SmallVec<[(u8, SegmentId); 8]> = n
            .segments
            .iter()
            .filter_map(|s| Some((arm_heading(net, node, *s)?, *s)))
            .collect();
        arms.sort_unstable();
        Some(Corners {
            arms: arms.into_iter().map(|(_, s)| s).collect(),
        })
    }

    fn index(&self, s: SegmentId) -> Option<usize> {
        self.arms.iter().position(|x| *x == s)
    }

    /// 角 `p` から角 `q` へ、渡る腕の少ない向きに回るときに渡る腕。
    fn walk(&self, p: usize, q: usize) -> Arms {
        let m = 2 * self.arms.len();
        let mut cw = Arms::new();
        let mut at = p;
        while at != q {
            if at % 2 == 0 {
                cw.push(self.arms[at / 2]);
            }
            at = (at + 1) % m;
        }
        let mut ccw = Arms::new();
        let mut at = p;
        while at != q {
            if at % 2 == 1 {
                ccw.push(self.arms[at / 2]);
            }
            at = (at + m - 1) % m;
        }
        if cw.len() <= ccw.len() { cw } else { ccw }
    }
}

/// 歩道のリンク `from` の `from_far` の側からノード `node` を通ってリンク `to` へ移る人の、`to` での側と、
/// 渡る腕。`need` があれば、その側へ出る。なければ渡る腕の少ない側（同じなら右）へ出る。
/// 3 本より少ない区間のノードでは渡る腕はない。
pub fn depart(
    net: &Network,
    node: NodeId,
    (from, from_far): (LinkId, bool),
    to: LinkId,
    need: Option<bool>,
) -> (bool, Arms) {
    let junction = junction::is_junction(net, node);
    if let (false, Some(side)) = (junction, need) {
        return (side, Arms::new());
    }
    let found = Corners::of(net, node).and_then(|c| {
        let (i, j) = (c.index(from.segment)?, c.index(to.segment)?);
        // 腕 i へ入ってくる人の右は反時計回りの側、腕 j から出ていく人の右は時計回りの側である。
        let p = 2 * i + usize::from(from_far);
        let q = |far: bool| 2 * j + usize::from(!far);
        let side = need.unwrap_or_else(|| c.walk(p, q(true)).len() < c.walk(p, q(false)).len());
        Some((side, c.walk(p, q(side))))
    });
    let (side, arms) = found.unwrap_or((need.unwrap_or(from_far), Arms::new()));
    if junction {
        (side, arms)
    } else {
        (side, Arms::new())
    }
}

/// 歩く人がいま渡っている所。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Occ {
    /// 交差点 `node` の、区間の腕の横断歩道。
    Crosswalk(NodeId, SegmentId),
    /// 歩道のリンクの上の位置から、区間の途中で横切っている。
    Jay(LinkId, u32),
}

/// 歩く人がこの tick に渡り始めてよいかを決めるための、前の tick の車の様子。
#[derive(Default)]
pub(super) struct Gate {
    pub now: Tick,
    /// 歩く人が渡り始めてはいけない横断歩道（交差点と腕の区間）。
    pub busy: BTreeSet<(NodeId, SegmentId)>,
    /// 区間の途中で横切る人がいる区間の、車線の上の車（向き、前の端の位置、速さ、止まった tick 数）。
    pub cars: BTreeMap<SegmentId, Vec<(Dir, u32, i32, u32)>>,
}

impl Gate {
    /// 交差点 `node` の、区間 `segment` の腕の横断歩道を渡り始めてよいか。
    pub fn may_cross(&self, net: &Network, node: NodeId, segment: SegmentId) -> bool {
        if self.busy.contains(&(node, segment)) {
            return false;
        }
        match signal::state(net, node, self.now) {
            None => true,
            Some(st) => {
                let Some(h) = arm_heading(net, node, segment) else {
                    return false;
                };
                st.walk(h % 4)
            }
        }
    }

    /// 歩道のリンク `link` の位置 `at` から、区間の途中で横切り始めてよいか。
    pub fn may_jaywalk(&self, net: &Network, link: LinkId, at: u32) -> bool {
        let Some(cars) = self.cars.get(&link.segment) else {
            return true;
        };
        let len = super::pos::link_geometry(net, link).map_or(0, |g| g.2.0);
        !cars.iter().any(|&(dir, front, v, stalled)| {
            // 車のリンクの上での、横切る位置。
            let z = if dir == link.dir {
                at
            } else {
                len.saturating_sub(at)
            };
            let (z, front) = (i64::from(z), i64::from(front));
            let far = z + i64::from(JAY_HALF_MM) + i64::from(super::CAR_LENGTH);
            if front >= far {
                return false;
            }
            let near = z - i64::from(JAY_HALF_MM) - front;
            let v = i64::from(v);
            let reach = super::car::braking_distance(v) + v + JAY_MARGIN_MM;
            near < reach || (v == 0 && stalled >= PED_PATIENCE && near < 3 * JAY_MARGIN_MM)
        })
    }
}

/// 歩く人がいま渡っている所から作った、車の側から見た衝突区間。
#[derive(Default)]
pub(super) struct Occupied {
    /// 人のいる横断歩道（交差点と腕の区間）。
    pub crosswalks: BTreeSet<(NodeId, SegmentId)>,
    /// 車のリンクごとの、区間の途中の衝突区間の真ん中の位置。小さい順に並ぶ。
    pub jay: BTreeMap<LinkId, Vec<u32>>,
}

impl Occupied {
    pub fn of(net: &Network, occs: impl IntoIterator<Item = Occ>) -> Occupied {
        let mut o = Occupied::default();
        for occ in occs {
            match occ {
                Occ::Crosswalk(n, s) => {
                    o.crosswalks.insert((n, s));
                }
                Occ::Jay(link, at) => {
                    let len = super::pos::link_geometry(net, link).map_or(0, |g| g.2.0);
                    for dir in [Dir::Forward, Dir::Backward] {
                        let z = if dir == link.dir {
                            at
                        } else {
                            len.saturating_sub(at)
                        };
                        let car = LinkId {
                            segment: link.segment,
                            dir,
                            mode: crate::network::LaneMode::Car,
                        };
                        o.jay.entry(car).or_default().push(z);
                    }
                }
            }
        }
        for v in o.jay.values_mut() {
            v.sort_unstable();
        }
        o
    }
}
