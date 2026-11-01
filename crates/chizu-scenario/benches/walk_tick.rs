//! 歩道を歩く移動体を 1 tick 進める時間。人口 10 万の規模の格子で、同時に 1 万人が歩く。

use std::hint::black_box;

use chizu_core::Tick;
use chizu_core::Trips;
use chizu_core::routing::AstarRouter;
use chizu_scenario::{city_grid_100k, walk_ends};
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};

fn walk_tick(c: &mut Criterion) {
    let net = city_grid_100k();
    let mut router = AstarRouter::new(&net);
    let mut trips = Trips::default();
    let ids = trips.spawn_many(&net, &mut router, &walk_ends(1, &net, 10_000), Tick::ZERO);
    assert!(ids.iter().all(Option::is_some));
    c.bench_function("walk_tick_10k", |b| {
        b.iter_batched(
            || trips.clone(),
            |mut t| black_box(t.step(&net)),
            BatchSize::LargeInput,
        );
    });
}

criterion_group!(benches, walk_tick);
criterion_main!(benches);
