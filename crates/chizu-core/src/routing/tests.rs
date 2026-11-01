use super::*;
use crate::command::RoadKind;
use crate::network::{Dir, Line};
use crate::tile::{TilePos, TileRect};
use proptest::prelude::*;

fn road(n: &mut Network, from: (i64, i64), to: (i64, i64), kind: RoadKind) {
    let line = Line::between(TilePos::new(from.0, from.1), TilePos::new(to.0, to.1)).unwrap();
    n.build_road(&line, kind).unwrap();
}

/// マス `from` のノードからマス `to` のノードへ向かうリンク。
fn link(n: &Network, from: (i64, i64), to: (i64, i64), mode: LaneMode) -> LinkId {
    let a = n.node_at(TilePos::new(from.0, from.1)).unwrap();
    let b = n.node_at(TilePos::new(to.0, to.1)).unwrap();
    n.segments()
        .find_map(|(id, s)| {
            let dir = if (s.a, s.b) == (a, b) {
                Dir::Forward
            } else if (s.a, s.b) == (b, a) {
                Dir::Backward
            } else {
                return None;
            };
            Some(LinkId {
                segment: id,
                dir,
                mode,
            })
        })
        .unwrap()
}

fn find(n: &Network, from: LinkId, to: LinkId) -> Option<Route> {
    astar(
        &Graph::build(n, from.mode),
        from,
        to,
        &mut Scratch::default(),
    )
}

/// 経路のリンクが辺でつながり、費用がリンクと辺の費用の和であることを確かめる。
fn check_route(g: &Graph, r: &Route) {
    let vs: Vec<u32> = r.links.iter().map(|l| g.vertex(*l).unwrap()).collect();
    let mut cost = g.cost(vs[0]);
    for w in vs.windows(2) {
        let e = g
            .edges(w[0])
            .iter()
            .find(|e| e.to == w[1])
            .expect("辺がない");
        cost += e.cost + g.cost(w[1]);
    }
    assert_eq!(cost, r.cost);
}

/// 素朴なダイクストラ。毎回すべての頂点から最小のものを探す。
fn dijkstra(g: &Graph, from: u32, to: u32) -> Option<u64> {
    let n = g.vertex_count();
    let mut dist = vec![u64::MAX; n];
    let mut done = vec![false; n];
    dist[from as usize] = g.cost(from);
    loop {
        let v = (0..n)
            .filter(|v| !done[*v] && dist[*v] != u64::MAX)
            .min_by_key(|v| dist[*v])?;
        if v == to as usize {
            return Some(dist[v]);
        }
        done[v] = true;
        for e in g.edges(v as u32) {
            let d = dist[v] + e.cost + g.cost(e.to);
            dist[e.to as usize] = dist[e.to as usize].min(d);
        }
    }
}

/// 西から東へ、細い道路でまっすぐ行く道と、太い道路で斜めに回る道がある網。
fn detour_network() -> Network {
    let mut n = Network::new();
    road(&mut n, (-5, 0), (0, 0), RoadKind::Small);
    road(&mut n, (0, 0), (20, 0), RoadKind::Small);
    road(&mut n, (20, 0), (25, 0), RoadKind::Small);
    road(&mut n, (0, 0), (5, 5), RoadKind::Large);
    road(&mut n, (5, 5), (15, 5), RoadKind::Large);
    road(&mut n, (15, 5), (20, 0), RoadKind::Large);
    n
}

#[test]
fn car_takes_the_faster_diagonal_detour() {
    let n = detour_network();
    let from = link(&n, (-5, 0), (0, 0), LaneMode::Car);
    let to = link(&n, (20, 0), (25, 0), LaneMode::Car);
    let r = find(&n, from, to).unwrap();
    let expected = [
        from,
        link(&n, (0, 0), (5, 5), LaneMode::Car),
        link(&n, (5, 5), (15, 5), LaneMode::Car),
        link(&n, (15, 5), (20, 0), LaneMode::Car),
        to,
    ];
    assert_eq!(r.links, expected);
    // 40m を時速 40km で 2 回、斜めの 56.57m を時速 60km で 2 回、80m を時速 60km で 1 回。
    let small = 2 * travel(40_000, 11_111);
    let large = 2 * travel(5 * 11_314, 16_667) + travel(80_000, 16_667);
    assert_eq!(r.cost, small + large);
    check_route(&Graph::build(&n, LaneMode::Car), &r);
}

fn travel(mm: u64, speed: u64) -> u64 {
    (mm * 1_000).div_ceil(speed)
}

#[test]
fn pedestrian_takes_the_shorter_straight_road() {
    let n = detour_network();
    let from = link(&n, (-5, 0), (0, 0), LaneMode::Sidewalk);
    let to = link(&n, (20, 0), (25, 0), LaneMode::Sidewalk);
    let r = find(&n, from, to).unwrap();
    assert_eq!(
        r.links,
        [from, link(&n, (0, 0), (20, 0), LaneMode::Sidewalk), to]
    );
    assert_eq!(r.cost, 2 * travel(40_000, 1_389) + travel(160_000, 1_389));
}

