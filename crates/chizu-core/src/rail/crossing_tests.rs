//! 踏切のテスト。置き方の決まり、取り壊し、列車が閉じる長さを確かめる。

use super::*;
use crate::chunk::TileUse;
use crate::command::{Command, RoadKind};
use crate::record::{CommandLog, replay};
use crate::tile::local_index;
use crate::units::{Money, Tick};
use crate::world::{ApplyOutcome, World};

fn at(x: i64, y: i64) -> TilePos {
    TilePos::new(x, y)
}

fn road(w: &mut World, from: (i64, i64), to: (i64, i64)) -> ApplyOutcome {
    w.apply(&Command::BuildRoad {
        from: at(from.0, from.1),
        to: at(to.0, to.1),
        kind: RoadKind::Small,
    })
}

fn rail(w: &mut World, from: (i64, i64), to: (i64, i64)) -> ApplyOutcome {
    w.apply(&Command::BuildRail {
        from: at(from.0, from.1),
        to: at(to.0, to.1),
    })
}

fn tile_use(w: &World, x: i64, y: i64) -> TileUse {
    let t = at(x, y);
    let chunk = w.chunk(t.chunk().unwrap()).unwrap();
    let (lx, ly) = t.local();
    chunk.layer(crate::chunk::Layer::Ground).unwrap().occupancy[local_index(lx, ly)]
}

#[track_caller]
fn deny(_: &mut World, c: ApplyOutcome, r: DenyReason) {
    assert_eq!(c, ApplyOutcome::Denied(r));
}

#[track_caller]
fn ok(outcome: ApplyOutcome) {
    assert_eq!(outcome, ApplyOutcome::Applied);
}

fn station(w: &mut World, east: (i64, i64)) -> StationId {
    // 駅の線路は道路の側から 2 列目で、rotation 2 の at は東の端の 1 つ南である。
    ok(w.apply(&Command::PlaceBuilding {
        kind: ServiceKind::Station,
        at: at(east.0, east.1 + 1),
        rotation: 2,
    }));
    w.rail().station_at(at(east.0, east.1)).expect("駅がない")
}

/// y = 10 の道路の西（x 0〜12）と東（x 50〜66）の切れ端に面した駅 2 つ（駅の線路は y = 8 の x 3〜12 と
/// x 55〜64）を、線路（x 12〜55）で結んだ街。線路の途中は道路から離れているので、縦の道路で横切れる。
fn crossing_world() -> (World, StationId, StationId) {
    crossing_world_with(World::new_with_money(0, Money(1_000_000_000)).with_unlimited_land())
}

fn crossing_world_with(mut w: World) -> (World, StationId, StationId) {
    ok(road(&mut w, (0, 10), (12, 10)));
    ok(road(&mut w, (50, 10), (66, 10)));
    let a = station(&mut w, (12, 8));
    let b = station(&mut w, (64, 8));
    ok(rail(&mut w, (12, 8), (55, 8)));
    (w, a, b)
}

#[test]
fn a_road_across_a_rail_makes_a_crossing_with_nodes_on_both_sides() {
    let (mut w, _, _) = crossing_world();
    let tracks = w.rail().tracks().count();
    ok(road(&mut w, (30, 0), (30, 20)));
    assert_eq!(tile_use(&w, 30, 8), TileUse::Crossing);
    assert!(w.rail().is_crossing(at(30, 8)));
    assert_eq!(w.rail().crossings().collect::<Vec<_>>(), vec![at(30, 8)]);
    // 踏切の両隣にノードができ、線路が 2 本増える。踏切のマスは道路の区間の途中のままである。
    assert!(w.rail().node_at(at(29, 8)).is_some());
    assert!(w.rail().node_at(at(31, 8)).is_some());
    assert!(w.rail().node_at(at(30, 8)).is_none());
    assert_eq!(w.rail().tracks().count(), tracks + 2);
    assert!(w.network().node_at(at(30, 8)).is_none());
}

