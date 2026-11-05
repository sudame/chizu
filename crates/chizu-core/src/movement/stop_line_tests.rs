use super::stop_line::{
    DRAWN_CAR_HALF_MM, STOP_GAP_MM, STOP_WIDTH_MM, ZEBRA_MM, clear_line, stop_line,
};
use super::*;
use crate::command::{Heading, RoadKind};
use crate::network::{DIAGONAL_TILE_MM, Line, TILE_MM};

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

fn off(v: i64) -> LaneOffset {
    LaneOffset(u32::try_from(v).unwrap())
}

/// 小さい道路の十字路で、停止線は道路の端、横断歩道、すき間、停止線の幅、描く車の長さの半分だけ手前にあり、
/// 抜けきる所は、描く車の後ろの端が横断歩道を抜ける所にある。
#[test]
fn the_lines_sit_where_the_map_draws_them() {
    let mut n = Network::new();
    road(&mut n, (0, 10), (20, 10), RoadKind::Small);
    road(&mut n, (10, 0), (10, 20), RoadKind::Small);
    let half = 2_400;
    let into = link(&n, (9, 10), (10, 10));
    let out = link(&n, (10, 10), (11, 10));
    let len = 10 * TILE_MM;
    let back = half + ZEBRA_MM + STOP_GAP_MM + STOP_WIDTH_MM + DRAWN_CAR_HALF_MM;
    assert_eq!(stop_line(&n, into), off(len - back));
    assert_eq!(
        clear_line(&n, out),
        off(half + ZEBRA_MM + DRAWN_CAR_HALF_MM)
    );
    // 行き止まりの端には区間がない。
    assert_eq!(clear_line(&n, into), LaneOffset::START);
    assert_eq!(stop_line(&n, out), off(len));
    // 横断歩道を外した腕では、縞の分だけノードに寄る。
    let node = n.node_at(TilePos::new(10, 10)).unwrap();
    n.set_crosswalk(node, Heading::W, false).unwrap();
    assert_eq!(stop_line(&n, into), off(len - back + ZEBRA_MM));
    let back_out = link(&n, (10, 10), (9, 10));
    assert_eq!(clear_line(&n, back_out), off(half + DRAWN_CAR_HALF_MM));
}

#[test]
fn diagonal_arms_and_bends_are_measured_along_the_arm() {
    let mut n = Network::new();
    road(&mut n, (0, 10), (20, 10), RoadKind::Medium);
    road(&mut n, (10, 10), (10, 20), RoadKind::Medium);
    road(&mut n, (2, 2), (10, 10), RoadKind::Medium);
    let half = 3_200;
    let diag = link(&n, (9, 9), (10, 10));
    let len = 8 * DIAGONAL_TILE_MM;
    let edge = half * DIAGONAL_TILE_MM / TILE_MM;
    assert_eq!(
        stop_line(&n, diag),
        off(len - (edge + ZEBRA_MM + STOP_GAP_MM + STOP_WIDTH_MM + DRAWN_CAR_HALF_MM))
    );
    // 角では、道路の端から端までが通り抜けの区間で、停止線はない。
    let mut n = Network::new();
    road(&mut n, (0, 10), (10, 10), RoadKind::Medium);
    road(&mut n, (10, 10), (10, 0), RoadKind::Small);
    let into = link(&n, (9, 10), (10, 10));
    let out = link(&n, (10, 10), (10, 9));
    assert_eq!(stop_line(&n, into), off(10 * TILE_MM - half));
    assert_eq!(clear_line(&n, out), off(half));
}

/// 1 マスのリンクでは、停止線が手前の交差点の通り抜けの区間の中になる。そこを抜けた所は先の交差点の車道の四角に
/// かかるので、車は先の四角の縁で待つ。2 つの四角は重なるので、待つ車は手前の交差点の四角にかかる。
#[test]
fn on_a_one_tile_link_cars_wait_at_the_edge_of_the_box_ahead() {
    use super::stop_line::box_reach;
    let mut n = Network::new();
    road(&mut n, (0, 10), (20, 10), RoadKind::Medium);
    road(&mut n, (10, 0), (10, 20), RoadKind::Medium);
    road(&mut n, (11, 0), (11, 20), RoadKind::Medium);
    let short = link(&n, (10, 10), (11, 10));
    let ahead = n.node_at(TilePos::new(11, 10)).unwrap();
    let behind = n.node_at(TilePos::new(10, 10)).unwrap();
    let edge = 8_000 - box_reach(&n, ahead, short);
    assert!(edge < i64::from(clear_line(&n, short).0));
    assert_eq!(stop_line(&n, short), off(edge));
    assert!(edge < box_reach(&n, behind, short));
}

