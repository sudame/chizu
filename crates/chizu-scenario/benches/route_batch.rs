//! 経路探索の一括の探索にかかる時間。覚えた結果がない状態から解く。
//!
//! 人口 10 万の規模の格子で、職場が車のリンクの約 1/10、1/100、1/1000 に集まっている通勤を解く。
//!
//! - `astar`：束ねずに、すべて A* で解く。
//! - `bundled`：同じ着くリンク（または出発のリンク）の依頼を束ね、1 対多の探索で解く。
//! - `1k` と `100k`：探索の件数。
//! - `mixed_100k`：職場が 1/10 の通勤の半分を歩いて探す。車と歩行者の依頼が混ざった一括である。

use std::hint::black_box;

use chizu_core::LaneMode;
use chizu_core::routing::{AstarRouter, RouteRequest, Router};
use chizu_scenario::{city_grid_100k, commute_requests};
use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};

fn route_batch(c: &mut Criterion) {
    let net = city_grid_100k();
    let bundled = AstarRouter::new(&net);
    let astar = bundled.clone().with_bundle_min(usize::MAX);
    let mut cases: Vec<(String, Vec<RouteRequest>)> = Vec::new();
    for count in [1_000, 100_000] {
        for office_every in [10, 100, 1_000] {
            let label = format!("{}k/offices_1_{office_every}", count / 1_000);
            cases.push((label, commute_requests(1, &net, count, office_every)));
        }
    }
    let mut mixed = commute_requests(1, &net, 100_000, 10);
    for r in mixed.iter_mut().skip(1).step_by(2) {
        r.from.mode = LaneMode::Sidewalk;
        r.to.mode = LaneMode::Sidewalk;
        r.mode = LaneMode::Sidewalk;
    }
    cases.push(("mixed_100k".to_string(), mixed));

    let mut group = c.benchmark_group("route_batch");
    group.sample_size(10);
    for (label, reqs) in &cases {
        for (name, router) in [("astar", &astar), ("bundled", &bundled)] {
            group.bench_function(BenchmarkId::new(name, label), |b| {
                b.iter_batched(
                    || router.clone(),
                    |mut r| black_box(r.route_batch(reqs)),
                    BatchSize::LargeInput,
                );
            });
        }
    }
    group.finish();
}

criterion_group!(benches, route_batch);
criterion_main!(benches);
