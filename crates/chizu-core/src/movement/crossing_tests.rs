use super::crossing::{self, JAY_HALF_MM, PED_PATIENCE};
use super::*;
use crate::command::{Heading, NodeId, RoadKind};
use crate::network::{Line, SegmentId};
use crate::tile::TileRect;
use crate::world::DenyReason;

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
    let (far, arms) = crossing::depart(&n, node, (side(w, c), false), side(c, e), None, None);
    assert_eq!((far, arms.as_slice()), (false, &[seg(c, s)][..]));
    // 左の側（北の側）を歩いてきたなら、北の腕を渡って北の側のまま進む。
    let (far, arms) = crossing::depart(&n, node, (side(w, c), true), side(c, e), None, None);
    assert_eq!((far, arms.as_slice()), (true, &[seg(c, north)][..]));
    // 南の側を歩いてきて南へ曲がるなら、どの腕も渡らない。
    let (far, arms) = crossing::depart(&n, node, (side(w, c), false), side(c, s), None, None);
    assert_eq!((far, arms.len()), (false, 0));
    // 南へ曲がって東の側に出るなら、南の腕を渡る。南へ進む向きの左は東である。
    let (far, arms) = crossing::depart(&n, node, (side(w, c), false), side(c, s), Some(true), None);
    assert_eq!((far, arms.as_slice()), (true, &[seg(c, s)][..]));
    // 折り返して反対の側へ出るなら、来た腕を渡る。
    let (far, arms) =
        crossing::depart(&n, node, (side(w, c), false), side(c, w), Some(false), None);
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
            None,
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
fn a_car_waiting_for_walkers_keeps_its_turn_while_the_crossing_flow_keeps_coming() {
    // 北から南へ抜ける車 c が、出ていく南の腕の横断歩道を渡る人 P に譲って待つ。c と交わる西から東への
    // 直進は途切れずに来る。c は人に譲るあいだ道筋を押さえないので、西からの車が先に入り、人が渡り終えた
    // 直後も、その車が交差点の中にいる。c が待ち始めたあとに横断歩道の前に来た人 Q たちは、c より短く
    // 待っている。c は人に譲った長さを失わず、Q たちより先に入る。
    // P を放す tick と、西からの車の来る周期との組み合わせを変えて確かめる。
    for extra in 0..8 {
        keeps_its_turn(u64::from(crossing::PED_WAIT) + 40 + extra);
    }
}

/// [`a_car_waiting_for_walkers_keeps_its_turn_while_the_crossing_flow_keeps_coming`] の場面で、P を `hold` tick
/// 止めておく。
fn keeps_its_turn(hold: u64) {
    let (n, node) = crossroads();
    let mut t = Town::new(n);
    let south = link(&t.net, (10, 10), (10, 11), LaneMode::Car).segment;
    let west_cars = |t: &mut Town, k: u64| {
        if k % 2 == 0 {
            t.drive(at(1, 10, (1, 0), 0), at(19, 10, (1, 0), 0));
        }
    };
    // 渡り始めたか、渡り終えた人の数。
    let started = |t: &Town, qs: &[TripId]| {
        qs.iter()
            .filter(|q| {
                t.trips
                    .get(**q)
                    .is_none_or(|q| q.leg > 0 || q.offset > q.link_len)
            })
            .count()
    };
    let p = t.walk(at(9, 10, (1, 0), 0), at(18, 10, (1, 0), 0));
    let mut k = 0;
    // P が横断歩道のなかほどに来るまで進める。
    let mid = loop {
        west_cars(&mut t, k);
        t.step(node);
        k += 1;
        let w = t.trips.get(p).expect("P が渡りきった");
        let into = w.offset.0.saturating_sub(w.link_len.0);
        if into >= crossing::width(&t.net, south) / 2 {
            break w.offset;
        }
    };
    // P をなかほどで止めておくあいだに、c が来て P に譲り、Q たちが横断歩道の前に並ぶ。P は PED_WAIT より
    // 長く横断歩道にいるので、Q たちのうち先に来た人は長く待った人になる。
    let c = t.drive(at(10, 5, (0, 1), 0), at(10, 18, (0, 1), 0));
    let mut qs = Vec::new();
    for j in 0..hold {
        west_cars(&mut t, k);
        if j >= 20 {
            qs.push(t.walk(at(9, 10, (1, 0), 0), at(18, 10, (1, 0), 0)));
        }
        t.step(node);
        k += 1;
        t.trips.rows[p.slot()].as_mut().unwrap().offset = mid;
        assert_eq!(
            started(&t, &qs),
            0,
            "{hold}: c より短く待った人が先に渡り始めた {j}"
        );
    }
    let car = t.trips.get(c).expect("c がない");
    assert!(
        car.link().segment != south && car.yielded > 0,
        "c が P に譲っていない"
    );
    let waited = qs
        .iter()
        .filter_map(|q| t.trips.get(*q))
        .map(|q| q.waited)
        .max()
        .unwrap_or(0);
    assert!(
        waited >= crossing::PED_WAIT,
        "長く待った人がいない {waited}"
    );
    // P を放す。c が入るまで、Q たちは渡り始めない。
    let mut entered = None;
    for j in 0..u64::from(junction::RESCUE) {
        west_cars(&mut t, k);
        t.step(node);
        k += 1;
        if t.trips.get(c).is_none_or(|c| c.link().segment == south) {
            entered = Some(j);
            break;
        }
        assert_eq!(
            started(&t, &qs),
            0,
            "{hold}: c より短く待った人が先に渡り始めた {j}"
        );
    }
    let entered = entered.expect("c が入れない");
    // P が渡りきるのと、交差点の中の車が抜けるのを待つだけである。
    assert!(entered < 10, "{hold}: c が待ちすぎた {entered}");
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
        assert_eq!(t.trips.lights().now, now);
        let st = t.trips.lights().state(&t.net, node).unwrap();
        for (_, arm) in t.occupied().into_iter().filter(|o| !before.contains(o)) {
            let axis = signal::axis(t.net.segment_line(arm).unwrap().step);
            assert!(st.walk(axis), "渡ってはいけない灯火で渡り始めた {now:?}");
            crossed += 1;
        }
    }
    assert!(crossed > 0);
    assert!(t.trips.is_empty(), "残った {}", t.trips.len());
}

