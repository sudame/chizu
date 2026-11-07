use super::signal::{self, ALL_RED, Demand, GREEN, Light, PHASE, YELLOW};
use super::*;
use crate::command::{NodeId, RoadKind};
use crate::network::Line;
use crate::routing::Delays;

fn road(n: &mut Network, from: (i64, i64), to: (i64, i64), kind: RoadKind) {
    let line = Line::between(TilePos::new(from.0, from.1), TilePos::new(to.0, to.1)).unwrap();
    n.build_road(&line, kind).unwrap();
}

fn at(x: i64, y: i64, step: (i8, i8), along: u32) -> SidewalkPos {
    SidewalkPos {
        tile: TilePos::new(x, y),
        step,
        along: LaneOffset(along),
    }
}

/// (10, 10) で十字に交わる小さい道路の網と、交差点のノード。`signal` なら交差点に信号を置く。
fn crossroads(signal: bool) -> (Network, NodeId) {
    let mut n = Network::new();
    road(&mut n, (0, 10), (20, 10), RoadKind::Small);
    road(&mut n, (10, 0), (10, 20), RoadKind::Small);
    let node = n.node_at(TilePos::new(10, 10)).unwrap();
    if signal {
        n.set_signal(node, true).unwrap();
    }
    (n, node)
}

/// 車も人もいない交差点の様子。信号は固定の周期のとおりに回る。
fn quiet() -> Demand {
    Demand::default()
}

/// 軸 `axis` が青になった最初の tick。
fn green_start(n: &Network, node: NodeId, axis: u8) -> u64 {
    (1..1_000)
        .find(|t| {
            let light = |t| {
                signal::state(n, node, Tick(t), &quiet())
                    .unwrap()
                    .light_for(axis)
            };
            light(*t) == Light::Green && light(*t - 1) != Light::Green
        })
        .unwrap()
}

#[test]
fn each_axis_gets_its_turn_and_never_two_at_once() {
    let (n, node) = crossroads(true);
    // 十字路の腕は x の向きと y の向きの 2 つの軸にまとまる。
    let cycle = 2 * PHASE;
    let mut count = [[0_u64; 3]; 4];
    for t in 0..cycle {
        let st = signal::state(&n, node, Tick(t), &quiet()).unwrap();
        assert_eq!(st.light_for(1), Light::Red, "十字路にない軸");
        assert_eq!(st.light_for(3), Light::Red, "十字路にない軸");
        assert!(
            st.light_for(0) == Light::Red || st.light_for(2) == Light::Red,
            "2 つの軸が同時に赤でない {st:?}"
        );
        for axis in 0..4_u8 {
            count[usize::from(axis)][st.light_for(axis) as usize] += 1;
        }
        // 周期で一回りする。
        assert_eq!(signal::state(&n, node, Tick(t + cycle), &quiet()), Some(st));
    }
    for axis in [0, 2] {
        assert_eq!(
            count[axis],
            [GREEN, YELLOW, cycle - GREEN - YELLOW],
            "軸 {axis}"
        );
    }
    // 腕ごとの灯火は、同じ軸の向き合う 2 本で同じになる。
    let lights = signal::arm_lights(&n, node, Tick(0), &quiet());
    assert_eq!(lights.len(), 4);
    for (step, light) in &lights {
        let opposite = lights
            .iter()
            .find(|(s, _)| *s == (-step.0, -step.1))
            .unwrap();
        assert_eq!(opposite.1, *light);
    }
}

#[test]
fn no_signal_means_no_state() {
    let (n, node) = crossroads(false);
    assert_eq!(signal::state(&n, node, Tick(0), &quiet()), None);
    assert!(signal::arm_lights(&n, node, Tick(0), &quiet()).is_empty());
}

struct Cars {
    net: Network,
    router: AstarRouter,
    trips: Trips,
    now: Tick,
}

impl Cars {
    fn new(net: Network, now: u64) -> Cars {
        let router = AstarRouter::new(&net);
        Cars {
            net,
            router,
            trips: Trips::default(),
            now: Tick(now),
        }
    }

    fn drive(&mut self, from: SidewalkPos, to: SidewalkPos) -> TripId {
        self.trips
            .spawn_drives(&self.net, &mut self.router, &[(from, to)], self.now)[0]
            .unwrap()
    }

