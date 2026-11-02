use super::crossing::{self, JAY_HALF_MM, PED_PATIENCE};
use super::*;
use crate::command::{NodeId, RoadKind};
use crate::network::{Line, SegmentId};

fn road(n: &mut Network, from: (i64, i64), to: (i64, i64)) {
    let line = Line::between(TilePos::new(from.0, from.1), TilePos::new(to.0, to.1)).unwrap();
    n.build_road(&line, RoadKind::Small).unwrap();
}

fn at(x: i64, y: i64, step: (i8, i8), along: u32) -> SidewalkPos {
    SidewalkPos {
        tile: TilePos::new(x, y),
        step,
        along: LaneOffset(along),
    }
}

/// マス `a` から隣のマス `b` へ向かう `mode` のリンク。
fn link(n: &Network, a: (i64, i64), b: (i64, i64), mode: LaneMode) -> LinkId {
    let step = ((b.0 - a.0).signum() as i8, (b.1 - a.1).signum() as i8);
    at(a.0, a.1, step, 1).resolve_on(n, mode).unwrap().0
}

/// (10, 10) で十字に交わる網と、交差点のノード。
fn crossroads() -> (Network, NodeId) {
    let mut n = Network::new();
    road(&mut n, (0, 10), (20, 10));
    road(&mut n, (10, 0), (10, 20));
    let node = n.node_at(TilePos::new(10, 10)).unwrap();
    (n, node)
}

#[test]
fn walkers_cross_the_arms_on_the_shorter_way_round() {
    let (n, node) = crossroads();
    let c = (10, 10);
    let side = |a, b| link(&n, a, b, LaneMode::Sidewalk);
    let seg = |a, b| side(a, b).segment;
    let (w, e, north, s) = ((9, 10), (11, 10), (10, 9), (10, 11));
    // 西から東へまっすぐ。進む向きの右（南の側）を歩くので、南の腕を渡る。
    let (far, arms) = crossing::depart(&n, node, (side(w, c), false), side(c, e), None);
    assert_eq!((far, arms.as_slice()), (false, &[seg(c, s)][..]));
    // 左の側（北の側）を歩いてきたなら、北の腕を渡って北の側のまま進む。
    let (far, arms) = crossing::depart(&n, node, (side(w, c), true), side(c, e), None);
    assert_eq!((far, arms.as_slice()), (true, &[seg(c, north)][..]));
    // 南の側を歩いてきて南へ曲がるなら、どの腕も渡らない。
    let (far, arms) = crossing::depart(&n, node, (side(w, c), false), side(c, s), None);
    assert_eq!((far, arms.len()), (false, 0));
    // 南へ曲がって東の側に出るなら、南の腕を渡る。南へ進む向きの左は東である。
    let (far, arms) = crossing::depart(&n, node, (side(w, c), false), side(c, s), Some(true));
    assert_eq!((far, arms.as_slice()), (true, &[seg(c, s)][..]));
    // 折り返して反対の側へ出るなら、来た腕を渡る。
    let (far, arms) = crossing::depart(&n, node, (side(w, c), false), side(c, w), Some(false));
    assert_eq!((far, arms.as_slice()), (false, &[seg(w, c)][..]));
}

#[test]
fn no_crosswalks_where_fewer_than_three_roads_meet() {
    let mut n = Network::new();
    road(&mut n, (0, 0), (10, 0));
    road(&mut n, (10, 0), (10, 10));
    let node = n.node_at(TilePos::new(10, 0)).unwrap();
    let side = |a, b| link(&n, a, b, LaneMode::Sidewalk);
    for need in [None, Some(false), Some(true)] {
        let (_, arms) = crossing::depart(
            &n,
            node,
            (side((9, 0), (10, 0)), false),
            side((10, 0), (10, 1)),
            need,
        );
        assert!(arms.is_empty());
    }
}

struct Town {
    net: Network,
    router: AstarRouter,
    trips: Trips,
    now: Tick,
}

impl Town {
    fn new(net: Network) -> Town {
        let router = AstarRouter::new(&net);
        Town {
            net,
            router,
            trips: Trips::default(),
            now: Tick(0),
        }
    }

