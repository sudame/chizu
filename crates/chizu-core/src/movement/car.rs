//! 車線を走る車の 1 tick。
//!
//! 1. 車線ごとに車を前から並べ、前の tick の位置だけを読んで新しい速さと位置を決める（並列）。
//!    速さは、加速の上限、道路の最高速度、前の車との車間、着く位置までの距離の小さいほうにする。
//!    前の車がこの tick に止まっても追突しない距離までしか進まない。
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

/// `next` のリンクで入る車線。`from` からつながる車線のうち、その次のリンク `after` へつながる車線を選ぶ。
/// どれも同じ条件なら番号の小さい車線にする。`from` からつながる車線がなければ、`next` の車線から選ぶ。
pub(super) fn choose_lane(
    net: &Network,
    conn: &[Connector],
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
    cands.sort_unstable();
    let reaches = |l: LaneId| {
        after.is_none_or(|a| {
            let lanes_after: Vec<LaneId> = net.link_lanes(a).collect();
            from_lane(conn, end_node(net, next), l)
                .iter()
                .any(|c| lanes_after.contains(&c.to))
        })
    };
    cands
        .iter()
        .copied()
        .find(|l| reaches(*l))
        .unwrap_or(cands[0])
}

/// 車の新しい速さと、リンクの始まりからの新しい位置。
fn drive(net: &Network, t: &Trip, leader: Option<LaneOffset>) -> (MmPerTick, LaneOffset) {
    let seg = net.segment(t.link().segment).expect("リンクの区間がない");
    let mut v = t
        .speed
        .0
        .saturating_add(CAR_ACCEL)
        .min(speed_limit(seg.kind).0);
    let room = |to: u32| i32::try_from(to.saturating_sub(t.offset.0)).unwrap_or(i32::MAX);
    if let Some(l) = leader {
        v = v.min(room(l.0.saturating_sub(SPACING)));
    }
    if t.last_leg() {
        v = v.min(room(t.dest_offset.0));
    }
    let v = MmPerTick(v.max(0));
    (v, t.offset.advance(v.distance(1)))
}

/// 車を 1 tick 進め、着いた主体の行の番号を返す。
pub(super) fn step(rows: &mut [Option<Trip>], net: &Network, conn: &[Connector]) -> Vec<usize> {
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
        let leader = k
            .checked_sub(1)
            .map(|p| order[p])
            .filter(|p| p.0 == lane)
            .map(|p| p.1.0);
        let t = rows_ro[i].as_ref().expect("車の主体がない");
        let (v, at) = drive(net, t, leader);
        (i, v, at)
    };
    #[cfg(feature = "parallel")]
    let moves: Vec<_> = (0..order.len()).into_par_iter().map(plan).collect();
    #[cfg(not(feature = "parallel"))]
    let moves: Vec<_> = (0..order.len()).map(plan).collect();

    let mut arrived = Vec::new();
    let mut crossing = Vec::new();
    for (i, v, at) in moves {
        let t = rows[i].as_mut().expect("車の主体がない");
        (t.speed, t.offset) = (v, at);
        if t.last_leg() {
            if at >= t.dest_offset {
                arrived.push(i);
            }
        } else if at >= t.link_len {
            crossing.push((end_node(net, t.link()), i));
        }
    }
    // 車線ごとの最後尾の位置。
    let mut rear = BTreeMap::<LaneId, LaneOffset>::new();
    for t in rows.iter().flatten().filter(|t| t.mode == LaneMode::Car) {
        let r = rear.entry(t.lane).or_insert(t.offset);
        *r = (*r).min(t.offset);
    }
    crossing.sort_unstable();
    for (node, i) in crossing {
        let t = rows[i].as_mut().expect("車の主体がない");
        let next = t.route.links[t.leg as usize + 1];
        let after = t.route.links.get(t.leg as usize + 2).copied();
        let lane = choose_lane(net, conn, t.lane, node, next, after);
        let over = t.link_len.distance_to(t.offset);
        let next_len = super::pos::link_geometry(net, next)
            .expect("経路のリンクがない")
            .2;
        let mut place = LaneOffset::START.advance(over).min(next_len);
        if let Some(r) = rear.get(&lane) {
            match r.0.checked_sub(SPACING) {
                Some(room) => place = place.min(LaneOffset(room)),
                None => {
                    (t.offset, t.speed) = (t.link_len, MmPerTick::ZERO);
                    continue;
                }
            }
        }
        t.leg += 1;
        t.enter(net, place, Some(lane));
        let r = rear.entry(lane).or_insert(place);
        *r = (*r).min(place);
        if t.last_leg() && t.offset >= t.dest_offset {
            arrived.push(i);
        }
    }
    arrived
}