    fn step(&mut self) -> Vec<TripId> {
        let arrived = self.trips.step(&self.net, &mut self.router, self.now);
        assert_eq!(
            self.trips.lights().now,
            self.now,
            "控えた灯火の tick が違う"
        );
        self.now = self.now.next();
        arrived
    }

    /// 直前に進めた tick に、交差点 `node` で効いた信号。
    fn lit(&self, node: NodeId) -> signal::SignalState {
        self.trips.lights().state(&self.net, node).unwrap()
    }
}

/// 十字路で、y の向きが青になった tick から、東へ向かう x の向きの車を 1 台走らせる。`stream` 台の車を、
/// 交差点の 3 マス北から南へ続けて走らせる。x の向きの車が交差点に入った tick と、入る前に止まったかを返す。
fn eastbound_against(stream: usize) -> (Tick, bool, u64) {
    let (n, node) = crossroads(true);
    let start = green_start(&n, node, 2);
    let mut c = Cars::new(n, start);
    let south = (at(10, 7, (0, 1), 0), at(10, 19, (0, 1), 0));
    let ids = c
        .trips
        .spawn_drives(&c.net, &mut c.router, &vec![south; stream], c.now);
    assert!(ids.iter().all(Option::is_some));
    let id = c.drive(at(6, 10, (1, 0), 0), at(18, 10, (1, 0), 0));
    let first = c.trips.get(id).unwrap().link();
    let mut stopped = false;
    let mut crossed = None;
    for _ in 0..(3 * PHASE) {
        let now = c.now;
        c.step();
        let Some(t) = c.trips.get(id) else {
            break;
        };
        // 交差点に入るのは、停止線を越えたときである。
        if t.link() == first && t.bound.is_none() {
            stopped |= t.speed() == MmPerTick::ZERO;
        } else if crossed.is_none() {
            let light = c.lit(node).light_for(0);
            assert_eq!(light, Light::Green, "青でないのに入った");
            crossed = Some(now);
        }
    }
    (crossed.expect("青になっても進まない"), stopped, start)
}

#[test]
fn a_car_stops_on_red_while_the_green_axis_has_traffic() {
    // y の向きの車が続くあいだは、x の向きの車は赤で待ち、遅くとも骨組みの周期で x の向きが青になれば入る。
    let (crossed, stopped, start) = eastbound_against(60);
    assert!(stopped, "赤で止まらない");
    let green = start + GREEN + YELLOW + ALL_RED;
    assert!(crossed.0 <= green + 5, "青になっても待ちすぎた {crossed:?}");
}

#[test]
fn a_lone_car_gets_the_green_without_waiting_for_the_cycle() {
    // 青の向きに車が来ていなければ、赤の向きに来た車へ青を渡す。骨組みの青が終わるのを待たない。
    let (crossed, _, start) = eastbound_against(0);
    assert!(
        crossed.0 < start + GREEN,
        "空いた向きの青を待った {crossed:?}"
    );
}

#[test]
fn signalled_crossroads_lets_cars_in_only_on_green_or_yellow() {
    let (n, node) = crossroads(true);
    let mut c = Cars::new(n, 0);
    // 4 つの腕から、向こうの腕へ直進する車を 15 台ずつ出す。
    let mut drives = Vec::new();
    let mut incoming = Vec::new();
    for (dx, dy) in [(-1_i64, 0_i64), (0, -1), (1, 0), (0, 1)] {
        let step = (-dx as i8, -dy as i8);
        let from = at(10 + 10 * dx, 10 + 10 * dy, step, 0);
        let to = at(10 - 9 * dx, 10 - 9 * dy, step, 0);
        drives.extend(std::iter::repeat_n((from, to), 15));
        let link = at(10 + dx, 10 + dy, step, 1)
            .resolve_on(&c.net, LaneMode::Car)
            .unwrap()
            .0;
        incoming.push((link, signal::axis((dx, dy))));
    }
    let ids = c.trips.spawn_drives(&c.net, &mut c.router, &drives, c.now);
    assert!(ids.iter().all(Option::is_some));
    let mut arrived = 0;
    let mut entered = [0_usize; 4];
    for _ in 0..1_500 {
        // 交差点に入るのは、停止線を越えたときである。
        let before: Vec<(TripId, LinkId)> = c
            .trips
            .iter()
            .filter(|(_, t)| t.bound.is_none())
            .map(|(id, t)| (id, t.link()))
            .collect();
        let now = c.now;
        arrived += c.step().len();
        let st = c.lit(node);
        for (id, link) in before {
            let Some(arm) = incoming.iter().position(|(l, _)| *l == link) else {
                continue;
            };
            if c.trips
                .get(id)
                .is_none_or(|t| t.link() != link || t.bound.is_some())
            {
                entered[arm] += 1;
                let light = st.light_for(incoming[arm].1);
                assert_ne!(light, Light::Red, "赤で入った {now:?}");
            }
        }
    }
    assert_eq!(arrived, 60);
    assert_eq!(entered, [15; 4]);
    assert_eq!(c.trips.rescues(), Default::default(), "救済が働いた");
}

