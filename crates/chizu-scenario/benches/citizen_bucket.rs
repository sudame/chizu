//! 住民データ層の 1 tick ぶんの組の更新にかかる時間。
//!
//! - `standard`：`World::step` と同じ標準の処理（`update_citizen`）。いまは中身が空なので、組をなめる仕組みだけの時間になる。
//! - `rng_write`：住民ごとに乱数を引き、幸福度と出発の予定を書き戻す。中身が入ったときの目安にする。

use std::hint::black_box;

use chizu_core::schedule::update_citizen;
use chizu_core::{BUCKET_COUNT, Tick, bucket_at};
use chizu_scenario::perf::{SEED, rng_write};
use chizu_scenario::synthetic_population;
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

fn citizen_bucket(c: &mut Criterion) {
    let mut group = c.benchmark_group("citizen_bucket");
    for n in [100_000_u32, 1_000_000] {
        let label = format!("{}k", n / 1_000);
        let mut p = synthetic_population(SEED, n);
        let mut tick = Tick::ZERO;
        group.bench_function(BenchmarkId::new("standard", &label), |b| {
            b.iter(|| {
                p.update_bucket(bucket_at(tick), |id, row| {
                    update_citizen(SEED, tick, id, row)
                });
                tick = tick.next();
            });
        });

        // 一巡させて、全員が出発の待ち行列に入った状態から測る。
        let mut p = synthetic_population(SEED, n);
        let mut tick = Tick::ZERO;
        for _ in 0..BUCKET_COUNT {
            p.update_bucket(bucket_at(tick), rng_write(tick));
            tick = tick.next();
        }
        group.bench_function(BenchmarkId::new("rng_write", &label), |b| {
            b.iter(|| {
                p.update_bucket(bucket_at(tick), rng_write(tick));
                tick = tick.next();
            });
        });
        black_box(&p);
    }
    group.finish();
}

criterion_group!(benches, citizen_bucket);
criterion_main!(benches);
