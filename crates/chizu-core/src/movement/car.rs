//! 車線を走る車の 1 tick。
//!
//! 1. 車線ごとに車を前から並べ、前の tick の位置と速さだけを読んで新しい速さと位置を決める（並列）。
//!    速さは、加速の上限、道路の最高速度、前の車に対する安全速度（Krauss 型）、着く位置に近づく速さの
//!    いちばん小さい値にする。前に車がなくリンクの先へ進む車は、次に入る車線の最後尾を前の車とみなす。
//!    前の車がこの tick に止まっても重ならない距離までしか進まない。
//!    交差点（`junction`）でほかの車に譲る車は、停止線（`stop_line`）で止まる速さまでしか出さない。
//! 2. ノードの中の通り抜けの区間に入る所（交差点の停止線と、角の道路の端）を越えた車に、ノード、主体の番号の
//!    順に 1 台ずつ、次のリンクで入る車線を決める（`Trip::bound`）。交差点では、止まりきれる車は、その車線に
//!    交差点を抜けきるだけの空きがなければ停止線で止める。
//! 3. リンクの終わりを越えた車を、ノード、主体の番号の順に 1 台ずつ、決めた車線へ移す。
//!    移る先の車線の最後尾との車間が足りなければ、リンクの終わりで止める。
//!
//! 1. の前に、前の tick の状態だけを読んで、交差点ごとに停止線で止める車をノードの順に決める。
//!    信号のある交差点では、tick と前の tick の車と歩く人の様子から決まる信号の段階も読む（[`signal_demand`]）。

use std::collections::{BTreeMap, BTreeSet};

#[cfg(feature = "parallel")]
use rayon::prelude::*;

use smallvec::SmallVec;

use super::Trip;
use super::crossing::{self, JayCar};
use super::junction::{self, Approach, Inside};
use super::rail_crossing::{self, RailCrossings};
use super::signal;
use super::stop_line::{box_reach, clear_line, has_interior, stop_line};
use crate::command::NodeId;
use crate::network::{Dir, LaneId, LaneMode, LinkId, Network};
use crate::routing::{Connector, speed_limit};
use crate::units::{LaneOffset, MmPerTick, Tick};

/// 車の長さ（mm）。
pub const CAR_LENGTH: u32 = 4_500;

/// 止まったときに前の車とのあいだに空ける距離（mm）。
pub const MIN_GAP: u32 = 2_500;

/// 1 tick に上げられる速さ（mm/tick）。
pub const CAR_ACCEL: i32 = 2_500;

/// 安全速度を求めるときの減速の上限（mm/tick）。
pub const CAR_DECEL: i32 = 4_500;

/// 駐車場に入る車が、出入りの位置で車線から外れるときの速さ（mm/tick）。時速 7.2km である。
pub const PARK_ENTRY_SPEED: i32 = 2_000;

/// 前の車の後ろの端から、自分の前の端までに空ける距離に、車の長さを足したもの。
pub(super) const SPACING: u32 = CAR_LENGTH + MIN_GAP;

/// 交差点 `node` の停止線を越えて出ていくリンク `next` へ向かう車が、行き先の車線の最後尾までに要る距離
/// （`next` の始まりから数える）。入った車は、前の車が止まっていても、交差点を抜けきる所（[`clear_line`]）まで
/// 進める。止まりきれずに入った車と、出ていくリンクがこの距離より短い車は例外で、`junction` の先頭に書いた
/// 救済が受け持つ。交差点でなければ車間だけ。
fn room_needed(net: &Network, node: NodeId, next: LinkId) -> u32 {
    if junction::is_junction(net, node) {
        SPACING + clear_line(net, next).0
    } else {
        SPACING
    }
}

/// 車 `t` が通り抜けの区間に入るときに決めた、次のリンクの車線。
pub(super) fn bound_lane(net: &Network, t: &Trip) -> Option<LaneId> {
    let index = usize::from(t.bound?);
    let next = *t.route.links.get(t.leg as usize + 1)?;
    let lanes: SmallVec<[LaneId; 4]> = net.link_lanes(next).collect();
    lanes.get(index.min(lanes.len().checked_sub(1)?)).copied()
}

/// 車 `t` が待つ位置。まだ通り抜けの区間に入っておらず、停止線より先にいる車（出発の位置が停止線より先の車や、
/// 網が変わって停止線が手前へ移った車）は、いまの位置で待つ。
fn line_of(net: &Network, t: &Trip) -> u32 {
    stop_line(net, t.link()).0.max(t.offset.0)
}

/// 車 `t` が、始まりの交差点の中にいるか。いまのリンクの始まりから、交差点を抜けきる所の手前までにいる車である。
/// 抜けてきたリンクが網が変わって消えていれば、中にいないとみなす。中にいれば、その交差点と抜けてきたリンク。
fn inside_start(net: &Network, t: &Trip) -> Option<(NodeId, LinkId)> {
    if t.leg == 0 || t.offset >= clear_line(net, t.link()) {
        return None;
    }
    let prev = t.route.links[t.leg as usize - 1];
    net.segment(prev.segment)?;
    let node = end_node(net, prev);
    junction::is_junction(net, node).then_some((node, prev))
}