/// (10, 10) を中心に、`arms` の向きへ 10 マスずつ腕を延ばした交差点に信号を置く。
/// 腕は向き合う 2 本をまとめて 1 本の道路で引くので、`arms` には片方の向きだけを書く。`t` なら
/// 最後の 1 本は中心から片側だけにする。
fn signalled(arms: &[(i64, i64)], t: bool) -> (Network, NodeId) {
    let mut n = Network::new();
    for (k, &(dx, dy)) in arms.iter().enumerate() {
        let from = if t && k + 1 == arms.len() {
            (10, 10)
        } else {
            (10 - 10 * dx, 10 - 10 * dy)
        };
        road(&mut n, from, (10 + 10 * dx, 10 + 10 * dy), RoadKind::Small);
    }
    let node = n.node_at(TilePos::new(10, 10)).unwrap();
    n.set_signal(node, true).unwrap();
    (n, node)
}

/// ノード `node` から出ていく腕の向き。
fn arm_steps(n: &Network, node: NodeId) -> Vec<(i64, i64)> {
    signal::arm_lights(n, node, Tick(0), &quiet())
        .into_iter()
        .map(|(s, _)| s)
        .collect()
}

/// 腕 `a` から入って腕 `b` へ出る車。
fn movement(a: (i64, i64), b: (i64, i64)) -> (SidewalkPos, SidewalkPos) {
    let step = |d: (i64, i64)| (d.0 as i8, d.1 as i8);
    (
        at(10 + 10 * a.0, 10 + 10 * a.1, step((-a.0, -a.1)), 0),
        at(10 + 9 * b.0, 10 + 9 * b.1, step(b), 0),
    )
}

fn end_node(n: &Network, link: LinkId) -> NodeId {
    let s = n.segment(link.segment).unwrap();
    match link.dir {
        crate::network::Dir::Forward => s.b,
        crate::network::Dir::Backward => s.a,
    }
}

/// 交差点のどの腕からも、ほかのどの腕へも `per` 台ずつ出して `ticks` 進める。交差点に入った車が
/// 赤の腕から来ていないことを確かめ、入った台数を返す。
fn every_movement(n: Network, node: NodeId, per: usize, ticks: usize) -> usize {
    let arms = arm_steps(&n, node);
    let mut c = Cars::new(n, 0);
    let mut drives = Vec::new();
    for &a in &arms {
        for &b in arms.iter().filter(|b| **b != a) {
            drives.extend(std::iter::repeat_n(movement(a, b), per));
        }
    }
    let ids = c.trips.spawn_drives(&c.net, &mut c.router, &drives, c.now);
    assert!(ids.iter().all(Option::is_some));
    let (mut arrived, mut entered) = (0, 0);
    for _ in 0..ticks {
        // 交差点に入るのは、停止線を越えたときである。
        let before: Vec<(TripId, LinkId)> = c
            .trips
            .iter()
            .filter(|(_, t)| {
                car::on_road(t) && t.bound.is_none() && end_node(&c.net, t.link()) == node
            })
            .map(|(id, t)| (id, t.link()))
            .collect();
        let now = c.now;
        arrived += c.step().len();
        let st = c.lit(node);
        for (id, link) in before {
            if c.trips
                .get(id)
                .is_some_and(|t| t.link() != link || t.bound.is_some())
            {
                entered += 1;
                let axis = signal::axis(c.net.segment_line(link.segment).unwrap().step);
                assert_ne!(
                    st.light_for(axis),
                    Light::Red,
                    "赤で入った {now:?} {link:?}"
                );
            }
        }
    }
    assert_eq!(arrived, drives.len(), "着かない車がある");
    assert_eq!(entered, drives.len());
    entered
}