#[test]
fn cars_turn_back_only_at_dead_ends_and_pedestrians_anywhere() {
    let mut n = Network::new();
    road(&mut n, (0, 0), (10, 0), RoadKind::Small);
    road(&mut n, (10, 0), (20, 0), RoadKind::Small);
    let east = link(&n, (0, 0), (10, 0), LaneMode::Car);
    let west = link(&n, (10, 0), (0, 0), LaneMode::Car);
    // 車は (20, 0) の行き止まりで折り返す。
    let r = find(&n, east, west).unwrap();
    assert_eq!(r.links.len(), 4);
    let walk = |l: LinkId| LinkId {
        mode: LaneMode::Sidewalk,
        ..l
    };
    let r = find(&n, walk(east), walk(west)).unwrap();
    assert_eq!(r.links, [walk(east), walk(west)]);
}

#[test]
fn unconnected_links_have_no_route() {
    let mut n = Network::new();
    road(&mut n, (0, 0), (10, 0), RoadKind::Small);
    road(&mut n, (0, 5), (10, 5), RoadKind::Small);
    let a = link(&n, (0, 0), (10, 0), LaneMode::Car);
    let b = link(&n, (0, 5), (10, 5), LaneMode::Car);
    assert_eq!(find(&n, a, b), None);
    // 手段の違うリンクはグラフにない。
    let walk = LinkId {
        mode: LaneMode::Sidewalk,
        ..a
    };
    assert_eq!(find(&n, a, walk), None);
    let r = find(&n, a, a).unwrap();
    assert_eq!(r.links, [a]);
}

#[test]
fn cars_reach_the_outside_by_the_highway_and_pedestrians_do_not() {
    let mut n = Network::new();
    n.add_highway(TilePos::new(0, 0), TilePos::new(100, 0));
    road(&mut n, (0, 0), (0, 10), RoadKind::Small);
    let city = link(&n, (0, 10), (0, 0), LaneMode::Car);
    let out = link(&n, (0, 0), (100, 0), LaneMode::Car);
    let back = link(&n, (100, 0), (0, 0), LaneMode::Car);
    let r = find(&n, city, out).unwrap();
    assert_eq!(r.links, [city, out]);
    assert_eq!(r.cost, travel(80_000, 11_111) + travel(800_000, 27_778));
    // 外から来た車は街へ入れる。
    let home = link(&n, (0, 0), (0, 10), LaneMode::Car);
    assert_eq!(find(&n, back, home).unwrap().links, [back, home]);
    assert_eq!(Graph::build(&n, LaneMode::Sidewalk).vertex_count(), 2);
}

#[test]
fn cars_do_not_turn_back_at_the_outside() {
    // 輪の道路に短い高速道路と、遠い行き止まりの枝がつながっている。
    let mut n = Network::new();
    n.add_highway(TilePos::new(0, 0), TilePos::new(-2, 0));
    road(&mut n, (0, 0), (10, 0), RoadKind::Small);
    road(&mut n, (10, 0), (10, 10), RoadKind::Small);
    road(&mut n, (10, 10), (0, 10), RoadKind::Small);
    road(&mut n, (0, 10), (0, 0), RoadKind::Small);
    road(&mut n, (10, 10), (10, 30), RoadKind::Small);
    let east = link(&n, (0, 0), (10, 0), LaneMode::Car);
    let west = link(&n, (10, 0), (0, 0), LaneMode::Car);
    let r = find(&n, east, west).unwrap();
    let highway = link(&n, (0, 0), (-2, 0), LaneMode::Car).segment;
    assert!(r.links.iter().all(|l| l.segment != highway), "{r:?}");
    assert!(
        r.links
            .contains(&link(&n, (10, 30), (10, 10), LaneMode::Car))
    );
}

#[test]
fn every_lane_is_connected_at_the_node_it_reaches() {
    let mut n = Network::new();
    road(&mut n, (0, 0), (10, 0), RoadKind::Large);
    road(&mut n, (10, 0), (20, 0), RoadKind::Small);
    road(&mut n, (10, 0), (10, 10), RoadKind::Medium);
    let cs = connectors(&n);
    assert!(cs.is_sorted());
    for (id, lane) in n.lanes() {
        assert!(
            cs.iter().any(|c| c.from == id),
            "{id:?} から出るつながりがない"
        );
        assert!(cs.iter().any(|c| c.to == id), "{id:?} に入るつながりがない");
        for c in cs.iter().filter(|c| c.from == id) {
            assert_eq!(n.lane(c.to).unwrap().mode, lane.mode);
        }
    }
    // 片側 3 車線から片側 1 車線へ直進する 3 本は、1 本に合流する。
    let from = link(&n, (0, 0), (10, 0), LaneMode::Car);
    let to = link(&n, (10, 0), (20, 0), LaneMode::Car);
    let to_lane = n.link_lanes(to).next().unwrap();
    let merged = cs
        .iter()
        .filter(|c| c.to == to_lane && n.link_lanes(from).any(|l| l == c.from))
        .count();
    assert_eq!(merged, 3);
}

