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
                // 候補の数より大きい値も混ぜ、余りで選ぶことも確かめる。
                variant: (i % 11) as u8,
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
        let n = variants_of(r.mode);
        let want = astar_variants(router.graph(r.mode), r.from, r.to, &mut s, n)
            .map(|v| v[usize::from(r.variant) % v.len()].clone());
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
        variant: 0,
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
    // 大きさは経路のリンクの数で数えるので、上限を超える前に一部だけを覚える。
    let mut router = AstarRouter::new(&n).with_cache_limit(60);
    assert_eq!(router.route_batch(&reqs), want);
    assert!(router.cached() > 0);
    assert!(router.cached_size() <= 60);
    assert_eq!(router.route_batch(&reqs), want);
    assert!(router.cached_size() <= 60);
}

#[test]
fn bundles_in_the_direction_that_covers_more_requests() {
    let n = grid(8);
    let links: Vec<LinkId> = n.links().filter(|l| l.mode == LaneMode::Car).collect();
    let req = |from: usize, to: usize| RouteRequest {
        from: links[from],
        to: links[to],
        mode: LaneMode::Car,
        variant: 0,
    };
    // 1 か所から 60 か所へ散らばる依頼と、その 60 か所へほかから 2 件ずつ来る依頼。
    // 着くリンクの組（60 組、3 件ずつ）のほうが少ないが、下限に届くのは出発のリンクの組だけである。
    let mut reqs: Vec<RouteRequest> = (1..=60).map(|t| req(0, t)).collect();
    reqs.extend((1..=60).flat_map(|t| [req(60 + t, t), req(120 + t, t)]));
    reqs.sort();
    let router = AstarRouter::new(&n);
    let keys: Vec<Key> = reqs.iter().map(RouteRequest::key).collect();
    let jobs = router.plan(&keys);
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
            variant: 0,
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
            variant: 0,
        })
        .collect();
    reqs.sort();
    let mut s = Scratches::default();
    for r in &reqs {
        router.solve(&Job::Single(r.key()), &mut s);
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
                let best = dijkstra(&g, a, b);
                let r = astar(&g, g.link(a), g.link(b), &mut s);
                prop_assert_eq!(r.as_ref().map(|r| r.cost), best);
                if let Some(r) = r {
                    check_route(&g, &r);
                }
                // どの候補も、出発から着くリンクまで辺でつながり、たどって足した費用が最短の費用になる。
                let vs = astar_variants(&g, g.link(a), g.link(b), &mut s, ROUTE_VARIANTS);
                prop_assert_eq!(vs.is_some(), best.is_some());
                for v in vs.iter().flatten() {
                    prop_assert_eq!(v.links.first(), Some(&g.link(a)));
                    prop_assert_eq!(v.links.last(), Some(&g.link(b)));
                    check_route(&g, v);
                    prop_assert_eq!(Some(v.cost), best);
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

/// 着くリンクで束ねても出発のリンクで束ねても、どの候補でも A* と同じ経路になることを確かめる。
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
    let n = ROUTE_VARIANTS;
    let mut m = Marks::default();
    let root = search_many(g, to, &starts, Toward::Backward, &mut s).unwrap();
    let back: Vec<_> = starts
        .iter()
        .map(|a| extract(g, &s, &mut m, root, *a, Toward::Backward, n))
        .collect();
    for (a, got) in starts.iter().zip(back) {
        assert_eq!(
            got,
            astar_variants(g, *a, to, &mut s, n),
            "{a:?} から {to:?}"
        );
    }
    let root = search_many(g, from, &goals, Toward::Forward, &mut s).unwrap();
    let fwd: Vec<_> = goals
        .iter()
        .map(|b| extract(g, &s, &mut m, root, *b, Toward::Forward, n))
        .collect();
    for (b, got) in goals.iter().zip(fwd) {
        assert_eq!(
            got,
            astar_variants(g, from, *b, &mut s, n),
            "{from:?} から {b:?}"
        );
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

#[test]
fn variants_spread_over_routes_of_the_same_cost_and_keep_to_straight_roads() {
    // 同じ道路の 6 × 6 区画の格子で、角から向かいの角へ行く。最短経路はすべて同じ費用である。
    let mut n = Network::new();
    for i in 0..=6 {
        road(&mut n, (0, i * 3), (18, i * 3), RoadKind::Small);
        road(&mut n, (i * 3, 0), (i * 3, 18), RoadKind::Small);
    }
    let g = Graph::build(&n, LaneMode::Car);
    let from = link(&n, (0, 0), (3, 0), LaneMode::Car);
    let to = link(&n, (18, 15), (18, 18), LaneMode::Car);
    let routes = astar_variants(&g, from, to, &mut Scratch::default(), ROUTE_VARIANTS).unwrap();
    let best = dijkstra(&g, g.vertex(from).unwrap(), g.vertex(to).unwrap()).unwrap();
    for r in &routes {
        check_route(&g, r);
        assert_eq!(r.cost, best);
    }
    let mut distinct: Vec<&Vec<LinkId>> = routes.iter().map(|r| &r.links).collect();
    distinct.sort();
    distinct.dedup();
    assert!(
        distinct.len() >= 4,
        "候補が {} 本にしか分かれない",
        distinct.len()
    );
    // 曲がる回数。区間ごとに 1 歩の向きが変わったら 1 回と数える。
    let turns = |r: &Route| {
        let steps: Vec<_> = r
            .links
            .iter()
            .map(|l| {
                let line = n.segment_line(l.segment).unwrap();
                match l.dir {
                    Dir::Forward => line.step,
                    Dir::Backward => (-line.step.0, -line.step.1),
                }
            })
            .collect();
        steps.windows(2).filter(|w| w[0] != w[1]).count()
    };
    // 直線ごとに順を決めるので、交差点ごとに向きを選ぶより曲がる回数が少ない。東へ 6 区画、北へ 6 区画の
    // 最短経路を一様に選ぶと、平均で 6 回曲がる。
    let total: usize = routes.iter().map(turns).sum();
    assert!(total < 4 * routes.len(), "曲がる回数が多すぎる {total}");
}

#[test]
fn batch_picks_the_requested_variant_from_one_search() {
    let n = grid(6);
    let links: Vec<LinkId> = n.links().filter(|l| l.mode == LaneMode::Car).collect();
    let (from, to) = (links[0], links[links.len() - 1]);
    let reqs: Vec<RouteRequest> = (0..ROUTE_VARIANTS * 2)
        .map(|variant| RouteRequest {
            from,
            to,
            mode: LaneMode::Car,
            variant,
        })
        .collect();
    let mut router = AstarRouter::new(&n);
    let got = router.route_batch(&reqs);
    // 候補は 1 つの鍵にまとめて覚える。大きさは、鍵の 1 に、共有した経路を 1 回だけ数えたリンクの数を足したもの。
    assert_eq!(router.cached(), 1);
    let mut distinct: Vec<&Vec<LinkId>> = got.iter().flatten().map(|r| &r.links).collect();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        router.cached_size(),
        1 + distinct.iter().map(|l| l.len()).sum::<usize>()
    );
    let want = astar_variants(
        router.graph(LaneMode::Car),
        from,
        to,
        &mut Scratch::default(),
        ROUTE_VARIANTS,
    )
    .unwrap();
    for (r, got) in reqs.iter().zip(&got) {
        assert_eq!(
            **got.as_ref().unwrap(),
            want[usize::from(r.variant) % want.len()]
        );
    }
    // 同じ経路の候補は 1 つを共有する。
    let a = got[0].as_ref().unwrap();
    let shared = got.iter().flatten().filter(|g| Arc::ptr_eq(g, a)).count();
    let same = got.iter().flatten().filter(|g| ***g == **a).count();
    assert_eq!(shared, same);
    // 歩く経路は散らさない。
    let walk = |variant| RouteRequest {
        from: LinkId {
            mode: LaneMode::Sidewalk,
            ..from
        },
        to: LinkId {
            mode: LaneMode::Sidewalk,
            ..to
        },
        mode: LaneMode::Sidewalk,
        variant,
    };
    let w = router.route_batch(&[walk(0), walk(3)]);
    assert_eq!(w[0], w[1]);
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
        variant: 0,
    };
    let walk = RouteRequest {
        from: link(&n, (-4, 0), (0, 0), LaneMode::Sidewalk),
        to: link(&n, (20, 0), (24, 0), LaneMode::Sidewalk),
        mode: LaneMode::Sidewalk,
        variant: 0,
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

#[test]
fn delays_never_raise_a_link_cost_past_the_cap() {
    let mut n = Network::new();
    road(&mut n, (0, 0), (20, 0), RoadKind::Small);
    let straight = link(&n, (0, 0), (20, 0), LaneMode::Car);
    let mut router = AstarRouter::new(&n);
    let free = router.graph(LaneMode::Car).link_cost(straight).unwrap();
    // 上限より短い遅れはそのまま足し、長い遅れは自由に走れたときの所要時間の上限の倍までに収める。
    router.set_delays(&Delays::from_sorted([(straight, free)]));
    assert_eq!(
        router.graph(LaneMode::Car).link_cost(straight),
        Some(2 * free)
    );
    router.set_delays(&Delays::from_sorted([(straight, 100 * free)]));
    assert_eq!(
        router.graph(LaneMode::Car).link_cost(straight),
        Some(graph::DELAY_CAP_TIMES * free)
    );
}

#[test]
fn signal_wait_adds_to_queue_delays_but_not_to_measured_ones() {
    let mut n = Network::new();
    road(&mut n, (0, 0), (20, 0), RoadKind::Small);
    road(&mut n, (20, -10), (20, 10), RoadKind::Small);
    let node = n.node_at(TilePos::new(20, 0)).unwrap();
    n.set_signal(node, true).unwrap();
    let car = LaneMode::Car;
    let into = link(&n, (0, 0), (20, 0), car);
    let away = link(&n, (20, 0), (0, 0), car);
    let cross = link(&n, (20, -10), (20, 0), car);
    let mut router = AstarRouter::new(&n);
    let g = router.graph(car);
    let free = g.link_cost(away).unwrap();
    // 交わる軸に車が来なければ、待つのは黄と全方向の赤のあいだだけである。
    let idle = 5 * 5 * 1_000 / 70;
    assert_eq!(g.signal_wait(into), Some(idle));
    assert_eq!(g.signal_wait(away), Some(0));
    assert_eq!(g.link_cost(into), Some(free + idle));
    assert_eq!(
        router
            .graph(LaneMode::Sidewalk)
            .signal_wait(link(&n, (0, 0), (20, 0), LaneMode::Sidewalk)),
        Some(0)
    );
    // 交わる軸の腕が忙しく、青を使い切ると、十字路と同じ見込みになる。信号へ向かうリンクにだけ足す。
    let busy = |d: Delays| d.with_busy([(cross, 1_000)]);
    let wait = 40 * 40 * 1_000 / 140;
    router.set_delays(&busy(Delays::default()));
    assert_eq!(router.graph(car).signal_wait(into), Some(wait));
    assert_eq!(router.graph(car).link_cost(away), Some(free));
    assert_eq!(router.graph(car).link_cost(into), Some(free + wait));
    // 並んでいる台数から見積もった遅れには、待ちの見込みを足す。遅れの上限は見込みの手前で決める。
    router.set_delays(&busy(Delays::from_sorted([(into, 5_000)])));
    assert_eq!(router.graph(car).link_cost(into), Some(free + 5_000 + wait));
    router.set_delays(&busy(Delays::from_sorted([(into, 100 * free)])));
    assert_eq!(
        router.graph(car).link_cost(into),
        Some(graph::DELAY_CAP_TIMES * free + wait)
    );
    // 抜けた台数から測った遅れは信号で待った時間を含むので、見込みとの長いほうにする。
    let measured = |ms| busy(Delays::from_rows([(into, Delay { ms, measured: true })]));
    router.set_delays(&measured(wait / 2));
    assert_eq!(router.graph(car).link_cost(into), Some(free + wait));
    router.set_delays(&measured(wait + 3_000));
    assert_eq!(router.graph(car).link_cost(into), Some(free + wait + 3_000));
    // 信号を外すと、遅れはそのままに、見込みだけを除く。
    n.set_signal(node, false).unwrap();
    router.set_signals(&n);
    assert_eq!(router.graph(car).signal_wait(into), Some(0));
    router.set_delays(&busy(Delays::from_sorted([(into, 5_000)])));
    assert_eq!(router.graph(car).link_cost(into), Some(free + 5_000));
}

#[test]
fn busy_links_without_signals_keep_the_cached_routes() {
    let n = grid(6);
    let mut router = AstarRouter::new(&n);
    router.route_batch(&requests(&n, 300));
    let all = router.cached();
    assert!(all > 0);
    // 信号がなければ、忙しさは費用を変えないので、覚えた結果を捨てない。
    let l = n.links().find(|l| l.mode == LaneMode::Car).unwrap();
    router.set_delays(&Delays::default().with_busy([(l, 500)]));
    assert_eq!(router.cached(), all);
    assert_eq!(router.delays().busy().count(), 1);
}

#[test]
fn placing_a_signal_drops_only_the_car_routes_it_may_change() {
    let mut n = grid(6);
    let mut router = AstarRouter::new(&n);
    router.route_batch(&requests(&n, 300));
    let walks = router
        .cache
        .keys()
        .filter(|k| k.mode == LaneMode::Sidewalk)
        .count();
    let all = router.cached();
    assert!(walks > 0 && walks < all);
    // 交差点でないノードは信号を置けないので、何も変わらない。
    router.set_signals(&n);
    assert_eq!(router.cached(), all);
    let (junction, _) = n.nodes().find(|(_, x)| x.segments.len() >= 3).unwrap();
    n.set_signal(junction, true).unwrap();
    router.set_signals(&n);
    assert_eq!(router.cached(), walks, "歩く経路の覚えた結果は残す");
    check_cached_size(&router);
    // 網がグラフより新しければ、作り直すまで何もしない。
    let version = router.version();
    router.route_batch(&requests(&n, 300));
    let cached = router.cached();
    road(&mut n, (100, 100), (110, 100), RoadKind::Small);
    n.set_signal(junction, false).unwrap();
    router.set_signals(&n);
    assert_eq!((router.version(), router.cached()), (version, cached));
    router.rebuild(&n);
    assert_eq!(
        router.graph(LaneMode::Car),
        AstarRouter::new(&n).graph(LaneMode::Car)
    );
}

/// 覚えている結果の大きさが、残っている結果から数え直した値と同じであることを確かめる。
fn check_cached_size(router: &AstarRouter) {
    let want: usize = router.cache.values().map(size_of_entry).sum();
    assert_eq!(router.cached_size(), want);
}

#[test]
fn cached_size_follows_the_cache_through_delays_and_bands() {
    let n = grid(6);
    let mut router = AstarRouter::new(&n);
    router.route_batch(&requests(&n, 300));
    let walks = router
        .cache
        .keys()
        .filter(|k| k.mode == LaneMode::Sidewalk)
        .count();
    assert!(walks > 0 && walks < router.cached());
    check_cached_size(&router);
    // 遅れが変わると車の結果だけを捨てるので、大きさも歩く結果のぶんだけになる。
    let car = n.links().find(|l| l.mode == LaneMode::Car).unwrap();
    router.set_delays(&Delays::from_sorted([(car, 5_000)]));
    assert_eq!(router.cached(), walks);
    check_cached_size(&router);
    // 数え直した大きさから、また覚え足していける。
    router.route_batch(&requests(&n, 300));
    check_cached_size(&router);
    router.set_band(1);
    assert_eq!((router.cached(), router.cached_size()), (0, 0));
    router.route_batch(&requests(&n, 300));
    check_cached_size(&router);
}

#[test]
fn surely_connected_links_always_have_a_route() {
    let mut n = grid(3);
    // 離れた道路は、どこともつながらない。
    road(&mut n, (100, 100), (110, 100), RoadKind::Small);
    let router = AstarRouter::new(&n);
    let mut s = Scratch::default();
    let mut connected = 0;
    for mode in [LaneMode::Car, LaneMode::Sidewalk] {
        let links: Vec<LinkId> = n.links().filter(|l| l.mode == mode).collect();
        for a in &links {
            for b in &links {
                if a != b && router.surely_connected(mode, *a, *b) {
                    connected += 1;
                    assert!(
                        astar(router.graph(mode), *a, *b, &mut s).is_some(),
                        "{a:?} から {b:?} への経路がない"
                    );
                }
            }
        }
    }
    assert!(connected > 0);
    let far = link(&n, (100, 100), (110, 100), LaneMode::Car);
    let near = link(&n, (0, 0), (4, 0), LaneMode::Car);
    assert!(router.surely_connected(LaneMode::Car, near, link(&n, (4, 4), (0, 4), LaneMode::Car)));
    assert!(!router.surely_connected(LaneMode::Car, near, far));
}

/// 外の世界へ出る高速道路と、行き止まりと、離れた道路のある網。外の世界のノードでは折り返せないので、
/// 出ていく高速道路のリンクは抜け出せない行き止まりになり、入ってくるリンクには戻れない。
fn highway_city() -> Network {
    let mut n = Network::new();
    n.add_highway(TilePos::new(8, 8), TilePos::new(8, 40));
    for i in 0..=2 {
        road(&mut n, (0, i * 4), (8, i * 4), RoadKind::Small);
        road(&mut n, (i * 4, 0), (i * 4, 8), RoadKind::Medium);
    }
    road(&mut n, (0, 8), (-6, 8), RoadKind::Small);
    road(&mut n, (100, 100), (110, 100), RoadKind::Small);
    n
}

#[test]
fn surely_connected_agrees_with_the_search_around_a_highway_exit() {
    let n = highway_city();
    let router = AstarRouter::new(&n);
    let mut s = Scratch::default();
    for mode in [LaneMode::Car, LaneMode::Sidewalk] {
        let g = router.graph(mode);
        let links: Vec<LinkId> = n.links().filter(|l| l.mode == mode).collect();
        let (mut sure, mut found, mut missing) = (0, 0, 0);
        for a in &links {
            for b in &links {
                let route = if a == b {
                    // 同じリンクへ戻る輪があるか。次のリンクのどれかから戻れればよい。
                    let v = g.vertex(*a).unwrap();
                    g.edges(v)
                        .iter()
                        .any(|e| astar(g, g.link(e.to), *a, &mut s).is_some())
                } else {
                    astar(g, *a, *b, &mut s).is_some()
                };
                let surely = router.surely_connected(mode, *a, *b);
                assert!(!surely || route, "{mode:?} {a:?} から {b:?} へは経路がない");
                sure += usize::from(surely);
                found += usize::from(route);
                missing += usize::from(!route);
            }
        }
        assert!(sure > 0 && missing > 0, "{mode:?} {sure} {missing}");
        if mode == LaneMode::Car {
            // 高速道路へ出ていくリンクには行けるが、成分が違うので網の形だけでは言えない組がある。
            assert!(found > sure, "{found} {sure}");
        }
    }
    // 外の世界へ出ていく高速道路のリンクからは、どこへも行けない。
    let out = link(&n, (8, 8), (8, 40), LaneMode::Car);
    let back = link(&n, (8, 40), (8, 8), LaneMode::Car);
    assert!(!router.surely_connected(LaneMode::Car, out, back));
    assert!(astar(router.graph(LaneMode::Car), out, back, &mut s).is_none());
}

/// 入口が道のりの上限の中にある頂点への距離は、根からの A* の費用と同じになり、入口が上限を超える頂点は入らない。
/// 辺を逆にたどった距離は、その頂点から根への A* の費用と同じになる。根の始めの距離を変えると、その分だけずれる。
#[test]
fn reach_within_matches_astar_in_both_directions() {
    let n = grid(6);
    let g = Graph::build(&n, LaneMode::Sidewalk);
    let links: Vec<LinkId> = n.links().filter(|l| l.mode == LaneMode::Sidewalk).collect();
    let mut s = Scratch::default();
    for root in links.iter().step_by(5) {
        let base = g.link_cost(*root).unwrap();
        let limit = base + 40_000;
        for (toward, extra) in [(Toward::Forward, 0), (Toward::Backward, 700)] {
            let got = reach_within(&g, &[(*root, base + extra)], toward, limit + extra, &mut s);
            assert!(
                got.windows(2).all(|w| w[0].0 < w[1].0),
                "リンクの順に並ばない"
            );
            for other in &links {
                let want = match toward {
                    Toward::Forward => find(&n, *root, *other),
                    Toward::Backward => find(&n, *other, *root),
                }
                .map(|r| r.cost + extra);
                let found = got.iter().find(|(l, _)| l == other).map(|(_, d)| *d);
                let entry = |w: u64| w - g.link_cost(*other).unwrap();
                match want {
                    Some(w) if other == root || entry(w) <= limit + extra => {
                        assert_eq!(found, Some(w), "{other:?}")
                    }
                    _ => assert_eq!(found, None, "{other:?}"),
                }
            }
        }
    }
}
