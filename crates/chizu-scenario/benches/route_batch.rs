//! 経路探索の一括の探索にかかる時間。覚えた結果がない状態から解く。
//!
//! - `1k`：探索 1,000 件。
//! - `commute_100k`：人口 10 万の規模の格子で、10 万人ぶんの通勤の経路。
//! - `mixed_100k`：同じ通勤の半分を歩いて探す。車と歩行者の依頼が混ざった一括である。

use std::hint::black_box;

use chizu_core::LaneMode;
use chizu_core::routing::{AstarRouter, Router};
use chizu_scenario::{city_grid_100k, commute_requests};
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};

fn route_batch(c: &mut Criterion) {
    let net = city_grid_100k();
    let router = AstarRouter::new(&net);
    let mut mixed = commute_requests(1, &net, 100_000);
    for r in mixed.iter_mut().skip(1).step_by(2) {
        r.from.mode = LaneMode::Sidewalk;
        r.to.mode = LaneMode::Sidewalk;
        r.mode = LaneMode::Sidewalk;
    }
    let cases = [
        ("1k", commute_requests(1, &net, 1_000)),
        ("commute_100k", commute_requests(1, &net, 100_000)),
        ("mixed_100k", mixed),
    ];
    let mut group = c.benchmark_group("route_batch");
    group.sample_size(10);
    for (name, reqs) in &cases {
        group.bench_function(*name, |b| {
            b.iter_batched(
                || router.clone(),
                |mut r| black_box(r.route_batch(reqs)),
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

criterion_group!(benches, route_batch);
criterion_main!(benches);
