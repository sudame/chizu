//! ネイティブと WASM で同じ条件で回す性能の測定。
//!
//! WASM では `std::time::Instant` が使えないので、時計は呼び出し側から渡す。時計はナノ秒を返す。
//! 並列に回すスレッド数は、呼び出し側が rayon のスレッドプールで決める。

use std::hint::black_box;

use chizu_core::routing::{AstarRouter, Router};
use chizu_core::{
    Activity, CitizenChange, CitizenId, CitizenRow, Population, RngPurpose, ScheduleState,
    TICKS_PER_DAY, Tick, Trips, bucket_at, rng,
};

use crate::{city_grid_100k, commute_requests, synthetic_population, walk_ends, zoned_grid_city};

/// 測定に使う種。
pub const SEED: u64 = 1;

/// 経路の一括探索で、職場が車のリンクのどれだけに集まっているか。
pub const OFFICE_EVERY: u32 = 100;

/// 住民ごとに乱数を引き、幸福度と出発の予定を書き戻す住民の処理。
/// 組をなめる仕組みの目安にする。
pub fn rng_write(
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

/// 何 tick かを続けて回した時間。`count` は回した規模、`left` は回し終えたときに残った数である。
#[derive(Clone, Copy, Debug)]
pub struct TickStats {
    pub count: u32,
    pub ticks: u64,
    pub total_ns: u64,
    pub worst_ns: u64,
    pub left: usize,
}

impl TickStats {
    pub fn mean_ns(&self) -> u64 {
        self.total_ns / self.ticks.max(1)
    }

    pub fn to_json(&self) -> String {
        format!(
            r#"{{"count":{},"ticks":{},"total_ns":{},"day_ns":{},"tick_mean_ns":{},"tick_worst_ns":{},"left":{}}}"#,
            self.count,
            self.ticks,
            self.total_ns,
            self.mean_ns() * TICKS_PER_DAY,
            self.mean_ns(),
            self.worst_ns,
            self.left
        )
    }
}

/// 建物が建つ街と `citizens` 人の住民の表で、`ticks` tick を回す。
/// JSON の `day_ns` は、1 tick の平均から見積もった 1 日（7,200 tick）の時間である。
///
/// 1 tick は、[`zoned_grid_city`] の `World::step`（建物が建つ処理）と、住民の表の 1 組を
/// [`rng_write`] で更新する処理である。住民の表は `World` の外に持つ。街と住民の表を作る時間は測らない。
/// `left` は建った建物の数である。
pub fn day(citizens: u32, ticks: u64, now: &dyn Fn() -> u64) -> TickStats {
    let mut w = zoned_grid_city(SEED);
    let mut p = synthetic_population(SEED, citizens);
    let mut tick = Tick::ZERO;
    let mut worst = 0;
    let start = now();
    for _ in 0..ticks {
        let t = now();
        w.step();
        p.update_bucket(bucket_at(tick), rng_write(tick));
        worst = u64::max(worst, now() - t);
        tick = tick.next();
    }
    let total = now() - start;
    black_box(&p);
    TickStats {
        count: citizens,
        ticks,
        total_ns: total,
        worst_ns: worst,
        left: w.buildings().len(),
    }
}

/// 人口 10 万の規模の格子で、歩く人と車を `each` ずつ出し、`ticks` tick 進める。
/// 出発と行き先は、ネイティブのベンチの `walk_tick` と `car_tick` と同じである。`left` は着かずに残った移動体の数である。
pub fn movers(each: u32, ticks: u64, now: &dyn Fn() -> u64) -> TickStats {
    let net = city_grid_100k();
    let mut router = AstarRouter::new(&net);
    let mut trips = Trips::default();
    let ends = walk_ends(SEED, &net, each);
    trips.spawn_many(&net, &mut router, &ends, Tick::ZERO);
    trips.spawn_drives(&net, &mut router, &ends, Tick::ZERO);
    let mut worst = 0;
    let start = now();
    for _ in 0..ticks {
        let t = now();
        black_box(trips.step(&net, &mut router));
        worst = u64::max(worst, now() - t);
    }
    TickStats {
        count: each * 2,
        ticks,
        total_ns: now() - start,
        worst_ns: worst,
        left: trips.len(),
    }
}

/// 経路の一括探索の時間。
#[derive(Clone, Copy, Debug)]
pub struct BatchStats {
    pub requests: u32,
    pub runs: u32,
    pub mean_ns: u64,
    pub found: usize,
}

impl BatchStats {
    pub fn to_json(&self) -> String {
        format!(
            r#"{{"requests":{},"runs":{},"mean_ns":{},"found":{}}}"#,
            self.requests, self.runs, self.mean_ns, self.found
        )
    }
}

/// 人口 10 万の規模の格子で、`requests` 件の通勤を、覚えた結果がない状態から `runs` 回解く。
/// ネイティブのベンチの `route_batch/bundled/*/offices_1_100` と同じ条件である。
pub fn route_batch(requests: u32, runs: u32, now: &dyn Fn() -> u64) -> BatchStats {
    let net = city_grid_100k();
    let router = AstarRouter::new(&net);
    let reqs = commute_requests(SEED, &net, requests, OFFICE_EVERY);
    let mut total = 0;
    let mut found = 0;
    for _ in 0..runs {
        let mut r = router.clone();
        let t = now();
        let routes = r.route_batch(&reqs);
        total += now() - t;
        found = routes.iter().filter(|r| r.is_some()).count();
        black_box(routes);
    }
    BatchStats {
        requests,
        runs,
        mean_ns: total / u64::from(runs.max(1)),
        found,
    }
}

/// 測る項目をすべて回し、JSON の 1 つのオブジェクトにする。
///
/// `memory` は、項目を回すたびに呼ぶメモリの大きさ（バイト）の読み取りである。
/// WASM の線形メモリは縮まないので、ある項目のあとの値は、そこまでで最も大きかったときの値になる。
pub fn run_all(now: &dyn Fn() -> u64, memory: &dyn Fn() -> Option<u64>) -> String {
    let json_mem = |m: Option<u64>| m.map_or("null".to_string(), |b| b.to_string());
    let base = memory();
    let day_100k = day(100_000, TICKS_PER_DAY, now);
    let mem_100k = memory();
    // 100 万人は 1 日を回すと長すぎるので、1/10 日で 1 日を見積もる。
    let day_1m = day(1_000_000, TICKS_PER_DAY / 10, now);
    let mem_1m = memory();
    let movers_20k = movers(10_000, 300, now);
    let batch_1k = route_batch(1_000, 20, now);
    let batch_100k = route_batch(100_000, 3, now);
    format!(
        r#"{{"day_100k":{},"day_1m":{},"movers_20k":{},"route_batch_1k":{},"route_batch_100k":{},"memory_bytes":{{"start":{},"after_100k":{},"after_1m":{}}}}}"#,
        day_100k.to_json(),
        day_1m.to_json(),
        movers_20k.to_json(),
        batch_1k.to_json(),
        batch_100k.to_json(),
        json_mem(base),
        json_mem(mem_100k),
        json_mem(mem_1m),
    )
}
