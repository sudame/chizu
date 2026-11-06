//! Worker が持つシミュレーションの本体。実時間は持たず、進める tick 数は呼び出し側が決める。

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};

use chizu_core::building::{DemandFactorKind, demand_factors};
use chizu_core::record::RecordError;
use chizu_core::service::building_demand;
use chizu_core::world::economy::{chunk_slots, milestone_population};
use chizu_core::world::growth::GROWTH_INTERVAL;
use chizu_core::zone::ZONE_DEPTH;
use chizu_core::{
    ApplyOutcome, CHUNK_AREA, CHUNK_SIDE, ChunkCoord, Command, CommandLog, DenyReason, GameTime,
    Layer, Line, NodeControl, SegmentKind, ServiceKind, StateHasher, TerrainTiles, Tick, TilePos,
    TileRect, TileUse, World, terrain,
};

use crate::snapshot::{Header, Mover, TripleBuffer};

/// [`Sim::overview`] が返す数のうち、固定の並びの個数。そのあとに止まっている道路網の並びが続く。画面の側の `OVERVIEW_LENGTH` と合わせる。
pub const OVERVIEW_LENGTH: usize = 22;

/// [`Sim::overview`] のうち、駐車場の点数の平均の位置。固定の並びの最後である。
#[cfg(test)]
const PARKING_AT: usize = OVERVIEW_LENGTH - 1;

/// [`Sim::carless`] が返す数のうち、固定の並びの個数。そのあとに世帯の多い区切りの並びが続く。画面の側の `CARLESS_FIXED` と合わせる。
pub const CARLESS_FIXED: usize = 6;

/// [`Sim::carless`] が並べる、世帯の多い区切りの数の上限。
pub const CARLESS_AREAS: usize = 10;