/// 大きい道路に車が途切れなく来るあいだ、小さい道路から入る車は停止線で待つ。
#[test]
fn a_yielding_car_waits_at_the_stop_line() {
    let mut n = Network::new();
    road(&mut n, (0, 10), (60, 10), RoadKind::Large);
    road(&mut n, (30, 0), (30, 10), RoadKind::Small);
    let mut router = AstarRouter::new(&n);
    let mut trips = Trips::default();
    let feed = |trips: &mut Trips, router: &mut AstarRouter| {
        let d = [(at(0, 10, (1, 0), 0), at(55, 10, (1, 0), 0)); 2];
        trips.spawn_drives(&n, router, &d, Tick::ZERO);
    };
    for _ in 0..60 {
        feed(&mut trips, &mut router);
        trips.step(&n, &mut router, Tick::ZERO);
    }
    let side = trips.spawn_drives(
        &n,
        &mut router,
        &[(at(30, 8, (0, 1), 4_000), at(55, 10, (1, 0), 0))],
        Tick::ZERO,
    )[0]
    .unwrap();
    let first = trips.get(side).unwrap().link();
    let line = stop_line(&n, first);
    let mut waited = 0;
    for _ in 0..20 {
        feed(&mut trips, &mut router);
        trips.step(&n, &mut router, Tick::ZERO);
        let t = trips.get(side).unwrap();
        assert_eq!(t.link(), first);
        assert!(t.offset <= line, "停止線を越えた {:?}", t.offset);
        if t.speed == MmPerTick::ZERO {
            assert_eq!(t.offset, line);
            waited += 1;
        }
    }
    assert!(waited > 0, "停止線で待たない");
}

/// 十字路のどの腕からどの腕へ向かう車も、止まるのは停止線の手前で、停止線を越えて交差点の中で止まる車はいない。
#[test]
fn cars_stop_behind_the_stop_line_and_not_inside_the_junction() {
    let mut n = Network::new();
    road(&mut n, (0, 10), (20, 10), RoadKind::Medium);
    road(&mut n, (10, 0), (10, 20), RoadKind::Medium);
    let mut router = AstarRouter::new(&n);
    let mut trips = Trips::default();
    let arms = [(1, 0), (-1, 0), (0, 1), (0, -1)];
    let mut drives = Vec::new();
    for (a, &(dx, dy)) in arms.iter().enumerate() {
        for (b, &(ex, ey)) in arms.iter().enumerate() {
            if a == b {
                continue;
            }
            let from = at(10 + 9 * dx, 10 + 9 * dy, (-dx as i8, -dy as i8), 0);
            let to = at(10 + 9 * ex, 10 + 9 * ey, (ex as i8, ey as i8), 0);
            drives.extend(std::iter::repeat_n((from, to), 4));
        }
    }
    let ids = trips.spawn_drives(&n, &mut router, &drives, Tick::ZERO);
    assert!(ids.iter().all(Option::is_some));
    let node = n.node_at(TilePos::new(10, 10)).unwrap();
    let (mut stopped_at_line, mut stopped_inside) = (0, 0);
    for _ in 0..600 {
        trips.step(&n, &mut router, Tick::ZERO);
        for (_, t) in trips.iter() {
            if !car::on_road(t) || t.speed != MmPerTick::ZERO || t.last_leg() {
                continue;
            }
            if car::end_node(&n, t.link()) != node {
                continue;
            }
            if t.bound.is_some() {
                stopped_inside += 1;
            } else {
                assert!(t.offset <= stop_line(&n, t.link()), "{:?}", t.offset);
                stopped_at_line += usize::from(t.offset == stop_line(&n, t.link()));
            }
        }
    }
    assert!(trips.iter().next().is_none(), "着かない車がいる");
    assert!(stopped_at_line > 0);
    assert_eq!(stopped_inside, 0);
}