#[test]
fn turning_cars_enter_only_on_green_or_yellow() {
    // 十字路で、直進、左折、対向の車線をまたぐ右折を、どの腕からも出す。
    let (n, node) = signalled(&[(1, 0), (0, 1)], false);
    every_movement(n, node, 4, 3_000);
}

#[test]
fn t_junction_enters_only_on_green_or_yellow() {
    let (n, node) = signalled(&[(1, 0), (0, 1)], true);
    assert_eq!(arm_steps(&n, node).len(), 3);
    every_movement(n, node, 5, 3_000);
}

#[test]
fn diagonal_arms_enter_only_on_green_or_yellow() {
    // 縦横と 2 本の斜めで、腕 8 本、軸 4 つの交差点。
    let (n, node) = signalled(&[(1, 0), (0, 1), (1, 1), (1, -1)], false);
    assert_eq!(arm_steps(&n, node).len(), 8);
    every_movement(n, node, 1, 4_000);
}

/// 車 `id` を、経路の中のリンク `link` の位置 `offset` に止めて置き直す。
fn put(c: &mut Cars, id: TripId, link: LinkId, offset: u32) {
    let t = c.trips.rows[id.slot()].as_mut().unwrap();
    let leg = t.route.links.iter().position(|l| *l == link).unwrap();
    t.leg = u32::try_from(leg).unwrap();
    let lane = c.net.link_lanes(link).next().unwrap();
    t.enter(&c.net, LaneOffset(offset), Some(lane));
    t.speed = MmPerTick::ZERO;
    t.stalled = 0;
    t.waiting = false;
}

#[test]
fn a_car_stuck_inside_a_signalled_junction_is_passed_before_the_rescue() {
    // 軸 4 つの信号の交差点で、北から南へ抜ける車が交差点の中で止まったままになる。
    // 西から東へ向かう車は、赤の待ちも数えて `GIVE_UP` に届き、`RESCUE` より先に脇を抜ける。
    let (n, _) = signalled(&[(1, 0), (0, 1), (1, 1), (1, -1)], false);
    let mut c = Cars::new(n, 0);
    let (from, to) = movement((0, -1), (0, 1));
    let stuck = c.drive(from, to);
    let south = at(10, 10, (0, 1), 1)
        .resolve_on(&c.net, LaneMode::Car)
        .unwrap()
        .0;
    let (from, to) = movement((-1, 0), (1, 0));
    let car = c.drive(from, to);
    let first = c.trips.get(car).unwrap().link();
    put(&mut c, stuck, south, 500);
    let mut entered = None;
    for tick in 0..junction::RESCUE {
        c.step();
        put(&mut c, stuck, south, 500);
        if entered.is_none() && c.trips.get(car).is_some_and(|t| t.link() != first) {
            entered = Some(tick);
        }
    }
    assert!(entered.is_some(), "抜ける前に救済が働く");
    let r = c.trips.rescues();
    assert_eq!(r.teleported, 0);
    assert!(r.passed_stuck >= 1, "{r:?}");
}

/// 十字路の tick `t` の灯火を、軸ごとの様子 `x` と `y`（x の向きと y の向き）から決める。
fn lit(
    n: &Network,
    node: NodeId,
    t: u64,
    x: signal::AxisDemand,
    y: signal::AxisDemand,
) -> [Light; 2] {
    let mut d = Demand::default();
    d.note(node, 0, |a| *a = x);
    d.note(node, 2, |a| *a = y);
    let st = signal::state(n, node, Tick(t), &d).unwrap();
    [st.light_for(0), st.light_for(2)]
}

