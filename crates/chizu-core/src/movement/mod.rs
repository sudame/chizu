//! 移動の主体（`Trips`）と、歩道を歩く人と車線を走る車の細かい計算。
//!
//! 移動の主体は、経路と、いまのリンクの上の位置（リンクの始まりからの距離）を持つ。
//! 1 tick に移動の 1 秒ぶん進み、行き先に着いたら消える。歩行者どうしはぶつからない。
//! 車は同じ車線の前の車との車間で速さを決め、ノードで次のリンクの車線へ移る。

mod car;
#[cfg(test)]
mod car_tests;
mod pos;
#[cfg(test)]
mod tests;

use std::sync::Arc;

#[cfg(feature = "parallel")]
use rayon::prelude::*;

pub use car::{CAR_ACCEL, CAR_LENGTH, MIN_GAP};
pub use pos::SidewalkPos;

use crate::id::{Slots, TripId};
use crate::network::{LaneId, LaneMode, LinkId, Network};
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

/// 手段 `mode` で 2 点を結ぶ経路を、まとめて探す。行き先が同じリンクの手前にあるときは、
/// 次のリンクへ出て戻ってくる経路にする。
fn plan(router: &mut AstarRouter, mode: LaneMode, ends: &[Ends]) -> Vec<Option<Arc<Route>>> {
    let behind = |((a, ao), (b, bo)): &Ends| a == b && bo < ao;
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

    fn remove(&mut self, id: TripId) {
        if self.slots.release(id.index, id.generation) {
            self.rows[id.slot()] = None;
        }
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
        let mut routes = plan(router, mode, &ends).into_iter();
        resolved
            .iter()
            .zip(walks)
            .map(|(e, (_, to))| {
                let e = (*e)?;
                let route = routes.next().expect("探した経路の数が合わない")?;
                Some(self.insert(make_trip(net, route, e, *to, now)))
            })
            .collect()
    }

    /// 1 tick 進め、着いた主体を消して、その ID の順に返す。`connectors` はノードでの車線のつながりで、
    /// 車が次のリンクの車線を選ぶのに使う。
    pub fn step(&mut self, net: &Network, connectors: &[Connector]) -> Vec<TripId> {
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
            .into_iter()
            .map(|i| {
                let (index, generation) = self.slots.current(i).expect("着いた主体がない");
                let id = TripId { index, generation };
                self.remove(id);
                id
            })
            .collect()
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
            for ((id, ends, dest), route) in found.into_iter().zip(plan(router, mode, &ends)) {
                let Some(route) = route else {
                    lost.push(id);
                    continue;
                };
                let old = self.get(id).expect("探し直す主体がない");
                let (departed, speed) = (old.departed, old.speed);
                let mut t = make_trip(net, route, ends, dest, departed);
                t.speed = speed;
                self.rows[id.slot()] = Some(t);
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
        route,
        leg: 0,
        offset: from,
        link_len: LaneOffset::START,
        start: TilePos::default(),
        step: (0, 0),
        dest,
        dest_offset: to,
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
        h.write(&self.departed);
    }
}

impl StateHash for Trips {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.slots);
        h.write(&self.rows);
    }
}
