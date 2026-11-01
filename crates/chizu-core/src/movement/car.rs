//! 車線を走る車の 1 tick。
//!
//! 1. 車線ごとに車を前から並べ、前の tick の位置と速さだけを読んで新しい速さと位置を決める（並列）。
//!    速さは、加速の上限、道路の最高速度、前の車に対する安全速度（Krauss 型）、着く位置に近づく速さの
//!    いちばん小さい値にする。前に車がなくリンクの先へ進む車は、次に入る車線の最後尾を前の車とみなす。
//!    前の車がこの tick に止まっても重ならない距離までしか進まない。
//!    交差点（`junction`）でほかの車に譲る車は、ノードの手前で止まる速さまでしか出さない。
//! 2. リンクの終わりを越えた車を、ノード、主体の番号の順に 1 台ずつ次のリンクへ移す。
//!    移る先の車線の最後尾との車間が足りなければ、リンクの終わりで止める。
//!
//! 1. の前に、前の tick の状態だけを読んで、交差点ごとに手前で止める車をノードの順に決める。

use std::collections::BTreeMap;

#[cfg(feature = "parallel")]
use rayon::prelude::*;

use super::Trip;
use super::junction::{self, Approach, Inside};
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

/// 交差点を抜けた車が、交差点の中にいるとみなされるあいだに進む距離。
const JUNCTION_CLEAR: u32 = SPACING;

/// 交差点から出ていく車線に入るときに要る、最後尾までの距離。入った車は、前の車が止まっていても
/// `JUNCTION_CLEAR` まで進んで交差点を抜けきれる。
const JUNCTION_ROOM: u32 = SPACING + JUNCTION_CLEAR;

/// ノード `node` をまたぐ車が、行き先の車線の最後尾までに要る距離。
fn room_needed(net: &Network, node: NodeId) -> u32 {
    if junction::is_junction(net, node) {
        JUNCTION_ROOM
    } else {
        SPACING
    }
}

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

/// 車線の上を走っている車か。出発を待っている車は、まだ車線の上にいない。
pub(super) fn on_road(t: &Trip) -> bool {
    t.mode == LaneMode::Car && !t.waiting
}

/// 車線ごとの、車線の上の車の位置。前から順に並ぶ。
#[derive(Default)]
pub(super) struct LaneIndex(BTreeMap<LaneId, Vec<LaneOffset>>);

impl LaneIndex {
    /// 行 `skip` を除いた、車線の上の車の位置。
    pub(super) fn of(rows: &[Option<Trip>], skip: Option<usize>) -> LaneIndex {
        let mut ix = LaneIndex::default();
        for (i, t) in rows.iter().enumerate() {
            if let Some(t) = t.as_ref().filter(|t| on_road(t) && Some(i) != skip) {
                ix.0.entry(t.lane).or_default().push(t.offset);
            }
        }
        for v in ix.0.values_mut() {
            v.sort_unstable_by(|a, b| b.cmp(a));
        }
        ix
    }

    fn count(&self, lane: LaneId) -> usize {
        self.0.get(&lane).map_or(0, Vec::len)
    }

    /// 車線 `lane` で、位置 `at` から後ろへ下げていき、ほかの車と重ならない最初の位置。
    fn room_behind(&self, lane: LaneId, at: LaneOffset) -> Option<LaneOffset> {
        let mut p = at.0;
        for c in self.0.get(&lane).into_iter().flatten() {
            if c.0 >= p.saturating_add(SPACING) {
                continue;
            }
            if c.0.saturating_add(SPACING) <= p {
                break;
            }
            p = c.0.checked_sub(SPACING)?;
        }
        Some(LaneOffset(p))
    }

    fn insert(&mut self, lane: LaneId, at: LaneOffset) {
        let v = self.0.entry(lane).or_default();
        let k = v.partition_point(|c| *c > at);
        v.insert(k, at);
    }

    /// 車の少ない順に並べた車線。同じなら番号の順にする。
    fn by_count(&self, lanes: &[LaneId]) -> Vec<LaneId> {
        let mut l = lanes.to_vec();
        l.sort_by_key(|x| (self.count(*x), *x));
        l
    }
}

/// 行 `i` の車を、いまのリンクの車線に置く。車の少ない車線から順に、いまの位置から後ろへ下げて
/// 空いた位置を探す。どの車線にも空きがなければ、出発を待たせて `false` を返す。
/// `ix` は行 `i` を除いた車線の上の車で、置いたら行 `i` を足す。
pub(super) fn place(
    rows: &mut [Option<Trip>],
    net: &Network,
    ix: &mut LaneIndex,
    i: usize,
) -> bool {
    let t = rows[i].as_mut().expect("置く車がない");
    let lanes: Vec<LaneId> = net.link_lanes(t.link()).collect();
    let found = ix
        .by_count(&lanes)
        .into_iter()
        .find_map(|l| Some((l, ix.room_behind(l, t.offset)?)));
    match found {
        Some((lane, at)) => {
            (t.lane, t.offset, t.waiting) = (lane, at, false);
            ix.insert(lane, at);
            true
        }
        None => {
            t.waiting = true;
            false
        }
    }
}