/// 信号の十字路で、x の向きが青になった tick。
fn x_green_start(n: &Network, node: NodeId) -> u64 {
    let light = |t| {
        signal::state(n, node, Tick(t), &signal::Demand::default())
            .unwrap()
            .light_for(0)
    };
    (1..1_000)
        .find(|t| light(*t) == signal::Light::Green && light(*t - 1) != signal::Light::Green)
        .unwrap()
}

#[test]
fn a_turning_car_on_green_goes_before_walkers_who_come_after_it() {
    // 信号の十字路で、x の向きが青のあいだ、西の腕の南の歩道から東へ、南の腕の横断歩道を渡る人が
    // 途切れずに来る。東から来て南へ左折する車は、人が渡っていない所へ着けば、あとから来た人より先に入る。
    // 信号のない交差点で手前まで来た車を先に通すのと同じである。
    let (mut n, node) = crossroads();
    n.set_signal(node, true).unwrap();
    let start = x_green_start(&n, node);
    let mut t = Town::new(n);
    t.now = Tick(start);
    let car = t.drive(at(16, 10, (-1, 0), 0), at(10, 18, (0, 1), 0));
    let first = t.trips.get(car).unwrap().link();
    let mut entered = None;
    for tick in 0..signal::GREEN {
        t.walk(at(9, 10, (1, 0), 4_000), at(18, 10, (1, 0), 0));
        t.step(node);
        if t.trips.get(car).is_none_or(|c| c.link() != first) {
            entered = Some(tick);
            break;
        }
    }
    let entered = entered.expect("青のうちに曲がれない");
    assert!(
        entered < u64::from(PED_PATIENCE),
        "あとから来た人を待った {entered}"
    );
    // 人は、車を先に通したあとで渡る。
    for _ in 0..1_500 {
        t.step(node);
    }
    assert!(t.trips.is_empty(), "残った {}", t.trips.len());
}

/// 信号の十字路で、東から来て南へ左折する車を `every` tick ごとに出し、`start` tick 目に、西の腕の南の歩道から
/// 東へ、南の腕の横断歩道を渡る人を 1 人出す。その人が着くまでの tick 数を返す。`limit` tick で着かなければ `None`。
fn walker_among_turning_cars(every: u64, start: u64, limit: u64) -> Option<u64> {
    let (mut n, node) = crossroads();
    n.set_signal(node, true).unwrap();
    let mut t = Town::new(n);
    let mut walker = None;
    for k in 0..start + limit {
        if k % every == 0 {
            t.drive(at(16, 10, (-1, 0), 0), at(10, 18, (0, 1), 0));
        }
        if k == start {
            walker = Some(t.walk(at(9, 10, (1, 0), 0), at(18, 10, (1, 0), 0)));
        }
        t.step(node);
        if walker.is_some_and(|w| t.trips.get(w).is_none()) {
            return Some(k - start);
        }
    }
    None
}

