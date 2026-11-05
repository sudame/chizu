//! 車を持てない世帯のいる街を作るテスト用の部品。
//!
//! 今の建物の駐車場の台数では、車を持てない世帯が自然には出ない。中核のテスト用の口
//! （`chizu_core::car_place::testing`）で、建物の自宅の枠を 0 にして作る。

use chizu_core::car_place::testing::set_home_spaces_limit;
use chizu_core::world::growth::GROWTH_INTERVAL;
use chizu_core::world::highway_line;
use chizu_core::{
    ApplyOutcome, Command, Money, RoadKind, ServiceKind, TICKS_PER_DAY, TilePos, TileRect, World,
    Zone,
};

/// 建物の自宅の枠を 0 にしているあいだ持つ。落とすと元に戻す。
pub struct NoHomePlaces;

impl Drop for NoHomePlaces {
    fn drop(&mut self) {
        set_home_spaces_limit(None);
    }
}

fn apply(w: &mut World, command: Command) {
    assert_eq!(w.apply(&command), ApplyOutcome::Applied, "{command:?}");
}

fn road(w: &mut World, from: (i64, i64), to: (i64, i64)) {
    apply(
        w,
        Command::BuildRoad {
            from: TilePos::new(from.0, from.1),
            to: TilePos::new(to.0, to.1),
            kind: RoadKind::Small,
        },
    );
}

pub fn run(w: &mut World, ticks: u64) {
    for _ in 0..ticks {
        w.step();
    }
}

/// 道路 (10, 10)〜(30, 10) の南側に住宅を塗り、北側に発電所と浄水場を置いた街を、`days` 日進めたもの。
/// 自宅の枠は 0 で、家のある世帯はすべて車を持てない。共同の駐車場は置いていない。
pub fn carless_world(days: u64) -> (World, NoHomePlaces) {
    set_home_spaces_limit(Some(0));
    let guard = NoHomePlaces;
    let mut w = World::new_with_money(0, Money(100_000_000));
    road(&mut w, (10, 10), (30, 10));
    let inner = highway_line(0).start;
    let corner = (inner.x, 10);
    for (from, to) in [
        ((inner.x, inner.y), corner),
        (corner, (inner.x.clamp(10, 30), 10)),
    ] {
        if from != to {
            road(&mut w, from, to);
        }
    }
    for (kind, x) in [(ServiceKind::PowerPlant, 16), (ServiceKind::WaterPump, 20)] {
        place(&mut w, kind, x);
    }
    apply(
        &mut w,
        Command::PaintZone {
            area: TileRect::from_corners(TilePos::new(10, 11), TilePos::new(30, 15)),
            zone: Some(Zone::Residential),
        },
    );
    run(&mut w, GROWTH_INTERVAL + 1 + days * TICKS_PER_DAY);
    (w, guard)
}

/// 道路の北側に施設 `kind` を、間口のマス (x, 9) に置く。
pub fn place(w: &mut World, kind: ServiceKind, x: i64) {
    apply(
        w,
        Command::PlaceBuilding {
            kind,
            at: TilePos::new(x, 9),
            rotation: 2,
        },
    );
}
