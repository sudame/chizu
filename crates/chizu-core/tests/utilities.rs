//! 電気と水道の釣り合いを、街を育てて確かめる。

use chizu_core::clock::TICKS_PER_DAY;
use chizu_core::command::{Command, RoadKind, ServiceKind, ZoneKind};
use chizu_core::service::building_demand;
use chizu_core::tile::{TilePos, TileRect};
use chizu_core::units::Money;
use chizu_core::world::{ApplyOutcome, World, highway_line};

fn apply(w: &mut World, c: Command) {
    assert_eq!(w.apply(&c), ApplyOutcome::Applied, "{c:?}");
}

fn road(w: &mut World, a: (i64, i64), b: (i64, i64)) {
    let c = Command::BuildRoad {
        from: TilePos::new(a.0, a.1),
        to: TilePos::new(b.0, b.1),
        kind: RoadKind::Small,
    };
    apply(w, c);
}

/// 横の道路 3 本を縦の道路でつなぎ、高速道路まで引いた街。北の道路沿いに施設を、
/// 南の 2 本の道路沿いに住宅、工業、住宅、商業の帯を塗る。施設はまだ置かない。
fn town() -> World {
    let mut w = World::new_with_money(3, Money(100_000_000));
    for y in [-40, 10, 40] {
        road(&mut w, (-60, y), (20, y));
    }
    road(&mut w, (-60, -40), (-60, 10));
    road(&mut w, (-60, 10), (-60, 40));
    let inner = highway_line(w.seed()).start;
    let corner = TilePos::new(inner.x, 10);
    if corner != inner {
        road(&mut w, (inner.x, inner.y), (corner.x, corner.y));
    }
    if !(-60..=20).contains(&corner.x) {
        road(&mut w, (corner.x, 10), (corner.x.clamp(-60, 20), 10));
    }
    let zones = [
        ZoneKind::Residential,
        ZoneKind::Industrial,
        ZoneKind::Residential,
        ZoneKind::Commercial,
    ];
    let bands = [(4, 9), (11, 16), (34, 39), (41, 46)];
    for (zone, (y0, y1)) in zones.into_iter().zip(bands) {
        let area = TileRect::from_corners(TilePos::new(-59, y0), TilePos::new(19, y1));
        apply(
            &mut w,
            Command::PaintZone {
                area,
                zone: Some(zone),
            },
        );
    }
    w
}

fn count(w: &World, kind: ServiceKind) -> i64 {
    w.facilities().iter().filter(|f| f.kind == kind).count() as i64
}

/// 北の道路沿いに、`kind` の施設をもう 1 つ置く。
fn add_facility(w: &mut World, kind: ServiceKind) {
    let i = count(w, kind);
    let (at, rotation) = match kind {
        ServiceKind::PowerPlant => (TilePos::new(-56 + 4 * i, -41), 2),
        ServiceKind::WaterPump => (TilePos::new(-55 + 2 * i, -39), 0),
        ServiceKind::ParkingLot | ServiceKind::ParkingGarage => unreachable!("駐車場は置かない"),
    };
    apply(w, Command::PlaceBuilding { kind, at, rotation });
}

fn demand(w: &World) -> u64 {
    w.buildings().iter().map(|(_, b)| building_demand(b)).sum()
}

/// 供給量の合計が、需要に新しく建つ分の 1/4 を足した量に届くまで、施設を足す。
fn add_facilities_for_demand(w: &mut World) {
    for kind in ServiceKind::UTILITIES {
        while (count(w, kind) as u64) * kind.capacity() * 4 < demand(w) * 5 {
            add_facility(w, kind);
        }
    }
}

fn run_day(w: &mut World) {
    for _ in 0..TICKS_PER_DAY {
        w.step();
    }
}

/// 1 日進める。ゲーム内の 1 時間ごとに、需要に合わせて施設を足す。
fn run_day_adding_facilities(w: &mut World) {
    for _ in 0..24 {
        for _ in 0..TICKS_PER_DAY / 24 {
            w.step();
        }
        add_facilities_for_demand(w);
    }
}

fn served(w: &World) -> usize {
    w.buildings()
        .iter()
        .filter(|(_, b)| b.supply.power && b.supply.water)
        .count()
}

