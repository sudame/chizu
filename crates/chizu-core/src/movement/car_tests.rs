use super::*;
use crate::command::{NodeId, RoadKind};
use crate::network::{Dir, Line};
use crate::tile::TileRect;
use crate::units::Mm;

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
        let arrived = self.trips.step(&self.net, self.router.connectors());
        check_spacing(&self.trips);
        arrived
    }
}

/// 同じ車線の車が、車の長さと最小の車間より近づいていないことを確かめる。
fn check_spacing(trips: &Trips) {
    let mut by_lane: Vec<(LaneId, LaneOffset)> = trips
        .iter()
        .filter(|(_, t)| t.mode() == LaneMode::Car)
        .map(|(_, t)| (t.lane(), t.offset()))
        .collect();
    by_lane.sort();
    for w in by_lane.windows(2) {
        if w[0].0 == w[1].0 {
            assert!(w[1].1.0 - w[0].1.0 >= CAR_LENGTH + MIN_GAP, "{w:?}");
        }
    }
}

#[test]
fn a_car_accelerates_to_the_limit_and_stops_at_the_destination() {
    let mut n = Network::new();
    road(&mut n, (0, 0), (100, 0), RoadKind::Small);
    let mut c = Cars::new(n);
    let id = c.drive(at(0, 0, (1, 0), 0), at(89, 0, (1, 0), 8_000));
    // 時速 40km の道路で、1 tick に 2,500mm/tick ずつ速くし、着く位置でちょうど止まる。
    let (mut pos, mut v, mut ticks) = (0_i32, 0_i32, 0);
    while pos < 720_000 {
        v = (v + CAR_ACCEL).min(11_111).min(720_000 - pos);
        pos += v;
        ticks += 1;
    }
    for t in 1..=ticks {
        let arrived = c.step();
        assert_eq!(arrived.contains(&id), t == ticks, "{t}");
        if t == 3 {
            assert_eq!(c.trips.get(id).unwrap().speed(), MmPerTick(7_500));
        }
        if t == 10 {
            assert_eq!(c.trips.get(id).unwrap().speed(), MmPerTick(11_111));
        }
    }
}

#[test]
fn followers_keep_their_distance_and_all_arrive() {
    let mut n = Network::new();
    road(&mut n, (0, 0), (60, 0), RoadKind::Small);
    let mut c = Cars::new(n);
    // 1 マスおきに並べた 10 台が、同じ所へ向かう。
    let ids: Vec<TripId> = (0..10)
        .map(|k| c.drive(at(k, 0, (1, 0), 0), at(55, 0, (1, 0), 0)))
        .collect();
    let mut arrived = Vec::new();
    for _ in 0..200 {
        arrived.extend(c.step());
    }
    arrived.sort();
    let mut want = ids.clone();
    want.sort();
    assert_eq!(arrived, want);
}

#[test]
fn a_car_waits_at_the_node_until_the_next_lane_has_room() {
    let mut n = Network::new();
    road(&mut n, (0, 0), (10, 0), RoadKind::Small);
    road(&mut n, (10, 0), (30, 0), RoadKind::Small);
    let mut c = Cars::new(n);
    let x = c.drive(at(9, 0, (1, 0), 7_000), at(29, 0, (1, 0), 0));
    let y = c.drive(at(10, 0, (1, 0), 1_000), at(29, 0, (1, 0), 0));
    let first = c.trips.get(x).unwrap().link();
    c.step();
    // y は 3,500mm まで進んだだけなので、x は入れずにリンクの終わりで止まる。
    let tx = c.trips.get(x).unwrap();
    assert_eq!(
        (tx.link(), tx.offset(), tx.speed()),
        (first, LaneOffset(80_000), MmPerTick::ZERO)
    );
    assert_eq!(c.trips.get(y).unwrap().offset(), LaneOffset(3_500));
    c.step();
    // y が 8,500mm まで進み、x は y の 7,000mm 後ろの 1,500mm に入る。
    let tx = c.trips.get(x).unwrap();
    assert_ne!(tx.link(), first);
    assert_eq!(tx.offset(), LaneOffset(1_500));
}

