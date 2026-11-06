//! 踏切の手前で止まる車と歩く人のテスト。

use std::collections::BTreeSet;

use super::*;
use crate::command::RoadKind;
use crate::network::Line;

fn at(x: i64, y: i64, step: (i8, i8), along: u32) -> SidewalkPos {
    SidewalkPos {
        tile: TilePos::new(x, y),
        step,
        along: LaneOffset(along),
    }
}

/// x 0〜60 の y = 0 の道路。
fn street() -> (Network, AstarRouter) {
    let mut n = Network::new();
    let line = Line::between(TilePos::new(0, 0), TilePos::new(60, 0)).unwrap();
    n.build_road(&line, RoadKind::Small).unwrap();
    let r = AstarRouter::new(&n);
    (n, r)
}

fn crossings(net: &Network, tiles: &[i64], closed: &[i64]) -> RailCrossings {
    let closed: BTreeSet<TilePos> = closed.iter().map(|x| TilePos::new(*x, 0)).collect();
    RailCrossings::new(net, tiles.iter().map(|x| TilePos::new(*x, 0)), &closed)
}

/// 東へ走る車の体（真ん中から前後へ車の長さの半分）が、x のマスにかかっているか。
fn car_on_tile(t: &Trip, x: i64) -> bool {
    let at = i64::from(t.offset.0);
    let half = i64::from(CAR_LENGTH / 2);
    let (near, far) = (x * 8_000 - 4_000, x * 8_000 + 4_000);
    at - half < far && near < at + half
}

#[test]
fn a_car_stops_before_a_closed_crossing_and_goes_when_it_opens() {
    let (net, mut router) = street();
    let mut trips = Trips::default();
    let id = trips.spawn_drives(
        &net,
        &mut router,
        &[(at(5, 0, (1, 0), 0), at(55, 0, (1, 0), 0))],
        Tick::ZERO,
    )[0]
    .unwrap();
    let closed = crossings(&net, &[30], &[30]);
    for _ in 0..200 {
        trips.step_with_rail(&net, &mut router, Tick::ZERO, &closed);
        let t = trips.get(id).expect("閉じた踏切を抜けて着いた");
        assert!(!car_on_tile(t, 30), "{t:?}");
    }
    let t = trips.get(id).unwrap();
    assert_eq!(t.speed, MmPerTick::ZERO);
    // 手前の端から 1〜6m の所で止まる。
    let front = i64::from(t.offset.0) + i64::from(CAR_LENGTH / 2);
    assert!(
        (30 * 8_000 - 4_000 - 6_000..=30 * 8_000 - 4_000).contains(&front),
        "{front}"
    );
    let (occupied, waits) = closed.traffic(&trips);
    assert!(occupied.is_empty());
    assert_eq!(waits, 1);
    let open = crossings(&net, &[30], &[]);
    let mut arrived = false;
    for _ in 0..200 {
        if trips
            .step_with_rail(&net, &mut router, Tick::ZERO, &open)
            .contains(&id)
        {
            arrived = true;
            break;
        }
    }
    assert!(arrived);
}

#[test]
fn a_walker_waits_before_a_closed_crossing() {
    let (net, mut router) = street();
    let mut trips = Trips::default();
    let id = trips
        .spawn(
            &net,
            &mut router,
            at(27, 0, (1, 0), 0),
            at(33, 0, (1, 0), 0),
            Tick::ZERO,
        )
        .unwrap();
    let closed = crossings(&net, &[30], &[30]);
    for _ in 0..100 {
        trips.step_with_rail(&net, &mut router, Tick::ZERO, &closed);
        let t = trips.get(id).expect("閉じた踏切を渡って着いた");
        assert!(t.offset.0 < 30 * 8_000 - 4_000);
    }
    assert!(closed.traffic(&trips).0.is_empty());
    let open = crossings(&net, &[30], &[]);
    let mut inside = false;
    for _ in 0..100 {
        trips.step_with_rail(&net, &mut router, Tick::ZERO, &open);
        inside |= !open.traffic(&trips).0.is_empty();
        if trips.get(id).is_none() {
            break;
        }
    }
    assert!(inside);
    assert!(trips.get(id).is_none());
}

