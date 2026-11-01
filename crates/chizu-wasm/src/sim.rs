//! Worker が持つシミュレーションの本体。実時間は持たず、進める tick 数は呼び出し側が決める。

use std::collections::{BTreeMap, BTreeSet};

use chizu_core::record::RecordError;
use chizu_core::zone::{ZONE_DEPTH, near_road};
use chizu_core::{
    ApplyOutcome, CHUNK_AREA, CHUNK_SIDE, ChunkCoord, Command, CommandLog, DenyReason, GameTime,
    Layer, Line, SegmentKind, TerrainTiles, TilePos, TileRect, TileUse, World, terrain,
};

use crate::snapshot::{Header, TripleBuffer};

pub struct Sim {
    world: World,
    log: CommandLog,
    paused: bool,
    snapshot: TripleBuffer,
    /// 適用したが、まだ画面に渡していない結果。
    reports: Vec<Report>,
    /// 道路網が変わって、区画を塗れるマスが変わりうるチャンクに足していく数。画面向けの版に混ぜる。
    near_road_changes: BTreeMap<ChunkCoord, u32>,
}

/// 記録したコマンドを適用した結果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub seq: u64,
    pub outcome: ApplyOutcome,
}

impl Sim {
    pub fn new(seed: u64) -> Sim {
        Sim {
            world: World::new(seed),
            log: CommandLog::new(),
            paused: false,
            snapshot: TripleBuffer::new(),
            reports: Vec::new(),
            near_road_changes: BTreeMap::new(),
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

    /// ノードを、1 つにつき `[x, y, つながる区間の数]` の 3 つの数で返す。
    pub fn road_nodes(&self) -> Vec<f64> {
        let mut out = Vec::new();
        for (_, node) in self.world.network().nodes() {
            let t = node.tile();
            out.extend([t.x as f64, t.y as f64, node.segments.len() as f64]);
        }
        out
    }

    /// いまの状態を三重バッファに書く。
    pub fn publish(&mut self) {
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
        };
        self.snapshot.publish(&header, &[]);
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
        DenyReason::NothingToBulldoze => "範囲に消せる道路がない",
        DenyReason::NotAdjacent => "買った土地に接していない",
        DenyReason::AlreadyOwned => "もう買ってある",
        DenyReason::NoZonableTile => "道路沿いに区画を塗れるマスがない",
        DenyReason::ZoneUnchanged => "変わるマスがない",
        DenyReason::Occupied => "ほかの施設か道路がある",
        DenyReason::NoRoadAccess => "道路に面していない",
        DenyReason::NoChunkSlot => "買える土地の枠がない。人口が増えると枠が増える",
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
    use chizu_core::{Chunk, RoadKind, SimSpeed, Tick, ZoneKind};

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
            tile(&sim, 13, 12) >> TILE_ZONE_SHIFT,
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

    fn road(from: (i64, i64), to: (i64, i64)) -> Command {
        Command::BuildRoad {
            from: TilePos::new(from.0, from.1),
            to: TilePos::new(to.0, to.1),
            kind: RoadKind::Small,
        }
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
        assert_eq!(nodes[nodes.len() - 6..], [0.0, 0.0, 1.0, 4.0, 0.0, 1.0]);

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
}
