//! 経路探索の一括の探索にかかる時間。覚えた結果がない状態から解く。
//!
//! - `1k`：探索 1,000 件。
//! - `commute_100k`：人口 10 万の規模の格子で、10 万人ぶんの通勤の経路。

use std::hint::black_box;

use chizu_core::routing::{AstarRouter, Router};
use chizu_scenario::{city_grid_100k, commute_requests};
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};

fn route_batch(c: &mut Criterion) {
    let net = city_grid_100k();
    let router = AstarRouter::new(&net);
    let mut group = c.benchmark_group("route_batch");
    group.sample_size(10);
    for (name, count) in [("1k", 1_000), ("commute_100k", 100_000)] {
        let reqs = commute_requests(1, &net, count);
        group.bench_function(name, |b| {
            b.iter_batched(
                || router.clone(),
                |mut r| black_box(r.route_batch(&reqs)),
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

criterion_group!(benches, route_batch);
criterion_main!(benches);
