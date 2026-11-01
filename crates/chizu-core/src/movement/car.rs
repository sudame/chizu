//! 車線を走る車の 1 tick。
//!
//! 1. 車線ごとに車を前から並べ、前の tick の位置と速さだけを読んで新しい速さと位置を決める（並列）。
//!    速さは、加速の上限、道路の最高速度、前の車に対する安全速度（Krauss 型）、着く位置に近づく速さの
//!    いちばん小さい値にする。前に車がなくリンクの先へ進む車は、次に入る車線の最後尾を前の車とみなす。
//!    前の車がこの tick に止まっても重ならない距離までしか進まない。
//! 2. リンクの終わりを越えた車を、ノード、主体の番号の順に 1 台ずつ次のリンクへ移す。
//!    移る先の車線の最後尾との車間が足りなければ、リンクの終わりで止める。

use std::collections::BTreeMap;

#[cfg(feature = "parallel")]
use rayon::prelude::*;

use super::Trip;
use crate::command::NodeId;
use crate::network::{Dir, LaneId, LaneMode, LinkId, Network};
use crate::routing::{Connector, speed_limit};
use crate::units::{LaneOffset, MmPerTick};

/// 車の長さ（mm）。
pub const CAR_LENGTH: u32 = 4_500;

/// 止まったときに前の車とのあいだに空ける距離（mm）。
pub const MIN_GAP: u32 = 2_500;

/// 1 tick に上げられる速さ（mm/tick）。
pub const CAR_ACCEL: i32 = 2_500;

/// 安全速度を求めるときの減速の上限（mm/tick）。
pub const CAR_DECEL: i32 = 4_500;

/// 前の車の後ろの端から、自分の前の端までに空ける距離に、車の長さを足したもの。
const SPACING: u32 = CAR_LENGTH + MIN_GAP;

fn end_node(net: &Network, link: LinkId) -> NodeId {
    let s = net.segment(link.segment).expect("リンクの区間がない");
    match link.dir {
        Dir::Forward => s.b,
        Dir::Backward => s.a,
    }
}

/// ノード `node` で車線 `from` から出るつながり。
fn from_lane(conn: &[Connector], node: NodeId, from: LaneId) -> &[Connector] {
    let lo = conn.partition_point(|c| (c.node, c.from) < (node, from));
    let hi = conn.partition_point(|c| (c.node, c.from) <= (node, from));
    &conn[lo..hi]
}

/// 車線ごとの、いまの車の数と最後尾（位置と速さ）。
#[derive(Default)]
pub(super) struct LaneState {
    count: BTreeMap<LaneId, u32>,
    rear: BTreeMap<LaneId, (LaneOffset, MmPerTick)>,
}

impl LaneState {
    pub(super) fn of(rows: &[Option<Trip>]) -> LaneState {
        let mut st = LaneState::default();
        for t in rows.iter().flatten().filter(|t| t.mode == LaneMode::Car) {
            *st.count.entry(t.lane).or_insert(0) += 1;
            let r = st.rear.entry(t.lane).or_insert((t.offset, t.speed));
            if t.offset < r.0 {
                *r = (t.offset, t.speed);
            }
        }
        st
    }

    pub(super) fn add(&mut self, lane: LaneId) {
        *self.count.entry(lane).or_insert(0) += 1;
    }

    pub(super) fn count(&self, lane: LaneId) -> u32 {
        self.count.get(&lane).copied().unwrap_or(0)
    }
}

/// 候補の車線のうち、車の少ない車線。同じなら番号の小さい車線にする。
pub(super) fn emptiest(lanes: &[LaneId], st: &LaneState) -> LaneId {
    *lanes
        .iter()
        .min_by_key(|l| (st.count(**l), **l))
        .expect("候補の車線がない")
}

