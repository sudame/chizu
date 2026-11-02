//! ブラウザの中に保存するデータの取り出しと、保存データからの作り直し。
//!
//! 保存するのは種、tick、コマンドの記録と、街の状態（`World::to_snapshot`）である。
//! 読み込むときは、街の状態を読み戻せればそこから開く。状態がないか、読み戻せないか、読み戻した街が
//! 保存データの種、tick、記録の件数、状態ハッシュと食い違えば、種から作った街に記録を再生して作り直す。
//! 記録は、フィードバックの再現と、状態から読めない保存データのために残す。
//! 再生は長くかかりうるので、[`Restore::step`] で少しずつ進め、そのあいだに Worker が進み具合を送れるようにする。

use core::fmt;

use chizu_core::record::FORMAT_VERSION;
use chizu_core::{CommandLog, DecodeError, GameTime, TICKS_PER_DAY, World, replay};
use wasm_bindgen::prelude::*;

use crate::Simulation;
use crate::sim::Sim;

/// 読み込む保存データの tick の上限。ゲーム内の 10 年にあたる。
///
/// 壊れた保存データの tick がとても大きいと、再生が実質終わらない。
/// 遊んで届く長さより十分に長く、再生し切れる長さで区切る。
pub const MAX_SAVED_TICK: u64 = 3_650 * TICKS_PER_DAY;

/// 保存するデータ。画面はこれを IndexedDB の 1 枠に書く。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveData {
    /// 記録の形式の版（`chizu_core::record::FORMAT_VERSION`）。
    pub format_version: u32,
    pub seed: u64,
    pub tick: u64,
    /// 記録の件数。記録の次の番号（`next_seq`）と同じである。
    pub command_count: u64,
    /// 保存したときの状態ハッシュ。
    pub state_hash: u64,
    /// `CommandLog::encode` の形式の記録。
    pub commands: Vec<u8>,
    /// `World::to_snapshot` の形式の街の状態。状態を入れる前の保存データにはない。
    pub state: Option<Vec<u8>>,
}

