use super::*;
use crate::command::RoadKind;
use crate::network::{Dir, Line, SegmentId};
use crate::tile::TileRect;
use crate::units::Mm;
use proptest::prelude::*;

fn road(n: &mut Network, from: (i64, i64), to: (i64, i64)) {
    let line = Line::between(TilePos::new(from.0, from.1), TilePos::new(to.0, to.1)).unwrap();
    n.build_road(&line, RoadKind::Small).unwrap();
}

/// マス `(x, y)` から `step` の向きに、`along` mm 進んだ歩道の上の位置。
fn at(x: i64, y: i64, step: (i8, i8), along: u32) -> SidewalkPos {
    SidewalkPos {
        tile: TilePos::new(x, y),
        step,
        along: LaneOffset(along),
    }
}

struct Walkers {
    net: Network,
    router: AstarRouter,
    trips: Trips,
}

impl Walkers {
    fn new(net: Network) -> Walkers {
        let router = AstarRouter::new(&net);
        Walkers {
            net,
            router,
            trips: Trips::default(),
        }
    }

    fn spawn(&mut self, from: SidewalkPos, to: SidewalkPos) -> Option<TripId> {
        self.trips
            .spawn(&self.net, &mut self.router, from, to, Tick::ZERO)
    }

    /// 着くまでの tick 数。
    fn ticks_to_arrive(&mut self, id: TripId) -> u64 {
        for t in 1..100_000 {
            if self.trips.step(&self.net, &mut self.router).contains(&id) {
                return t;
            }
        }
        panic!("着かない");
    }

    fn edit(&mut self, f: impl FnOnce(&mut Network)) -> Vec<TripId> {
        f(&mut self.net);
        self.router.rebuild(&self.net);
        self.trips.reroute(&self.net, &mut self.router)
    }
}

fn walk_ticks(mm: u64) -> u64 {
    mm.div_ceil(1_389)
}

#[test]
fn positions_round_trip_through_both_sidewalks_and_diagonals() {
    let mut n = Network::new();
    road(&mut n, (0, 0), (10, 0));
    road(&mut n, (0, 0), (-5, 5));
    for (id, _) in n.segments().collect::<Vec<(SegmentId, _)>>() {
        for dir in [Dir::Forward, Dir::Backward] {
            let link = LinkId {
                segment: id,
                dir,
                mode: LaneMode::Sidewalk,
            };
            let len = pos::link_geometry(&n, link).unwrap().2;
            for off in [0, 1, 7_999, 8_000, 11_314, len.0 / 2, len.0] {
                let p = SidewalkPos::on_link(&n, link, LaneOffset(off)).unwrap();
                assert_eq!(p.resolve(&n), Some((link, LaneOffset(off))), "{p:?}");
            }
        }
    }
    // 道路のないところと、車線のリンクには歩道の位置がない。
    assert_eq!(at(0, 3, (1, 0), 0).resolve(&n), None);
    let car = LinkId {
        segment: n.segments().next().unwrap().0,
        dir: Dir::Forward,
        mode: LaneMode::Car,
    };
    assert_eq!(SidewalkPos::on_link(&n, car, LaneOffset(0)), None);
}

#[test]
fn walks_at_walking_speed_and_vanishes_at_the_destination() {
    let mut n = Network::new();
    road(&mut n, (0, 0), (10, 0));
    road(&mut n, (10, 0), (10, -10));
    let mut w = Walkers::new(n);
    // (2, 0) の 3m 先から、角を曲がって (10, -4) の 1m 先まで歩く。
    let id = w
        .spawn(at(2, 0, (1, 0), 3_000), at(10, -4, (0, -1), 1_000))
        .unwrap();
    assert_eq!(w.trips.get(id).unwrap().route().links.len(), 2);
    let mm = 8 * 8_000 - 3_000 + 4 * 8_000 + 1_000;
    assert_eq!(w.ticks_to_arrive(id), walk_ticks(mm));
    assert!(w.trips.is_empty());
    assert_eq!(w.trips.get(id), None);
}

