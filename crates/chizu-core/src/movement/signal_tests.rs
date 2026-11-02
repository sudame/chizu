use super::signal::{self, ALL_RED, GREEN, Light, PHASE, YELLOW};
use super::*;
use crate::command::{NodeId, RoadKind};
use crate::network::Line;

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

/// 軸 `axis` が青になった最初の tick。
fn green_start(n: &Network, node: NodeId, axis: u8) -> u64 {
    (1..1_000)
        .find(|t| {
            let light = |t| signal::state(n, node, Tick(t)).unwrap().light_for(axis);
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
        let st = signal::state(&n, node, Tick(t)).unwrap();
        assert!(st.axis == 0 || st.axis == 2, "十字路にない軸 {}", st.axis);
        for axis in 0..4_u8 {
            count[usize::from(axis)][st.light_for(axis) as usize] += 1;
        }
        // 周期で一回りする。
        assert_eq!(signal::state(&n, node, Tick(t + cycle)), Some(st));
    }
    for axis in [0, 2] {
        assert_eq!(
            count[axis],
            [GREEN, YELLOW, cycle - GREEN - YELLOW],
            "軸 {axis}"
        );
    }
    // 腕ごとの灯火は、同じ軸の向き合う 2 本で同じになる。
    let lights = signal::arm_lights(&n, node, Tick(0));
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
    assert_eq!(signal::state(&n, node, Tick(0)), None);
    assert!(signal::arm_lights(&n, node, Tick(0)).is_empty());
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
        self.now = self.now.next();
        arrived
    }
}

#[test]
fn a_car_stops_on_red_and_goes_on_green() {
    let (n, node) = crossroads(true);
    // y の向きが青になった tick から始める。東へ向かう x の向きの車は赤で待つ。
    let start = green_start(&n, node, 2);
    let mut c = Cars::new(n, start);
    let id = c.drive(at(6, 10, (1, 0), 0), at(18, 10, (1, 0), 0));
    let first = c.trips.get(id).unwrap().link();
    let mut stopped = false;
    let mut crossed = None;
    for _ in 0..(2 * PHASE) {
        let now = c.now;
        c.step();
        let Some(t) = c.trips.get(id) else {
            break;
        };
        if t.link() == first {
            stopped |= t.speed() == MmPerTick::ZERO;
        } else if crossed.is_none() {
            crossed = Some(now);
        }
    }
    assert!(stopped, "赤で止まらない");
    let crossed = crossed.expect("青になっても進まない");
    let light = signal::state(&c.net, node, crossed).unwrap().light_for(0);
    assert_eq!(light, Light::Green, "青でないのに入った");
    // 赤のあいだは入らない。青になってから、止まった所から走り出して数 tick で入る。
    let green = start + GREEN + YELLOW + ALL_RED;
    assert!(crossed.0 >= green, "青の前に入った {crossed:?}");
    assert!(crossed.0 <= green + 5, "青になっても待ちすぎた {crossed:?}");
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
        let before: Vec<(TripId, LinkId)> = c.trips.iter().map(|(id, t)| (id, t.link())).collect();
        let now = c.now;
        arrived += c.step().len();
        let st = signal::state(&c.net, node, now).unwrap();
        for (id, link) in before {
            let Some(arm) = incoming.iter().position(|(l, _)| *l == link) else {
                continue;
            };
            if c.trips.get(id).is_none_or(|t| t.link() != link) {
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
