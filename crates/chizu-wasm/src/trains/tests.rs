use super::*;
use crate::sim::Sim;
use chizu_core::{Command, CommandLog, Money, RoadKind, ServiceKind, TilePos, World};

fn at(x: i64, y: i64) -> TilePos {
    TilePos::new(x, y)
}

/// y = 10 の道路に面した駅 2 つ（駅の線路は y = 8 の x 3〜12 と x 53〜62）を線路で結び、路線を作った街。
fn shuttle(trains: u8) -> Sim {
    let world = World::new_with_money(0, Money(1_000_000_000)).with_unlimited_land();
    let mut sim = Sim::from_parts(world, CommandLog::new());
    let mut submit = |c: Command| {
        sim.submit(c).unwrap();
    };
    submit(Command::BuildRoad {
        from: at(0, 10),
        to: at(70, 10),
        kind: RoadKind::Small,
    });
    for x in [12, 62] {
        submit(Command::PlaceBuilding {
            kind: ServiceKind::Station,
            at: at(x, 9),
            rotation: 2,
        });
    }
    submit(Command::BuildRail {
        from: at(12, 8),
        to: at(53, 8),
    });
    let ids: Vec<_> = sim.world().rail().stations().map(|(id, _)| id).collect();
    sim.submit(Command::CreateLine {
        from: ids[0],
        to: ids[1],
    })
    .unwrap();
    let line = sim.world().rail().lines().next().unwrap().0;
    sim.submit(Command::SetLineTrains { line, trains }).unwrap();
    sim
}

#[test]
fn no_trains_no_cars() {
    let sim = shuttle(0);
    let mut cars = Vec::new();
    collect_cars(sim.world(), &mut cars);
    assert!(cars.is_empty());
}

#[test]
fn a_train_has_four_cars_spaced_by_the_car_length() {
    let mut sim = shuttle(2);
    sim.advance(400);
    let trains = sim.world().rail().train_count();
    assert!(trains >= 1);
    let mut cars = Vec::new();
    let base = collect_cars(sim.world(), &mut cars);
    assert_eq!(cars.len(), trains * CARS_PER_TRAIN);
    let origin = (
        f64::from(base.cx) * CHUNK_SIDE as f64,
        f64::from(base.cy) * CHUNK_SIDE as f64,
    );
    let world = |m: &Mover| (f64::from(m.x) + origin.0, f64::from(m.y) + origin.1);
    for train in cars.chunks(CARS_PER_TRAIN) {
        // 先頭から順に並び、種類は列車、路線の色は同じ
        for (k, car) in train.iter().enumerate() {
            assert_eq!(car.kind & 3, KIND_TRAIN);
            assert_eq!(car.kind, train[0].kind);
            assert_eq!(car.id % CARS_PER_TRAIN as u32, k as u32);
            assert_eq!(car.generation, train[0].generation);
        }
        // 真っすぐな線路の上では、隣の車両の真ん中は 1 両の長さ（2.5 マス）だけ離れる
        for pair in train.windows(2) {
            let (a, b) = (world(&pair[0]), world(&pair[1]));
            let d = (a.0 - b.0).hypot(a.1 - b.1);
            if (d - 2.5).abs() > 1e-3 {
                // 駅の線路と外の線路のつなぎ目をまたぐ場合も、同じ y の直線なので 2.5 のはず
                panic!("車両の間隔が 2.5 マスでない：{d}");
            }
        }
    }
}

#[test]
fn trains_keep_to_the_left_of_their_direction_on_the_track() {
    let mut sim = shuttle(1);
    // 列車が走り出すまで進め、先頭の車両を見る
    let mut seen_east = false;
    let mut seen_west = false;
    for _ in 0..400 {
        sim.advance(10);
        let mut cars = Vec::new();
        let base = collect_cars(sim.world(), &mut cars);
        let Some(head) = cars.first() else { continue };
        let y = f64::from(head.y) + f64::from(base.cy) * CHUNK_SIDE as f64;
        // 線路の中心線は y = 8 のマスの真ん中（8.5）で、東へ進むなら北（8.3）、西へ進むなら南（8.7）を通る
        let heading = f64::from(head.heading) / 65536.0;
        if (heading - 0.0).abs() < 0.01 {
            assert!((y - 8.3).abs() < 1e-3, "東向きの y が {y}");
            seen_east = true;
        } else if (heading - 0.5).abs() < 0.01 {
            assert!((y - 8.7).abs() < 1e-3, "西向きの y が {y}");
            seen_west = true;
        }
    }
    assert!(seen_east && seen_west, "往復の両方向を見ていない");
}

/// y = 10 の道路の西と東の切れ端に面した駅 2 つを、y = 8 の線路で結び、x = 30 の縦の道路で踏切を作った街。
fn crossing_shuttle() -> Sim {
    let world = World::new_with_money(0, Money(1_000_000_000)).with_unlimited_land();
    let mut sim = Sim::from_parts(world, CommandLog::new());
    let mut submit = |c: Command| {
        sim.submit(c).unwrap();
    };
    for (from, to) in [((0, 10), (12, 10)), ((50, 10), (66, 10))] {
        submit(Command::BuildRoad {
            from: at(from.0, from.1),
            to: at(to.0, to.1),
            kind: RoadKind::Small,
        });
    }
    for x in [12, 64] {
        submit(Command::PlaceBuilding {
            kind: ServiceKind::Station,
            at: at(x, 9),
            rotation: 2,
        });
    }
    submit(Command::BuildRail {
        from: at(12, 8),
        to: at(55, 8),
    });
    submit(Command::BuildRoad {
        from: at(30, 0),
        to: at(30, 20),
        kind: RoadKind::Small,
    });
    let ids: Vec<_> = sim.world().rail().stations().map(|(id, _)| id).collect();
    sim.submit(Command::CreateLine {
        from: ids[0],
        to: ids[1],
    })
    .unwrap();
    let line = sim.world().rail().lines().next().unwrap().0;
    sim.submit(Command::SetLineTrains { line, trains: 1 })
        .unwrap();
    sim
}

