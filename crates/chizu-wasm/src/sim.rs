//! Worker が持つシミュレーションの本体。実時間は持たず、進める tick 数は呼び出し側が決める。

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};

use chizu_core::building::{DemandFactorKind, demand_factors};
use chizu_core::record::RecordError;
use chizu_core::service::building_demand;
use chizu_core::world::economy::{chunk_slots, milestone_population};
use chizu_core::world::growth::GROWTH_INTERVAL;
use chizu_core::zone::{ZONE_DEPTH, near_road};
use chizu_core::{
    ApplyOutcome, CHUNK_AREA, CHUNK_SIDE, ChunkCoord, Command, CommandLog, DenyReason, GameTime,
    Layer, Line, NodeControl, SegmentKind, ServiceKind, StateHasher, TerrainTiles, Tick, TilePos,
    TileRect, TileUse, World, terrain,
};

use crate::snapshot::{Header, Mover, TripleBuffer};

/// [`Sim::overview`] が返す数のうち、固定の並びの個数。そのあとに止まっている道路網の並びが続く。画面の側の `OVERVIEW_LENGTH` と合わせる。
pub const OVERVIEW_LENGTH: usize = 21;

/// [`Sim::city_stats`] が返す数のうち、固定の並びの個数。そのあとに需要の内訳が続く。
/// 画面の側は `city-stats-layout.json` から読むので、変えたら `CHIZU_UPDATE_LAYOUT=1 cargo test -p chizu-wasm` で書き直す。
pub const CITY_STATS_FIXED: usize = 21;

/// 画面の側が読む、街の数字の並びと要因の番号の表。
pub fn city_stats_layout_json() -> String {
    let kinds: Vec<String> = DemandFactorKind::ALL
        .iter()
        .map(|k| format!("    \"{}\": {}", k.name(), *k as u8))
        .collect();
    format!(
        "{{\n  \"fixedLength\": {CITY_STATS_FIXED},\n  \"zones\": 4,\n  \"factorKinds\": {{\n{}\n  }}\n}}\n",
        kinds.join(",\n")
    )
}

pub struct Sim {
    world: World,
    log: CommandLog,
    paused: bool,
    snapshot: TripleBuffer,
    /// 適用したが、まだ画面に渡していない結果。
    reports: Vec<Report>,
    /// 道路網が変わって、区画を塗れるマスが変わりうるチャンクに足していく数。画面向けの版に混ぜる。
    near_road_changes: BTreeMap<ChunkCoord, u32>,
    /// 移動体を描く範囲。画面だけが決め、中核には渡さない。`None` なら移動体を書かない。
    mover_view: Option<TileRect>,
    /// 最後に集めた移動体と、その基準のチャンク。
    movers: Vec<Mover>,
    movers_base: ChunkCoord,
    /// その前の回に集めた移動体。集め直すたびに `movers` と入れ替える。
    movers_before: Vec<Mover>,
    /// 移動体を描くために覚えておく、車ごとの覚え書きと道筋。描画のためだけに覚え、状態には入れない。
    mover_memory: crate::movers::Memory,
    /// `movers` を集めたときの（tick、記録の件数、範囲、記録に入らない操作の回数）。どれも変わらなければ集め直さない。
    movers_key: Option<(Tick, usize, Option<TileRect>, u32)>,
    /// 記録に入らない操作（開発用の道具）をした回数。1 回でもあれば、記録を再生しても同じ状態にならない。
    unrecorded_ops: u32,
    /// 建物の表の版の控え。`(記録の件数, 建つ・育つが走った回数, 版)` で、前の 2 つが変わったときだけ計算し直す。
    buildings_version: Cell<Option<(usize, u64, f64)>>,
    /// 街の数字の電気と水道の分の控え。`(供給の版, 値)` で、版が変わったときだけ計算し直す。
    utility_cache: Cell<Option<(u64, [f64; 14])>>,
}

/// 記録したコマンドを適用した結果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub seq: u64,
    pub outcome: ApplyOutcome,
}

impl Sim {
    pub fn new(seed: u64) -> Sim {
        Sim::from_parts(World::new(seed), CommandLog::new())
    }

