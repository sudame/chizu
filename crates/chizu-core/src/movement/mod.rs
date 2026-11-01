//! 移動の主体（`Trips`）と、歩道を歩く人と車線を走る車の細かい計算。
//!
//! 移動の主体は、経路と、いまのリンクの上の位置（リンクの始まりからの距離）を持つ。
//! 1 tick に移動の 1 秒ぶん進み、行き先に着いたら消える。歩行者どうしはぶつからない。
//! 車は同じ車線の前の車との車間で速さを決め、ノードで次のリンクの車線へ移る。
//! 建物から建物へ車で向かう移動は、歩く → 車 → 歩く の 3 つの区切りを順に進む（`parking`）。

mod car;
#[cfg(test)]
mod car_tests;
mod commute;
mod parking;
#[cfg(test)]
mod parking_tests;
mod pos;
#[cfg(test)]
mod tests;

use std::sync::Arc;

#[cfg(feature = "parallel")]
use rayon::prelude::*;

pub use car::{CAR_ACCEL, CAR_DECEL, CAR_LENGTH, MIN_GAP};
pub use commute::{CommuteRequest, Commuter, DRIVE_MIN_MM};
pub use parking::{Lot, PARK_RADIUS, building_spot};
pub use pos::SidewalkPos;

use crate::id::{Slots, TripId};
use crate::network::{Dir, LaneId, LaneMode, LinkId, Network};
use crate::routing::{
    AstarRouter, Connector, Route, RouteRequest, Router, Scratch, Toward, WALK_SPEED, one_to_many,
};
use crate::state_hash::{StateHash, StateHasher};
use crate::tile::TilePos;
use crate::units::{LaneOffset, MmPerTick, Tick, WorldPos};

/// 歩くか車で走って移動している主体。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trip {
    mode: LaneMode,
    route: Arc<Route>,
    /// いまいる車線。歩くときは歩道の車線である。
    lane: LaneId,
    /// 車の速さ。歩くときは使わない。
    speed: MmPerTick,
    /// 出発の位置の車線に空きがなく、車線の上に出るのを待っている車か。
    waiting: bool,
    /// いま歩いているリンクの、経路の中での番号。
    leg: u32,
    offset: LaneOffset,
    /// いまのリンクの長さ。
    link_len: LaneOffset,
    /// いまのリンクの始まりのマスと、1 歩の向き。網が変わっても位置を求め直せるように持つ。
    start: TilePos,
    step: (i8, i8),
    /// 行き先の出入口と、経路の最後のリンクの上でのその位置。
    dest: SidewalkPos,
    dest_offset: LaneOffset,
    /// いまの区切りが終わったあとにすること。
    then: parking::Then,
    departed: Tick,
}

impl Trip {
    pub fn route(&self) -> &Route {
        &self.route
    }

    pub fn mode(&self) -> LaneMode {
        self.mode
    }

    pub fn lane(&self) -> LaneId {
        self.lane
    }

    pub fn speed(&self) -> MmPerTick {
        self.speed
    }

    pub fn link(&self) -> LinkId {
        self.route.links[self.leg as usize]
    }

    /// いまのリンクの始まりからの距離。
    pub fn offset(&self) -> LaneOffset {
        self.offset
    }

    pub fn departed(&self) -> Tick {
        self.departed
    }

    /// いまの位置。
    pub fn position(&self) -> SidewalkPos {
        SidewalkPos::along(self.start, self.step, self.offset)
    }

    fn last_leg(&self) -> bool {
        self.leg as usize + 1 == self.route.links.len()
    }

    /// 1 tick 進める。着いたら `true`。
    fn advance(&mut self, net: &Network) -> bool {
        self.offset = self.offset.advance(WALK_SPEED.distance(1));
        loop {
            if self.last_leg() {
                return self.offset >= self.dest_offset;
            }
            if self.offset < self.link_len {
                return false;
            }
            let rest = self.link_len.distance_to(self.offset);
            self.leg += 1;
            self.enter(net, LaneOffset::START.advance(rest), None);
        }
    }