#[test]
fn a_crossing_is_sent_with_the_road_and_rail_directions() {
    let sim = crossing_shuttle();
    // 縦の道路（小は 1）と、横の線路
    assert_eq!(
        sim.rail_crossings(),
        vec![30.0, 8.0, 0.0, 1.0, 1.0, 0.0, 1.0]
    );
}

#[test]
fn closed_crossings_follow_the_train() {
    let mut sim = crossing_shuttle();
    assert!(closed_crossings(sim.world()).is_empty());
    let mut closed_ticks = 0;
    let mut open_ticks = 0;
    for _ in 0..3_000 {
        sim.advance(1);
        let closed = closed_crossings(sim.world());
        if closed.is_empty() {
            open_ticks += 1;
        } else {
            assert_eq!(closed, vec![[30, 8]]);
            closed_ticks += 1;
        }
    }
    // 列車が通るあいだだけ閉じる
    assert!(closed_ticks > 0);
    assert!(open_ticks > closed_ticks);
}

#[test]
fn the_kind_carries_the_line_color_and_the_load_level() {
    assert_eq!(load_level(0), 0);
    assert_eq!(load_level(249), 0);
    assert_eq!(load_level(250), 1);
    assert_eq!(load_level(999), 3);
    assert_eq!(load_level(4_000), 3);
    let mut sim = shuttle(1);
    sim.advance(100);
    let mut cars = Vec::new();
    collect_cars(sim.world(), &mut cars);
    let kind = cars[0].kind;
    // 乗っている人はいないので、混み具合は 0。路線の色は kind の 2〜4 ビット目で、画面の 8 色の剰余と同じ
    assert_eq!(kind & 3, KIND_TRAIN);
    assert_eq!((kind >> 2) & 7, 0);
    assert_eq!(kind >> LOAD_SHIFT, 0);
}

#[test]
fn rail_stats_lists_stations_and_lines_in_the_documented_strides() {
    use crate::sim::{RAIL_STATS_HEADER, RAIL_STATS_LINE, RAIL_STATS_STATION};
    let mut sim = shuttle(1);
    sim.advance(50);
    let stats = sim.rail_stats();
    assert_eq!(
        stats.len(),
        RAIL_STATS_HEADER + 2 * RAIL_STATS_STATION + RAIL_STATS_LINE
    );
    assert_eq!(&stats[..2], &[2.0, 1.0]);
    for k in 0..2 {
        let st = &stats[RAIL_STATS_HEADER + k * RAIL_STATS_STATION..][..RAIL_STATS_STATION];
        // 道路に面しているので出入口がある。世帯も待つ人もいない
        assert_eq!(st[6], 1.0);
        assert_eq!(&st[7..10], &[0.0, 0.0, 0.0]);
        // 出入口は y = 10 の道路の上
        assert_eq!(st[5], 10.0);
    }
    let line = &stats[RAIL_STATS_HEADER + 2 * RAIL_STATS_STATION..];
    assert_eq!(line[2], 0.0);
    assert_eq!(line[7], 1.0);
}

#[test]
fn rail_tally_matches_the_cumulative_counts_in_rail_stats() {
    use crate::sim::{RAIL_STATS_HEADER, RAIL_STATS_STATION};
    let mut sim = shuttle(1);
    sim.advance(50);
    let stats = sim.rail_stats();
    let tally = sim.rail_tally();
    assert_eq!(tally.len(), 1 + 2 * 4);
    assert_eq!(tally[0], stats[2]);
    for k in 0..2 {
        let st = &stats[RAIL_STATS_HEADER + k * RAIL_STATS_STATION..][..RAIL_STATS_STATION];
        let row = &tally[1 + k * 4..][..4];
        // 添え字と世代、乗った人と降りた人の積み上げが同じ
        assert_eq!(&row[..2], &st[..2]);
        assert_eq!(&row[2..], &st[10..12]);
    }
}

#[test]
fn station_probe_gives_the_door_for_the_rotations_that_can_be_placed() {
    let world = World::new_with_money(0, Money(1_000_000_000)).with_unlimited_land();
    let mut sim = Sim::from_parts(world, CommandLog::new());
    sim.submit(Command::BuildRoad {
        from: at(0, 10),
        to: at(70, 10),
        kind: RoadKind::Small,
    })
    .unwrap();
    let probe = sim.station_probe(at(30, 9));
    // 道路は y = 10 なので、南へ向く向き 2 だけ置ける
    assert!(probe[0].is_none() && probe[1].is_none() && probe[3].is_none());
    let [x, y, households, carless] = probe[2].expect("置けるはず");
    assert_eq!(y, 10.0);
    assert!((20.0..=40.0).contains(&x));
    assert_eq!((households, carless), (0.0, 0.0));
}
