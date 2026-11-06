//! 列車 512 本（64 本の往復の路線に 8 本ずつ）を 1 tick 動かす時間。列車の動き、閉塞の出入り、
//! 駅の止まりと折り返し、行き詰まりの輪の検査を含む。設計 3.10 の 7 の `rail_trains` である。

use std::hint::black_box;

use chizu_core::Tick;
use chizu_scenario::{RAIL_BENCH_LINES, RAIL_BENCH_TRAINS, rail_trains_running};
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};

fn rail_trains(c: &mut Criterion) {
    let rail = rail_trains_running(1, RAIL_BENCH_LINES);
    assert_eq!(
        rail.train_count(),
        RAIL_BENCH_LINES * usize::from(RAIL_BENCH_TRAINS)
    );
    c.bench_function("rail_trains_512", |b| {
        b.iter_batched(
            || rail.clone(),
            |mut r| {
                r.step(Tick(1_000_000));
                black_box(r)
            },
            BatchSize::LargeInput,
        );
    });
}

criterion_group!(benches, rail_trains);
criterion_main!(benches);