impl SaveData {
    pub fn of(sim: &Sim) -> SaveData {
        let w = sim.world();
        SaveData {
            format_version: FORMAT_VERSION,
            seed: w.seed(),
            tick: w.tick().0,
            command_count: sim.log().next_seq(),
            state_hash: w.state_hash(),
            commands: sim.log().encode(),
            state: Some(w.to_snapshot()),
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
    TickTooLarge {
        tick: u64,
    },
    /// 記録の最後のコマンドが、保存した tick より後にある。
    CommandAfterTick {
        command_tick: u64,
        tick: u64,
    },
    /// 記録の件数が、保存データに書いた件数と違う。
    CommandCount {
        saved: u64,
        actual: u64,
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
            LoadError::TickTooLarge { tick } => {
                write!(f, "tick {tick} は上限 {MAX_SAVED_TICK} を超えている")
            }
            LoadError::CommandAfterTick { command_tick, tick } => write!(
                f,
                "tick {tick} の保存データに、tick {command_tick} のコマンドがある"
            ),
            LoadError::CommandCount { saved, actual } => write!(
                f,
                "記録の件数が {actual} 件で、保存データに書いた {saved} 件と違う"
            ),
        }
    }
}

/// 保存データから作り直している途中の街。
pub struct Restore {
    world: World,
    log: CommandLog,
    target: u64,
    saved_hash: u64,
    from_state: bool,
}

/// 作り直し終えた街。
pub struct Restored {
    pub sim: Sim,
    /// 記録を再生せずに、街の状態から開いた。
    pub from_state: bool,
    /// 作り直した街の状態ハッシュが、保存したときと違う。記録を読めても、シミュレーションの規則が
    /// 変わっていると違う街になる。読み込みは止めず、画面で知らせる。
    pub rules_changed: bool,
}

impl Restore {
    pub fn new(data: &SaveData) -> Result<Restore, LoadError> {
        if data.format_version != FORMAT_VERSION {
            return Err(LoadError::Version {
                saved: data.format_version,
            });
        }
        if data.tick > MAX_SAVED_TICK {
            return Err(LoadError::TickTooLarge { tick: data.tick });
        }
        let log = CommandLog::decode(&data.commands).map_err(|e| match e {
            DecodeError::UnsupportedVersion(saved) => LoadError::Version { saved },
            e => LoadError::Decode(e),
        })?;
        if log.next_seq() != data.command_count {
            return Err(LoadError::CommandCount {
                saved: data.command_count,
                actual: log.next_seq(),
            });
        }
        if let Some(last) = log.entries().last()
            && last.tick.0 > data.tick
        {
            return Err(LoadError::CommandAfterTick {
                command_tick: last.tick.0,
                tick: data.tick,
            });
        }
        if let Some(world) = data
            .state
            .as_deref()
            .and_then(|s| world_from_state(data, s))
        {
            return Ok(Restore {
                world,
                log,
                target: data.tick,
                saved_hash: data.state_hash,
                from_state: true,
            });
        }
        let mut world = World::new(data.seed);
        // tick 0 のコマンドを適用する
        replay(&mut world, &log, 0);
        Ok(Restore {
            world,
            log,
            target: data.tick,
            saved_hash: data.state_hash,
            from_state: false,
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

    /// 再生し終えた街を取り出し、状態ハッシュを保存したときと比べる。
    ///
    /// # Panics
    ///
    /// 再生し終えていないと panic する。
    pub fn finish(self) -> Restored {
        assert!(self.done(), "再生し終えていない");
        let rules_changed = self.world.state_hash() != self.saved_hash;
        Restored {
            sim: Sim::from_parts(self.world, self.log),
            from_state: self.from_state,
            rules_changed,
        }
    }
}

/// 街の状態を読み戻す。読み戻せないか、保存データのほかの欄と食い違えば `None` を返し、記録の再生に任せる。
///
/// 状態ハッシュも比べる。状態の形を変えたのに版を上げ忘れていても、読み違えた街から始めないためである。
fn world_from_state(data: &SaveData, state: &[u8]) -> Option<World> {
    let world = World::from_snapshot(state).ok()?;
    let agrees = world.seed() == data.seed
        && world.tick().0 == data.tick
        && world.next_seq() == data.command_count
        && world.state_hash() == data.state_hash;
    agrees.then_some(world)
}

/// 画面から受け取った保存データを確かめて [`SaveData`] にする。
/// 種と状態ハッシュは文字列（10 進と 16 進の 16 桁）、tick と件数は JS の数で受け取る。
pub fn parse_input(
    format_version: u32,
    seed: &str,
    tick: f64,
    command_count: f64,
    state_hash: &str,
    commands: Vec<u8>,
    state: Option<Vec<u8>>,
) -> Result<SaveData, String> {
    let seed: u64 = seed
        .parse()
        .map_err(|_| format!("種 {seed:?} を読めない"))?;
    let whole = |what: &str, v: f64| {
        if v >= 0.0 && v.fract() == 0.0 && v <= 2f64.powi(53) {
            Ok(v as u64)
        } else {
            Err(format!("{what} {v} を読めない"))
        }
    };
    let tick = whole("tick", tick)?;
    let command_count = whole("記録の件数", command_count)?;
    let state_hash = if state_hash.len() == 16 && state_hash.bytes().all(|b| b.is_ascii_hexdigit())
    {
        u64::from_str_radix(state_hash, 16).ok()
    } else {
        None
    }
    .ok_or_else(|| format!("状態ハッシュ {state_hash:?} を読めない"))?;
    Ok(SaveData {
        format_version,
        seed,
        tick,
        command_count,
        state_hash,
        commands,
        state,
    })
}

/// 保存する記録の形式の版。画面は、版の違う保存データを読めないと示すのに使う。
#[wasm_bindgen]
pub fn record_format_version() -> u32 {
    FORMAT_VERSION
}

/// 同じ時点で取り出した保存データ。`meta` は記録と街の状態のバイト列を除いたものの JSON である。
#[wasm_bindgen]
pub struct SavePayload {
    meta: String,
    commands: Vec<u8>,
    state: Vec<u8>,
}

#[wasm_bindgen]
impl SavePayload {
    /// [`Simulation::save_meta`] と同じ形の JSON。
    pub fn meta(&self) -> String {
        self.meta.clone()
    }

    /// 記録のバイト列を取り出す。複製しないので、2 度目は空になる。
    pub fn take_commands(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.commands)
    }

    /// 街の状態のバイト列を取り出す。複製しないので、2 度目は空になる。
    pub fn take_state(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.state)
    }
}

/// 記録のバイト列を除いた保存データの JSON。記録は encode しない。
fn meta_json(sim: &Sim) -> String {
    let w = sim.world();
    let time = GameTime::at(w.tick());
    format!(
        "{{\"formatVersion\":{},\"seed\":\"{}\",\"tick\":{},\"commandCount\":{},\"stateHash\":\"{:016x}\",\"day\":{},\"secondOfDay\":{},\"money\":\"{}\"}}",
        FORMAT_VERSION,
        w.seed(),
        w.tick().0,
        sim.log().next_seq(),
        w.state_hash(),
        time.day,
        time.second_of_day,
        w.money().0,
    )
}

#[wasm_bindgen]
impl Simulation {
    /// 自動保存の判断に使う軽い数。`[tick, 日, 記録の件数]`。
    pub fn save_clock(&self) -> Vec<f64> {
        let w = self.0.world();
        let day = GameTime::at(w.tick()).day;
        vec![w.tick().0 as f64, day as f64, self.0.log().len() as f64]
    }

