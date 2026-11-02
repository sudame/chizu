//! 横断歩道と、道路を横切る歩行者の衝突区間。
//!
//! 歩道は区間の両側にあり、どちらの歩道もどちらの向きにも歩ける。歩道のリンクは歩く向きを表し、
//! 歩く人はそのリンクの進む向きの右（`far` なら左）の歩道にいる。建物の出入口は、建物の側の歩道の
//! 上にある（`lane_shift` と `growth` の側の決め方と同じ）。
//!
//! 横断歩道は、CS2 と同じく 3 本以上の区間が集まる交差点の、高速道路でないすべての腕に、自動で置く。交差点で
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
//! - 信号のない交差点では、歩く人は手前まで来た車を先に通す。車が来ていなければ渡り始め、来た車は
//!   止まりきれるかぎり人の前で止まる。
//! - 車道を渡り始めるのを [`PED_WAIT`] tick より長く待った人がいる横断歩道では、止まりきれる車を止め、
//!   止まりきれない車と交差点の中の車が抜けたら、待っていた人がまとめて渡る。ただし、横断歩道の人に譲って
//!   その人より長く待った車がいれば、先にその車を通す。車が途切れない交差点でも人はいつか渡れ、
//!   人が途切れない交差点でも車はいつか進める。待った長さを比べるので、人と車が交互に進む。
//!   車の待ちは、横断歩道の人だけのために止まった tick 数（`Trip::yielded`）で数える。ほかの車や先の
//!   車線の空きを待つ車は、渡っても待ちが延びないので、人を止めない。
//! - 信号のある交差点では、歩く人は信号に従う。腕の横断歩道は、その腕の車が赤で、ほかの軸が青に
//!   なってから [`signal::WALK_CLEAR`] tick 前までのあいだにだけ渡り始める。青の腕で、横断歩道の人に
//!   [`PED_PATIENCE`] tick より長く譲った車がいれば、その人より長く待った人のほかは渡り始めない。
//! - 区間の途中で横切る人は、止まりきれない車が来ているか、車が横切る所にかかっているあいだは横切らない。
//!   [`PED_WAIT`] tick より長く待った人は、止まっている車のすぐ前でも横切り、車がかかっていなければ、
//!   止まりきれる車にその前を空けさせる（[`Jaywalk::Hold`]）。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use super::{junction, signal};
use crate::command::NodeId;
use crate::network::{Dir, LinkId, Network, SegmentId, SegmentKind};
use crate::units::Tick;

/// 車線 1 本を渡る距離（mm）。
pub const LANE_CROSS_MM: u32 = 3_000;

/// 区間の途中で横切る代わりに、横断歩道まで遠回りしてもよい距離（mm）。経路を選ぶときに、横切りに
/// この長さを足して比べる。
pub const JAYWALK_DETOUR_MM: u64 = 100_000;

/// 交差点の手前で、横断歩道の人のためにこの tick 数より長く待った車の前では、歩く人は新しく渡り始めない。
pub const PED_PATIENCE: u32 = 15;

/// 車道を渡り始めるのをこの tick 数より長く待った人は、止まりきれる車を止めて渡る。
pub const PED_WAIT: u32 = 40;

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
    /// 腕の区間と、高速道路か。
    arms: SmallVec<[(SegmentId, bool); 8]>,
}

/// 角から角へ回る道筋。越える高速道路の腕の数と、渡る腕。
type Way = (usize, Arms);

impl Corners {
    fn of(net: &Network, node: NodeId) -> Option<Corners> {
        let n = net.node(node)?;
        let mut arms: SmallVec<[(u8, SegmentId, bool); 8]> = n
            .segments
            .iter()
            .filter_map(|s| {
                let highway = net.segment(*s)?.kind == SegmentKind::Highway;
                Some((arm_heading(net, node, *s)?, *s, highway))
            })
            .collect();
        arms.sort_unstable();
        Some(Corners {
            arms: arms.into_iter().map(|(_, s, h)| (s, h)).collect(),
        })
    }

    fn index(&self, s: SegmentId) -> Option<usize> {
        self.arms.iter().position(|x| x.0 == s)
    }

    /// 角 `p` から角 `q` へ回る道筋のうち、越える高速道路の腕が少なく、次に渡る腕の少ないもの。
    /// 高速道路の腕には横断歩道がないので、渡る腕には入れない。
    fn walk(&self, p: usize, q: usize) -> Way {
        let m = 2 * self.arms.len();
        let way = |forward: bool| {
            let (mut highways, mut arms) = (0, Arms::new());
            let mut at = p;
            while at != q {
                if at % 2 == usize::from(!forward) {
                    let (s, highway) = self.arms[at / 2];
                    if highway {
                        highways += 1;
                    } else {
                        arms.push(s);
                    }
                }
                at = if forward {
                    (at + 1) % m
                } else {
                    (at + m - 1) % m
                };
            }
            (highways, arms)
        };
        let (cw, ccw) = (way(true), way(false));
        if (cw.0, cw.1.len()) <= (ccw.0, ccw.1.len()) {
            cw
        } else {
            ccw
        }
    }
}