    fn walk(&mut self, from: SidewalkPos, to: SidewalkPos) -> TripId {
        self.trips
            .spawn(&self.net, &mut self.router, from, to, self.now)
            .unwrap()
    }

    fn drive(&mut self, from: SidewalkPos, to: SidewalkPos) -> TripId {
        self.trips
            .spawn_drives(&self.net, &mut self.router, &[(from, to)], self.now)[0]
            .unwrap()
    }

    /// 歩く人が渡っている横断歩道（交差点と腕の区間）。
    fn occupied(&self) -> Vec<(NodeId, SegmentId)> {
        let mut out = Vec::new();
        for (_, t) in self.trips.iter() {
            if t.mode != LaneMode::Sidewalk || t.last_leg() || t.offset <= t.link_len {
                continue;
            }
            let node = car::end_node(&self.net, t.link());
            let mut s = t.link_len.0;
            for arm in t.departure(&self.net).1 {
                let e = s + crossing::width(&self.net, arm);
                if s < t.offset.0 && t.offset.0 < e {
                    out.push((node, arm));
                }
                s = e;
            }
        }
        out
    }

    /// 1 tick 進め、交差点 `node` に入った車について、入ってくる腕か出ていく腕の横断歩道に人が
    /// いなかったことを確かめる。着いた主体を返す。
    fn step(&mut self, node: NodeId) -> Vec<TripId> {
        let before: Vec<(TripId, LinkId)> = self
            .trips
            .iter()
            .filter(|(_, t)| car::on_road(t) && car::end_node(&self.net, t.link()) == node)
            .map(|(id, t)| (id, t.link()))
            .collect();
        let arrived = self.trips.step(&self.net, &mut self.router, self.now);
        let walks = self.occupied();
        for (id, from) in before {
            let Some(t) = self.trips.get(id).filter(|t| t.mode == LaneMode::Car) else {
                continue;
            };
            if t.link() != from {
                for s in [from.segment, t.link().segment] {
                    assert!(
                        !walks.contains(&(node, s)),
                        "人のいる横断歩道を車が通った {:?}",
                        self.now
                    );
                }
            }
        }
        self.now = self.now.next();
        arrived
    }
}

#[test]
fn cars_wait_for_walkers_on_crosswalks_and_everyone_gets_through() {
    let (n, node) = crossroads();
    let mut t = Town::new(n);
    let mut walkers = 0;
    let mut cars = 0;
    for k in 0..40_u32 {
        // 4 つの腕の歩道を回る人と、交差点を直進する車を、少しずつずらして出す。
        let along = (k * 1_700) % 8_000;
        t.walk(at(4, 10, (1, 0), along), at(16, 10, (1, 0), along));
        t.walk(at(10, 4, (0, 1), along), at(10, 16, (0, 1), along));
        t.walk(at(16, 10, (-1, 0), along), at(4, 10, (-1, 0), along));
        walkers += 3;
        if k % 4 == 0 {
            t.drive(at(10, 1, (0, 1), 0), at(10, 19, (0, 1), 0));
            t.drive(at(1, 10, (1, 0), 0), at(19, 10, (1, 0), 0));
            cars += 2;
        }
        for _ in 0..3 {
            t.step(node);
        }
    }
    for _ in 0..1_500 {
        t.step(node);
    }
    assert!(t.trips.is_empty(), "残った {}", t.trips.len());
    assert_eq!(t.trips.rescues(), Default::default(), "救済が働いた");
    assert!(walkers > 0 && cars > 0);
}

#[test]
fn a_car_gets_through_a_steady_stream_of_walkers() {
    // 西の腕の南の歩道から東へ、南の腕の横断歩道を途切れずに渡る人の列があっても、北から来る車は
    // 待ちすぎずに入る。車は南の腕へ出ていく。
    let (n, node) = crossroads();
    let mut t = Town::new(n);
    for _ in 0..30 {
        for along in [0, 2_000, 4_000, 6_000] {
            t.walk(at(9, 10, (1, 0), along), at(18, 10, (1, 0), 0));
        }
        t.step(node);
    }
    let car = t.drive(at(10, 5, (0, 1), 0), at(10, 18, (0, 1), 0));
    let first = t.trips.get(car).unwrap().link();
    let mut entered = None;
    for tick in 0..200 {
        for along in [0, 2_000, 4_000, 6_000] {
            t.walk(at(9, 10, (1, 0), along), at(18, 10, (1, 0), 0));
        }
        t.step(node);
        if entered.is_none() && t.trips.get(car).is_none_or(|c| c.link() != first) {
            entered = Some(tick);
        }
    }
    let entered = entered.expect("車が入れない");
    assert!(
        entered < 60 + u64::from(PED_PATIENCE) as usize,
        "待ちすぎた {entered}"
    );
}

