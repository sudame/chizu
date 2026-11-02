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

/// (0, 0)〜(20, 0) と (20, 0)〜(60, 0) の 2 本の区間がノード (20, 0) でつながる道路に、区間 2 の始まりから
/// `at` mm の所で北の側から横切る人を置く。人の行き先は同じリンクの先にあり、横切る予定だけを足す。
fn crossing_near_the_node(at: u32, started: u32) -> (Town, TripId) {
    let mut n = Network::new();
    road(&mut n, (0, 0), (20, 0));
    road(&mut n, (20, 0), (60, 0));
    let mut t = Town::new(n);
    let walker = t.walk(at_on(20, at), at_on(50, 0));
    let w = t.trips.rows[walker.slot()].as_mut().unwrap();
    assert_eq!(w.route().links.len(), 1);
    w.offset = LaneOffset(at);
    w.walk.jay = Some(started);
    (t, walker)
}

/// マス `x` から東へ `along` mm の、東向きの歩道の位置。
fn at_on(x: i64, along: u32) -> SidewalkPos {
    let tiles = i64::from(along / 8_000);
    at(x + tiles, 0, (1, 0), along % 8_000)
}

/// 区間の途中を横切っている人と、車の体が同じリンクの上で重ならないことを確かめる。
fn assert_apart(t: &Town, walker: TripId) {
    let Some(w) = t.trips.get(walker) else {
        return;
    };
    if !w.walk.jay.is_some_and(|p| p > 0) {
        return;
    }
    let z = w.offset.0;
    for (_, c) in t.trips.iter().filter(|(_, c)| car::on_road(c)) {
        if c.link().segment != w.link().segment {
            continue;
        }
        let len = w.link_len.0;
        let zone = if c.link().dir == w.link().dir {
            z
        } else {
            len - z
        };
        let front = c.offset().0;
        let apart = front + JAY_HALF_MM <= zone || front >= zone + JAY_HALF_MM + CAR_LENGTH;
        assert!(
            apart,
            "横切っている人に車が重なった（人 {zone}、車の前 {front}）"
        );
    }
}

/// 区間 2 の始まりから 4m の所で横切ろうとする人と、区間 1 をノードの `before` mm 手前で最高速度で
/// 走ってくる車を置いて、終わるまで進める。人と車は重ならない。人が車の過ぎる前に横切り始めたかを返す。
fn car_over_the_node(before: u32) -> bool {
    let (mut t, walker) = crossing_near_the_node(4_000, 0);
    let node = t.net.node_at(TilePos::new(20, 0)).unwrap();
    let car = t.drive(at_on(0, 160_000 - before), at(55, 0, (1, 0), 0));
    let first = t.trips.get(car).unwrap().link();
    assert_eq!(t.trips.get(car).unwrap().offset().0, 160_000 - before);
    let vmax = crate::routing::speed_limit(t.net.segment(first.segment).unwrap().kind);
    t.trips.rows[car.slot()].as_mut().unwrap().speed = vmax;
    let mut early = false;
    for _ in 0..200 {
        t.step(node);
        assert_apart(&t, walker);
        if t.trips.get(car).is_some_and(|c| c.link() == first) {
            early |= t.trips.get(walker).unwrap().walk.jay != Some(0);
        }
    }
    assert!(t.trips.is_empty(), "残った {}", t.trips.len());
    early
}

#[test]
fn a_car_coming_over_the_node_does_not_run_into_a_walker_crossing_near_it() {
    car_over_the_node(10_000);
    // 隣の区間で止まりきれない所まで来ている車がいるあいだは、人は横切り始めない。
    assert!(!car_over_the_node(18_000), "車が来ているのに横切り始めた");
    // 止まれる所にいる車の前では横切り始め、車は人の手前で止まる。
    assert!(car_over_the_node(40_000), "止まれる車の前でも横切らない");
}

#[test]
fn a_car_arriving_while_a_walker_already_crosses_near_the_node_waits_at_the_node() {
    // 人が横切り始めたあとで、止まりきれない速さの車がノードの手前に現れる。車はノードを越えずに待つ。
    let (mut t, walker) = crossing_near_the_node(4_000, 1_389);
    let node = t.net.node_at(TilePos::new(20, 0)).unwrap();
    let car = t.drive(at(19, 0, (1, 0), 4_000), at(55, 0, (1, 0), 0));
    let vmax = crate::routing::speed_limit(
        t.net
            .segment(t.trips.get(car).unwrap().link().segment)
            .unwrap()
            .kind,
    );
    t.trips.rows[car.slot()].as_mut().unwrap().speed = vmax;
    for _ in 0..200 {
        t.step(node);
        assert_apart(&t, walker);
    }
    assert!(t.trips.is_empty(), "残った {}", t.trips.len());
}

