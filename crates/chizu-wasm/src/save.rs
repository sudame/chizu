//! ブラウザの中に保存するデータの取り出しと、保存データからの作り直し。
//!
//! 中核は決定論的なので、保存するのは種、tick、コマンドの記録だけにする。
//! 読み込むときは、種から作った街に記録を再生して同じ状態を作り直す。
//! 再生は長くかかりうるので、[`Restore::step`] で少しずつ進め、そのあいだに Worker が進み具合を送れるようにする。

use core::fmt;

use chizu_core::record::FORMAT_VERSION;
use chizu_core::{CommandLog, DecodeError, GameTime, World, replay};
use wasm_bindgen::prelude::*;

use crate::Simulation;
use crate::sim::Sim;

/// 保存するデータ。画面はこれを IndexedDB の 1 枠に書く。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveData {
    /// 記録の形式の版（`chizu_core::record::FORMAT_VERSION`）。
    pub format_version: u32,
    pub seed: u64,
    pub tick: u64,
    /// `CommandLog::encode` の形式の記録。
    pub commands: Vec<u8>,
}

impl SaveData {
    pub fn of(sim: &Sim) -> SaveData {
        SaveData {
            format_version: FORMAT_VERSION,
            seed: sim.world().seed(),
            tick: sim.world().tick().0,
            commands: sim.log().encode(),
        }
    }
}

/// 保存データを読み込めない理由。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadError {
    /// 記録の形式の版が違う。古い版の記録は読み替えずに断る。
    Version {
        saved: u32,
    },
    Decode(DecodeError),
    /// 記録の最後のコマンドが、保存した tick より後にある。
    CommandAfterTick {
        command_tick: u64,
        tick: u64,
    },
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Version { saved } => write!(
                f,
                "記録の形式の版 {saved} は読めない（読めるのは {FORMAT_VERSION}）"
            ),
            LoadError::Decode(e) => write!(f, "記録を読めない：{e}"),
            LoadError::CommandAfterTick { command_tick, tick } => write!(
                f,
                "tick {tick} の保存データに、tick {command_tick} のコマンドがある"
            ),
        }
    }
}

/// 保存データから作り直している途中の街。
pub struct Restore {
    world: World,
    log: CommandLog,
    target: u64,
}

impl Restore {
    pub fn new(data: &SaveData) -> Result<Restore, LoadError> {
        if data.format_version != FORMAT_VERSION {
            return Err(LoadError::Version {
                saved: data.format_version,
            });
        }
        let log = CommandLog::decode(&data.commands).map_err(|e| match e {
            DecodeError::UnsupportedVersion(saved) => LoadError::Version { saved },
            e => LoadError::Decode(e),
        })?;
        if let Some(last) = log.entries().last()
            && last.tick.0 > data.tick
        {
            return Err(LoadError::CommandAfterTick {
                command_tick: last.tick.0,
                tick: data.tick,
            });
        }
        let mut world = World::new(data.seed);
        // tick 0 のコマンドを適用する
        replay(&mut world, &log, 0);
        Ok(Restore {
            world,
            log,
            target: data.tick,
        })
    }

    /// 多くて `max_ticks` だけ再生を進める。
    pub fn step(&mut self, max_ticks: u64) {
        let ticks = max_ticks.min(self.target - self.world.tick().0);
        replay(&mut self.world, &self.log, ticks);
    }

    pub fn tick(&self) -> u64 {
        self.world.tick().0
    }

    pub fn target(&self) -> u64 {
        self.target
    }

    pub fn done(&self) -> bool {
        self.world.tick().0 == self.target
    }

    /// 再生し終えた街から `Sim` を作る。
    ///
    /// # Panics
    ///
    /// 再生し終えていないと panic する。
    pub fn finish(self) -> Sim {
        assert!(self.done(), "再生し終えていない");
        Sim::from_parts(self.world, self.log)
    }
}

/// 保存する記録の形式の版。画面は、版の違う保存データを読めないと示すのに使う。
#[wasm_bindgen]
pub fn record_format_version() -> u32 {
    FORMAT_VERSION
}

#[wasm_bindgen]
impl Simulation {
    /// 自動保存の判断に使う軽い数。`[tick, 日, 記録の件数]`。
    pub fn save_clock(&self) -> Vec<f64> {
        let w = self.0.world();
        let day = GameTime::at(w.tick()).day;
        vec![w.tick().0 as f64, day as f64, self.0.log().len() as f64]
    }

    /// 保存データのうち、記録のバイト列を除いたものを JSON で返す。記録は `encode_log` で取る。
    /// 画面に並べるための日、時刻、所持金、読み込んだあとの確かめに使う状態ハッシュも添える。
    /// 種と所持金は JS の数で誤差なく表せないことがあるので、文字列にする。
    pub fn save_meta(&self) -> String {
        let w = self.0.world();
        let data = SaveData::of(&self.0);
        let time = GameTime::at(w.tick());
        format!(
            "{{\"formatVersion\":{},\"seed\":\"{}\",\"tick\":{},\"day\":{},\"secondOfDay\":{},\"money\":\"{}\",\"stateHash\":\"{:016x}\"}}",
            data.format_version,
            data.seed,
            data.tick,
            time.day,
            time.second_of_day,
            w.money().0,
            w.state_hash()
        )
    }
}

