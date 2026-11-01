//! Worker が持つシミュレーションの本体。実時間は持たず、進める tick 数は呼び出し側が決める。

use chizu_core::network::road_cost;
use chizu_core::record::RecordError;
use chizu_core::{
    ApplyOutcome, CHUNK_AREA, CHUNK_SIDE, ChunkCoord, Command, CommandLog, GameTime, Line,
    SegmentKind, TerrainTiles, World, terrain,
};

use crate::snapshot::{Header, TripleBuffer};

pub struct Sim {
    world: World,
    log: CommandLog,
    paused: bool,
    snapshot: TripleBuffer,
    /// 適用したが、まだ画面に渡していない結果。
    reports: Vec<Report>,
    /// 道路網を変えたコマンドを適用した回数。画面は、この値が変わったときだけ道路の形を受け取り直す。
    network_revision: u32,
}

/// 記録したコマンドを適用した結果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub seq: u64,
    pub outcome: ApplyOutcome,
    /// `Denied` のときの理由。
    pub reason: Option<&'static str>,
}

impl Sim {
    pub fn new(seed: u64) -> Sim {
        Sim {
            world: World::new(seed),
            log: CommandLog::new(),
            paused: false,
            snapshot: TripleBuffer::new(),
            reports: Vec::new(),
            network_revision: 0,
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

    /// 検証したコマンドを、次に進める tick の頭で適用するものとして記録する。
    pub fn submit(&mut self, command: Command) -> Result<u64, RecordError> {
        command.validate().map_err(RecordError::Invalid)?;
        self.log.record(self.world.tick(), command)
    }

    /// チャンクの版。メモリに置いていないチャンクは、種から作ったばかりのチャンクと同じ 0 を返す。
    pub fn chunk_version(&self, coord: ChunkCoord) -> u32 {
        self.world.chunk(coord).map_or(0, |c| c.version())
    }

    pub fn chunk_owned(&self, coord: ChunkCoord) -> bool {
        self.world.is_owned(coord)
    }

    /// チャンクのマスを 1 マス 1 バイトで、行ごとに並べて返す。ビットの意味は [`TILE_WATER`] にある。
    /// メモリに置いていないチャンクは、種から地形を作って返す。中核の状態は変えない。
    pub fn chunk_tiles(&self, coord: ChunkCoord) -> Vec<u8> {
        match self.world.chunk(coord) {
            Some(chunk) => encode_tiles(chunk.terrain()),
            None => encode_tiles(&terrain(self.world.seed(), coord)),
        }
    }

    /// 一時停止していなければ `ticks` だけ進める。
    ///
    /// 進め方は `chizu_core::replay` と同じで、各 tick の頭にその tick のコマンドを番号順に
    /// 適用してから `step` を呼ぶ。コマンドごとの結果を受け取るため、ここで同じ手順を回す。
    pub fn advance(&mut self, ticks: u32) {
        if self.paused {
            return;
        }
        let entries = self.log.entries();
        let mut next = entries.partition_point(|e| e.tick < self.world.tick());
        for _ in 0..ticks {
            while let Some(e) = entries.get(next)
                && e.tick == self.world.tick()
            {
                let outcome = self.world.apply(&e.command);
                let reason = (outcome == ApplyOutcome::Denied)
                    .then(|| denial_reason(&self.world, &e.command));
                if outcome == ApplyOutcome::Applied
                    && matches!(
                        e.command,
                        Command::BuildRoad { .. } | Command::Bulldoze { .. }
                    )
                {
                    self.network_revision = self.network_revision.wrapping_add(1);
                }
                self.reports.push(Report {
                    seq: e.seq,
                    outcome,
                    reason,
                });
                next += 1;
            }
            self.world.step();
        }
    }

    /// まだ渡していない結果を、適用した順に取り出す。
    pub fn take_reports(&mut self) -> Vec<Report> {
        std::mem::take(&mut self.reports)
    }

    pub fn network_revision(&self) -> u32 {
        self.network_revision
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

/// `Denied` になったコマンドの理由。`Denied` のコマンドは状態を変えないので、適用したあとの状態で調べてよい。
/// 網に引けない理由は中核が外に出さないので、ほかの理由に当たらなければ網の決まりに反したとみなす。
fn denial_reason(world: &World, command: &Command) -> &'static str {
    match *command {
        Command::BuildRoad { from, to, kind } => match Line::between(from, to) {
            Ok(line) if !line.tiles().all(|t| world.is_buildable(t)) => {
                "買っていない土地か水辺を通る"
            }
            Ok(line) if world.money() < road_cost(kind, &line) => "所持金が足りない",
            _ => "ほかの道路と重なるか、途中で交わる",
        },
        Command::Bulldoze { .. } => "範囲に消せる道路がない",
        Command::BuyChunk { coord } if !world.can_buy_chunk(coord) => {
            "買った土地に接していないか、もう買ってある"
        }
        Command::BuyChunk { .. } => "所持金が足りない",
        _ => "いまの状態では受け付けられない",
    }
}

/// 画面に渡すマスのバイトのうち、水辺を表すビット。
pub const TILE_WATER: u8 = 1;

fn encode_tiles(terrain: &TerrainTiles) -> Vec<u8> {
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
    use chizu_core::{Chunk, RoadKind, SimSpeed, Tick, TilePos, TileRect, ZoneKind};

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
    fn command_applies_at_the_head_of_the_next_tick() {
        let mut sim = Sim::new(0);
        sim.advance(2);
        let speed = SimSpeed::new(4).unwrap();
        assert_eq!(sim.submit(Command::SetSimSpeed { speed }), Ok(0));
        assert_eq!(sim.log().entries()[0].tick, Tick(2));
        assert_eq!(sim.world().speed(), SimSpeed::NORMAL);
        sim.advance(1);
        assert_eq!(sim.world().speed(), speed);
        assert_eq!(sim.world().tick(), Tick(3));
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
    fn reports_outcomes_and_counts_network_changes() {
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
            [(0, ApplyOutcome::Applied), (1, ApplyOutcome::Denied)]
        );
        assert_eq!(
            reports[1].reason,
            Some("ほかの道路と重なるか、途中で交わる")
        );
        assert!(sim.take_reports().is_empty());
        assert_eq!(sim.network_revision(), 1);
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
        let reasons: Vec<_> = sim.take_reports().iter().map(|r| r.reason).collect();
        assert_eq!(
            reasons,
            [
                Some("買っていない土地か水辺を通る"),
                Some("範囲に消せる道路がない")
            ]
        );
        assert_eq!(sim.network_revision(), 1);
    }

    #[test]
    fn advance_matches_replay() {
        let mut sim = Sim::new(3);
        sim.submit(road((0, 0), (6, 6))).unwrap();
        sim.advance(2);
        sim.submit(road((0, 6), (6, 0))).unwrap();
        sim.submit(road((3, 0), (3, 5))).unwrap();
        sim.advance(3);
        let mut world = World::new(3);
        chizu_core::replay(&mut world, sim.log(), 5);
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