#[test]
fn a_rail_across_a_road_makes_the_same_crossing() {
    let mut w = World::new_with_money(0, Money(1_000_000_000)).with_unlimited_land();
    ok(road(&mut w, (30, 0), (30, 20)));
    ok(rail(&mut w, (12, 8), (55, 8)));
    assert_eq!(tile_use(&w, 30, 8), TileUse::Crossing);
    assert!(w.rail().node_at(at(29, 8)).is_some());
    assert!(w.rail().node_at(at(31, 8)).is_some());
    // 斜めの線路も、縦の道路を横切れる。
    ok(rail(&mut w, (26, 20), (34, 12)));
    assert_eq!(tile_use(&w, 30, 16), TileUse::Crossing);
    assert!(w.rail().node_at(at(29, 17)).is_some());
    assert!(w.rail().node_at(at(31, 15)).is_some());
}

#[test]
fn crossings_are_denied_where_the_rules_do_not_hold() {
    let (mut w, _, _) = crossing_world();
    let before = w.clone();
    // 線路のマスで終わる道路。
    let c = road(&mut w, (30, 0), (30, 8));
    deny(&mut w, c, DenyReason::NodeOnCrossing);
    // 線路に沿う道路。
    let c = road(&mut w, (20, 8), (25, 8));
    deny(&mut w, c, DenyReason::Occupied);
    // 駅の線路を横切る道路。
    let c = road(&mut w, (8, 0), (8, 9));
    deny(&mut w, c, DenyReason::Occupied);
    assert_eq!(w, before);
    ok(road(&mut w, (30, 0), (30, 20)));
    let before = w.clone();
    // 隣の踏切。
    let c = road(&mut w, (31, 0), (31, 20));
    deny(&mut w, c, DenyReason::CrossingTooClose);
    // 踏切から 2 マスの所に交差点を作る道路。3 マス離せばよい。
    let c = road(&mut w, (25, 10), (30, 10));
    deny(&mut w, c, DenyReason::CrossingNearJunction);
    let c = road(&mut w, (25, 6), (30, 6));
    deny(&mut w, c, DenyReason::CrossingNearJunction);
    // 踏切のマスを通る道路は、そこに交差点を作る。
    let c = road(&mut w, (26, 4), (34, 12));
    deny(&mut w, c, DenyReason::NodeOnCrossing);
    assert_eq!(w, before);
    ok(road(&mut w, (25, 11), (30, 11)));
    // 線路を引いて、交差点の 2 マス以内に踏切を作る。
    let c = rail(&mut w, (40, 13), (20, 13));
    deny(&mut w, c, DenyReason::CrossingNearJunction);
    // 曲がり角の 2 マス以内も断る。
    ok(road(&mut w, (40, 0), (40, 20)));
    ok(road(&mut w, (40, 20), (45, 20)));
    let c = rail(&mut w, (36, 18), (44, 18));
    deny(&mut w, c, DenyReason::CrossingNearJunction);
    ok(rail(&mut w, (36, 17), (44, 17)));
}