/// 保存データから作り直している途中の街。`step` を繰り返し、`done` になったら `finish` で取り出す。
#[wasm_bindgen]
pub struct Restoring(Restore);

#[wasm_bindgen]
impl Restoring {
    /// 種は 10 進の文字列、tick は 0 以上の整数で渡す。
    #[wasm_bindgen(constructor)]
    pub fn new(
        format_version: u32,
        seed: &str,
        tick: f64,
        commands: Vec<u8>,
    ) -> Result<Restoring, JsError> {
        let seed: u64 = seed
            .parse()
            .map_err(|_| JsError::new(&format!("種 {seed:?} を読めない")))?;
        if !(tick >= 0.0 && tick.fract() == 0.0 && tick <= 2f64.powi(53)) {
            return Err(JsError::new(&format!("tick {tick} を読めない")));
        }
        let data = SaveData {
            format_version,
            seed,
            tick: tick as u64,
            commands,
        };
        Restore::new(&data)
            .map(Restoring)
            .map_err(|e| JsError::new(&e.to_string()))
    }

    pub fn step(&mut self, max_ticks: u32) {
        self.0.step(u64::from(max_ticks));
    }

    pub fn tick(&self) -> f64 {
        self.0.tick() as f64
    }

    pub fn target(&self) -> f64 {
        self.0.target() as f64
    }

    pub fn done(&self) -> bool {
        self.0.done()
    }

    pub fn finish(self) -> Simulation {
        Simulation(self.0.finish())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chizu_core::world::growth::GROWTH_INTERVAL;
    use chizu_core::{Command, RoadKind, TilePos, TileRect, ZoneKind};

    fn build_city(sim: &mut Sim) {
        sim.submit(Command::BuildRoad {
            from: TilePos::new(10, 10),
            to: TilePos::new(40, 10),
            kind: RoadKind::Small,
        })
        .unwrap();
        sim.submit(Command::PaintZone {
            area: TileRect::from_corners(TilePos::new(10, 11), TilePos::new(30, 16)),
            zone: Some(ZoneKind::Residential),
        })
        .unwrap();
        sim.advance(u32::try_from(GROWTH_INTERVAL * 3 + 5).unwrap());
        sim.submit(Command::PaintZone {
            area: TileRect::from_corners(TilePos::new(10, 4), TilePos::new(30, 9)),
            zone: Some(ZoneKind::Commercial),
        })
        .unwrap();
        sim.advance(u32::try_from(GROWTH_INTERVAL * 2).unwrap());
    }

    fn restore(data: &SaveData, chunk: u64) -> Sim {
        let mut r = Restore::new(data).unwrap();
        while !r.done() {
            r.step(chunk);
        }
        r.finish()
    }

    #[test]
    fn restored_state_matches_the_saved_state() {
        let mut sim = Sim::new(3);
        build_city(&mut sim);
        assert!(!sim.world().buildings().is_empty(), "建物が建っていない");
        let data = SaveData::of(&sim);
        for chunk in [1, 7, 1_000_000] {
            let restored = restore(&data, chunk);
            assert_eq!(restored.world().state_hash(), sim.world().state_hash());
            assert_eq!(restored.world().money(), sim.world().money());
            assert_eq!(restored.world().tick(), sim.world().tick());
        }
    }

    #[test]
    fn commands_recorded_while_paused_are_restored() {
        let mut sim = Sim::new(5);
        sim.advance(10);
        sim.set_paused(true);
        build_city(&mut sim);
        assert_eq!(sim.world().tick().0, 10);
        let restored = restore(&SaveData::of(&sim), 3);
        assert_eq!(restored.world().state_hash(), sim.world().state_hash());
    }

    #[test]
    fn continuing_after_load_matches_an_unbroken_run() {
        let mut through = Sim::new(9);
        build_city(&mut through);
        let mut loaded = restore(&SaveData::of(&through), 100);
        for sim in [&mut through, &mut loaded] {
            sim.submit(Command::BuildRoad {
                from: TilePos::new(40, 10),
                to: TilePos::new(40, 30),
                kind: RoadKind::Medium,
            })
            .unwrap();
            sim.advance(u32::try_from(GROWTH_INTERVAL * 2).unwrap());
        }
        assert_eq!(loaded.world().state_hash(), through.world().state_hash());
        assert_eq!(loaded.log(), through.log());
    }

    #[test]
    fn other_format_versions_are_refused() {
        let mut data = SaveData::of(&Sim::new(1));
        data.format_version = FORMAT_VERSION + 1;
        assert_eq!(
            Restore::new(&data).err(),
            Some(LoadError::Version {
                saved: FORMAT_VERSION + 1
            })
        );
        // 記録の中に書いた版も見る
        let mut data = SaveData::of(&Sim::new(1));
        data.commands[8..12].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
        assert_eq!(
            Restore::new(&data).err(),
            Some(LoadError::Version {
                saved: FORMAT_VERSION + 1
            })
        );
    }

    #[test]
    fn a_command_after_the_saved_tick_is_refused() {
        let mut sim = Sim::new(1);
        sim.advance(20);
        build_city(&mut sim);
        let mut data = SaveData::of(&sim);
        data.tick = 5;
        assert!(matches!(
            Restore::new(&data),
            Err(LoadError::CommandAfterTick { tick: 5, .. })
        ));
    }
}
