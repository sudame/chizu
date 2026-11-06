//! 手段の選び方の、鉄道の候補の見積もりと、駅の組の表と駅まで歩く表の作り直しの時間。設計 3.10 の 7 の
//! `rail_choice_100k` と `rail_table_rebuild` を手元で測る。

use std::hint::black_box;

use chizu_core::network::LaneMode;
use chizu_core::rail::choice::{RailPlanner, RideChoice, WalkEnd};
use chizu_core::routing::Graph;
use chizu_scenario::{city_grid_100k, commute_doors, rail_choice_inputs};
use criterion::{Criterion, criterion_group, criterion_main};

fn rail_choice(c: &mut Criterion) {
    let net = city_grid_100k();
    let walk = Graph::build(&net, LaneMode::Sidewalk);
    let inputs = rail_choice_inputs(1);
    let planner =
        RailPlanner::from_shapes(inputs.doors.clone(), &inputs.lines).with_walks(&net, &walk);
    let choice = RideChoice::from_parts(&planner, inputs.live.clone(), Default::default(), 5);
    let ends = commute_doors(1, &net, 1_000);
    c.bench_function("rail_choice_1k", |b| {
        b.iter(|| {
            for [(ta, la, oa), (tz, lz, oz)] in &ends {
                let a = WalkEnd::on_link(&net, *ta, *la, *oa);
                let z = WalkEnd::on_link(&net, *tz, *lz, *oz);
                black_box(choice.offer(&a, &z));
            }
        });
    });
    // 路線か本数だけが変わったときは、駅まで歩く表を使い回し、駅の空間索引と駅の組の表だけを作り直す。
    c.bench_function("rail_table_rebuild", |b| {
        b.iter(|| {
            black_box(RailPlanner::from_shapes(
                inputs.doors.clone(),
                &inputs.lines,
            ))
        });
    });
    // 道路の網か駅の出入口が変わったときは、駅まで歩く表も作り直す。
    c.bench_function("rail_walk_tables", |b| {
        let planner = RailPlanner::from_shapes(inputs.doors.clone(), &inputs.lines);
        b.iter(|| black_box(planner.clone().with_walks(&net, &walk)));
    });
}

criterion_group!(benches, rail_choice);
criterion_main!(benches);
