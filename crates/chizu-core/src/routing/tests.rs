use super::*;
use crate::command::RoadKind;
use crate::network::{Dir, Line, TILE_MM};
use crate::tile::{TilePos, TileRect};
use crate::units::{Mm, WorldPos};
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
fn bundling_and_the_cache_limit_do_not_change_the_routes() {
    let n = grid(6);
    let mut reqs = requests(&n, 300);
    // 着くリンクを 3 つに集めた依頼を足し、束ねる組を作る。
    let links: Vec<LinkId> = n.links().filter(|l| l.mode == LaneMode::Car).collect();
    reqs.extend((0..300).map(|i| RouteRequest {
        from: links[(i * 11) % links.len()],
        to: links[i % 3],
        mode: LaneMode::Car,
    }));
    let want = AstarRouter::new(&n)
        .with_bundle_min(usize::MAX)
        .route_batch(&reqs);
    let mut router = AstarRouter::new(&n)
        .with_bundle_min(2)
        .with_cache_limit(400);
    assert_eq!(router.route_batch(&reqs[..250]), want[..250]);
    let before = router.cached();
    assert_eq!(router.route_batch(&reqs), want);
    assert!(router.cached() < before + reqs.len() - 250);
    assert!(router.cached() <= reqs.len());
}

#[test]
fn one_batch_larger_than_the_cache_limit_keeps_only_the_limit() {
    let n = grid(6);
    let reqs = requests(&n, 100);
    let want = AstarRouter::new(&n).route_batch(&reqs);
    let mut router = AstarRouter::new(&n).with_cache_limit(10);
    assert_eq!(router.route_batch(&reqs), want);
    assert!(router.cached() <= 10);
    assert_eq!(router.route_batch(&reqs), want);
    assert!(router.cached() <= 10);
}

#[test]
fn bundles_in_the_direction_that_covers_more_requests() {
    let n = grid(8);
    let links: Vec<LinkId> = n.links().filter(|l| l.mode == LaneMode::Car).collect();
    let req = |from: usize, to: usize| RouteRequest {
        from: links[from],
        to: links[to],
        mode: LaneMode::Car,
    };
    // 1 か所から 60 か所へ散らばる依頼と、その 60 か所へほかから 2 件ずつ来る依頼。
    // 着くリンクの組（60 組、3 件ずつ）のほうが少ないが、下限に届くのは出発のリンクの組だけである。
    let mut reqs: Vec<RouteRequest> = (1..=60).map(|t| req(0, t)).collect();
    reqs.extend((1..=60).flat_map(|t| [req(60 + t, t), req(120 + t, t)]));
    reqs.sort();
    let router = AstarRouter::new(&n);
    let jobs = router.plan(&reqs);
    let bundled: Vec<(Toward, usize)> = jobs
        .iter()
        .filter_map(|j| match j {
            Job::Bundle { toward, others, .. } => Some((*toward, others.len())),
            Job::Single(_) => None,
        })
        .collect();
    assert_eq!(bundled, [(Toward::Forward, 60)]);
    let want = AstarRouter::new(&n)
        .with_bundle_min(usize::MAX)
        .route_batch(&reqs);
    assert_eq!(AstarRouter::new(&n).route_batch(&reqs), want);
}

#[test]
fn a_large_group_extracts_routes_in_parallel_with_the_same_result() {
    let n = grid(8);
    let links: Vec<LinkId> = n.links().filter(|l| l.mode == LaneMode::Car).collect();
    // 1 か所から 280 か所へ散らばる依頼。経路の取り出しをスレッドに分ける大きさである。
    let reqs: Vec<RouteRequest> = (1..=280)
        .map(|t| RouteRequest {
            from: links[0],
            to: links[t],
            mode: LaneMode::Car,
        })
        .collect();
    let want = AstarRouter::new(&n)
        .with_bundle_min(usize::MAX)
        .route_batch(&reqs);
    assert_eq!(AstarRouter::new(&n).route_batch(&reqs), want);
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
        router.solve(&Job::Single(*r), &mut s);
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
            let ends: Vec<(LinkId, LinkId)> = pairs
                .iter()
                .map(|(a, b)| (g.link((a % g.vertex_count()) as u32), g.link((b % g.vertex_count()) as u32)))
                .collect();
            check_bundles(&g, &ends);
        }
    }
}