    /// 種、tick、記録の件数、状態ハッシュに、画面に並べる日、時刻、所持金を添えた JSON。
    /// 種と所持金は JS の数で誤差なく表せないことがあるので、文字列にする。
    pub fn save_meta(&self) -> String {
        meta_json(&self.0)
    }

    /// いまの状態の保存データ。メタデータ、記録、街の状態は同じ時点のものである。
    pub fn save(&self) -> SavePayload {
        SavePayload {
            meta: meta_json(&self.0),
            commands: self.0.log().encode(),
            state: self.0.world().to_snapshot(),
        }
    }
}

/// 保存データから作り直している途中の街。`step` を繰り返し、`done` になったら `finish` で取り出す。
#[wasm_bindgen]
pub struct Restoring(Restore);

#[wasm_bindgen]
impl Restoring {
    /// 引数の形は [`parse_input`] にある。`state` は、街の状態を入れる前の保存データでは渡さない。
    #[wasm_bindgen(constructor)]
    pub fn new(
        format_version: u32,
        seed: &str,
        tick: f64,
        command_count: f64,
        state_hash: &str,
        commands: Vec<u8>,
        state: Option<Vec<u8>>,
    ) -> Result<Restoring, JsError> {
        let data = parse_input(
            format_version,
            seed,
            tick,
            command_count,
            state_hash,
            commands,
            state,
        )
        .map_err(|e| JsError::new(&e))?;
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

    pub fn finish(self) -> RestoredSimulation {
        let r = self.0.finish();
        RestoredSimulation {
            sim: Some(Simulation(r.sim, crate::layers::LayerCache::default())),
            from_state: r.from_state,
            rules_changed: r.rules_changed,
        }
    }
}

/// 作り直し終えた街。`rules_changed` を見てから `take` で取り出す。
#[wasm_bindgen]
pub struct RestoredSimulation {
    sim: Option<Simulation>,
    from_state: bool,
    rules_changed: bool,
}

#[wasm_bindgen]
impl RestoredSimulation {
    /// 作り直した街の状態ハッシュが、保存したときと違う。シミュレーションの規則が変わっている。
    pub fn rules_changed(&self) -> bool {
        self.rules_changed
    }

    /// 記録を再生せずに、街の状態から開いた。
    pub fn from_state(&self) -> bool {
        self.from_state
    }

