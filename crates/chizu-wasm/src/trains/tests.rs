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
