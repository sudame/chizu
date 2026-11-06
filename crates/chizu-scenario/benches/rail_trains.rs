//! 列車 512 本（分岐と共有する区間のある 2 本の路線の組を 32 組、路線ごとに 8 本。終点は路線ごとに別）を 1 tick 動かす時間。
//! 列車の動き、閉塞の出入り、駅の止まりと折り返し、共有する区間をまとめて押さえる決まり、行き詰まりの輪の検査を含む。
//! 列車が出そろってふだんの走り方になってから、続けて回す tick を 1 つずつ測る。設計 3.10 の 7 の `rail_trains` である。

use std::hint::black_box;

use chizu_core::Tick;
use chizu_scenario::{RAIL_BENCH_LINES, RAIL_BENCH_TRAINS, rail_trains_running};
use criterion::{Criterion, criterion_group, criterion_main};

fn rail_trains(c: &mut Criterion) {
    let mut rail = rail_trains_running(1, RAIL_BENCH_LINES);
    assert_eq!(
        rail.train_count(),
        RAIL_BENCH_LINES * usize::from(RAIL_BENCH_TRAINS)
    );
    let mut tick = Tick(1_000_000);
    c.bench_function("rail_trains_512", |b| {
        b.iter(|| {
            rail.step(tick);
            tick = tick.next();
            black_box(&rail);
        });
    });
}

criterion_group!(benches, rail_trains);
criterion_main!(benches);