/// 歩道のリンク `from` の `from_far` の側からノード `node` を通ってリンク `to` へ移る人の、`to` での側と、
/// 渡る腕。`need` があれば、その側へ出る。なければ、越える高速道路の腕が少なく、次に渡る腕の少ない
/// 側（同じなら右）へ出る。3 本より少ない区間のノードでは渡る腕はない。
///
/// 高速道路の腕は渡らずに回る。どちらに回っても高速道路の腕を越えるときだけ、横断歩道なしに越える。
/// 外部接続の高速道路は買った土地の端の 1 本だけなので、そうなるノードはふつうはできない。
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
        let key = |w: &Way| (w.0, w.1.len());
        let side = need.unwrap_or_else(|| key(&c.walk(p, q(true))) < key(&c.walk(p, q(false))));
        Some((side, c.walk(p, q(side)).1))
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
    /// 歩道のリンクの上の位置から、区間の途中で横切っている。`false` なら、まだ横切り始めていないが、
    /// [`PED_WAIT`] tick より長く待ったので、車にその前を空けさせている。
    Jay(LinkId, u32, bool),
}

/// 歩く人がこの tick に渡り始めてよいかを決めるための、前の tick の車と歩く人の様子。
#[derive(Default)]
pub(super) struct Gate {
    pub now: Tick,
    /// 止まりきれない車か交差点の中の車が通っていて、歩く人が渡り始めてはいけない横断歩道（交差点と腕の区間）。
    pub busy: BTreeSet<(NodeId, SegmentId)>,
    /// 交差点の手前まで来た、止まりきれる車が通る横断歩道と、止まって横断歩道の人に譲った車の、譲った tick 数の
    /// いちばん大きい値。信号のある交差点では、腕が青の車だけを数える。
    pub near: BTreeMap<(NodeId, SegmentId), u32>,
    /// [`PED_WAIT`] tick より長く待った人がいて、止まりきれる車を止めて渡らせる横断歩道。
    pub claimed: BTreeSet<(NodeId, SegmentId)>,
    /// 区間の途中で横切る人がいる区間の車（向き、前の端の位置、速さ、止まった tick 数、車線に出ているか）。
    /// 隣のリンクからノードを越えて入ってくる車は、前の端の位置を負の値で表す。出発を待つ車は、
    /// 出発の位置に止まった、車線に出ていない車として入れ、車線に出るのを待った tick 数を添える。
    pub cars: BTreeMap<SegmentId, Vec<JayCar>>,
}

/// 区間の途中で横切る人から見た車。向き、前の端の位置、速さ、止まった tick 数、車線に出ているか。
pub(super) type JayCar = (Dir, i64, i32, u32, bool);

impl Gate {
    /// 交差点 `node` の、区間 `segment` の腕の横断歩道を渡り始めてよいか。
    ///
    /// 止まりきれない車か交差点の中の車がいれば渡らない。信号があれば、信号に従う。
    /// そのうえで、長く待った人がいれば渡る。そうでなければ、信号のない交差点では手前まで来た車を
    /// 先に通し、信号のある交差点では [`PED_PATIENCE`] tick より長く譲った車がいるときだけ先に通す。
    /// 信号のない交差点で車が途切れなくても、人は待ちすぎた所で車を止めて渡る。
    pub fn may_cross(&self, net: &Network, node: NodeId, segment: SegmentId) -> bool {
        let at = (node, segment);
        if self.busy.contains(&at) || !walk_light(net, node, segment, self.now) {
            return false;
        }
        if self.claimed.contains(&at) {
            return true;
        }
        match self.near.get(&at) {
            None => true,
            Some(&c) => signal::state(net, node, self.now).is_some() && c < PED_PATIENCE,
        }
    }

    /// 横断歩道 `at` の前で、いちばん長く待った人が `waited` tick 待ったとき、止まりきれる車を止めて渡らせるかを決め、
    /// 渡らせるなら [`Gate::claimed`] に入れる。譲って待った車がいれば、その車より長く待った人だけが渡らせる。
    /// 待った長さを比べるので、人と車のどちらも、相手を待たせ続けることはない。
    pub fn claim(&mut self, net: &Network, at: (NodeId, SegmentId), waited: u32) {
        let c = self.near.get(&at).copied().unwrap_or(0);
        if waited > c && walk_light(net, at.0, at.1, self.now) {
            self.claimed.insert(at);
        }
    }