/// 踏切の先に抜けきる空きがなければ、開いていても入らずに手前で待つ。
#[test]
fn a_car_does_not_enter_a_crossing_without_room_beyond_it() {
    let (net, mut router) = street();
    let mut trips = Trips::default();
    let trips_to = |trips: &mut Trips, router: &mut AstarRouter| {
        trips.spawn_drives(
            &net,
            router,
            &[(at(5, 0, (1, 0), 0), at(55, 0, (1, 0), 0))],
            Tick::ZERO,
        )[0]
        .unwrap()
    };
    // 先の車は、x = 32 の閉じた踏切の手前で止まる。その尾は x = 30 の踏切のすぐ先にある。
    let first = trips_to(&mut trips, &mut router);
    let rail = crossings(&net, &[30, 32], &[32]);
    for _ in 0..20 {
        trips.step_with_rail(&net, &mut router, Tick::ZERO, &rail);
    }
    let second = trips_to(&mut trips, &mut router);
    for _ in 0..200 {
        trips.step_with_rail(&net, &mut router, Tick::ZERO, &rail);
        let t = trips.get(second).unwrap();
        assert!(!car_on_tile(t, 30), "{t:?}");
    }
    assert_eq!(trips.get(first).unwrap().speed, MmPerTick::ZERO);
    assert_eq!(trips.get(second).unwrap().speed, MmPerTick::ZERO);
}

/// 歩いて行った駐車場の出入りの位置 `lot` で車に乗り、x = 55 へ走る移動を作る。
fn walk_then_drive(
    trips: &mut Trips,
    net: &Network,
    router: &mut AstarRouter,
    from: SidewalkPos,
    lot: SidewalkPos,
) -> TripId {
    let id = trips.spawn(net, router, from, lot, Tick::ZERO).unwrap();
    trips.rows[id.slot()].as_mut().unwrap().then = parking::Then::Drive {
        park_at: at(55, 0, (1, 0), 0),
        lot: None,
        to: at(56, 0, (1, 0), 0),
    };
    id
}

/// 駐車場の出入りの位置で車に乗るとき、その位置が踏切のすぐ手前でも、車の体が踏切にかかる所には置かない。
/// 閉じた踏切にかかる所に置くと、前の端が踏切に入っている車として止まらずに踏切を通り、列車とぶつかりかける。
#[test]
fn a_car_leaving_a_lot_next_to_a_closed_crossing_waits_before_it() {
    let (net, mut router) = street();
    let mut trips = Trips::default();
    // 乗る位置は、車の真ん中が踏切（x = 30）の手前の端の 1.5m 手前になる所である。
    let lot = at(29, 0, (1, 0), 2_500);
    let id = walk_then_drive(&mut trips, &net, &mut router, at(26, 0, (1, 0), 0), lot);
    let closed = crossings(&net, &[30], &[30]);
    let mut drove = false;
    for _ in 0..300 {
        trips.step_with_rail(&net, &mut router, Tick::ZERO, &closed);
        let t = trips.get(id).expect("閉じた踏切を抜けて着いた");
        assert!(closed.traffic(&trips).0.is_empty(), "{t:?}");
        drove |= t.mode == LaneMode::Car;
    }
    assert!(drove, "{:?}", trips.get(id));
    let t = trips.get(id).unwrap();
    assert_eq!(t.mode, LaneMode::Car);
    assert!(!car_on_tile(t, 30), "{t:?}");
    // 開けば、車は踏切を通って着く。
    let open = crossings(&net, &[30], &[]);
    let mut arrived = false;
    for _ in 0..300 {
        if trips
            .step_with_rail(&net, &mut router, Tick::ZERO, &open)
            .contains(&id)
        {
            arrived = true;
            break;
        }
    }
    assert!(arrived);
}

/// 開いた踏切にも、車の体がかかる所には置かない。踏切の先に抜けきる空きがないときに、踏切の中に止まる車を
/// 出さないためである。
#[test]
fn a_departing_car_is_not_placed_on_an_open_crossing() {
    let (net, mut router) = street();
    let mut trips = Trips::default();
    // 乗る位置は、車の尾が踏切（x = 30）の向こうの端にかかる所である。
    let lot = at(30, 0, (1, 0), 5_000);
    let id = walk_then_drive(&mut trips, &net, &mut router, at(33, 0, (-1, 0), 0), lot);
    let open = crossings(&net, &[30], &[]);
    for _ in 0..300 {
        trips.step_with_rail(&net, &mut router, Tick::ZERO, &open);
        let t = trips.get(id).unwrap();
        if t.mode == LaneMode::Car {
            assert!(!car_on_tile(t, 30), "{t:?}");
            return;
        }
    }
    panic!("車に乗らない");
}
