//! 車を持てない世帯のいる街を作るテスト用の部品。
//!
//! 今の建物の駐車場の台数では、車を持てない世帯が自然には出ない。中核のテスト用の口
//! （`chizu_core::car_place::testing`）で、建物の自宅の枠を 0 にして作る。

use chizu_core::car_place::testing::set_home_spaces_limit;
use chizu_core::world::growth::GROWTH_INTERVAL;
use chizu_core::{Command, RoadKind, ServiceKind, TICKS_PER_DAY, TilePos, TileRect, Zone};

use crate::sim::Sim;

/// 建物の自宅の枠を 0 にしているあいだ持つ。落とすと元に戻す。
pub struct NoHomePlaces;

impl NoHomePlaces {
    pub fn new() -> Self {
        set_home_spaces_limit(Some(0));
        NoHomePlaces
    }
}

impl Drop for NoHomePlaces {
    fn drop(&mut self) {
        set_home_spaces_limit(None);
    }
}

/// 道路の南側に住宅を塗り、北側に発電所と浄水場を置いた街を、`days` 日進めたもの。自宅の枠は 0 で、
/// 家のある世帯はすべて車を持てない。共同の駐車場は置いていない。
pub fn carless_town(days: u64) -> (Sim, NoHomePlaces) {
    let guard = NoHomePlaces::new();
    let mut sim = Sim::new(0);
    let road = |from: (i64, i64), to: (i64, i64)| Command::BuildRoad {
        from: TilePos::new(from.0, from.1),
        to: TilePos::new(to.0, to.1),
        kind: RoadKind::Small,
    };
    sim.submit(road((10, 10), (30, 10))).unwrap();
    let inner = chizu_core::world::highway_line(0).start;
    let corner = (inner.x, 10);
    for (from, to) in [
        ((inner.x, inner.y), corner),
        (corner, (inner.x.clamp(10, 30), 10)),
    ] {
        if from != to {
            sim.submit(road(from, to)).unwrap();
        }
    }
    for (kind, x) in [(ServiceKind::PowerPlant, 16), (ServiceKind::WaterPump, 20)] {
        sim.submit(Command::PlaceBuilding {
            kind,
            at: TilePos::new(x, 9),
            rotation: 2,
        })
        .unwrap();
    }
    sim.submit(Command::PaintZone {
        area: TileRect::from_corners(TilePos::new(10, 11), TilePos::new(30, 15)),
        zone: Some(Zone::Residential),
    })
    .unwrap();
    sim.advance(GROWTH_INTERVAL as u32 + 1);
    sim.advance((TICKS_PER_DAY * days) as u32);
    (sim, guard)
}

/// 道路の北側に共同の駐車場 `kind` を、間口のマス (x, 9) に置く。
pub fn place_lot(sim: &mut Sim, kind: ServiceKind, x: i64) {
    sim.submit(Command::PlaceBuilding {
        kind,
        at: TilePos::new(x, 9),
        rotation: 2,
    })
    .unwrap();
}