    /// 歩道のリンク `link` の位置 `at` から、区間の途中で横切り始めてよいか。`waited` は、その人が待った tick 数である。
    ///
    /// 止まりきれない車が来ているあいだと、車が横切る所にかかっているあいだは横切らない。長く止まった車の
    /// 前でも、先に通すために横切らない。[`PED_WAIT`] tick より長く待った人は、止まっている車の前なら
    /// 間を空けずに横切り始め、自分より短く待った、出発を待つ車も待たない。横切れずに待つときも、横切る所に
    /// 車がかかっていなければ、車にその前を空けさせる（[`Jaywalk::Hold`]）。かかっている所で空けさせると、
    /// かかっている車の後ろが動けなくなり、空けさせた所どうしが互いを待ち続けうる。空けさせた所は、
    /// 来ている車が止まるか過ぎれば横切れる。
    pub fn jaywalk(&self, net: &Network, link: LinkId, at: u32, waited: u32) -> Jaywalk {
        let Some(cars) = self.cars.get(&link.segment) else {
            return Jaywalk::Cross;
        };
        let len = super::pos::link_geometry(net, link).map_or(0, |g| g.2.0);
        let patient = waited >= PED_WAIT;
        let (mut over, mut blocked) = (false, false);
        for &(dir, front, v, stalled, on_road) in cars {
            // 出発を待つ車は、横切る人の衝突区間を避けて置かれる。その車より長く待った人は、その車を待たない。
            if patient && !on_road && waited > stalled {
                continue;
            }
            // 車のリンクの上での、横切る位置。
            let z = if dir == link.dir {
                at
            } else {
                len.saturating_sub(at)
            };
            let z = i64::from(z);
            let far = z + i64::from(JAY_HALF_MM) + i64::from(super::CAR_LENGTH);
            if front >= far {
                continue;
            }
            let near = z - i64::from(JAY_HALF_MM) - front;
            over |= near < 0;
            let v = i64::from(v);
            let margin = if patient && v == 0 { 0 } else { JAY_MARGIN_MM };
            let reach = super::car::braking_distance(v) + v + margin;
            let turn = v == 0 && stalled >= PED_PATIENCE && !patient;
            blocked |= near < reach || (turn && near < 3 * JAY_MARGIN_MM);
        }
        match (blocked, patient && !over) {
            (false, _) => Jaywalk::Cross,
            (true, true) => Jaywalk::Hold,
            (true, false) => Jaywalk::Wait,
        }
    }
}

/// 区間の途中で横切ろうとする人が、この tick にすること。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Jaywalk {
    /// 横切り始める。
    Cross,
    /// 待ちながら、車に横切る所の前を空けさせる。
    Hold,
    /// 待つ。
    Wait,
}

/// 信号が、交差点 `node` の区間 `segment` の腕の横断歩道を渡り始めてよいとしているか。信号がなければ `true`。
fn walk_light(net: &Network, node: NodeId, segment: SegmentId, now: Tick) -> bool {
    match signal::state(net, node, now) {
        None => true,
        Some(st) => arm_heading(net, node, segment).is_some_and(|h| st.walk(h % 4)),
    }
}

/// いま区間の途中を横切っている人の、車のリンクごとの衝突区間の真ん中の位置。tick の途中で車を置くときに使う。
pub(super) fn jay_zones(net: &Network, rows: &[Option<super::Trip>]) -> BTreeMap<LinkId, Vec<u32>> {
    let occs = rows.iter().flatten().filter_map(|t| {
        let crossing =
            t.mode == crate::network::LaneMode::Sidewalk && t.walk.jay.is_some_and(|p| p > 0);
        crossing.then(|| Occ::Jay(t.link(), t.offset().0, true))
    });
    Occupied::of(net, occs).jay
}

/// 歩く人がいま渡っている所から作った、車の側から見た衝突区間。
#[derive(Default)]
pub(super) struct Occupied {
    /// 人のいる横断歩道（交差点と腕の区間）。
    pub crosswalks: BTreeSet<(NodeId, SegmentId)>,
    /// 車のリンクごとの、区間の途中を横切っている人の衝突区間の真ん中の位置。小さい順に並ぶ。
    pub jay: BTreeMap<LinkId, Vec<u32>>,
    /// `jay` に、長く待って前を空けさせている人の位置を足したもの。止まりきれる車はこの手前で止まり、
    /// 出発する車もここには置かない。
    pub zones: BTreeMap<LinkId, Vec<u32>>,
}

impl Occupied {
    pub fn of(net: &Network, occs: impl IntoIterator<Item = Occ>) -> Occupied {
        let mut o = Occupied::default();
        for occ in occs {
            match occ {
                Occ::Crosswalk(n, s) => {
                    o.crosswalks.insert((n, s));
                }
                Occ::Jay(link, at, crossing) => {
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
                        o.zones.entry(car).or_default().push(z);
                        if crossing {
                            o.jay.entry(car).or_default().push(z);
                        }
                    }
                }
            }
        }
        for v in o.jay.values_mut().chain(o.zones.values_mut()) {
            v.sort_unstable();
        }
        o
    }
}