/// 最初の発電所と浄水場 1 つずつで、段階 1 の家を中心にした数十棟の街をまかなえる。
#[test]
fn one_plant_and_one_pump_serve_a_small_town() {
    let mut w = town();
    add_facility(&mut w, ServiceKind::PowerPlant);
    add_facility(&mut w, ServiceKind::WaterPump);
    run_day(&mut w);
    assert!(served(&w) >= 40, "{} 棟にしか届かない", served(&w));
    // 段階が上がって需要が伸びると、1 基ずつでは足りなくなる。足りない分は、
    // 施設を置かない限り届かないまま残る。
    for _ in 0..5 {
        run_day(&mut w);
    }
    assert!(served(&w) < w.buildings().len());
}

/// 最初の発電所と浄水場の維持費は、街が育った日の収入の 2 割に収まる。
#[test]
fn upkeep_of_one_plant_and_one_pump_stays_within_a_fifth_of_income() {
    let mut w = town();
    add_facility(&mut w, ServiceKind::PowerPlant);
    add_facility(&mut w, ServiceKind::WaterPump);
    for _ in 0..4 {
        run_day(&mut w);
    }
    let f = w.finances();
    let upkeep: i64 = f.facility_upkeep.iter().map(|m| m.0).sum();
    assert!(upkeep * 5 <= f.income().0, "{f:?}");
}

/// 需要に合わせて施設を足していく街は、税率が標準のままでも黒字を保つ。
#[test]
fn a_town_with_enough_facilities_stays_in_the_black() {
    let mut w = town();
    add_facility(&mut w, ServiceKind::PowerPlant);
    add_facility(&mut w, ServiceKind::WaterPump);
    for day in 1..=6 {
        run_day_adding_facilities(&mut w);
        let f = w.finances();
        // 最初の日は家が建ったばかりで、働く住民がまだ少ない。
        if day >= 2 {
            assert!(f.net() > Money::ZERO, "{day} 日め：{f:?}");
        }
    }
    let n = w.buildings().len();
    assert!((100..=400).contains(&n), "{n} 棟");
    assert!(served(&w) * 10 >= n * 9, "{} / {n} 棟", served(&w));
}

/// 施設を置かない街には数軒しか建たず、施設のある街よりずっと小さいままである。
#[test]
fn a_town_without_facilities_barely_grows() {
    let mut bare = town();
    let mut served_town = town();
    add_facility(&mut served_town, ServiceKind::PowerPlant);
    add_facility(&mut served_town, ServiceKind::WaterPump);
    for _ in 0..3 {
        run_day(&mut bare);
        run_day_adding_facilities(&mut served_town);
    }
    assert!(
        (1..=6).contains(&bare.buildings().len()),
        "{} 棟",
        bare.buildings().len()
    );
    assert!(served_town.buildings().len() >= 100);
    let people = |w: &World| w.city_stats().citizens;
    assert!(people(&served_town) > 10 * people(&bare));
}

/// 余りのない網では段階が上がらず、届く建物の数は減らない。施設を足すと、また上がりはじめる。
#[test]
fn levels_rise_only_while_the_network_has_room() {
    let mut w = town();
    add_facility(&mut w, ServiceKind::PowerPlant);
    add_facility(&mut w, ServiceKind::WaterPump);
    let mut settled = 0;
    for day in 1..=10 {
        run_day(&mut w);
        // 需要が供給量（と新築の許容 48）を超えて伸びない。
        let cap = ServiceKind::WaterPump.capacity();
        assert!(demand(&w) <= cap + 48, "{day} 日め：{}", demand(&w));
        // 届いていた建物がまとめて届かなくなることはない。
        let n = w.buildings().len();
        assert!(
            served(&w) * 10 >= n * 8,
            "{day} 日め：{} / {n} 棟",
            served(&w)
        );
        settled = demand(&w);
    }
    // 施設を足すと、段階が上がって需要が伸びる。
    for _ in 0..3 {
        add_facility(&mut w, ServiceKind::PowerPlant);
        add_facility(&mut w, ServiceKind::WaterPump);
    }
    for _ in 0..4 {
        run_day(&mut w);
    }
    assert!(
        demand(&w) > settled + 500,
        "{} から {}",
        settled,
        demand(&w)
    );
}
