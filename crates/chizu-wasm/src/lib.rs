//! chizu-core と画面（Web Worker）のあいだの受け渡しの層。

pub mod command_input;
pub mod sim;
pub mod snapshot;

use chizu_core::{ChunkCoord, Command};
use command_input::Field;
use js_sys::Reflect;
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

/// 三重バッファの配置を JSON で返す。画面の側の定数と食い違っていないかを確かめるのに使う。
#[wasm_bindgen]
pub fn snapshot_layout() -> String {
    snapshot::layout_json()
}

/// 等速のときに実時間の 1 秒で進める tick 数。
#[wasm_bindgen]
pub fn ticks_per_second() -> u32 {
    chizu_core::clock::TICKS_PER_SECOND_AT_NORMAL_SPEED
}

/// Worker から使うシミュレーション。
#[wasm_bindgen]
pub struct Simulation(sim::Sim);

#[wasm_bindgen]
impl Simulation {
    #[wasm_bindgen(constructor)]
    pub fn new(seed: u64) -> Simulation {
        Simulation(sim::Sim::new(seed))
    }

    /// 三重バッファの先頭の、WASM のメモリの中でのバイト位置。
    pub fn snapshot_offset(&self) -> usize {
        self.0.snapshot().byte_offset()
    }

    /// 画面から届いたコマンドを `Command` に直し、検証して記録する。
    pub fn submit(&mut self, command: JsValue) -> Result<(), JsError> {
        let command = command_from_js(&command)?;
        self.0
            .submit(command)
            .map(|_| ())
            .map_err(|e| JsError::new(&e.to_string()))
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.0.set_paused(paused);
    }

    /// 等速の何倍で回しているか。
    pub fn speed(&self) -> u16 {
        self.0.world().speed().multiplier()
    }

    /// チャンクの版。画面の手元の版と違うときだけマスを送り直す。
    pub fn chunk_version(&self, cx: i32, cy: i32) -> u32 {
        self.0.chunk_version(ChunkCoord::new(cx, cy))
    }

    pub fn chunk_owned(&self, cx: i32, cy: i32) -> bool {
        self.0.chunk_owned(ChunkCoord::new(cx, cy))
    }

    /// チャンクのマスを 64×64 バイトで返す。ビットの意味は `sim::TILE_WATER` にある。
    pub fn chunk_tiles(&self, cx: i32, cy: i32) -> Vec<u8> {
        self.0.chunk_tiles(ChunkCoord::new(cx, cy))
    }

    /// 一時停止していなければ `ticks` だけ進め、いまの状態を三重バッファに書く。
    pub fn advance(&mut self, ticks: u32) {
        self.0.advance(ticks);
        self.0.publish();
    }
}

fn command_from_js(value: &JsValue) -> Result<Command, JsError> {
    let field = |name: &str| match Reflect::get(value, &JsValue::from_str(name)) {
        Ok(v) if v.is_undefined() => Field::Missing,
        Ok(v) => match (v.as_f64(), v.as_string()) {
            (Some(n), _) => Field::Number(n),
            (None, Some(s)) => Field::Text(s),
            (None, None) => Field::Other,
        },
        Err(_) => Field::Missing,
    };
    command_input::parse(value.is_object(), field).map_err(|e| JsError::new(&e.to_string()))
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