/// [`Sim::city_stats`] が返す数のうち、固定の並びの個数。そのあとに需要の内訳が続く。
/// 画面の側は `city-stats-layout.json` から読むので、変えたら `CHIZU_UPDATE_LAYOUT=1 cargo test -p chizu-wasm` で書き直す。
pub const CITY_STATS_FIXED: usize = 27;

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
    /// チャンクごとの、辺の近くの建物と施設のマスの控え。`(チャンクの版, 署名)` で、版が変わったときだけ計算し直す。
    edge_signatures: BTreeMap<ChunkCoord, (u32, u64)>,
    /// 移動体を描く範囲。画面だけが決め、中核には渡さない。`None` なら移動体を書かない。
    mover_view: Option<TileRect>,
    /// 最後に集めた移動体と、その基準のチャンク。
    movers: Vec<Mover>,
    movers_base: ChunkCoord,
    /// 公開のたびに集め直す列車の車両と、その基準のチャンク。範囲によらず全部書く。
    cars: Vec<Mover>,
    cars_base: ChunkCoord,
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
    utility_cache: Cell<Option<(u64, [f64; 15])>>,
    /// 線路と駅の版。線路や駅を変えうるコマンドを受けるたびに上げる。画面向けで、状態には入れない。
    rail_revision: u64,
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
        let mut sim = Sim {
            world,
            log,
            paused: false,
            snapshot: TripleBuffer::new(),
            reports: Vec::new(),
            near_road_changes: BTreeMap::new(),
            mover_view: None,
            movers: Vec::new(),
            movers_base: ChunkCoord::default(),
            cars: Vec::new(),
            cars_base: ChunkCoord::default(),
            movers_before: Vec::new(),
            mover_memory: crate::movers::Memory::default(),
            movers_key: None,
            unrecorded_ops: 0,
            buildings_version: Cell::new(None),
            utility_cache: Cell::new(None),
            rail_revision: 0,
            edge_signatures: BTreeMap::new(),
        };
        sim.note_edge_changes();
        sim
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
        if matches!(
            command,
            Command::BuildRail { .. }
                | Command::Bulldoze { .. }
                | Command::PlaceBuilding { .. }
                | Command::CreateLine { .. }
                | Command::ExtendLine { .. }
                | Command::RemoveLine { .. }
                | Command::SetLineTrains { .. }
        ) {
            self.rail_revision += 1;
        }
        if outcome == ApplyOutcome::Applied {
            self.mark_near_road_changes(&touched);
            // 隣のチャンクを買うと、辺の近くの塗れるマスが変わる。
            if let Command::BuyChunk { coord } = command {
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        let n = ChunkCoord::new(
                            coord.cx.saturating_add(dx),
                            coord.cy.saturating_add(dy),
                        );
                        if (dx, dy) != (0, 0) && self.world.chunk(n).is_some() {
                            let c = self.near_road_changes.entry(n).or_insert(0);
                            *c = c.wrapping_add(1);
                        }
                    }
                }
            }
        }
        self.note_edge_changes();
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
            .then(|| self.world.buildable_mask(TileRect { min, max }, None));
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
        let changes = self.near_road_changes.get(&coord).copied().unwrap_or(0);
        let changes = changes & ((1 << 21) - 1);
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
        self.note_edge_changes();
    }

    /// 辺の近く（`ZONE_DEPTH` マス以内）の建物か施設のマスが変わったチャンクの、隣のチャンクの画面向けの版を上げる。
    /// 区画を塗れるかは隣のチャンクのマスまでたどるので、そのチャンクの版だけでは古い案内が残る。
    /// 道路は `mark_near_road_changes` が見るので、ここでは建物と施設のマスだけを見る。
    fn note_edge_changes(&mut self) {
        let side = CHUNK_SIDE;
        let depth = ZONE_DEPTH as usize;
        let mut changed = Vec::new();
        for chunk in self.world.chunks() {
            let coord = chunk.coord();
            let version = chunk.version();
            if self.edge_signatures.get(&coord).map(|e| e.0) == Some(version) {
                continue;
            }
            let Some(layer) = chunk.layer(Layer::Ground) else {
                continue;
            };
            let mut sig = 0_u64;
            for (i, u) in layer.occupancy.iter().enumerate() {
                let (x, y) = (i % side, i / side);
                let edge = x < depth || y < depth || x >= side - depth || y >= side - depth;
                if edge && matches!(u, TileUse::Building | TileUse::Facility) {
                    sig = sig
                        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                        .wrapping_add((i as u64) << 2 | *u as u64);
                }
            }
            let before = self.edge_signatures.insert(coord, (version, sig));
            if before.is_some_and(|b| b.1 != sig) {
                changed.push(coord);
            }
        }
        for coord in changed {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let n =
                        ChunkCoord::new(coord.cx.saturating_add(dx), coord.cy.saturating_add(dy));
                    if (dx, dy) != (0, 0) && self.world.chunk(n).is_some() {
                        let c = self.near_road_changes.entry(n).or_insert(0);
                        *c = c.wrapping_add(1);
                    }
                }
            }
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
        h.write(self.world.rail());
        (h.finish() & ((1 << 52) - 1)) as f64
    }

    /// これまでに進めた tick のうち、建物の表が変わりうる tick の数。`GROWTH_INTERVAL` で割った余りが
    /// 0（建つ・育つ）か 1（供給の判定し直し）の tick を数える。
    fn growth_runs(&self) -> u64 {
        let ticks = self.world.tick().0;
        ticks.div_ceil(GROWTH_INTERVAL) + ticks.saturating_sub(1).div_ceil(GROWTH_INTERVAL)
    }

    /// 建物を、1 つにつき `[min の x, min の y, max の x, max の y, 区画の種類, 段階, 供給]` の 7 つの数で返す。
    /// 区画の種類は `Zone` の番号（高密度の住宅は 1、低密度の住宅は 5）、供給は電気が届いていれば 1、水道が届いていれば 2 を足した値である。
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

    /// 切り欠いた駐車場の形を、切り欠く建物の分と、共同の駐車場の分だけ返す。並びは [`crate::lots::encode`] にある。
    /// 間口のどちらの端に寄せるかは道路網で変わるので、版は建物の表と道路網の両方で決まる。
    pub fn lots(&self) -> Vec<f64> {
        let net = self.world.network();
        let mut out = crate::lots::encode(
            net,
            self.world.seed(),
            self.world.buildings().iter().map(|(_, b)| b),
        );
        out.extend(crate::lots::encode_facilities(
            net,
            self.world.facilities().iter(),
        ));
        out
    }

    /// 停めてある車の台数を、駐車場のある建物の分だけ返す。並びは [`crate::lots::encode_parked`] にある。
    /// 台数は毎 tick 変わるので、画面へは三重バッファの外で、およそ 1 秒に 1 回送る。
    pub fn parked(&self) -> Vec<f64> {
        crate::lots::encode_parked(&self.world)
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

    /// 長方形を取り壊したときに消えるものを、状態を変えずに求める。範囲が世界の外なら空。
    ///
    /// 先頭から、消える道路のマスの数、施設の数、建物の数、家をなくす世帯の数、道路の区間の数、
    /// 線路のマスの数、駅の数、線路の数が続き、区間ごとに `[始点の x, 始点の y, 終点の x, 終点の y]`、
    /// 施設ごとに `[min の x, min の y, max の x, max の y]`、建物ごとと駅ごとに同じ 4 つ、
    /// 線路ごとに区間と同じ 4 つが、この順に並ぶ。その後ろに、消える道路のマスごとの `[x, y]` と、
    /// 消える線路のマスごとの `[x, y]` が続く（ほかの区間や線路が残って通るマスは入らない）。
    pub fn bulldoze_preview(&self, a: TilePos, b: TilePos) -> Vec<f64> {
        let area = chizu_core::TileRect::from_corners(a, b);
        if !area.in_world() {
            return Vec::new();
        }
        let p = self.world.bulldoze_preview(area);
        let mut out = vec![
            p.road_tiles as f64,
            p.facilities.len() as f64,
            p.buildings.len() as f64,
            p.displaced_households as f64,
            p.segments.len() as f64,
            p.rail_tiles as f64,
            p.stations.len() as f64,
            p.tracks.len() as f64,
        ];
        for l in &p.segments {
            let (s, e) = (l.start, l.end());
            out.extend([s.x, s.y, e.x, e.y].map(|v| v as f64));
        }
        for r in p.facilities.iter().chain(&p.buildings).chain(&p.stations) {
            out.extend([r.min.x, r.min.y, r.max.x, r.max.y].map(|v| v as f64));
        }
        for l in &p.tracks {
            let (s, e) = (l.start, l.end());
            out.extend([s.x, s.y, e.x, e.y].map(|v| v as f64));
        }
        for t in p.removed_road_tiles.iter().chain(&p.removed_rail_tiles) {
            out.extend([t.x, t.y].map(|v| v as f64));
        }
        out
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
        // 駅は施設の一覧に入らないが、画面では施設の記号と名前で示すので、続けて並べる
        for (_, st) in self.world.rail().stations() {
            let a = st.area;
            out.extend([a.min.x, a.min.y, a.max.x, a.max.y].map(|v| v as f64));
            out.push(f64::from(ServiceKind::Station as u8));
        }
        out
    }

    /// 線路と駅の版。画面は、この値が変わったときだけ線路と駅の形を受け取り直す。
    pub fn rail_revision(&self) -> u64 {
        self.rail_revision
    }

    /// 線路を、1 本につき `[a の x, a の y, b の x, b の y, 駅の線路か（0 か 1）]` の 5 つの数で返す。
    /// 座標は端のノードのマスで、線路は複線の 1 組である。
    pub fn rail_tracks(&self) -> Vec<f64> {
        let rail = self.world.rail();
        let mut out = Vec::new();
        for (id, track) in rail.tracks() {
            let line = rail.track_line(id).expect("線路の直線がない");
            let end = line.end();
            out.extend([line.start.x, line.start.y, end.x, end.y].map(|v| v as f64));
            out.push(f64::from(u8::from(track.station.is_some())));
        }
        out
    }

    /// 駅を、1 つにつき `[min の x, min の y, max の x, max の y, 道路へ向かう 1 歩の x, y]` の 6 つの数で返す。
    pub fn rail_stations(&self) -> Vec<f64> {
        let mut out = Vec::new();
        for (_, st) in self.world.rail().stations() {
            let a = st.area;
            out.extend([a.min.x, a.min.y, a.max.x, a.max.y].map(|v| v as f64));
            out.extend([st.toward_road.0, st.toward_road.1].map(|v| v as f64));
        }
        out
    }

    /// 路線を、1 路線につき
    /// `[添え字, 世代, 環状か, 決めた本数, 本数の上限, 走らせる本数, 止まっているか, 運転間隔(tick。なければ 0), 1 周の見込み(tick),
    /// 駅の数, 線路の数, 待てる所の数, 理由に挙げる路線の数]` の 13 個の数に、駅ごとの `[敷地の min の x, y]` と、
    /// 線路ごとの `[a の x, y, b の x, y]` と、理由に挙げる路線ごとの `[添え字, 世代]` を続けて返す。
    /// 理由に挙げる路線は、上限が 0 の路線が進路を共有するほかの路線である。上限が 0 でなければ 0 本にする。
    /// 線路は進路に出てくるものを、重ならないよう最初に出てきた順に並べる。駅は路線の順である。
    /// 路線の添え字と世代は、路線のコマンドで路線を指すのにそのまま使う。
    pub fn rail_lines(&self) -> Vec<f64> {
        let rail = self.world.rail();
        let mut out = Vec::new();
        for (id, line) in rail.lines() {
            let mut seen = BTreeSet::new();
            let mut tracks = Vec::new();
            for step in &line.path {
                if seen.insert(step.track)
                    && let Some(l) = rail.track_line(step.track)
                {
                    let e = l.end();
                    tracks.push([l.start.x, l.start.y, e.x, e.y]);
                }
            }
            let stations: Vec<[i64; 2]> = line
                .stations
                .iter()
                .filter_map(|s| rail.station(*s))
                .map(|st| [st.area.min.x, st.area.min.y])
                .collect();
            let capacity = rail.line_capacity(id).expect("路線がある");
            let shared: &[chizu_core::LineId] = if capacity.cap == 0 {
                &capacity.shared_with
            } else {
                &[]
            };
            out.extend([
                f64::from(id.index),
                f64::from(id.generation),
                f64::from(u8::from(line.looped)),
                f64::from(line.trains),
                f64::from(line.cap()),
                f64::from(line.allowed()),
                f64::from(u8::from(line.stopped)),
                line.headway_ticks().unwrap_or(0) as f64,
                line.round_trip_ticks() as f64,
                stations.len() as f64,
                tracks.len() as f64,
                capacity.wait_places as f64,
                shared.len() as f64,
            ]);
            out.extend(stations.iter().flatten().map(|v| *v as f64));
            out.extend(tracks.iter().flatten().map(|v| *v as f64));
            out.extend(
                shared
                    .iter()
                    .flat_map(|l| [f64::from(l.index), f64::from(l.generation)]),
            );
        }
        out
    }

    /// 線路を `from` から `to` に引けるかを、状態を変えずに調べる。引けないときは理由の文を、
    /// 引けるなら `None` を返し、どちらも建設費を添える。直線の形が正しくないときの建設費は 0 である。
    pub fn rail_probe(&self, from: TilePos, to: TilePos) -> (Option<&'static str>, i64) {
        let Ok(line) = Line::between(from, to) else {
            return (Some("線路は縦横か 45 度の直線で、1,024 マスまで引ける"), 0);
        };
        let cost = chizu_core::rail::rail_cost(&line).0;
        match self.world.check_build_rail(&line) {
            Ok(_) => (None, cost),
            Err(reason) => (Some(deny_text(reason)), cost),
        }
    }

    /// 街の数字を次の 26 個の数で返す。建物の数、世帯の上限、雇える職場の数（税率が高いと建物の上限より少ない）、
    /// 住宅・商業・工業・オフィスの需要、電気の供給量、水道の供給量、電気と水道の需要、
    /// 電気と水道のそれぞれで供給量が需要に足りない網の数、
    /// 電気と水道のそれぞれで施設のない網の数、電気と水道のそれぞれの不足量、
    /// 主の網（需要がいちばん大きい網）の電気の供給量、水道の供給量、需要、
    /// 主の網以外で施設か建物のある網の数（電気、水道の順）、
    /// 主の網の電気と水道の余りが住民の何人分か（足りないときは負）、その人数が決まるか（決まれば 1、人口か需要が 0 なら 0）、
    /// 発電所 1 基と浄水場 1 基で増える住民の数（決まらないときは 0）。
    /// 人数の求め方は `chizu_core::service::headroom_people` と `facility_people` にある。建物がまだなく主の網がないうちは、施設の供給量の合計で見る。
    /// 供給量は道路網につながった施設の合計である。余りは網をまたいで足さず、主の網で見る。
    /// そのあとに、主の網を見分ける印（`chizu_core::service::SupplyBalance::network_mark`。主の網がなければ 0）が 1 個、
    /// 続けて需要の内訳が続く。区画の種類ごとに、要因の数と、要因ごとの `[要因の番号, 寄与]` を並べる。
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
        let utility = self.utility_numbers();
        out.extend(&utility[..14]);
        out.extend(self.headroom_people_numbers(&utility));
        // 最後に、主の網を見分ける印。画面が、需要の控えを別の網のものと比べないために使う
        out.push(utility[14]);
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

    /// 街の数字のうち、余りが住民の何人分かの 3 個と、施設 1 基で増える人数の 2 個。`utility` は [`Sim::utility_numbers`] の値である。
    /// 住民の数は建物の表が変わらなくても動くので、キャッシュの外で毎回求める。
    fn headroom_people_numbers(&self, utility: &[f64; 15]) -> [f64; 5] {
        let [
            power,
            water,
            need,
            ..,
            main_power,
            main_water,
            main_need,
            _,
            _,
            _,
        ] = *utility;
        let (power, water, demand) = if main_need > 0.0 {
            (main_power, main_water, main_need)
        } else {
            (power, water, need)
        };
        let citizens = self.world.housed_citizens();
        let people = |capacity: f64| {
            chizu_core::service::headroom_people(
                capacity as i128 - demand as i128,
                need as u64,
                citizens,
            )
        };
        let per_facility =
            |kind: ServiceKind| chizu_core::service::facility_people(kind, need as u64, citizens);
        match (
            people(power),
            people(water),
            per_facility(ServiceKind::PowerPlant),
            per_facility(ServiceKind::WaterPump),
        ) {
            (Some(p), Some(w), Some(pp), Some(wp)) => {
                [p as f64, w as f64, 1.0, pp as f64, wp as f64]
            }
            _ => [0.0; 5],
        }
    }

    /// 街の数字のうち、電気と水道の 14 個と、主の網を見分ける印 1 個（最後）。電気の供給量、水道の供給量、需要、足りない網の数（電気、水道）、
    /// 施設のない網の数と不足量、主の網の値と、ほかの網の数。
    /// 道路と建物の全体をたどって求めるので、建物の表と道路網の版（[`crate::layers::supply_version`]）が
    /// 変わらないあいだは前の値を使い回す。
    fn utility_numbers(&self) -> [f64; 15] {
        let version = crate::layers::supply_version(self);
        if let Some((cached, numbers)) = self.utility_cache.get()
            && cached == version
        {
            return numbers;
        }
        let (totals, main, gaps, mark) = crate::layers::utility_summary(&self.world);
        let [power, water, power_short, water_short] = totals;
        let need: u64 = self
            .world
            .buildings()
            .iter()
            .map(|(_, b)| building_demand(b))
            .sum();
        let mut out = [0.0; 15];
        let values = [power, water, need, power_short, water_short]
            .into_iter()
            .chain(gaps)
            .chain(main)
            .chain([mark]);
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

    /// 駐車場が足りない建物ごとに `[建物が占めるマスの最小の x, y, 区画の種類, 満車で車をやめた数]`。
    /// 区画の種類は住宅・商業・工業・オフィスの順の番号、数は今日と前の日の和で、
    /// それが `chizu_core::SHORTAGE_LOTS_FULL` 以上の建物だけを出す。建物の番号の順に並ぶ。
    /// 状態は変えない。
    pub fn parking_shortage(&self) -> Vec<f64> {
        let day = self.world.tick().0 / chizu_core::TICKS_PER_DAY;
        let mut out = Vec::new();
        for (id, m) in self.world.trips().lot_misses(day) {
            let full = m.lots_full();
            let Some(b) = self.world.buildings().get(id) else {
                continue;
            };
            if full >= chizu_core::SHORTAGE_LOTS_FULL {
                out.extend([
                    b.area.min.x as f64,
                    b.area.min.y as f64,
                    b.zone.kind().index() as f64,
                    f64::from(full),
                ]);
            }
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
    /// 道路の維持費、発電所と浄水場の維持費、共同の駐車場の維持費、共同の駐車場の料金、料金の上限、料金の収入、鉄道の維持費。
    /// 区画の種類の順は住宅、商業、工業、オフィス。金額はどれも 1 日あたりである。
    pub fn finance(&self) -> Vec<f64> {
        let f = self.world.finances();
        let mut out = vec![f64::from(chizu_core::command::MAX_TAX_PERMILLE)];
        out.extend(chizu_core::ZoneKind::ALL.map(|z| f64::from(self.world.tax_permille(z))));
        out.extend(f.taxes.map(|m| m.0 as f64));
        out.push(f.road_upkeep.0 as f64);
        out.extend(f.facility_upkeep.map(|m| m.0 as f64));
        out.push(f.parking_upkeep.0 as f64);
        out.push(f64::from(self.world.parking_fee()));
        out.push(f64::from(chizu_core::command::MAX_PARKING_FEE));
        out.push(f.parking_fees.0 as f64);
        out.push(f.rail_upkeep.0 as f64);
        out
    }

    /// 車を持てない世帯を、次の数で返す。家のある世帯のうち、車を置く枠のない世帯である。
    /// 世帯の数、理由ごとの数（範囲に駐車場がない、満車、料金が上限を超える、借りるのを待っている、道路に面していない）が [`CARLESS_FIXED`] 個、
    /// そのあとに、世帯の多い区切り（8×8 マス）の上位 [`CARLESS_AREAS`] 件ぶんの `[真ん中のマスの x, y, 世帯の数]` が続く。
    /// 状態は変えない。
    pub fn carless(&self) -> Vec<f64> {
        let all = self.world.carless_households();
        let mut by_reason = [0_u64; chizu_core::CarlessReason::ALL.len()];
        for c in &all {
            by_reason[c.reason.index()] += 1;
        }
        let mut out = vec![all.len() as f64];
        out.extend(by_reason.map(|n| n as f64));
        debug_assert_eq!(out.len(), CARLESS_FIXED);
        for a in chizu_core::carless_areas(&all, CARLESS_AREAS) {
            let center = a.center();
            out.extend([center.x as f64, center.y as f64, f64::from(a.households)]);
        }
        out
    }

    /// 家の正面のマスが `at` から [`chizu_core::car_place::SHARED_LOT_RADIUS`] マスの中にある、車を持てない世帯の数。
    /// 共同の駐車場を `at` に置く前に、届く範囲の中の世帯を見せるのに使う。状態は変えない。
    pub fn carless_in_range(&self, at: TilePos) -> f64 {
        let r = i128::from(chizu_core::car_place::SHARED_LOT_RADIUS);
        let near = |c: &chizu_core::Carless| {
            let d = |p: i64, q: i64| i128::from(p) - i128::from(q);
            let (dx, dy) = (d(c.front.x, at.x), d(c.front.y, at.y));
            dx * dx + dy * dy <= r * r
        };
        self.world
            .carless_households()
            .iter()
            .filter(|c| near(c))
            .count() as f64
    }

    /// 人口と幸福度とマイルストーンを、次の [`OVERVIEW_LENGTH`] 個の数で返す。
    /// 家のある住民の数、幸福度の平均、通勤・生活サービス・暮らし向きの平均、届いた段階、
    /// 次の段階に届く人口、いまの段階までで買える枠の数、次の段階で買える枠の数。
    /// そのあとに、住宅・商業・工業・オフィスの順で、区画ごとに
    /// 電気か水道の余裕がなくて新築が止まっている間口の数、そのうち電気、水道の余裕がない間口の数を並べる。
    /// 最後に、駐車場の点数の平均を置く（あとから足した要素なので、ほかの要素の位置を変えないよう末尾にした）。
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
        out.push(f64::from(parts.parking));
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
        self.cars_base = crate::trains::collect_cars(&self.world, &mut self.cars);
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
            train_base_cx: self.cars_base.cx,
            train_base_cy: self.cars_base.cy,
        };
        self.snapshot.publish(&header, &self.movers, &self.cars);
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
        DenyReason::NoZonableTile => "道路に面した所から続く、区画を塗れるマスがない",
        DenyReason::ZoneUnchanged => "変わるマスがない",
        DenyReason::Occupied => "ほかの施設、道路、建物がある",
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
        DenyReason::RailTooSharp => "線路はノードで 45 度までしか曲げられない",
        DenyReason::RailTooManyBranches => "線路の分岐は 1 つのノードに 1 つまで",
        DenyReason::RailNoRoute => "途中で向きを変えずに線路で届かない",
        DenyReason::NoSuchStation => "その駅はない",
        DenyReason::NoSuchLine => "その路線はない",
        DenyReason::LineLimit => "路線の数か、路線の駅の数が上限に届いている",
        DenyReason::StationInLine => "その駅はもう路線に入っている",
        DenyReason::LineStarvesOther => {
            "ほかの路線と分ける待てる所の枠が足りず、その路線の列車が走れなくなる"
        }
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
/// 区画の種類（`Zone` の値。低密度の住宅は 5。なければ 0）を置くビットの位置。3 ビットを使う。
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
    use chizu_core::{Chunk, RoadKind, SimSpeed, Zone, ZoneKind};

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
        // 水辺のあるチャンクを買い、水辺のマスのすぐ南に道路を引く。道路の脇にある水辺のマスは塗れず、
        // その隣の水辺でないマスは塗れる
        let east = ChunkCoord::new(2, 0);
        let (mut sim, water) = (0..256)
            .find_map(|seed| {
                let sim = Sim::new(seed);
                let tiles = sim.chunk_tiles(east);
                let is_water = |x: usize, y: usize| tiles[y * CHUNK_SIDE + x] & TILE_WATER != 0;
                let at = (2..CHUNK_SIDE - 3)
                    .flat_map(|x| (0..CHUNK_SIDE - 1).map(move |y| (x, y)))
                    .find(|&(x, y)| {
                        is_water(x, y)
                            && !is_water(x + 1, y)
                            && (x - 2..=x + 2).all(|rx| !is_water(rx, y + 1))
                    })?;
                Some((seed, (128 + at.0 as i64, at.1 as i64)))
            })
            .map(|(seed, at)| {
                let mut sim = Sim::new(seed);
                sim.submit(Command::BuyChunk { coord: east }).unwrap();
                sim.submit(road((at.0 - 2, at.1 + 1), (at.0 + 2, at.1 + 1)))
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
    fn buying_a_chunk_bumps_the_tiles_version_of_its_neighbours() {
        let mut sim = Sim::new(0);
        let (near, far) = (ChunkCoord::new(1, 0), ChunkCoord::new(-1, -1));
        let east = ChunkCoord::new(2, 0);
        let (v_near, v_far) = (sim.chunk_tiles_version(near), sim.chunk_tiles_version(far));
        sim.submit(Command::BuyChunk { coord: east }).unwrap();
        assert!(sim.world().chunk(east).is_some());
        assert!(sim.chunk_tiles_version(near) > v_near);
        assert_eq!(sim.chunk_tiles_version(far), v_far);
    }

    #[test]
    fn a_facility_near_the_edge_bumps_the_tiles_version_of_the_next_chunk() {
        let mut sim = Sim::new(0);
        sim.submit(road((100, 30), (127, 30))).unwrap();
        let (next, far) = (ChunkCoord::new(1, 0), ChunkCoord::new(-1, 0));
        let (v_next, v_far) = (sim.chunk_tiles_version(next), sim.chunk_tiles_version(far));
        sim.submit(Command::PlaceBuilding {
            kind: chizu_core::ServiceKind::PowerPlant,
            at: TilePos::new(122, 29),
            rotation: 2,
        })
        .unwrap();
        assert!(sim.world().chunk(next).is_some());
        assert!(
            sim.take_reports()
                .iter()
                .all(|r| r.outcome == ApplyOutcome::Applied)
        );
        assert!(sim.chunk_tiles_version(next) > v_next);
        assert_eq!(sim.chunk_tiles_version(far), v_far);
    }

    #[test]
    fn buildings_send_the_zone_code_of_each_density() {
        for (zone, code) in [(Zone::Residential, 1.0), (Zone::ResidentialLow, 5.0)] {
            let mut sim = Sim::new(0);
            sim.submit(road((10, 10), (30, 10))).unwrap();
            sim.submit(Command::PaintZone {
                area: TileRect::from_corners(TilePos::new(10, 11), TilePos::new(30, 16)),
                zone: Some(zone),
            })
            .unwrap();
            sim.advance(chizu_core::world::growth::GROWTH_INTERVAL as u32 + 1);
            let buildings = sim.buildings();
            assert!(!buildings.is_empty(), "{zone:?} の家が建たない");
            for b in buildings.chunks(7) {
                assert_eq!(b[4], code, "{zone:?}");
            }
        }
    }

    #[test]
    fn buildings_and_city_stats_follow_growth() {
        let mut sim = Sim::new(0);
        sim.submit(road((10, 10), (30, 10))).unwrap();
        sim.submit(Command::PaintZone {
            area: TileRect::from_corners(TilePos::new(10, 11), TilePos::new(30, 16)),
            zone: Some(Zone::Residential),
        })
        .unwrap();
        let version = sim.buildings_version();
        assert!(sim.buildings().is_empty());
        assert_eq!(sim.city_stats()[..3], [0.0, 0.0, 0.0]);
        // 人口がないうちは人数が決まらない
        assert_eq!(sim.city_stats()[21..26], [0.0; 5]);
        assert_eq!(sim.city_stats()[26], 0.0, "主の網がないので印もない");
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
            zone: Some(Zone::Residential),
        })
        .unwrap();
        sim.advance(chizu_core::world::growth::GROWTH_INTERVAL as u32 + 1);
        let o = sim.overview();
        let [blocked, no_power, no_water] = [o[9], o[10], o[11]];
        assert!(blocked > 0.0, "{o:?}");
        assert_eq!([no_power, no_water], [blocked, blocked], "{o:?}");
        assert!(o[12..PARKING_AT].iter().all(|v| *v == 0.0), "{o:?}");
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
            zone: Some(Zone::Residential),
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

    #[test]
    fn a_new_city_has_no_parking_shortage() {
        assert!(Sim::new(0).parking_shortage().is_empty());
    }

    /// つながっていない通りの発電所は、孤立した施設として出る。余りは主の網だけで数え、ほかの網に分ける。
    #[test]
    fn isolated_facilities_and_main_network_numbers_are_reported_apart() {
        let mut sim = Sim::new(0);
        assert!(sim.isolated_facilities().is_empty());
        sim.submit(road((10, 10), (30, 10))).unwrap();
        // 住民が入るよう、高速道路の端から主の網につなぐ
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
        sim.submit(Command::PaintZone {
            area: TileRect::from_corners(TilePos::new(10, 11), TilePos::new(30, 16)),
            zone: Some(Zone::Residential),
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
        sim.advance(
            chizu_core::TICKS_PER_DAY as u32
                + chizu_core::world::growth::GROWTH_INTERVAL as u32 * 3,
        );
        let stats = sim.city_stats();
        assert!(stats[9] > 0.0, "{stats:?}");
        assert!(stats[26] > 0.0, "主の網の印がある: {stats:?}");
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
        // 主の網は電気が 0 で足りない。水道は余りがあるので正、電気は負の人数になる。
        assert!(sim.world().housed_citizens() > 0, "住民が入っていない");
        assert_eq!(stats[23], 1.0, "{stats:?}");
        assert!(stats[21] < 0.0 && stats[22] > 0.0, "{stats:?}");
        // 施設 1 基で増える人数は、供給量を 1 人あたりの需要で割った値で、余りの人数と同じ式である
        let (citizens, need) = (sim.world().housed_citizens(), stats[9] as u64);
        for (at, kind) in [(24, ServiceKind::PowerPlant), (25, ServiceKind::WaterPump)] {
            let want =
                chizu_core::service::headroom_people(i128::from(kind.capacity()), need, citizens)
                    .unwrap();
            assert_eq!(stats[at], want as f64, "{stats:?}");
            assert!(stats[at] > 0.0);
        }
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
            zone: Some(Zone::Residential),
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
        assert!(o[9..PARKING_AT].iter().all(|v| *v == 0.0));
        // 駐車場の点数は、ほかの要素と同じく中立の値から始まる
        assert_eq!(o[PARKING_AT], 50.0);
        assert!(o[OVERVIEW_LENGTH..].is_empty());
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
    fn bulldoze_preview_lists_the_whole_segment_for_one_tile() {
        let mut sim = Sim::new(0);
        sim.submit(road((10, 10), (30, 10))).unwrap();
        let before = sim.world().network().road_tile_count();
        let p = sim.bulldoze_preview(TilePos::new(20, 10), TilePos::new(20, 10));
        // 道路のマス、施設、建物、家をなくす世帯、区間、線路のマス、駅、線路の数の順で、区間は両端のマスで指す
        assert_eq!(&p[..8], &[21.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0]);
        assert_eq!(&p[8..12], &[10.0, 10.0, 30.0, 10.0]);
        // 区間のあとに、消える道路のマスが 21 個 [x, y] で続く
        assert_eq!(p.len(), 12 + 2 * 21);
        assert_eq!(&p[12..16], &[10.0, 10.0, 11.0, 10.0]);
        // 見積もりでは街は変わらず、世界の外は空を返す
        assert_eq!(sim.world().network().road_tile_count(), before);
        let far = TilePos::new(1 << 40, 0);
        assert!(sim.bulldoze_preview(far, far).is_empty());
    }

    #[test]
    fn bulldoze_preview_lists_the_rail_and_the_station() {
        let mut sim = Sim::new(0);
        sim.submit(road((0, 10), (30, 10))).unwrap();
        // 敷地は x 3〜12、y 7〜9 で、駅の線路は y = 8 の x 3〜12 である
        sim.submit(Command::PlaceBuilding {
            kind: ServiceKind::Station,
            at: TilePos::new(12, 9),
            rotation: 2,
        })
        .unwrap();
        sim.submit(Command::BuildRail {
            from: TilePos::new(12, 8),
            to: TilePos::new(20, 8),
        })
        .unwrap();
        assert_eq!(sim.world().rail().tracks().count(), 2);
        let p = sim.bulldoze_preview(TilePos::new(5, 7), TilePos::new(15, 8));
        // 駅と、範囲にかかる駅の線路と外の線路が消える。線路のマスは x 3〜20 の 18 マス
        assert_eq!(&p[..8], &[0.0, 0.0, 0.0, 0.0, 0.0, 18.0, 1.0, 2.0]);
        assert_eq!(&p[8..12], &[3.0, 7.0, 12.0, 9.0]);
        // 線路のマスの一覧が、数と同じ 18 個 [x, y] で続く
        assert_eq!(p.len(), 8 + 4 * 3 + 2 * 18);
    }

    #[test]
    fn rail_probe_gives_the_reason_and_the_cost_without_changing_the_city() {
        let mut sim = Sim::new(0);
        sim.submit(road((0, 10), (30, 10))).unwrap();
        let money = sim.world().money();
        // 空き地に引ける線路は、理由がなく、中核の建設費と同じ額になる
        let (reason, cost) = sim.rail_probe(TilePos::new(2, 4), TilePos::new(12, 4));
        assert_eq!(reason, None);
        assert_eq!(
            cost,
            chizu_core::rail::rail_cost(
                &Line::between(TilePos::new(2, 4), TilePos::new(12, 4)).unwrap()
            )
            .0
        );
        // 道路のマスを通る線路は断る
        let (reason, _) = sim.rail_probe(TilePos::new(5, 6), TilePos::new(5, 14));
        assert_eq!(reason, Some(deny_text(DenyReason::Occupied)));
        // 直線でない指定は形の誤りとして断り、建設費は 0
        let (reason, cost) = sim.rail_probe(TilePos::new(0, 0), TilePos::new(5, 2));
        assert!(reason.is_some());
        assert_eq!(cost, 0);
        assert_eq!(sim.world().money(), money);
        assert_eq!(sim.world().rail().tracks().count(), 0);
    }

    #[test]
    fn rail_lines_carry_the_wait_places_and_no_reason_when_the_cap_is_positive() {
        let world =
            World::new_with_money(0, chizu_core::Money(1_000_000_000)).with_unlimited_land();
        let mut sim = Sim::from_parts(world, CommandLog::new());
        sim.submit(road((0, 10), (70, 10))).unwrap();
        for x in [12, 62] {
            sim.submit(Command::PlaceBuilding {
                kind: ServiceKind::Station,
                at: TilePos::new(x, 9),
                rotation: 2,
            })
            .unwrap();
        }
        sim.submit(Command::BuildRail {
            from: TilePos::new(12, 8),
            to: TilePos::new(53, 8),
        })
        .unwrap();
        let ids: Vec<_> = sim.world().rail().stations().map(|(id, _)| id).collect();
        sim.submit(Command::CreateLine {
            from: ids[0],
            to: ids[1],
        })
        .unwrap();
        let data = sim.rail_lines();
        let (id, line) = {
            let rail = sim.world().rail();
            let (id, line) = rail.lines().next().unwrap();
            (id, line.cap())
        };
        let c = sim.world().rail().line_capacity(id).unwrap();
        assert!(line > 0);
        // 見出しの 12 番目が待てる所の数、13 番目が理由に挙げる路線の数
        assert_eq!(data[11], c.wait_places as f64);
        assert_eq!(data[12], 0.0);
        // 見出し 13 個、駅 2 つ、線路 n 本のあとに理由の路線が続かない
        assert_eq!(data.len(), 13 + 4 + data[10] as usize * 4);
    }

    #[test]
    fn rail_data_follows_the_commands() {
        let mut sim = Sim::new(0);
        sim.submit(road((0, 10), (30, 10))).unwrap();
        let before = sim.rail_revision();
        assert!(sim.rail_tracks().is_empty());
        assert!(sim.rail_stations().is_empty());
        sim.submit(Command::PlaceBuilding {
            kind: ServiceKind::Station,
            at: TilePos::new(12, 9),
            rotation: 2,
        })
        .unwrap();
        sim.submit(Command::BuildRail {
            from: TilePos::new(12, 8),
            to: TilePos::new(20, 8),
        })
        .unwrap();
        assert!(sim.rail_revision() > before);
        // 駅の線路と外の線路の 2 本で、駅の線路だけ印が立つ
        let tracks = sim.rail_tracks();
        assert_eq!(tracks.len(), 10);
        assert_eq!(tracks.iter().skip(4).step_by(5).sum::<f64>(), 1.0);
        // 駅の敷地と道路へ向かう向き
        assert_eq!(sim.rail_stations(), vec![3.0, 7.0, 12.0, 9.0, 0.0, 1.0]);
        // 駅は施設の記号と名前を出すため、施設の並びの最後に添える
        let facilities = sim.facilities();
        assert_eq!(
            &facilities[facilities.len() - 5..],
            &[3.0, 7.0, 12.0, 9.0, 5.0]
        );
        // 線路の維持費が収支の最後に出る
        let finance = sim.finance();
        assert_eq!(finance.len(), 17);
        assert_eq!(finance[16], sim.world().finances().rail_upkeep.0 as f64);
        assert!(finance[16] > 0.0);
    }

    #[test]
    fn the_station_changes_the_buildings_version() {
        let mut sim = Sim::new(0);
        sim.submit(road((0, 10), (30, 10))).unwrap();
        let before = sim.buildings_version();
        sim.submit(Command::PlaceBuilding {
            kind: ServiceKind::Station,
            at: TilePos::new(12, 9),
            rotation: 2,
        })
        .unwrap();
        assert_ne!(sim.buildings_version(), before);
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
            zone: Some(Zone::Residential),
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
            zone: Some(Zone::Residential),
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
            zone: Some(Zone::Commercial),
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

    /// 低密度の住宅のマスは `Zone` の値 5 で送り、高密度の住宅（1）と画面で見分けられる。
    #[test]
    fn low_density_tiles_go_to_the_screen_as_their_own_zone() {
        let mut sim = Sim::new(0);
        let origin = ChunkCoord::new(0, 0);
        sim.submit(road((10, 10), (20, 10))).unwrap();
        sim.submit(Command::PaintZone {
            area: TileRect::from_corners(TilePos::new(12, 11), TilePos::new(14, 12)),
            zone: Some(Zone::ResidentialLow),
        })
        .unwrap();
        sim.submit(Command::PaintZone {
            area: TileRect::from_corners(TilePos::new(16, 11), TilePos::new(18, 12)),
            zone: Some(Zone::Residential),
        })
        .unwrap();
        let tiles = sim.chunk_tiles(origin);
        let code = |x: usize| (tiles[12 * CHUNK_SIDE + x] >> TILE_ZONE_SHIFT) & TILE_ZONE_MASK;
        assert_eq!(code(13), 5);
        assert_eq!(code(17), 1);
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
        assert_eq!(before.len(), 17);
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

    const HOME_COUNT_FILE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../web/src/protocol/home-count-fixture.json"
    );

    /// 画面の側は、低密度の家の棟の数を世帯数の式から求める（`web/src/map/lots.ts` の `homeCount`）。
    /// 中核の `Building::households` と食い違わないよう、大きさと段階ごとの世帯数をここで書き出し、画面のテストが読む。
    #[test]
    fn home_count_fixture_matches_the_core_households() {
        let mut sim = Sim::new(0);
        sim.submit(road((10, 10), (30, 10))).unwrap();
        sim.submit(Command::PaintZone {
            area: TileRect::from_corners(TilePos::new(10, 11), TilePos::new(30, 16)),
            zone: Some(Zone::ResidentialLow),
        })
        .unwrap();
        sim.advance(chizu_core::world::growth::GROWTH_INTERVAL as u32 + 1);
        let template = *sim
            .world()
            .buildings()
            .iter()
            .map(|(_, b)| b)
            .next()
            .expect("低密度の家が建たない");
        let mut rows = Vec::new();
        for w in 1..=4i64 {
            for h in 1..=4i64 {
                for level in 1..=5u8 {
                    let mut b = template;
                    b.zone = Zone::ResidentialLow;
                    b.level = level;
                    b.area = TileRect::from_corners(
                        TilePos::new(100, 100),
                        TilePos::new(100 + w - 1, 100 + h - 1),
                    );
                    rows.push(format!(
                        "  {{ \"w\": {w}, \"h\": {h}, \"level\": {level}, \"households\": {} }}",
                        b.households()
                    ));
                }
            }
        }
        let expected = format!("[\n{}\n]\n", rows.join(",\n"));
        if std::env::var_os("CHIZU_UPDATE_LAYOUT").is_some() {
            std::fs::write(HOME_COUNT_FILE, &expected).unwrap();
        }
        let actual = std::fs::read_to_string(HOME_COUNT_FILE).unwrap();
        assert!(
            actual == expected,
            "{HOME_COUNT_FILE} が中核の世帯数と食い違う。\
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

    /// 車を持てない世帯の数と理由、世帯の多い所が、中核の読み取りと合う。共同の駐車場を置くと、範囲の中の世帯が借りて減る。
    #[test]
    fn carless_counts_match_the_core_and_fall_when_a_lot_is_placed() {
        let (mut sim, _guard) = crate::test_town::carless_town(2);
        let housed = sim.world().carless_households().len();
        assert!(housed > 12, "世帯が足りない: {housed}");
        let out = sim.carless();
        assert_eq!(out[0], housed as f64);
        // 駐車場がないので、理由は範囲に駐車場がないだけ
        assert_eq!(out[1..CARLESS_FIXED], [housed as f64, 0.0, 0.0, 0.0, 0.0]);
        assert!(out.len() > CARLESS_FIXED && (out.len() - CARLESS_FIXED) % 3 == 0);
        let areas = &out[CARLESS_FIXED..];
        assert!(areas.len() / 3 <= CARLESS_AREAS);
        // 区画の数の和は世帯の数以下で、多い順に並ぶ
        let counts: Vec<f64> = areas.chunks(3).map(|a| a[2]).collect();
        assert!(counts.windows(2).all(|w| w[0] >= w[1]), "{counts:?}");
        assert!(counts.iter().sum::<f64>() <= housed as f64);
        // 画面が寄せる先は、世帯のいる区切りの真ん中である
        let lot_at = TilePos::new(24, 9);
        assert_eq!(sim.carless_in_range(lot_at), housed as f64);
        assert_eq!(sim.carless_in_range(TilePos::new(500, 9)), 0.0);

        crate::test_town::place_lot(&mut sim, ServiceKind::ParkingLot, 24);
        // 置いただけでは貸さない。次の移住を見る tick で、範囲の中の世帯が借りる
        assert_eq!(sim.carless()[4], housed as f64, "借りるのを待っている");
        sim.advance(chizu_core::world::people::MIGRATION_INTERVAL as u32);
        let after = sim.carless();
        let rented: u32 = sim.world().car_places().rented_counts().values().sum();
        assert_eq!(rented, 12, "平面の駐車場の 12 台ぶんが借りられる");
        let now = sim.world().carless_households().len();
        assert!(now >= housed - 12, "借りた世帯が減った分より多い");
        assert_eq!(after[0], now as f64);
        assert_eq!(after[2], after[0], "残りは満車");
        assert_eq!(after[1..2], [0.0]);
        assert_eq!(after[4], 0.0);
    }

    #[test]
    fn finance_includes_the_parking_upkeep() {
        let (mut sim, _guard) = crate::test_town::carless_town(0);
        assert_eq!(sim.finance()[12], 0.0);
        crate::test_town::place_lot(&mut sim, ServiceKind::ParkingLot, 24);
        crate::test_town::place_lot(&mut sim, ServiceKind::ParkingGarage, 28);
        sim.advance(1);
        let f = sim.finance();
        let upkeep = ServiceKind::ParkingLot.upkeep_per_day().0
            + ServiceKind::ParkingGarage.upkeep_per_day().0;
        assert_eq!(f[12], upkeep as f64);
        // 発電所と浄水場の欄には入らない
        assert_eq!(f[10], ServiceKind::PowerPlant.upkeep_per_day().0 as f64);
        assert_eq!(f[11], ServiceKind::WaterPump.upkeep_per_day().0 as f64);
        let core = sim.world().finances();
        assert_eq!(core.parking_upkeep.0, upkeep);
    }

    #[test]
    fn finance_includes_the_parking_fee_and_its_income() {
        let (mut sim, _guard) = crate::test_town::carless_town(1);
        let f = sim.finance();
        assert_eq!(f.len(), 17);
        assert_eq!(f[13], f64::from(chizu_core::world::INITIAL_PARKING_FEE));
        assert_eq!(f[14], f64::from(chizu_core::command::MAX_PARKING_FEE));
        assert_eq!(f[15], 0.0);
        crate::test_town::place_lot(&mut sim, ServiceKind::ParkingLot, 24);
        sim.advance(120);
        let rented: u64 = sim
            .world()
            .car_places()
            .rented_counts()
            .values()
            .map(|n| u64::from(*n))
            .sum();
        assert!(rented > 0, "借りた世帯がいない");
        assert_eq!(sim.finance()[15], (rented * 10) as f64);
        sim.submit(Command::SetParkingFee { fee: 30 }).unwrap();
        sim.advance(1);
        assert_eq!(sim.finance()[13], 30.0);
    }
}