#[test]
fn a_walker_gets_across_turning_cars_at_a_signal() {
    // 信号の十字路で、青の向きから曲がる車が途切れずに来る。人は曲がる車を先に通すが、待ちすぎた所で
    // 止まりきれる車を止めて渡る。信号のある交差点でも、人が待ち続けることはない。
    let cycle = 2 * signal::PHASE;
    for every in 1..=3 {
        for start in (100..100 + cycle).step_by(7) {
            // 歩くのに約 60 tick かかる。待つのは、渡ってよい窓が来るまでの 1 周期と、待ちすぎるまでである。
            let gone = walker_among_turning_cars(every, start, 1_200)
                .unwrap_or_else(|| panic!("歩く人が渡れない every={every} start={start}"));
            assert!(
                gone < 60 + cycle + u64::from(crossing::PED_WAIT),
                "待ちすぎた every={every} start={start} {gone}"
            );
        }
    }
}

#[test]
fn a_lone_walker_at_a_signal_does_not_wait_for_the_cycle() {
    // 車の来ない信号の十字路で、南の腕（y の向き）の横断歩道を渡る人。y の向きが青の段階に着いても、
    // 人の求めで x の向きに青が移るので、骨組みの周期を待たない。待つのは、角に着いた tick だけである。
    let cycle = 2 * signal::PHASE;
    for start in (100..100 + cycle).step_by(5) {
        let (mut n, node) = crossroads();
        n.set_signal(node, true).unwrap();
        let mut t = Town::new(n);
        let mut walker = None;
        let mut waited = 0;
        for k in 0..start + 200 {
            if k == start {
                walker = Some(t.walk(at(9, 10, (1, 0), 0), at(18, 10, (1, 0), 0)));
            }
            t.step(node);
            let Some(w) = walker else { continue };
            match t.trips.get(w) {
                Some(trip) => waited = waited.max(trip.waited),
                None => break,
            }
        }
        assert!(
            walker.is_some_and(|w| t.trips.get(w).is_none()),
            "着かない {start}"
        );
        assert!(waited <= 2, "空いた信号で待った start={start} {waited}");
    }
}

#[test]
fn a_walker_at_a_signal_gets_across_a_steady_stream_of_cars_on_its_arm() {
    // 信号の十字路で、北から南へ直進する車が 2 tick ごとに来て、y の向きの車が途切れない。南の腕を渡る人は、
    // 信号のない交差点と同じく来た車を先に通し、待ちすぎた所で x の向きの青を引き止めて渡る。
    let (mut n, node) = crossroads();
    n.set_signal(node, true).unwrap();
    let mut t = Town::new(n);
    let mut walker = None;
    let mut gone = None;
    for k in 0..800_u64 {
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
    // 歩くのに約 60 tick かかる。待つのは、待ちすぎるまでと、そのあと x の向きの段階が来るまでの 1 周期である。
    let gone = gone.expect("歩く人が渡れない");
    let cycle = 2 * signal::PHASE;
    assert!(
        gone < 100 + 60 + u64::from(crossing::PED_WAIT) + cycle + 20,
        "待ちすぎた {gone}"
    );
}

#[test]
fn walkers_do_not_wait_for_cars_queued_at_a_red_light() {
    // 信号の十字路で、x の向きの直進の車が青を使い続け、北から来る車は赤で停止線に並ぶ。北の腕の横断歩道を
    // 東から西へ渡る人は、並んだ車を待たずに渡り始める。赤の車は横断歩道を通らないからである。
    let (mut n, node) = crossroads();
    n.set_signal(node, true).unwrap();
    let start = x_green_start(&n, node);
    let mut t = Town::new(n);
    t.now = Tick(start);
    let north = link(&t.net, (10, 9), (10, 10), LaneMode::Car);
    let mut walker = None;
    let mut started = None;
    for k in 0..60_u64 {
        if k % 2 == 0 && k < 40 {
            t.drive(at(1, 10, (1, 0), 0), at(19, 10, (1, 0), 0));
        }
        if k < 6 && k % 2 == 0 {
            t.drive(at(10, 5, (0, 1), 0), at(10, 19, (0, 1), 0));
        }
        if k == 8 {
            walker = Some(t.walk(at(11, 10, (-1, 0), 0), at(2, 10, (-1, 0), 0)));
        }
        t.step(node);
        let Some(w) = walker.and_then(|w| t.trips.get(w)) else {
            continue;
        };
        assert_eq!(w.waited, 0, "人が待った {k}");
        if w.leg == 0 && w.offset > w.link_len {
            let queued = t
                .trips
                .iter()
                .filter(|(_, c)| {
                    c.mode == LaneMode::Car && c.link() == north && c.speed() == MmPerTick::ZERO
                })
                .count();
            assert!(queued >= 2, "赤で並んだ車がいない {queued}");
            started = Some(k);
            break;
        }
    }
    started.expect("人が渡り始めない");
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
            let (_, arms) =
                crossing::depart(&n, node, (side(w, c), from_far), side(c, e), need, None);
            assert!(!arms.contains(&highway), "{from_far} {need:?} {arms:?}");
        }
    }
    // 北の側（東へ歩く人の左）から北の側へ行くなら、高速道路の腕を越えずに、南へ回って西と東の腕を渡る。
    let (far, arms) = crossing::depart(&n, node, (side(w, c), true), side(c, e), Some(true), None);
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
    // 車が 2 tick ごとに来て、交差点の手前まで来た車が途切れない。待ちすぎていない人は車を先に通すので、
    // 人が渡れるのは、待ちすぎた所で止まりきれる車を止め、止まりきれない車が抜けたときだけである。
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