/// `next` のリンクで入る車線。`from` からつながる車線のうち、その次のリンク `after` へつながる車線を選び、
/// その中で車の少ない車線にする。`from` からつながる車線がなければ、`next` の車線から選ぶ。
pub(super) fn choose_lane(
    net: &Network,
    conn: &[Connector],
    st: &LaneState,
    from: LaneId,
    node: NodeId,
    next: LinkId,
    after: Option<LinkId>,
) -> LaneId {
    let lanes: Vec<LaneId> = net.link_lanes(next).collect();
    let mut cands: Vec<LaneId> = from_lane(conn, node, from)
        .iter()
        .map(|c| c.to)
        .filter(|l| lanes.contains(l))
        .collect();
    if cands.is_empty() {
        cands = lanes;
    }
    let lanes_after: Vec<LaneId> = after
        .map(|a| net.link_lanes(a).collect())
        .unwrap_or_default();
    let reaches = |l: &LaneId| {
        after.is_none()
            || from_lane(conn, end_node(net, next), *l)
                .iter()
                .any(|c| lanes_after.contains(&c.to))
    };
    let good: Vec<LaneId> = cands.iter().copied().filter(reaches).collect();
    emptiest(if good.is_empty() { &cands } else { &good }, st)
}

/// 速さ `v` から、1 tick に `CAR_DECEL` ずつ落として止まるまでに進む距離。
fn braking_distance(v: i64) -> i64 {
    let b = i64::from(CAR_DECEL);
    let n = v / b;
    n * v - b * n * (n + 1) / 2
}