/// 着くリンクで束ねても出発のリンクで束ねても、A* と同じ経路になることを確かめる。
fn check_bundles(g: &Graph, ends: &[(LinkId, LinkId)]) {
    let mut s = Scratch::default();
    let (from, to) = ends[0];
    let starts: Vec<LinkId> = ends.iter().map(|e| e.0).collect();
    let goals: Vec<LinkId> = ends.iter().map(|e| e.1).collect();
    let back = one_to_many(g, to, &starts, Toward::Backward, &mut s);
    for (a, got) in starts.iter().zip(back) {
        assert_eq!(got, astar(g, *a, to, &mut s), "{a:?} から {to:?}");
    }
    let fwd = one_to_many(g, from, &goals, Toward::Forward, &mut s);
    for (b, got) in goals.iter().zip(fwd) {
        assert_eq!(got, astar(g, from, *b, &mut s), "{from:?} から {b:?}");
    }
}

#[test]
fn bundles_pick_the_same_route_as_astar_among_ties() {
    // 同じ道路の格子には、費用の同じ最短経路がたくさんある。
    let mut n = Network::new();
    for i in 0..=6 {
        road(&mut n, (0, i * 3), (18, i * 3), RoadKind::Small);
        road(&mut n, (i * 3, 0), (i * 3, 18), RoadKind::Small);
    }
    for mode in [LaneMode::Car, LaneMode::Sidewalk] {
        let g = Graph::build(&n, mode);
        let m = g.vertex_count();
        for root in [0, m / 3, m - 1] {
            let ends: Vec<_> = (0..m)
                .step_by(5)
                .map(|v| (g.link(root as u32), g.link(v as u32)))
                .collect();
            check_bundles(&g, &ends);
            let swapped: Vec<_> = ends.iter().map(|(a, b)| (*b, *a)).collect();
            check_bundles(&g, &swapped);
        }
    }
}

/// 整合を崩した網から、手段ごとのグラフ、車線のつながり、経路探索のどれを作っても panic せず `Err` になる。
fn assert_rejected(n: &Network, want: &GraphError) {
    for mode in [LaneMode::Car, LaneMode::Sidewalk] {
        assert_eq!(Graph::try_build(n, mode).as_ref(), Err(want));
    }
    assert_eq!(try_connectors(n).as_ref(), Err(want));
    assert_eq!(AstarRouter::try_new(n).err().as_ref(), Some(want));
}

#[test]
fn broken_network_is_rejected_without_panic() {
    let base = || {
        let mut n = Network::new();
        road(&mut n, (0, 0), (4, 0), RoadKind::Small);
        road(&mut n, (4, 0), (4, 4), RoadKind::Small);
        n
    };
    let at = |n: &Network, x, y| n.node_at(TilePos::new(x, y)).unwrap();
    let seg = |n: &Network, a: (i64, i64), b: (i64, i64)| link(n, a, b, LaneMode::Car).segment;

    // 区間の端のノードがない。
    let mut n = base();
    let corner = at(&n, 4, 0);
    n.remove_node_unchecked(corner);
    assert_rejected(&n, &GraphError::MissingNode(corner));

    // 区間の車線がない。
    let mut n = base();
    let s = seg(&n, (0, 0), (4, 0));
    let lane = n.segment(s).unwrap().lanes[0];
    n.remove_lane_unchecked(lane);
    assert_rejected(&n, &GraphError::MissingLane(lane));

    // 区間の両端が 8 方向の直線で結べない。
    let mut n = base();
    road(&mut n, (4, 4), (6, 4), RoadKind::Small);
    let s = seg(&n, (0, 0), (4, 0));
    n.segment_mut(s).unwrap().b = at(&n, 6, 4);
    assert_rejected(&n, &GraphError::NotStraight(s));

    // ノードが、そのノードを端に持たない区間を指す。
    let mut n = base();
    let start = at(&n, 0, 0);
    let s2 = seg(&n, (4, 0), (4, 4));
    n.node_mut(start).unwrap().segments.push(s2);
    assert_rejected(
        &n,
        &GraphError::NodeNotOnSegment {
            node: start,
            segment: s2,
        },
    );
}

