//! 道路の種類を変えたときに、走っている車がおかしくならないことを確かめる。

use super::*;
use crate::command::RoadKind;
use crate::network::Line;
use crate::state_hash::state_hash_of;

fn road(n: &mut Network, from: (i64, i64), to: (i64, i64), kind: RoadKind) {
    let line = Line::between(TilePos::new(from.0, from.1), TilePos::new(to.0, to.1)).unwrap();
    n.build_road(&line, kind).unwrap();
}

fn at(x: i64, y: i64, step: (i8, i8)) -> SidewalkPos {
    SidewalkPos {
        tile: TilePos::new(x, y),
        step,
        along: LaneOffset(0),
    }
}

struct Cars {
    net: Network,
    router: AstarRouter,
    trips: Trips,
}

impl Cars {
    fn new(net: Network) -> Cars {
        Cars {
            router: AstarRouter::new(&net),
            net,
            trips: Trips::default(),
        }
    }

    fn drive(&mut self, from: SidewalkPos, to: SidewalkPos) -> TripId {
        self.trips
            .spawn_drives(&self.net, &mut self.router, &[(from, to)], Tick::ZERO)[0]
            .unwrap()
    }

    fn step(&mut self) -> Vec<TripId> {
        let ended = self.trips.step(&self.net, &mut self.router, Tick::ZERO);
        self.check();
        ended
    }

    /// `from` から `to` への直線が通る道路を `kind` に変え、`World` と同じく経路探索を作り直して探し直す。
    fn change(&mut self, from: (i64, i64), to: (i64, i64), kind: RoadKind) {
        let line = Line::between(TilePos::new(from.0, from.1), TilePos::new(to.0, to.1)).unwrap();
        let change = self.net.plan_road_kind(&line, kind).unwrap();
        self.net.change_road_kind(&change);
        assert!(self.router.sync(&self.net));
        let lost = self.trips.reroute(&self.net, &mut self.router);
        assert!(lost.is_empty(), "通れなくなった車がある {lost:?}");
        self.check();
    }

    /// どの車も、いまのリンクの車線の上にいて、同じ車線の車と重ならない。
    fn check(&self) {
        let mut by_lane: Vec<(LaneId, LaneOffset)> = Vec::new();
        for (id, t) in self.trips.iter() {
            let lane = self.net.lane(t.lane()).expect("車が消えた車線にいる");
            let link = t.link();
            assert_eq!(
                (lane.segment, lane.dir, lane.mode),
                (link.segment, link.dir, LaneMode::Car),
                "{id:?} がリンクの外の車線にいる"
            );
            if car::on_road(t) {
                by_lane.push((t.lane(), t.offset()));
            }
        }
        by_lane.sort();
        for w in by_lane.windows(2) {
            if w[0].0 == w[1].0 {
                assert!(w[1].1.0 - w[0].1.0 >= CAR_LENGTH + MIN_GAP, "{w:?}");
            }
        }
    }

    /// すべての車が着くまで進める。止まったまま救済された車がないことも確かめる。
    fn run_until_empty(&mut self, ticks: usize) {
        for _ in 0..ticks {
            self.step();
            if self.trips.is_empty() {
                break;
            }
        }
        assert!(
            self.trips.is_empty(),
            "着かない車が {} 台残った",
            self.trips.len()
        );
        let r = self.trips.rescues();
        assert_eq!(
            (r.teleported, r.walked_instead),
            (0, 0),
            "止まったままの車がいた"
        );
    }
}

/// 小さい道路を東西に 2 区間つなぎ、西の区間の上を両向きに車が走る。東の区間から西へ向かう車もいる。
fn two_ways() -> Cars {
    let mut n = Network::new();
    road(&mut n, (0, 0), (30, 0), RoadKind::Small);
    road(&mut n, (30, 0), (60, 0), RoadKind::Small);
    let mut c = Cars::new(n);
    for _ in 0..6 {
        c.drive(at(2, 0, (1, 0)), at(55, 0, (1, 0)));
    }
    for _ in 0..4 {
        c.drive(at(28, 0, (-1, 0)), at(3, 0, (-1, 0)));
        c.drive(at(50, 0, (-1, 0)), at(5, 0, (-1, 0)));
    }
    for _ in 0..8 {
        c.step();
    }
    c
}

#[test]
fn cars_keep_their_lane_and_arrive_after_the_road_is_upgraded() {
    let mut c = two_ways();
    let moving = c.trips.iter().filter(|(_, t)| car::on_road(t)).count();
    assert!(moving >= 8, "走っている車が少ない {moving}");
    let before: Vec<(TripId, SidewalkPos, usize)> = c
        .trips
        .iter()
        .filter(|(_, t)| car::on_road(t))
        .map(|(id, t)| {
            let index = c
                .net
                .link_lanes(t.link())
                .position(|l| l == t.lane())
                .unwrap();
            (id, t.position(), index)
        })
        .collect();
    c.change((0, 0), (30, 0), RoadKind::Large);
    // 位置と、リンクの中の車線の番号は変わらない。
    for (id, pos, index) in before {
        let t = c.trips.get(id).unwrap();
        assert!(car::on_road(t), "{id:?} が車線から外れた");
        assert_eq!(t.position(), pos, "{id:?} の位置が飛んだ");
        let now = c
            .net
            .link_lanes(t.link())
            .position(|l| l == t.lane())
            .unwrap();
        assert_eq!(now, index, "{id:?} の車線が変わった");
    }
    c.run_until_empty(400);
}

/// 大きい道路の 3 車線に並んで走る車がいるところで、小さい道路に下げる。
fn narrowed(check_spread: bool) -> u64 {
    let mut n = Network::new();
    road(&mut n, (0, 0), (40, 0), RoadKind::Large);
    road(&mut n, (40, 0), (70, 0), RoadKind::Large);
    let mut c = Cars::new(n);
    for _ in 0..12 {
        c.drive(at(5, 0, (1, 0)), at(65, 0, (1, 0)));
    }
    for _ in 0..6 {
        c.step();
    }
    if check_spread {
        let lanes: std::collections::BTreeSet<LaneId> = c
            .trips
            .iter()
            .filter(|(_, t)| car::on_road(t))
            .map(|(_, t)| t.lane())
            .collect();
        assert_eq!(lanes.len(), 3, "3 車線に並んでいない");
    }
    c.change((0, 0), (40, 0), RoadKind::Small);
    let h = state_hash_of(&c.trips);
    c.run_until_empty(600);
    h
}

#[test]
fn cars_merge_without_overlapping_after_the_road_is_narrowed() {
    let one = narrowed(true);
    #[cfg(feature = "parallel")]
    for threads in [1, 4] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        assert_eq!(pool.install(|| narrowed(false)), one);
    }
}
