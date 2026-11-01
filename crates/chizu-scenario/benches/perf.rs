//! `mise run bench:wasm` が WASM の値と並べるための、ネイティブの値。
//!
//! 環境変数 `CHIZU_PERF_THREADS` に `4` や `1,4` のようにスレッド数を渡すと、
//! そのスレッド数ごとに [`chizu_scenario::perf::run_all`] を回し、JSON の 1 行で書き出す。
//! 1 回に数分かかるので、`mise run bench` のように環境変数なしで呼ばれたときは何もしない。
//! メモリの大きさは WASM でだけ測るので、ここでは `null` になる。

use std::sync::OnceLock;
use std::thread::available_parallelism;
use std::time::Instant;

use chizu_scenario::perf::run_all;

fn main() {
    let start = OnceLock::<Instant>::new();
    let now = || start.get_or_init(Instant::now).elapsed().as_nanos() as u64;
    let Ok(threads) = std::env::var("CHIZU_PERF_THREADS") else {
        eprintln!("CHIZU_PERF_THREADS がないので perf は回さない。mise run bench:wasm から回す");
        return;
    };
    let mut counts: Vec<usize> = threads
        .split(',')
        .map(|n| {
            n.trim()
                .parse()
                .expect("CHIZU_PERF_THREADS がスレッド数の並びでない")
        })
        .collect();
    counts.sort_unstable();
    counts.dedup();
    let runs: Vec<String> = counts
        .iter()
        .map(|&threads| {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .expect("スレッドプールが作れない");
            let results = pool.install(|| run_all(&now, &|| None));
            format!(r#"{{"threads":{threads},"results":{results}}}"#)
        })
        .collect();
    let cores = available_parallelism().map_or(1, |n| n.get());
    println!(
        r#"{{"hardware_concurrency":{cores},"runs":[{}]}}"#,
        runs.join(",")
    );
}
