//! chizu-core と画面（Web Worker）のあいだの受け渡しの層。

pub mod command_input;
pub mod layers;
pub mod sim;
pub mod snapshot;

use chizu_core::network::{DIAGONAL_TILE_MM, TILE_MM, road_cost_per_tile};
use chizu_core::{ApplyOutcome, ChunkCoord, Command, RoadKind, ServiceKind};
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

/// 道路の建設費の表。`[小, 中, 大の 1 マスあたりの費用, 1 マスの長さ (mm), 斜め 1 マスの長さ (mm)]`。
/// 画面は、引く前の見積もりをこの表から出す。
#[wasm_bindgen]
pub fn road_cost_table() -> Vec<f64> {
    let kinds = [RoadKind::Small, RoadKind::Medium, RoadKind::Large];
    kinds
        .map(|k| road_cost_per_tile(k).0 as f64)
        .into_iter()
        .chain([TILE_MM as f64, DIAGONAL_TILE_MM as f64])
        .collect()
}

/// 施設の表。`[発電所の費用, 間口, 奥行き, 浄水場の費用, 間口, 奥行き]`。
#[wasm_bindgen]
pub fn service_table() -> Vec<f64> {
    [ServiceKind::PowerPlant, ServiceKind::WaterPump]
        .into_iter()
        .flat_map(|k| {
            let (w, d) = k.size();
            [k.cost().0 as f64, w as f64, d as f64]
        })
        .collect()
}

/// Worker から使うシミュレーション。
#[wasm_bindgen]
pub struct Simulation(sim::Sim, layers::LayerCache);

#[wasm_bindgen]
impl Simulation {
    #[wasm_bindgen(constructor)]
    pub fn new(seed: u64) -> Simulation {
        Simulation(sim::Sim::new(seed), layers::LayerCache::default())
    }

    /// 三重バッファの先頭の、WASM のメモリの中でのバイト位置。
    pub fn snapshot_offset(&self) -> usize {
        self.0.snapshot().byte_offset()
    }

    /// 画面から届いたコマンドを `Command` に直し、検証して記録し、その場で適用する。記録した番号を返す。
    pub fn submit(&mut self, command: JsValue) -> Result<f64, JsError> {
        let command = command_from_js(&command)?;
        self.0
            .submit(command)
            .map(|seq| seq as f64)
            .map_err(|e| JsError::new(&e.to_string()))
    }

    /// 適用したコマンドの結果を `{ seq, outcome, reason? }` の配列で取り出す。
    pub fn take_reports(&mut self) -> js_sys::Array {
        let set = |o: &js_sys::Object, k: &str, v: JsValue| {
            Reflect::set(o, &JsValue::from_str(k), &v).expect("オブジェクトに書けない");
        };
        self.0
            .take_reports()
            .into_iter()
            .map(|r| {
                let o = js_sys::Object::new();
                set(&o, "seq", JsValue::from_f64(r.seq as f64));
                let outcome = match r.outcome {
                    ApplyOutcome::Applied => "Applied",
                    ApplyOutcome::NotImplemented => "NotImplemented",
                    ApplyOutcome::Rejected => "Rejected",
                    ApplyOutcome::Denied(_) => "Denied",
                };
                set(&o, "outcome", JsValue::from_str(outcome));
                if let ApplyOutcome::Denied(reason) = r.outcome {
                    set(&o, "reason", JsValue::from_str(sim::deny_text(reason)));
                }
                JsValue::from(o)
            })
            .collect()
    }

    /// 道路網の版。変わったときだけ道路の形を読み直す。
    pub fn network_version(&self) -> f64 {
        self.0.network_version() as f64
    }

    /// 区間ごとに `[a の x, a の y, b の x, b の y, 種類]`。座標はマスの座標で、種類は `sim::segment_kind_code`。
    pub fn road_segments(&self) -> Vec<f64> {
        self.0.road_segments()
    }

    /// ノードごとに `[x, y, つながる区間の数]`。
    pub fn road_nodes(&self) -> Vec<f64> {
        self.0.road_nodes()
    }

    /// 建物の表の版。変わったときだけ建物を読み直す。
    pub fn buildings_version(&self) -> f64 {
        self.0.buildings_version()
    }

    /// 建物ごとに `[min の x, min の y, max の x, max の y, 区画の種類, 段階, 供給]`。
    pub fn buildings(&self) -> Vec<f64> {
        self.0.buildings()
    }

    /// 施設を間口のマス (x, y) に置けるかを、向き 0〜3 の順に返す。置けるなら null、置けないなら理由の文。
    /// `service` は `ServiceKind` の値である。
    pub fn service_probe(&self, service: u8, x: f64, y: f64) -> Result<js_sys::Array, JsError> {
        let kind = ServiceKind::from_code(service).ok_or_else(|| JsError::new("知らない施設"))?;
        let coord = |v: f64| {
            (v.fract() == 0.0 && v.abs() <= 9_007_199_254_740_991.0)
                .then_some(v as i64)
                .ok_or_else(|| JsError::new("マスの座標が整数でない"))
        };
        let at = chizu_core::TilePos::new(coord(x)?, coord(y)?);
        Ok(self
            .0
            .service_probe(kind, at)
            .iter()
            .map(|r| match r {
                Ok(()) => JsValue::NULL,
                Err(reason) => JsValue::from_str(sim::deny_text(*reason)),
            })
            .collect())
    }

    /// 施設ごとに `[min の x, min の y, max の x, max の y, 種類]`。
    pub fn facilities(&self) -> Vec<f64> {
        self.0.facilities()
    }

    /// `[建物の数, 世帯の上限, 職場の上限, 住宅・商業・工業・オフィスの需要, 電気の供給量,
    /// 水道の供給量, 電気と水道の需要, 電気の足りない網の数, 水道の足りない網の数]`。
    pub fn city_stats(&self) -> Vec<f64> {
        self.0.city_stats()
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.0.set_paused(paused);
    }

    /// フィードバックに添える状態。種、tick、所持金、状態ハッシュ（16 進の 16 桁）を JSON で返す。
    /// 種と所持金は JS の数で誤差なく表せないことがあるので、文字列にする。
    pub fn feedback_state(&self) -> String {
        let w = self.0.world();
        format!(
            "{{\"seed\":\"{}\",\"tick\":{},\"money\":\"{}\",\"stateHash\":\"{:016x}\"}}",
            w.seed(),
            w.tick().0,
            w.money().0,
            w.state_hash()
        )
    }

    /// これまでの記録を、chizu-cli で再生できる形式で返す。
    pub fn encode_log(&self) -> Vec<u8> {
        self.0.log().encode()
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

    /// `chunk_tiles` の中身が変わったかを見分ける版。チャンクの版と、近くで道路網が変わった回数から作る。
    pub fn chunk_tiles_version(&self, cx: i32, cy: i32) -> f64 {
        self.0.chunk_tiles_version(ChunkCoord::new(cx, cy))
    }

    /// チャンクのマスを 64×64 バイトで返す。ビットの意味は `sim::TILE_WATER` などにある。
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