/// 南の腕の出口に動かない車を置き、`from` から南の腕へ入る車に、交差点の手前で先の車線の空きを待たせる。
/// 100 tick 目に、西の腕の南の歩道から東へ、南の腕の横断歩道を渡る人を出し、その人が着いた tick と、
/// 渡り始めるのを待った tick 数を返す。空きを待つ車が、人が着くまで動かなかったことも確かめる。
fn walker_past_a_blocked_exit(signalled: bool, from: SidewalkPos) -> (u64, u32) {
    let (mut n, node) = crossroads();
    if signalled {
        n.set_signal(node, true).unwrap();
    }
    let mut t = Town::new(n);
    let block = t.drive(at(10, 11, (0, 1), 2_000), at(10, 19, (0, 1), 0));
    let stuck = t.drive(from, at(10, 19, (0, 1), 0));
    let hold = |t: &mut Town| {
        // 交差点を抜けた所より先にいるが、後ろの車が交差点を抜けきるだけの空きはない。
        let b = t.trips.rows[block.slot()].as_mut().unwrap();
        (b.offset, b.speed, b.stalled) = (LaneOffset(10_000), MmPerTick::ZERO, 0);
    };
    hold(&mut t);
    let mut walker = None;
    let mut gone = None;
    let mut waited = 0;
    for k in 0..400_u64 {
        if k == 100 {
            walker = Some(t.walk(at(9, 10, (1, 0), 0), at(18, 10, (1, 0), 0)));
        }
        t.step(node);
        hold(&mut t);
        if let Some(w) = walker.and_then(|w| t.trips.get(w)) {
            waited = waited.max(w.waited);
        }
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
    (gone, waited)
}

#[test]
fn a_car_stuck_behind_a_blocked_exit_does_not_keep_walkers_waiting() {
    // 北から来た車が、交差点の手前で先の車線の空きを待ち続ける。この車は横断歩道の人に譲って待っているのでは
    // ないので、人は待ちすぎた所で渡る。
    let (gone, _) = walker_past_a_blocked_exit(false, at(10, 2, (0, 1), 0));
    // 歩くのに約 60 tick かかる。待つのは、待ちすぎるまでである。
    assert!(
        gone < 100 + 60 + u64::from(crossing::PED_WAIT) + 20,
        "待ちすぎた {gone}"
    );
}

#[test]
fn a_car_stuck_behind_a_blocked_exit_does_not_keep_walkers_waiting_at_a_signal() {
    // 信号の十字路で、東から来て南へ左折する車が、交差点の手前で先の車線の空きを待ち続ける。車の腕は青でも、
    // 動く見込みのない車を、人は待たない。待つのは、渡ってよい窓が来るまでである。
    let (gone, waited) = walker_past_a_blocked_exit(true, at(16, 10, (-1, 0), 0));
    assert!(waited < crossing::PED_WAIT, "車を待った {waited}");
    // 歩くのに約 60 tick かかる。待つのは、渡ってよい窓が来るまでの 1 周期までである。
    assert!(gone < 100 + 60 + 2 * signal::PHASE, "待ちすぎた {gone}");
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

#[test]
fn walkers_waiting_behind_a_car_stuck_in_the_junction_do_not_hold_other_cars() {
    // 北から来た車 I が、交差点を抜けきらずに南の腕の上で止まっている。I が動けるのは、南から北へ交差点を
    // 抜ける車 H が抜けたあととする（先の詰まりが街区をひと回りして H につながる輪の代わり）。
    // 南の腕の横断歩道を渡る人は、I が抜けるまで渡れない。待ちすぎても H を止めないので、輪は解ける。
    let (n, node) = crossroads();
    let mut t = Town::new(n);
    let stuck = t.drive(at(10, 5, (0, 1), 0), at(10, 19, (0, 1), 0));
    let south = link(&t.net, (10, 10), (10, 11), LaneMode::Car);
    let pin = |t: &mut Town| {
        let net = &t.net;
        let c = t.trips.rows[stuck.slot()].as_mut().unwrap();
        let leg = c.route.links.iter().position(|l| *l == south).unwrap();
        c.leg = u32::try_from(leg).unwrap();
        let lane = net.link_lanes(south).next().unwrap();
        c.enter(net, LaneOffset(3_000), Some(lane));
        (c.speed, c.stalled) = (MmPerTick::ZERO, 0);
    };
    pin(&mut t);
    let walker = t.walk(at(9, 10, (1, 0), 0), at(18, 10, (1, 0), 0));
    let mut other = None;
    let mut passed = None;
    for k in 0..250_u64 {
        if k == 60 {
            other = Some(t.drive(at(10, 17, (0, -1), 0), at(10, 2, (0, -1), 0)));
        }
        t.step(node);
        let through = other.is_some_and(|h| {
            t.trips
                .get(h)
                .is_none_or(|c| c.link().segment != south.segment)
        });
        if through && passed.is_none() {
            passed = Some(k);
        }
        if passed.is_none() {
            pin(&mut t);
        }
    }
    let passed = passed.expect("H が交差点を抜けない");
    assert!(passed < 60 + 30, "H が止められた {passed}");
    assert!(t.trips.get(walker).is_none(), "人が渡れない");
    assert!(t.trips.is_empty(), "残った {}", t.trips.len());
    assert_eq!(t.trips.rescues(), Default::default(), "救済が働いた");
}

#[test]
fn crosswalks_can_be_removed_and_restored_per_arm() {
    let (mut n, node) = crossroads();
    let south = n.arm_segment(node, Heading::S).unwrap();
    assert!(n.has_crosswalk(node, south));
    let v = n.version();
    // 戻すものはない。斜めには腕がない。
    assert_eq!(
        n.set_crosswalk(node, Heading::S, true),
        Err(DenyReason::CrosswalkUnchanged)
    );
    assert_eq!(
        n.set_crosswalk(node, Heading::SE, false),
        Err(DenyReason::NoCrosswalkArm)
    );
    assert_eq!(n.version(), v);
    n.set_crosswalk(node, Heading::S, false).unwrap();
    assert!(!n.has_crosswalk(node, south));
    assert!(n.version() > v, "経路の費用が変わるので網の版を上げる");
    assert_eq!(
        n.set_crosswalk(node, Heading::S, false),
        Err(DenyReason::CrosswalkUnchanged)
    );
    // 3 本より少ない区間のノードには横断歩道がない。
    let end = n.node_at(TilePos::new(10, 20)).unwrap();
    assert_eq!(
        n.set_crosswalk(end, Heading::N, false),
        Err(DenyReason::NotJunction)
    );
    // 腕を分けても、種類を変えても、外したまま残る。
    road(&mut n, (5, 15), (15, 15));
    let line = Line::between(TilePos::new(10, 10), TilePos::new(10, 15)).unwrap();
    let change = n.plan_road_kind(&line, RoadKind::Large).unwrap();
    n.change_road_kind(&change);
    let south = n.arm_segment(node, Heading::S).unwrap();
    assert_ne!(south, change.segments[0]);
    assert!(!n.has_crosswalk(node, south));
    // 戻すと元どおりになる。
    n.set_crosswalk(node, Heading::S, true).unwrap();
    assert!(n.has_crosswalk(node, south));
    assert_eq!(n.node(node).unwrap().no_crosswalk, 0);
}

#[test]
fn highway_arms_have_no_crosswalk_to_remove() {
    let mut n = Network::new();
    n.add_highway(TilePos::new(10, 10), TilePos::new(10, -20));
    road(&mut n, (0, 10), (20, 10));
    let node = n.node_at(TilePos::new(10, 10)).unwrap();
    assert_eq!(
        n.set_crosswalk(node, Heading::N, false),
        Err(DenyReason::NoCrosswalkArm)
    );
    n.set_crosswalk(node, Heading::E, false).unwrap();
}

#[test]
fn bulldozing_an_arm_forgets_its_removed_crosswalk() {
    let (mut n, node) = crossroads();
    n.set_crosswalk(node, Heading::S, false).unwrap();
    n.set_crosswalk(node, Heading::E, false).unwrap();
    // 南の腕を取り壊すと、その印は消え、東の腕の印は残る。
    let rect = |a: (i64, i64), b: (i64, i64)| {
        TileRect::from_corners(TilePos::new(a.0, a.1), TilePos::new(b.0, b.1))
    };
    n.bulldoze(rect((10, 11), (10, 20))).unwrap();
    assert_eq!(n.node(node).unwrap().no_crosswalk, 1 << Heading::E as u8);
    // 南へ引き直した腕には横断歩道がある。
    road(&mut n, (10, 10), (10, 20));
    let south = n.arm_segment(node, Heading::S).unwrap();
    assert!(n.has_crosswalk(node, south));
    // 交差点でなくなれば、印はすべて消える。
    n.bulldoze(rect((11, 10), (20, 10))).unwrap();
    n.bulldoze(rect((10, 11), (10, 20))).unwrap();
    assert_eq!(n.node(node).unwrap().no_crosswalk, 0);
}

#[test]
fn walkers_go_round_a_removed_crosswalk() {
    let (mut n, node) = crossroads();
    n.set_crosswalk(node, Heading::S, false).unwrap();
    let c = (10, 10);
    let side = |a, b| link(&n, a, b, LaneMode::Sidewalk);
    let seg = |a, b| side(a, b).segment;
    let (w, e, north) = ((9, 10), (11, 10), (10, 9));
    // 西から東へ、南の側を歩いてきた人は、南の腕を渡らずに、西と北の腕を渡って北の側へ出る。
    let (far, arms) = crossing::depart(&n, node, (side(w, c), false), side(c, e), None, None);
    assert_eq!(
        (far, arms.as_slice()),
        (true, &[seg(w, c), seg(c, north)][..])
    );
    // 南の側へ出なければならないなら、南の腕を越えずに東の腕まで回る。
    let (far, arms) =
        crossing::depart(&n, node, (side(w, c), false), side(c, e), Some(false), None);
    assert_eq!(
        (far, arms.as_slice()),
        (false, &[seg(w, c), seg(c, north), seg(c, e)][..])
    );
}

#[test]
fn walkers_move_early_to_the_side_that_avoids_a_removed_crosswalk_ahead() {
    // 東西の道路に、(10, 10) と (20, 10) で南北の道路が交わる。東の交差点の、西と南の腕の横断歩道を外す。
    // 東の交差点を東へ抜けるには、西の腕の北の側から来て、北の腕と東の腕を渡るしかない。
    let mut n = Network::new();
    road(&mut n, (0, 10), (30, 10));
    road(&mut n, (10, 0), (10, 20));
    road(&mut n, (20, 0), (20, 20));
    let west = n.node_at(TilePos::new(10, 10)).unwrap();
    let east = n.node_at(TilePos::new(20, 10)).unwrap();
    n.set_crosswalk(east, Heading::W, false).unwrap();
    n.set_crosswalk(east, Heading::S, false).unwrap();
    let side = |a, b| link(&n, a, b, LaneMode::Sidewalk);
    let (into, out) = (side((19, 10), (20, 10)), side((20, 10), (21, 10)));
    let removed = |far| crossing::removed_crossings(&n, east, into.segment, Some(far), out.segment);
    assert_eq!((removed(false), removed(true)), (1, 0));
    // 先を見なければ、西の交差点を南の側のまま抜ける。
    let from = (side((9, 10), (10, 10)), false);
    let next = side((10, 10), (11, 10));
    let (far, _) = crossing::depart(&n, west, from, next, None, None);
    assert!(!far);
    // 先を見れば、西の交差点で北の側へ移っておく。
    let (far, _) = crossing::depart(&n, west, from, next, None, Some((out, None)));
    assert!(far);
}

#[test]
fn routes_pay_for_crossing_a_removed_crosswalk() {
    let (mut n, node) = crossroads();
    let edge_cost = |n: &Network, from: LinkId, to: LinkId| {
        let g = crate::routing::Graph::build(n, LaneMode::Sidewalk);
        let v = g.vertex(from).unwrap();
        let to = g.vertex(to).unwrap();
        g.edges(v).iter().find(|e| e.to == to).unwrap().cost
    };
    let side = |n: &Network, a, b| link(n, a, b, LaneMode::Sidewalk);
    let (w, c, e, s) = ((9, 10), (10, 10), (11, 10), (10, 11));
    assert_eq!(edge_cost(&n, side(&n, w, c), side(&n, c, e)), 0);
    // 1 本だけ外しても、反対に回れるので費用はかからない。
    n.set_crosswalk(node, Heading::S, false).unwrap();
    assert_eq!(edge_cost(&n, side(&n, w, c), side(&n, c, e)), 0);
    // 南と北を外すと、西から東へは、どちらかを越えるしかない。南へ曲がるなら角を回るだけで済む。
    n.set_crosswalk(node, Heading::N, false).unwrap();
    assert!(edge_cost(&n, side(&n, w, c), side(&n, c, e)) > 0);
    assert_eq!(edge_cost(&n, side(&n, w, c), side(&n, c, s)), 0);
}

#[test]
fn walkers_avoid_a_removed_crosswalk_and_turning_cars_do_not_stop_there() {
    // 西の腕の南の歩道から東へ歩く人の列と、東から来て南へ曲がる車。人は (15, 10) の交差点の先まで歩く。
    // 横断歩道があれば、人は南の腕を渡り、車はその前で譲る。外せば、人は南の腕を渡らずに西と北の腕を
    // 渡って北の側へ移り、南へ曲がる車は譲らない。
    let run = |removed: bool| {
        let (mut n, node) = crossroads();
        road(&mut n, (15, 0), (15, 20));
        if removed {
            n.set_crosswalk(node, Heading::S, false).unwrap();
        }
        let south = n.arm_segment(node, Heading::S).unwrap();
        let mut t = Town::new(n);
        let (mut on_south, mut yielded, mut cars) = (0, 0, Vec::new());
        for k in 0..400_u64 {
            if k < 300 {
                for along in [0, 4_000] {
                    t.walk(at(9, 10, (1, 0), along), at(18, 10, (1, 0), 0));
                }
            }
            if k % 20 == 0 && k < 300 {
                cars.push(t.drive(at(14, 10, (-1, 0), 0), at(10, 18, (0, 1), 0)));
            }
            t.step(node);
            on_south += t.occupied().iter().filter(|o| **o == (node, south)).count();
            yielded += cars
                .iter()
                .filter_map(|c| t.trips.get(*c))
                .filter(|c| c.yielded > 0)
                .count();
        }
        for _ in 0..1_500 {
            t.step(node);
        }
        assert!(t.trips.is_empty(), "残った {}", t.trips.len());
        (on_south, yielded)
    };
    let (on_south, yielded) = run(false);
    assert!(on_south > 0 && yielded > 0, "{on_south} {yielded}");
    assert_eq!(run(true), (0, 0));
}

#[test]
fn walkers_still_get_through_a_junction_with_every_crosswalk_removed() {
    // どの腕の横断歩道も外すと、西から東へ抜ける人はどこかの腕を越えるしかない。そのときは、横断歩道が
    // あるときと同じく、車を止めて渡る。人も車も残らず着く。
    let (mut n, node) = crossroads();
    for h in [Heading::E, Heading::S, Heading::W, Heading::N] {
        n.set_crosswalk(node, h, false).unwrap();
    }
    let mut t = Town::new(n);
    let mut crossed = std::collections::BTreeSet::new();
    for k in 0..30_u32 {
        let along = (k * 1_700) % 8_000;
        t.walk(at(4, 10, (1, 0), along), at(16, 10, (1, 0), along));
        if k % 5 == 0 {
            t.drive(at(10, 1, (0, 1), 0), at(10, 19, (0, 1), 0));
        }
        for _ in 0..3 {
            t.step(node);
            crossed.extend(t.occupied());
        }
    }
    for _ in 0..1_500 {
        t.step(node);
        crossed.extend(t.occupied());
    }
    assert!(t.trips.is_empty(), "残った {}", t.trips.len());
    assert_eq!(crossed.len(), 1, "越えるのは 1 本だけ {crossed:?}");
}

#[test]
fn walkers_take_a_removed_crosswalk_rather_than_crossing_a_highway_unguarded() {
    // 高速道路の内側の T 字路で東の腕の横断歩道を外す。西の腕の北の側から東の腕の北の側へ出る人は、
    // 高速道路の腕を車を止めずに越えるのではなく、南へ回って西の腕と、外した東の腕を渡る。
    let mut n = Network::new();
    n.add_highway(TilePos::new(10, 10), TilePos::new(10, -20));
    road(&mut n, (0, 10), (20, 10));
    let node = n.node_at(TilePos::new(10, 10)).unwrap();
    n.set_crosswalk(node, Heading::E, false).unwrap();
    let side = |a, b| link(&n, a, b, LaneMode::Sidewalk);
    let (w, c, e) = ((9, 10), (10, 10), (11, 10));
    let (far, arms) = crossing::depart(&n, node, (side(w, c), true), side(c, e), Some(true), None);
    assert!(far);
    assert_eq!(arms.as_slice(), &[side(w, c).segment, side(c, e).segment]);
}

/// `t` を、歩く人 `walker` が交差点の車道を渡っている途中になるまで進める。
fn until_crossing(t: &mut Town, node: NodeId, walker: TripId) {
    for _ in 0..200 {
        let inside = t
            .trips
            .get(walker)
            .is_some_and(|w| !w.last_leg() && w.offset > w.link_len);
        if inside {
            return;
        }
        t.step(node);
    }
    panic!("渡り始めない");
}

/// 渡っている途中の `walker` の道のりが、網を `change` で変えても渡り終えるまで変わらないことを確かめる。
fn crossing_keeps_its_way(
    mut t: Town,
    node: NodeId,
    walker: TripId,
    change: impl Fn(&mut Network),
) {
    until_crossing(&mut t, node, walker);
    let way = t.trips.get(walker).unwrap().departure(&t.net);
    let leg = t.trips.get(walker).unwrap().leg;
    change(&mut t.net);
    loop {
        t.step(node);
        let Some(w) = t.trips.get(walker).filter(|w| w.leg == leg) else {
            break;
        };
        assert_eq!(w.departure(&t.net), way, "渡っている途中で道のりが変わった");
    }
    for _ in 0..1_500 {
        t.step(node);
    }
    assert!(t.trips.is_empty(), "残った {}", t.trips.len());
}

#[test]
fn walkers_keep_their_way_when_crosswalks_change_while_they_cross() {
    // 西の腕の南の側から東へ、南の腕を渡っている人がいるあいだに、北と東の腕を外し、南の腕も外す。
    // 人は渡りかけた南の腕を渡りきり、そのあいだ車はその前で止まる（`Town::step` が確かめる）。
    for arms in [vec![Heading::N, Heading::E], vec![Heading::S]] {
        let (n, node) = crossroads();
        let mut t = Town::new(n);
        let walker = t.walk(at(9, 10, (1, 0), 6_000), at(18, 10, (1, 0), 0));
        for k in 0..6 {
            t.drive(at(10, 1 + k, (0, 1), 0), at(10, 19, (0, 1), 0));
        }
        crossing_keeps_its_way(t, node, walker, |n| {
            for h in &arms {
                n.set_crosswalk(node, *h, false).unwrap();
            }
        });
    }
}

#[test]
fn walkers_keep_their_way_when_the_next_junction_changes_while_they_cross() {
    // 西の交差点を渡っている人がいるあいだに、東の交差点の西と南の腕を外す。外すと、西の交差点で
    // 先に移っておく側が変わるが、渡りかけた人の道のりは変えない。
    let mut n = Network::new();
    road(&mut n, (0, 10), (30, 10));
    road(&mut n, (10, 0), (10, 20));
    road(&mut n, (20, 0), (20, 20));
    let west = n.node_at(TilePos::new(10, 10)).unwrap();
    let east = n.node_at(TilePos::new(20, 10)).unwrap();
    let mut t = Town::new(n);
    let walker = t.walk(at(9, 10, (1, 0), 6_000), at(28, 10, (1, 0), 0));
    crossing_keeps_its_way(t, west, walker, |n| {
        n.set_crosswalk(east, Heading::W, false).unwrap();
        n.set_crosswalk(east, Heading::S, false).unwrap();
    });
}

#[test]
fn corner_route_goes_through_the_corners_of_the_arms_it_crosses() {
    let (n, node) = crossroads();
    let c = (10, 10);
    let side = |a, b| link(&n, a, b, LaneMode::Sidewalk);
    let seg = |a, b| side(a, b).segment;
    let (w, e, s) = ((9, 10), (11, 10), (10, 11));
    // 西から東へまっすぐ、南の側を歩く。角は腕の時計回りの側かどうかで表す（南の腕の西の側が時計回り）。
    // 西の腕の南の角から南の腕の西の角へ回り、南の腕を渡って、南の腕の東の角から東の腕の南の角へ回る。
    let route = crossing::corner_route(&n, node, (side(w, c), false), (side(c, e), false), None);
    let want = [
        (seg(w, c), false),
        (seg(c, s), true),
        (seg(c, s), false),
        (seg(c, e), true),
    ];
    assert_eq!(route.as_slice(), want);
    // 渡る腕は、同じ腕の角が続くところと同じである。
    let (_, arms) = crossing::depart(&n, node, (side(w, c), false), side(c, e), None, None);
    let crossed: Vec<_> = route.windows(2).filter(|p| p[0].0 == p[1].0).collect();
    assert_eq!(crossed.len(), arms.len());
    // 南へ曲がる人は、どの腕も渡らずに角を回る。
    let route = crossing::corner_route(&n, node, (side(w, c), false), (side(c, s), false), None);
    assert_eq!(route.len(), 2);
    assert_ne!(route[0].0, route[1].0);
}
