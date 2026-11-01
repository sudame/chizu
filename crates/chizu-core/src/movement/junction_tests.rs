use super::junction::{Move, PATIENCE};
use super::*;
use crate::command::RoadKind;
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

/// マス `a` から隣のマス `b` へ向かう車のリンク。
fn link(n: &Network, a: (i64, i64), b: (i64, i64)) -> LinkId {
    let step = ((b.0 - a.0).signum() as i8, (b.1 - a.1).signum() as i8);
    at(a.0, a.1, step, 1)
        .resolve_on(n, LaneMode::Car)
        .unwrap()
        .0
}

/// (10, 10) で十字に交わる、小さい道路の網。
fn crossroads() -> Network {
    let mut n = Network::new();
    road(&mut n, (0, 10), (20, 10), RoadKind::Small);
    road(&mut n, (10, 0), (10, 20), RoadKind::Small);
    n
}

#[test]
fn moves_conflict_when_their_paths_cross_or_merge() {
    let n = crossroads();
    let c = (10, 10);
    let into = |a| link(&n, a, c);
    let out = |b| link(&n, c, b);
    let mv = |a, b| Move::new(&n, into(a), out(b));
    let (w, e, north, s) = ((9, 10), (11, 10), (10, 9), (10, 11));
    let straight = mv(w, e);
    // 直交する直進どうしは交わり、対向の直進どうしは交わらない。
    assert!(straight.conflicts(&mv(north, s)));
    assert!(!straight.conflicts(&mv(e, w)));
    // 左側通行なので、東へ向かう車にとって北への曲がりは左折で、対向の直進をまたがない。
    // 南への曲がり（右折）は対向の直進と交わる。
    assert!(!mv(w, north).conflicts(&mv(e, w)));
    assert!(mv(w, s).conflicts(&mv(e, w)));
    // 同じリンクへ入る動きは合流として衝突する。同じリンクから出る動きどうしは衝突しない。
    assert!(mv(w, north).conflicts(&mv(s, north)));
    assert!(!mv(w, north).conflicts(&mv(w, s)));
    // 衝突は対称である。
    for a in [w, e, north, s] {
        for b in [w, e, north, s].into_iter().filter(|b| *b != a) {
            for x in [w, e, north, s] {
                for y in [w, e, north, s].into_iter().filter(|y| *y != x) {
                    assert_eq!(mv(a, b).conflicts(&mv(x, y)), mv(x, y).conflicts(&mv(a, b)));
                }
            }
        }
    }
}

struct Cars {
    net: Network,
    router: AstarRouter,
    trips: Trips,
}

impl Cars {
    fn new(net: Network) -> Cars {
        let router = AstarRouter::new(&net);
        Cars {
            net,
            router,
            trips: Trips::default(),
        }
    }

    fn drive(&mut self, from: SidewalkPos, to: SidewalkPos) -> TripId {
        self.trips
            .spawn_drives(&self.net, &mut self.router, &[(from, to)], Tick::ZERO)[0]
            .unwrap()
    }

    fn step(&mut self) -> Vec<TripId> {
        self.trips.step(&self.net, &mut self.router)
    }
}

#[test]
fn a_car_on_the_minor_road_yields_to_the_main_road() {
    // 中くらいの道路を東へ走る車と、北から来て東へ合流する小さい道路の車。
    let mut n = Network::new();
    road(&mut n, (0, 10), (30, 10), RoadKind::Medium);
    road(&mut n, (10, 0), (10, 10), RoadKind::Small);
    let mut c = Cars::new(n);
    let main = c.drive(at(5, 10, (1, 0), 0), at(25, 10, (1, 0), 0));
    let side = c.drive(at(10, 8, (0, 1), 4_000), at(25, 10, (1, 0), 0));
    let (main_first, side_first) = (
        c.trips.get(main).unwrap().link(),
        c.trips.get(side).unwrap().link(),
    );
    let mut order = Vec::new();
    for _ in 0..40 {
        c.step();
        for (id, first) in [(main, main_first), (side, side_first)] {
            if c.trips.get(id).is_some_and(|t| t.link() != first) && !order.contains(&id) {
                order.push(id);
            }
        }
    }
    // 小さい道路の車は、先に着いていても中くらいの道路の車を先に通す。
    assert_eq!(order, [main, side]);
}