/// tick の終わりに交差点 `node` の車道の四角にかかる車の、交差点の中の動き。停止線を越えた車は決めた車線への動き、
/// ノードを越えた車はいまの車線への動きを持つ。停止線を越えていないのに四角にかかる車は、入りうる車線への動きを
/// すべて持つ。
fn box_moves(n: &Network, trips: &Trips, node: NodeId) -> Vec<Vec<super::junction::Move>> {
    use super::junction::Move;
    use super::stop_line::box_reach;
    let mut inside = Vec::new();
    for (_, t) in trips.iter().filter(|(_, t)| car::on_road(t)) {
        let at = i64::from(t.offset.0) - i64::from(t.link_len.0);
        let ends_here = !t.last_leg() && car::end_node(n, t.link()) == node;
        if ends_here && at > -box_reach(n, node, t.link()) {
            let next = t.route.links[t.leg as usize + 1];
            let lanes: Vec<LaneId> = match car::bound_lane(n, t) {
                Some(l) => vec![l],
                None => n.link_lanes(next).collect(),
            };
            inside.push(
                lanes
                    .into_iter()
                    .map(|l| Move::into_lane(n, t.link(), next, l))
                    .collect(),
            );
        }
        if t.leg > 0 && t.bound.is_none() {
            let prev = t.route.links[t.leg as usize - 1];
            if car::end_node(n, prev) == node
                && i64::from(t.offset.0) < box_reach(n, node, t.link())
            {
                inside.push(vec![Move::into_lane(n, prev, t.link(), t.lane)]);
            }
        }
    }
    inside
}

/// [`box_moves`] のうち、交わる動きの組の数。
fn crossing_pairs(inside: &[Vec<super::junction::Move>]) -> usize {
    let mut pairs = 0;
    for (i, a) in inside.iter().enumerate() {
        for b in &inside[i + 1..] {
            let hit = a.iter().any(|x| b.iter().any(|y| x.conflicts_in(y)));
            pairs += usize::from(hit);
        }
    }
    pairs
}

/// 十字路を、交わる動きの車が途切れなく通っても、tick の終わりに車道の四角の中に交わる動きの車が 2 台いない。
/// 交差点の中の車が四角を抜けるのが、候補が四角に入るより先なら、候補はその車を待たずに入る。それでも
/// 四角の中では重ならないことを確かめる。
#[test]
fn crossing_moves_never_share_the_box() {
    let mut n = Network::new();
    road(&mut n, (0, 20), (40, 20), RoadKind::Medium);
    road(&mut n, (20, 0), (20, 40), RoadKind::Medium);
    let node = n.node_at(TilePos::new(20, 20)).unwrap();
    let mut router = AstarRouter::new(&n);
    let mut trips = Trips::default();
    let moves = [
        ((1, 0), (-1, 0)),
        ((-1, 0), (0, 1)),
        ((0, 1), (1, 0)),
        ((0, -1), (-1, 0)),
        ((1, 0), (0, 1)),
        ((0, 1), (0, -1)),
    ];
    let (mut arrived, mut busy) = (0, 0);
    for tick in 0..1_500 {
        if tick < 80 && tick % 2 == 0 {
            for &((dx, dy), (ex, ey)) in &moves {
                let from = at(20 + 19 * dx, 20 + 19 * dy, (-dx as i8, -dy as i8), 0);
                let to = at(20 + 19 * ex, 20 + 19 * ey, (ex as i8, ey as i8), 0);
                trips.spawn_drives(&n, &mut router, &[(from, to)], Tick::ZERO);
            }
        }
        arrived += trips.step(&n, &mut router, Tick::ZERO).len();
        let inside = box_moves(&n, &trips, node);
        busy += usize::from(!inside.is_empty());
        assert_eq!(crossing_pairs(&inside), 0, "{tick}: {inside:?}");
    }
    assert_eq!(arrived, 40 * moves.len());
    assert!(busy > 100, "四角を通る車が少ない {busy}");
}