#[test]
fn bulldozing_a_crossing_keeps_the_other_side() {
    let (mut w, _, _) = crossing_world();
    ok(road(&mut w, (30, 0), (30, 20)));
    // 道路だけを取り壊すと、線路のマスに戻る。
    let mut a = w.clone();
    let area = TileRect::from_corners(at(30, 15), at(30, 15));
    let preview = a.bulldoze_preview(area);
    ok(a.apply(&Command::Bulldoze { area }));
    assert_eq!(tile_use(&a, 30, 8), TileUse::Rail);
    assert!(!a.rail().is_crossing(at(30, 8)));
    assert!(preview.removed_road_tiles.contains(&at(30, 8)));
    assert!(preview.tracks.is_empty());
    // 線路だけを取り壊すと、道路のマスに戻る。踏切の東の線路（x 31〜55）が消える。
    let mut b = w.clone();
    let area = TileRect::from_corners(at(40, 8), at(40, 8));
    let preview = b.bulldoze_preview(area);
    b.apply(&Command::Bulldoze { area });
    assert_eq!(tile_use(&b, 30, 8), TileUse::Crossing);
    assert_eq!(tile_use(&b, 40, 8), TileUse::Empty);
    assert_eq!(preview.rail_tiles, 23);
    // 踏切のマスを取り壊すと、道路の区間と、踏切の短い線路の両方が消える。
    let mut c = w.clone();
    let area = TileRect::from_corners(at(30, 8), at(30, 8));
    let preview = c.bulldoze_preview(area);
    c.apply(&Command::Bulldoze { area });
    assert_eq!(tile_use(&c, 30, 8), TileUse::Empty);
    assert_eq!(tile_use(&c, 29, 8), TileUse::Rail);
    assert_eq!(c.rail().crossing_count(), 0);
    assert_eq!(preview.removed_rail_tiles, vec![at(30, 8)]);
    assert_eq!(preview.tracks.len(), 1);
    // また道路を引くと、踏切の短い線路はもうないので、道路のマスになる。
    ok(road(&mut c, (30, 0), (30, 20)));
    assert_eq!(tile_use(&c, 30, 8), TileUse::Road);
}

/// 列車を走らせて、踏切が閉じる長さを確かめる。閉じているのは列車が近づいて抜けるまでのあいだだけで、
/// 車の救済（300 tick）よりずっと短い。
fn closes_briefly(cross_x: i64) {
    let (mut w, a, b) = crossing_world();
    ok(road(&mut w, (cross_x, 0), (cross_x, 20)));
    assert_eq!(w.rail().crossing_count(), 1);
    ok(w.apply(&Command::CreateLine { from: a, to: b }));
    let line = w.rail().lines().next().unwrap().0;
    ok(w.apply(&Command::SetLineTrains { line, trains: 2 }));
    let tile = at(cross_x, 8);
    let mut closed_ticks = 0;
    for _ in 0..6_000 {
        w.step();
        let closed = w.rail().closed_crossings().contains(&tile);
        if closed {
            closed_ticks += 1;
        }
    }
    let tally = w.rail().crossing_tally();
    assert!(tally.closures > 10, "{tally:?}");
    assert!(tally.longest_closed <= 60, "{tally:?}");
    assert!(closed_ticks < 6_000 / 2, "{closed_ticks}");
    assert_eq!(tally.overrun, 0);
    assert_eq!(w.rail().tally().deadlock, 0);
}

#[test]
fn a_crossing_closes_only_while_a_train_passes() {
    closes_briefly(30);
}

/// 駅の端のすぐ先の踏切も、駅に止まっている列車では閉じたままにならない。
#[test]
fn a_crossing_next_to_a_station_does_not_stay_closed() {
    closes_briefly(13);
}

/// 踏切のある記録を再生し、書き出して読み戻しても、同じ街になる。
#[test]
fn crossings_replay_and_round_trip_through_the_snapshot() {
    let (w0, a, b) = crossing_world();
    let mut log = CommandLog::new();
    let _ = (a, b);
    log.record(
        Tick(0),
        Command::BuildRoad {
            from: at(30, 0),
            to: at(30, 20),
            kind: RoadKind::Small,
        },
    )
    .unwrap();
    log.record(Tick(1), Command::CreateLine { from: a, to: b })
        .unwrap();
    let mut w = w0.clone();
    let mut v = w0;
    replay(&mut w, &log, 400);
    replay(&mut v, &log, 400);
    assert_eq!(w.state_hash(), v.state_hash());
    let bytes = w.to_snapshot();
    let back = World::from_snapshot(&bytes).unwrap();
    assert_eq!(back.state_hash(), w.state_hash());
    assert_eq!(back.rail().crossings().collect::<Vec<_>>(), vec![at(30, 8)]);
}

