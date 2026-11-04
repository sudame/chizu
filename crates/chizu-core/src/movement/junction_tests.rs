use super::junction::{GIVE_UP, Move, PATIENCE, Rescues};
use super::*;
use crate::command::RoadKind;
use crate::network::Line;
use crate::tile::TileRect;

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
    /// 減速の上限を越えて速さを落とした回数。
    hard_brakes: usize,
}

impl Cars {
    fn new(net: Network) -> Cars {
        let router = AstarRouter::new(&net);
        Cars {
            net,
            router,
            trips: Trips::default(),
            hard_brakes: 0,
        }
    }

    fn drive(&mut self, from: SidewalkPos, to: SidewalkPos) -> TripId {
        self.trips
            .spawn_drives(&self.net, &mut self.router, &[(from, to)], Tick::ZERO)[0]
            .unwrap()
    }

    /// 1 tick 進め、減速の上限を越えて速さを落とした車を数える。
    fn step(&mut self) -> Vec<TripId> {
        let before: Vec<(TripId, *const Route, i32)> = self
            .trips
            .iter()
            .filter(|(_, t)| car::on_road(t))
            .map(|(id, t)| (id, std::ptr::from_ref(t.route()), t.speed().0))
            .collect();
        let arrived = self.trips.step(&self.net, &mut self.router, Tick::ZERO);
        for (id, route, v) in before {
            let Some(t) = self.trips.get(id) else {
                continue;
            };
            if car::on_road(t) && std::ptr::eq(t.route(), route) && v - t.speed().0 > CAR_DECEL {
                self.hard_brakes += 1;
            }
        }
        arrived
    }
}