#[test]
fn lane_choice_prefers_lanes_that_reach_the_turn_after() {
    let mut n = Network::new();
    road(&mut n, (0, 0), (10, 0), RoadKind::Small);
    road(&mut n, (10, 0), (20, 0), RoadKind::Large);
    road(&mut n, (20, 0), (20, 10), RoadKind::Small);
    road(&mut n, (20, 0), (30, 0), RoadKind::Small);
    let link = |a: (i64, i64), b: (i64, i64)| {
        let sa = SidewalkPos::along(
            TilePos::new(a.0, a.1),
            ((b.0 - a.0).signum() as i8, (b.1 - a.1).signum() as i8),
            LaneOffset(1),
        );
        sa.resolve_on(&n, LaneMode::Car).unwrap().0
    };
    let (west, mid, north) = (
        link((0, 0), (10, 0)),
        link((10, 0), (20, 0)),
        link((20, 0), (20, 10)),
    );
    let from = n.link_lanes(west).next().unwrap();
    let mid_lanes: Vec<LaneId> = n.link_lanes(mid).collect();
    let north_lane = n.link_lanes(north).next().unwrap();
    let node = |x, y| n.node_at(TilePos::new(x, y)).unwrap();
    let c = |node: NodeId, from, to| Connector {
        node,
        from,
        to,
        signal_group: 0,
    };
    // 西のリンクからは真ん中の 3 車線のどれにも入れるが、北へ曲がれるのは 3 番めの車線だけとする。
    let mut conn: Vec<Connector> = mid_lanes.iter().map(|l| c(node(10, 0), from, *l)).collect();
    conn.push(c(node(20, 0), mid_lanes[2], north_lane));
    conn.sort();
    let got = car::choose_lane(&n, &conn, from, node(10, 0), mid, Some(north));
    assert_eq!(got, mid_lanes[2]);
    // 曲がり先へつながる車線がなければ、番号の小さい車線に入る。
    let got = car::choose_lane(&n, &conn[..3], from, node(10, 0), mid, Some(north));
    assert_eq!(got, mid_lanes[0]);
}

#[test]
fn cars_reroute_around_a_removed_road_and_show_as_movers() {
    let mut n = Network::new();
    road(&mut n, (0, 0), (10, 0), RoadKind::Small);
    road(&mut n, (10, 0), (20, 0), RoadKind::Small);
    road(&mut n, (10, 0), (10, 10), RoadKind::Small);
    road(&mut n, (10, 10), (20, 10), RoadKind::Small);
    road(&mut n, (20, 10), (20, 0), RoadKind::Small);
    road(&mut n, (20, 0), (30, 0), RoadKind::Small);
    let mut c = Cars::new(n);
    let id = c.drive(at(0, 0, (1, 0), 0), at(25, 0, (1, 0), 0));
    for _ in 0..3 {
        c.step();
    }
    let speed = c.trips.get(id).unwrap().speed();
    c.net
        .bulldoze(TileRect::from_corners(
            TilePos::new(15, 0),
            TilePos::new(15, 0),
        ))
        .unwrap();
    c.router.rebuild(&c.net);
    assert!(c.trips.reroute(&c.net, &mut c.router).is_empty());
    let t = c.trips.get(id).unwrap();
    assert_eq!(t.speed(), speed);
    assert!(t.route().links.len() >= 5);
    let m = c.trips.movers_in(
        WorldPos::new(Mm(0), Mm(0)),
        WorldPos::new(Mm(80_000), Mm(8_000)),
    );
    assert_eq!(m.len(), 1);
    assert_eq!(
        (m[0].id, m[0].mode, m[0].heading),
        (id, LaneMode::Car, (1, 0))
    );
    let mut arrived = false;
    for _ in 0..300 {
        arrived |= c.step().contains(&id);
    }
    assert!(arrived);
}

/// 格子の道路の上で何台も走らせ、着いた順と状態ハッシュを返す。
fn grid_run() -> (Vec<TripId>, u64) {
    use crate::state_hash::state_hash_of;
    let mut n = Network::new();
    for i in 0..=3 {
        road(&mut n, (0, i * 10), (30, i * 10), RoadKind::Medium);
        road(&mut n, (i * 10, 0), (i * 10, 30), RoadKind::Small);
    }
    let mut c = Cars::new(n);
    let drives: Vec<_> = (0..120_i64)
        .map(|k| {
            let from = at(k % 30, 10 * (k / 30), (1, 0), 0);
            let to = at(10 * ((k + 1) % 4), (k * 7) % 29, (0, 1), 4_000);
            (from, to)
        })
        .collect();
    let ids = c
        .trips
        .spawn_drives(&c.net, &mut c.router, &drives, Tick::ZERO);
    assert!(ids.iter().all(Option::is_some));
    let mut arrived = Vec::new();
    for _ in 0..200 {
        arrived.extend(c.step());
    }
    (arrived, state_hash_of(&c.trips))
}

#[test]
fn cars_do_not_depend_on_the_thread_count() {
    let one = grid_run();
    assert!(one.0.len() > 60, "着いた車が少ない {}", one.0.len());
    #[cfg(feature = "parallel")]
    for threads in [1, 3, 8] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        assert_eq!(pool.install(grid_run), one);
    }
}

#[test]
fn car_positions_resolve_on_the_car_link_of_the_same_side() {
    let mut n = Network::new();
    road(&mut n, (0, 0), (10, 0), RoadKind::Medium);
    let p = at(3, 0, (-1, 0), 2_000);
    let (car, off) = p.resolve_on(&n, LaneMode::Car).unwrap();
    let (walk, woff) = p.resolve(&n).unwrap();
    assert_eq!(
        (car.segment, car.dir, car.mode),
        (walk.segment, Dir::Backward, LaneMode::Car)
    );
    assert_eq!(off, woff);
}