#[test]
fn walkers_obey_the_signal() {
    let (mut n, node) = crossroads();
    n.set_signal(node, true).unwrap();
    let mut t = Town::new(n);
    for k in 0..60_u32 {
        let along = (k * 1_300) % 8_000;
        t.walk(at(4, 10, (1, 0), along), at(16, 10, (1, 0), along));
        t.walk(at(10, 16, (0, -1), along), at(10, 4, (0, -1), along));
    }
    t.drive(at(10, 1, (0, 1), 0), at(10, 19, (0, 1), 0));
    t.drive(at(1, 10, (1, 0), 0), at(19, 10, (1, 0), 0));
    let mut crossed = 0;
    for _ in 0..1_500 {
        let before = t.occupied();
        let now = t.now;
        t.step(node);
        // この tick に渡り始めた横断歩道は、渡ってよい灯火だった。
        let st = signal::state(&t.net, node, now).unwrap();
        for (_, arm) in t.occupied().into_iter().filter(|o| !before.contains(o)) {
            let axis = signal::axis(t.net.segment_line(arm).unwrap().step);
            assert!(st.walk(axis), "渡ってはいけない灯火で渡り始めた {now:?}");
            crossed += 1;
        }
    }
    assert!(crossed > 0);
    assert!(t.trips.is_empty(), "残った {}", t.trips.len());
}

#[test]
fn a_walker_crossing_midblock_stops_the_car() {
    // 両端の行き止まりまで遠い長い道路の真ん中で、南の側の出入口から北の側の出入口へ横切る。
    // 大きい道路は 6 車線あり、横切るのに 13 tick かかる。
    let mut n = Network::new();
    let line = Line::between(TilePos::new(0, 0), TilePos::new(60, 0)).unwrap();
    n.build_road(&line, RoadKind::Large).unwrap();
    let mut t = Town::new(n);
    let node = t.net.node_at(TilePos::new(0, 0)).unwrap();
    let walker = t.walk(at(30, 0, (1, 0), 0), at(30, 0, (-1, 0), 7_000));
    assert_eq!(t.trips.get(walker).unwrap().walk.jay, Some(0));
    let car = t.drive(at(27, 0, (1, 0), 0), at(58, 0, (1, 0), 0));
    let car_link = link(&t.net, (29, 0), (30, 0), LaneMode::Car);
    let zone = 30 * 8_000_u32;
    let mut stopped = false;
    for _ in 0..200 {
        t.step(node);
        let crossing = t
            .trips
            .get(walker)
            .and_then(|w| w.walk.jay)
            .is_some_and(|p| p > 0);
        if let Some(c) = t
            .trips
            .get(car)
            .filter(|c| c.link() == car_link && crossing)
        {
            assert!(
                c.offset().0 + JAY_HALF_MM <= zone,
                "横切っている人に車が入った"
            );
            stopped |= c.speed() == MmPerTick::ZERO;
        }
    }
    assert!(stopped, "車が止まらない");
    assert!(t.trips.is_empty(), "残った {}", t.trips.len());
}

#[test]
fn a_nearby_crosswalk_is_used_instead_of_crossing_midblock() {
    // 交差点まで 2 マスなら、横切らずに交差点の横断歩道を使う。
    let (n, _) = crossroads();
    let mut t = Town::new(n);
    let walker = t.walk(at(8, 10, (1, 0), 0), at(8, 10, (-1, 0), 4_000));
    let w = t.trips.get(walker).unwrap();
    assert_eq!(w.walk.jay, None);
    assert!(w.route().links.len() >= 2);
}