#[test]
fn the_green_goes_where_the_cars_are() {
    let (n, node) = crossroads(true);
    let none = signal::AxisDemand::default();
    let cars = signal::AxisDemand { cars: true, ..none };
    let committed = signal::AxisDemand {
        cars: true,
        committed: true,
        ..none
    };
    let walkers = signal::AxisDemand {
        walkers: true,
        ..none
    };
    let cars_and_walkers = signal::AxisDemand {
        walkers: true,
        ..cars
    };
    let cars_and_patient = signal::AxisDemand {
        patient: true,
        ..cars_and_walkers
    };
    // y の向きの段階の、青の始めと黄の始め。
    let g = green_start(&n, node, 2);
    let yellow = g + GREEN;
    use Light::{Green, Red, Yellow};
    // 車も人もいなければ、骨組みのとおりに回る。
    assert_eq!(lit(&n, node, g, none, none), [Red, Green]);
    assert_eq!(lit(&n, node, yellow, none, none), [Red, Yellow]);
    // 青の向きに車がなく、赤の向きに車が来れば、赤の向きへ青を渡す。
    assert_eq!(lit(&n, node, g, cars, none), [Green, Red]);
    // 青の向きにも車がいれば、渡さない。
    assert_eq!(lit(&n, node, g, cars, cars), [Red, Green]);
    // x の向きの腕を渡ろうと待つ人は、信号のない交差点と同じく、来た車を先に通す。
    assert_eq!(lit(&n, node, g, cars_and_walkers, none), [Green, Red]);
    // 長く待った人がいれば、y の向きの青のあいだに渡れるよう、渡さない。
    assert_eq!(lit(&n, node, g, cars_and_patient, none), [Red, Green]);
    // どの向きにも車がなく、青の向き（y）の腕を渡ろうと待つ人だけがいれば、x の向きを青にして渡らせる。
    assert_eq!(lit(&n, node, g, none, walkers), [Green, Red]);
    // x の向きの腕を渡ろうと待つ人もいれば、その人が渡れる y の向きの青のままにする。
    assert_eq!(lit(&n, node, g, walkers, walkers), [Red, Green]);
    // 青を戻すときに、渡していた向きに止まりきれない車がいれば、その tick は黄にする。
    assert_eq!(lit(&n, node, g, committed, cars), [Yellow, Red]);
    // 段階の向きにも止まりきれない車がいれば、どちらも黄にする。どちらの車も赤では入らない。
    assert_eq!(lit(&n, node, g, committed, committed), [Yellow, Yellow]);
    // 黄の段階では、止まりきれない車のいる向きを黄で通し、止まれる車だけなら次の向きへ早めに渡す。
    assert_eq!(lit(&n, node, yellow, cars, committed), [Red, Yellow]);
    assert_eq!(lit(&n, node, yellow, cars, cars), [Green, Red]);
    assert_eq!(lit(&n, node, yellow + YELLOW, cars, none), [Green, Red]);
    assert_eq!(lit(&n, node, yellow + YELLOW, none, none), [Red, Red]);
    // ほかの向きに車が来ていなければ、黄と赤の段階でも、段階の向きの車と、ほかの向きの腕を渡る人のために
    // 青のままにする。
    assert_eq!(lit(&n, node, yellow, none, cars), [Red, Green]);
    assert_eq!(lit(&n, node, yellow + YELLOW, none, cars), [Red, Green]);
    assert_eq!(lit(&n, node, yellow + YELLOW, walkers, none), [Red, Green]);
}

#[test]
fn walkers_may_start_late_in_the_green_when_their_arm_has_no_cars() {
    let (n, node) = crossroads(true);
    // y の向きの段階の、青の終わり際。x の向きの腕を渡る人は、骨組みの青の終わる WALK_CLEAR tick 前からは
    // 渡り始めない。ただし、x の向きに車が来ていなければ渡り始める。
    let late = green_start(&n, node, 2) + GREEN - 1;
    let walk = |x_cars: bool| {
        let mut d = Demand::default();
        d.note(node, 0, |a| {
            a.cars = x_cars;
            a.walkers = true;
        });
        d.note(node, 2, |a| a.cars = true);
        signal::state(&n, node, Tick(late), &d).unwrap()
    };
    let st = walk(true);
    assert_eq!(st.light_for(0), Light::Red);
    assert_eq!(st.light_for(2), Light::Green);
    assert!(!st.walk(0), "車の来る腕を青の終わり際に渡り始めた");
    let st = walk(false);
    assert_eq!(st.light_for(2), Light::Green);
    assert!(st.walk(0), "車の来ない腕を渡り始めない");
}