    /// いまの経路の番号のリンクに入る。車線を指定しなければ、リンクの最初の車線に入る。
    fn enter(&mut self, net: &Network, offset: LaneOffset, lane: Option<LaneId>) {
        let link = self.link();
        let (start, step, len) = pos::link_geometry(net, link).expect("経路のリンクがない");
        let lane = lane.unwrap_or_else(|| net.link_lanes(link).next().expect("リンクに車線がない"));
        (self.start, self.step, self.link_len, self.offset, self.lane) =
            (start, step, len, offset, lane);
    }
}

/// 移動の主体の表。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Trips {
    slots: Slots,
    rows: Vec<Option<Trip>>,
    parking: parking::Parking,
    commuters: commute::Commuters,
}

/// 画面に描く移動体。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mover {
    pub id: TripId,
    pub pos: WorldPos,
    /// 進む向き。x と y はそれぞれ -1、0、1 のどれかである。
    pub heading: (i8, i8),
    /// 歩いているなら `Sidewalk`、車なら `Car`。
    pub mode: LaneMode,
}

/// リンクと、その始まりからの距離。
type Spot = (LinkId, LaneOffset);

/// 出発の位置と行き先の位置。
type Ends = (Spot, Spot);

/// 歩道の位置 `(link, offset)` を、同じ区間の逆向きの歩道のリンクの上で表したもの。
fn flip(net: &Network, (link, offset): Spot) -> Option<Spot> {
    let back = LinkId {
        dir: match link.dir {
            Dir::Forward => Dir::Backward,
            Dir::Backward => Dir::Forward,
        },
        ..link
    };
    let len = pos::link_geometry(net, back)?.2;
    net.link_lanes(back).next()?;
    Some((back, LaneOffset(len.0.checked_sub(offset.0)?)))
}

/// 経路 `route` で `from` から `to` まで歩く距離。
fn walked_mm(net: &Network, route: &Route, (from, to): Ends) -> Option<u64> {
    let len = |l: &LinkId| pos::link_geometry(net, *l).map(|g| u64::from(g.2.0));
    let total = route
        .links
        .iter()
        .try_fold(0_u64, |acc, l| acc.checked_add(len(l)?))?;
    let rest = len(route.links.last()?)?.checked_sub(u64::from(to.1.0))?;
    total.checked_sub(u64::from(from.1.0))?.checked_sub(rest)
}

/// 手段 `mode` で 2 点を結ぶ経路を、まとめて探し、経路と、使った出発と行き先の位置を返す。
///
/// 歩道は 1 本の歩道を 2 つの向きに歩けるものとし、区間の 2 本の歩道のリンクを、その 2 つの向きとして使う。
/// 歩くときは、出発と行き先のそれぞれで 2 つの向きを試し、歩く距離のいちばん短い組を選ぶ。
/// 距離の同じ組は、位置を表したままの向きを先に選ぶ。同じリンクの手前へ戻る組は、向きを入れ替えた
/// まっすぐの組に必ず負けるので、ほかの組があれば探さない。
/// 横切りの罰則と衝突区間が入るまでは、反対側の歩道へ移る費用を 0 として扱う。
fn plan(
    router: &mut AstarRouter,
    net: &Network,
    mode: LaneMode,
    ends: &[Ends],
) -> Vec<Option<(Arc<Route>, Ends)>> {
    let candidates: Vec<Vec<Ends>> = ends
        .iter()
        .map(|(a, b)| {
            if mode == LaneMode::Car {
                return vec![(*a, *b)];
            }
            let starts = [Some(*a), flip(net, *a)];
            let goals = [Some(*b), flip(net, *b)];
            let all: Vec<Ends> = starts
                .iter()
                .flatten()
                .flat_map(|s| goals.iter().flatten().map(|g| (*s, *g)))
                .collect();
            let ahead: Vec<Ends> = all.iter().filter(|e| !behind(e)).copied().collect();
            if ahead.is_empty() { all } else { ahead }
        })
        .collect();
    let flat: Vec<Ends> = candidates.iter().flatten().copied().collect();
    let mut found = plan_exact(router, mode, &flat).into_iter();
    candidates
        .iter()
        .map(|c| {
            let routes: Vec<_> = c
                .iter()
                .map(|e| Some((found.next().expect("探した経路の数が合わない")?, *e)))
                .collect();
            if mode == LaneMode::Car {
                return routes.into_iter().next().flatten();
            }
            routes
                .into_iter()
                .flatten()
                .filter_map(|(r, e)| Some((walked_mm(net, &r, e)?, r, e)))
                .min_by_key(|(mm, _, _)| *mm)
                .map(|(_, r, e)| (r, e))
        })
        .collect()
}

