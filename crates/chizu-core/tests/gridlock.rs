//! 人間が遊んで車が何日も止まったままになった街（Issue #81）を再生し、詰まりが残らないことを見る。
//!
//! 記録 `tests/gridlock/city.rec` は、種 0 で人口が 5,000 人ほどまで育った街である。道路は小さい道路と
//! 中くらいの道路の格子に、大きい道路と外部接続の高速道路が 1 本ずつつながる。直す前は、66 日目の朝の
//! 混雑から抜けられず、67 日目の終わりには 1 日より長く動けない車が残った。
//! 混雑で車が待つのはよいが、出発してから 1 日たっても着かない車は残らないことを確かめる。

use std::path::PathBuf;

use chizu_core::{CommandLog, LaneMode, TICKS_PER_DAY, World, replay};

/// 混雑の始まる少し前の tick。65 日目の終わりである。
const BEFORE_JAM: u64 = 66 * TICKS_PER_DAY;

/// 見終える tick。直す前の版では、ここまでに 1 日より長く走り続ける車が出る。
const UNTIL: u64 = 69 * TICKS_PER_DAY;

/// 見る間隔（tick）。
const EVERY: u64 = TICKS_PER_DAY / 8;

#[test]
fn no_car_stays_on_the_road_for_a_day_in_a_jammed_city() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/gridlock/city.rec");
    let log = CommandLog::decode(&std::fs::read(path).unwrap()).unwrap();
    let mut world = World::new(0);
    replay(&mut world, &log, BEFORE_JAM);
    let mut busiest = 0;
    while world.tick().0 < UNTIL {
        replay(&mut world, &log, EVERY);
        let now = world.tick().0;
        let cars: Vec<u64> = world
            .trips()
            .iter()
            .filter(|(_, t)| t.mode() == LaneMode::Car)
            .map(|(_, t)| now - t.departed().0)
            .collect();
        busiest = busiest.max(cars.len());
        let oldest = cars.iter().max().copied().unwrap_or(0);
        assert!(
            oldest < TICKS_PER_DAY,
            "tick {now}: 出発してから {oldest} tick たっても着かない車がいる（車 {} 台）",
            cars.len()
        );
    }
    // 混雑が起きる街であることも確かめておく。
    assert!(busiest >= 400, "混雑が起きなかった {busiest}");
}
