//! chizu-core と画面（Web Worker）のあいだの受け渡しの層。

pub mod car_path;
pub mod command_input;
pub mod inspect;
pub mod layers;
pub mod lots;
pub mod movers;
pub mod save;
pub mod sim;
pub mod snapshot;
#[cfg(test)]
mod test_town;
pub mod walk_path;

use chizu_core::network::{DIAGONAL_TILE_MM, TILE_MM, road_cost_per_tile};
use chizu_core::{ApplyOutcome, ChunkCoord, Command, RoadKind, ServiceKind, TilePos, TileRect};
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

/// 施設の表。施設の種類（`ServiceKind::ALL` の順に、発電所、浄水場、平面の駐車場、駐車場ビル、続けて駅）ごとに
/// `[費用, 間口, 奥行き, 台数]` を並べる。台数は共同の駐車場の台数で、ほかの施設は 0 である。
#[wasm_bindgen]
pub fn service_table() -> Vec<f64> {
    ServiceKind::ALL
        .into_iter()
        .chain([ServiceKind::Station])
        .flat_map(|k| {
            let (w, d) = k.size();
            [
                k.cost().0 as f64,
                w as f64,
                d as f64,
                f64::from(k.parking_spaces()),
            ]
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
        let net = self.0.world().network();
        let command = command_from_js(&command, |t| net.node_at(t))?;
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
                    ApplyOutcome::AppliedPartly(_) => "AppliedPartly",
                    ApplyOutcome::NotImplemented => "NotImplemented",
                    ApplyOutcome::Rejected => "Rejected",
                    ApplyOutcome::Denied(_) => "Denied",
                };
                set(&o, "outcome", JsValue::from_str(outcome));
                if let ApplyOutcome::Denied(reason) | ApplyOutcome::AppliedPartly(reason) =
                    r.outcome
                {
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

    /// 信号の腕ごとに `[ノードの x, ノードの y, 腕の向きの x, 腕の向きの y, 灯火]`。並びは `sim::Sim::signal_lights` にある。
    pub fn signal_lights(&self) -> Vec<f64> {
        self.0.signal_lights()
    }

    /// 建物の表の版。変わったときだけ建物を読み直す。
    pub fn buildings_version(&self) -> f64 {
        self.0.buildings_version()
    }

    /// 建物ごとに `[min の x, min の y, max の x, max の y, 区画の種類, 段階, 供給]`。区画の種類は `Zone` の番号（高密度の住宅は 1、低密度の住宅は 5）。
    pub fn buildings(&self) -> Vec<f64> {
        self.0.buildings()
    }

    /// 切り欠いた駐車場の形。並びは `lots::encode` にある。版は建物の表と道路網の版（`supply_version`）で見分ける。
    pub fn lots(&self) -> Vec<f64> {
        self.0.lots()
    }

    /// 停めてある車の台数。並びは `lots::encode_parked` にある。毎 tick 変わるので、三重バッファの外で間引いて送る。
    pub fn parked(&self) -> Vec<f64> {
        self.0.parked()
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

    /// マス (x, y) にある建物か施設の様子。何もなければ空の配列。並びは `inspect::encode` にある。
    pub fn inspect(&self, x: f64, y: f64) -> Result<Vec<f64>, JsError> {
        let coord = |v: f64| {
            (v.fract() == 0.0 && v.abs() <= 9_007_199_254_740_991.0)
                .then_some(v as i64)
                .ok_or_else(|| JsError::new("マスの座標が整数でない"))
        };
        Ok(self.0.inspect(TilePos::new(coord(x)?, coord(y)?)))
    }

    /// 押したものの識別子 `[種類, a, b]`（`inspect` の答えの先頭と最後の 2 つ）で様子を求める。
    /// 消えていれば空の配列。
    pub fn inspect_target(&self, kind: f64, a: f64, b: f64) -> Vec<f64> {
        self.0.inspect_target(kind, a, b)
    }

    /// 線路と駅の版。変わったときだけ線路と駅の形を読み直す。
    pub fn rail_revision(&self) -> f64 {
        self.0.rail_revision() as f64
    }

    /// 線路ごとに `[a の x, a の y, b の x, b の y, 駅の線路か（0 か 1）]`。
    pub fn rail_tracks(&self) -> Vec<f64> {
        self.0.rail_tracks()
    }

    /// 駅ごとに `[min の x, min の y, max の x, max の y, 道路へ向かう 1 歩の x, y]`。
    pub fn rail_stations(&self) -> Vec<f64> {
        self.0.rail_stations()
    }

    /// 線路を (x0, y0) から (x1, y1) に引けるかと建設費を、`[理由の文か null, 建設費]` で返す。
    pub fn rail_probe(&self, x0: f64, y0: f64, x1: f64, y1: f64) -> Result<js_sys::Array, JsError> {
        let coord = |v: f64| {
            (v.fract() == 0.0 && v.abs() <= 9_007_199_254_740_991.0)
                .then_some(v as i64)
                .ok_or_else(|| JsError::new("マスの座標が整数でない"))
        };
        let (reason, cost) = self.0.rail_probe(
            TilePos::new(coord(x0)?, coord(y0)?),
            TilePos::new(coord(x1)?, coord(y1)?),
        );
        Ok([
            reason.map_or(JsValue::NULL, JsValue::from_str),
            JsValue::from_f64(cost as f64),
        ]
        .into_iter()
        .collect())
    }

    /// 長方形 (x0, y0)-(x1, y1) を取り壊したときに消えるもの。並びは `sim::Sim::bulldoze_preview` にある。
    pub fn bulldoze_preview(
        &self,
        x0: f64,
        y0: f64,
        x1: f64,
        y1: f64,
    ) -> Result<Vec<f64>, JsError> {
        let coord = |v: f64| {
            (v.fract() == 0.0 && v.abs() <= 9_007_199_254_740_991.0)
                .then_some(v as i64)
                .ok_or_else(|| JsError::new("マスの座標が整数でない"))
        };
        Ok(self.0.bulldoze_preview(
            TilePos::new(coord(x0)?, coord(y0)?),
            TilePos::new(coord(x1)?, coord(y1)?),
        ))
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

    /// 孤立した施設ごとに `[道路に面したマスの x, y, 種類]`。並びは `sim::Sim::isolated_facilities` にある。
    pub fn isolated_facilities(&self) -> Vec<f64> {
        self.0.isolated_facilities()
    }

    /// 駐車場が足りない建物ごとに `[x, y, 区画の種類, 満車で車をやめた数]`。並びは `sim::Sim::parking_shortage` にある。
    pub fn parking_shortage(&self) -> Vec<f64> {
        self.0.parking_shortage()
    }

    /// いまの tick。並びは `sim::Sim::current_tick` にある。
    pub fn current_tick(&self) -> f64 {
        self.0.current_tick()
    }

    /// 車を持てない世帯の数と理由ごとの数、世帯の多い所。並びは `sim::Sim::carless` にある。
    pub fn carless(&self) -> Vec<f64> {
        self.0.carless()
    }

    /// 共同の駐車場を (x, y) に置いたとき、届く範囲の中の車を持てない世帯の数。
    pub fn carless_in_range(&self, x: f64, y: f64) -> Result<f64, JsError> {
        let coord = |v: f64| {
            (v.fract() == 0.0 && v.abs() <= 9_007_199_254_740_991.0)
                .then_some(v as i64)
                .ok_or_else(|| JsError::new("マスの座標が整数でない"))
        };
        Ok(self.0.carless_in_range(TilePos::new(coord(x)?, coord(y)?)))
    }

    /// 区画の種類ごとに、需要が正なのに空き区画がないかを 1 か 0 で並べた 4 個の数。並びは `sim::Sim::lot_shortage` にある。
    pub fn lot_shortage(&self) -> Vec<f64> {
        self.0.lot_shortage()
    }

    /// 収支の見込みの 17 個の数。並びは `sim::Sim::finance` にある。
    pub fn finance(&self) -> Vec<f64> {
        self.0.finance()
    }

    /// `[家のある住民の数, 幸福度の平均, 通勤・生活サービス・暮らし向きの平均, 届いた段階,
    /// 次の段階に届く人口, いまの段階までの枠, 次の段階の枠]`。
    pub fn overview(&self) -> Vec<f64> {
        self.0.overview()
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.0.set_paused(paused);
    }

    /// フィードバックに添える状態。種、tick、所持金、状態ハッシュ（16 進の 16 桁）と、記録に入らない操作を
    /// したか（`unrecordedOps`）を JSON で返す。`unrecordedOps` が true なら、記録を再生しても同じ状態にならない。
    /// 種と所持金は JS の数で誤差なく表せないことがあるので、文字列にする。
    pub fn feedback_state(&self) -> String {
        let w = self.0.world();
        format!(
            "{{\"seed\":\"{}\",\"tick\":{},\"money\":\"{}\",\"stateHash\":\"{:016x}\",\"unrecordedOps\":{}}}",
            w.seed(),
            w.tick().0,
            w.money().0,
            w.state_hash(),
            self.0.has_unrecorded_ops()
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

    /// 移動体を描くマスの範囲（両端を含む）。`clear` なら移動体を書かない。
    pub fn set_mover_view(&mut self, min_x: f64, min_y: f64, max_x: f64, max_y: f64, clear: bool) {
        let tile = |x: f64, y: f64| TilePos::new(x as i64, y as i64);
        let view = (!clear).then(|| TileRect {
            min: tile(min_x, min_y),
            max: tile(max_x, max_y),
        });
        self.0.set_mover_view(view);
    }

    /// 開発用に、マス (`from_x`, `from_y`) と (`to_x`, `to_y`) のあいだに歩く人を `count` 人出す。
    /// 記録に入れないので、フィードバックの記録を再生しても現れない。出せた人数を返す。
    pub fn spawn_dev_walks(
        &mut self,
        from_x: f64,
        from_y: f64,
        to_x: f64,
        to_y: f64,
        count: u32,
    ) -> u32 {
        let tile = |x: f64, y: f64| TilePos::new(x as i64, y as i64);
        self.0
            .spawn_dev_walks(tile(from_x, from_y), tile(to_x, to_y), count)
    }

    /// 一時停止していなければ `ticks` だけ進め、見える範囲の移動体を集める。三重バッファには
    /// [`Simulation::publish`] で書く。あいだに [`Simulation::take_mover_paths`] で道筋を受け取って送ると、
    /// 画面は枠より先に、枠が指す道筋を受け取れる。
    pub fn advance(&mut self, ticks: u32) {
        self.0.advance(ticks);
        self.0.collect_movers();
    }

    /// いまの状態を三重バッファに書く。
    pub fn publish(&mut self) {
        self.0.publish();
    }

    /// 前に呼んでから新しく求めた、車を描く道筋と、忘れた道筋の番号。並びは `car_path::Paths::take` の
    /// とおりである。
    pub fn take_mover_paths(&mut self) -> Vec<f64> {
        self.0.take_mover_paths()
    }
}

fn command_from_js(
    value: &JsValue,
    node_at: impl Fn(TilePos) -> Option<chizu_core::NodeId>,
) -> Result<Command, JsError> {
    let field = |name: &str| match Reflect::get(value, &JsValue::from_str(name)) {
        Ok(v) if v.is_undefined() => Field::Missing,
        Ok(v) => match (v.as_f64(), v.as_string(), v.as_bool()) {
            (Some(n), _, _) => Field::Number(n),
            (None, Some(s), _) => Field::Text(s),
            (None, None, Some(b)) => Field::Bool(b),
            (None, None, None) => Field::Other,
        },
        Err(_) => Field::Missing,
    };
    command_input::parse(value.is_object(), field, node_at)
        .map_err(|e| JsError::new(&e.to_string()))
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