#[test]
fn a_destination_behind_on_the_same_sidewalk_is_walked_straight() {
    let mut n = Network::new();
    road(&mut n, (0, 0), (10, 0));
    let mut w = Walkers::new(n);
    let id = w.spawn(at(6, 0, (1, 0), 0), at(2, 0, (1, 0), 0)).unwrap();
    // 端まで行って戻らず、逆の向きに歩いて手前の行き先へまっすぐ向かう。
    let t = w.trips.get(id).unwrap();
    assert_eq!(t.route().links.len(), 1);
    assert_eq!(t.route().links[0].dir, Dir::Backward);
    assert_eq!(t.step, (-1, 0));
    assert_eq!(w.ticks_to_arrive(id), walk_ticks(4 * 8_000));
    let ahead = w.spawn(at(2, 0, (1, 0), 0), at(6, 0, (1, 0), 0)).unwrap();
    let t = w.trips.get(ahead).unwrap();
    assert_eq!(t.route().links.len(), 1);
    assert_eq!(t.route().links[0].dir, Dir::Forward);
}

#[test]
fn the_start_and_the_destination_face_the_shorter_way() {
    // 東西の道路の途中から出て、北へ曲がった先の行き先へ向かう。行き先は北向きの歩道の位置で
    // 表すが、南向きに表しても同じ道のりになる。
    let mut n = Network::new();
    road(&mut n, (0, 0), (10, 0));
    road(&mut n, (0, 0), (0, -10));
    let mut w = Walkers::new(n);
    let id = w.spawn(at(3, 0, (1, 0), 0), at(0, -4, (0, 1), 0)).unwrap();
    // 西へ 3 マス歩いて角を曲がり、北へ 4 マス。
    assert_eq!(w.ticks_to_arrive(id), walk_ticks(7 * 8_000));
    let id = w.spawn(at(3, 0, (1, 0), 0), at(0, -4, (0, -1), 0)).unwrap();
    assert_eq!(w.ticks_to_arrive(id), walk_ticks(7 * 8_000));
}

#[test]
fn walkers_cross_to_the_opposite_sidewalk_at_a_node() {
    // T 字の交差点で、来た区間の逆向きの歩道にも、ほかの区間の歩道にも移れる。
    let mut n = Network::new();
    road(&mut n, (0, 0), (10, 0));
    road(&mut n, (10, 0), (20, 0));
    road(&mut n, (10, 0), (10, 10));
    let g = crate::routing::Graph::build(&n, LaneMode::Sidewalk);
    let link = |x, y, step| at(x, y, step, 0).resolve(&n).unwrap().0;
    let west = link(5, 0, (1, 0));
    let next: Vec<LinkId> = g
        .edges(g.vertex(west).unwrap())
        .iter()
        .map(|e| g.link(e.to))
        .collect();
    let mut want = vec![
        link(5, 0, (-1, 0)),
        link(15, 0, (1, 0)),
        link(10, 5, (0, 1)),
    ];
    want.sort();
    assert_eq!(next, want);
    // 行き先を北向きの歩道の位置で表しても、交差点で曲がって南向きに歩いて着く。
    let mut w = Walkers::new(n);
    let id = w.spawn(at(8, 0, (1, 0), 0), at(10, 3, (0, -1), 0)).unwrap();
    assert_eq!(w.ticks_to_arrive(id), walk_ticks(5 * 8_000));
}

#[test]
fn spawning_off_the_sidewalk_or_unconnected_fails() {
    let mut n = Network::new();
    road(&mut n, (0, 0), (10, 0));
    road(&mut n, (0, 5), (10, 5));
    let mut w = Walkers::new(n);
    assert_eq!(w.spawn(at(0, 2, (1, 0), 0), at(5, 0, (1, 0), 0)), None);
    assert_eq!(w.spawn(at(1, 0, (1, 0), 0), at(5, 5, (1, 0), 0)), None);
    assert!(w.trips.is_empty());
}

#[test]
fn splitting_the_road_keeps_the_walk_and_its_timing() {
    let mut n = Network::new();
    road(&mut n, (0, 0), (20, 0));
    let mut w = Walkers::new(n);
    let id = w.spawn(at(0, 0, (1, 0), 0), at(19, 0, (1, 0), 0)).unwrap();
    for _ in 0..30 {
        w.trips.step(&w.net, &mut w.router);
    }
    // 歩いている道路の途中と先を、交わる道路で分ける。
    let lost = w.edit(|n| {
        road(n, (2, -3), (2, 3));
        road(n, (12, -3), (12, 3));
    });
    assert!(lost.is_empty());
    // 30 tick で 41.67m 進み、(2, 0) の分かれ目は過ぎている。
    assert_eq!(w.trips.get(id).unwrap().route().links.len(), 2);
    assert_eq!(30 + w.ticks_to_arrive(id), walk_ticks(19 * 8_000));
}

