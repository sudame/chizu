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

#[test]
fn on_a_one_tile_link_cars_wait_clear_of_the_junction_behind() {
    let mut n = Network::new();
    road(&mut n, (0, 10), (20, 10), RoadKind::Medium);
    road(&mut n, (10, 0), (10, 20), RoadKind::Medium);
    road(&mut n, (11, 0), (11, 20), RoadKind::Medium);
    let short = link(&n, (10, 10), (11, 10));
    assert_eq!(stop_line(&n, short), clear_line(&n, short));
    assert!(stop_line(&n, short).0 < 8_000);
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

/// 十字路を、交わる動きの車が途切れなく通っても、tick の終わりに車道の四角の中に交わる動きの車が 2 台いない。
/// 交差点の中の車が四角を抜けるのが、候補が四角に入るより先なら、候補はその車を待たずに入る。それでも
/// 四角の中では重ならないことを確かめる。
#[test]
fn crossing_moves_never_share_the_box() {
    use super::junction::Move;
    use super::stop_line::box_reach;
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
        let mut inside = Vec::new();
        for (_, t) in trips.iter().filter(|(_, t)| car::on_road(t)) {
            if let Some(l) = car::bound_lane(&n, t) {
                let at = i64::from(t.offset.0) - i64::from(t.link_len.0);
                if car::end_node(&n, t.link()) == node && at > -box_reach(&n, node, t.link()) {
                    let next = t.route.links[t.leg as usize + 1];
                    inside.push(Move::into_lane(&n, t.link(), next, l));
                }
            } else if t.leg > 0 {
                let prev = t.route.links[t.leg as usize - 1];
                if car::end_node(&n, prev) == node
                    && i64::from(t.offset.0) < box_reach(&n, node, t.link())
                {
                    inside.push(Move::into_lane(&n, prev, t.link(), t.lane));
                }
            }
        }
        busy += usize::from(!inside.is_empty());
        for (i, a) in inside.iter().enumerate() {
            for b in &inside[i + 1..] {
                assert!(!a.conflicts_in(b), "{tick}: {a:?} と {b:?} が四角の中にいる");
            }
        }
    }
    assert_eq!(arrived, 40 * moves.len());
    assert!(busy > 100, "四角を通る車が少ない {busy}");
}