#[test]
fn node_pointing_at_a_missing_segment_is_rejected_without_panic() {
    let mut n = Network::new();
    road(&mut n, (0, 0), (4, 0), RoadKind::Small);
    // 別の網にだけある区間の ID は、この網にはない。
    let mut other = Network::new();
    road(&mut other, (0, 0), (4, 0), RoadKind::Small);
    road(&mut other, (4, 0), (4, 4), RoadKind::Small);
    let missing = link(&other, (4, 0), (4, 4), LaneMode::Car).segment;
    assert!(n.segment(missing).is_none());
    let start = n.node_at(TilePos::new(0, 0)).unwrap();
    n.node_mut(start).unwrap().segments.push(missing);
    assert_rejected(&n, &GraphError::MissingSegment(missing));
}

#[test]
fn overflowing_length_is_rejected_without_panic() {
    let mut n = Network::new();
    road(&mut n, (0, 0), (4, 0), RoadKind::Small);
    let s = link(&n, (0, 0), (4, 0), LaneMode::Car).segment;
    let (a, b) = {
        let seg = n.segment(s).unwrap();
        (seg.a, seg.b)
    };
    // 区間の長さが i64 の mm に収まらない。
    let far = 1_000_000_000_000_000;
    let at = |x: i64| WorldPos::new(Mm(x * TILE_MM), Mm(0));
    n.node_mut(a).unwrap().pos = at(-far);
    n.node_mut(b).unwrap().pos = at(far);
    for mode in [LaneMode::Car, LaneMode::Sidewalk] {
        assert_eq!(Graph::try_build(&n, mode), Err(GraphError::Overflow));
    }
    assert_eq!(AstarRouter::try_new(&n).err(), Some(GraphError::Overflow));
}

#[test]
fn try_new_matches_new_on_networks_built_by_commands() {
    let n = grid(4);
    let reqs = requests(&n, 50);
    assert_eq!(
        AstarRouter::try_new(&n).unwrap().route_batch(&reqs),
        AstarRouter::new(&n).route_batch(&reqs)
    );
    for mode in [LaneMode::Car, LaneMode::Sidewalk] {
        assert_eq!(Graph::try_build(&n, mode).unwrap(), Graph::build(&n, mode));
    }
}

#[test]
fn delays_steer_cars_to_the_parallel_road_and_survive_a_rebuild() {
    // (0, 0) から (20, 0) へ、まっすぐの道と、北へ回る道がある。
    let mut n = Network::new();
    road(&mut n, (0, 0), (20, 0), RoadKind::Small);
    road(&mut n, (0, 0), (0, -4), RoadKind::Small);
    road(&mut n, (0, -4), (20, -4), RoadKind::Small);
    road(&mut n, (20, -4), (20, 0), RoadKind::Small);
    road(&mut n, (20, 0), (24, 0), RoadKind::Small);
    road(&mut n, (-4, 0), (0, 0), RoadKind::Small);
    let car = LaneMode::Car;
    let req = RouteRequest {
        from: link(&n, (-4, 0), (0, 0), car),
        to: link(&n, (20, 0), (24, 0), car),
        mode: car,
    };
    let walk = RouteRequest {
        from: link(&n, (-4, 0), (0, 0), LaneMode::Sidewalk),
        to: link(&n, (20, 0), (24, 0), LaneMode::Sidewalk),
        mode: LaneMode::Sidewalk,
    };
    let straight = link(&n, (0, 0), (20, 0), car);
    let mut router = AstarRouter::new(&n);
    let free = router.route_batch(&[req, walk]);
    assert!(free[0].as_ref().unwrap().links.contains(&straight));
    // まっすぐの道に、回り道の 8 マスぶんより長い遅れを足すと、車は北へ回る。歩く経路は変わらない。
    let detour_ms = router
        .graph(car)
        .link_cost(link(&n, (0, 0), (0, -4), car))
        .unwrap()
        * 3;
    let delays = Delays::from_sorted([(straight, detour_ms)]);
    router.set_delays(&delays);
    assert_eq!(router.cached(), 1, "歩く経路の覚えた結果は残す");
    let jammed = router.route_batch(&[req, walk]);
    let r = jammed[0].as_ref().unwrap();
    assert!(!r.links.contains(&straight), "混んだ道を避けない {r:?}");
    check_route(router.graph(car), r);
    assert_eq!(jammed[1], free[1]);
    // 網を作り直しても遅れは残り、遅れを消せば元の経路に戻る。
    router.rebuild(&n);
    assert_eq!(router.delays(), &delays);
    assert_eq!(router.route_batch(&[req])[0], jammed[0]);
    router.set_delays(&Delays::default());
    assert_eq!(router.route_batch(&[req])[0], free[0]);
}