#[test]
fn removed_roads_reroute_or_drop_the_walk() {
    let mut n = Network::new();
    road(&mut n, (0, 0), (10, 0));
    road(&mut n, (10, 0), (20, 0));
    road(&mut n, (10, 0), (10, 10));
    road(&mut n, (10, 10), (20, 10));
    road(&mut n, (20, 10), (20, 0));
    let mut w = Walkers::new(n);
    let go = w
        .spawn(at(0, 0, (1, 0), 0), at(20, 0, (0, 1), 4_000))
        .unwrap();
    let stay = w.spawn(at(0, 0, (1, 0), 0), at(5, 0, (1, 0), 0)).unwrap();
    for _ in 0..10 {
        w.trips.step(&w.net, &mut w.router);
    }
    // 先の道路を消すと、北へ回る道を探し直す。
    let lost = w.edit(|n| {
        n.bulldoze(TileRect::from_corners(
            TilePos::new(15, 0),
            TilePos::new(15, 0),
        ))
        .unwrap();
    });
    assert!(lost.is_empty());
    assert!(w.trips.get(go).unwrap().route().links.len() >= 4);
    // 歩いている道路を消すと、その移動は消える。
    let lost = w.edit(|n| {
        n.bulldoze(TileRect::from_corners(
            TilePos::new(3, 0),
            TilePos::new(3, 0),
        ))
        .unwrap();
    });
    assert_eq!(lost, [go, stay]);
    assert!(w.trips.is_empty());
}

#[test]
fn movers_report_world_positions_in_the_area() {
    let mut n = Network::new();
    road(&mut n, (0, 0), (5, 5));
    let mut w = Walkers::new(n);
    let id = w.spawn(at(1, 1, (1, 1), 0), at(4, 4, (1, 1), 0)).unwrap();
    w.trips.step(&w.net, &mut w.router);
    let world = |t: i64| Mm(t * 8_000);
    let all = w.trips.movers_in(
        &w.net,
        WorldPos::new(world(0), world(0)),
        WorldPos::new(world(6), world(6)),
    );
    // 斜めに 1,389mm 進むと、縦横には 1,389 × 8,000 ÷ 11,314 を切り捨てた 982mm ずつ進む。
    let p = 8_000 + 4_000 + 982;
    assert_eq!(
        all,
        [Mover {
            id,
            pos: WorldPos::new(Mm(p), Mm(p)),
            heading: (1, 1),
            mode: LaneMode::Sidewalk,
            // 小さい道路の歩道は、中心線から車線 1 本と歩道の半分（2,080mm）、進む向きの右にある。
            // 斜めなので、√2 で割って 1,470mm ずつ、右（左下）へずれる。
            shift: (Mm(-1_470), Mm(1_470)),
        }]
    );
    let none = w.trips.movers_in(
        &w.net,
        WorldPos::new(world(3), world(3)),
        WorldPos::new(world(6), world(6)),
    );
    assert!(none.is_empty());
}