#[test]
fn a_car_departing_next_to_a_crossing_walker_is_placed_clear_of_it() {
    // 区間 2 の 20m の所を横切っている人のすぐそばから、車が出発する。
    let (mut t, walker) = crossing_near_the_node(20_000, 1_389);
    let node = t.net.node_at(TilePos::new(20, 0)).unwrap();
    for along in [18_000, 20_000, 22_000, 25_000] {
        t.drive(at_on(20, along), at(55, 0, (1, 0), 0));
        assert_apart(&t, walker);
    }
    // 次の tick に置かれる、出発を待つ車も重ならない。
    for _ in 0..200 {
        t.step(node);
        assert_apart(&t, walker);
        let x = at_on(20, 20_000);
        t.drive(x, at(55, 0, (1, 0), 0));
        assert_apart(&t, walker);
        if t.trips.get(walker).is_none() {
            break;
        }
    }
}

#[test]
fn walkers_go_round_highway_arms_instead_of_crossing_them() {
    // (10, 10) で、東西の道路に北から高速道路がつながる。高速道路の腕には横断歩道がない。
    let mut n = Network::new();
    n.add_highway(TilePos::new(10, 10), TilePos::new(10, -20));
    road(&mut n, (0, 10), (20, 10));
    let node = n.node_at(TilePos::new(10, 10)).unwrap();
    assert_eq!(n.node(node).unwrap().segments.len(), 3);
    let highway = n
        .node(node)
        .unwrap()
        .segments
        .iter()
        .copied()
        .find(|s| n.segment(*s).unwrap().kind == crate::network::SegmentKind::Highway)
        .unwrap();
    let side = |a, b| link(&n, a, b, LaneMode::Sidewalk);
    let (w, c, e) = ((9, 10), (10, 10), (11, 10));
    for from_far in [false, true] {
        for need in [None, Some(false), Some(true)] {
            let (_, arms) = crossing::depart(&n, node, (side(w, c), from_far), side(c, e), need);
            assert!(!arms.contains(&highway), "{from_far} {need:?} {arms:?}");
        }
    }
    // 北の側（東へ歩く人の左）から北の側へ行くなら、高速道路の腕を越えずに、南へ回って西と東の腕を渡る。
    let (far, arms) = crossing::depart(&n, node, (side(w, c), true), side(c, e), Some(true));
    assert!(far);
    assert_eq!(arms.as_slice(), &[side(w, c).segment, side(c, e).segment]);
}

/// 北から南へ交差点を直進する車を `every` tick ごとに出し、西の腕の南の歩道から東へ歩いて南の腕の横断歩道を
/// 渡る人を毎 tick 出して、`ticks` tick 進める。人と車は重ならないことを確かめる。
/// 車と人それぞれの、着いた数と、着くまでにかかったいちばん長い tick 数を返す。終わりに残ったものは、
/// 終わりまでの tick 数で数える。
fn cars_and_walkers(every: u64, ticks: u64) -> ([usize; 2], [u64; 2]) {
    let (n, node) = crossroads();
    let mut t = Town::new(n);
    let mut out: BTreeMap<TripId, (usize, u64)> = BTreeMap::new();
    let (mut gone, mut longest) = ([0; 2], [0; 2]);
    for k in 0..ticks {
        if k % every == 0 {
            out.insert(t.drive(at(10, 1, (0, 1), 0), at(10, 19, (0, 1), 0)), (0, k));
        }
        out.insert(t.walk(at(9, 10, (1, 0), 0), at(18, 10, (1, 0), 0)), (1, k));
        for id in t.step(node) {
            // 車を降りる主体はないので、消えた主体は着いた主体である。
            if let Some((kind, from)) = out.remove(&id) {
                gone[kind] += 1;
                longest[kind] = longest[kind].max(k - from);
            }
        }
    }
    for (kind, from) in out.into_values() {
        longest[kind] = longest[kind].max(ticks - from);
    }
    (gone, longest)
}

#[test]
fn a_walker_gets_across_a_steady_stream_of_cars() {
    // 車が 2 tick ごとに途切れずに来て、止まりきれない車か交差点の中の車がいつもいる。それでも、歩く人は
    // 待ちすぎた所で車を止めて渡る。
    let (n, node) = crossroads();
    let mut t = Town::new(n);
    let mut walker = None;
    let mut gone = None;
    for k in 0..600_u64 {
        if k % 2 == 0 {
            t.drive(at(10, 1, (0, 1), 0), at(10, 19, (0, 1), 0));
        }
        if k == 100 {
            walker = Some(t.walk(at(9, 10, (1, 0), 0), at(18, 10, (1, 0), 0)));
        }
        t.step(node);
        if gone.is_none() && walker.is_some_and(|w| t.trips.get(w).is_none()) {
            gone = Some(k);
        }
    }
    // 歩くのに約 60 tick かかる。待つのは、待ちすぎるまでと、交差点の車が抜けるまでである。
    let gone = gone.expect("歩く人が渡れない");
    assert!(
        gone < 100 + 60 + u64::from(crossing::PED_WAIT) + 20,
        "待ちすぎた {gone}"
    );
}

