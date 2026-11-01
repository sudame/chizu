//! 移動の主体（`Trips`）と、歩道を歩く移動体の細かい計算。
//!
//! 移動の主体は、経路と、いまのリンクの上の位置（リンクの始まりからの距離）を持つ。
//! 1 tick に移動の 1 秒ぶん進み、行き先の出入口に着いたら消える。歩行者どうしはぶつからない。

mod pos;
#[cfg(test)]
mod tests;

use std::sync::Arc;

#[cfg(feature = "parallel")]
use rayon::prelude::*;

pub use pos::SidewalkPos;

use crate::id::{Slots, TripId};
use crate::network::{LaneMode, LinkId, Network};
use crate::routing::{
    AstarRouter, Route, RouteRequest, Router, Scratch, Toward, WALK_SPEED, one_to_many,
};
use crate::state_hash::{StateHash, StateHasher};
use crate::tile::TilePos;
use crate::units::{LaneOffset, Tick, WorldPos};

/// 歩いて移動している主体。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trip {
    route: Arc<Route>,
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
            self.enter(net, LaneOffset::START.advance(rest));
        }
    }

    fn enter(&mut self, net: &Network, offset: LaneOffset) {
        let (start, step, len) = pos::link_geometry(net, self.link()).expect("経路のリンクがない");
        (self.start, self.step, self.link_len, self.offset) = (start, step, len, offset);
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
}

/// 歩道のリンクと、その始まりからの距離。
type Spot = (LinkId, LaneOffset);

/// 出発の位置と行き先の位置。
type Ends = (Spot, Spot);

/// 歩道の上の 2 点を結ぶ経路を、まとめて探す。行き先が同じリンクの手前にあるときは、
/// 次のリンクへ出て戻ってくる経路にする。
fn plan(router: &mut AstarRouter, ends: &[Ends]) -> Vec<Option<Arc<Route>>> {
    let behind = |((a, ao), (b, bo)): &Ends| a == b && bo < ao;
    let reqs: Vec<RouteRequest> = ends
        .iter()
        .filter(|e| !behind(e))
        .map(|((a, _), (b, _))| RouteRequest {
            from: *a,
            to: *b,
            mode: LaneMode::Sidewalk,
        })
        .collect();
    let mut found = router.route_batch(&reqs).into_iter();
    let g = router.graph(LaneMode::Sidewalk);
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

    /// 歩く主体をまとめて作り、依頼の順に ID を返す。経路はまとめて探す。
    pub fn spawn_many(
        &mut self,
        net: &Network,
        router: &mut AstarRouter,
        walks: &[(SidewalkPos, SidewalkPos)],
        now: Tick,
    ) -> Vec<Option<TripId>> {
        let resolved: Vec<Option<Ends>> = walks
            .iter()
            .map(|(a, b)| Some((a.resolve(net)?, b.resolve(net)?)))
            .collect();
        let ends: Vec<Ends> = resolved.iter().flatten().copied().collect();
        let mut routes = plan(router, &ends).into_iter();
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

    /// 1 tick 進め、着いた主体を消して、その ID を返す。
    pub fn step(&mut self, net: &Network) -> Vec<TripId> {
        let advance = |(i, t): (usize, &mut Option<Trip>)| t.as_mut()?.advance(net).then_some(i);
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
    pub fn reroute(&mut self, net: &Network, router: &mut AstarRouter) -> Vec<TripId> {
        let broken: Vec<(TripId, SidewalkPos, SidewalkPos)> = self
            .iter()
            .filter(|(_, t)| {
                t.route.links[t.leg as usize..]
                    .iter()
                    .any(|l| net.segment(l.segment).is_none())
            })
            .map(|(id, t)| (id, t.position(), t.dest))
            .collect();
        let mut lost = Vec::new();
        let mut found = Vec::new();
        for (id, at, dest) in broken {
            match (at.resolve(net), dest.resolve(net)) {
                (Some(a), Some(b)) => found.push((id, (a, b), dest)),
                _ => lost.push(id),
            }
        }
        let ends: Vec<_> = found.iter().map(|f| f.1).collect();
        for ((id, ends, dest), route) in found.into_iter().zip(plan(router, &ends)) {
            match route {
                Some(route) => {
                    let departed = self.get(id).expect("探し直す主体がない").departed;
                    self.rows[id.slot()] = Some(make_trip(net, route, ends, dest, departed));
                }
                None => lost.push(id),
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
    let mut t = Trip {
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
    t.enter(net, from);
    t
}

impl StateHash for Trip {
    fn write_state(&self, h: &mut StateHasher) {
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