#[test]
fn an_axis_with_a_car_that_cannot_stop_is_never_red() {
    // 軸 3 つ（x の向き、斜め、y の向き）の交差点。x の向きの段階で、x の向きに車がいない。
    // 斜めの向きに車が来ていて、y の向きには、前の tick まで青を渡されていた止まりきれない車がいる。
    let (n, node) = signalled(&[(1, 0), (0, 1), (1, 1)], false);
    let g = green_start(&n, node, 0);
    let mut d = Demand::default();
    d.note(node, 1, |a| a.cars = true);
    d.note(node, 2, |a| {
        a.cars = true;
        a.committed = true;
    });
    use Light::{Red, Yellow};
    for t in [g, g + GREEN, g + GREEN + YELLOW] {
        let st = signal::state(&n, node, Tick(t), &d).unwrap();
        // 青の段階でも、黄と赤の段階でも、止まりきれない車のいる軸は黄で、青はその車が抜けるまで渡さない。
        assert_eq!(
            [st.light_for(0), st.light_for(1), st.light_for(2)],
            [Red, Red, Yellow],
            "tick {t}"
        );
    }
}

/// 西の (-8, 4) から東の (48, 4) へ、北の y=0 と南の y=10 の 2 本の並行する道のどちらかを通って抜ける網。
/// 北の道を通るほうが 4 マス短い。どちらの道にも 8 マスおきに横道が付き、T 字の交差点が 4 つずつある。
/// 北の道の交差点を返す。
fn parallel_roads() -> (Network, Vec<NodeId>) {
    let mut n = Network::new();
    road(&mut n, (-8, 4), (0, 4), RoadKind::Small);
    road(&mut n, (40, 4), (48, 4), RoadKind::Small);
    for y in [0, 10] {
        road(&mut n, (0, y), (40, y), RoadKind::Small);
    }
    road(&mut n, (0, 0), (0, 10), RoadKind::Small);
    road(&mut n, (40, 0), (40, 10), RoadKind::Small);
    let mut north = Vec::new();
    for x in [8, 16, 24, 32] {
        road(&mut n, (x, 0), (x, -4), RoadKind::Small);
        road(&mut n, (x, 10), (x, 14), RoadKind::Small);
        north.push(n.node_at(TilePos::new(x, 0)).unwrap());
    }
    (n, north)
}

/// 西の道の上の 7 か所から東の道の上の 7 か所へ車を出し、北の道を通る車の数と、出した車の数を返す。
fn drives_north(c: &mut Cars) -> (usize, usize) {
    let (a, b) = (
        c.net.node_at(TilePos::new(16, 0)).unwrap(),
        c.net.node_at(TilePos::new(24, 0)).unwrap(),
    );
    let north = c
        .net
        .segments()
        .find(|(_, s)| (s.a, s.b) == (a, b) || (s.a, s.b) == (b, a))
        .expect("北の道がない")
        .0;
    let pairs: Vec<_> = (-7..0)
        .flat_map(|x| (41..48).map(move |y| (at(x, 4, (1, 0), 0), at(y, 4, (1, 0), 0))))
        .collect();
    let ids = c.trips.spawn_drives(&c.net, &mut c.router, &pairs, c.now);
    let routes: Vec<_> = ids
        .iter()
        .map(|id| {
            c.trips
                .get(id.expect("車を出せない"))
                .unwrap()
                .route()
                .clone()
        })
        .collect();
    let via_north = routes
        .iter()
        .filter(|r| r.links.iter().any(|l| l.segment == north))
        .count();
    (via_north, routes.len())
}