    /// 記録をすべて適用し終えた `world` から作る。保存データを読み込んだときに使う。
    pub(crate) fn from_parts(world: World, log: CommandLog) -> Sim {
        Sim {
            world,
            log,
            paused: false,
            snapshot: TripleBuffer::new(),
            reports: Vec::new(),
            near_road_changes: BTreeMap::new(),
            mover_view: None,
            movers: Vec::new(),
            movers_base: ChunkCoord::default(),
            movers_before: Vec::new(),
            mover_memory: crate::movers::Memory::default(),
            movers_key: None,
            unrecorded_ops: 0,
            buildings_version: Cell::new(None),
            utility_cache: Cell::new(None),
        }
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    pub fn log(&self) -> &CommandLog {
        &self.log
    }

    pub fn snapshot(&self) -> &TripleBuffer {
        &self.snapshot
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    /// 一時停止は tick を進めないだけなので、記録しない。
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    /// 検証したコマンドを、いまの tick（まだ `step` していない tick）に記録し、その場で適用する。
    ///
    /// tick T の状態は tick が T 以下のコマンドを適用したものなので、`chizu_core::replay` で
    /// T tick 進めた状態と、ここで受けてすぐ適用した状態は一致する。
    /// 一時停止のあいだも tick T のままなので、止めたまま建てられる。
    pub fn submit(&mut self, command: Command) -> Result<u64, RecordError> {
        command.validate().map_err(RecordError::Invalid)?;
        let seq = self.log.record(self.world.tick(), command)?;
        let touched = self.lines_touched_by(&command);
        let outcome = self.world.apply_recorded(seq, &command);
        if outcome == ApplyOutcome::Applied {
            self.mark_near_road_changes(&touched);
        }
        self.reports.push(Report { seq, outcome });
        Ok(seq)
    }

    /// チャンクの版。メモリに置いていないチャンクは、種から作ったばかりのチャンクと同じ 0 を返す。
    pub fn chunk_version(&self, coord: ChunkCoord) -> u32 {
        self.world.chunk(coord).map_or(0, |c| c.version())
    }

    pub fn chunk_owned(&self, coord: ChunkCoord) -> bool {
        self.world.is_owned(coord)
    }

    /// チャンクのマスを 1 マス 1 バイトで、行ごとに並べて返す。ビットの意味は [`TILE_WATER`] などにある。
    /// メモリに置いていないチャンクは、種から地形を作って返す。中核の状態は変えない。
    ///
    /// 区画を塗れるかは、隣のチャンクの道路にも左右される。チャンクの版だけでは変わったことが
    /// 分からないので、画面に送るかどうかは [`Sim::chunk_tiles_version`] で見分ける。
    pub fn chunk_tiles(&self, coord: ChunkCoord) -> Vec<u8> {
        let Some(chunk) = self.world.chunk(coord) else {
            return encode_terrain(&terrain(self.world.seed(), coord));
        };
        let mut tiles = encode_terrain(chunk.terrain());
        let Some(layer) = chunk.layer(Layer::Ground) else {
            return tiles;
        };
        let side = CHUNK_SIDE as i64;
        let min = TilePos::new(i64::from(coord.cx) * side, i64::from(coord.cy) * side);
        let max = TilePos::new(min.x + side - 1, min.y + side - 1);
        let near = chunk
            .owned()
            .then(|| near_road(self.world.network(), TileRect { min, max }));
        for (i, tile) in tiles.iter_mut().enumerate() {
            let road = layer.occupancy[i] == TileUse::Road;
            if road {
                *tile |= TILE_ROAD;
            }
            if layer.occupancy[i] == TileUse::Building {
                *tile |= TILE_BUILDING;
            }
            if let Some(zone) = layer.zone[i] {
                *tile |= (zone as u8) << TILE_ZONE_SHIFT;
            }
            let zonable = near.as_ref().is_some_and(|n| n[i]) && !road && *tile & TILE_WATER == 0;
            if zonable {
                *tile |= TILE_ZONABLE;
            }
        }
        tiles
    }

    /// [`Sim::chunk_tiles`] の中身が変わったかを見分ける版。チャンクの版と、そのチャンクの近くで
    /// 道路網が変わった回数から作る。JS の数で誤差なく表せるよう、回数は下位 21 ビットだけを使う。
    pub fn chunk_tiles_version(&self, coord: ChunkCoord) -> f64 {
        let changes = self.near_road_changes.get(&coord).copied().unwrap_or(0) & ((1 << 21) - 1);
        f64::from(self.chunk_version(coord)) * f64::from(1_u32 << 21) + f64::from(changes)
    }

    /// コマンドが道路網を変えるなら、変わりうる区間の直線。適用する前に呼ぶ。
    /// ブルドーザは、長方形にかかる区間をすべて挙げる（消えない高速道路も含むが、多めに数えるだけである）。
    fn lines_touched_by(&self, command: &Command) -> Vec<Line> {
        match *command {
            Command::BuildRoad { from, to, .. } => Line::between(from, to).into_iter().collect(),
            Command::Bulldoze { area } => {
                let network = self.world.network();
                let (Some(c0), Some(c1)) = (area.min.chunk(), area.max.chunk()) else {
                    return Vec::new();
                };
                let mut ids = BTreeSet::new();
                for cy in c0.cy..=c1.cy {
                    for cx in c0.cx..=c1.cx {
                        ids.extend(network.segments_in_chunk(ChunkCoord::new(cx, cy)));
                    }
                }
                ids.into_iter()
                    .filter_map(|id| network.segment_line(id))
                    .filter(|l| l.tiles().any(|t| rect_contains(area, t)))
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    /// 直線の各マスから `ZONE_DEPTH` 以内にかかる、メモリにあるチャンクの画面向けの版を上げる。
    /// メモリにない（買っていない）チャンクには塗れるマスがないので、道路網に左右されない。
    fn mark_near_road_changes(&mut self, lines: &[Line]) {
        let mut coords = BTreeSet::new();
        for line in lines {
            for t in line.tiles() {
                for (dx, dy) in [(-1, -1), (1, -1), (-1, 1), (1, 1)] {
                    let corner = TilePos::new(
                        t.x.saturating_add(dx * ZONE_DEPTH),
                        t.y.saturating_add(dy * ZONE_DEPTH),
                    );
                    coords.extend(corner.chunk());
                }
            }
        }
        for coord in coords {
            if self.world.chunk(coord).is_some() {
                let n = self.near_road_changes.entry(coord).or_insert(0);
                *n = n.wrapping_add(1);
            }
        }
    }

    /// 一時停止していなければ `ticks` だけ進める。コマンドは受けたときに適用してあるので、tick を進めるだけである。
    pub fn advance(&mut self, ticks: u32) {
        if self.paused {
            return;
        }
        for _ in 0..ticks {
            self.world.step();
        }
    }

    /// まだ渡していない結果を、適用した順に取り出す。
    pub fn take_reports(&mut self) -> Vec<Report> {
        std::mem::take(&mut self.reports)
    }

    /// 道路網の版。画面は、この値が変わったときだけ道路の形を受け取り直す。
    pub fn network_version(&self) -> u64 {
        self.world.network().version()
    }

    /// 道路の区間を、1 本につき `[a の x, a の y, b の x, b の y, 種類]` の 5 つの数で返す。
    /// 種類は [`segment_kind_code`] の番号である。
    /// 座標はマスの座標で、JS の数で誤差なく表せる範囲にある。
    pub fn road_segments(&self) -> Vec<f64> {
        let network = self.world.network();
        let mut out = Vec::new();
        for (id, segment) in network.segments() {
            let line = network.segment_line(id).expect("区間の直線がない");
            let end = line.end();
            out.extend(
                [line.start.x, line.start.y, end.x, end.y]
                    .map(|v| v as f64)
                    .into_iter()
                    .chain([f64::from(segment_kind_code(segment.kind))]),
            );
        }
        out
    }

    /// 信号の腕を、1 本につき `[ノードの x, ノードの y, 腕の向きの x, 腕の向きの y, 灯火]` の 5 つの数で返す。
    /// 腕の向きはノードから出ていく 1 歩で、灯火は `Light` の値（0 が青、1 が黄、2 が赤）である。
    /// 灯火は、直前に進めた tick に効いた段階を表す。ノードの ID、区間の ID の順に並べる。
    pub fn signal_lights(&self) -> Vec<f64> {
        let net = self.world.network();
        let lights = self.world.trips().lights();
        let mut out = Vec::new();
        for (id, node) in net.nodes() {
            if node.control != NodeControl::Signal {
                continue;
            }
            let t = node.tile();
            for ((dx, dy), light) in lights.arm_lights(net, id) {
                out.extend([t.x, t.y, dx, dy].map(|v| v as f64));
                out.push(f64::from(light as u8));
            }
        }
        out
    }

    /// ノードを、1 つにつき `[x, y, つながる区間の数, 外の世界か（0 か 1）, 横断歩道を外した腕]` の 5 つの数で返す。
    /// 横断歩道を外した腕は、腕の向き（`Heading` の値）のビットを立てた数である。
    pub fn road_nodes(&self) -> Vec<f64> {
        let mut out = Vec::new();
        for (_, node) in self.world.network().nodes() {
            let t = node.tile();
            out.extend([
                t.x as f64,
                t.y as f64,
                node.segments.len() as f64,
                f64::from(u8::from(node.outside)),
                f64::from(node.no_crosswalk),
            ]);
        }
        out
    }

    pub fn set_mover_view(&mut self, view: Option<TileRect>) {
        self.mover_view = view.filter(|v| v.is_ordered());
    }

    /// 開発用に、記録に入れずに歩く人を出す。出せた人数を返す。
    /// 出せなくても経路探索の作り直しなどで状態が変わりうるので、記録に入らない操作として数える。
    pub fn spawn_dev_walks(&mut self, from: TilePos, to: TilePos, count: u32) -> u32 {
        self.unrecorded_ops = self.unrecorded_ops.saturating_add(1);
        crate::movers::spawn_dev_walks(&mut self.world, from, to, count)
    }

    /// 記録に入らない操作をしたか。したなら、記録を再生しても同じ状態にならない。
    pub fn has_unrecorded_ops(&self) -> bool {
        self.unrecorded_ops > 0
    }

    /// 見える範囲の移動体を集め直す。tick、記録、範囲、記録に入らない操作のどれも変わっていなければ前の結果を使う。
    pub fn collect_movers(&mut self) {
        let key = (
            self.world.tick(),
            self.log.len(),
            self.mover_view,
            self.unrecorded_ops,
        );
        if self.movers_key == Some(key) {
            return;
        }
        self.movers_key = Some(key);
        match self.mover_view {
            Some(v) => {
                // 前の回の結果を残しておき、網が変わった直後の歩く人を前に描いた位置に描くのに使う。
                core::mem::swap(&mut self.movers, &mut self.movers_before);
                let before = crate::movers::Before {
                    out: &self.movers_before,
                    base: self.movers_base,
                };
                self.movers_base = crate::movers::visible_movers(
                    &self.world,
                    v,
                    &mut self.movers,
                    &mut self.mover_memory,
                    before,
                );
            }
            None => {
                self.movers.clear();
                self.movers_base = ChunkCoord::default();
            }
        }
    }

    /// 前に渡してから新しく求めた、車を描く道筋（`car_path::Paths::take` の並び）。
    pub fn take_mover_paths(&mut self) -> Vec<f64> {
        self.mover_memory.paths.take()
    }

    /// 建物の表の版。建物が建つ、消える、段階が上がるたびに変わる。表の状態ハッシュの下位 52 ビットを使う。
    /// 施設も、建物の供給の有無も含む。
    ///
    /// 建物の表が変わるのは、コマンドを適用したときと、建つ・育つが走る tick（`GROWTH_INTERVAL` の倍数）と、
    /// その次の tick だけである。次の tick の頭では、建つ・育つで変わった需要に合わせて供給を判定し直す。
    /// 表全体のハッシュは重いので、そのどれかが起きたときだけ計算し直す。
    pub fn buildings_version(&self) -> f64 {
        let key = (self.log.len(), self.growth_runs());
        if let Some((commands, growth, version)) = self.buildings_version.get()
            && (commands, growth) == key
        {
            return version;
        }
        let version = self.buildings_hash();
        self.buildings_version.set(Some((key.0, key.1, version)));
        version
    }

    /// 建物と施設の表の状態ハッシュの下位 52 ビット。
    fn buildings_hash(&self) -> f64 {
        let mut h = StateHasher::new();
        h.write(self.world.buildings());
        h.write(self.world.facilities());
        (h.finish() & ((1 << 52) - 1)) as f64
    }

    /// これまでに進めた tick のうち、建物の表が変わりうる tick の数。`GROWTH_INTERVAL` で割った余りが
    /// 0（建つ・育つ）か 1（供給の判定し直し）の tick を数える。
    fn growth_runs(&self) -> u64 {
        let ticks = self.world.tick().0;
        ticks.div_ceil(GROWTH_INTERVAL) + ticks.saturating_sub(1).div_ceil(GROWTH_INTERVAL)
    }

    /// 建物を、1 つにつき `[min の x, min の y, max の x, max の y, 区画の種類, 段階, 供給]` の 7 つの数で返す。
    /// 区画の種類は `ZoneKind` の値、供給は電気が届いていれば 1、水道が届いていれば 2 を足した値である。
    pub fn buildings(&self) -> Vec<f64> {
        let mut out = Vec::new();
        for (_, b) in self.world.buildings().iter() {
            let a = b.area;
            out.extend([a.min.x, a.min.y, a.max.x, a.max.y].map(|v| v as f64));
            let supply = u8::from(b.supply.power) + 2 * u8::from(b.supply.water);
            out.extend([b.zone as u8, b.level, supply].map(f64::from));
        }
        out
    }

    /// 切り欠いた駐車場の形を、切り欠く建物の分だけ返す。並びは [`crate::lots::encode`] にある。
    /// 間口のどちらの端に寄せるかは道路網で変わるので、版は建物の表と道路網の両方で決まる。
    pub fn lots(&self) -> Vec<f64> {
        crate::lots::encode(
            self.world.network(),
            self.world.buildings().iter().map(|(_, b)| b),
        )
    }

    /// 施設を `at` に置けるかを、向き 0〜3 のそれぞれについて中核の判定で調べる。状態は変えない。
    pub fn service_probe(&self, kind: ServiceKind, at: TilePos) -> [Result<(), DenyReason>; 4] {
        [0, 1, 2, 3].map(|r| self.world.check_service(kind, at, r).map(|_| ()))
    }

    /// マス `at` にある建物か施設の様子を、[`crate::inspect::encode`] の形の数で返す。何もなければ空。
    /// 押された 1 つについてだけ集計し、状態は変えない。
    pub fn inspect(&self, at: TilePos) -> Vec<f64> {
        crate::inspect::encode(crate::inspect::inspect(&self.world, at))
    }

    /// 押したものの識別子（[`crate::inspect::decode_target`]）で様子を返す。消えていたり読めなければ空。
    pub fn inspect_target(&self, kind: f64, a: f64, b: f64) -> Vec<f64> {
        let found = crate::inspect::decode_target(kind, a, b)
            .and_then(|t| crate::inspect::inspect_target(&self.world, t));
        crate::inspect::encode(found)
    }

    /// 施設を、1 つにつき `[min の x, min の y, max の x, max の y, 種類]` の 5 つの数で返す。
    /// 種類は `ServiceKind` の値である。
    pub fn facilities(&self) -> Vec<f64> {
        let mut out = Vec::new();
        for f in self.world.facilities() {
            let a = f.area;
            out.extend([a.min.x, a.min.y, a.max.x, a.max.y].map(|v| v as f64));
            out.push(f64::from(f.kind as u8));
        }
        out
    }

    /// 街の数字を次の 21 個の数で返す。建物の数、世帯の上限、雇える職場の数（税率が高いと建物の上限より少ない）、
    /// 住宅・商業・工業・オフィスの需要、電気の供給量、水道の供給量、電気と水道の需要、
    /// 電気と水道のそれぞれで供給量が需要に足りない網の数、
    /// 電気と水道のそれぞれで施設のない網の数、電気と水道のそれぞれの不足量、
    /// 主の網（需要がいちばん大きい網）の電気の供給量、水道の供給量、需要、
    /// 主の網以外で施設か建物のある網の数（電気、水道の順）。
    /// 供給量は道路網につながった施設の合計である。余りは網をまたいで足さず、主の網で見る。
    /// そのあとに、需要の内訳が続く。区画の種類ごとに、要因の数と、要因ごとの `[要因の番号, 寄与]` を並べる。
    pub fn city_stats(&self) -> Vec<f64> {
        let buildings = self.world.buildings();
        let capacity = buildings.capacity();
        let mut out = vec![
            buildings.len() as f64,
            capacity.households as f64,
            buildings
                .job_slots(chizu_core::ZoneKind::ALL.map(|z| self.world.tax_permille(z)))
                .iter()
                .sum::<u64>() as f64,
        ];
        let breakdown = demand_factors(&self.world.city_stats());
        out.extend(breakdown.totals().map(|d| d as f64));
        out.extend(self.utility_numbers());
        debug_assert_eq!(out.len(), CITY_STATS_FIXED);
        // 固定の並びのあとに、需要の内訳を区画の種類（住宅・商業・工業・オフィスの順）ごとに並べる。
        // 種類ごとに、要因の数、そのあとに要因ごとの `[要因の番号, 寄与]`。番号は `DemandFactorKind` の値である。
        for zone in 0..4 {
            let factors = breakdown.zone(zone);
            out.push(factors.len() as f64);
            for f in factors {
                out.extend([f64::from(f.kind as u8), f.value as f64]);
            }
        }
        out
    }

    /// 街の数字のうち、電気と水道の 14 個。電気の供給量、水道の供給量、需要、足りない網の数（電気、水道）、
    /// 施設のない網の数と不足量、主の網の値と、ほかの網の数。
    /// 道路と建物の全体をたどって求めるので、建物の表と道路網の版（[`crate::layers::supply_version`]）が
    /// 変わらないあいだは前の値を使い回す。
    fn utility_numbers(&self) -> [f64; 14] {
        let version = crate::layers::supply_version(self);
        if let Some((cached, numbers)) = self.utility_cache.get()
            && cached == version
        {
            return numbers;
        }
        let (totals, main, gaps) = crate::layers::utility_summary(&self.world);
        let [power, water, power_short, water_short] = totals;
        let need: u64 = self
            .world
            .buildings()
            .iter()
            .map(|(_, b)| building_demand(b))
            .sum();
        let mut out = [0.0; 14];
        let values = [power, water, need, power_short, water_short]
            .into_iter()
            .chain(gaps)
            .chain(main);
        for (slot, v) in out.iter_mut().zip(values) {
            *slot = v as f64;
        }
        self.utility_cache.set(Some((version, out)));
        out
    }

    /// 孤立した施設（街の道路網につながっていない施設。定義は `chizu_core::service::isolated_facilities`）を、
    /// 施設ごとに `[道路に面したマスの x, y, 種類]` で並べる。種類は `ServiceKind` の値である。
    /// 余りの数や建たない理由とは別の読み取りで、状態は変えない。
    pub fn isolated_facilities(&self) -> Vec<f64> {
        let mut out = Vec::new();
        for f in self.world.isolated_facilities() {
            out.extend([f.front.x as f64, f.front.y as f64, f64::from(f.kind as u8)]);
        }
        out
    }

    /// いまの tick。画面の知らせが、状態の続いた長さを Worker の時計で数えるために使う。
    pub fn current_tick(&self) -> f64 {
        self.world.tick().0 as f64
    }

    /// 区画の種類（住宅・商業・工業・オフィスの順）ごとに、需要が正なのに空き区画がなければ 1、そうでなければ 0 を並べる。
    /// 定義は `chizu_core::World::lot_shortage`。電気や水道の余裕とは別の読み取りで、状態は変えない。
    pub fn lot_shortage(&self) -> Vec<f64> {
        self.world
            .lot_shortage()
            .map(|b| f64::from(u8::from(b)))
            .to_vec()
    }

    /// 収支の見込みを次の数で返す。税率の上限（‰）、区画の種類ごとの税率（‰）、区画の種類ごとの税収、
    /// 道路の維持費、発電所と浄水場の維持費。区画の種類の順は住宅、商業、工業、オフィス。
    /// 金額はどれも 1 日あたりである。
    pub fn finance(&self) -> Vec<f64> {
        let f = self.world.finances();
        let mut out = vec![f64::from(chizu_core::command::MAX_TAX_PERMILLE)];
        out.extend(chizu_core::ZoneKind::ALL.map(|z| f64::from(self.world.tax_permille(z))));
        out.extend(f.taxes.map(|m| m.0 as f64));
        out.push(f.road_upkeep.0 as f64);
        out.extend(f.facility_upkeep.map(|m| m.0 as f64));
        out
    }

    /// 人口と幸福度とマイルストーンを、次の [`OVERVIEW_LENGTH`] 個の数で返す。
    /// 家のある住民の数、幸福度の平均、通勤・生活サービス・暮らし向きの平均、届いた段階、
    /// 次の段階に届く人口、いまの段階までで買える枠の数、次の段階で買える枠の数。
    /// そのあとに、住宅・商業・工業・オフィスの順で、区画ごとに
    /// 電気か水道の余裕がなくて新築が止まっている間口の数、そのうち電気、水道の余裕がない間口の数を並べる。
    /// 建物の表が変わらなくても動く値なので、街の数字（[`Sim::city_stats`]）とは別に返す。
    pub fn overview(&self) -> Vec<f64> {
        let (happiness, parts) = self.world.happiness_summary();
        let step = self.world.milestone();
        let mut out = vec![
            self.world.housed_citizens() as f64,
            f64::from(happiness),
            f64::from(parts.commute),
            f64::from(parts.services),
            f64::from(parts.wealth),
            f64::from(step),
            milestone_population(step + 1) as f64,
            f64::from(chunk_slots(step)),
            f64::from(chunk_slots(step + 1)),
        ];
        let (blocks, stalls) = self.world.growth_report();
        for b in blocks {
            out.extend([b.blocked, b.no_power, b.no_water].map(|v| v as f64));
        }
        // 最初の [`OVERVIEW_LENGTH`] 個のあとに、新築が止まっている道路網ごとの `[x, y, 足りない資源]` を並べる。
        // `x, y` は網の中で止まっている間口のうち座標が最小のマス、足りない資源は電気が 1、水道が 2 の和である。
        // 網は中核が決めた順に並ぶ。
        for n in stalls {
            let lacks = u8::from(n.no_power) + 2 * u8::from(n.no_water);
            out.extend([n.at.x as f64, n.at.y as f64, f64::from(lacks)]);
        }
        out
    }

    /// いまの状態を三重バッファに書く。
    ///
    /// 設計書 2.1 は最高速では移動体を送らないとするが、粗い計算が入るまではどの速度でも細かい計算で
    /// 移動体が動くので、どの速度でも送る。
    pub fn publish(&mut self) {
        self.collect_movers();
        let base = self.movers_base;
        let tick = self.world.tick();
        let time = GameTime::at(tick);
        let header = Header {
            tick: tick.0,
            day: u32::try_from(time.day).unwrap_or(u32::MAX),
            second_of_day: time.second_of_day,
            money: self.world.money().0,
            population: 0,
            speed: self.world.speed().multiplier(),
            paused: self.paused,
            base_cx: base.cx,
            base_cy: base.cy,
        };
        self.snapshot.publish(&header, &self.movers);
    }
}

/// 画面に渡す区間の種類の番号。道路は `RoadKind` の値（1〜3）をそのまま使い、高速道路は 4 にする。
pub fn segment_kind_code(kind: SegmentKind) -> u8 {
    match kind {
        SegmentKind::Road(k) => k as u8,
        SegmentKind::Highway => 4,
    }
}

/// 画面に出す、コマンドを受け付けなかった理由。
pub fn deny_text(reason: DenyReason) -> &'static str {
    match reason {
        DenyReason::NotOwned => "買っていない土地を通る",
        DenyReason::Water => "水辺を通る",
        DenyReason::NotEnoughMoney => "所持金が足りない",
        DenyReason::Overlap => "ほかの道路と重なる",
        DenyReason::CornerCrossing => "斜めの道路と角で交わる",
        DenyReason::HighwayMidway => "高速道路にはその端でしかつなげない",
        DenyReason::NothingToBulldoze => "範囲に取り壊せるものがない",
        DenyReason::NotAdjacent => "買った土地に接していない",
        DenyReason::AlreadyOwned => "もう買ってある",
        DenyReason::NoZonableTile => "道路沿いに区画を塗れるマスがない",
        DenyReason::ZoneUnchanged => "変わるマスがない",
        DenyReason::Occupied => "ほかの施設か道路がある",
        DenyReason::NoRoadAccess => "道路に面していない",
        DenyReason::NoChunkSlot => "買える土地の枠がない。人口が増えると枠が増える",
        DenyReason::NotJunction => "3 本以上の道路が集まる交差点でない",
        DenyReason::SignalUnchanged => "信号はもうその状態になっている",
        DenyReason::NoRoadToChange => "種類を変えられる道路がない（高速道路は変えない）",
        DenyReason::RoadKindUnchanged => "道路はもうその種類になっている",
        DenyReason::NoCrosswalkArm => {
            "その向きに、横断歩道を置ける道路の腕がない（高速道路には置かない）"
        }
        DenyReason::CrosswalkUnchanged => "横断歩道はもうその状態になっている",
    }
}

fn rect_contains(r: TileRect, t: TilePos) -> bool {
    (r.min.x..=r.max.x).contains(&t.x) && (r.min.y..=r.max.y).contains(&t.y)
}

/// 画面に渡すマスのバイトのうち、水辺を表すビット。
pub const TILE_WATER: u8 = 1;
/// 道路が通っているマスのビット。
pub const TILE_ROAD: u8 = 1 << 1;
/// 区画を塗れる（道路から奥行きの範囲にある、買った陸の空き）マスのビット。
pub const TILE_ZONABLE: u8 = 1 << 2;
/// 区画の種類（`ZoneKind` の値、なければ 0）を置くビットの位置。3 ビットを使う。
pub const TILE_ZONE_SHIFT: u8 = 3;
/// 区画の種類を取り出すマスク（`TILE_ZONE_SHIFT` だけずらしたあとに掛ける）。
pub const TILE_ZONE_MASK: u8 = 0b111;
/// 区画に建った建物があるマスのビット。施設のマスには立てない。
pub const TILE_BUILDING: u8 = 1 << 6;

fn encode_terrain(terrain: &TerrainTiles) -> Vec<u8> {
    let mut tiles = vec![0; CHUNK_AREA];
    for (i, tile) in tiles.iter_mut().enumerate() {
        if terrain.is_water(i % CHUNK_SIDE, i / CHUNK_SIDE) {
            *tile |= TILE_WATER;
        }
    }
    tiles
}

#[cfg(test)]
mod tests {
    use super::*;
    use chizu_core::command::MAX_TAX_PERMILLE;
    use chizu_core::{Chunk, RoadKind, SimSpeed, ZoneKind};

    fn has_water(sim: &Sim, coord: ChunkCoord) -> bool {
        sim.chunk_tiles(coord).iter().any(|t| t & TILE_WATER != 0)
    }

    #[test]
    fn tiles_match_the_terrain() {
        let sim = Sim::new(7);
        // 開始地点から離れたチャンクを、水辺の見つかるまで探す
        let coord = (2..200)
            .map(|cx| ChunkCoord::new(cx, 0))
            .find(|&c| has_water(&sim, c))
            .expect("水辺のあるチャンクが見つからない");
        let expected = Chunk::generate(7, coord);
        let tiles = sim.chunk_tiles(coord);
        assert_eq!(tiles.len(), CHUNK_AREA);
        for ly in 0..CHUNK_SIDE {
            for lx in 0..CHUNK_SIDE {
                let water = tiles[ly * CHUNK_SIDE + lx] & TILE_WATER != 0;
                assert_eq!(water, expected.terrain().is_water(lx, ly), "({lx}, {ly})");
            }
        }
    }

    fn tiles_versions(sim: &Sim) -> Vec<(ChunkCoord, f64)> {
        (-2..=2)
            .flat_map(|cy| (-2..=2).map(move |cx| ChunkCoord::new(cx, cy)))
            .map(|c| (c, sim.chunk_tiles_version(c)))
            .collect()
    }

    fn changed(before: &[(ChunkCoord, f64)], after: &[(ChunkCoord, f64)]) -> Vec<ChunkCoord> {
        before
            .iter()
            .zip(after)
            .filter(|(b, a)| b.1 != a.1)
            .map(|(b, _)| b.0)
            .collect()
    }

    #[test]
    fn only_chunks_near_a_changed_road_are_resent() {
        let mut sim = Sim::new(0);
        let before = tiles_versions(&sim);
        sim.submit(road((10, 10), (20, 10))).unwrap();
        let after = tiles_versions(&sim);
        assert_eq!(changed(&before, &after), [ChunkCoord::new(0, 0)]);
        // チャンクの境目から 6 マス以内なら、隣のチャンクも送り直す
        sim.submit(road((60, 61), (62, 61))).unwrap();
        let next = tiles_versions(&sim);
        assert_eq!(
            changed(&after, &next),
            [
                ChunkCoord::new(0, 0),
                ChunkCoord::new(1, 0),
                ChunkCoord::new(0, 1),
                ChunkCoord::new(1, 1)
            ]
        );
        // 消すときも、消した区間の近くだけを送り直す
        sim.submit(Command::Bulldoze {
            area: TileRect::from_corners(TilePos::new(15, 10), TilePos::new(15, 10)),
        })
        .unwrap();
        assert_eq!(
            changed(&next, &tiles_versions(&sim)),
            [ChunkCoord::new(0, 0)]
        );
    }

    #[test]
    fn water_and_unowned_tiles_are_not_zonable() {
        let zonable = |sim: &Sim, x: i64, y: i64| {
            let t = TilePos::new(x, y);
            let (lx, ly) = t.local();
            sim.chunk_tiles(t.chunk().unwrap())[ly * CHUNK_SIDE + lx] & TILE_ZONABLE != 0
        };
        // 買った土地の右端（x = 127）に沿って道路を引くと、その右の買っていない土地は塗れない
        let mut sim = Sim::new(0);
        sim.submit(road((127, 20), (127, 30))).unwrap();
        assert_eq!(sim.world().network().segments().count(), 2);
        assert!(zonable(&sim, 126, 25));
        assert!(!zonable(&sim, 128, 25));
        // 水辺のあるチャンクを買い、水辺のマスの 2 マス先から道路を引いても、水辺のマスは塗れない
        let east = ChunkCoord::new(2, 0);
        let (mut sim, water) = (0..256)
            .find_map(|seed| {
                let sim = Sim::new(seed);
                let tiles = sim.chunk_tiles(east);
                let is_water = |x: usize, y: usize| tiles[y * CHUNK_SIDE + x] & TILE_WATER != 0;
                let at = (0..CHUNK_SIDE)
                    .flat_map(|y| (0..CHUNK_SIDE - 5).map(move |x| (x, y)))
                    .find(|&(x, y)| is_water(x, y) && (1..=5).all(|d| !is_water(x + d, y)))?;
                Some((seed, (128 + at.0 as i64, at.1 as i64)))
            })
            .map(|(seed, at)| {
                let mut sim = Sim::new(seed);
                sim.submit(Command::BuyChunk { coord: east }).unwrap();
                sim.submit(road((at.0 + 2, at.1), (at.0 + 4, at.1)))
                    .unwrap();
                (sim, at)
            })
            .expect("水辺のあるチャンクが見つからない");
        assert!(
            sim.take_reports()
                .iter()
                .all(|r| r.outcome == ApplyOutcome::Applied)
        );
        assert!(!zonable(&sim, water.0, water.1));
        assert!(zonable(&sim, water.0 + 1, water.1));
    }

    #[test]
    fn buildings_and_city_stats_follow_growth() {
        let mut sim = Sim::new(0);
        sim.submit(road((10, 10), (30, 10))).unwrap();
        sim.submit(Command::PaintZone {
            area: TileRect::from_corners(TilePos::new(10, 11), TilePos::new(30, 16)),
            zone: Some(ZoneKind::Residential),
        })
        .unwrap();
        let version = sim.buildings_version();
        assert!(sim.buildings().is_empty());
        assert_eq!(sim.city_stats()[..3], [0.0, 0.0, 0.0]);
        sim.advance(chizu_core::world::growth::GROWTH_INTERVAL as u32 + 1);
        let buildings = sim.buildings();
        let count = sim.world().buildings().len();
        assert!(count > 0, "住宅が建たない");
        assert_eq!(buildings.len(), count * 7);
        assert_eq!(buildings[4], f64::from(ZoneKind::Residential as u8));
        assert_eq!(buildings[5], 1.0);
        assert_eq!(buildings[6], 0.0, "施設がないので電気も水道も届かない");
        // 建物のマスにだけ、建物のビットが立つ
        let tiles = sim.chunk_tiles(ChunkCoord::new(0, 0));
        let side = CHUNK_SIDE as i64;
        for (i, t) in tiles.iter().enumerate() {
            let at = TilePos::new(i as i64 % side, i as i64 / side);
            let built = sim
                .world()
                .buildings()
                .iter()
                .any(|(_, b)| rect_contains(b.area, at));
            assert_eq!(t & TILE_BUILDING != 0, built, "{at:?}");
        }
        // 道路の下側に発電所と浄水場を置くと、どちらも届き、版が変わる
        let before = sim.buildings_version();
        for (kind, x) in [(ServiceKind::PowerPlant, 16), (ServiceKind::WaterPump, 20)] {
            sim.submit(Command::PlaceBuilding {
                kind,
                at: TilePos::new(x, 9),
                rotation: 2,
            })
            .unwrap();
        }
        assert!(
            sim.take_reports()
                .iter()
                .all(|r| r.outcome == ApplyOutcome::Applied)
        );
        assert_ne!(sim.buildings_version(), before);
        assert_eq!(sim.buildings()[6], 3.0);
        assert_eq!(
            sim.facilities()[4],
            f64::from(ServiceKind::PowerPlant as u8)
        );
        assert_eq!(sim.facilities().len(), 10);
        let stats = sim.city_stats();
        let capacity = |k: ServiceKind| k.capacity() as f64;
        assert_eq!(
            stats[7..9],
            [
                capacity(ServiceKind::PowerPlant),
                capacity(ServiceKind::WaterPump)
            ]
        );
        assert!(stats[9] > 0.0);
        let stats = sim.city_stats();
        // 需要の内訳は、種類ごとに足し合わせると需要になる。
        let mut at = CITY_STATS_FIXED;
        for zone in 0..4 {
            let n = stats[at] as usize;
            let sum: f64 = (0..n).map(|i| stats[at + 2 + 2 * i]).sum();
            assert_eq!(sum, stats[3 + zone], "{zone}");
            at += 1 + 2 * n;
        }
        assert_eq!(at, stats.len());
        assert_eq!(stats[0], count as f64);
        assert!(stats[1] > 0.0);
        assert_ne!(sim.buildings_version(), version);
        sim.advance(20_000);
        let o = sim.overview();
        assert!(o.len() >= OVERVIEW_LENGTH && (o.len() - OVERVIEW_LENGTH) % 3 == 0);
        assert!(o[0] >= 0.0, "{o:?}");
    }

    #[test]
    fn overview_reports_lots_stalled_on_a_network_without_facilities() {
        let mut sim = Sim::new(0);
        sim.submit(road((10, 10), (30, 10))).unwrap();
        sim.submit(Command::PaintZone {
            area: TileRect::from_corners(TilePos::new(10, 11), TilePos::new(30, 16)),
            zone: Some(ZoneKind::Residential),
        })
        .unwrap();
        sim.advance(chizu_core::world::growth::GROWTH_INTERVAL as u32 + 1);
        let o = sim.overview();
        let [blocked, no_power, no_water] = [o[9], o[10], o[11]];
        assert!(blocked > 0.0, "{o:?}");
        assert_eq!([no_power, no_water], [blocked, blocked], "{o:?}");
        assert!(o[12..OVERVIEW_LENGTH].iter().all(|v| *v == 0.0), "{o:?}");
        // 発電所だけなら水道だけが、浄水場だけなら電気だけが足りない
        let place = |sim: &mut Sim, kind, x| {
            sim.submit(Command::PlaceBuilding {
                kind,
                at: TilePos::new(x, 9),
                rotation: 2,
            })
            .unwrap();
            sim.advance(chizu_core::world::growth::GROWTH_INTERVAL as u32 + 1);
        };
        place(&mut sim, ServiceKind::PowerPlant, 16);
        let o = sim.overview();
        assert!(o[10] == 0.0 && o[11] > 0.0, "{o:?}");
        place(&mut sim, ServiceKind::WaterPump, 20);
        assert_eq!(sim.overview()[9], 0.0);
    }

    #[test]
    fn growth_stalls_point_at_the_network_without_facilities() {
        let mut sim = Sim::new(0);
        assert_eq!(sim.overview().len(), OVERVIEW_LENGTH);
        sim.submit(road((10, 10), (30, 10))).unwrap();
        sim.submit(Command::PaintZone {
            area: TileRect::from_corners(TilePos::new(10, 11), TilePos::new(30, 16)),
            zone: Some(ZoneKind::Residential),
        })
        .unwrap();
        sim.advance(chizu_core::world::growth::GROWTH_INTERVAL as u32 + 1);
        let o = sim.overview();
        let s = &o[OVERVIEW_LENGTH..];
        assert_eq!(s.len(), 3, "{s:?}");
        assert!(
            (10.0..=30.0).contains(&s[0]) && (10.0..=16.0).contains(&s[1]),
            "{s:?}"
        );
        assert_eq!(s[2], 3.0, "{s:?}");
    }

    /// 何も塗っていない街では、基礎需要があっても 1 にならない。
    #[test]
    fn lot_shortage_is_zero_for_zones_never_painted() {
        let sim = Sim::new(404);
        let flags = sim.lot_shortage();
        assert_eq!(flags, vec![0.0; 4]);
        assert!(sim.city_stats()[3] > 0.0);
    }

    /// つながっていない通りの発電所は、孤立した施設として出る。余りは主の網だけで数え、ほかの網に分ける。
    #[test]
    fn isolated_facilities_and_main_network_numbers_are_reported_apart() {
        let mut sim = Sim::new(0);
        assert!(sim.isolated_facilities().is_empty());
        sim.submit(road((10, 10), (30, 10))).unwrap();
        sim.submit(Command::PaintZone {
            area: TileRect::from_corners(TilePos::new(10, 11), TilePos::new(30, 16)),
            zone: Some(ZoneKind::Residential),
        })
        .unwrap();
        // 主の網には浄水場だけを置くので、電気が足りない
        sim.submit(Command::PlaceBuilding {
            kind: ServiceKind::WaterPump,
            at: TilePos::new(24, 9),
            rotation: 2,
        })
        .unwrap();
        // 離れた通りに、建物のない発電所を置く
        sim.submit(road((10, 40), (30, 40))).unwrap();
        sim.submit(Command::PlaceBuilding {
            kind: ServiceKind::PowerPlant,
            at: TilePos::new(20, 39),
            rotation: 2,
        })
        .unwrap();
        sim.advance(chizu_core::world::growth::GROWTH_INTERVAL as u32 * 3);
        let stats = sim.city_stats();
        assert!(stats[9] > 0.0, "{stats:?}");
        let power = ServiceKind::PowerPlant.capacity() as f64;
        // 主の網の電気は 0、合計は 1 基ぶん。ほかの網は電気だけが 1 つある。
        assert_eq!(stats[7], power, "{stats:?}");
        assert_eq!(
            stats[16..21],
            [
                0.0,
                ServiceKind::WaterPump.capacity() as f64,
                stats[9],
                1.0,
                0.0
            ]
        );
        let isolated = sim.isolated_facilities();
        assert_eq!(isolated.len(), 3, "{isolated:?}");
        assert_eq!(isolated[2], f64::from(ServiceKind::PowerPlant as u8));
        assert!((10.0..=30.0).contains(&isolated[0]) && isolated[1] >= 39.0 && isolated[1] <= 41.0);
    }

    #[test]
    fn overview_shows_power_short_when_only_a_water_pump_stands() {
        let mut sim = Sim::new(0);
        sim.submit(road((10, 10), (30, 10))).unwrap();
        sim.submit(Command::PaintZone {
            area: TileRect::from_corners(TilePos::new(10, 11), TilePos::new(30, 16)),
            zone: Some(ZoneKind::Residential),
        })
        .unwrap();
        sim.submit(Command::PlaceBuilding {
            kind: ServiceKind::WaterPump,
            at: TilePos::new(20, 9),
            rotation: 2,
        })
        .unwrap();
        sim.advance(chizu_core::world::growth::GROWTH_INTERVAL as u32 * 2);
        let o = sim.overview();
        assert!(o[9] > 0.0 && o[10] > 0.0 && o[11] == 0.0, "{o:?}");
    }

    #[test]
    fn overview_starts_neutral_and_points_at_the_first_milestone() {
        let sim = Sim::new(0);
        let o = sim.overview();
        assert!(o.len() >= OVERVIEW_LENGTH && (o.len() - OVERVIEW_LENGTH) % 3 == 0);
        assert_eq!(o[0], 0.0);
        assert_eq!(o[2..5], [50.0, 50.0, 50.0]);
        assert_eq!(o[5], 0.0);
        assert_eq!(o[6], 250.0);
        assert_eq!(o[7], 6.0);
        assert_eq!(o[8], 9.0);
        assert!(o[9..].iter().all(|v| *v == 0.0));
    }

    #[test]
    fn service_probe_does_not_count_the_highway_as_a_road() {
        // 高速道路が東へ出ていく種を選び、その先のチャンクを買って高速道路の脇に置けるようにする
        let seed = (0..256)
            .find(|s| {
                let line = chizu_core::world::highway_line(*s);
                // 南に 10 マスの道路を引くので、同じチャンクの中に収まる位置を選ぶ
                line.step == (1, 0) && line.start.y <= 50
            })
            .expect("東へ出る高速道路の種がない");
        let a = chizu_core::world::highway_line(seed).start.y;
        let mut sim = Sim::new(seed);
        sim.submit(Command::BuyChunk {
            coord: ChunkCoord::new(2, 0),
        })
        .unwrap();
        // 高速道路のすぐ南で、東の縦の普通の道路にも面するマス
        sim.submit(road((132, a + 1), (132, a + 10))).unwrap();
        let outcomes: Vec<_> = sim.take_reports().iter().map(|r| r.outcome).collect();
        assert!(
            outcomes.iter().all(|o| *o == ApplyOutcome::Applied),
            "{seed} {a} {outcomes:?}"
        );
        let probe = sim.service_probe(ServiceKind::WaterPump, TilePos::new(131, a + 1));
        // 0（北の高速道路に向く）は置けず、1（東の普通の道路に向く）は置ける
        assert!(probe[0].is_err());
        assert_eq!(probe[1], Ok(()));
        // 高速道路にしか面していないマスには、どの向きでも置けない。高速道路には歩道がないので、
        // 北に向けると「道路に面していない」になる
        let lonely = sim.service_probe(ServiceKind::WaterPump, TilePos::new(150, a + 1));
        assert_eq!(lonely[0], Err(DenyReason::NoRoadAccess));
        assert!(lonely.iter().all(Result::is_err), "{lonely:?}");
    }

    #[test]
    fn cached_buildings_version_follows_supply_changes() {
        let mut sim = Sim::new(0);
        sim.submit(road((10, 10), (30, 10))).unwrap();
        // 段階が上がるには住む世帯で埋まる必要があるので、外の世界とつなぐ
        let inner = chizu_core::world::highway_line(0).start;
        let corner = (inner.x, 10);
        for (from, to) in [
            ((inner.x, inner.y), corner),
            (corner, (inner.x.clamp(10, 30), 10)),
        ] {
            if from != to {
                sim.submit(road(from, to)).unwrap();
            }
        }
        for (kind, x) in [(ServiceKind::PowerPlant, 16), (ServiceKind::WaterPump, 20)] {
            sim.submit(Command::PlaceBuilding {
                kind,
                at: TilePos::new(x, 9),
                rotation: 2,
            })
            .unwrap();
        }
        sim.submit(Command::PaintZone {
            area: TileRect::from_corners(TilePos::new(10, 11), TilePos::new(30, 16)),
            zone: Some(ZoneKind::Residential),
        })
        .unwrap();
        // 建つ、供給が届く、段階が上がって需要が増える、を 1 tick ずつ確かめる
        let mut supplied = false;
        for _ in 0..(chizu_core::TICKS_PER_DAY + GROWTH_INTERVAL * 3) {
            sim.advance(1);
            assert_eq!(
                sim.buildings_version(),
                sim.buildings_hash(),
                "tick {}",
                sim.world().tick().0
            );
            supplied |= sim.world().buildings().iter().any(|(_, b)| b.supply.power);
        }
        assert!(supplied, "供給が届かない");
        assert!(
            sim.world().buildings().iter().any(|(_, b)| b.level > 1),
            "段階が上がらない"
        );
    }

    #[test]
    fn cached_buildings_version_never_misses_a_change() {
        let mut sim = Sim::new(0);
        sim.submit(road((10, 10), (30, 10))).unwrap();
        sim.submit(Command::PaintZone {
            area: TileRect::from_corners(TilePos::new(10, 11), TilePos::new(30, 16)),
            zone: Some(ZoneKind::Residential),
        })
        .unwrap();
        let fresh = |sim: &Sim| sim.buildings_hash();
        let mut changes = 0;
        let mut last = sim.buildings_version();
        for _ in 0..(GROWTH_INTERVAL * 5) {
            sim.advance(1);
            let v = sim.buildings_version();
            assert_eq!(v, fresh(&sim), "tick {}", sim.world().tick().0);
            if v != last {
                changes += 1;
            }
            last = v;
        }
        assert!(changes > 0, "建物が建たない");
        // 道路を消すと建物も消え、版が変わり、数字は 0 に戻る
        sim.submit(Command::Bulldoze {
            area: TileRect::from_corners(TilePos::new(10, 10), TilePos::new(30, 10)),
        })
        .unwrap();
        assert!(sim.world().buildings().is_empty());
        assert_ne!(sim.buildings_version(), last);
        assert_eq!(sim.buildings_version(), fresh(&sim));
        assert_eq!(sim.city_stats()[..3], [0.0, 0.0, 0.0]);
    }

    #[test]
    fn tiles_carry_roads_zones_and_zonable_tiles() {
        let mut sim = Sim::new(0);
        let origin = ChunkCoord::new(0, 0);
        // 道路の 6 マス先は塗れて、7 マス先は塗れない
        sim.submit(road((10, 10), (20, 10))).unwrap();
        let tile = |sim: &Sim, x: usize, y: usize| sim.chunk_tiles(origin)[y * CHUNK_SIDE + x];
        assert_ne!(tile(&sim, 15, 10) & TILE_ROAD, 0);
        assert_eq!(tile(&sim, 15, 10) & TILE_ZONABLE, 0);
        assert_ne!(tile(&sim, 15, 16) & TILE_ZONABLE, 0);
        assert_eq!(tile(&sim, 15, 17) & TILE_ZONABLE, 0);
        let before = sim.chunk_tiles_version(origin);
        sim.submit(Command::PaintZone {
            area: TileRect::from_corners(TilePos::new(12, 11), TilePos::new(14, 12)),
            zone: Some(ZoneKind::Commercial),
        })
        .unwrap();
        assert_eq!(
            (tile(&sim, 13, 12) >> TILE_ZONE_SHIFT) & TILE_ZONE_MASK,
            ZoneKind::Commercial as u8
        );
        assert_ne!(sim.chunk_tiles_version(origin), before);
        // 隣のチャンクのすぐそばに道路を引くと、こちらのチャンクの版が変わらなくても送り直す
        let east = ChunkCoord::new(1, 0);
        let east_before = sim.chunk_tiles_version(east);
        sim.submit(road((60, 30), (60, 40))).unwrap();
        assert_ne!(sim.chunk_tiles_version(east), east_before);
        assert_ne!(sim.chunk_tiles(east)[30 * CHUNK_SIDE] & TILE_ZONABLE, 0);
    }

    #[test]
    fn chunks_outside_memory_read_as_fresh_unowned_chunks() {
        let sim = Sim::new(0);
        let start = ChunkCoord::new(0, 0);
        assert!(sim.chunk_owned(start));
        assert_ne!(sim.chunk_version(start), 0);
        let far = ChunkCoord::new(40, -40);
        assert!(!sim.chunk_owned(far));
        assert_eq!(sim.chunk_version(far), 0);
    }

    #[test]
    fn buying_a_chunk_changes_its_version() {
        let mut sim = Sim::new(0);
        let coord = ChunkCoord::new(2, 0);
        let before = sim.chunk_version(coord);
        sim.submit(Command::BuyChunk { coord }).unwrap();
        sim.advance(1);
        assert!(sim.chunk_owned(coord));
        assert_ne!(sim.chunk_version(coord), before);
    }

    #[test]
    fn command_applies_before_the_current_tick_steps() {
        let mut sim = Sim::new(0);
        sim.advance(2);
        let speed = SimSpeed::new(4).unwrap();
        assert_eq!(sim.submit(Command::SetSimSpeed { speed }), Ok(0));
        assert_eq!(sim.log().entries()[0].tick, Tick(2));
        assert_eq!(sim.world().speed(), speed);
        assert_eq!(sim.world().tick(), Tick(2));
    }

    #[test]
    fn state_right_after_submit_matches_replay() {
        let mut sim = Sim::new(2);
        sim.advance(4);
        sim.submit(road((0, 0), (4, 0))).unwrap();
        sim.submit(Command::Bulldoze {
            area: TileRect::from_corners(TilePos::new(0, 0), TilePos::new(4, 0)),
        })
        .unwrap();
        let mut world = World::new(2);
        chizu_core::replay(&mut world, sim.log(), 4);
        assert_eq!(world.state_hash(), sim.world().state_hash());
        // そこから続けても、同じコマンドを二度は適用しない
        sim.advance(3);
        chizu_core::replay(&mut world, sim.log(), 3);
        assert_eq!(world.state_hash(), sim.world().state_hash());
    }

    #[test]
    fn building_while_paused_matches_replay() {
        let mut sim = Sim::new(5);
        sim.advance(3);
        sim.set_paused(true);
        sim.submit(road((0, 0), (5, 0))).unwrap();
        sim.submit(road((2, -3), (2, 3))).unwrap();
        let version = World::new(5).network().version();
        assert_eq!(sim.network_version(), version + 2, "止めたままでも建つ");
        assert_eq!(sim.world().tick(), Tick(3));
        sim.set_paused(false);
        sim.advance(2);
        sim.submit(road((0, 0), (0, 5))).unwrap();
        sim.advance(1);
        let mut world = World::new(5);
        chizu_core::replay(&mut world, sim.log(), 6);
        assert_eq!(world.tick(), sim.world().tick());
        assert_eq!(world.state_hash(), sim.world().state_hash());
    }

    #[test]
    fn invalid_command_is_not_recorded() {
        let mut sim = Sim::new(0);
        let bad = Command::SetTaxRate {
            zone: ZoneKind::Office,
            permille: MAX_TAX_PERMILLE + 1,
        };
        assert!(matches!(sim.submit(bad), Err(RecordError::Invalid(_))));
        assert!(sim.log().is_empty());
    }

    #[test]
    fn finance_follows_tax_rate_commands_and_the_log() {
        let mut sim = Sim::new(0);
        let before = sim.finance();
        assert_eq!(before.len(), 12);
        assert_eq!(before[0], f64::from(MAX_TAX_PERMILLE));
        // 住宅、商業、工業、オフィスの順の税率
        assert_eq!(&before[1..5], &[100.0; 4]);
        let set = Command::SetTaxRate {
            zone: ZoneKind::Commercial,
            permille: 170,
        };
        sim.submit(set).unwrap();
        sim.advance(1);
        let after = sim.finance();
        assert_eq!(&after[1..5], &[100.0, 170.0, 100.0, 100.0]);
        // 記録に残る
        assert_eq!(sim.log().len(), 1);
    }

    /// 税率を変えると、建物の表が変わらなくても、街の数字の需要と内訳が変わる。
    #[test]
    fn city_stats_factors_follow_tax_rate_commands() {
        let mut sim = Sim::new(0);
        let before = sim.city_stats();
        // 住宅の内訳の 4 つ目が税で、`[要因の番号, 寄与]` の寄与は 1 つ先にある。
        let tax_at = CITY_STATS_FIXED + 1 + 2 * 3;
        assert_eq!(before[tax_at], f64::from(DemandFactorKind::Tax as u8));
        assert_eq!(before[tax_at + 1], 0.0);
        sim.submit(Command::SetTaxRate {
            zone: ZoneKind::Residential,
            permille: 150,
        })
        .unwrap();
        let after = sim.city_stats();
        assert_eq!(after[tax_at + 1], -25.0);
        assert_eq!(after[3], before[3] - 25.0);
        assert_ne!(before, after);
    }

    /// 電気と水道の分を使い回しても、毎回求め直した値と同じである(道路や施設を足したあとも)。
    #[test]
    fn cached_utility_numbers_match_a_fresh_computation() {
        let fresh = |sim: &Sim| -> Vec<f64> {
            let [power, water, power_short, water_short] =
                crate::layers::utility_totals(sim.world());
            let need: u64 = sim
                .world()
                .buildings()
                .iter()
                .map(|(_, b)| building_demand(b))
                .sum();
            [power, water, need, power_short, water_short]
                .into_iter()
                .chain(crate::layers::utility_gaps(sim.world()))
                .chain(crate::layers::utility_main(sim.world()))
                .map(|v| v as f64)
                .collect()
        };
        let mut sim = Sim::new(0);
        sim.submit(road((0, 5), (10, 5))).unwrap();
        for step in 0..6 {
            assert_eq!(sim.city_stats()[7..21], fresh(&sim)[..], "{step}");
            // 2 回目は使い回しで、同じ値になる。
            assert_eq!(sim.city_stats()[7..21], fresh(&sim)[..], "{step}");
            let kind = if step % 2 == 0 {
                ServiceKind::PowerPlant
            } else {
                ServiceKind::WaterPump
            };
            sim.submit(Command::PlaceBuilding {
                kind,
                at: TilePos::new(2 + 4 * step, 9),
                rotation: 2,
            })
            .ok();
            sim.advance(200);
        }
    }

    const STATS_LAYOUT_FILE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../web/src/protocol/city-stats-layout.json"
    );

    #[test]
    fn city_stats_layout_file_matches() {
        let expected = city_stats_layout_json();
        if std::env::var_os("CHIZU_UPDATE_LAYOUT").is_some() {
            std::fs::write(STATS_LAYOUT_FILE, &expected).unwrap();
        }
        let actual = std::fs::read_to_string(STATS_LAYOUT_FILE).unwrap();
        assert!(
            actual == expected,
            "{STATS_LAYOUT_FILE} が Rust 側の並びと食い違う。\
             CHIZU_UPDATE_LAYOUT=1 cargo test -p chizu-wasm で書き直す"
        );
    }

    fn road(from: (i64, i64), to: (i64, i64)) -> Command {
        Command::BuildRoad {
            from: TilePos::new(from.0, from.1),
            to: TilePos::new(to.0, to.1),
            kind: RoadKind::Small,
        }
    }

    #[test]
    fn signal_lights_list_the_arms_of_each_signal() {
        let mut sim = Sim::new(0);
        sim.submit(road((0, 5), (10, 5))).unwrap();
        sim.submit(road((5, 5), (5, 10))).unwrap();
        assert!(sim.signal_lights().is_empty());
        let node = sim.world().network().node_at(TilePos::new(5, 5)).unwrap();
        sim.submit(Command::SetSignal { node, on: true }).unwrap();
        let lights = sim.signal_lights();
        // T 字路の 3 本の腕。向き合う東西の腕は同じ灯火になり、南の腕とは同時に青にならない。
        assert_eq!(lights.len(), 3 * 5);
        let arms: Vec<&[f64]> = lights.chunks(5).collect();
        for arm in &arms {
            assert_eq!(&arm[..2], &[5.0, 5.0]);
        }
        let light = |dx: f64, dy: f64| {
            arms.iter()
                .find(|a| a[2] == dx && a[3] == dy)
                .map(|a| a[4])
                .unwrap()
        };
        assert_eq!(light(1.0, 0.0), light(-1.0, 0.0));
        assert!(light(1.0, 0.0) != 0.0 || light(0.0, 1.0) != 0.0);
    }

    #[test]
    fn reports_outcomes_and_network_version() {
        let mut sim = Sim::new(0);
        let money = sim.world().money();
        sim.submit(road((0, 0), (4, 0))).unwrap();
        sim.submit(road((0, 0), (4, 0))).unwrap();
        sim.advance(1);
        let reports = sim.take_reports();
        assert_eq!(
            reports
                .iter()
                .map(|r| (r.seq, r.outcome))
                .collect::<Vec<_>>(),
            [
                (0, ApplyOutcome::Applied),
                (1, ApplyOutcome::Denied(DenyReason::Overlap))
            ]
        );
        assert!(sim.take_reports().is_empty());
        let version = sim.network_version();
        assert!(sim.world().money() < money);
        // 外部接続の高速道路は最初からあるので、引いた道路はそのあとに並ぶ
        let segments = sim.road_segments();
        assert_eq!(segments[4], 4.0);
        assert_eq!(segments[segments.len() - 5..], [0.0, 0.0, 4.0, 0.0, 1.0]);
        let nodes = sim.road_nodes();
        assert_eq!(
            nodes[nodes.len() - 10..],
            [0.0, 0.0, 1.0, 0.0, 0.0, 4.0, 0.0, 1.0, 0.0, 0.0]
        );

        let far = (CHUNK_SIDE as i64) * 3;
        sim.submit(road((far, 0), (far + 2, 0))).unwrap();
        sim.submit(Command::Bulldoze {
            area: TileRect::from_corners(TilePos::new(-5, -5), TilePos::new(-4, -4)),
        })
        .unwrap();
        sim.advance(1);
        let outcomes: Vec<_> = sim.take_reports().iter().map(|r| r.outcome).collect();
        assert_eq!(
            outcomes,
            [
                ApplyOutcome::Denied(DenyReason::NotOwned),
                ApplyOutcome::Denied(DenyReason::NothingToBulldoze)
            ]
        );
        assert_eq!(
            sim.network_version(),
            version,
            "断られたコマンドでは版が変わらない"
        );
    }

    #[test]
    fn advance_matches_replay() {
        let mut sim = Sim::new(3);
        sim.submit(road((0, 0), (6, 6))).unwrap();
        sim.advance(2);
        sim.submit(road((0, 6), (6, 0))).unwrap();
        sim.submit(road((3, 0), (3, 5))).unwrap();
        sim.submit(Command::Bulldoze {
            area: TileRect::from_corners(TilePos::new(3, 4), TilePos::new(3, 5)),
        })
        .unwrap();
        sim.advance(1);
        // 値段は 50,000 から 25,000 ずつ上がるので、4 つ買うと 350,000 使う。
        // 道路の費用を引いた残りは 150,000 に届かず、5 つめは所持金が足りない
        for (cx, cy) in [(2, 0), (3, 0), (-2, 0), (0, 2), (0, -2)] {
            sim.submit(Command::BuyChunk {
                coord: ChunkCoord::new(cx, cy),
            })
            .unwrap();
        }
        sim.advance(3);
        let outcomes: Vec<_> = sim.take_reports().iter().map(|r| r.outcome).collect();
        assert_eq!(
            outcomes,
            [
                [ApplyOutcome::Applied; 8].as_slice(),
                &[ApplyOutcome::Denied(DenyReason::NotEnoughMoney)]
            ]
            .concat()
        );
        let mut world = World::new(3);
        chizu_core::replay(&mut world, sim.log(), 6);
        assert_eq!(world.state_hash(), sim.world().state_hash());
    }

    #[test]
    fn paused_sim_does_not_advance() {
        let mut sim = Sim::new(0);
        sim.set_paused(true);
        sim.advance(5);
        assert_eq!(sim.world().tick(), Tick::ZERO);
        sim.set_paused(false);
        sim.advance(5);
        assert_eq!(sim.world().tick(), Tick(5));
    }

    #[test]
    fn dev_walks_mark_unrecorded_ops_and_refresh_movers() {
        let mut sim = Sim::new(1);
        sim.submit(Command::BuildRoad {
            from: TilePos::new(0, 10),
            to: TilePos::new(30, 10),
            kind: RoadKind::Small,
        })
        .unwrap();
        sim.set_mover_view(Some(TileRect {
            min: TilePos::new(-64, -64),
            max: TilePos::new(127, 127),
        }));
        sim.publish();
        assert!(sim.movers.is_empty());
        assert!(!sim.has_unrecorded_ops());
        // 止めたまま出しても、tick は変わらないが集め直す。
        sim.set_paused(true);
        assert_eq!(
            sim.spawn_dev_walks(TilePos::new(2, 10), TilePos::new(25, 10), 5),
            5
        );
        assert!(sim.has_unrecorded_ops());
        sim.publish();
        assert_eq!(sim.movers.len(), 5);
        sim.set_mover_view(None);
        sim.publish();
        assert!(sim.movers.is_empty());
    }
}