/// 行き先が出発と同じリンクの、出発より手前にあるか。
fn behind(((a, ao), (b, bo)): &Ends) -> bool {
    a == b && bo < ao
}

/// 手段 `mode` で 2 点を結ぶ経路を、まとめて探す。行き先が同じリンクの手前にあるときは、
/// 次のリンクへ出て戻ってくる経路にする。
fn plan_exact(router: &mut AstarRouter, mode: LaneMode, ends: &[Ends]) -> Vec<Option<Arc<Route>>> {
    let reqs: Vec<RouteRequest> = ends
        .iter()
        .filter(|e| !behind(e))
        .map(|((a, _), (b, _))| RouteRequest {
            from: *a,
            to: *b,
            mode,
        })
        .collect();
    let mut found = router.route_batch(&reqs).into_iter();
    let g = router.graph(mode);
    let mut s = Scratch::default();
    ends.iter()
        .map(|e| {
            if !behind(e) {
                return found.next().expect("探した経路の数が合わない");
            }
            let link = e.0.0;
            let v = g.vertex(link)?;
            let next: Vec<LinkId> = g.edges(v).iter().map(|x| g.link(x.to)).collect();
            let back = one_to_many(g, link, &next, Toward::Backward, &mut s);
            // 費用の同じ戻り道は、先に並ぶ（番号の小さい）リンクを選ぶ。
            let best = back.into_iter().flatten().min_by_key(|r| r.cost)?;
            let mut links = vec![link];
            links.extend(best.links);
            Some(Arc::new(Route {
                links,
                cost: best.cost.checked_add(g.link_cost(link)?)?,
            }))
        })
        .collect()
}

impl Trips {
    pub fn len(&self) -> usize {
        self.slots.live_count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, id: TripId) -> Option<&Trip> {
        self.slots
            .contains(id.index, id.generation)
            .then(|| self.rows[id.slot()].as_ref())
            .flatten()
    }

    /// 移動の主体を ID の順に返す。
    pub fn iter(&self) -> impl Iterator<Item = (TripId, &Trip)> {
        self.rows.iter().enumerate().filter_map(|(i, t)| {
            let (index, generation) = self.slots.current(i)?;
            Some((TripId { index, generation }, t.as_ref()?))
        })
    }

    fn insert(&mut self, trip: Trip) -> TripId {
        let ((index, generation), new) = self.slots.alloc();
        if new {
            self.rows.push(Some(trip));
        } else {
            self.rows[index as usize] = Some(trip);
        }
        TripId { index, generation }
    }

    /// `from` から `to` へ歩く主体を作る。出入口が歩道の上にないか、つながっていなければ `None`。
    pub fn spawn(
        &mut self,
        net: &Network,
        router: &mut AstarRouter,
        from: SidewalkPos,
        to: SidewalkPos,
        now: Tick,
    ) -> Option<TripId> {
        self.spawn_many(net, router, &[(from, to)], now)
            .pop()
            .flatten()
    }

    /// 歩く主体をまとめて作り、依頼の順に ID を返す。経路はまとめて探す。経路探索が古い網のままなら、
    /// 先に作り直す。
    pub fn spawn_many(
        &mut self,
        net: &Network,
        router: &mut AstarRouter,
        walks: &[(SidewalkPos, SidewalkPos)],
        now: Tick,
    ) -> Vec<Option<TripId>> {
        self.spawn_mode(net, router, LaneMode::Sidewalk, walks, now)
    }

