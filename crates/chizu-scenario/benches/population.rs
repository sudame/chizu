//! 住民 10 万人の街で、全員をなめる処理が重なる tick の時間。
//!
//! [`serviced_grid_city`] を人口が 10 万人に届くまで回し、届いた日数を書き出す。そのあと 1 日回し、
//! 移住・職場の割り当て・建つ処理がそろう 300 の倍数の tick と、すべての tick の最悪の時間を測る。

use std::time::{Duration, Instant};

use chizu_core::TICKS_PER_DAY;
use chizu_core::world::people::JOB_INTERVAL;
use chizu_scenario::serviced_grid_city;

const TARGET: usize = 100_000;
const MAX_DAYS: u64 = 15;

fn main() {
    let mut w = serviced_grid_city(1);
    let start = Instant::now();
    while w.population().citizen_count() < TARGET {
        assert!(
            w.tick().0 < MAX_DAYS * TICKS_PER_DAY,
            "{MAX_DAYS} 日で {} 人にしか届かない",
            w.population().citizen_count()
        );
        w.step();
        if w.tick().0.is_multiple_of(TICKS_PER_DAY) {
            println!(
                "{} 日目：住民 {} 人",
                w.tick().0 / TICKS_PER_DAY,
                w.population().citizen_count()
            );
        }
    }
    let reached = w.tick().0;
    println!(
        "{TARGET} 人に届いた：ゲーム内 {}.{:02} 日（tick {reached}）、回すのに {:?}",
        reached / TICKS_PER_DAY,
        reached % TICKS_PER_DAY * 100 / TICKS_PER_DAY,
        start.elapsed()
    );
    let (mut worst, mut worst_heavy) = (Duration::ZERO, Duration::ZERO);
    let mut heavy_total = Duration::ZERO;
    let mut runs = 0_u32;
    for _ in 0..TICKS_PER_DAY {
        let heavy = w.tick().0.is_multiple_of(JOB_INTERVAL);
        let t = Instant::now();
        w.step();
        let e = t.elapsed();
        worst = worst.max(e);
        if heavy {
            worst_heavy = worst_heavy.max(e);
            heavy_total += e;
            runs += 1;
        }
    }
    println!(
        "住民 {} 人、建物 {} 棟：300 の倍数の tick は最悪 {worst_heavy:?}、平均 {:?}、すべての tick の最悪 {worst:?}",
        w.population().citizen_count(),
        w.buildings().len(),
        heavy_total / runs,
    );
}
