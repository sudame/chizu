//! 建物が建つ処理の、1 tick の最悪の時間と 1 日ぶんの時間。
//!
//! 7×7 チャンクに 12 マスごとの格子の道路を引き、4 種類の区画を塗った街を 1 日（7,200 tick）回す。
//! criterion では最悪の 1 tick を測れないので、時計で測って書き出す。速く見えるのが建つ処理が
//! 止まったせいでないことを見るために、建った棟数と、住める世帯の数と職場の数も書き出す。

use std::time::{Duration, Instant};

use chizu_core::world::growth::GROWTH_INTERVAL;
use chizu_core::{TICKS_PER_DAY, state_hash_of};
use chizu_scenario::zoned_grid_city;

fn main() {
    let mut w = zoned_grid_city(1);
    let segments = w.network().segments().count();
    let mut worst = Duration::ZERO;
    let mut built = 0;
    let mut runs = 0;
    let start = Instant::now();
    for _ in 0..TICKS_PER_DAY {
        let before = w.buildings().len();
        if w.tick().0.is_multiple_of(GROWTH_INTERVAL) {
            runs += 1;
        }
        let t = Instant::now();
        w.step();
        worst = worst.max(t.elapsed());
        built += w.buildings().len().saturating_sub(before);
    }
    let total = start.elapsed();
    let cap = w.buildings().capacity();
    println!(
        "区間 {segments} 本：1 日 {total:?}、最悪の 1 tick {worst:?}、建つ処理 {runs} 回、建った {built} 棟、\
         残った {} 棟、世帯の上限 {}、職場の上限 {:?}、建物の表のハッシュ {:016x}",
        w.buildings().len(),
        cap.households,
        cap.workplaces,
        state_hash_of(w.buildings()),
    );
}