proptest! {
    #[test]
    fn walks_take_the_route_length_at_walking_speed(
        a in 0_u64..40, b in 0_u64..40, ao in 0_u32..8_000, bo in 0_u32..8_000,
    ) {
        // 輪の道路の上の 2 点。
        let mut n = Network::new();
        road(&mut n, (0, 0), (10, 0));
        road(&mut n, (10, 0), (10, 10));
        road(&mut n, (10, 10), (0, 10));
        road(&mut n, (0, 10), (0, 0));
        let ring = |k: u64| match k / 10 {
            0 => (k as i64, 0, (1, 0)),
            1 => (10, (k - 10) as i64, (0, 1)),
            2 => (10 - (k - 20) as i64, 10, (-1, 0)),
            _ => (0, 10 - (k - 30) as i64, (0, -1)),
        };
        let (pa, pb) = (ring(a), ring(b));
        let (from, to) = (at(pa.0, pa.1, pa.2, ao), at(pb.0, pb.1, pb.2, bo));
        let mut w = Walkers::new(n);
        let id = w.spawn(from, to).unwrap();
        let mm = walked_mm(&w, id, from, to);
        prop_assert_eq!(w.ticks_to_arrive(id), walk_ticks(mm).max(1));
    }

    #[test]
    fn walks_across_diagonals_take_the_route_length(
        a in 0_u64..30, b in 0_u64..30, ao in 0_u32..11_314, bo in 0_u32..11_314,
    ) {
        // 縦横と斜めの区間が交互に並ぶ輪の道路の上の 2 点。
        let mut n = Network::new();
        road(&mut n, (0, 0), (10, 0));
        road(&mut n, (10, 0), (15, 5));
        road(&mut n, (15, 5), (5, 5));
        road(&mut n, (5, 5), (0, 0));
        let ring = |k: u64| -> (i64, i64, (i8, i8)) {
            let k = k as i64;
            match k {
                0..10 => (k, 0, (1, 0)),
                10..15 => (k, k - 10, (1, 1)),
                15..25 => (30 - k, 5, (-1, 0)),
                _ => (30 - k, 30 - k, (-1, -1)),
            }
        };
        let per = |s: (i8, i8)| if s.0 != 0 && s.1 != 0 { 11_314 } else { 8_000 };
        let (pa, pb) = (ring(a), ring(b));
        let from = at(pa.0, pa.1, pa.2, ao % per(pa.2));
        let to = at(pb.0, pb.1, pb.2, bo % per(pb.2));
        let mut w = Walkers::new(n);
        let id = w.spawn(from, to).unwrap();
        let mm = walked_mm(&w, id, from, to);
        prop_assert_eq!(w.ticks_to_arrive(id), walk_ticks(mm).max(1));
    }
}

/// 歩く距離。経路のリンクの長さの和から、最初のリンクの手前と最後のリンクの先を引いたもの。
/// 歩き始めと行き先の位置は、どちらの向きの歩道で表しても、`from` と `to` と同じ点である。
/// 斜めの歩道のワールド座標は切り捨てるので、向きによって 1mm ずれることがある。
fn walked_mm(w: &Walkers, id: TripId, from: SidewalkPos, to: SidewalkPos) -> u64 {
    let t = w.trips.get(id).unwrap();
    let links = t.route().links.clone();
    let near = |p: WorldPos, q: WorldPos| p.x.0.abs_diff(q.x.0) <= 1 && p.y.0.abs_diff(q.y.0) <= 1;
    assert!(near(t.position().world(), from.world()));
    let last = *links.last().unwrap();
    let dest = SidewalkPos::on_link(&w.net, last, t.dest_offset).unwrap();
    assert!(near(dest.world(), to.world()));
    let len = |l: &LinkId| u64::from(pos::link_geometry(&w.net, *l).unwrap().2.0);
    let total: u64 = links.iter().map(len).sum();
    let start = u64::from(t.offset.0);
    let end = u64::from(t.dest_offset.0);
    total - start - (len(&last) - end)
}

#[test]
fn walking_onto_a_diagonal_carries_the_rest_of_the_step() {
    let mut n = Network::new();
    road(&mut n, (0, 0), (10, 0));
    road(&mut n, (10, 0), (15, 5));
    let mut w = Walkers::new(n);
    let id = w
        .spawn(at(2, 0, (1, 0), 0), at(12, 2, (1, 1), 5_000))
        .unwrap();
    assert_eq!(
        w.ticks_to_arrive(id),
        walk_ticks(8 * 8_000 + 2 * 11_314 + 5_000)
    );
}

#[test]
fn a_destination_behind_on_a_diagonal_is_walked_straight() {
    let mut n = Network::new();
    road(&mut n, (0, 0), (5, 5));
    let mut w = Walkers::new(n);
    let id = w.spawn(at(3, 3, (1, 1), 0), at(1, 1, (1, 1), 0)).unwrap();
    assert_eq!(w.trips.get(id).unwrap().route().links.len(), 1);
    assert_eq!(w.ticks_to_arrive(id), walk_ticks(2 * 11_314));
}

#[test]
fn rerouting_with_an_old_router_rebuilds_it_first() {
    let mut n = Network::new();
    road(&mut n, (0, 0), (10, 0));
    road(&mut n, (10, 0), (20, 0));
    road(&mut n, (10, 0), (10, 10));
    road(&mut n, (10, 10), (20, 10));
    road(&mut n, (20, 10), (20, 0));
    let mut w = Walkers::new(n);
    let id = w
        .spawn(at(0, 0, (1, 0), 0), at(20, 0, (0, 1), 4_000))
        .unwrap();
    w.net
        .bulldoze(TileRect::from_corners(
            TilePos::new(15, 0),
            TilePos::new(15, 0),
        ))
        .unwrap();
    // 経路探索を作り直さずに呼んでも、古いグラフで経路を組まない。
    assert!(w.trips.reroute(&w.net, &mut w.router).is_empty());
    assert_eq!(w.router.version(), w.net.version());
    w.ticks_to_arrive(id);
}