/// 探し直した行 `i` の車の車線を決める。前と同じ番号の車線に重ならずにいられればそこに、だめなら
/// 重ならない車線のうち車の少ない車線に置く。どこも重なるなら、前と同じ番号の車線のままにする。
pub(super) fn keep_lane(rows: &mut [Option<Trip>], net: &Network, i: usize, index: usize) {
    let ix = LaneIndex::of(rows, Some(i));
    let t = rows[i].as_ref().expect("置く車がない");
    let lanes: Vec<LaneId> = net.link_lanes(t.link()).collect();
    let same = lanes[index.min(lanes.len() - 1)];
    let fits = |l: LaneId| ix.room_behind(l, t.offset) == Some(t.offset);
    let others: Vec<LaneId> = lanes.iter().copied().filter(|l| *l != same).collect();
    let lane = std::iter::once(same)
        .chain(ix.by_count(&others))
        .find(|l| fits(*l))
        .unwrap_or(same);
    rows[i].as_mut().expect("置く車がない").lane = lane;
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
        for t in rows.iter().flatten().filter(|t| on_road(t)) {
            *st.count.entry(t.lane).or_insert(0) += 1;
            let r = st.rear.entry(t.lane).or_insert((t.offset, t.speed));
            if t.offset < r.0 {
                *r = (t.offset, t.speed);
            }
        }
        st
    }

    pub(super) fn count(&self, lane: LaneId) -> u32 {
        self.count.get(&lane).copied().unwrap_or(0)
    }
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
    let on = |l: LaneId, link: LinkId| {
        net.lane(l)
            .is_some_and(|x| (x.segment, x.dir, x.mode) == (link.segment, link.dir, link.mode))
    };
    let reaches = |l: &LaneId| {
        after.is_none_or(|a| {
            from_lane(conn, end_node(net, next), *l)
                .iter()
                .any(|c| on(c.to, a))
        })
    };
    let joined = || {
        from_lane(conn, node, from)
            .iter()
            .map(|c| c.to)
            .filter(move |l| on(*l, next))
    };
    // 車の少ない車線。同じなら番号の小さい車線にする。
    let best = |good: &mut dyn Iterator<Item = LaneId>| good.min_by_key(|l| (st.count(*l), *l));
    let found = if joined().next().is_some() {
        best(&mut joined().filter(reaches)).or_else(|| best(&mut joined()))
    } else {
        best(&mut net.link_lanes(next).filter(reaches)).or_else(|| best(&mut net.link_lanes(next)))
    };
    found.expect("候補の車線がない")
}

/// 速さ `v` から、1 tick に `CAR_DECEL` ずつ落として止まるまでに進む距離。
fn braking_distance(v: i64) -> i64 {
    let b = i64::from(CAR_DECEL);
    let n = v / b;
    n * v - b * n * (n + 1) / 2
}

