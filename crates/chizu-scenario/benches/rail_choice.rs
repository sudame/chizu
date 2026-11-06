//! 手段の選び方の、鉄道の候補の見積もりと、駅の組の表の作り直しの時間。設計 3.10 の 7 の `rail_choice_100k` と
//! `rail_table_rebuild` を手元で測る。

use std::hint::black_box;

use chizu_core::rail::choice::{RailPlanner, RideChoice};
use chizu_scenario::{city_grid_100k, commute_tiles, rail_choice_inputs};
use criterion::{Criterion, criterion_group, criterion_main};

fn rail_choice(c: &mut Criterion) {
    let net = city_grid_100k();
    let inputs = rail_choice_inputs(1);
    let planner = RailPlanner::from_parts(inputs.doors.clone(), &inputs.lines);
    let choice = RideChoice::from_parts(&planner, inputs.live.clone(), Default::default(), 5);
    let ends = commute_tiles(1, &net, 1_000);
    c.bench_function("rail_choice_1k", |b| {
        b.iter(|| {
            for (a, z) in &ends {
                black_box(choice.offer(*a, *z));
            }
        });
    });
    c.bench_function("rail_table_rebuild", |b| {
        b.iter(|| black_box(RailPlanner::from_parts(inputs.doors.clone(), &inputs.lines)));
    });
}

criterion_group!(benches, rail_choice);
criterion_main!(benches);
