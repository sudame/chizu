//! `mise run bench:wasm` が WASM の値と並べるための、ネイティブの値。
//!
//! [`chizu_scenario::perf::run_all`] を、スレッド数 1、4、使える論理コアの数で回し、JSON の 1 行で書き出す。
//! 環境変数 `CHIZU_PERF_THREADS` に `4` や `1,4` を渡すと、そのスレッド数だけで回す。
//! メモリの大きさは WASM でだけ測るので、ここでは `null` になる。

use std::sync::OnceLock;
use std::thread::available_parallelism;
use std::time::Instant;

use chizu_scenario::perf::run_all;

fn main() {
    let start = OnceLock::<Instant>::new();
    let now = || start.get_or_init(Instant::now).elapsed().as_nanos() as u64;
    let cores = available_parallelism().map_or(1, |n| n.get());
    let mut counts: Vec<usize> = match std::env::var("CHIZU_PERF_THREADS") {
        Ok(v) => v
            .split(',')
            .map(|n| {
                n.trim()
                    .parse()
                    .expect("CHIZU_PERF_THREADS がスレッド数の並びでない")
            })
            .collect(),
        Err(_) => vec![1, 4, cores],
    };
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
    println!(
        r#"{{"hardware_concurrency":{cores},"runs":[{}]}}"#,
        runs.join(",")
    );
}