/// 踏切を測る街。西の住宅（y -6〜-1）から東の工場（y 1〜6）へ、y = 0 の通りを車で通う。通りは x = 5 で
/// 南北の線路を横切る。線路は、北（y = -42）と南（y = 42）の駅を、斜めと縦の線路で結ぶ。
fn commuter_town(trains: u8) -> World {
    let mut w = World::new_with_money(7, Money(1_000_000_000)).with_unlimited_land();
    let medium = |from: (i64, i64), to: (i64, i64)| Command::BuildRoad {
        from: at(from.0, from.1),
        to: at(to.0, to.1),
        kind: RoadKind::Medium,
    };
    ok(w.apply(&medium((-60, 0), (60, 0))));
    let inner = crate::world::highway_line(w.seed()).start;
    let corner = at(inner.x, 0);
    if inner != corner {
        ok(w.apply(&Command::BuildRoad {
            from: inner,
            to: corner,
            kind: RoadKind::Medium,
        }));
    }
    if !(-60..=60).contains(&corner.x) {
        let end = at(corner.x.clamp(-60, 60), 0);
        ok(w.apply(&Command::BuildRoad {
            from: corner,
            to: end,
            kind: RoadKind::Medium,
        }));
    }
    // 北の駅（線路は y = -42 の x -9〜0）と、南の駅（線路は y = 42 の x 10〜19）。
    ok(road(&mut w, (-20, -40), (20, -40)));
    ok(road(&mut w, (0, 40), (30, 40)));
    let a = station(&mut w, (0, -42));
    ok(w.apply(&Command::PlaceBuilding {
        kind: ServiceKind::Station,
        at: at(10, 41),
        rotation: 0,
    }));
    let b = w.rail().station_at(at(10, 42)).expect("南の駅がない");
    ok(rail(&mut w, (0, -42), (5, -37)));
    ok(rail(&mut w, (5, -37), (5, 37)));
    ok(rail(&mut w, (5, 37), (10, 42)));
    // 通りの踏切と、駅の前の道路を斜めの線路が横切る踏切 2 つ。
    assert_eq!(w.rail().crossing_count(), 3);
    for (area, zone) in [
        (((-60, -6), (-5, -1)), crate::command::Zone::Residential),
        (((10, 1), (60, 6)), crate::command::Zone::Industrial),
    ] {
        let c = w.apply(&Command::PaintZone {
            area: TileRect::from_corners(at(area.0.0, area.0.1), at(area.1.0, area.1.1)),
            zone: Some(zone),
        });
        assert!(matches!(
            c,
            ApplyOutcome::Applied | ApplyOutcome::AppliedPartly(_)
        ));
    }
    for (kind, x) in [
        (ServiceKind::PowerPlant, -3),
        (ServiceKind::WaterPump, 30),
        (ServiceKind::WaterPump, 33),
    ] {
        ok(w.apply(&Command::PlaceBuilding {
            kind,
            at: at(x, -1),
            rotation: 2,
        }));
    }
    ok(w.apply(&Command::CreateLine { from: a, to: b }));
    let line = w.rail().lines().next().unwrap().0;
    ok(w.apply(&Command::SetLineTrains { line, trains }));
    w
}

/// 踏切のある街を、列車を走らせない版と走らせる版で回し、止まった車、救済、閉じていた長さを出す。
/// `cargo test -p chizu-core measure_crossing -- --ignored --nocapture` で回す。
#[test]
#[ignore]
fn measure_crossing() {
    for trains in [0, 1, 3] {
        let mut w = commuter_town(trains);
        let days = 4;
        for _ in 0..days * crate::TICKS_PER_DAY {
            w.step();
        }
        let r = w.trips().rescues();
        let c = w.rail().crossing_tally();
        eprintln!(
            "trains {trains}: 人口 {} 救済 {} 歩きに切り替え {} 交差点の割り込み {} 踏切 {:?}",
            w.population().citizen_ids().count(),
            r.teleported,
            r.walked_instead,
            r.passed_stuck,
            c
        );
    }
}