/// リンクの終わりまで `left` のとき、次のリンクの最高速度 `v_next` まで減速の上限で落としながら
/// 入れる、いまのいちばん速い速さ。`v_next` より速いあいだに進む距離の和が `left` に届かない速さである。
fn entry_speed(left: i64, v_next: i64) -> i64 {
    let b = i64::from(CAR_DECEL);
    let fast = |v: i64| {
        let n = (v - v_next + b - 1) / b;
        n * v - b * n * (n - 1) / 2
    };
    let (mut lo, mut hi) = (v_next, v_next + left.max(0));
    while lo < hi {
        let mid = lo + (hi - lo + 1) / 2;
        if fast(mid) < left {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo
}

/// 前の車（位置 `at`、速さ `v_l`）に追突しない安全速度。Krauss 型と同じく、前の車がいまから減速の上限で
/// 止まっても、自分も減速の上限で止まれば重ならない速さのうち、いちばん速いものを整数で求める。
/// 1 tick を反応時間とし、この tick に進む距離と、そのあと止まるまでの距離の和が、車間と前の車が
/// 止まるまでの距離の和を超えないようにする。前の tick にこの条件を満たしていれば、減速は上限に収まる。
///
/// 合流では減速の上限を破りうる。ノードで次の車線へ移るときに位置をずらされた車や、入れ替わった
/// 最後尾は、前の tick の条件を満たしていないためである。重なりは起きない。
/// 交差点では、別のリンクから同じリンクへ入る車をノードの手前で待たせるので、横から入った車で
/// 最後尾が入れ替わるのは、止まりきれない車どうしが同じ tick に入るときだけになる。
/// 同じリンクの何本かの車線が 1 本へ寄るときと、先の車線の最後尾が見込みより進まずに位置を
/// ずらされたときは、まだ破りうる。
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
/// `held` なら、交差点でほかの車に譲り、ノードの手前で止まる。
fn drive(
    net: &Network,
    t: &Trip,
    leader: Option<(LaneOffset, MmPerTick)>,
    held: bool,
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
    if !t.last_leg() && near_end(net, t) {
        let next = t.route.links[t.leg as usize + 1];
        let next_kind = net.segment(next.segment).expect("リンクの区間がない").kind;
        let left = i64::from(t.link_len.0) - i64::from(t.offset.0);
        v = v.min(entry_speed(left, i64::from(speed_limit(next_kind).0)));
    }
    if held {
        v = v.min(stop_speed(t));
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

/// リンクの終わりに止まった車があるものとして近づき、リンクの終わりを越えない速さ。
fn stop_speed(t: &Trip) -> i64 {
    let left = i64::from(t.link_len.0) - i64::from(t.offset.0);
    let line = LaneOffset(t.link_len.0.saturating_add(SPACING));
    safe_speed(t.offset, line, MmPerTick::ZERO).min(left)
}

/// 位置を求める前に、車線の先頭の車ごとに見ておくこと。
#[derive(Default)]
struct Look {
    /// リンクの先へ進む車が次に入る車線。リンクの終わりが止まれる距離より近いときだけ選ぶ。
    next_lane: Option<LaneId>,
    /// 交差点の手前まで来た車。
    approach: Option<Approach>,
    /// 交差点の中にいる車。
    inside: Option<Inside>,
}

/// `order[k]` の車について [`Look`] を作る。`order` は車線ごとに前から並べた車である。
fn look(
    rows: &[Option<Trip>],
    net: &Network,
    conn: &[Connector],
    st: &LaneState,
    order: &[(LaneId, core::cmp::Reverse<LaneOffset>, usize)],
    k: usize,
) -> Look {
    let (lane, _, i) = order[k];
    let t = rows[i].as_ref().expect("車の主体がない");
    let mut out = Look::default();
    if t.leg > 0 && t.offset.0 < JUNCTION_CLEAR {
        let prev = t.route.links[t.leg as usize - 1];
        let node = end_node(net, prev);
        if junction::is_junction(net, node) {
            out.inside = Some(Inside {
                node,
                from: prev,
                to: t.link(),
                stopped: t.speed == MmPerTick::ZERO,
            });
        }
    }
    // 後ろの車は前の車に追従するので、車線の先頭の車だけを見る。
    let head = k == 0 || order[k - 1].0 != lane;
    if !head || t.last_leg() || !near_end(net, t) {
        return out;
    }
    let node = end_node(net, t.link());
    let next = t.route.links[t.leg as usize + 1];
    let after = t.route.links.get(t.leg as usize + 2).copied();
    let to_lane = choose_lane(net, conn, st, t.lane, node, next, after);
    out.next_lane = Some(to_lane);
    if !junction::is_junction(net, node) {
        return out;
    }
    // 先の車線の最後尾がこの tick も同じ速さで進むとして、入った車が交差点を抜けきれるか。
    // 見込みが外れても、ノードをまたぐときに確かめ直すので、交差点の中で止まる車は出ない。
    let room = st.rear.get(&to_lane).is_none_or(|(at, v)| {
        i64::from(at.0) + i64::from(v.0) >= i64::from(room_needed(net, node))
    });
    let seg = net.segment(t.link().segment).expect("リンクの区間がない");
    out.approach = Some(Approach {
        id: k,
        node,
        from: t.link(),
        to: next,
        lane,
        class: junction::class(seg.kind),
        committed: stop_speed(t) < i64::from(t.speed.0) - i64::from(CAR_DECEL),
        room,
        waited: t.waited,
    });
    out
}

/// リンクの終わりまでが、最高速度から止まるまでの距離と車間より近いか。
fn near_end(net: &Network, t: &Trip) -> bool {
    let seg = net.segment(t.link().segment).expect("リンクの区間がない");
    let vmax = i64::from(speed_limit(seg.kind).0);
    let left = i64::from(t.link_len.0) - i64::from(t.offset.0);
    left <= vmax + i64::from(CAR_ACCEL) + braking_distance(vmax) + i64::from(SPACING)
}

#[cfg(test)]
fn check_lane(rows: &[Option<Trip>], lane: LaneId) {
    let mut at: Vec<u32> = rows
        .iter()
        .flatten()
        .filter(|t| on_road(t) && t.lane == lane)
        .map(|t| t.offset.0)
        .collect();
    at.sort_unstable();
    for w in at.windows(2) {
        assert!(w[1] - w[0] >= SPACING, "車線 {lane:?} の車が重なった {w:?}");
    }
}

/// 車を 1 tick 進め、着いた主体の行の番号を返す。
pub(super) fn step(rows: &mut [Option<Trip>], net: &Network, conn: &[Connector]) -> Vec<usize> {
    // 出発を待っている車を、主体の番号の順に置き直す。
    let waiting: Vec<usize> = (0..rows.len())
        .filter(|i| rows[*i].as_ref().is_some_and(|t| t.waiting))
        .collect();
    let mut ix = LaneIndex::of(rows, None);
    for i in waiting {
        place(rows, net, &mut ix, i);
    }
    let st = LaneState::of(rows);
    // 車線ごとに、前の車から順に並べる。
    let mut order: Vec<(LaneId, core::cmp::Reverse<LaneOffset>, usize)> = rows
        .iter()
        .enumerate()
        .filter_map(|(i, t)| {
            let t = t.as_ref().filter(|t| on_road(t))?;
            Some((t.lane, core::cmp::Reverse(t.offset), i))
        })
        .collect();
    order.sort_unstable();
    let rows_ro = &*rows;
    let look_at = |k: usize| look(rows_ro, net, conn, &st, &order, k);
    #[cfg(feature = "parallel")]
    let looks: Vec<Look> = (0..order.len()).into_par_iter().map(look_at).collect();
    #[cfg(not(feature = "parallel"))]
    let looks: Vec<Look> = (0..order.len()).map(look_at).collect();
    // 交差点の手前で止める車を、前の tick の状態だけからノードの順に決める。
    let mut held = vec![false; order.len()];
    let (cands, inside) = looks.iter().fold((Vec::new(), Vec::new()), |mut acc, l| {
        acc.0.extend(l.approach);
        acc.1.extend(l.inside);
        acc
    });
    for k in junction::holds(net, cands, inside) {
        held[k] = true;
    }
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
        // 次の車線の最後尾がこの tick の速さに効くのは、リンクの終わりが止まれる距離より近いときだけである。
        let next_lane = looks[k].next_lane;
        let leader = ahead.or_else(|| {
            st.rear
                .get(&next_lane?)
                .map(|(at, v)| (LaneOffset(t.link_len.0.saturating_add(at.0)), *v))
        });
        let (v, at) = drive(net, t, leader, held[k]);
        (i, v, at, next_lane, held[k])
    };
    #[cfg(feature = "parallel")]
    let moves: Vec<_> = (0..order.len()).into_par_iter().map(plan).collect();
    #[cfg(not(feature = "parallel"))]
    let moves: Vec<_> = (0..order.len()).map(plan).collect();

    let mut arrived = Vec::new();
    let mut crossing = Vec::new();
    for (i, v, at, next_lane, held) in moves {
        let t = rows[i].as_mut().expect("車の主体がない");
        (t.speed, t.offset) = (v, at);
        if held {
            t.waited = t.waited.saturating_add(1);
            if t.waited > junction::RESCUE {
                arrived.push(i);
                continue;
            }
        }
        if t.last_leg() {
            if at >= t.dest_offset {
                arrived.push(i);
            }
        } else if at >= t.link_len && !held {
            let lane = next_lane.expect("リンクの先の車線を選んでいない");
            crossing.push((end_node(net, t.link()), i, lane));
        }
    }
    // 車線ごとの最後尾。リンクの終わりを越えた車は、移れずに終わりで止まるものとして数える。
    // こうしておくと、どの順に移しても、移った先の車間が保たれる。
    let mut rear = BTreeMap::<LaneId, LaneOffset>::new();
    for t in rows.iter().flatten().filter(|t| on_road(t)) {
        let at = t.offset.min(t.link_len);
        let r = rear.entry(t.lane).or_insert(at);
        *r = (*r).min(at);
    }
    crossing.sort_unstable();
    for (node, i, lane) in crossing {
        let t = rows[i].as_mut().expect("車の主体がない");
        // 交差点では、入った車が交差点の中で止まらないだけの空きを求める。
        let need = room_needed(net, node);
        let next = t.route.links[t.leg as usize + 1];
        let over = t.link_len.distance_to(t.offset);
        let next_len = super::pos::link_geometry(net, next)
            .expect("経路のリンクがない")
            .2;
        let mut place = LaneOffset::START.advance(over).min(next_len);
        let room = rear
            .get(&lane)
            .map(|r| (r.0 >= need).then(|| r.0 - SPACING));
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
        t.waited = 0;
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
