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
        let arrived = self.trips.step(&self.net, &mut self.router);
        check_spacing(&self.trips);
        arrived
    }
}

/// 同じ車線の車が、車の長さと最小の車間より近づいていないことを確かめる。
fn check_spacing(trips: &Trips) {
    let mut by_lane: Vec<(LaneId, LaneOffset)> = trips
        .iter()
        .filter(|(_, t)| car::on_road(t))
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
fn a_car_accelerates_to_the_limit_and_slows_down_to_the_destination() {
    let mut n = Network::new();
    n.add_highway(TilePos::new(0, 0), TilePos::new(200, 0));
    let mut c = Cars::new(n);
    let id = c.drive(at(0, 0, (1, 0), 0), at(150, 0, (1, 0), 0));
    // 時速 100km まで、1 tick に 2,500mm/tick ずつ速くする。
    let mut speeds = vec![0];
    let mut arrived = false;
    for _ in 0..400 {
        if c.step().contains(&id) {
            arrived = true;
            break;
        }
        speeds.push(c.trips.get(id).unwrap().speed().0);
    }
    assert!(arrived);
    assert_eq!(&speeds[1..4], [2_500, 5_000, 7_500]);
    assert!(speeds.contains(&27_778));
    // 着く位置の手前で、減速の上限を超えずに速さを落とす。
    for w in speeds.windows(2) {
        assert!(w[0] - w[1] <= CAR_DECEL, "{w:?}");
    }
}

#[test]
fn a_car_behind_a_stopped_car_slows_down_gradually() {
    // 時速 100km で、200m 先に止まった車がある。最高速度からでも 1 tick では止まらず、
    // 減速の上限を守って、前の車の 7m 手前までに止まる。
    let leader = LaneOffset(200_000);
    let (mut pos, mut v) = (0_i64, 27_778_i64);
    let mut ticks = 0;
    while v > 0 || ticks == 0 {
        let me = LaneOffset(u32::try_from(pos).unwrap());
        let room = i64::from(leader.0) - i64::from(CAR_LENGTH + MIN_GAP) - pos;
        let next = (v + i64::from(CAR_ACCEL))
            .min(27_778)
            .min(car::safe_speed(me, leader, MmPerTick::ZERO))
            .min(room)
            .max(0);
        assert!(v - next <= i64::from(CAR_DECEL), "{v} から {next}");
        (pos, v) = (pos + next, next);
        ticks += 1;
    }
    assert!(ticks > 5);
    assert!(pos <= i64::from(leader.0 - CAR_LENGTH - MIN_GAP));
}

#[test]
fn followers_keep_their_distance_and_all_arrive() {
    let mut n = Network::new();
    road(&mut n, (0, 0), (60, 0), RoadKind::Small);
    let mut c = Cars::new(n);
    // 同じマスから出た 10 台が、前の車の 7m 後ろずつに詰めて並び、同じ所へ向かう。
    let ids: Vec<TripId> = (0..10)
        .map(|_| c.drive(at(20, 0, (1, 0), 0), at(55, 0, (1, 0), 0)))
        .collect();
    check_spacing(&c.trips);
    let mut arrived = Vec::new();
    for _ in 0..300 {
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
    // y がまだ近いので、x はリンクの終わりを越えずに止まって待つ。
    let tx = c.trips.get(x).unwrap();
    assert_eq!(
        (tx.link(), tx.offset(), tx.speed()),
        (first, LaneOffset(79_000), MmPerTick::ZERO)
    );
    let mut entered = None;
    for t in 2..10 {
        c.step();
        let tx = c.trips.get(x).unwrap();
        if tx.link() != first {
            entered = Some(t);
            break;
        }
        assert!(tx.offset() <= LaneOffset(80_000));
    }
    assert!(entered.is_some(), "空いても入らない");
    let _ = y;
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
    let got = car::choose_lane(
        &n,
        &conn,
        &car::LaneState::default(),
        from,
        node(10, 0),
        mid,
        Some(north),
    );
    assert_eq!(got, mid_lanes[2]);
    // 曲がり先へつながる車線がなければ、番号の小さい車線に入る。
    let got = car::choose_lane(
        &n,
        &conn[..3],
        &car::LaneState::default(),
        from,
        node(10, 0),
        mid,
        Some(north),
    );
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
        &c.net,
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

#[test]
fn a_car_entering_behind_a_held_car_keeps_its_distance() {
    // 短いリンク A の先の C の入口に車がいて、A の終わりの車が待たされているところへ、B から車が入る。
    let mut n = Network::new();
    road(&mut n, (0, 0), (10, 0), RoadKind::Small);
    road(&mut n, (10, 0), (11, 0), RoadKind::Small);
    road(&mut n, (11, 0), (30, 0), RoadKind::Small);
    let mut c = Cars::new(n);
    c.drive(at(11, 0, (1, 0), 500), at(29, 0, (1, 0), 0));
    c.drive(at(10, 0, (1, 0), 7_000), at(29, 0, (1, 0), 0));
    c.drive(at(9, 0, (1, 0), 7_900), at(29, 0, (1, 0), 0));
    let mut arrived = 0;
    for _ in 0..60 {
        arrived += c.step().len();
    }
    assert_eq!(arrived, 3);
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(32))]
    #[test]
    fn queues_at_an_intersection_keep_their_distance(
        west in proptest::collection::btree_set(0_i64..9, 0..9),
        north in proptest::collection::btree_set(0_i64..9, 0..9),
        along in 0_u32..1_000,
    ) {
        // 西と北から来た車が、交差点で東へ合流する。
        let mut n = Network::new();
        road(&mut n, (0, 0), (10, 0), RoadKind::Small);
        road(&mut n, (10, 0), (12, 0), RoadKind::Small);
        road(&mut n, (12, 0), (30, 0), RoadKind::Small);
        road(&mut n, (10, -10), (10, 0), RoadKind::Small);
        let mut c = Cars::new(n);
        let dest = at(29, 0, (1, 0), 0);
        let mut drives: Vec<_> = west.iter().map(|x| (at(*x, 0, (1, 0), along), dest)).collect();
        drives.extend(north.iter().map(|y| (at(10, y - 10, (0, 1), along), dest)));
        let ids = c.trips.spawn_drives(&c.net, &mut c.router, &drives, Tick::ZERO);
        proptest::prop_assert!(ids.iter().all(Option::is_some));
        let mut arrived = 0;
        for _ in 0..200 {
            arrived += c.step().len();
        }
        proptest::prop_assert_eq!(arrived, drives.len());
    }
}

#[test]
fn cars_use_more_than_the_first_lane_on_large_roads() {
    let mut n = Network::new();
    for i in 0..=3 {
        road(&mut n, (0, i * 12), (36, i * 12), RoadKind::Large);
        road(&mut n, (i * 12, 0), (i * 12, 36), RoadKind::Large);
    }
    let mut c = Cars::new(n);
    let drives: Vec<_> = (0..90_i64)
        .map(|k| {
            (
                at(k % 36, 12 * (k % 3), (1, 0), 0),
                at(36, (k * 5) % 36, (0, 1), 0),
            )
        })
        .collect();
    let ids = c
        .trips
        .spawn_drives(&c.net, &mut c.router, &drives, Tick::ZERO);
    assert!(ids.iter().all(Option::is_some));
    let mut used = std::collections::BTreeSet::new();
    for _ in 0..60 {
        c.step();
        for (_, t) in c
            .trips
            .iter()
            .filter(|(_, t)| t.route().links[0] != t.link())
        {
            let k = c
                .net
                .link_lanes(t.link())
                .position(|l| l == t.lane())
                .unwrap();
            used.insert(k);
        }
    }
    assert!(used.len() >= 2, "移ったあとの車線 {used:?}");
}

#[test]
fn mover_shifts_follow_the_lane_each_car_runs_in() {
    let mut n = Network::new();
    road(&mut n, (0, 0), (12, 0), RoadKind::Large);
    let mut c = Cars::new(n);
    // 上り（+x）の車線ごとに 1 台ずつ、下り（-x）に 1 台走らせる。
    for _ in 0..3 {
        c.drive(at(2, 0, (1, 0), 0), at(10, 0, (1, 0), 0));
    }
    c.drive(at(10, 0, (-1, 0), 0), at(2, 0, (-1, 0), 0));
    let min = WorldPos::new(Mm(-8_000), Mm(-8_000));
    let max = WorldPos::new(Mm(160_000), Mm(16_000));
    for _ in 0..40 {
        c.step();
        for m in c.trips.movers_in(&c.net, min, max) {
            assert_eq!(m.mode, LaneMode::Car);
            let t = c.trips.get(m.id).unwrap();
            let k = c
                .net
                .link_lanes(t.link())
                .position(|l| l == t.lane())
                .unwrap();
            // 横のずれは中心線に直交し、左側通行で、車線の番号が小さいほど端（中心線から遠い）にある。
            assert_eq!(m.shift.0, Mm(0));
            let side = if m.heading.0 > 0 { -1 } else { 1 };
            let settled = t.blend.is_none_or(|b| {
                i64::from(t.offset().0) >= i64::from(b.start) + lane_shift::BLEND_MM
            });
            if settled {
                // 大きい道路の車線は 1,120mm 幅で、端の車線の中心は中心線から 2,800mm にある。
                assert_eq!(m.shift.1, Mm(side * (2_800 - 1_120 * k as i64)));
            }
        }
    }
}

#[test]
fn cars_from_the_same_tile_queue_behind_or_wait() {
    let mut n = Network::new();
    road(&mut n, (0, 0), (10, 0), RoadKind::Small);
    let mut c = Cars::new(n);
    let a = c.drive(at(1, 0, (1, 0), 0), at(9, 0, (1, 0), 0));
    let b = c.drive(at(1, 0, (1, 0), 0), at(9, 0, (1, 0), 0));
    check_spacing(&c.trips);
    // 2 台めは前の車の 7m 後ろの 1,000mm に置く。3 台めには空きがないので、出発を待つ。
    assert_eq!(c.trips.get(a).unwrap().offset(), LaneOffset(8_000));
    assert_eq!(c.trips.get(b).unwrap().offset(), LaneOffset(1_000));
    let w = c.drive(at(1, 0, (1, 0), 0), at(9, 0, (1, 0), 0));
    let m = c.trips.movers_in(
        &c.net,
        WorldPos::new(Mm(0), Mm(0)),
        WorldPos::new(Mm(80_000), Mm(8_000)),
    );
    assert_eq!(m.len(), 2);
    let mut arrived = Vec::new();
    for _ in 0..100 {
        arrived.extend(c.step());
    }
    assert!(arrived.contains(&w), "待っていた車が出発しない");
}

#[test]
fn rerouting_keeps_cars_side_by_side_in_their_lanes() {
    let mut n = Network::new();
    road(&mut n, (0, 0), (20, 0), RoadKind::Large);
    road(&mut n, (20, 0), (40, 0), RoadKind::Large);
    road(&mut n, (20, 0), (20, 10), RoadKind::Large);
    road(&mut n, (20, 10), (40, 10), RoadKind::Large);
    road(&mut n, (40, 10), (40, 0), RoadKind::Large);
    road(&mut n, (40, 0), (50, 0), RoadKind::Large);
    let mut c = Cars::new(n);
    let a = c.drive(at(2, 0, (1, 0), 0), at(45, 0, (1, 0), 0));
    let b = c.drive(at(2, 0, (1, 0), 0), at(45, 0, (1, 0), 0));
    let lanes = |c: &Cars| {
        (
            c.trips.get(a).unwrap().lane(),
            c.trips.get(b).unwrap().lane(),
        )
    };
    assert_ne!(lanes(&c).0, lanes(&c).1);
    c.step();
    let before = lanes(&c);
    c.net
        .bulldoze(TileRect::from_corners(
            TilePos::new(30, 0),
            TilePos::new(30, 0),
        ))
        .unwrap();
    c.router.rebuild(&c.net);
    assert!(c.trips.reroute(&c.net, &mut c.router).is_empty());
    check_spacing(&c.trips);
    assert_ne!(lanes(&c).0, lanes(&c).1);
    let _ = before;
    for _ in 0..200 {
        c.step();
    }
    assert!(c.trips.is_empty());
}

#[test]
fn a_car_slows_down_before_a_slower_road() {
    let mut n = Network::new();
    n.add_highway(TilePos::new(0, 0), TilePos::new(100, 0));
    road(&mut n, (0, 0), (0, -40), RoadKind::Small);
    let mut c = Cars::new(n);
    let id = c.drive(at(99, 0, (-1, 0), 0), at(0, -39, (0, -1), 0));
    let mut speeds = vec![0];
    let mut on_small = None;
    for _ in 0..300 {
        if c.step().contains(&id) {
            break;
        }
        let t = c.trips.get(id).unwrap();
        speeds.push(t.speed().0);
        if on_small.is_none() && t.link() != t.route().links[0] {
            on_small = Some(t.speed().0);
        }
    }
    // 細い道路に入るときには、その最高速度まで落としてある。
    assert!(on_small.unwrap() <= 11_111);
    for w in speeds.windows(2) {
        assert!(w[0] - w[1] <= CAR_DECEL, "{w:?}");
    }
}

#[test]
fn cars_pull_out_and_park_on_either_side_of_the_road() {
    // 行き止まりの道で、東向きの側の建物から、西の東向きの側の建物へ向かう。
    // 同じ側に限ると、東の端で折り返し、西の端でもう一度折り返して戻る。どちらの側からも
    // 出て着けるなら、西向きの車線を 1 本走るだけで着く。
    let mut n = Network::new();
    road(&mut n, (0, 0), (20, 0), RoadKind::Small);
    let mut c = Cars::new(n);
    let id = c.drive(at(10, 0, (1, 0), 0), at(3, 0, (1, 0), 0));
    let t = c.trips.get(id).unwrap();
    assert_eq!(t.route().links.len(), 1);
    assert_eq!(t.link().dir, Dir::Backward);
    let mut ticks = 0;
    while c.trips.get(id).is_some() {
        c.step();
        ticks += 1;
        assert!(ticks < 100, "着かない");
    }
}

#[test]
fn a_car_keeps_its_side_when_it_drives_toward_the_destination() {
    // 同じ側のまま前へ進めば着くときは、向きを変えない。
    let mut n = Network::new();
    road(&mut n, (0, 0), (20, 0), RoadKind::Small);
    let mut c = Cars::new(n);
    let id = c.drive(at(3, 0, (1, 0), 0), at(10, 0, (1, 0), 0));
    let t = c.trips.get(id).unwrap();
    assert_eq!(t.route().links.len(), 1);
    assert_eq!(t.link().dir, Dir::Forward);
}

/// 範囲の中の移動体の、中心線の上の位置とずれ。
type Drawn = std::collections::BTreeMap<crate::id::TripId, ((i64, i64), (i64, i64))>;

fn drawn(c: &Cars) -> Drawn {
    let min = WorldPos::new(Mm(-800_000), Mm(-800_000));
    let max = WorldPos::new(Mm(800_000), Mm(800_000));
    c.trips
        .movers_in(&c.net, min, max)
        .into_iter()
        .map(|m| (m.id, ((m.pos.x.0, m.pos.y.0), (m.shift.0.0, m.shift.1.0))))
        .collect()
}

#[test]
fn lane_shifts_follow_the_travelled_distance_across_road_kinds_and_turns() {
    // 大きい道路から小さい道路へ移り、2 回曲がる。道路の種類が変わると車線の幅も数も変わる。
    let mut n = Network::new();
    road(&mut n, (0, 0), (12, 0), RoadKind::Large);
    road(&mut n, (12, 0), (12, 10), RoadKind::Medium);
    road(&mut n, (12, 10), (2, 10), RoadKind::Small);
    road(&mut n, (2, 10), (2, 14), RoadKind::Large);
    let mut c = Cars::new(n);
    for k in 0..6_i64 {
        c.drive(at(1 + k % 2, 0, (1, 0), 0), at(2, 13, (0, 1), 0));
    }
    let mut before = drawn(&c);
    let mut samples = 0_usize;
    let mut turned = false;
    for _ in 0..600 {
        c.step();
        let after = drawn(&c);
        for (id, (pos, shift)) in &after {
            let Some((pos0, shift0)) = before.get(id) else {
                continue;
            };
            let dist = (pos.0 - pos0.0).abs().max((pos.1 - pos0.1).abs());
            let dshift = (shift.0 - shift0.0).abs().max((shift.1 - shift0.1).abs());
            // 横のずれの変わり方は、進んだ距離に比べて高々 3 倍である（3m で移りきる）。
            // 車線が飛べば、進んだ距離が短いときにこれを超える。
            assert!(
                dshift <= 3 * dist + 50,
                "{id:?} が 1 tick に {dist}mm 進むあいだに、ずれが {dshift}mm 変わった"
            );
            samples += 1;
            turned |= c.trips.get(*id).is_some_and(|t| t.leg >= 2);
        }
        before = after;
    }
    assert!(samples > 100 && turned, "走った車が少ない {samples}");
}

#[test]
fn a_running_car_keeps_its_direction_when_it_reroutes() {
    // 四角い輪の下の辺を東へ走る車の前で、右下の区間を消して下の辺を行き止まりにする。
    // 反対の車線へ移れるなら西へ戻るのがいちばん早いが、走っている車はその場で向きを変えられない。
    // 行き止まりまで進んで折り返す。
    let mut n = Network::new();
    road(&mut n, (0, 0), (14, 0), RoadKind::Small);
    road(&mut n, (14, 0), (20, 0), RoadKind::Small);
    road(&mut n, (0, 0), (0, 10), RoadKind::Small);
    road(&mut n, (0, 10), (20, 10), RoadKind::Small);
    road(&mut n, (20, 10), (20, 0), RoadKind::Small);
    let mut c = Cars::new(n);
    let id = c.drive(at(4, 0, (1, 0), 0), at(20, 5, (0, -1), 0));
    for _ in 0..3 {
        c.step();
    }
    c.net
        .bulldoze(TileRect::from_corners(
            TilePos::new(15, 0),
            TilePos::new(15, 0),
        ))
        .unwrap();
    c.router.rebuild(&c.net);
    assert!(c.trips.reroute(&c.net, &mut c.router).is_empty());
    let t = c.trips.get(id).unwrap();
    assert_eq!(t.link().dir, Dir::Forward);
    assert_eq!(t.position().step, (1, 0));
    let mut arrived = false;
    for _ in 0..600 {
        arrived |= c.step().contains(&id);
    }
    assert!(arrived);
}

/// 先頭の車を動かさないことで、後ろの車を詰まらせて止める。先頭の車は、1 tick ごとに元の状態へ戻す。
fn step_with_lead_pinned(c: &mut Cars, lead: TripId) {
    let pinned = c.trips.get(lead).expect("先頭の車がない").clone();
    c.step();
    c.trips.rows[lead.slot()] = Some(pinned);
}

/// 詰まって止まった車は、そこに交差する道路が引かれて車線の ID が変わっても、止まったままと数える。
#[test]
fn stalled_cars_stay_stalled_when_the_road_is_split() {
    use crate::world::CarSample;

    let mut n = Network::new();
    road(&mut n, (0, 0), (60, 0), RoadKind::Small);
    let mut c = Cars::new(n);
    let lead = c.drive(at(40, 0, (1, 0), 0), at(59, 0, (1, 0), 0));
    let followers: Vec<TripId> = (0..3)
        .map(|i| c.drive(at(5 * i, 0, (1, 0), 0), at(59, 0, (1, 0), 0)))
        .collect();
    for _ in 0..150 {
        step_with_lead_pinned(&mut c, lead);
    }
    let before = CarSample::of(&c.trips);
    assert_eq!(before.len(), 4);
    // 先頭の車に追いついて止まっている。動いている車は 1 台もない。
    for _ in 0..20 {
        step_with_lead_pinned(&mut c, lead);
    }
    let still = CarSample::of(&c.trips);
    assert_eq!(still.unmoved_since(&before), 4, "詰まった車が動いている");

    // 止まっている後ろの車の真ん中に、道路を交差させて区間を分ける。
    let lanes: Vec<_> = followers
        .iter()
        .map(|id| c.trips.get(*id).unwrap().lane())
        .collect();
    let x = c.trips.get(followers[1]).unwrap().position().tile.x;
    road(&mut c.net, (x, -5), (x, 5), RoadKind::Small);
    c.router.rebuild(&c.net);
    assert!(c.trips.reroute(&c.net, &mut c.router).is_empty());
    for _ in 0..40 {
        step_with_lead_pinned(&mut c, lead);
    }
    let after: Vec<_> = followers
        .iter()
        .map(|id| c.trips.get(*id).unwrap().lane())
        .collect();
    assert_ne!(lanes, after, "区間が分けられても車線が変わらない");
    let later = CarSample::of(&c.trips);
    assert_eq!(
        later.unmoved_since(&before),
        4,
        "分けられた区間で止まった車が動いた扱いになった"
    );
}