/// [`crossing_world`] の西の駅の端（x = 12）のすぐ先の踏切（x = 13）を、北の住宅から南の工場へ通う車が通る街。
/// 列車は西の駅を出るたびに、踏切のすぐ手前から走り出す。2 本なら、西の駅が空くのを待つ列車が、東から来て
/// 踏切のすぐ手前（待てる所の先の端）で止まる。
fn station_crossing_town(trains: u8) -> World {
    // 種 6 の高速道路は、(20, -64) から北へ出る。
    let seed = 6;
    assert_eq!(crate::world::highway_line(seed).start, at(20, -64));
    let (mut w, a, b) = crossing_world_with(
        World::new_with_money(seed, Money(1_000_000_000)).with_unlimited_land(),
    );
    ok(road(&mut w, (20, -64), (20, -60)));
    ok(road(&mut w, (20, -60), (-10, -60)));
    ok(road(&mut w, (13, -60), (13, 60)));
    assert_eq!(w.rail().crossings().collect::<Vec<_>>(), vec![at(13, 8)]);
    for (area, zone) in [
        (((14, -55), (19, 4)), crate::command::Zone::Residential),
        (((7, -55), (12, 4)), crate::command::Zone::Residential),
        (((14, 12), (19, 58)), crate::command::Zone::Industrial),
        (((7, 14), (12, 58)), crate::command::Zone::Industrial),
    ] {
        let c = w.apply(&Command::PaintZone {
            area: TileRect::from_corners(at(area.0.0, area.0.1), at(area.1.0, area.1.1)),
            zone: Some(zone),
        });
        assert!(
            matches!(c, ApplyOutcome::Applied | ApplyOutcome::AppliedPartly(_)),
            "{c:?}"
        );
    }
    for (kind, x) in [
        (ServiceKind::PowerPlant, -5),
        (ServiceKind::WaterPump, 5),
        (ServiceKind::WaterPump, 8),
    ] {
        ok(w.apply(&Command::PlaceBuilding {
            kind,
            at: at(x, -61),
            rotation: 2,
        }));
    }
    ok(w.apply(&Command::CreateLine { from: a, to: b }));
    let line = w.rail().lines().next().unwrap().0;
    ok(w.apply(&Command::SetLineTrains { line, trains }));
    w
}

/// 駅を出る列車も、先の閉塞を待って踏切のすぐ手前で止まっていた列車も、踏切を閉じてから 16 tick たつまで
/// 踏切に入らない。閉じる前に踏切に入った車は抜けきれるので、列車とぶつかりかけることはない。
#[test]
fn trains_starting_next_to_a_crossing_wait_for_the_warning() {
    let tile = at(13, 8);
    for trains in [1, 2] {
        let mut w = station_crossing_town(trains);
        let mut on = false;
        let mut entries = 0;
        for _ in 0..3 * crate::TICKS_PER_DAY {
            w.step();
            let now = w.tick().0;
            let was = on;
            on = w.rail().crossings_under_trains().contains(&tile);
            if on && !was {
                entries += 1;
                let since = w
                    .rail()
                    .closed_since(tile)
                    .expect("列車がかかった踏切が開いている");
                assert!(
                    now - since >= CROSSING_WARN_TICKS.unsigned_abs(),
                    "列車 {trains} 本：tick {since} に閉じた踏切に tick {now} に入った"
                );
            }
        }
        let tally = w.rail().crossing_tally();
        assert!(entries > 10, "列車 {trains} 本：{entries}");
        assert!(tally.wait > 0, "列車 {trains} 本：車が通らない {tally:?}");
        assert_eq!(tally.overrun, 0, "列車 {trains} 本：{tally:?}");
        assert!(tally.longest_closed <= 60, "列車 {trains} 本：{tally:?}");
        assert_eq!(w.trips().rescues().teleported, 0, "列車 {trains} 本");
    }
}