#[test]
fn a_minor_road_car_gets_in_after_waiting_long_enough() {
    // 大きい道路の車が途切れなく来ても、小さい道路の車は待ちすぎたら入る。
    let mut n = Network::new();
    road(&mut n, (0, 10), (60, 10), RoadKind::Large);
    road(&mut n, (30, 0), (30, 10), RoadKind::Small);
    let mut c = Cars::new(n);
    let feed = |c: &mut Cars| {
        // 大きい道路の始まりに、毎 tick 2 台ずつ車を出し続ける。
        let d = [(at(0, 10, (1, 0), 0), at(55, 10, (1, 0), 0)); 2];
        c.trips.spawn_drives(&c.net, &mut c.router, &d, Tick::ZERO);
    };
    for _ in 0..60 {
        feed(&mut c);
        c.step();
    }
    let side = c.drive(at(30, 8, (0, 1), 4_000), at(55, 10, (1, 0), 0));
    let first = c.trips.get(side).unwrap().link();
    let mut entered = None;
    for tick in 0..200 {
        feed(&mut c);
        c.step();
        if entered.is_none() && c.trips.get(side).is_none_or(|t| t.link() != first) {
            entered = Some(tick);
        }
    }
    let entered = entered.expect("小さい道路の車が入らない");
    assert!(entered >= PATIENCE as usize, "待たずに入った {entered}");
    assert!(entered <= PATIENCE as usize + 10, "待ちすぎた {entered}");
}

/// 十字路の 4 つの腕のうち `arms` から、それぞれ `per_arm` 台が直進して向こうの腕へ抜ける。
/// 着いた台数と、どれかの腕の手前で止まって並んだ車のいちばん多い台数を返す。
fn heavy_crossroads(arms: &[(i64, i64)], per_arm: usize) -> (usize, usize) {
    let mut c = Cars::new(crossroads());
    let mut drives = Vec::new();
    let mut incoming = Vec::new();
    for &(dx, dy) in arms {
        let step = (-dx as i8, -dy as i8);
        let from = at(10 + 10 * dx, 10 + 10 * dy, step, 0);
        let to = at(10 - 9 * dx, 10 - 9 * dy, step, 0);
        drives.extend(std::iter::repeat_n((from, to), per_arm));
        incoming.push(link(&c.net, (10 + dx, 10 + dy), (10, 10)));
    }
    let ids = c
        .trips
        .spawn_drives(&c.net, &mut c.router, &drives, Tick::ZERO);
    assert!(ids.iter().all(Option::is_some));
    let (mut arrived, mut longest) = (0, 0);
    for _ in 0..1_500 {
        arrived += c.step().len();
        for l in &incoming {
            let stopped = c
                .trips
                .iter()
                .filter(|(_, t)| car::on_road(t) && t.link() == *l && t.speed() == MmPerTick::ZERO)
                .count();
            longest = longest.max(stopped);
        }
        // 交差点の中に、交わる向きの車が同時にいない。
        let inside: Vec<Move> = c
            .trips
            .iter()
            .filter(|(_, t)| car::on_road(t) && t.leg > 0 && t.offset().0 < CAR_LENGTH + MIN_GAP)
            .filter(|(_, t)| {
                let prev = t.route().links[t.leg as usize - 1];
                incoming.contains(&prev)
            })
            .map(|(_, t)| Move::new(&c.net, t.route().links[t.leg as usize - 1], t.link()))
            .collect();
        for a in &inside {
            assert!(
                !inside.iter().any(|b| a.conflicts(b)),
                "交差点の中で交わった"
            );
        }
    }
    (arrived, longest)
}

#[test]
fn crossing_traffic_queues_before_the_intersection() {
    let (arrived, longest) = heavy_crossroads(&[(-1, 0), (0, -1)], 25);
    assert_eq!(arrived, 50);
    assert!(longest >= 3, "列ができない {longest}");
}

#[test]
fn traffic_from_every_arm_does_not_deadlock() {
    let (arrived, longest) = heavy_crossroads(&[(-1, 0), (0, -1), (1, 0), (0, 1)], 20);
    assert_eq!(arrived, 80);
    assert!(longest >= 3, "列ができない {longest}");
}

#[test]
fn opposing_straight_traffic_does_not_wait() {
    // 対向の直進どうしは衝突しないので、どちらの腕でも止まらずに抜ける。
    let (arrived, longest) = heavy_crossroads(&[(-1, 0), (1, 0)], 1);
    assert_eq!(arrived, 2);
    assert_eq!(longest, 0);
}