fn grid(size: i64) -> Network {
    let mut n = Network::new();
    for i in 0..=size {
        road(&mut n, (0, i * 4), (size * 4, i * 4), RoadKind::Small);
        road(&mut n, (i * 4, 0), (i * 4, size * 4), RoadKind::Medium);
    }
    n
}

fn requests(n: &Network, count: usize) -> Vec<RouteRequest> {
    let links: Vec<LinkId> = n.links().collect();
    (0..count)
        .map(|i| {
            let from = links[(i * 7) % links.len()];
            let mut to = links[(i * 13 + 5) % links.len()];
            to.mode = if i % 3 == 0 { to.mode } else { from.mode };
            RouteRequest {
                from,
                to,
                mode: from.mode,
            }
        })
        .collect()
}

#[test]
fn batch_returns_results_in_request_order_and_reuses_them() {
    let n = grid(4);
    let reqs = requests(&n, 200);
    let mut router = AstarRouter::new(&n);
    let got = router.route_batch(&reqs);
    let mut s = Scratch::default();
    for (r, got) in reqs.iter().zip(&got) {
        let want = astar(router.graph(r.mode), r.from, r.to, &mut s);
        assert_eq!(got.as_deref(), want.as_ref());
    }
    assert!(got.iter().any(Option::is_none));
    let cached = router.cached();
    assert!(cached < reqs.len());
    assert_eq!(router.route_batch(&reqs), got);
    assert_eq!(router.cached(), cached);
    router.set_band(1);
    assert_eq!(router.cached(), 0);
    router.rebuild(&n);
    assert_eq!((router.band(), router.cached()), (1, 0));
    assert_eq!(router.route_batch(&reqs), got);
}

#[test]
fn mixed_modes_do_not_reallocate_the_scratch() {
    // 高速道路があると、車と歩行者でグラフの頂点の数が違う。
    let mut n = grid(4);
    n.add_highway(TilePos::new(0, 0), TilePos::new(-20, 0));
    let router = AstarRouter::new(&n);
    assert_ne!(
        router.graph(LaneMode::Car).vertex_count(),
        router.graph(LaneMode::Sidewalk).vertex_count()
    );
    let mut reqs: Vec<RouteRequest> = n
        .links()
        .map(|l| RouteRequest {
            from: l,
            to: l,
            mode: l.mode,
        })
        .collect();
    reqs.sort();
    let mut s = Scratches::default();
    for r in &reqs {
        router.solve(r, &mut s);
    }
    assert_eq!((s.car.allocations(), s.walk.allocations()), (1, 1));
}

#[cfg(feature = "parallel")]
#[test]
fn batch_does_not_depend_on_the_thread_count() {
    let n = grid(8);
    let reqs = requests(&n, 2_000);
    let run = |threads: usize| {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        pool.install(|| AstarRouter::new(&n).route_batch(&reqs))
    };
    let one = run(1);
    assert_eq!(one, run(8));
    assert_eq!(one, run(3));
}

proptest! {
    #[test]
    fn astar_cost_matches_dijkstra(
        roads in proptest::collection::vec(
            (0_u8..3, 0_i64..16, 0_i64..16, -1_i64..=1, -1_i64..=1, 1_i64..10),
            1..30,
        ),
        bulldoze in proptest::option::of((0_i64..16, 0_i64..16)),
        pairs in proptest::collection::vec((0_usize..1_000, 0_usize..1_000), 10),
    ) {
        let mut n = Network::new();
        n.add_highway(TilePos::new(0, 0), TilePos::new(-30, 0));
        let kinds = [RoadKind::Small, RoadKind::Medium, RoadKind::Large];
        for (k, x, y, sx, sy, len) in roads {
            if (sx, sy) == (0, 0) {
                continue;
            }
            let line = Line::between(TilePos::new(x, y), TilePos::new(x + sx * len, y + sy * len)).unwrap();
            let _ = n.build_road(&line, kinds[k as usize]);
        }
        if let Some((x, y)) = bulldoze {
            let _ = n.bulldoze(TileRect::from_corners(TilePos::new(x, y), TilePos::new(x + 1, y)));
        }
        for mode in [LaneMode::Car, LaneMode::Sidewalk] {
            let g = Graph::build(&n, mode);
            if g.vertex_count() == 0 {
                continue;
            }
            let mut s = Scratch::default();
            for (a, b) in &pairs {
                let (a, b) = ((a % g.vertex_count()) as u32, (b % g.vertex_count()) as u32);
                let r = astar(&g, g.link(a), g.link(b), &mut s);
                prop_assert_eq!(r.as_ref().map(|r| r.cost), dijkstra(&g, a, b));
                if let Some(r) = r {
                    check_route(&g, &r);
                }
            }
        }
    }
}
