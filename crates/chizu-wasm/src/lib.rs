//! chizu-core と画面（Web Worker）のあいだの受け渡しの層。

// 中核の API はまだないが、依存の向きを先に決めておく
use chizu_core as _;
use rayon::prelude::*;
use wasm_bindgen::prelude::*;

#[cfg(target_arch = "wasm32")]
pub use wasm_bindgen_rayon::init_thread_pool;

/// 1 から `n` までの和を rayon で並列に求める。スレッドつきの WASM が動くかを確かめるためのもの。
#[wasm_bindgen]
pub fn parallel_sum(n: u32) -> u64 {
    (1..=u64::from(n)).into_par_iter().sum()
}

/// rayon のスレッドプールのスレッド数。
#[wasm_bindgen]
pub fn thread_count() -> usize {
    rayon::current_num_threads()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parallel_sum_matches_formula() {
        assert_eq!(parallel_sum(0), 0);
        assert_eq!(parallel_sum(1), 1);
        assert_eq!(parallel_sum(100), 5050);
        assert_eq!(parallel_sum(1_000_000), 500_000_500_000);
    }
}