    /// 街を取り出す。2 度目は失敗する。
    pub fn take(&mut self) -> Result<Simulation, JsError> {
        self.sim
            .take()
            .ok_or_else(|| JsError::new("街はもう取り出してある"))
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

    fn restore(data: &SaveData, chunk: u64) -> Restored {
        let mut r = Restore::new(data).unwrap();
        while !r.done() {
            r.step(chunk);
        }
        r.finish()
    }

    /// 街の状態を入れる前の形の保存データ。読み込むと記録を再生する。
    fn without_state(mut data: SaveData) -> SaveData {
        data.state = None;
        data
    }

    #[test]
    fn restored_state_matches_the_saved_state() {
        let mut sim = Sim::new(3);
        build_city(&mut sim);
        assert!(!sim.world().buildings().is_empty(), "建物が建っていない");
        let data = SaveData::of(&sim);
        for (data, from_state) in [(data.clone(), true), (without_state(data), false)] {
            for chunk in [1, 7, 1_000_000] {
                let restored = restore(&data, chunk);
                assert!(!restored.rules_changed);
                assert_eq!(restored.from_state, from_state);
                let w = restored.sim.world();
                assert_eq!(w.state_hash(), sim.world().state_hash());
                assert_eq!(w.money(), sim.world().money());
                assert_eq!(w.tick(), sim.world().tick());
                assert_eq!(restored.sim.log(), sim.log());
            }
        }
    }

    #[test]
    fn a_save_with_state_opens_without_replaying() {
        let mut sim = Sim::new(3);
        build_city(&mut sim);
        let r = Restore::new(&SaveData::of(&sim)).unwrap();
        assert!(r.done(), "状態があるのに再生しようとしている");
        assert_eq!(r.tick(), sim.world().tick().0);
        let restored = r.finish();
        assert!(restored.from_state && !restored.rules_changed);
        assert_eq!(restored.sim.world().state_hash(), sim.world().state_hash());
    }

    #[test]
    fn an_unreadable_or_mismatched_state_falls_back_to_the_record() {
        let mut sim = Sim::new(3);
        build_city(&mut sim);
        let data = SaveData::of(&sim);
        let state = data.state.clone().unwrap();
        let mut other_version = state.clone();
        other_version[8..12].copy_from_slice(&(chizu_core::SNAPSHOT_VERSION + 1).to_le_bytes());
        let mut earlier = Sim::new(3);
        earlier.advance(5);
        let cases = [
            ("版が違う", other_version),
            ("途中で切れている", state[..state.len() / 2].to_vec()),
            ("別の時点の状態", earlier.world().to_snapshot()),
        ];
        for (what, state) in cases {
            let data = SaveData {
                state: Some(state),
                ..data.clone()
            };
            let restored = restore(&data, 1_000);
            assert!(!restored.from_state, "{what}");
            assert!(!restored.rules_changed, "{what}");
            assert_eq!(
                restored.sim.world().state_hash(),
                sim.world().state_hash(),
                "{what}"
            );
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
        assert_eq!(restored.sim.world().state_hash(), sim.world().state_hash());
    }

    #[test]
    fn a_save_at_tick_zero_is_restored() {
        let mut sim = Sim::new(4);
        sim.set_paused(true);
        build_city(&mut sim);
        let data = SaveData::of(&sim);
        assert_eq!(data.tick, 0);
        let mut r = Restore::new(&data).unwrap();
        assert!(r.done());
        r.step(10);
        let restored = r.finish();
        assert!(!restored.rules_changed);
        assert_eq!(restored.sim.world().state_hash(), sim.world().state_hash());
    }

    #[test]
    fn continuing_after_load_matches_an_unbroken_run() {
        let mut through = Sim::new(9);
        build_city(&mut through);
        let data = SaveData::of(&through);
        let mut from_state = restore(&data, 100).sim;
        let mut replayed = restore(&without_state(data), 100).sim;
        for sim in [&mut through, &mut from_state, &mut replayed] {
            sim.submit(Command::BuildRoad {
                from: TilePos::new(40, 10),
                to: TilePos::new(40, 30),
                kind: RoadKind::Medium,
            })
            .unwrap();
            sim.advance(u32::try_from(GROWTH_INTERVAL * 2).unwrap());
        }
        for loaded in [&from_state, &replayed] {
            assert_eq!(loaded.world().state_hash(), through.world().state_hash());
            assert_eq!(loaded.log(), through.log());
        }
    }

    #[test]
    fn a_different_hash_is_reported_as_changed_rules() {
        let mut sim = Sim::new(2);
        build_city(&mut sim);
        let mut data = SaveData::of(&sim);
        data.state_hash ^= 1;
        let restored = restore(&data, 1_000);
        assert!(restored.rules_changed);
        // 状態ハッシュの食い違う状態からは開かず、記録を再生する
        assert!(!restored.from_state);
        assert_eq!(restored.sim.world().state_hash(), sim.world().state_hash());
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
    fn broken_records_are_refused() {
        let mut sim = Sim::new(1);
        build_city(&mut sim);
        let mut data = SaveData::of(&sim);
        data.commands.truncate(data.commands.len() - 1);
        assert!(matches!(
            Restore::new(&data),
            Err(LoadError::Decode(DecodeError::UnexpectedEnd(_)))
        ));
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

    #[test]
    fn a_wrong_command_count_is_refused() {
        let mut sim = Sim::new(1);
        build_city(&mut sim);
        let mut data = SaveData::of(&sim);
        data.command_count += 1;
        assert!(matches!(
            Restore::new(&data),
            Err(LoadError::CommandCount { .. })
        ));
    }

    #[test]
    fn a_tick_over_the_limit_is_refused() {
        let mut data = SaveData::of(&Sim::new(1));
        data.tick = MAX_SAVED_TICK + 1;
        assert_eq!(
            Restore::new(&data).err(),
            Some(LoadError::TickTooLarge {
                tick: MAX_SAVED_TICK + 1
            })
        );
        data.tick = MAX_SAVED_TICK;
        assert!(Restore::new(&data).is_ok());
    }

    #[test]
    fn meta_matches_the_save_data() {
        let mut sim = Sim::new(6);
        build_city(&mut sim);
        let data = SaveData::of(&sim);
        let meta = meta_json(&sim);
        for part in [
            format!("\"seed\":\"{}\"", data.seed),
            format!("\"tick\":{},", data.tick),
            format!("\"commandCount\":{},", data.command_count),
            format!("\"stateHash\":\"{:016x}\"", data.state_hash),
        ] {
            assert!(meta.contains(&part), "{meta} に {part} がない");
        }
    }

    #[test]
    fn input_from_the_page_is_checked() {
        let good = SaveData::of(&Sim::new(8));
        let hash = format!("{:016x}", good.state_hash);
        let parse = |seed: &str, tick: f64, count: f64, hash: &str| {
            parse_input(
                1,
                seed,
                tick,
                count,
                hash,
                good.commands.clone(),
                good.state.clone(),
            )
        };
        assert_eq!(parse("8", 0.0, 0.0, &hash), Ok(good.clone()));
        assert_eq!(
            parse("18446744073709551615", 3.0, 0.0, &hash).map(|d| d.seed),
            Ok(u64::MAX)
        );
        for seed in ["", "abc", "1.5", "-1", "18446744073709551616"] {
            assert!(parse(seed, 0.0, 0.0, &hash).is_err(), "種 {seed:?}");
        }
        for tick in [-1.0, 0.5, f64::NAN, f64::INFINITY, 2f64.powi(53) + 2.0] {
            assert!(parse("8", tick, 0.0, &hash).is_err(), "tick {tick}");
        }
        assert!(parse("8", 0.0, -1.0, &hash).is_err());
        for bad in [
            "",
            "123",
            "zzzzzzzzzzzzzzzz",
            "+000000000000000",
            "00000000000000000",
        ] {
            assert!(parse("8", 0.0, 0.0, bad).is_err(), "ハッシュ {bad:?}");
        }
        // 数としては読めても、上限を超える tick は読み込みで断る
        let far = parse("8", (MAX_SAVED_TICK + 1) as f64, 0.0, &hash).unwrap();
        assert!(matches!(
            Restore::new(&far),
            Err(LoadError::TickTooLarge { .. })
        ));
    }
}
