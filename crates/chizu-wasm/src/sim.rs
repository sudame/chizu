//! Worker が持つシミュレーションの本体。実時間は持たず、進める tick 数は呼び出し側が決める。

use chizu_core::record::RecordError;
use chizu_core::{
    CHUNK_AREA, CHUNK_SIDE, ChunkCoord, Command, CommandLog, GameTime, TerrainTiles, World, replay,
    terrain,
};

use crate::snapshot::{Header, TripleBuffer};

pub struct Sim {
    world: World,
    log: CommandLog,
    paused: bool,
    snapshot: TripleBuffer,
}

impl Sim {
    pub fn new(seed: u64) -> Sim {
        Sim {
            world: World::new(seed),
            log: CommandLog::new(),
            paused: false,
            snapshot: TripleBuffer::new(),
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
    pub fn advance(&mut self, ticks: u32) {
        if !self.paused {
            replay(&mut self.world, &self.log, u64::from(ticks));
        }
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
    use chizu_core::{Chunk, SimSpeed, Tick, ZoneKind};

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
