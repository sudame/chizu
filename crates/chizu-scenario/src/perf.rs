//! ネイティブと WASM で同じ条件で回す性能の測定。
//!
//! WASM では `std::time::Instant` が使えないので、時計は呼び出し側から渡す。時計はナノ秒を返す。
//! 並列に回すスレッド数は、呼び出し側が rayon のスレッドプールで決める。

use std::hint::black_box;

use chizu_core::routing::{AstarRouter, Router};
use chizu_core::{
    Activity, BUCKET_COUNT, CitizenChange, CitizenId, CitizenRow, Population, RngPurpose,
    ScheduleState, TICKS_PER_DAY, Tick, Trips, bucket_at, rng,
};

use chizu_core::rail::choice::{RailPlanner, RideChoice};

use crate::{
    RAIL_BENCH_LINES, city_grid_100k, commute_requests, commute_tiles, rail_choice_inputs,
    rail_trains_running, synthetic_population, walk_ends, zoned_grid_city,
};

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
/// 道路を引いたあとの最初の `World::step` は経路のグラフを作り直すので、1 回進めてから測り始める。
/// `left` は建った建物の数である。
pub fn day(citizens: u32, ticks: u64, now: &dyn Fn() -> u64) -> TickStats {
    let mut w = zoned_grid_city(SEED);
    w.step();
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
        black_box(trips.step(&net, &mut router, Tick::ZERO));
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

/// 列車 512 本（分岐と共有する区間のある [`RAIL_BENCH_LINES`] 本の路線に [`crate::RAIL_BENCH_TRAINS`] 本ずつ）を
/// `ticks` tick 動かす。1 tick は、列車の動き、閉塞の出入り、駅の止まりと折り返し、共有する区間をまとめて押さえる決まり、
/// 行き詰まりの輪の検査である。列車が出そろってふだんの走り方になるまでは測らない。
/// `left` は回し終えたときの列車の数である。
pub fn rail_trains(ticks: u64, now: &dyn Fn() -> u64) -> TickStats {
    let mut rail = rail_trains_running(SEED, RAIL_BENCH_LINES);
    let count = u32::try_from(rail.train_count()).expect("列車が多すぎる");
    let mut tick = Tick(1_000_000);
    let mut worst = 0;
    let start = now();
    for _ in 0..ticks {
        let t = now();
        rail.step(tick);
        worst = u64::max(worst, now() - t);
        tick = tick.next();
    }
    TickStats {
        count,
        ticks,
        total_ns: now() - start,
        worst_ns: worst,
        left: rail.train_count(),
    }
}

/// 手段の選び方の時間。
#[derive(Clone, Copy, Debug)]
pub struct ChoiceStats {
    pub requests: u32,
    /// 1 件の平均（ns）。
    pub mean_ns: u64,
    /// 朝の山の 1 tick ぶん（1,000 件）の、いちばん長かった時間（ns）。
    pub peak_tick_worst_ns: u64,
    /// 鉄道の候補があった件数。
    pub offered: usize,
}

impl ChoiceStats {
    pub fn to_json(&self) -> String {
        format!(
            r#"{{"requests":{},"mean_ns":{},"peak_tick_worst_ns":{},"offered":{}}}"#,
            self.requests, self.mean_ns, self.peak_tick_worst_ns, self.offered
        )
    }
}

/// 人口 10 万の規模の格子に、駅 200 と路線 64 本を置き、`requests` 件の移動を鉄道の候補の見積もりに通す。
/// 設計 3.10 の 7 の `rail_choice_100k` である。1 件は、駅の空間索引を 2 回引き、9 つの駅の組の表を引く。
pub fn rail_choice(requests: u32, now: &dyn Fn() -> u64) -> ChoiceStats {
    let net = city_grid_100k();
    let inputs = rail_choice_inputs(SEED);
    let planner = RailPlanner::from_shapes(inputs.doors, &inputs.lines);
    let choice = RideChoice::from_parts(&planner, inputs.live, Default::default(), 5);
    let ends = commute_tiles(SEED, &net, requests);
    let mut offered = 0;
    let mut worst = 0;
    let start = now();
    for chunk in ends.chunks(1_000) {
        let t = now();
        for (a, b) in chunk {
            offered += usize::from(black_box(choice.offer(*a, *b)).is_some());
        }
        worst = u64::max(worst, now() - t);
    }
    let total = now() - start;
    ChoiceStats {
        requests,
        mean_ns: total / u64::from(requests.max(1)),
        peak_tick_worst_ns: worst,
        offered,
    }
}

/// 駅の組の表の作り直しの時間。
#[derive(Clone, Copy, Debug)]
pub struct RebuildStats {
    pub runs: u32,
    pub mean_ns: u64,
    pub worst_ns: u64,
    /// 組の数と、そのうち乗り換え 1 回の進路を持つ組の数。
    pub pairs: usize,
    pub transfer_pairs: usize,
}

impl RebuildStats {
    pub fn to_json(&self) -> String {
        format!(
            r#"{{"runs":{},"mean_ns":{},"worst_ns":{},"pairs":{},"transfer_pairs":{}}}"#,
            self.runs, self.mean_ns, self.worst_ns, self.pairs, self.transfer_pairs
        )
    }
}

/// 駅 200 と路線 64 本で、駅の空間索引と駅の組の表（乗り換え 1 回を含む）を `runs` 回作り直す時間（ns）の
/// 平均と最悪と、組の数。設計 3.10 の 7 の `rail_table_rebuild` である。
pub fn rail_table_rebuild(runs: u32, now: &dyn Fn() -> u64) -> RebuildStats {
    let inputs = rail_choice_inputs(SEED);
    let mut total = 0;
    let mut worst = 0;
    let mut pairs = 0;
    let mut transfer_pairs = 0;
    for _ in 0..runs {
        let doors = inputs.doors.clone();
        let t = now();
        let planner = RailPlanner::from_shapes(doors, &inputs.lines);
        let took = now() - t;
        total += took;
        worst = worst.max(took);
        let planner = black_box(planner);
        pairs = planner.pairs();
        transfer_pairs = planner.transfer_pairs();
    }
    RebuildStats {
        runs,
        mean_ns: total / u64::from(runs.max(1)),
        worst_ns: worst,
        pairs,
        transfer_pairs,
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

/// 測る項目を小さい規模で一度ずつ回す。
///
/// V8 は WASM をまず速くコンパイルする Liftoff で動かし、よく呼ぶ関数を TurboFan でコンパイルし直す。
/// 先に回しておき、コンパイルし直す時間を測った値に入れない。
pub fn warm_up(now: &dyn Fn() -> u64) {
    black_box(day(10_000, 2 * u64::from(BUCKET_COUNT), now));
    black_box(movers(500, 20, now));
    black_box(route_batch(1_000, 1, now));
    black_box(rail_trains(20, now));
    black_box(rail_choice(2_000, now));
    black_box(rail_table_rebuild(1, now));
}

/// 測る項目をすべて回し、JSON の 1 つのオブジェクトにする。
/// 始めに [`warm_up`] を回す。
///
/// `memory` は、項目を回すたびに呼ぶメモリの大きさ（バイト）の読み取りである。
/// WASM の線形メモリは縮まないので、ある項目のあとの値は、そこまでで最も大きかったときの値になる。
pub fn run_all(now: &dyn Fn() -> u64, memory: &dyn Fn() -> Option<u64>) -> String {
    let json_mem = |m: Option<u64>| m.map_or("null".to_string(), |b| b.to_string());
    let base = memory();
    warm_up(now);
    let day_100k = day(100_000, TICKS_PER_DAY, now);
    let mem_100k = memory();
    // 100 万人は 1 日を回すと長すぎるので、1/10 日で 1 日を見積もる。
    let day_1m = day(1_000_000, TICKS_PER_DAY / 10, now);
    let mem_1m = memory();
    let movers_20k = movers(10_000, 300, now);
    let batch_1k = route_batch(1_000, 20, now);
    let batch_100k = route_batch(100_000, 3, now);
    let trains_512 = rail_trains(1_000, now);
    let choice_100k = rail_choice(250_000, now);
    let rebuild = rail_table_rebuild(10, now);
    format!(
        r#"{{"day_100k":{},"day_1m":{},"movers_20k":{},"route_batch_1k":{},"route_batch_100k":{},"rail_trains":{},"rail_choice_100k":{},"rail_table_rebuild":{},"memory_bytes":{{"start":{},"after_100k":{},"after_1m":{}}}}}"#,
        day_100k.to_json(),
        day_1m.to_json(),
        movers_20k.to_json(),
        batch_1k.to_json(),
        batch_100k.to_json(),
        trains_512.to_json(),
        choice_100k.to_json(),
        rebuild.to_json(),
        json_mem(base),
        json_mem(mem_100k),
        json_mem(mem_1m),
    )
}