/// 北の道の交差点 `node` へ、北の横道から入るリンク。
fn side_link(n: &Network, node: NodeId) -> LinkId {
    let tile = n.node(node).unwrap().tile();
    let top = n.node_at(TilePos::new(tile.x, tile.y - 4)).unwrap();
    let (segment, s) = n
        .segments()
        .find(|(_, s)| (s.a, s.b) == (top, node) || (s.a, s.b) == (node, top))
        .expect("横道がない");
    LinkId {
        segment,
        dir: if s.b == node {
            crate::network::Dir::Forward
        } else {
            crate::network::Dir::Backward
        },
        mode: LaneMode::Car,
    }
}

#[test]
fn cars_avoid_the_parallel_road_lined_with_busy_signals() {
    let (n, north) = parallel_roads();
    // 信号がなければ、どの車も短い北の道を選ぶ。
    let mut c = Cars::new(n.clone(), 0);
    let (via, all) = drives_north(&mut c);
    assert_eq!(via, all);
    // 北の道の交差点に信号を並べても、横道に車が来なければ、感応式の信号はほとんど待たせないので、北の道を選ぶ。
    let version = c.net.version();
    for node in &north {
        c.net.set_signal(*node, true).unwrap();
    }
    assert_eq!(c.net.version(), version);
    c.router.set_signals(&c.net);
    assert_eq!(c.router.version(), version);
    assert_eq!(drives_north(&mut c).0, all);
    // 横道が忙しく、交わる軸が青を使い切ると、網を作り直さずに、どの車も南の道を選ぶ。
    let mut side: Vec<LinkId> = north.iter().map(|x| side_link(&c.net, *x)).collect();
    side.sort_unstable();
    let busy = Delays::default().with_busy(side.iter().map(|l| (*l, 1_000)));
    c.router.set_delays(&busy);
    assert_eq!(drives_north(&mut c).0, 0);
    // 信号の費用は、網から作り直したグラフに同じ忙しさを入れたものと同じである。
    let mut fresh = AstarRouter::new(&c.net);
    fresh.set_delays(&busy);
    assert_eq!(c.router.graph(LaneMode::Car), fresh.graph(LaneMode::Car));
    // 信号を外せば、横道が忙しくても北の道に戻る。
    for node in &north {
        c.net.set_signal(*node, false).unwrap();
    }
    c.router.set_signals(&c.net);
    let mut plain = Cars::new(n, 0).router;
    plain.set_delays(&busy);
    assert_eq!(c.router.graph(LaneMode::Car), plain.graph(LaneMode::Car));
    assert_eq!(drives_north(&mut c), (all, all));
}

#[test]
fn the_expected_wait_follows_how_busy_the_other_axes_are() {
    let (x, y) = (0_u8, 2_u8);
    let cross = [true, false, true, false];
    // 十字路の交わる軸が青を使い切ると、周期 70 tick のうち 40 tick が赤である。40 × 40 ÷ (2 × 70) tick。
    let full = [1_000, 0, 1_000, 0];
    assert_eq!(
        signal::expected_wait_ms(cross, x, full),
        40 * 40 * 1_000 / 140
    );
    // 自分の軸がどれだけ忙しくても、見込みは変わらない。
    assert_eq!(
        signal::expected_wait_ms(cross, x, [0, 0, 1_000, 0]),
        40 * 40 * 1_000 / 140
    );
    // 交わる軸に車が来なければ、待つのは自分の軸の黄と全方向の赤のあいだだけである。5 × 5 ÷ (2 × 35) tick。
    assert_eq!(
        signal::expected_wait_ms(cross, x, [1_000, 0, 0, 0]),
        5 * 5 * 1_000 / 70
    );
    // 交わる軸が周期の 1/7（10 tick）を要るなら、その段階は 1/3 に縮む。赤は 5 + 35 / 3 tick。
    let some = signal::expected_wait_ms(cross, y, [143, 0, 0, 0]);
    let red = 5_000 + 143 * 70 * 35 / 30;
    assert_eq!(some, red * red / (2 * (red + 30_000)));
    assert!(5 * 5 * 1_000 / 70 < some && some < 40 * 40 * 1_000 / 140);
    // 軸 3 つで、ほかの 2 つの軸が青を使い切ると、周期 105 tick のうち 75 tick が赤である。
    let three = [true, true, true, false];
    assert_eq!(
        signal::expected_wait_ms(three, x, [0, 1_000, 1_000, 0]),
        75 * 75 * 1_000 / 210
    );
}