#[test]
fn walkers_and_cars_take_turns_when_neither_stops_coming() {
    // 人も車も途切れずに来る交差点で、どちらも待ち続けず、車は 1 回に何台かまとめて通る。
    let (every, ticks) = (3, 2_000);
    let ([cars, walkers], [car, walker]) = cars_and_walkers(every, ticks);
    // 人が渡り終えるたびに 1 台しか通れなければ、車の列が伸び続け、着く車が出した車の 9 割に届かない。
    assert!(
        cars as u64 > ticks / every * 9 / 10,
        "着いた車が少ない {cars}"
    );
    assert!(
        walkers as u64 > ticks * 9 / 10,
        "着いた人が少ない {walkers}"
    );
    // 走るのに約 20 tick、歩くのに約 60 tick かかる。待つのは、交互に回る 1 周ぶんほどにとどまる。
    assert!(car < 20 + 60, "車が待ちすぎた {car}");
    assert!(walker < 60 + 80, "人が待ちすぎた {walker}");
}

#[test]
fn a_car_stuck_behind_a_blocked_exit_does_not_keep_walkers_waiting() {
    // 南の腕の出口に動かない車があり、北から来た車が交差点の手前で先の車線の空きを待ち続ける。
    // この車は横断歩道の人に譲って待っているのではないので、人は待ちすぎた所で渡る。
    let (n, node) = crossroads();
    let mut t = Town::new(n);
    let block = t.drive(at(10, 11, (0, 1), 2_000), at(10, 19, (0, 1), 0));
    let stuck = t.drive(at(10, 2, (0, 1), 0), at(10, 19, (0, 1), 0));
    let hold = |t: &mut Town| {
        // 交差点を抜けた所より先にいるが、後ろの車が交差点を抜けきるだけの空きはない。
        let b = t.trips.rows[block.slot()].as_mut().unwrap();
        (b.offset, b.speed, b.stalled) = (LaneOffset(10_000), MmPerTick::ZERO, 0);
    };
    hold(&mut t);
    let mut walker = None;
    let mut gone = None;
    for k in 0..300_u64 {
        if k == 100 {
            walker = Some(t.walk(at(9, 10, (1, 0), 0), at(18, 10, (1, 0), 0)));
        }
        t.step(node);
        hold(&mut t);
        if walker.is_some_and(|w| t.trips.get(w).is_none()) {
            gone = Some(k);
            break;
        }
    }
    let gone = gone.expect("歩く人が渡れない");
    let c = t.trips.get(stuck).expect("待っている車がない");
    assert!(
        c.speed() == MmPerTick::ZERO && c.waited > 100,
        "車が空きを待っていない"
    );
    // 歩くのに約 60 tick かかる。待つのは、待ちすぎるまでである。
    assert!(
        gone < 100 + 60 + u64::from(crossing::PED_WAIT) + 20,
        "待ちすぎた {gone}"
    );
}

#[test]
fn a_walker_crossing_midblock_gets_through_a_creeping_queue() {
    // 長い道路の真ん中で横切る人の前を、1 tick に 1m ずつ進む先頭の車に続いて、車の列が隙間なく這う。
    // 人は待ちすぎた所で、車に前を空けさせて渡る。
    let mut n = Network::new();
    let line = Line::between(TilePos::new(0, 0), TilePos::new(60, 0)).unwrap();
    n.build_road(&line, RoadKind::Small).unwrap();
    let mut t = Town::new(n);
    let node = t.net.node_at(TilePos::new(0, 0)).unwrap();
    let lead = t.drive(at(32, 0, (1, 0), 0), at(59, 0, (1, 0), 0));
    let creep = |t: &mut Town, k: u32| {
        let c = t.trips.rows[lead.slot()].as_mut().unwrap();
        (c.offset, c.speed, c.stalled) = (LaneOffset(256_000 + k * 1_000), MmPerTick(1_000), 0);
    };
    creep(&mut t, 0);
    let mut walker = None;
    let mut gone = None;
    for k in 0..200_u32 {
        if k % 3 == 0 {
            t.drive(at(1, 0, (1, 0), 0), at(58, 0, (1, 0), 0));
        }
        if k == 60 {
            let w = t.walk(at(30, 0, (1, 0), 0), at(30, 0, (-1, 0), 7_000));
            assert_eq!(t.trips.get(w).unwrap().walk.jay, Some(0));
            walker = Some(w);
        }
        t.step(node);
        creep(&mut t, k + 1);
        if let Some(w) = walker {
            assert_apart(&t, w);
            if gone.is_none() && t.trips.get(w).is_none() {
                gone = Some(k);
            }
        }
    }
    // 横切るのに 5 tick かかる。待つのは、待ちすぎるまでと、前にいた車が過ぎるまでである。
    let gone = gone.expect("歩く人が横切れない");
    assert!(gone < 60 + crossing::PED_WAIT + 30, "待ちすぎた {gone}");
}
