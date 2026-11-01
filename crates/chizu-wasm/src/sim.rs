//! Worker が持つシミュレーションの本体。実時間は持たず、進める tick 数は呼び出し側が決める。

use chizu_core::record::RecordError;
use chizu_core::{Command, CommandLog, GameTime, World, replay};

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

#[cfg(test)]
mod tests {
    use super::*;
    use chizu_core::command::MAX_TAX_PERMILLE;
    use chizu_core::{SimSpeed, Tick, ZoneKind};

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