    /// 車で走る主体をまとめて作り、依頼の順に ID を返す。位置は、同じ区間の同じ向きの車のリンクの上に置く。
    /// 車は止まった状態で、リンクの最初の車線から走り出す。
    pub fn spawn_drives(
        &mut self,
        net: &Network,
        router: &mut AstarRouter,
        drives: &[(SidewalkPos, SidewalkPos)],
        now: Tick,
    ) -> Vec<Option<TripId>> {
        self.spawn_mode(net, router, LaneMode::Car, drives, now)
    }

    fn spawn_mode(
        &mut self,
        net: &Network,
        router: &mut AstarRouter,
        mode: LaneMode,
        walks: &[(SidewalkPos, SidewalkPos)],
        now: Tick,
    ) -> Vec<Option<TripId>> {
        router.sync(net);
        let resolved: Vec<Option<Ends>> = walks
            .iter()
            .map(|(a, b)| Some((a.resolve_on(net, mode)?, b.resolve_on(net, mode)?)))
            .collect();
        let ends: Vec<Ends> = resolved.iter().flatten().copied().collect();
        let mut routes = plan(router, net, mode, &ends).into_iter();
        // 車は、出発のリンクの車線のうち車の少ない車線に置く。前の車と重なるなら、その後ろの空いた
        // 位置まで下げる。どの車線にも空きがなければ、空くまで待たせる。
        let mut ix = car::LaneIndex::of(&self.rows, None);
        resolved
            .iter()
            .zip(walks)
            .map(|(e, (_, to))| {
                (*e)?;
                let (route, e) = routes.next().expect("探した経路の数が合わない")?;
                let id = self.insert(make_trip(net, route, e, *to, now));
                if mode == LaneMode::Car {
                    car::place(&mut self.rows, net, &mut ix, id.slot());
                }
                Some(id)
            })
            .collect()
    }

    /// 1 tick 進め、移動を終えた主体を消して、その ID の順に返す。区切りの終わりに着いた主体は、
    /// その場で次の区切りの経路を探して進む。経路が見つからなかった主体も消して返す。
    /// `router` は `net` に合わせてあること。
    pub fn step(&mut self, net: &Network, router: &mut AstarRouter) -> Vec<TripId> {
        let done = self.advance(net, router.connectors());
        let mut ended = self.next_legs(net, router, done);
        ended.sort_unstable();
        ended
            .into_iter()
            .map(|i| {
                let (index, generation) = self.slots.current(i).expect("着いた主体がない");
                let id = TripId { index, generation };
                self.remove(id);
                id
            })
            .collect()
    }

    /// 歩く人と車を 1 tick 進め、区切りの終わりに着いた行を番号の順に返す。
    fn advance(&mut self, net: &Network, connectors: &[Connector]) -> Vec<usize> {
        let advance = |(i, t): (usize, &mut Option<Trip>)| {
            let t = t.as_mut()?;
            (t.mode == LaneMode::Sidewalk && t.advance(net)).then_some(i)
        };
        #[cfg(feature = "parallel")]
        let arrived: Vec<usize> = self
            .rows
            .par_iter_mut()
            .enumerate()
            .filter_map(advance)
            .collect();
        #[cfg(not(feature = "parallel"))]
        let arrived: Vec<usize> = self
            .rows
            .iter_mut()
            .enumerate()
            .filter_map(advance)
            .collect();
        let mut arrived = arrived;
        arrived.extend(car::step(&mut self.rows, net, connectors));
        arrived.sort_unstable();
        arrived
    }