/// 十字路の 3 マス先の四方に大きい道路の交差点があり、出口が詰まって交差点の中の車が速さを落としても、
/// 交わる動きの車が車道の四角に同時にいない。中の車がいまの速さで四角を抜けるという見込みは、
/// 中の車の先に止まりうる所が近いときは使わない。
#[test]
fn crossing_moves_never_share_the_box_when_the_exits_jam() {
    let mut n = Network::new();
    road(&mut n, (0, 20), (40, 20), RoadKind::Medium);
    road(&mut n, (20, 0), (20, 40), RoadKind::Medium);
    for k in [17, 23] {
        road(&mut n, (k, 0), (k, 40), RoadKind::Large);
        road(&mut n, (0, k), (40, k), RoadKind::Large);
    }
    let node = n.node_at(TilePos::new(20, 20)).unwrap();
    let mut router = AstarRouter::new(&n);
    let mut trips = Trips::default();
    let moves = [
        ((1, 0), (-1, 0)),
        ((-1, 0), (0, 1)),
        ((0, 1), (1, 0)),
        ((0, -1), (-1, 0)),
        ((1, 0), (0, 1)),
        ((0, 1), (0, -1)),
        ((-1, 0), (1, 0)),
        ((0, -1), (0, 1)),
    ];
    let mut busy = 0;
    for tick in 0..3_000 {
        if tick < 1_000 {
            for &((dx, dy), (ex, ey)) in &moves {
                let from = at(20 + 19 * dx, 20 + 19 * dy, (-dx as i8, -dy as i8), 0);
                let to = at(20 + 19 * ex, 20 + 19 * ey, (ex as i8, ey as i8), 0);
                trips.spawn_drives(&n, &mut router, &[(from, to)], Tick::ZERO);
            }
            // 大きい道路を流れる車が、十字路の出口の先の交差点を詰まらせる。
            for k in [17, 23] {
                for y in [1, 5] {
                    let d = [
                        (at(k, y, (0, 1), 0), at(k, 38, (0, 1), 0)),
                        (at(y, k, (1, 0), 0), at(38, k, (1, 0), 0)),
                        (at(k, 39 - y, (0, -1), 0), at(k, 2, (0, -1), 0)),
                        (at(39 - y, k, (-1, 0), 0), at(2, k, (-1, 0), 0)),
                    ];
                    trips.spawn_drives(&n, &mut router, &d, Tick::ZERO);
                }
            }
        }
        trips.step(&n, &mut router, Tick::ZERO);
        let inside = box_moves(&n, &trips, node);
        busy += usize::from(!inside.is_empty());
        assert_eq!(crossing_pairs(&inside), 0, "{tick}: {inside:?}");
    }

    assert!(busy > 100, "四角を通る車が少ない {busy}");
    assert!(trips.iter().next().is_none(), "着かない車がいる");
}

/// 1 マスのリンクの先が大きい道路の交差点で、その道路に車が途切れなく流れても、1 マスのリンクから来る車と
/// 交わる車が、先の交差点の車道の四角に同時にいない。1 マスのリンクへ入る車は、先の交差点の停止線で
/// 止まれる速さで入り、停止線は先の四角の縁にある。
#[test]
fn one_tile_link_cars_never_share_the_box_ahead() {
    let mut n = Network::new();
    road(&mut n, (0, 10), (30, 10), RoadKind::Medium);
    road(&mut n, (10, 0), (10, 20), RoadKind::Medium);
    road(&mut n, (11, 0), (11, 30), RoadKind::Large);
    let node = n.node_at(TilePos::new(11, 10)).unwrap();
    let mut router = AstarRouter::new(&n);
    let mut trips = Trips::default();
    let mut busy = 0;
    for tick in 0..1_500 {
        if tick < 400 {
            let mut d = vec![(at(1, 10, (1, 0), 0), at(25, 10, (1, 0), 0))];
            for y in [1, 5] {
                d.push((at(11, y, (0, 1), 0), at(11, 28, (0, 1), 0)));
                d.push((at(11, 29 - y, (0, -1), 0), at(11, 1, (0, -1), 0)));
            }
            trips.spawn_drives(&n, &mut router, &d, Tick::ZERO);
        }
        trips.step(&n, &mut router, Tick::ZERO);
        let inside = box_moves(&n, &trips, node);
        busy += usize::from(inside.len() > 1);
        assert_eq!(crossing_pairs(&inside), 0, "{tick}: {inside:?}");
    }
    assert!(busy > 100, "四角を通る車が少ない {busy}");
    assert!(trips.iter().next().is_none(), "着かない車がいる");
}
