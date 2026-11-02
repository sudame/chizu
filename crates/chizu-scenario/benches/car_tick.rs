//! 車線を走る車を 1 tick 進める時間。人口 10 万の規模の格子で、同時に 1 万台が走る。
//! 出発と行き先は `walk_ends` と同じ位置を、同じ区間の同じ向きの車のリンクに置き直したもの。
//! 20 tick 走らせて、止まった車と走っている車が混ざった状態から測る。

use std::hint::black_box;

use chizu_core::Tick;
use chizu_core::Trips;
use chizu_core::routing::AstarRouter;
use chizu_scenario::{city_grid_100k, walk_ends};
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};

fn car_tick(c: &mut Criterion) {
    let net = city_grid_100k();
    let mut router = AstarRouter::new(&net);
    let mut trips = Trips::default();
    let ids = trips.spawn_drives(&net, &mut router, &walk_ends(1, &net, 10_000), Tick::ZERO);
    assert!(ids.iter().all(Option::is_some));
    for _ in 0..20 {
        trips.step(&net, &mut router, Tick::ZERO);
    }
    c.bench_function("car_tick_10k", |b| {
        b.iter_batched(
            || trips.clone(),
            |mut t| black_box(t.step(&net, &mut router, Tick::ZERO)),
            BatchSize::LargeInput,
        );
    });
}

criterion_group!(benches, car_tick);
criterion_main!(benches);
