//! 建物が建つ処理の、1 tick の最悪の時間と 1 日ぶんの時間。
//!
//! 7×7 チャンクに 12 マスごとの格子の道路を引き、4 種類の区画を塗った街を 1 日（7,200 tick）回す。
//! criterion では最悪の 1 tick を測れないので、時計で測って書き出す。

use std::time::{Duration, Instant};

use chizu_core::TICKS_PER_DAY;
use chizu_scenario::zoned_grid_city;

fn main() {
    let mut w = zoned_grid_city(1);
    let segments = w.network().segments().count();
    let mut worst = Duration::ZERO;
    let start = Instant::now();
    for _ in 0..TICKS_PER_DAY {
        let t = Instant::now();
        w.step();
        worst = worst.max(t.elapsed());
    }
    let total = start.elapsed();
    println!(
        "区間 {segments} 本、建物 {} 棟：1 日 {total:?}、最悪の 1 tick {worst:?}",
        w.buildings().len()
    );
}