#[test]
fn a_car_on_the_minor_road_yields_to_the_main_road() {
    // 中くらいの道路を東へ走る車と、北から来て右へ曲がり、西へ向かう小さい道路の車。右折は東へ走る車の
    // 道筋と、どの車線を走っていても交わる。
    let mut n = Network::new();
    road(&mut n, (0, 10), (30, 10), RoadKind::Medium);
    road(&mut n, (10, 0), (10, 10), RoadKind::Small);
    let mut c = Cars::new(n);
    let main = c.drive(at(5, 10, (1, 0), 0), at(25, 10, (1, 0), 0));
    let side = c.drive(at(10, 8, (0, 1), 4_000), at(2, 10, (-1, 0), 0));
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
    assert_eq!(c.trips.rescues(), Default::default(), "救済が働いた");
    assert_eq!(c.hard_brakes, 0, "減速の上限を越えた");
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

/// 車 `id` を、経路の中のリンク `link` の位置 `offset` に止めて置き直す。
fn put(c: &mut Cars, id: TripId, link: LinkId, offset: u32) {
    let t = c.trips.rows[id.slot()].as_mut().unwrap();
    let leg = t.route.links.iter().position(|l| *l == link).unwrap();
    t.leg = u32::try_from(leg).unwrap();
    let lane = c.net.link_lanes(link).next().unwrap();
    t.enter(&c.net, LaneOffset(offset), Some(lane));
    t.speed = MmPerTick::ZERO;
    t.waiting = false;
}

/// 東西の道 1 本と、x=10 と x=11 の南北の道 2 本で、隣り合う交差点 A=(10, 10) と B=(11, 10) を作る。
fn twin_junctions() -> Network {
    let mut n = Network::new();
    road(&mut n, (0, 10), (20, 10), RoadKind::Small);
    road(&mut n, (10, 0), (10, 20), RoadKind::Small);
    road(&mut n, (11, 0), (11, 20), RoadKind::Small);
    n
}

#[test]
fn cars_stuck_inside_adjacent_junctions_are_passed_after_a_while() {
    // A と B のあいだのリンクの両端に、互いに相手の交差点の中で止まった車を置く。
    // ノードをまたぐときの空きの確かめでは起きない並びだが、起きても待ちすぎた車が入って解ける。
    let mut c = Cars::new(twin_junctions());
    let (a, b) = ((10, 10), (11, 10));
    let ab = link(&c.net, a, b);
    let ba = link(&c.net, b, a);
    let h1 = c.drive(at(10, 10, (1, 0), 7_000), at(11, 15, (0, 1), 0));
    let p2 = c.drive(at(5, 10, (1, 0), 0), at(15, 10, (1, 0), 0));
    let h2 = c.drive(at(11, 10, (-1, 0), 7_000), at(10, 5, (0, -1), 0));
    let q2 = c.drive(at(15, 10, (-1, 0), 0), at(5, 10, (-1, 0), 0));
    put(&mut c, h1, ab, 8_000);
    put(&mut c, p2, ab, 500);
    put(&mut c, h2, ba, 8_000);
    put(&mut c, q2, ba, 500);
    let mut arrived = 0;
    for _ in 0..(GIVE_UP as usize + 200) {
        arrived += c.step().len();
    }
    assert_eq!(arrived, 4, "行き詰まった");
}

#[test]
fn cars_driving_into_each_others_exits_do_not_lock_up() {
    // A で南へ、B で北へ曲がる車と、A と B を東西に抜ける車を、両方の向きから続けて出す。
    let mut c = Cars::new(twin_junctions());
    let d = [
        (at(0, 10, (1, 0), 0), at(11, 18, (0, 1), 0)),
        (at(0, 10, (1, 0), 0), at(19, 10, (1, 0), 0)),
        (at(20, 10, (-1, 0), 0), at(10, 2, (0, -1), 0)),
        (at(20, 10, (-1, 0), 0), at(1, 10, (-1, 0), 0)),
        (at(10, 0, (0, 1), 0), at(19, 10, (1, 0), 0)),
        (at(11, 20, (0, -1), 0), at(1, 10, (-1, 0), 0)),
    ];
    let mut total = 0;
    for _ in 0..30 {
        total += c
            .trips
            .spawn_drives(&c.net, &mut c.router, &d, Tick::ZERO)
            .iter()
            .flatten()
            .count();
        c.step();
    }
    for _ in 0..3_000 {
        c.step();
    }
    assert_eq!(total, 180);
    assert!(c.trips.is_empty(), "残った {}", c.trips.len());
    assert_eq!(c.trips.rescues().teleported, 0, "詰まりの救済が働いた");
    // 前の車がノードを抜けて車線の先頭になったとたんに止まりきれない車どうしが、同じリンクへ
    // 合流することがある。その後ろの車だけは、減速の上限を越えて止まる。
    assert!(c.hard_brakes <= 10, "減速の上限を越えた {}", c.hard_brakes);
}

/// 小さい道路を東西と南北に `lines` の位置へ引いた格子で、外周から擬似乱数の行き先へ
/// `every` tick ごとに `per` 台ずつ `ticks` tick 出す。着かずに残った台数と、救済の回数を返す。
fn random_grid(lines: [i64; 3], (per, every): (usize, usize), ticks: usize) -> (usize, Rescues) {
    let mut n = Network::new();
    for v in lines {
        road(&mut n, (0, v), (30, v), RoadKind::Small);
        road(&mut n, (v, 0), (v, 30), RoadKind::Small);
    }
    let mut c = Cars::new(n);
    let mut ends = Vec::new();
    for v in lines {
        ends.push(at(0, v, (1, 0), 0));
        ends.push(at(30, v, (-1, 0), 0));
        ends.push(at(v, 0, (0, 1), 0));
        ends.push(at(v, 30, (0, -1), 0));
    }
    let mut s: u64 = 12_345;
    let mut rnd = || {
        s = s
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        usize::try_from(s >> 33).unwrap()
    };
    for tick in 0..ticks {
        let mut d = Vec::new();
        for _ in 0..if tick % every == 0 { per } else { 0 } {
            let a = rnd() % ends.len();
            let mut b = rnd() % ends.len();
            if b == a {
                b = (b + 1) % ends.len();
            }
            // 外周の入口から、その道を 29 マス進んだ所を行き先にする。
            let to = ends[b];
            let far = |p: i64, step: i8| p + 29 * i64::from(step);
            d.push((
                ends[a],
                at(
                    far(to.tile.x, to.step.0),
                    far(to.tile.y, to.step.1),
                    to.step,
                    0,
                ),
            ));
        }
        c.trips.spawn_drives(&c.net, &mut c.router, &d, Tick::ZERO);
        c.step();
    }
    for _ in 0..30_000 {
        c.step();
    }
    (c.trips.len(), c.trips.rescues())
}

#[test]
fn a_dense_grid_with_heavy_random_traffic_drains() {
    // 毎 tick 4 台では出ていく車線の詰まりが輪になり、`RESCUE` でほどけて抜ける。
    // 交差点で譲らせない版でも、出す期間を変えると同じように輪になって止まる。
    let (left, rescues) = random_grid([10, 11, 12], (4, 1), 600);
    assert_eq!(left, 0);
    assert!(rescues.teleported > 0);
}

#[test]
fn a_sparse_grid_with_heavy_random_traffic_drains() {
    let (left, rescues) = random_grid([10, 13, 16], (4, 1), 600);
    assert_eq!(left, 0);
    assert!(rescues.teleported > 0);
}

#[test]
fn light_random_traffic_needs_no_rescue() {
    // 2 tick に 1 台なら、どちらの格子でも救済なしに抜ける。
    for lines in [[10, 11, 12], [10, 13, 16]] {
        let (left, rescues) = random_grid(lines, (1, 2), 600);
        assert_eq!(left, 0);
        assert_eq!(rescues, Rescues::default(), "{lines:?}");
    }
}

#[test]
fn a_gridlock_without_junctions_is_rescued() {
    // 四角い環状の道路の 4 つの角は、2 本の区間をつなぐだけのノードで、交差点ではない。
    // 両方の向きの車線を、次の辺へ向かう車で埋めると、どの辺の先頭も次の辺に入れず止まる。
    let mut n = Network::new();
    let corners = [(0, 0), (10, 0), (10, 10), (0, 10)];
    for k in 0..4 {
        road(&mut n, corners[k], corners[(k + 1) % 4], RoadKind::Small);
    }
    assert!(n.nodes().all(|(id, _)| !junction::is_junction(&n, id)));
    let mut c = Cars::new(n);
    let step =
        |p: (i64, i64), q: (i64, i64)| ((q.0 - p.0).signum() as i8, (q.1 - p.1).signum() as i8);
    // 角 `p` から向き `s` へ `mm` 進んだ位置。
    let along = |p: (i64, i64), s: (i8, i8), mm: u32| {
        let tiles = i64::from(mm / 8_000);
        at(
            p.0 + tiles * i64::from(s.0),
            p.1 + tiles * i64::from(s.1),
            s,
            mm % 8_000,
        )
    };
    let mut drives = Vec::new();
    for k in 0..4 {
        let (prev, a, b, next) = (
            corners[(k + 3) % 4],
            corners[k],
            corners[(k + 1) % 4],
            corners[(k + 2) % 4],
        );
        // 辺の長さ 80m に、7m おきに隙間なく並べる。
        for mm in (1_000..80_000).step_by(7_000) {
            // 時計回りの車は次の辺へ、反時計回りの車は前の辺へ向かう。
            drives.push((along(a, step(a, b), mm), along(b, step(b, next), mm)));
            drives.push((along(b, step(b, a), mm), along(a, step(a, prev), mm)));
        }
    }
    let ids = c
        .trips
        .spawn_drives(&c.net, &mut c.router, &drives, Tick::ZERO);
    assert!(ids.iter().all(Option::is_some));
    let total = c.trips.len();
    for _ in 0..junction::RESCUE {
        c.step();
    }
    // 救済の待ちが過ぎるまでは、詰まったまま誰も消えない。
    assert_eq!(c.trips.rescues().teleported, 0);
    assert!(
        c.trips.len() > total / 2,
        "詰まらなかった {}",
        c.trips.len()
    );
    for _ in 0..2_000 {
        c.step();
    }
    assert!(c.trips.is_empty(), "残った {}", c.trips.len());
    assert!(c.trips.rescues().teleported > 0);
}

#[test]
fn a_car_just_past_a_junction_survives_the_removal_of_the_road_behind_it() {
    // 交差点を抜けたばかりの車の、抜けてきた区間を消す。残りの経路は無事なので探し直されず、
    // 経路には消えた区間が残る。それでも交差点の中かを調べるところで止まらない。
    let mut c = Cars::new(crossroads());
    // 止まった状態から走り出し、ノードをまたいだ tick に 7m より手前に入る位置から出す。
    let id = c.drive(at(4, 10, (1, 0), 7_000), at(18, 10, (1, 0), 0));
    while c.trips.get(id).unwrap().leg == 0 {
        c.step();
    }
    assert!(c.trips.get(id).unwrap().offset().0 < CAR_LENGTH + MIN_GAP);
    c.net
        .bulldoze(TileRect::from_corners(
            TilePos::new(7, 10),
            TilePos::new(7, 10),
        ))
        .unwrap();
    c.router.rebuild(&c.net);
    assert!(c.trips.reroute(&c.net, &mut c.router).is_empty());
    let t = c.trips.get(id).unwrap();
    assert_eq!(t.leg, 1, "探し直された");
    assert!(c.net.segment(t.route().links[0].segment).is_none());
    let mut arrived = false;
    for _ in 0..200 {
        arrived |= c.step().contains(&id);
    }
    assert!(arrived);
}

#[test]
fn a_car_that_waits_past_the_rescue_walks_instead() {
    use super::parking::Then;
    let mut c = Cars::new(crossroads());
    let id = c.drive(at(4, 10, (1, 0), 7_000), at(18, 10, (1, 0), 0));
    let slot = id.slot();
    let to = at(18, 10, (1, 0), 0);
    let lot = crate::id::BuildingId {
        index: 0,
        generation: 0,
    };
    {
        let t = c.trips.rows[slot].as_mut().unwrap();
        t.waiting = true;
        t.then = Then::Walk { lot, to };
        t.stalled = junction::RESCUE;
        assert!(!car::given_up(t));
        t.stalled = junction::RESCUE + 1;
        assert!(car::given_up(t));
    }
    let mut ended = Vec::new();
    c.trips
        .walk_instead(&c.net, &mut c.router, vec![slot], &mut ended);
    assert!(ended.is_empty());
    let t = c.trips.get(id).unwrap();
    assert_eq!(t.mode(), LaneMode::Sidewalk);
    assert!(!car::given_up(t));
    assert_eq!(c.trips.rescues().walked_instead, 1);
    assert_eq!(c.trips.rescues().teleported, 0);
}

#[test]
fn a_waiting_car_that_gets_a_place_starts_counting_again() {
    let mut c = Cars::new(crossroads());
    let id = c.drive(at(4, 10, (1, 0), 7_000), at(18, 10, (1, 0), 0));
    {
        let t = c.trips.rows[id.slot()].as_mut().unwrap();
        t.waiting = true;
        t.stalled = junction::RESCUE;
    }
    c.step();
    let t = c.trips.get(id).unwrap();
    assert!(!t.waiting);
    assert_eq!(t.stalled, 0);
}

#[test]
fn cars_turning_into_the_same_road_from_both_sides_take_separate_lanes() {
    // 南北の中くらいの道路（片側 2 車線）に、西と東から小さい道路が突き当たる。西の車は右へ、東の車は左へ
    // 曲がって、どちらも南へ向かう。左へ曲がる車は道路の端の車線に、右へ曲がる車は中央寄りの車線に入り、
    // 同じリンクへ同時に入れる。
    let mut n = Network::new();
    road(&mut n, (10, 0), (10, 30), RoadKind::Medium);
    road(&mut n, (0, 10), (10, 10), RoadKind::Small);
    road(&mut n, (10, 10), (20, 10), RoadKind::Small);
    let mut c = Cars::new(n);
    let to = at(10, 25, (0, 1), 0);
    let west = c.drive(at(8, 10, (1, 0), 4_000), to);
    let east = c.drive(at(12, 10, (-1, 0), 4_000), to);
    let south = link(&c.net, (10, 10), (10, 11));
    let lanes: Vec<LaneId> = c.net.link_lanes(south).collect();
    let mut entered = [None; 2];
    for tick in 0..60_u32 {
        c.step();
        for (k, id) in [west, east].into_iter().enumerate() {
            let Some(t) = c.trips.get(id) else { continue };
            if entered[k].is_none() && car::on_road(t) && t.link() == south {
                entered[k] = Some((tick, t.lane()));
            }
        }
    }
    let [Some((tw, lw)), Some((te, le))] = entered else {
        panic!("南へ入らない {entered:?}");
    };
    assert!(tw.abs_diff(te) <= 1, "片方が待った {tw} {te}");
    assert_eq!((le, lw), (lanes[0], lanes[1]));
    assert_eq!(c.trips.rescues(), Rescues::default());
    assert_eq!(c.hard_brakes, 0, "減速の上限を越えた");
}

/// (10, 10) で十字に交わる、中くらいの道路（片側 2 車線）の網。
fn medium_crossroads() -> Network {
    let mut n = Network::new();
    road(&mut n, (0, 10), (20, 10), RoadKind::Medium);
    road(&mut n, (10, 0), (10, 20), RoadKind::Medium);
    n
}

/// 十字路の腕（中心から見た隣のマスへのずれ）。
const ARMS: [(i64, i64); 4] = [(-1, 0), (1, 0), (0, -1), (0, 1)];

/// 腕 `a` から入って腕 `b` へ出る、出ていくリンクの `lane` 番の車線へ入る動き。
fn lane_move(n: &Network, a: (i64, i64), b: (i64, i64), lane: usize) -> Move {
    let c = (10, 10);
    let from = link(n, (c.0 + a.0, c.1 + a.1), c);
    let to = link(n, c, (c.0 + b.0, c.1 + b.1));
    let l = n.link_lanes(to).nth(lane).unwrap();
    Move::into_lane(n, from, to, l)
}

#[test]
fn lane_conflicts_are_symmetric() {
    let n = medium_crossroads();
    let mut moves = Vec::new();
    for a in ARMS {
        for b in ARMS.into_iter().filter(|b| *b != a) {
            for lane in 0..2 {
                moves.push(lane_move(&n, a, b, lane));
            }
        }
    }
    for x in &moves {
        for y in &moves {
            assert_eq!(x.conflicts_in(y), y.conflicts_in(x), "{x:?} {y:?}");
        }
    }
}

#[test]
fn turns_into_separate_lanes_conflict_only_when_their_paths_cross() {
    let n = medium_crossroads();
    let (w, e, north, s) = ((-1, 0), (1, 0), (0, -1), (0, 1));
    // 東から左へ曲がって南へ、西から右へ曲がって南へ。左折が端の車線、右折が中央寄りの車線なら交わらない。
    assert!(!lane_move(&n, e, s, 0).conflicts_in(&lane_move(&n, w, s, 1)));
    // 左折が中央寄り、右折が端の車線へ入ると、道筋が交わる。2 車線から 2 車線へ曲がるときに起きうる。
    assert!(lane_move(&n, e, s, 1).conflicts_in(&lane_move(&n, w, s, 0)));
    assert!(lane_move(&n, w, s, 0).conflicts_in(&lane_move(&n, e, s, 1)));
    // 北からの直進と東からの左折も、車線の並びが入口の並びと逆なら交わる。
    assert!(lane_move(&n, north, s, 0).conflicts_in(&lane_move(&n, e, s, 1)));
    assert!(!lane_move(&n, north, s, 1).conflicts_in(&lane_move(&n, e, s, 0)));
    // 同じ車線へ入れば合流する。
    assert!(lane_move(&n, north, s, 1).conflicts_in(&lane_move(&n, e, s, 1)));
}

/// 中くらいの道路の十字路で、腕 `from` から腕 `to` の車線 `lane` へ入ろうとする候補。
fn approach(
    n: &Network,
    id: usize,
    from: (i64, i64),
    to: (i64, i64),
    lane: usize,
    waited: u32,
) -> junction::Approach {
    let c = (10, 10);
    let f = link(n, (c.0 + from.0, c.1 + from.1), c);
    let t = link(n, c, (c.0 + to.0, c.1 + to.1));
    junction::Approach {
        id,
        node: car::end_node(n, f),
        from: f,
        to: t,
        lane: n.link_lanes(f).next().unwrap(),
        to_lane: n.link_lanes(t).nth(lane).unwrap(),
        class: 2,
        committed: false,
        room: true,
        waited,
    }
}

#[test]
fn a_left_turn_waits_for_a_right_turn_that_went_first() {
    let n = medium_crossroads();
    let (w, e, s) = ((-1, 0), (1, 0), (0, 1));
    let held = |cands: Vec<junction::Approach>, inside: Vec<junction::Inside>| {
        junction::holds(
            &n,
            cands,
            inside,
            Tick::ZERO,
            &Default::default(),
            &Default::default(),
        )
        .ids
    };
    // 待ちすぎた右折が先に入る。交わる車線へ曲がる左折は止まり、交わらない車線なら一緒に入る。
    let late = PATIENCE + 1;
    let none: Vec<usize> = Vec::new();
    assert_eq!(
        held(
            vec![approach(&n, 0, w, s, 0, late), approach(&n, 1, e, s, 1, 0)],
            vec![]
        ),
        [1]
    );
    assert_eq!(
        held(
            vec![approach(&n, 0, w, s, 1, late), approach(&n, 1, e, s, 0, 0)],
            vec![]
        ),
        none
    );
    // 右折の車が交差点の中にいるときも同じである。
    let inside = |lane: usize| {
        let a = approach(&n, 9, w, s, lane, 0);
        junction::Inside {
            node: a.node,
            from: a.from,
            to: a.to,
            lane: a.to_lane,
            stopped: false,
        }
    };
    assert_eq!(
        held(vec![approach(&n, 1, e, s, 1, 0)], vec![inside(0)]),
        [1]
    );
    assert_eq!(
        held(vec![approach(&n, 1, e, s, 0, 0)], vec![inside(1)]),
        none
    );
}

#[test]
fn turning_cars_never_cross_inside_a_two_lane_crossroads() {
    let mut c = Cars::new(medium_crossroads());
    let mut drives = Vec::new();
    let mut incoming = Vec::new();
    for a in ARMS {
        let step = (-a.0 as i8, -a.1 as i8);
        let from = at(10 + 10 * a.0, 10 + 10 * a.1, step, 0);
        for b in ARMS.into_iter().filter(|b| *b != a) {
            let to = at(10 + 9 * b.0, 10 + 9 * b.1, (b.0 as i8, b.1 as i8), 0);
            drives.extend(std::iter::repeat_n((from, to), 6));
        }
        incoming.push(link(&c.net, (10 + a.0, 10 + a.1), (10, 10)));
    }
    let ids = c
        .trips
        .spawn_drives(&c.net, &mut c.router, &drives, Tick::ZERO);
    assert!(ids.iter().all(Option::is_some));
    let mut arrived = 0;
    for _ in 0..2_000 {
        arrived += c.step().len();
        let inside: Vec<Move> = c
            .trips
            .iter()
            .filter(|(_, t)| car::on_road(t) && t.leg > 0 && t.offset().0 < CAR_LENGTH + MIN_GAP)
            .filter(|(_, t)| incoming.contains(&t.route().links[t.leg as usize - 1]))
            .map(|(_, t)| {
                Move::into_lane(
                    &c.net,
                    t.route().links[t.leg as usize - 1],
                    t.link(),
                    t.lane(),
                )
            })
            .collect();
        for a in &inside {
            assert!(
                !inside.iter().any(|b| a.conflicts_in(b)),
                "交差点の中で交わった"
            );
        }
    }
    // 一度に出した車どうしは、合流を同じリンクで決めても減速の上限を破ることがある。交差点の中の
    // 交わりとは関わらないので、ここでは数えない。
    assert_eq!(arrived, drives.len());
    assert_eq!(c.trips.rescues(), Rescues::default(), "救済が働いた");
}