/// 前の車（位置 `at`、速さ `v_l`）に追突しない安全速度。Krauss 型と同じく、前の車がいまから減速の上限で
/// 止まっても、自分も減速の上限で止まれば重ならない速さのうち、いちばん速いものを整数で求める。
/// 1 tick を反応時間とし、この tick に進む距離と、そのあと止まるまでの距離の和が、車間と前の車が
/// 止まるまでの距離の和を超えないようにする。前の tick にこの条件を満たしていれば、減速は上限に収まる。
pub(super) fn safe_speed(me: LaneOffset, at: LaneOffset, v_l: MmPerTick) -> i64 {
    let gap = i64::from(at.0) - i64::from(SPACING) - i64::from(me.0);
    let budget = gap + braking_distance(i64::from(v_l.0.max(0)));
    if budget < 0 {
        return 0;
    }
    // v + braking_distance(v) は v について増えるので、条件を満たすいちばん大きい v を二分探索で求める。
    let (mut lo, mut hi) = (0_i64, budget);
    while lo < hi {
        let mid = lo + (hi - lo + 1) / 2;
        if mid + braking_distance(mid) <= budget {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo
}

/// 車の新しい速さと、リンクの始まりからの新しい位置。`leader` は前の車（いまのリンクの始まりから
/// 数えた位置と速さ）。
fn drive(
    net: &Network,
    t: &Trip,
    leader: Option<(LaneOffset, MmPerTick)>,
) -> (MmPerTick, LaneOffset) {
    let seg = net.segment(t.link().segment).expect("リンクの区間がない");
    let mut v = i64::from(t.speed.0)
        .saturating_add(i64::from(CAR_ACCEL))
        .min(i64::from(speed_limit(seg.kind).0));
    if let Some((at, v_l)) = leader {
        // 安全速度に加えて、前の車がこの tick に止まっても重ならない距離までしか進まない。
        let room = i64::from(at.0) - i64::from(SPACING) - i64::from(t.offset.0);
        v = v.min(safe_speed(t.offset, at, v_l)).min(room);
    }
    if t.last_leg() {
        // 着く位置には、止まった前の車があるものとして近づき、残りが加速の幅に収まったら着く。
        let left = i64::from(t.dest_offset.0) - i64::from(t.offset.0);
        let stop = LaneOffset(t.dest_offset.0.saturating_add(SPACING));
        let approach = safe_speed(t.offset, stop, MmPerTick::ZERO);
        v = v.min(if left <= i64::from(CAR_ACCEL) {
            left
        } else {
            approach.min(left)
        });
    }
    let v = MmPerTick(i32::try_from(v.max(0)).expect("速さが i32 に収まらない"));
    (v, t.offset.advance(v.distance(1)))
}

#[cfg(test)]
fn check_lane(rows: &[Option<Trip>], lane: LaneId) {
    let mut at: Vec<u32> = rows
        .iter()
        .flatten()
        .filter(|t| t.mode == LaneMode::Car && t.lane == lane)
        .map(|t| t.offset.0)
        .collect();
    at.sort_unstable();
    for w in at.windows(2) {
        assert!(w[1] - w[0] >= SPACING, "車線 {lane:?} の車が重なった {w:?}");
    }
}

/// 車を 1 tick 進め、着いた主体の行の番号を返す。
pub(super) fn step(rows: &mut [Option<Trip>], net: &Network, conn: &[Connector]) -> Vec<usize> {
    let st = LaneState::of(rows);
    // 車線ごとに、前の車から順に並べる。
    let mut order: Vec<(LaneId, core::cmp::Reverse<LaneOffset>, usize)> = rows
        .iter()
        .enumerate()
        .filter_map(|(i, t)| {
            let t = t.as_ref().filter(|t| t.mode == LaneMode::Car)?;
            Some((t.lane, core::cmp::Reverse(t.offset), i))
        })
        .collect();
    order.sort_unstable();
    let rows_ro = &*rows;
    let plan = |k: usize| {
        let (lane, _, i) = order[k];
        let t = rows_ro[i].as_ref().expect("車の主体がない");
        let ahead = k
            .checked_sub(1)
            .map(|p| order[p])
            .filter(|p| p.0 == lane)
            .map(|p| {
                let l = rows_ro[p.2].as_ref().expect("車の主体がない");
                (l.offset, l.speed)
            });
        // 前に車がなく、リンクの先へ進むなら、次に入る車線の最後尾を前の車とみなす。
        let mut next_lane = None;
        let leader = match ahead {
            Some(l) => Some(l),
            None if !t.last_leg() => {
                let node = end_node(net, t.link());
                let next = t.route.links[t.leg as usize + 1];
                let after = t.route.links.get(t.leg as usize + 2).copied();
                let lane = choose_lane(net, conn, &st, t.lane, node, next, after);
                next_lane = Some(lane);
                st.rear
                    .get(&lane)
                    .map(|(at, v)| (LaneOffset(t.link_len.0.saturating_add(at.0)), *v))
            }
            None => None,
        };
        let (v, at) = drive(net, t, leader);
        (i, v, at, next_lane)
    };
    #[cfg(feature = "parallel")]
    let moves: Vec<_> = (0..order.len()).into_par_iter().map(plan).collect();
    #[cfg(not(feature = "parallel"))]
    let moves: Vec<_> = (0..order.len()).map(plan).collect();

    let mut arrived = Vec::new();
    let mut crossing = Vec::new();
    for (i, v, at, next_lane) in moves {
        let t = rows[i].as_mut().expect("車の主体がない");
        (t.speed, t.offset) = (v, at);
        if t.last_leg() {
            if at >= t.dest_offset {
                arrived.push(i);
            }
        } else if at >= t.link_len {
            let lane = next_lane.expect("リンクの先の車線を選んでいない");
            crossing.push((end_node(net, t.link()), i, lane));
        }
    }
    // 車線ごとの最後尾。リンクの終わりを越えた車は、移れずに終わりで止まるものとして数える。
    // こうしておくと、どの順に移しても、移った先の車間が保たれる。
    let mut rear = BTreeMap::<LaneId, LaneOffset>::new();
    for t in rows.iter().flatten().filter(|t| t.mode == LaneMode::Car) {
        let at = t.offset.min(t.link_len);
        let r = rear.entry(t.lane).or_insert(at);
        *r = (*r).min(at);
    }
    crossing.sort_unstable();
    for (_, i, lane) in crossing {
        let t = rows[i].as_mut().expect("車の主体がない");
        let next = t.route.links[t.leg as usize + 1];
        let over = t.link_len.distance_to(t.offset);
        let next_len = super::pos::link_geometry(net, next)
            .expect("経路のリンクがない")
            .2;
        let mut place = LaneOffset::START.advance(over).min(next_len);
        let room = rear.get(&lane).map(|r| r.0.checked_sub(SPACING));
        match room {
            Some(None) => {
                (t.offset, t.speed) = (t.link_len, MmPerTick::ZERO);
                #[cfg(test)]
                check_lane(rows, rows[i].as_ref().expect("車の主体がない").lane);
                continue;
            }
            Some(Some(room)) => place = place.min(LaneOffset(room)),
            None => {}
        }
        t.leg += 1;
        t.enter(net, place, Some(lane));
        let r = rear.entry(lane).or_insert(place);
        *r = (*r).min(place);
        if t.last_leg() && t.offset >= t.dest_offset {
            arrived.push(i);
        }
        #[cfg(test)]
        check_lane(rows, lane);
    }
    arrived
}
