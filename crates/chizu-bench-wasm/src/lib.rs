//! ブラウザで性能を測るための入口。測る中身は [`chizu_scenario::perf`] にあり、ネイティブのベンチと共有する。

use wasm_bindgen::prelude::*;

#[cfg(target_arch = "wasm32")]
pub use wasm_bindgen_rayon::init_thread_pool;

#[wasm_bindgen]
extern "C" {
    /// Worker の `performance.now()`。ミリ秒を小数で返す。
    #[wasm_bindgen(js_namespace = performance, js_name = now)]
    fn performance_now() -> f64;
}

fn now_ns() -> u64 {
    (performance_now() * 1e6) as u64
}

/// WASM の線形メモリの大きさ（バイト）。
fn memory_bytes() -> Option<u64> {
    #[cfg(target_arch = "wasm32")]
    {
        Some(core::arch::wasm32::memory_size::<0>() as u64 * 65_536)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        None
    }
}

/// 測る項目をすべて回し、結果を JSON の文字列で返す。スレッド数は先に `initThreadPool` で決めておく。
#[wasm_bindgen]
pub fn run_all() -> String {
    chizu_scenario::perf::run_all(&now_ns, &memory_bytes)
}