#[test]
fn highways_have_no_sidewalk_positions() {
    let mut n = Network::new();
    n.add_highway(TilePos::new(0, 0), TilePos::new(10, 0));
    let seg = n.segments().next().unwrap().0;
    let link = LinkId {
        segment: seg,
        dir: Dir::Forward,
        mode: LaneMode::Sidewalk,
    };
    assert_eq!(SidewalkPos::on_link(&n, link, LaneOffset(0)), None);
}

#[cfg(feature = "parallel")]
#[test]
fn stepping_does_not_depend_on_the_thread_count() {
    use crate::state_hash::state_hash_of;
    let mut n = Network::new();
    for i in 0..=3 {
        road(&mut n, (0, i * 10), (30, i * 10));
        road(&mut n, (i * 10, 0), (i * 10, 30));
    }
    let run = |threads: usize| {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        pool.install(|| {
            let mut w = Walkers::new(n.clone());
            let walks: Vec<_> = (0..200_i64)
                .map(|k| {
                    let from = at(k % 29, 10 * (k % 4), (1, 0), 500);
                    let to = at(10 * ((k + 1) % 4), (k * 7) % 29, (0, 1), 7_000);
                    (from, to)
                })
                .collect();
            let ids = w
                .trips
                .spawn_many(&w.net, &mut w.router, &walks, Tick::ZERO);
            assert!(ids.iter().all(Option::is_some));
            let mut arrived = Vec::new();
            for _ in 0..60 {
                arrived.extend(w.trips.step(&w.net, &mut w.router));
            }
            (arrived, state_hash_of(&w.trips))
        })
    };
    let one = run(1);
    assert!(!one.0.is_empty());
    assert_eq!(run(8), one);
    assert_eq!(run(3), one);
}

/// 格子の道路を引いた街で歩く人を何人も作り、`ticks` だけ進めた状態ハッシュ。
/// `look` なら毎 tick、画面の範囲の移動体を問い合わせる。
fn walking_world_hash(ticks: u64, look: bool) -> u64 {
    use crate::command::Command;
    use crate::world::{ApplyOutcome, World};
    let mut w = World::new(7);
    for i in 0..=3 {
        for (from, to) in [((0, i * 10), (30, i * 10)), ((i * 10, 0), (i * 10, 30))] {
            let c = Command::BuildRoad {
                from: TilePos::new(from.0, from.1),
                to: TilePos::new(to.0, to.1),
                kind: RoadKind::Small,
            };
            assert_eq!(w.apply(&c), ApplyOutcome::Applied);
        }
    }
    for k in 0..40_i64 {
        let from = at(k % 29, 10 * (k % 4), (1, 0), 500);
        let to = at(10 * ((k + 1) % 4), (k * 7) % 29, (0, 1), 7_000);
        assert!(w.spawn_walk(from, to).is_some());
    }
    let area = TileRect::from_corners(TilePos::new(0, 0), TilePos::new(15, 15));
    for t in 0..ticks {
        if t == 50 {
            let c = Command::Bulldoze {
                area: TileRect::from_corners(TilePos::new(15, 10), TilePos::new(15, 10)),
            };
            assert_eq!(w.apply(&c), ApplyOutcome::Applied);
        }
        if look {
            assert!(t > 100 || !w.movers_in(area).is_empty());
        }
        w.step();
    }
    w.state_hash()
}

#[test]
fn world_walks_do_not_depend_on_threads_or_the_view() {
    let base = walking_world_hash(300, false);
    assert_eq!(base, walking_world_hash(300, true));
    assert_ne!(base, walking_world_hash(299, false));
    #[cfg(feature = "parallel")]
    for threads in [1, 3, 8] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        assert_eq!(pool.install(|| walking_world_hash(300, false)), base);
    }
}