pub(super) fn end_node(net: &Network, link: LinkId) -> NodeId {
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

/// 車線ごとの、車線の上の車の位置。前から順に並ぶ。通り抜けの区間に入ってその車線へ向かう車は、
/// 車線の始まりにいるものとして入れる。その車がノードを越えたときに、車線の始まりに空きが残るようにする。
#[derive(Default)]
pub(super) struct LaneIndex(BTreeMap<LaneId, Vec<LaneOffset>>);

impl LaneIndex {
    /// 行 `skip` を除いた、車線の上の車の位置。
    pub(super) fn of(rows: &[Option<Trip>], net: &Network, skip: Option<usize>) -> LaneIndex {
        let mut ix = LaneIndex::default();
        for (i, t) in rows.iter().enumerate() {
            if let Some(t) = t.as_ref().filter(|t| on_road(t) && Some(i) != skip) {
                ix.0.entry(t.lane).or_default().push(t.offset);
                if let Some(l) = bound_lane(net, t) {
                    ix.0.entry(l).or_default().push(LaneOffset::START);
                }
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

/// 車線に一度も出られないまま `RESCUE` tick を越えて待っている車か。
pub(super) fn given_up(t: &Trip) -> bool {
    t.mode == LaneMode::Car && t.waiting && t.stalled > junction::RESCUE
}

/// 交差点の手前まで来た車線の先頭の車が、交差点の先で入りうる車線（[`lane_choices`]）。
///
/// 駐車場から出る車がこの車線で後ろへ下がって入ると、交差点から入ろうとする車の空きを先に取ってしまう。
/// 列が詰まっていると、交差点の先頭の車は空きを待ち続け、その手前の交差点まで列が延びて輪になる。
/// 先頭の車は車線の車の数と空きで入る車線を選ぶので、駐車場から出る車を置くと選ぶ車線が変わりうる。そのため、
/// 選びうる車線をすべて守る。
#[derive(Default)]
pub(super) struct Entering(BTreeSet<LaneId>);

impl Entering {
    /// `rows` の車線の上の車から決める。
    pub(super) fn of(rows: &[Option<Trip>], net: &Network, conn: &[Connector]) -> Entering {
        // 車線ごとの、まだ通り抜けの区間に入っていない先頭の車の行。入った車は、決めた車線を守る。
        let mut heads = BTreeMap::<LaneId, (LaneOffset, usize)>::new();
        let mut out = BTreeSet::new();
        for (i, t) in rows.iter().enumerate() {
            if let Some(t) = t.as_ref().filter(|t| on_road(t)) {
                if let Some(l) = bound_lane(net, t) {
                    if junction::is_junction(net, end_node(net, t.link())) {
                        out.insert(l);
                    }
                    continue;
                }
                let h = heads.entry(t.lane).or_insert((t.offset, i));
                if t.offset > h.0 {
                    *h = (t.offset, i);
                }
            }
        }
        for (_, i) in heads.into_values() {
            let t = rows[i].as_ref().expect("車の主体がない");
            if t.last_leg() || !near_end(net, t) {
                continue;
            }
            let node = end_node(net, t.link());
            if !junction::is_junction(net, node) {
                continue;
            }
            let next = t.route.links[t.leg as usize + 1];
            let after = t.route.links.get(t.leg as usize + 2).copied();
            out.extend(lane_choices(net, conn, t.lane, node, next, after));
        }
        Entering(out)
    }

    fn guards(&self, lane: LaneId) -> bool {
        self.0.contains(&lane)
    }
}

/// 行 `i` の車を、いまのリンクの車線に置く。車の少ない車線から順に、いまの位置から後ろへ下げて
/// 空いた位置を探す。ただし、交差点から車の入ってくる車線（`entering`）では、いまの位置から
/// 車 2 台ぶんより後ろへは下げない。どの車線にも空きがなければ、車線の外（駐車場の中）で待たせて `false` を返す。
/// `ix` は行 `i` を除いた車線の上の車で、置いたら行 `i` を足す。
/// `jay` は車のリンクごとの、区間の途中の衝突区間の真ん中の位置で、車がその区間にかかる位置には置かない。
/// 踏切 `rail` にも、開いていても閉じていても、体がかかる位置には置かない。
pub(super) fn place(
    rows: &mut [Option<Trip>],
    net: &Network,
    ix: &mut LaneIndex,
    i: usize,
    jay: &BTreeMap<LinkId, Vec<u32>>,
    rail: &RailCrossings,
    entering: &Entering,
) -> bool {
    let t = rows[i].as_mut().expect("置く車がない");
    let lanes: Vec<LaneId> = net.link_lanes(t.link()).collect();
    let zones = jay.get(&t.link()).map_or(&[][..], Vec::as_slice);
    let spans: SmallVec<[(u32, u32); 2]> = rail
        .on_link(t.link(), t.link_len.0)
        .iter()
        .map(|c| (c.near, c.far))
        .collect();
    let nearest = t.offset.0.saturating_sub(2 * SPACING);
    let found = ix.by_count(&lanes).into_iter().find_map(|l| {
        let at = room_clear_of(ix, l, t.offset, zones, &spans)?;
        (at.0 >= nearest || !entering.guards(l)).then_some((l, at))
    });
    match found {
        Some((lane, at)) => {
            (t.lane, t.offset, t.waiting, t.rejoining, t.stalled) = (lane, at, false, false, 0);
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
/// 重ならない車線のうち車の少ない車線に置く。どこも重なるなら（道路の種類を下げて車線が減ったときなど）、
/// 止めて出発を待つ車にし、空いたら `place` で置き直す。
pub(super) fn keep_lane(rows: &mut [Option<Trip>], net: &Network, i: usize, index: usize) {
    let ix = LaneIndex::of(rows, net, Some(i));
    let t = rows[i].as_ref().expect("置く車がない");
    let lanes: Vec<LaneId> = net.link_lanes(t.link()).collect();
    let same = lanes[index.min(lanes.len() - 1)];
    let fits = |l: LaneId| ix.room_behind(l, t.offset) == Some(t.offset);
    let others: Vec<LaneId> = lanes.iter().copied().filter(|l| *l != same).collect();
    let found = std::iter::once(same)
        .chain(ix.by_count(&others))
        .find(|l| fits(*l));
    let t = rows[i].as_mut().expect("置く車がない");
    match found {
        Some(lane) => t.lane = lane,
        // 待ち始めから数え直す。引き継いだまま待たせると、すぐ歩きに切り替わってしまう。
        None => {
            (t.lane, t.speed, t.waiting, t.rejoining, t.stalled) =
                (same, MmPerTick::ZERO, true, true, 0);
        }
    }
}

/// 車線の上の車の、車線のリンクの始まりからの位置（mm）と速さ（mm/tick）。後ろから順に並ぶ。通り抜けの区間に
/// 入ってこの車線へ向かう車も含め、その位置はノードの手前にいるので 0 か負になる。
#[derive(Clone, Default)]
pub(super) struct Tail(Vec<(i64, i64)>);

impl Tail {
    fn add(tails: &mut BTreeMap<LaneId, Tail>, lane: LaneId, at: i64, v: MmPerTick) {
        let cars = &mut tails.entry(lane).or_default().0;
        let k = cars.partition_point(|c| c.0 <= at);
        cars.insert(k, (at, i64::from(v.0)));
    }

    /// 最後尾。
    fn last(&self) -> (i64, i64) {
        self.0[0]
    }

    /// 位置 `at` にいる車のすぐ前の車の位置と速さ。`at` にいる車のうち 1 台は、その車自身とみなして飛ばす。
    fn ahead_of(&self, at: i64) -> Option<(i64, i64)> {
        let k = self.0.partition_point(|c| c.0 < at);
        let rest = &self.0[k..];
        match rest.first() {
            Some(c) if c.0 == at => rest.get(1).copied(),
            c => c.copied(),
        }
    }

    /// 最後尾が `need` より先まで進んでいそうか。最後尾が交差点を抜けていく車（`leaving`）なら、入る車が抜けきる所に
    /// 着くまでの `ticks` tick のあいだ、車線の車がそれぞれ同じ速さで進むとみなす。ただし、どの車も前の車から車間を
    /// 空けた所までしか進まず、先頭の車はその車線の停止線 `stop` までしか進まない。そうでなければ、最後尾が `base`
    /// tick のあいだ進むとみなす。交差点を抜けていく車の後ろに続く車を、前の車が交差点を抜けきるまで待たせない
    /// ためである。
    fn roomy(&self, base: i64, ticks: i64, leaving: bool, stop: i64, need: i64) -> bool {
        let (at, v) = self.last();
        if !leaving {
            return at + v * base >= need;
        }
        let mut reach = i64::MAX;
        for (k, &(a, w)) in self.0.iter().enumerate().rev() {
            let free = if k + 1 == self.0.len() {
                (a + w * ticks).min(stop.max(a))
            } else {
                a + w * ticks
            };
            reach = free.min(reach.saturating_sub(i64::from(SPACING)));
        }
        reach >= need
    }
}

/// 車線ごとの、いまの車の数と最後尾（位置と速さ）。
#[derive(Default)]
pub(super) struct LaneState {
    count: BTreeMap<LaneId, u32>,
    rear: BTreeMap<LaneId, (LaneOffset, MmPerTick)>,
    /// 車線ごとの車。通り抜けの区間に入ってこの車線へ向かう車も含める。
    tails: BTreeMap<LaneId, Tail>,
}

impl LaneState {
    pub(super) fn of(rows: &[Option<Trip>], net: &Network) -> LaneState {
        let mut st = LaneState::default();
        for t in rows.iter().flatten().filter(|t| on_road(t)) {
            *st.count.entry(t.lane).or_insert(0) += 1;
            let r = st.rear.entry(t.lane).or_insert((t.offset, t.speed));
            if t.offset < r.0 {
                *r = (t.offset, t.speed);
            }
            Tail::add(&mut st.tails, t.lane, i64::from(t.offset.0), t.speed);
            if let Some(l) = bound_lane(net, t) {
                // 車線を選ぶときは、その車線へ向かう車も数える。
                *st.count.entry(l).or_insert(0) += 1;
                let at = i64::from(t.offset.0) - i64::from(t.link_len.0);
                Tail::add(&mut st.tails, l, at, t.speed);
            }
        }
        st
    }

    /// 車線 `lane` の最後尾。通り抜けの区間に入ってこの車線へ向かう車も含める。
    fn last(&self, lane: LaneId) -> Option<(i64, MmPerTick)> {
        self.tails.get(&lane).map(|t| {
            (
                t.last().0,
                MmPerTick(i32::try_from(t.last().1).expect("速さが i32 に収まらない")),
            )
        })
    }

    pub(super) fn count(&self, lane: LaneId) -> u32 {
        self.count.get(&lane).copied().unwrap_or(0)
    }
}

/// `next` のリンクで入りうる車線。`from` からつながる車線のうち、その次のリンク `after` へつながる車線にする。
/// そういう車線がなければ、`from` からつながる車線すべてにする。`from` からつながる車線がなければ、
/// `next` の車線から同じように選ぶ。
pub(super) fn lane_choices(
    net: &Network,
    conn: &[Connector],
    from: LaneId,
    node: NodeId,
    next: LinkId,
    after: Option<LinkId>,
) -> SmallVec<[LaneId; 4]> {
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
    let mut pool: SmallVec<[LaneId; 4]> = from_lane(conn, node, from)
        .iter()
        .map(|c| c.to)
        .filter(|l| on(*l, next))
        .collect();
    if pool.is_empty() {
        pool = net.link_lanes(next).collect();
    }
    let good: SmallVec<[LaneId; 4]> = pool.iter().copied().filter(reaches).collect();
    if good.is_empty() { pool } else { good }
}

/// `next` のリンクで入る車線。[`lane_choices`] のうち車の少ない車線にする。同じなら番号の小さい車線にする。
pub(super) fn choose_lane(
    net: &Network,
    conn: &[Connector],
    st: &LaneState,
    from: LaneId,
    node: NodeId,
    next: LinkId,
    after: Option<LinkId>,
) -> LaneId {
    lane_choices(net, conn, from, node, next, after)
        .into_iter()
        .min_by_key(|l| (st.count(*l), *l))
        .expect("候補の車線がない")
}

/// 交差点で `next` のリンクへ入る車線。左側通行なので、左へ曲がる車は道路の端の車線に、右へ曲がる車は
/// 中央寄りの車線に入る。こうすると、同じリンクへ左と右から曲がって入る車が、別の車線へ同時に入れる。
/// その車線に交差点を抜けきるだけの空き（`room`）がなければ、直進と同じく [`choose_lane`] で選ぶ。
#[allow(clippy::too_many_arguments)]
fn junction_lane(
    net: &Network,
    conn: &[Connector],
    st: &LaneState,
    t: &Trip,
    node: NodeId,
    next: LinkId,
    after: Option<LinkId>,
    room: impl Fn(&LaneId) -> bool,
) -> LaneId {
    let pool = lane_choices(net, conn, t.lane, node, next, after);
    if pool.len() > 1 {
        let index = |l: &LaneId| net.link_lanes(next).position(|x| x == *l);
        let side = match junction::Move::new(net, t.link(), next).bend() {
            1..=3 => pool.iter().min_by_key(|l| index(l)),
            5..=7 => pool.iter().max_by_key(|l| index(l)),
            _ => None,
        };
        if let Some(l) = side.filter(|l| room(l)) {
            return *l;
        }
    }
    pool.into_iter()
        .min_by_key(|l| (st.count(*l), *l))
        .expect("候補の車線がない")
}

/// 速さ `v` から、1 tick に `CAR_DECEL` ずつ落として止まるまでに進む距離。
pub(super) fn braking_distance(v: i64) -> i64 {
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
/// 交差点では、別のリンクから同じリンクへ入る車を停止線で待たせるので、横から入った車で
/// 最後尾が入れ替わるのは、止まりきれない車どうしが同じ tick に入るときだけになる。前の車が
/// ノードを抜けて車線の先頭になったとたんに止まりきれない車が、これにあたる。
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
/// `stop` なら、交差点でほかの車に譲るか先の車線の空きを待って、停止線で止まる。
/// `jay` は、いまのリンクの上の、区間の途中の衝突区間の真ん中の位置で、小さい順に並ぶ。前にあれば
/// その手前で止まる。どちらも、減速の上限の内で止まれないときは止まらない。
fn drive(
    net: &Network,
    t: &Trip,
    leader: Option<(LaneOffset, MmPerTick)>,
    stop: bool,
    jay: &[u32],
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
    if stop {
        let s = stop_speed(net, t);
        if s >= i64::from(t.speed.0) - i64::from(CAR_DECEL) {
            v = v.min(s);
        }
    }
    // 次のリンクの終わりが交差点なら、その停止線で止まれる速さまでしか出さない。次のリンクが短いと、ノードを越えた
    // とたんに止まりきれない車になり、譲るかを確かめずにその交差点の四角へ入るためである。次のリンクが長ければ
    // 停止線は遠いので、この上限は効かない。
    if t.leg as usize + 2 < t.route.links.len() {
        let next = t.route.links[t.leg as usize + 1];
        if junction::is_junction(net, end_node(net, next)) {
            let line = t.link_len.0.saturating_add(stop_line(net, next).0);
            let s = safe_speed(
                t.offset,
                LaneOffset(line.saturating_add(SPACING)),
                MmPerTick::ZERO,
            )
            .min(i64::from(line) - i64::from(t.offset.0));
            if s >= i64::from(t.speed.0) - i64::from(CAR_DECEL) {
                v = v.min(s);
            }
        }
    }
    // 前の端がまだ衝突区間を抜けていなければ、区間の手前に止まった車があるものとして近づく。
    if let Some(&z) = jay
        .iter()
        .find(|z| **z + crossing::JAY_HALF_MM >= t.offset.0)
    {
        let line = jay_line(z);
        let at = LaneOffset(line.saturating_add(SPACING));
        let s =
            safe_speed(t.offset, at, MmPerTick::ZERO).min(i64::from(line) - i64::from(t.offset.0));
        if s >= i64::from(t.speed.0) - i64::from(CAR_DECEL) {
            v = v.min(s);
        }
    }
    // 着く位置へは、入る速さまで減速の上限で落としながら近づき、届いたら車線から外れる。
    // 車線の上で止まらないので、後ろの車は止まらずに済む。
    if let Some(left) = left_to_dest(net, t, seg.kind) {
        v = v.min(entry_speed(left, i64::from(PARK_ENTRY_SPEED)));
    }
    let v = MmPerTick(i32::try_from(v.max(0)).expect("速さが i32 に収まらない"));
    (v, t.offset.advance(v.distance(1)))
}

/// 着く位置までの距離。減速が要るほど近いときだけ返す。着く位置が先のリンクにあれば、
/// あいだのリンクの長さを足す。網が変わって先のリンクがなくなっていれば `None`。
fn left_to_dest(net: &Network, t: &Trip, kind: crate::network::SegmentKind) -> Option<i64> {
    let vmax = i64::from(speed_limit(kind).0) + i64::from(CAR_ACCEL);
    let reach = vmax + braking_distance(vmax);
    if t.last_leg() {
        let left = i64::from(t.dest_offset.0) - i64::from(t.offset.0);
        return (left <= reach).then_some(left);
    }
    let mut left = i64::from(t.link_len.0) - i64::from(t.offset.0);
    let last = t.route.links.len() - 1;
    for k in t.leg as usize + 1..=last {
        if left > reach {
            return None;
        }
        left += if k == last {
            i64::from(t.dest_offset.0)
        } else {
            i64::from(super::pos::link_geometry(net, t.route.links[k])?.2.0)
        };
    }
    (left <= reach).then_some(left)
}

/// 車 `t` が、踏切の先に抜けきる空きがないために手前で止まる衝突区間（いまのリンクの始まりから測る）。
/// 空きは、前の車 `leader` が減速の上限で止まった所から車間を空けた所か、前の車がなければリンクの終わりの
/// 停止線までで測る。`next` なら、ノードの先の次のリンクの踏切も見る。
fn crossing_holds(
    net: &Network,
    rail: &RailCrossings,
    t: &Trip,
    leader: Option<(LaneOffset, MmPerTick)>,
    next: bool,
) -> SmallVec<[u32; 2]> {
    let link = t.link();
    let len = t.link_len.0;
    let front = i64::from(t.offset.0) + i64::from(CAR_LENGTH / 2);
    let lead = leader.map(rail_crossing::bound_behind);
    let bound = lead.unwrap_or_else(|| i64::from(stop_line(net, link).0));
    let after = next
        .then(|| t.route.links.get(t.leg as usize + 1).copied())
        .flatten()
        .and_then(|n| {
            let n_len = super::pos::link_geometry(net, n)?.2.0;
            let b = lead.unwrap_or_else(|| i64::from(len) + i64::from(stop_line(net, n).0));
            Some((n, n_len, b))
        });
    rail.no_room_zones((link, len, bound), after, front)
}

/// 体が踏切にかかった車の前の車。同じ車線に前の車がなく（`from_next`）、前の車が、ほかのリンクから交差点に
/// 入って同じ車線へ向かう車（まだノードを越えていない車）なら、停止線までは進めるとみなす。
///
/// 踏切に入るときに抜けきる空きを確かめても、そのあとにほかの腕の車が交差点に入ると、その車を前の車として
/// 車間を空けるので、交差点のすぐ手前の踏切では体が踏切に残ったまま止まる。その車は交差点の四角の中にいて、
/// 停止線までの車線にはいないので、停止線まで進んでもぶつからない。停止線を越えるかは、ほかの車と同じく交差点の
/// 譲り合いと先の車線の空きで決まる。
fn clear_of_crossing(
    net: &Network,
    rail: &RailCrossings,
    t: &Trip,
    from_next: bool,
    leader: Option<(LaneOffset, MmPerTick)>,
) -> Option<(LaneOffset, MmPerTick)> {
    let (at, _) = leader?;
    if !from_next || t.bound.is_some() || at >= t.link_len || rail.is_empty() {
        return leader;
    }
    let p = i64::from(t.offset.0);
    let half = i64::from(CAR_LENGTH / 2);
    let on = rail
        .on_link(t.link(), t.link_len.0)
        .iter()
        .any(|c| p - half < i64::from(c.far) && i64::from(c.near) < p + half);
    let line = LaneOffset(stop_line(net, t.link()).0.saturating_add(SPACING));
    if on && at < line {
        Some((line, MmPerTick::ZERO))
    } else {
        leader
    }
}

/// 区間の途中の衝突区間の手前で止まる車が、衝突区間とのあいだに空ける距離（mm）。
const JAY_STOP_GAP: u32 = 1_000;

/// 真ん中が `z` の衝突区間の手前で、車の前の端を止める位置。
pub(super) fn jay_line(z: u32) -> u32 {
    z.saturating_sub(crossing::JAY_HALF_MM + JAY_STOP_GAP)
}

/// 車線 `lane` で、位置 `at` から後ろへ下げていき、ほかの車とも、衝突区間 `zones` とも、踏切 `rail` とも
/// 重ならない最初の位置。`rail` は踏切の手前の端と向こうの端で、開いていても閉じていても、体が踏切にかかる
/// 位置には置かず、閉じた踏切で止まる位置まで下げる。踏切の中に置いた車は、閉じた踏切でも止まらずに
/// 踏切を通り、列車とぶつかりかけるためである。
fn room_clear_of(
    ix: &LaneIndex,
    lane: LaneId,
    at: LaneOffset,
    zones: &[u32],
    rail: &[(u32, u32)],
) -> Option<LaneOffset> {
    let half = CAR_LENGTH / 2;
    // 真ん中がこの範囲（両端を含まない）にあれば重なるとみなし、3 つ目の位置まで下げる。
    let keep_out = zones
        .iter()
        .map(|z| {
            (
                jay_line(*z),
                z.saturating_add(crossing::JAY_HALF_MM) + CAR_LENGTH,
                jay_line(*z),
            )
        })
        .chain(rail.iter().map(|(near, far)| {
            (
                near.saturating_sub(half),
                far.saturating_add(half),
                jay_line(rail_crossing::zone_of(*near)),
            )
        }))
        .collect::<SmallVec<[(u32, u32, u32); 4]>>();
    let mut p = ix.room_behind(lane, at)?;
    // 下げるたびに前の衝突区間より手前へ移るので、衝突区間の数より多くは回らない。
    for _ in 0..=keep_out.len() {
        let hit = keep_out
            .iter()
            .filter(|(lo, hi, _)| *lo < p.0 && p.0 < *hi)
            .map(|k| k.2)
            .max();
        let Some(back) = hit else {
            return Some(p);
        };
        p = ix.room_behind(lane, LaneOffset(back))?;
    }
    Some(p)
}

/// 歩く人がこの tick に渡り始めてよいかを決めるための、前の tick の車と歩く人の様子を集める。
///
/// 止まりきれずに交差点へ入る車と交差点の中の車が入る腕と出る腕の横断歩道は、`busy` に入れる。
/// 交差点の手前まで来た、止まりきれる車が入る腕と出る腕の横断歩道は、`near` に入れ、止まって横断歩道の人に
/// 譲った tick 数を添える。信号のある交差点では、腕が青の車だけを数える。黄と赤の車は停止線で止まり、
/// 横断歩道を通らないからである。先の車線の最後尾が止まっていて、交差点を抜けきる空きがない車も数えない。
/// 最後に、[`crossing::PED_WAIT`] tick 以上待った人のいる横断歩道を、譲った車と待った長さを比べて `claimed` に入れる。
pub(super) fn gate(
    rows: &[Option<Trip>],
    net: &Network,
    conn: &[Connector],
    now: Tick,
    signals: signal::Demand,
) -> crossing::Gate {
    let mut g = crossing::Gate {
        now,
        signals,
        ..Default::default()
    };
    // 区間の途中で横切る人のいる区間と、車の主体を 1 度に集める。
    let mut jay_segments = std::collections::BTreeSet::new();
    let mut cars = Vec::new();
    let mut waiting = Vec::new();
    let mut walkers = false;
    // 横断歩道の前で長く待った人。
    let mut patient = Vec::new();
    for t in rows.iter().flatten() {
        if t.mode == LaneMode::Sidewalk {
            walkers = true;
            if t.walk.jay.is_some() {
                jay_segments.insert(t.link().segment);
            } else if t.waited >= crossing::PED_WAIT {
                patient.push(t);
            }
        } else if t.waiting {
            waiting.push(t);
        } else {
            cars.push(t);
        }
    }
    if !walkers {
        return g;
    }
    let st = LaneState::of(rows, net);
    // 出発で置かれる車は、出発の位置に止まった、まだ車線に出ていない車として数える。
    for t in waiting {
        let link = t.link();
        if jay_segments.contains(&link.segment) {
            g.cars.entry(link.segment).or_default().push(JayCar {
                dir: link.dir,
                front: i64::from(t.offset.0),
                speed: 0,
                stalled: t.stalled,
                on_road: false,
            });
        }
    }
    for t in cars {
        let link = t.link();
        if jay_segments.contains(&link.segment) {
            g.cars.entry(link.segment).or_default().push(JayCar {
                dir: link.dir,
                front: i64::from(t.offset.0),
                speed: t.speed.0,
                stalled: t.stalled,
                on_road: true,
            });
        }
        // ノードを越えて横切る区間へ入ってくる車は、次のリンクの始まりより手前にいるものとして数える。
        if !t.last_leg() && near_end(net, t) {
            let next = t.route.links[t.leg as usize + 1];
            if jay_segments.contains(&next.segment) {
                let behind = i64::from(t.offset.0) - i64::from(t.link_len.0);
                g.cars.entry(next.segment).or_default().push(JayCar {
                    dir: next.dir,
                    front: behind,
                    speed: t.speed.0,
                    stalled: t.stalled,
                    on_road: true,
                });
            }
        }
        let mut busy = |node: NodeId, segs: [crate::network::SegmentId; 2]| {
            for s in segs {
                g.busy.insert((node, s));
                if t.speed == MmPerTick::ZERO {
                    g.stuck.insert((node, s));
                }
            }
        };
        if let Some((node, prev)) = inside_start(net, t) {
            busy(node, [prev.segment, link.segment]);
        }
        if t.last_leg() || !near_end(net, t) {
            continue;
        }
        let node = end_node(net, link);
        if !junction::is_junction(net, node) {
            continue;
        }
        let next = t.route.links[t.leg as usize + 1];
        // 停止線を越えた車は、交差点の中にいる。
        if t.bound.is_some() {
            busy(node, [link.segment, next.segment]);
            continue;
        }
        if committed(net, t) {
            g.busy.insert((node, link.segment));
            g.busy.insert((node, next.segment));
            continue;
        }
        // 信号のある交差点では、腕が青の車だけを手前まで来た車として数える。黄と赤の車は停止線で止まり、
        // 横断歩道を通らないので、歩く人はその車を待たない。
        let axis = net
            .segment_line(link.segment)
            .map_or(0, |l| signal::axis(l.step));
        let light = signal::state(net, node, now, &g.signals).map(|st| st.light_for(axis));
        if light.is_some_and(|l| l != signal::Light::Green) {
            continue;
        }
        // 信号のある交差点では、先の車線の最後尾が止まっていて、交差点を抜けきるだけの空きがない車も数えない。
        // 動く見込みのない車に、渡ってよい窓を使わせない。最後尾が動いていれば空きはすぐにできるので、その車は数える。
        // 信号のない交差点では数える。人が先に渡ると、空きができたときに車が人の列に止められ、止まった車が増えた。
        let after = t.route.links.get(t.leg as usize + 2).copied();
        let (to_lane, room) = exit_lane(
            net,
            conn,
            &st,
            t,
            node,
            next,
            after,
            &RailCrossings::default(),
        );
        if light.is_some()
            && !room
            && st
                .rear
                .get(&to_lane)
                .is_some_and(|r| r.1 == MmPerTick::ZERO)
        {
            continue;
        }
        let stopped = t.speed == MmPerTick::ZERO;
        let yielded = if stopped && t.yielded > 0 {
            t.yielded
        } else {
            0
        };
        for s in [link.segment, next.segment] {
            let c = g.near.entry((node, s)).or_insert(0);
            *c = (*c).max(yielded);
        }
    }
    // 長く待った人が、いまどの横断歩道の前にいるか。渡り始めるのを待つ人は、渡る所の始まりにいる。
    let mut longest = BTreeMap::<(NodeId, crate::network::SegmentId), u32>::new();
    for t in patient {
        if t.last_leg() || t.offset < t.link_len {
            continue;
        }
        let node = end_node(net, t.link());
        let mut at = t.link_len.0;
        for s in t.departure(net).1 {
            if t.offset.0 == at {
                let w = longest.entry((node, s)).or_insert(0);
                *w = (*w).max(t.waited);
                break;
            }
            at = at.saturating_add(crossing::width(net, s));
        }
    }
    for (at, waited) in longest {
        g.claim(net, at, waited);
    }
    g
}

/// 待つ位置（[`line_of`]）に止まった車があるものとして近づき、その位置を越えない速さ。
fn stop_speed(net: &Network, t: &Trip) -> i64 {
    let at = line_of(net, t);
    let left = i64::from(at) - i64::from(t.offset.0);
    let line = LaneOffset(at.saturating_add(SPACING));
    safe_speed(t.offset, line, MmPerTick::ZERO).min(left)
}

/// 車 `t` が、待つ位置の手前で止まりきれないか。
fn committed(net: &Network, t: &Trip) -> bool {
    stop_speed(net, t) < i64::from(t.speed.0) - i64::from(CAR_DECEL)
}

/// 位置を求める前に、車線の先頭の車ごとに見ておくこと。
#[derive(Default)]
struct Look {
    /// リンクの先へ進む車が次に入る車線。リンクの終わりが止まれる距離より近いときだけ選ぶ。
    next_lane: Option<LaneId>,
    /// 交差点の先の車線に、交差点を抜けきるだけの空きができそうにないか。
    no_room: bool,
    /// 交差点の手前まで来た車。
    approach: Option<Approach>,
    /// 始まりの交差点の中にいる車。
    inside: Option<Inside>,
    /// 終わりの交差点の中にいる車。停止線を越えた車は決めた車線への動きを 1 つ持つ。停止線を越えていないのに
    /// 車道の四角にかかる車（1 マスのリンクで、手前の交差点を抜けた所で待つ車など）は、入りうる車線への動きを
    /// すべて持つ。
    inside_end: SmallVec<[Inside; 3]>,
}

/// 交差点の中の車が、車道の四角を抜けるまで `rest` を、いまの速さ `v` を落とさずに進めそうか。
/// `run` は、前の車の後ろや、待つかもしれない停止線など、いちばん近く止まりうる所までの距離である。
/// そこに止まった車があっても、四角を抜けるまで安全速度が `v` を下回らない距離があれば、進めるとみなす。
fn keeps_pace(rest: i64, v: i64, run: i64) -> bool {
    run >= rest.saturating_add(v).saturating_add(braking_distance(v))
}

/// 車 `order[k]` が、いまの位置から先で止まりうるいちばん近い所までの距離。前の車がいまから減速の上限で止まった
/// 所の後ろから車間を空けた所、交差点の停止線、着く位置、区間の途中の衝突区間（`zones`）のうち、いちばん近い所である。
/// 前の車は、安全速度（[`safe_speed`]）と同じく、減速の上限より速くは止まらないとみなす。
/// 通り抜けの区間に入った車は、次のリンクの決めた車線の前の車（ほかのリンクからその車線へ向かう車を含む）と、
/// 次のリンクの停止線か着く位置を見る。先頭の車で、リンクの終わりが交差点でなければ、次のリンクのどの車線の
/// 最後尾も前の車とみなす。
fn clear_run(
    rows: &[Option<Trip>],
    net: &Network,
    st: &LaneState,
    order: &[(LaneId, core::cmp::Reverse<LaneOffset>, usize)],
    k: usize,
    zones: &BTreeMap<LinkId, Vec<u32>>,
) -> i64 {
    let (lane, _, i) = order[k];
    let t = rows[i].as_ref().expect("車の主体がない");
    let p = i64::from(t.offset.0);
    let base = i64::from(t.link_len.0);
    let mut run = i64::MAX;
    let spacing = i64::from(SPACING);
    // 衝突区間の手前で止まる所。`from` はそのリンクの上の車の位置、`shift` はそのリンクの始まりの、いまのリンクの上の位置。
    let jay = |run: &mut i64, link: LinkId, from: i64, shift: i64| {
        for z in zones.get(&link).into_iter().flatten() {
            if i64::from(z.saturating_add(crossing::JAY_HALF_MM)) >= from {
                *run = (*run).min(shift + i64::from(jay_line(*z)) - p);
            }
        }
    };
    if let Some(to_lane) = bound_lane(net, t) {
        let next = t.route.links[t.leg as usize + 1];
        let at = p - base;
        if let Some((a, w)) = st.tails.get(&to_lane).and_then(|tl| tl.ahead_of(at)) {
            run = run.min(base + a + braking_distance(w.max(0)) - spacing - p);
        }
        if t.leg as usize + 2 == t.route.links.len() {
            run = run.min(base + i64::from(t.dest_offset.0) - p);
        } else if junction::is_junction(net, end_node(net, next)) {
            run = run.min(base + i64::from(stop_line(net, next).0) - p);
        }
        jay(&mut run, next, at, base);
        return run;
    }
    if let Some(a) = k
        .checked_sub(1)
        .map(|q| order[q])
        .filter(|q| q.0 == lane)
        .and_then(|q| rows[q.2].as_ref())
    {
        let stops = braking_distance(i64::from(a.speed.0.max(0)));
        run = run.min(i64::from(a.offset.0) + stops - spacing - p);
    } else if !t.last_leg() {
        if junction::is_junction(net, end_node(net, t.link())) {
            run = run.min(i64::from(line_of(net, t)) - p);
        } else {
            let next = t.route.links[t.leg as usize + 1];
            for l in net.link_lanes(next) {
                if let Some((a, w)) = st.last(l) {
                    let stops = braking_distance(i64::from(w.0.max(0)));
                    run = run.min(base + a + stops - spacing - p);
                }
            }
        }
    }
    if t.last_leg() {
        run = run.min(i64::from(t.dest_offset.0) - p);
    }
    jay(&mut run, t.link(), p, 0);
    run
}

/// `order[k]` の車について [`Look`] を作る。`order` は車線ごとに前から並べた車である。
#[allow(clippy::too_many_arguments)]
fn look(
    rows: &[Option<Trip>],
    net: &Network,
    conn: &[Connector],
    st: &LaneState,
    order: &[(LaneId, core::cmp::Reverse<LaneOffset>, usize)],
    k: usize,
    zones: &BTreeMap<LinkId, Vec<u32>>,
    rail: &RailCrossings,
) -> Look {
    let (lane, _, i) = order[k];
    let t = rows[i].as_ref().expect("車の主体がない");
    let mut out = Look::default();
    let v = i64::from(t.speed.0);
    let stopped = t.speed == MmPerTick::ZERO;
    let at = i64::from(t.offset.0);
    // 四角を抜けるまでの見込みに使う速さ。止まりうる所が近く、速さを落としうる車は、止まっているとみなして、
    // 候補に待たせる。出口が詰まって中の車が速さを落とすと、候補が入るまでに抜けるという見込みが外れるためである。
    let pace = |rest: i64, v: i64| {
        if keeps_pace(rest, v, clear_run(rows, net, st, order, k, zones)) {
            v
        } else {
            0
        }
    };
    // ほかの車と道筋が交わるのは車道の四角の中だけなので、ノードを越えた車は、四角を抜けるまで交差点の中にいる。
    if let Some((node, prev)) = inside_start(net, t) {
        let out_at = box_reach(net, node, t.link());
        if at < out_at {
            out.inside = Some(Inside {
                node,
                from: prev,
                to: t.link(),
                lane: t.lane,
                stopped,
                leaves: (out_at - at, pace(out_at - at, v)),
            });
        }
    }
    // 通り抜けの区間に入った車は、決めた車線へ進む。
    if let Some(to_lane) = bound_lane(net, t) {
        out.next_lane = Some(to_lane);
        let node = end_node(net, t.link());
        if junction::is_junction(net, node) {
            let next = t.route.links[t.leg as usize + 1];
            let rest = box_reach(net, node, next) - (at - i64::from(t.link_len.0));
            // ノードを越えると次のリンクの最高速度より速くは進まない。
            let next_kind = net.segment(next.segment).expect("リンクの区間がない").kind;
            let v = v.min(i64::from(speed_limit(next_kind).0));
            out.inside_end.push(Inside {
                node,
                from: t.link(),
                to: next,
                lane: to_lane,
                stopped,
                leaves: (rest, pace(rest, v)),
            });
        }
        return out;
    }
    // 停止線を越えていなくても、終わりの交差点の車道の四角にかかっていれば、その交差点の中にいる。
    // 停止線で止まりうるので、候補はこの車が抜けると見込まずに待つ。
    let end = (!t.last_leg())
        .then(|| end_node(net, t.link()))
        .filter(|n| junction::is_junction(net, *n))
        .filter(|n| at > i64::from(t.link_len.0) - box_reach(net, *n, t.link()));
    let in_box = |to_lane: Option<LaneId>, out: &mut Look| {
        let Some(node) = end else {
            return;
        };
        let next = t.route.links[t.leg as usize + 1];
        let rest = box_reach(net, node, next) + i64::from(t.link_len.0) - at;
        let lanes: SmallVec<[LaneId; 3]> = match to_lane {
            Some(l) => smallvec::smallvec![l],
            None => net.link_lanes(next).collect(),
        };
        out.inside_end.extend(lanes.into_iter().map(|l| Inside {
            node,
            from: t.link(),
            to: next,
            lane: l,
            stopped,
            leaves: (rest, 0),
        }));
    };
    // 後ろの車は前の車に追従するので、車線の先頭の車だけを見る。前の車が停止線を越えていれば、
    // 交差点に入るかを決めるのは後ろの車である。
    let head = k == 0
        || order[k - 1].0 != lane
        || rows[order[k - 1].2]
            .as_ref()
            .is_some_and(|a| a.bound.is_some());
    if !head || t.last_leg() || !near_end(net, t) {
        in_box(None, &mut out);
        return out;
    }
    let node = end_node(net, t.link());
    let next = t.route.links[t.leg as usize + 1];
    let after = t.route.links.get(t.leg as usize + 2).copied();
    if !junction::is_junction(net, node) {
        out.next_lane = Some(choose_lane(net, conn, st, t.lane, node, next, after));
        return out;
    }
    let (to_lane, room) = exit_lane(net, conn, st, t, node, next, after, rail);
    out.next_lane = Some(to_lane);
    in_box(Some(to_lane), &mut out);
    let seg = net.segment(t.link().segment).expect("リンクの区間がない");
    // 車道の四角に入るまでの距離と、この tick に出しうるいちばん速い速さ。
    let to_box = i64::from(t.link_len.0) - at - box_reach(net, node, t.link());
    let fastest = (v + i64::from(CAR_ACCEL)).min(i64::from(speed_limit(seg.kind).0));
    out.approach = Some(Approach {
        id: k,
        node,
        from: t.link(),
        to: next,
        lane,
        to_lane,
        class: junction::class(seg.kind),
        committed: committed(net, t),
        room,
        waited: t.waited,
        arrives: (to_box, fastest),
        in_box: end.is_some(),
    });
    out.no_room = !room;
    out
}

/// 交差点 `node` で `next` のリンクへ入る車 `t` が入る車線と、先の車線に交差点を抜けきるだけの空きが
/// ありそうか。先の車線の最後尾（停止線を越えてその車線へ向かう車を含む）がこの tick も同じ速さで進むとして決める。
/// ただし、最後尾がまだ交差点の中にいれば、`t` が交差点を抜けきる所に着くまで同じ速さで進むとして決める（[`roomy`]）。
/// 空きがなければ、車は停止線で止まる。
#[allow(clippy::too_many_arguments)]
fn exit_lane(
    net: &Network,
    conn: &[Connector],
    st: &LaneState,
    t: &Trip,
    node: NodeId,
    next: LinkId,
    after: Option<LinkId>,
    rail: &RailCrossings,
) -> (LaneId, bool) {
    let need = i64::from(room_needed(net, node, next));
    let ticks = ticks_to(
        net,
        t,
        i64::from(t.link_len.0) - i64::from(t.offset.0) + i64::from(clear_line(net, next).0),
    );
    let clear = i64::from(clear_line(net, next).0);
    let stop = i64::from(stop_line(net, next).0);
    let room = |l: &LaneId| {
        let fit = LaneFit {
            base: 1,
            back: 0,
            ticks,
            clear,
            stop,
            need,
        };
        fit.roomy(
            st.tails.get(l),
            crossing_wall(net, rail, next, st.tails.get(l), stop),
        )
    };
    let to_lane = junction_lane(net, conn, st, t, node, next, after, room);
    (to_lane, room(&to_lane))
}

/// 交差点の先のリンク `next` の、踏切の手前で止まった車とみなす位置（[`RailCrossings::closing_wall`]）。
/// 閉じたかもうすぐ閉じる踏切に加えて、開いていても先に車 1 台が抜けきる空きがない踏切の手前で止まる車も数える
/// （[`RailCrossings::no_room_zones`] と同じ決まり）。空きは、`tail`（`next` の車線の車）のうち踏切の向こうの
/// 端より先で最も後ろの車が減速の上限で止まった所から車間を空けた所までで、その車がいなければ停止線 `stop`
/// までで測る。踏切の手前で止まる車の後ろに並ぶ車が、交差点の四角の中に残らないようにするためである。
fn crossing_wall(
    net: &Network,
    rail: &RailCrossings,
    next: LinkId,
    tail: Option<&Tail>,
    stop: i64,
) -> Option<i64> {
    if rail.is_empty() {
        return None;
    }
    let len = super::pos::link_geometry(net, next)?.2.0;
    if let Some(w) = rail.closing_wall(next, len) {
        return Some(w);
    }
    rail.on_link(next, len).iter().find_map(|c| {
        let ahead = tail.and_then(|t| t.0.iter().find(|x| x.0 > i64::from(c.near)));
        let bound = match ahead {
            Some(&(a, v)) => a + braking_distance(v.max(0)) - i64::from(SPACING),
            None => stop,
        };
        (bound < i64::from(c.far) + i64::from(CAR_LENGTH / 2))
            .then(|| i64::from(jay_line(rail_crossing::zone_of(c.near))) + i64::from(SPACING))
    })
}

/// 交差点を抜けきる空きの見込み方（[`Tail::roomy`]）。`back` は、最後尾が交差点を抜けていく車かを、何 tick 分
/// 戻した位置で決めるか。
struct LaneFit {
    base: i64,
    back: i64,
    ticks: i64,
    clear: i64,
    stop: i64,
    need: i64,
}

impl LaneFit {
    /// 車線の車 `tail` に、出ていく腕の先の閉じたかもうすぐ閉じる踏切の手前で止まった車 `wall` を足して、入る車が
    /// 交差点を抜けきる空きがありそうか。踏切の手前で止まる車の後ろに並ぶ車が、交差点の四角の中に残らないように
    /// するためである。
    fn roomy(&self, tail: Option<&Tail>, wall: Option<i64>) -> bool {
        let walled;
        let tail = match wall {
            None => tail,
            Some(w) => {
                let mut cars = tail.map_or_else(Vec::new, |t| t.0.clone());
                let k = cars.partition_point(|c| c.0 <= w);
                cars.insert(k, (w, 0));
                walled = Tail(cars);
                Some(&walled)
            }
        };
        tail.is_none_or(|t| {
            let (at, v) = t.last();
            t.roomy(
                self.base,
                self.ticks,
                at - v * self.back < self.clear,
                self.stop,
                self.need,
            )
        })
    }
}

/// 車 `t` が、いまの速さから加速の上限で速さを上げていき、いまのリンクの最高速度で `d` だけ進むのに要る
/// いちばん少ない tick 数。少なくとも 1 で、`EXIT_TICKS` を超えない。
fn ticks_to(net: &Network, t: &Trip, d: i64) -> i64 {
    let seg = net.segment(t.link().segment).expect("リンクの区間がない");
    let vmax = i64::from(speed_limit(seg.kind).0);
    let (mut v, mut went, mut k) = (i64::from(t.speed.0), 0, 0);
    while k < EXIT_TICKS {
        v = (v + i64::from(CAR_ACCEL)).min(vmax);
        went += v;
        k += 1;
        if went >= d {
            break;
        }
    }
    k
}

/// 交差点を抜けきる所に着くまでの tick 数として見込む上限。先の車線の最後尾がこれより長く同じ速さで
/// 進むとは見込まない。
const EXIT_TICKS: i64 = 3;

/// 停止線までが、最高速度から止まるまでの距離と車間より近いか。
fn near_end(net: &Network, t: &Trip) -> bool {
    let seg = net.segment(t.link().segment).expect("リンクの区間がない");
    let vmax = i64::from(speed_limit(seg.kind).0);
    let left = i64::from(stop_line(net, t.link()).0) - i64::from(t.offset.0);
    left <= vmax + i64::from(CAR_ACCEL) + braking_distance(vmax) + i64::from(SPACING)
}

/// 信号のある交差点ごとの、軸の様子。交差点の手前まで来た車（[`near_end`]）と、そのうち止まりきれない車と、
/// 腕の横断歩道を渡り始めようと待つ人を、腕の軸ごとに集める。tick を進める前の行だけを読む。
pub(super) fn signal_demand(rows: &[Option<Trip>], net: &Network) -> signal::Demand {
    let mut d = signal::Demand::default();
    let signalled = |node: NodeId| {
        net.node(node)
            .is_some_and(|n| n.control == crate::network::NodeControl::Signal)
    };
    if !net
        .nodes()
        .any(|(_, n)| n.control == crate::network::NodeControl::Signal)
    {
        return d;
    }
    for t in rows.iter().flatten() {
        if t.last_leg() {
            continue;
        }
        if t.mode == LaneMode::Sidewalk {
            // 渡り始めるのを待つ人は、渡る所の始まりにいる（`gate` の長く待った人と同じ）。渡っている人は、
            // 渡る所の途中にいる。
            if t.walk.jay.is_some() || t.offset < t.link_len {
                continue;
            }
            let node = end_node(net, t.link());
            if !signalled(node) {
                continue;
            }
            let mut at = t.link_len.0;
            for s in t.departure(net).1 {
                let end = at.saturating_add(crossing::width(net, s));
                let waiting = t.offset.0 == at;
                let crossing = at < t.offset.0 && t.offset.0 < end;
                if waiting || crossing {
                    if let Some(l) = net.segment_line(s) {
                        let patient = waiting && t.waited >= crossing::PED_WAIT;
                        d.note(node, signal::axis(l.step), |a| {
                            a.walkers |= waiting;
                            a.crossing |= crossing;
                            a.patient |= patient;
                        });
                    }
                    break;
                }
                at = end;
            }
        } else if on_road(t) && t.bound.is_none() && near_end(net, t) {
            // 停止線を越えた車は、もう交差点の中にいて灯火に従わないので数えない。交わる向きの車は、
            // 信号が変わっても交差点の譲り合いでその車を待つ（`junction`）。
            let node = end_node(net, t.link());
            if !signalled(node) {
                continue;
            }
            let Some(l) = net.segment_line(t.link().segment) else {
                continue;
            };
            let committed = committed(net, t);
            d.note(node, signal::axis(l.step), |a| {
                a.cars = true;
                a.committed |= committed;
            });
        }
    }
    d
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

/// 車を tick `now` から 1 tick 進め、着いた主体の行の番号を返す。`rail` の踏切の先に抜けきる空きがなければ、手前で止まる。
#[allow(clippy::too_many_arguments)]
pub(super) fn step(
    rows: &mut [Option<Trip>],
    net: &Network,
    conn: &[Connector],
    rescues: &mut junction::Rescues,
    now: Tick,
    occupied: &crossing::Occupied,
    signals: &signal::Demand,
    rail: &RailCrossings,
) -> Vec<usize> {
    // 出発を待っている車を、主体の番号の順に置き直す。
    let waiting: Vec<usize> = (0..rows.len())
        .filter(|i| rows[*i].as_ref().is_some_and(|t| t.waiting))
        .collect();
    let mut still_waiting = Vec::new();
    if !waiting.is_empty() {
        let mut ix = LaneIndex::of(rows, net, None);
        let entering = Entering::of(rows, net, conn);
        for i in waiting {
            if !place(rows, net, &mut ix, i, &occupied.zones, rail, &entering) {
                still_waiting.push(i);
            }
        }
    }
    let st = LaneState::of(rows, net);
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
    let look_at = |k: usize| look(rows_ro, net, conn, &st, &order, k, &occupied.zones, rail);
    #[cfg(feature = "parallel")]
    let looks: Vec<Look> = (0..order.len())
        .into_par_iter()
        .with_min_len(super::PAR_MIN_CARS)
        .map(look_at)
        .collect();
    #[cfg(not(feature = "parallel"))]
    let looks: Vec<Look> = (0..order.len()).map(look_at).collect();
    // 交差点の手前で止める車を、前の tick の状態だけからノードの順に決める。
    let mut held = vec![false; order.len()];
    let mut yielded = vec![false; order.len()];
    let (cands, inside) = looks.iter().fold((Vec::new(), Vec::new()), |mut acc, l| {
        acc.0.extend(l.approach);
        acc.1.extend(l.inside);
        acc.1.extend(l.inside_end.iter().copied());
        acc
    });
    let stop = junction::holds(net, cands, inside, now, &occupied.crosswalks, signals);
    rescues.passed_stuck = rescues.passed_stuck.saturating_add(stop.passed);
    for k in stop.ids {
        held[k] = true;
    }
    for k in stop.yielded {
        yielded[k] = true;
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
        // 停止線をまだ越えていない車は、ほかのリンクから停止線を越えてその車線へ向かう車も前の車とみなす。
        let next_lane = looks[k].next_lane;
        let leader = ahead.or_else(|| {
            let lane = next_lane?;
            let (at, v) = if t.bound.is_some() {
                st.rear.get(&lane).map(|(a, v)| (i64::from(a.0), *v))?
            } else {
                st.last(lane)?
            };
            let at = (i64::from(t.link_len.0) + at).max(0);
            Some((
                LaneOffset(u32::try_from(at).expect("位置が u32 に収まらない")),
                v,
            ))
        });
        let leader = clear_of_crossing(net, rail, t, ahead.is_none(), leader);
        // いまのリンクと、ノードの先の次のリンクの衝突区間。次のリンクの位置は、いまのリンクの始まりから数え直す。
        let here = occupied.zones.get(&t.link()).map_or(&[][..], Vec::as_slice);
        let ahead = next_lane
            .and_then(|_| occupied.zones.get(&t.route.links[t.leg as usize + 1]))
            .filter(|zs| !zs.is_empty());
        let both: Vec<u32>;
        let jay = match ahead {
            None => here,
            Some(zs) => {
                both = here
                    .iter()
                    .copied()
                    .chain(zs.iter().map(|z| z.saturating_add(t.link_len.0)))
                    .collect();
                &both
            }
        };
        // 踏切の先に抜けきる空きがなければ、踏切の手前で止まる。
        let holds = if rail.is_empty() {
            SmallVec::new()
        } else {
            crossing_holds(net, rail, t, leader, next_lane.is_some())
        };
        let merged: Vec<u32>;
        let jay = if holds.is_empty() {
            jay
        } else {
            let mut v: Vec<u32> = jay.iter().copied().chain(holds).collect();
            v.sort_unstable();
            merged = v;
            &merged
        };
        let (v, at) = drive(net, t, leader, held[k] || looks[k].no_room, jay);
        (i, t.speed, t.offset, v, at, next_lane, held[k])
    };
    #[cfg(feature = "parallel")]
    let moves: Vec<_> = (0..order.len())
        .into_par_iter()
        .with_min_len(super::PAR_MIN_CARS)
        .map(plan)
        .collect();
    #[cfg(not(feature = "parallel"))]
    let moves: Vec<_> = (0..order.len()).map(plan).collect();

    // この tick の前の位置。位置が変わらなかった車を、あとで止まったままの車として数える。
    let was: Vec<(usize, u32, LaneOffset)> = order
        .iter()
        .map(|(_, at, i)| (*i, rows[*i].as_ref().expect("車の主体がない").leg, at.0))
        .collect();
    let mut arrived = Vec::new();
    let mut crossing = Vec::new();
    let mut entering = Vec::new();
    for (k, (i, before, from, v, at, next_lane, held)) in moves.into_iter().enumerate() {
        let t = rows[i].as_mut().expect("車の主体がない");
        (t.speed, t.offset) = (v, at);
        if held {
            t.waited = t.waited.saturating_add(1);
        }
        // 横断歩道の人のために止まった tick を数える。ほかの車や先の車線の空きを待つあいだは数えず、
        // それまでに人に譲った長さは、交差点の手前で止められているあいだ残す。人が渡り終えた直後に交差点の中の車を待つ車が、
        // 人に譲った長さを失って、また人に先を越されないようにするためである。
        if yielded[k] {
            t.yielded = t.yielded.saturating_add(1);
        } else if !held {
            t.yielded = 0;
        }
        if t.last_leg() {
            if at >= t.dest_offset {
                arrived.push(i);
            }
        } else if !held {
            let node = end_node(net, t.link());
            let line = stop_line(net, t.link()).0.max(from.0);
            if t.bound.is_none() && has_interior(net, node) && at.0 > line {
                // 車線を選んでいない車（車線の先頭でない車）は、停止線を越えない。
                match next_lane {
                    Some(lane) => entering.push((node, i, lane, before, line)),
                    None => (t.offset, t.speed) = (LaneOffset(line), MmPerTick::ZERO),
                }
            } else if at >= t.link_len {
                let lane = next_lane.expect("リンクの先の車線を選んでいない");
                crossing.push((node, i, lane));
            }
        }
    }
    // 通り抜けの区間に入った車に、次のリンクで入る車線を決める。交差点では、止まりきれる車は、その車線の最後尾
    // （通り抜けの区間に入ってその車線へ向かう車を含む）が同じ速さで進むとして、交差点を抜けきる所に着くまでに
    // 空きができそうになければ停止線で止める（[`exit_lane`] と同じ見込み）。ここで止めると減速の上限を破る車は、
    // 空きを見ずに入れる。同じ空きを何台もが取らないよう、入れた車をノードの順にその車線の最後尾に足していく。
    let mut last = BTreeMap::<LaneId, Tail>::new();
    for t in rows.iter().flatten().filter(|t| on_road(t)) {
        let at = i64::from(t.offset.min(t.link_len).0);
        Tail::add(&mut last, t.lane, at, t.speed);
        if let Some(l) = bound_lane(net, t) {
            let at = i64::from(t.offset.0) - i64::from(t.link_len.0);
            Tail::add(&mut last, l, at, t.speed);
        }
    }
    entering.sort_unstable();
    for (node, i, lane, before, line) in entering {
        let t = rows[i].as_mut().expect("車の主体がない");
        let next = t.route.links[t.leg as usize + 1];
        if before.0 <= CAR_DECEL {
            let need = i64::from(room_needed(net, node, next));
            let left = i64::from(t.link_len.0) - i64::from(t.offset.0)
                + i64::from(clear_line(net, next).0);
            let ticks = ticks_to(net, t, left);
            let clear = i64::from(clear_line(net, next).0);
            let stop = i64::from(stop_line(net, next).0);
            // 最後尾が交差点を抜けていく車かは、この tick に動く前の位置で決め、`exit_lane` と同じ見込みにする。
            let fit = LaneFit {
                base: 0,
                back: 1,
                ticks,
                clear,
                stop,
                need,
            };
            if !fit.roomy(
                last.get(&lane),
                crossing_wall(net, rail, next, last.get(&lane), stop),
            ) {
                (t.offset, t.speed) = (LaneOffset(line), MmPerTick::ZERO);
                continue;
            }
        }
        let index = net
            .link_lanes(next)
            .position(|l| l == lane)
            .expect("決めた車線が次のリンクにない");
        t.bound = Some(u8::try_from(index).expect("車線の番号が u8 に収まらない"));
        let at = i64::from(t.offset.0) - i64::from(t.link_len.0);
        Tail::add(&mut last, lane, at, t.speed);
        if t.offset >= t.link_len {
            crossing.push((node, i, lane));
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
    for (_, i, lane) in crossing {
        let t = rows[i].as_mut().expect("車の主体がない");
        // 交差点を抜けきるだけの空きは、停止線を越えるときに確かめてある。ここでは車間だけを保つ。
        let need = SPACING;
        let next = t.route.links[t.leg as usize + 1];
        let over = t.link_len.distance_to(t.offset);
        let next_len = super::pos::link_geometry(net, next)
            .expect("経路のリンクがない")
            .2;
        let mut place = LaneOffset::START.advance(over).min(next_len);
        let room = rear
            .get(&lane)
            .map(|r| (r.0 >= need).then(|| r.0 - SPACING));
        // 置き先までに、区間の途中を横切る人の衝突区間があれば、ノードを越えずにリンクの終わりで止まる。
        // 止まりきれない車の来る所では人は横切り始めないので（`gate`）、ここで止まるのは稀である。
        let jay_ahead = occupied
            .jay
            .get(&next)
            .is_some_and(|zs| zs.iter().any(|z| jay_line(*z) < place.0));
        let room = if jay_ahead { Some(None) } else { room };
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
        // 描くずれは、通り抜けの区間に入った所のずれから、リンクの始まりで移り始める。通り抜けの区間の曲線の
        // 始まりのずれを、ノードを越える前にも同じに求められるようにするためである（`Trip::car_path`）。
        // リンクの始まりから移り始めれば、次の停止線までに移り終えるので、次の曲線の始まりはこのリンクの車線の上にある。
        let shown = t.shift_at(net, stop_line(net, t.link()));
        t.leg += 1;
        (t.waited, t.yielded, t.bound) = (0, 0, None);
        t.enter(net, place, Some(lane));
        if let Some(b) = t.blend.as_mut() {
            (b.from, b.start) = (shown, 0);
        }
        let r = rear.entry(lane).or_insert(place);
        *r = (*r).min(place);
        if t.last_leg() && t.offset >= t.dest_offset {
            arrived.push(i);
        }
        #[cfg(test)]
        check_lane(rows, lane);
    }
    // 満車の車線に出られないまま待つ車の tick 数を数える。`RESCUE` を越えた車は、呼び出し側が歩きに切り替える。
    for i in still_waiting {
        let t = rows[i].as_mut().expect("車の主体がない");
        t.stalled = t.stalled.saturating_add(1);
    }
    rescue_stalled(rows, &was, &mut arrived, rescues);
    arrived
}

/// 位置の変わらなかった車の `stalled` を数え、`RESCUE` tick より長く止まったままの車を、行き先に
/// 着いたことにして `arrived` に足す。`was` は、この tick の前に車線の上にいた車と、その位置である。
/// 交差点の手前に限らず、前の車の後ろ、交差点でないノードの手前、行き止まりの折り返しなど、
/// どこで止まった車も同じに扱う。
fn rescue_stalled(
    rows: &mut [Option<Trip>],
    was: &[(usize, u32, LaneOffset)],
    arrived: &mut Vec<usize>,
    rescues: &mut junction::Rescues,
) {
    arrived.sort_unstable();
    let done = arrived.len();
    for &(i, leg, at) in was {
        if arrived[..done].binary_search(&i).is_ok() {
            continue;
        }
        let t = rows[i].as_mut().expect("車の主体がない");
        if (t.leg, t.offset) != (leg, at) {
            t.stalled = 0;
            continue;
        }
        t.stalled = t.stalled.saturating_add(1);
        if t.stalled > junction::RESCUE {
            rescues.teleported = rescues.teleported.saturating_add(1);
            arrived.push(i);
        }
    }
}