    /// 網が変わったあとに呼ぶ。残りの経路に消えたリンクがある主体を、いまの位置から探し直す。
    /// いまの位置か行き先が歩道の上になくなったか、つながらなくなった主体は消し、その ID を返す。
    /// 経路探索が古い網のままなら、先に作り直す。
    pub fn reroute(&mut self, net: &Network, router: &mut AstarRouter) -> Vec<TripId> {
        router.sync(net);
        let broken: Vec<(TripId, LaneMode, SidewalkPos, SidewalkPos)> = self
            .iter()
            .filter(|(_, t)| {
                t.route.links[t.leg as usize..]
                    .iter()
                    .any(|l| net.segment(l.segment).is_none())
            })
            .map(|(id, t)| (id, t.mode, t.position(), t.dest))
            .collect();
        let mut lost = Vec::new();
        for mode in [LaneMode::Car, LaneMode::Sidewalk] {
            let mut found = Vec::new();
            for (id, _, at, dest) in broken.iter().filter(|b| b.1 == mode) {
                match (at.resolve_on(net, mode), dest.resolve_on(net, mode)) {
                    (Some(a), Some(b)) => found.push((*id, (a, b), *dest)),
                    _ => lost.push(*id),
                }
            }
            let ends: Vec<_> = found.iter().map(|f| f.1).collect();
            for ((id, _, dest), route) in found.into_iter().zip(plan(router, net, mode, &ends)) {
                let Some((route, ends)) = route else {
                    lost.push(id);
                    continue;
                };
                let old = self.get(id).expect("探し直す主体がない");
                let (departed, speed, waiting, then) =
                    (old.departed, old.speed, old.waiting, old.then);
                // 探し直す前の、リンクの中の車線の番号。網が変わると車線の ID も変わるので番号で覚える。
                let index = net
                    .link_lanes(old.link())
                    .position(|l| l == old.lane)
                    .unwrap_or(0);
                let mut t = make_trip(net, route, ends, dest, departed);
                (t.speed, t.waiting, t.then) = (speed, waiting, then);
                self.rows[id.slot()] = Some(t);
                if mode == LaneMode::Car && !waiting {
                    car::keep_lane(&mut self.rows, net, id.slot(), index);
                }
            }
        }
        lost.sort();
        for id in &lost {
            self.remove(*id);
        }
        lost
    }

    /// ワールド座標の範囲 `[min, max)` にいる移動体を ID の順に返す。
    pub fn movers_in(&self, min: WorldPos, max: WorldPos) -> Vec<Mover> {
        self.iter()
            .filter(|(_, t)| !t.waiting)
            .filter_map(|(id, t)| {
                let pos = t.position().world();
                let inside = (min.x..max.x).contains(&pos.x) && (min.y..max.y).contains(&pos.y);
                inside.then_some(Mover {
                    id,
                    pos,
                    heading: t.step,
                    mode: t.mode,
                })
            })
            .collect()
    }
}

fn make_trip(
    net: &Network,
    route: Arc<Route>,
    ((_, from), (_, to)): Ends,
    dest: SidewalkPos,
    now: Tick,
) -> Trip {
    let mode = route.links[0].mode;
    let first = route.links[0];
    let mut t = Trip {
        mode,
        lane: net.link_lanes(first).next().expect("リンクに車線がない"),
        speed: MmPerTick::ZERO,
        waiting: false,
        route,
        leg: 0,
        offset: from,
        link_len: LaneOffset::START,
        start: TilePos::default(),
        step: (0, 0),
        dest,
        dest_offset: to,
        then: parking::Then::Arrive,
        departed: now,
    };
    t.enter(net, from, None);
    t
}

impl StateHash for Trip {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&(self.mode as u8));
        h.write(&self.lane);
        h.write(&self.speed);
        h.write(&self.waiting);
        h.write_len(self.route.links.len());
        for l in &self.route.links {
            h.write(&l.segment);
            h.write(&(l.dir as u8));
            h.write(&(l.mode as u8));
        }
        h.write(&self.route.cost);
        h.write(&self.leg);
        h.write(&self.offset);
        h.write(&self.link_len);
        h.write(&(self.start.x, self.start.y));
        h.write(&self.step);
        h.write(&self.dest);
        h.write(&self.dest_offset);
        h.write(&self.then);
        h.write(&self.departed);
    }
}

impl StateHash for Trips {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.slots);
        h.write(&self.rows);
        h.write(&self.parking);
        h.write(&self.commuters);
    }
}
