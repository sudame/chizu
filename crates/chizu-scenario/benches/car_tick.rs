//! 車線を走る車を 1 tick 進める時間。人口 10 万の規模の格子で、同時に 1 万台が走る。
//! 出発と行き先は `walk_ends` と同じ位置を、同じ区間の同じ向きの車のリンクに置き直したもの。
//! 20 tick 走らせて、止まった車と走っている車が混ざった状態から測る。
//!
//! `car_upgrade_10k` は、同じ状態で車のいちばん多い区間の種類を変え、経路探索のグラフを作り直して、
//! 通れなくなった車の経路を探し直す時間である。道路の種類を変えるコマンドの重さの目安にする。

use std::collections::BTreeMap;
use std::hint::black_box;

use chizu_core::Tick;
use chizu_core::Trips;
use chizu_core::routing::AstarRouter;
use chizu_core::{KindChange, LaneMode, Money, RoadKind, SegmentKind};
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

    // 車のいちばん多い道路の区間を、いまと違う種類に変える。
    let mut cars = BTreeMap::new();
    for (_, t) in trips.iter().filter(|(_, t)| t.mode() == LaneMode::Car) {
        *cars.entry(t.link().segment).or_insert(0_u32) += 1;
    }
    let (&busy, _) = cars
        .iter()
        .filter(|(s, _)| matches!(net.segment(**s).unwrap().kind, SegmentKind::Road(_)))
        .max_by_key(|(s, n)| (**n, std::cmp::Reverse(**s)))
        .unwrap();
    let kind = match net.segment(busy).unwrap().kind {
        SegmentKind::Road(RoadKind::Large) => RoadKind::Medium,
        _ => RoadKind::Large,
    };
    let change = KindChange {
        kind,
        segments: vec![busy],
        cost: Money::ZERO,
    };
    c.bench_function("car_upgrade_10k", |b| {
        b.iter_batched(
            || (net.clone(), router.clone(), trips.clone()),
            |(mut n, mut r, mut t)| {
                n.change_road_kind(&change);
                black_box(t.reroute(&n, &mut r))
            },
            BatchSize::LargeInput,
        );
    });
}

criterion_group!(benches, car_tick);
criterion_main!(benches);
