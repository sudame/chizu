//! 住民データ層の 1 tick ぶんの組の更新にかかる時間。
//!
//! - `standard`：`World::step` と同じ標準の処理（`update_citizen`）。[`serviced_grid_city`] を住民 10 万人まで
//!   育てた街の住民と建物の表で、幸福度の点数を求める時間を測る。
//! - `rng_write`：合成の住民ごとに乱数を引き、幸福度と出発の予定を書き戻す。組をなめる仕組みの目安にする。

use std::hint::black_box;

use chizu_core::happiness::{CitizenContext, update_citizen};
use chizu_core::{
    Activity, BUCKET_COUNT, CitizenChange, CitizenId, CitizenRow, Population, RngPurpose,
    ScheduleState, TICKS_PER_DAY, Tick, ZoneKind, bucket_at, rng,
};
use chizu_scenario::{serviced_grid_city, synthetic_population};
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

const SEED: u64 = 1;

fn rng_write(
    tick: Tick,
) -> impl Fn(&Population, CitizenId, CitizenRow) -> Option<CitizenChange> + Sync {
    move |_, id, row| {
        let mut r = rng(SEED, RngPurpose::DailySchedule, u64::from(id.index), tick);
        let mut happiness = row.happiness;
        happiness.commute = r.below(101) as u8;
        Some(CitizenChange {
            schedule: ScheduleState {
                next: Activity::Work,
                depart_at: Some(tick + 1 + u64::from(r.below(7_200))),
            },
            happiness,
        })
    }
}

fn citizen_bucket(c: &mut Criterion) {
    let mut group = c.benchmark_group("citizen_bucket");
    let mut city = serviced_grid_city(SEED);
    while city.population().citizen_count() < 100_000 {
        assert!(city.tick().0 < 15 * TICKS_PER_DAY, "10 万人に届かない");
        city.step();
    }
    let tax_permille = ZoneKind::ALL.map(|z| city.tax_permille(z));
    let mut p = city.population().clone();
    let buildings = city.buildings();
    let mut tick = city.tick();
    group.bench_function(BenchmarkId::new("standard", "city_100k"), |b| {
        b.iter(|| {
            p.update_bucket(bucket_at(tick), |population, id, row| {
                let ctx = CitizenContext {
                    seed: SEED,
                    tick,
                    population,
                    buildings,
                    tax_permille,
                };
                update_citizen(&ctx, id, row)
            });
            tick = tick.next();
        });
    });
    black_box(&p);

    for n in [100_000_u32, 1_000_000] {
        let label = format!("{}k", n / 1_000);
        // 一巡させて、全員が出発の待ち行列に入った状態から測る。
        let mut p = synthetic_population(SEED, n);
        let mut tick = Tick::ZERO;
        for _ in 0..BUCKET_COUNT {
            p.update_bucket(bucket_at(tick), rng_write(tick));
            tick = tick.next();
        }
        group.bench_function(BenchmarkId::new("rng_write", &label), |b| {
            b.iter(|| {
                p.update_bucket(bucket_at(tick), rng_write(tick));
                tick = tick.next();
            });
        });
        black_box(&p);
    }
    group.finish();
}

criterion_group!(benches, citizen_bucket);
criterion_main!(benches);
