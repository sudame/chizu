//! 乗る人の毎 tick の処理を含む、街全体の 1 tick の時間。列車 1 本の `rail_riders_town` を 3 日回して、次の 2 つを測る。
//!
//! - `rail_riders_peak_tick`：鉄道を離れた人のいちばん多かった tick。降りた人の歩く経路の探索（`release_rail_exits`）、
//!   列車が着いたときの乗り降り、駅に着いた人を入れる `add_rider`（`direct` と `stops`）、待ちきれない人を探す
//!   `give_up` を含む。
//! - `rail_riders_remove_line`：乗る人のいちばん多い時点で路線をなくし、待つ人と乗る人をまとめて歩きに戻す tick。
//!
//! 設計 3.10 の「作る順の 6 で決めた細部」の性能に載せる。

use std::hint::black_box;

use chizu_core::{Command, World};
use chizu_scenario::rail_riders_peak;
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};

fn rail_riders(c: &mut Criterion) {
    let peak = rail_riders_peak(1, 3);
    eprintln!(
        "鉄道を離れた人のいちばん多い tick：{} 人、乗る人のいちばん多い時点：{} 人",
        peak.alighted, peak.riders
    );
    c.bench_function("rail_riders_peak_tick", |b| {
        b.iter_batched(
            || World::from_snapshot(&peak.before_peak).expect("読み戻せない"),
            |mut w| {
                w.step();
                black_box(w)
            },
            BatchSize::LargeInput,
        );
    });
    c.bench_function("rail_riders_remove_line", |b| {
        b.iter_batched(
            || World::from_snapshot(&peak.crowded).expect("読み戻せない"),
            |mut w| {
                let line = w.rail().lines().next().expect("路線がない").0;
                w.apply(&Command::RemoveLine { line });
                w.step();
                black_box(w)
            },
            BatchSize::LargeInput,
        );
    });
}

criterion_group!(benches, rail_riders);
criterion_main!(benches);