/// 東西に 30 マスの道路を引いた街で、西の端から東の端の 1 マス手前まで歩く人を作る。
fn world_with_a_walker() -> (crate::world::World, TripId) {
    use crate::command::Command;
    use crate::world::{ApplyOutcome, World};
    let mut w = World::new(7);
    let c = Command::BuildRoad {
        from: TilePos::new(0, 0),
        to: TilePos::new(30, 0),
        kind: RoadKind::Small,
    };
    assert_eq!(w.apply(&c), ApplyOutcome::Applied);
    let id = w
        .spawn_walk(at(0, 0, (1, 0), 0), at(29, 0, (1, 0), 0))
        .unwrap();
    (w, id)
}

/// 歩く人が着いて消えるまでの tick 数。`edit` は 30 tick めの頭で呼ぶ。
fn world_ticks_to_arrive(edit: impl FnOnce(&mut crate::world::World)) -> u64 {
    let (mut w, id) = world_with_a_walker();
    let mut edit = Some(edit);
    for t in 1..1_000 {
        if t == 30 {
            (edit.take().unwrap())(&mut w);
        }
        w.step();
        if w.trips().get(id).is_none() {
            return t;
        }
    }
    panic!("着かない");
}

#[test]
fn world_walkers_keep_walking_across_a_split_and_arrive_on_time() {
    use crate::command::Command;
    // 29 マスを時速 5km で歩くと、232m ÷ 1,389mm を切り上げた tick で着く。
    let plain = world_ticks_to_arrive(|_| {});
    assert_eq!(plain, walk_ticks(29 * 8_000));
    let split = world_ticks_to_arrive(|w| {
        for x in [3, 20] {
            let c = Command::BuildRoad {
                from: TilePos::new(x, -5),
                to: TilePos::new(x, 5),
                kind: RoadKind::Small,
            };
            assert_eq!(w.apply(&c), crate::world::ApplyOutcome::Applied);
        }
    });
    assert_eq!(split, plain);
}

#[test]
fn world_walkers_on_a_removed_road_vanish() {
    use crate::command::Command;
    let (mut w, id) = world_with_a_walker();
    for _ in 0..10 {
        w.step();
    }
    let c = Command::Bulldoze {
        area: TileRect::from_corners(TilePos::new(5, 0), TilePos::new(5, 0)),
    };
    assert_eq!(w.apply(&c), crate::world::ApplyOutcome::Applied);
    w.step();
    assert!(w.trips().get(id).is_none());
}

#[test]
fn world_movers_in_accepts_any_area() {
    let (w, id) = world_with_a_walker();
    let all = TileRect::from_corners(
        TilePos::new(i64::MIN, i64::MIN),
        TilePos::new(i64::MAX, i64::MAX),
    );
    let m = w.movers_in(all);
    assert_eq!(m.len(), 1);
    assert_eq!(m[0].id, id);
}

/// 通勤の街で 2 日目を回し、そのあいだに終わった、歩くだけの通勤の所要 tick の平均と件数。
fn mean_walking_commute() -> (u64, u64) {
    use crate::clock::TICKS_PER_DAY;
    use crate::command::ZoneKind;
    use crate::world::people::tests::{city, run_days};
    use std::collections::{BTreeMap, BTreeSet};
    let mut w = city(&[ZoneKind::Residential, ZoneKind::Industrial]);
    run_days(&mut w, 1);
    let mut seen = BTreeSet::new();
    let mut walking: BTreeMap<TripId, Tick> = BTreeMap::new();
    let (mut sum, mut count) = (0_u64, 0_u64);
    while w.tick().0 < 2 * TICKS_PER_DAY {
        w.step();
        let trips = w.trips();
        for (id, t) in trips.iter() {
            if trips.commuter(id).is_some() && seen.insert(id) {
                let walk = t.mode == LaneMode::Sidewalk && t.then == parking::Then::Arrive;
                if walk {
                    walking.insert(id, t.departed);
                }
            }
        }
        let ended: Vec<TripId> = walking
            .keys()
            .filter(|id| trips.get(**id).is_none())
            .copied()
            .collect();
        for id in ended {
            let departed = walking.remove(&id).unwrap();
            sum += w.tick().0 - departed.0;
            count += 1;
        }
    }
    (sum / count.max(1), count)
}

#[test]
fn walking_commutes_take_the_near_way() {
    let (mean, count) = mean_walking_commute();
    println!("歩くだけの通勤：{count} 件、平均 {mean} tick");
    assert!(count >= 20, "歩くだけの通勤が {count} 件しかない");
    // 片側の歩道を一方の向きにしか歩けなかったころは、平均で 750 tick ほどかかっていた。
    assert!(mean <= 500, "平均 {mean} tick");
}
