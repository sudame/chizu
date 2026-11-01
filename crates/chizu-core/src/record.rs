//! コマンドの記録と再生。
//!
//! 受け取ったコマンドには通し番号を振り、「次に進める tick の頭で適用する」と決めて
//! `(tick, 番号, Command)` を記録する。同じ tick のコマンドは番号順に適用する。
//! 再現に必要なのは、種とこの記録だけである。
//!
//! 記録の並びは、tick が減らず、番号が 1 件ごとに増えていく。
//! 記録するときも読み戻すときも [`Command::validate`] を通すので、記録に入ったものは必ず読み戻せる。
//!
//! # バイト列の形式
//!
//! 整数はすべてリトルエンディアンで、型の幅のまま書く。
//!
//! ```text
//! 先頭   b"CHIZUREC"（8 バイト）
//! 版     u32（いまは 1）
//! 件数   u64
//! 各件   tick u64、番号 u64、種類 u8、種類ごとの中身
//! ```
//!
//! 種類ごとの中身（`TilePos` は x、y の `i64`、`TileRect` は min、max の `TilePos`）：
//!
//! | 種類 | コマンド | 中身 |
//! |---|---|---|
//! | 1 | `BuildRoad` | from `TilePos`、to `TilePos`、kind `u8` |
//! | 2 | `Bulldoze` | area `TileRect` |
//! | 3 | `PaintZone` | area `TileRect`、zone `u8` |
//! | 4 | `PlaceBuilding` | kind `u8`、at `TilePos`、rotation `u8` |
//! | 5 | `SetSignal` | index `u32`、generation `u16`、on `u8`（0 か 1） |
//! | 6 | `BuyChunk` | cx `i32`、cy `i32` |
//! | 7 | `SetTaxRate` | zone `u8`、permille `u16` |
//! | 8 | `SetSimSpeed` | 倍率 `u16` |
//!
//! 形式を変えるときは版を上げる。種類の番号は使い回さない。

use core::fmt;

use crate::command::{Command, InvalidCommand, NodeId, RoadKind, ServiceKind, SimSpeed, ZoneKind};
use crate::tile::{ChunkCoord, TilePos, TileRect};
use crate::units::Tick;
use crate::world::{ApplyOutcome, World};

const MAGIC: &[u8; 8] = b"CHIZUREC";

/// 書き出す形式の版。
pub const FORMAT_VERSION: u32 = 1;

/// 記録の 1 件。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordedCommand {
    /// 頭で適用する tick。
    pub tick: Tick,
    /// 受け取った順の通し番号。
    pub seq: u64,
    pub command: Command,
}

/// tick が減っているか、番号が増えていない。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutOfOrder {
    pub previous: (Tick, u64),
    pub next: (Tick, u64),
}

impl fmt::Display for OutOfOrder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "記録の順番が崩れている：(tick {}, 番号 {}) のあとに (tick {}, 番号 {}) がある",
            self.previous.0.0, self.previous.1, self.next.0.0, self.next.1
        )
    }
}

impl std::error::Error for OutOfOrder {}

/// 記録できなかった理由。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordError {
    OutOfOrder(OutOfOrder),
    Invalid(InvalidCommand),
    /// 直前の番号が `u64::MAX` で、次の番号を振れない。
    SeqExhausted,
}

impl fmt::Display for RecordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RecordError::OutOfOrder(e) => e.fmt(f),
            RecordError::Invalid(e) => e.fmt(f),
            RecordError::SeqExhausted => write!(f, "番号を使い切った"),
        }
    }
}

impl std::error::Error for RecordError {}

/// コマンドの記録。tick は減らず、番号は増えていく。中のコマンドはすべて検証を通っている。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandLog {
    entries: Vec<RecordedCommand>,
}

impl CommandLog {
    pub fn new() -> CommandLog {
        CommandLog::default()
    }

    pub fn entries(&self) -> &[RecordedCommand] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 1 件足す。並びが崩れるときと、コマンドが検証を通らないときは足さない。
    pub fn push(&mut self, entry: RecordedCommand) -> Result<(), RecordError> {
        if let Some(last) = self.entries.last()
            && (entry.tick < last.tick || entry.seq <= last.seq)
        {
            return Err(RecordError::OutOfOrder(OutOfOrder {
                previous: (last.tick, last.seq),
                next: (entry.tick, entry.seq),
            }));
        }
        entry.command.validate().map_err(RecordError::Invalid)?;
        self.entries.push(entry);
        Ok(())
    }

    /// 受け取ったコマンドを、直前の番号の次の番号で記録する。振った番号を返す。
    pub fn record(&mut self, tick: Tick, command: Command) -> Result<u64, RecordError> {
        let seq = match self.entries.last() {
            None => 0,
            Some(last) => last.seq.checked_add(1).ok_or(RecordError::SeqExhausted)?,
        };
        self.push(RecordedCommand { tick, seq, command })?;
        Ok(seq)
    }

    /// 決まった形式のバイト列に書き出す。
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::default();
        w.bytes(MAGIC);
        w.u32(FORMAT_VERSION);
        w.len(self.entries.len());
        for e in &self.entries {
            w.u64(e.tick.0);
            w.u64(e.seq);
            encode_command(&mut w, &e.command);
        }
        w.buf
    }

    /// [`CommandLog::encode`] の形式から読み戻す。
    pub fn decode(bytes: &[u8]) -> Result<CommandLog, DecodeError> {
        let mut r = Reader { bytes, pos: 0 };
        if r.take(MAGIC.len())? != MAGIC {
            return Err(DecodeError::BadMagic);
        }
        let version = r.u32()?;
        if version != FORMAT_VERSION {
            return Err(DecodeError::UnsupportedVersion(version));
        }
        let count = r.u64()?;
        let mut log = CommandLog::new();
        for _ in 0..count {
            let tick = Tick(r.u64()?);
            let seq = r.u64()?;
            let command = decode_command(&mut r)?;
            log.push(RecordedCommand { tick, seq, command })
                .map_err(|e| match e {
                    RecordError::OutOfOrder(e) => DecodeError::OutOfOrder(e),
                    RecordError::Invalid(e) => DecodeError::InvalidCommand {
                        error: e,
                        at: r.pos,
                    },
                    // 番号を振るのは record だけなので、push からは返らない。
                    RecordError::SeqExhausted => unreachable!(),
                })?;
        }
        if r.pos != bytes.len() {
            return Err(DecodeError::TrailingBytes(bytes.len() - r.pos));
        }
        Ok(log)
    }
}

/// 記録を読み戻せなかった理由。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    BadMagic,
    UnsupportedVersion(u32),
    /// 途中で終わっている。値は読もうとした位置である。
    UnexpectedEnd(usize),
    UnknownCommand(u8),
    /// 値が受け付ける範囲の外にある。値はその値を読み終えた位置である。
    InvalidValue {
        what: &'static str,
        at: usize,
    },
    /// コマンドが検証を通らない。`at` はそのコマンドを読み終えた位置である。
    InvalidCommand {
        error: InvalidCommand,
        at: usize,
    },
    OutOfOrder(OutOfOrder),
    TrailingBytes(usize),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::BadMagic => write!(f, "chizu の記録ではない"),
            DecodeError::UnsupportedVersion(v) => {
                write!(f, "記録の版 {v} は読めない（読めるのは {FORMAT_VERSION}）")
            }
            DecodeError::UnexpectedEnd(at) => write!(f, "{at} バイト目で記録が途切れている"),
            DecodeError::UnknownCommand(c) => write!(f, "知らないコマンドの種類 {c}"),
            DecodeError::InvalidValue { what, at } => {
                write!(f, "{at} バイト目までの {what} が範囲の外にある")
            }
            DecodeError::InvalidCommand { error, at } => {
                write!(f, "{at} バイト目までのコマンドが不正：{error}")
            }
            DecodeError::OutOfOrder(e) => e.fmt(f),
            DecodeError::TrailingBytes(n) => write!(f, "記録のあとに {n} バイト余っている"),
        }
    }
}

impl std::error::Error for DecodeError {}

#[derive(Default)]
struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    fn bytes(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }
    fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.bytes(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.bytes(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }
    fn i32(&mut self, v: i32) {
        self.bytes(&v.to_le_bytes());
    }
    fn i64(&mut self, v: i64) {
        self.bytes(&v.to_le_bytes());
    }
    fn len(&mut self, len: usize) {
        self.u64(u64::try_from(len).expect("件数が u64 に収まらない"));
    }
    fn tile(&mut self, p: TilePos) {
        self.i64(p.x);
        self.i64(p.y);
    }
    fn rect(&mut self, r: TileRect) {
        self.tile(r.min);
        self.tile(r.max);
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&end| end <= self.bytes.len())
            .ok_or(DecodeError::UnexpectedEnd(self.pos))?;
        let s = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        let mut a = [0; N];
        a.copy_from_slice(self.take(N)?);
        Ok(a)
    }
    fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.array::<1>()?[0])
    }
    fn u16(&mut self) -> Result<u16, DecodeError> {
        self.array().map(u16::from_le_bytes)
    }
    fn u32(&mut self) -> Result<u32, DecodeError> {
        self.array().map(u32::from_le_bytes)
    }
    fn u64(&mut self) -> Result<u64, DecodeError> {
        self.array().map(u64::from_le_bytes)
    }
    fn i32(&mut self) -> Result<i32, DecodeError> {
        self.array().map(i32::from_le_bytes)
    }
    fn i64(&mut self) -> Result<i64, DecodeError> {
        self.array().map(i64::from_le_bytes)
    }
    fn tile(&mut self) -> Result<TilePos, DecodeError> {
        Ok(TilePos::new(self.i64()?, self.i64()?))
    }
    fn rect(&mut self) -> Result<TileRect, DecodeError> {
        Ok(TileRect {
            min: self.tile()?,
            max: self.tile()?,
        })
    }
    fn check<T>(&self, ok: bool, what: &'static str, value: T) -> Result<T, DecodeError> {
        if ok {
            Ok(value)
        } else {
            Err(DecodeError::InvalidValue { what, at: self.pos })
        }
    }
    fn code<T>(&mut self, what: &'static str, from: fn(u8) -> Option<T>) -> Result<T, DecodeError> {
        let code = self.u8()?;
        from(code).ok_or(DecodeError::InvalidValue { what, at: self.pos })
    }
}

fn encode_command(w: &mut Writer, c: &Command) {
    match *c {
        Command::BuildRoad { from, to, kind } => {
            w.u8(1);
            w.tile(from);
            w.tile(to);
            w.u8(kind as u8);
        }
        Command::Bulldoze { area } => {
            w.u8(2);
            w.rect(area);
        }
        Command::PaintZone { area, zone } => {
            w.u8(3);
            w.rect(area);
            w.u8(zone as u8);
        }
        Command::PlaceBuilding { kind, at, rotation } => {
            w.u8(4);
            w.u8(kind as u8);
            w.tile(at);
            w.u8(rotation);
        }
        Command::SetSignal { node, on } => {
            w.u8(5);
            w.u32(node.index);
            w.u16(node.generation);
            w.u8(u8::from(on));
        }
        Command::BuyChunk { coord } => {
            w.u8(6);
            w.i32(coord.cx);
            w.i32(coord.cy);
        }
        Command::SetTaxRate { zone, permille } => {
            w.u8(7);
            w.u8(zone as u8);
            w.u16(permille);
        }
        Command::SetSimSpeed { speed } => {
            w.u8(8);
            w.u16(speed.multiplier());
        }
    }
}

fn decode_command(r: &mut Reader<'_>) -> Result<Command, DecodeError> {
    let c = match r.u8()? {
        1 => Command::BuildRoad {
            from: r.tile()?,
            to: r.tile()?,
            kind: r.code("RoadKind", RoadKind::from_code)?,
        },
        2 => Command::Bulldoze { area: r.rect()? },
        3 => Command::PaintZone {
            area: r.rect()?,
            zone: r.code("ZoneKind", ZoneKind::from_code)?,
        },
        4 => Command::PlaceBuilding {
            kind: r.code("ServiceKind", ServiceKind::from_code)?,
            at: r.tile()?,
            rotation: r.u8()?,
        },
        5 => {
            let node = NodeId {
                index: r.u32()?,
                generation: r.u16()?,
            };
            let on = r.u8()?;
            let on = r.check(on <= 1, "SetSignal の on", on == 1)?;
            Command::SetSignal { node, on }
        }
        6 => Command::BuyChunk {
            coord: ChunkCoord::new(r.i32()?, r.i32()?),
        },
        7 => Command::SetTaxRate {
            zone: r.code("ZoneKind", ZoneKind::from_code)?,
            permille: r.u16()?,
        },
        8 => {
            let speed = SimSpeed::new(r.u16()?).ok_or(DecodeError::InvalidValue {
                what: "速度",
                at: r.pos,
            })?;
            Command::SetSimSpeed { speed }
        }
        other => return Err(DecodeError::UnknownCommand(other)),
    };
    Ok(c)
}

/// 再生のあいだに適用したコマンドの件数。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReplayStats {
    pub applied: u64,
    pub not_implemented: u64,
    pub rejected: u64,
    pub denied: u64,
}

/// `world` を `ticks` だけ進め、そのあいだの tick に記録されたコマンドを各 tick の頭で適用する。
///
/// `world` のいまの tick より前に記録されたコマンドは、適用済みとみなして飛ばす。
pub fn replay(world: &mut World, log: &CommandLog, ticks: u64) -> ReplayStats {
    let entries = log.entries();
    let mut next = entries.partition_point(|e| e.tick < world.tick());
    let mut stats = ReplayStats::default();
    for _ in 0..ticks {
        while let Some(e) = entries.get(next)
            && e.tick == world.tick()
        {
            match world.apply(&e.command) {
                ApplyOutcome::Applied => stats.applied += 1,
                ApplyOutcome::NotImplemented => stats.not_implemented += 1,
                ApplyOutcome::Rejected => stats.rejected += 1,
                ApplyOutcome::Denied => stats.denied += 1,
            }
            next += 1;
        }
        world.step();
    }
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::MAX_TAX_PERMILLE;
    use proptest::prelude::*;

    fn sample_commands() -> Vec<Command> {
        vec![
            Command::BuildRoad {
                from: TilePos::new(-3, 4),
                to: TilePos::new(5, -4),
                kind: RoadKind::Medium,
            },
            Command::Bulldoze {
                area: TileRect::from_corners(TilePos::new(5, 5), TilePos::new(-1, 9)),
            },
            Command::PaintZone {
                area: TileRect::from_corners(TilePos::new(0, 0), TilePos::new(3, 3)),
                zone: ZoneKind::Industrial,
            },
            Command::PlaceBuilding {
                kind: ServiceKind::WaterPump,
                at: TilePos::new(7, -7),
                rotation: 3,
            },
            Command::SetSignal {
                node: NodeId {
                    index: 42,
                    generation: 2,
                },
                on: true,
            },
            Command::BuyChunk {
                coord: ChunkCoord::new(i32::MIN, 1),
            },
            Command::SetTaxRate {
                zone: ZoneKind::Commercial,
                permille: MAX_TAX_PERMILLE,
            },
            Command::SetSimSpeed {
                speed: SimSpeed::MAX,
            },
        ]
    }

    fn sample_log() -> CommandLog {
        let mut log = CommandLog::new();
        for (i, c) in sample_commands().into_iter().enumerate() {
            log.record(Tick(i as u64 / 3), c).unwrap();
        }
        log
    }

    #[test]
    fn roundtrip_every_command() {
        let log = sample_log();
        assert_eq!(CommandLog::decode(&log.encode()), Ok(log));
    }

    #[test]
    fn encodes_the_documented_bytes() {
        let mut log = CommandLog::new();
        log.record(
            Tick(2),
            Command::SetTaxRate {
                zone: ZoneKind::Office,
                permille: 0x0102,
            },
        )
        .unwrap();
        let mut expected = b"CHIZUREC".to_vec();
        expected.extend_from_slice(&1_u32.to_le_bytes());
        expected.extend_from_slice(&1_u64.to_le_bytes());
        expected.extend_from_slice(&2_u64.to_le_bytes());
        expected.extend_from_slice(&0_u64.to_le_bytes());
        expected.extend_from_slice(&[7, 4, 0x02, 0x01]);
        assert_eq!(log.encode(), expected);
    }

    #[test]
    fn empty_log_roundtrips() {
        let log = CommandLog::new();
        assert_eq!(CommandLog::decode(&log.encode()), Ok(log));
    }

    #[test]
    fn rejects_bad_header_and_tail() {
        let bytes = sample_log().encode();
        let mut bad = bytes.clone();
        bad[0] = b'X';
        assert_eq!(CommandLog::decode(&bad), Err(DecodeError::BadMagic));

        let mut bad = bytes.clone();
        bad[8] = 2;
        assert_eq!(
            CommandLog::decode(&bad),
            Err(DecodeError::UnsupportedVersion(2))
        );

        let mut bad = bytes.clone();
        bad.push(0);
        assert_eq!(CommandLog::decode(&bad), Err(DecodeError::TrailingBytes(1)));

        let bad = &bytes[..bytes.len() - 1];
        assert!(matches!(
            CommandLog::decode(bad),
            Err(DecodeError::UnexpectedEnd(_))
        ));
    }

    fn single(tick: u64, seq: u64, tail: &[u8]) -> Vec<u8> {
        let mut b = b"CHIZUREC".to_vec();
        b.extend_from_slice(&1_u32.to_le_bytes());
        b.extend_from_slice(&1_u64.to_le_bytes());
        b.extend_from_slice(&tick.to_le_bytes());
        b.extend_from_slice(&seq.to_le_bytes());
        b.extend_from_slice(tail);
        b
    }

    #[test]
    fn rejects_out_of_range_values() {
        let invalid = |tail: &[u8]| {
            matches!(
                CommandLog::decode(&single(0, 0, tail)),
                Err(DecodeError::InvalidValue { .. })
            )
        };
        let invalid_command = |tail: &[u8]| {
            matches!(
                CommandLog::decode(&single(0, 0, tail)),
                Err(DecodeError::InvalidCommand { .. })
            )
        };
        assert!(invalid_command(&[7, 1, 0x2d, 0x01])); // 税率 301‰
        assert!(invalid(&[7, 0, 0, 0])); // 区画の種類 0
        assert!(invalid(&[8, 0, 0])); // 速度 0 倍
        assert!(invalid(&[8, 49, 0])); // 速度 49 倍
        let mut signal = vec![5];
        signal.extend_from_slice(&[0; 6]);
        signal.push(2);
        assert!(invalid(&signal));
        let mut rect = vec![2];
        rect.extend_from_slice(&1_i64.to_le_bytes());
        rect.extend_from_slice(&0_i64.to_le_bytes());
        rect.extend_from_slice(&0_i64.to_le_bytes());
        rect.extend_from_slice(&0_i64.to_le_bytes());
        assert!(invalid_command(&rect));
        assert_eq!(
            CommandLog::decode(&single(0, 0, &[9])),
            Err(DecodeError::UnknownCommand(9))
        );
    }

    #[test]
    fn rejects_out_of_order_entries() {
        let mut log = CommandLog::new();
        log.record(
            Tick(5),
            Command::SetSimSpeed {
                speed: SimSpeed::NORMAL,
            },
        )
        .unwrap();
        let err = log
            .record(
                Tick(4),
                Command::SetSimSpeed {
                    speed: SimSpeed::NORMAL,
                },
            )
            .unwrap_err();
        assert!(matches!(err, RecordError::OutOfOrder(e) if e.next == (Tick(4), 1)));
        let dup = RecordedCommand {
            tick: Tick(5),
            seq: 0,
            command: Command::SetSimSpeed {
                speed: SimSpeed::NORMAL,
            },
        };
        assert!(log.push(dup).is_err());
        // tick が進んでも、番号が戻るのは並びの崩れである。
        let seq_back = RecordedCommand {
            tick: Tick(6),
            ..dup
        };
        assert!(matches!(
            log.push(seq_back),
            Err(RecordError::OutOfOrder(_))
        ));
        assert_eq!(log.len(), 1);
    }

    #[test]
    fn decode_rejects_seq_going_back_across_ticks() {
        let mut b = b"CHIZUREC".to_vec();
        b.extend_from_slice(&1_u32.to_le_bytes());
        b.extend_from_slice(&2_u64.to_le_bytes());
        for (tick, seq) in [(1_u64, 5_u64), (2, 0)] {
            b.extend_from_slice(&tick.to_le_bytes());
            b.extend_from_slice(&seq.to_le_bytes());
            b.extend_from_slice(&[8, 1, 0]);
        }
        assert!(matches!(
            CommandLog::decode(&b),
            Err(DecodeError::OutOfOrder(_))
        ));
    }

    #[test]
    fn record_rejects_invalid_commands() {
        let mut log = CommandLog::new();
        let bad = Command::SetTaxRate {
            zone: ZoneKind::Office,
            permille: MAX_TAX_PERMILLE + 1,
        };
        assert_eq!(
            log.record(Tick(0), bad),
            Err(RecordError::Invalid(InvalidCommand::TaxRateTooHigh(
                MAX_TAX_PERMILLE + 1
            )))
        );
        let rect = TileRect {
            min: TilePos::new(0, 1),
            max: TilePos::new(0, 0),
        };
        assert!(matches!(
            log.record(Tick(0), Command::Bulldoze { area: rect }),
            Err(RecordError::Invalid(_))
        ));
        assert!(log.is_empty());
    }

    #[test]
    fn record_after_last_seq_returns_error() {
        let mut log = CommandLog::new();
        let c = Command::SetSimSpeed {
            speed: SimSpeed::NORMAL,
        };
        log.push(RecordedCommand {
            tick: Tick(0),
            seq: u64::MAX,
            command: c,
        })
        .unwrap();
        assert_eq!(log.record(Tick(0), c), Err(RecordError::SeqExhausted));
        assert_eq!(log.len(), 1);
    }

    #[test]
    fn same_tick_commands_apply_in_seq_order() {
        let set = |permille| Command::SetTaxRate {
            zone: ZoneKind::Residential,
            permille,
        };
        let mut log = CommandLog::new();
        log.record(Tick(1), set(50)).unwrap();
        log.record(Tick(1), set(70)).unwrap();
        let mut w = World::new(0);
        replay(&mut w, &log, 2);
        assert_eq!(w.tax_permille(ZoneKind::Residential), 70);
    }

    #[test]
    fn commands_apply_at_the_head_of_their_tick() {
        let mut log = CommandLog::new();
        log.record(
            Tick(3),
            Command::SetSimSpeed {
                speed: SimSpeed::MAX,
            },
        )
        .unwrap();
        let mut w = World::new(0);
        replay(&mut w, &log, 3);
        assert_eq!(w.speed(), SimSpeed::NORMAL);
        replay(&mut w, &log, 1);
        assert_eq!(w.speed(), SimSpeed::MAX);
        assert_eq!(w.tick(), Tick(4));
    }

    #[test]
    fn replay_in_pieces_matches_replay_in_one_go() {
        let log = sample_log();
        let mut whole = World::new(9);
        let stats = replay(&mut whole, &log, 10);
        assert_eq!(
            stats,
            ReplayStats {
                applied: 3,
                not_implemented: 3,
                rejected: 0,
                denied: 2,
            }
        );
        let mut pieces = World::new(9);
        for _ in 0..10 {
            replay(&mut pieces, &log, 1);
        }
        assert_eq!(whole, pieces);
    }

    fn arb_command() -> impl Strategy<Value = Command> {
        let tile = (any::<i64>(), any::<i64>()).prop_map(|(x, y)| TilePos::new(x, y));
        let rect = (tile.clone(), tile.clone()).prop_map(|(a, b)| TileRect::from_corners(a, b));
        let zone = proptest::sample::select(ZoneKind::ALL.to_vec());
        prop_oneof![
            (
                -1_000_i64..1_000,
                -1_000_i64..1_000,
                -1_i64..=1,
                -1_i64..=1,
                1_i64..=1_024,
                1_u8..=3
            )
                .prop_filter("向きがない", |(_, _, sx, sy, _, _)| (*sx, *sy)
                    != (0, 0))
                .prop_map(|(x, y, sx, sy, n, k)| Command::BuildRoad {
                    from: TilePos::new(x, y),
                    to: TilePos::new(x + sx * n, y + sy * n),
                    kind: RoadKind::from_code(k).unwrap(),
                }),
            rect.clone().prop_map(|area| Command::Bulldoze { area }),
            (rect, zone.clone()).prop_map(|(area, zone)| Command::PaintZone { area, zone }),
            (1_u8..=2, tile, any::<u8>()).prop_map(|(k, at, rotation)| Command::PlaceBuilding {
                kind: ServiceKind::from_code(k).unwrap(),
                at,
                rotation,
            }),
            (any::<u32>(), any::<u16>(), any::<bool>()).prop_map(|(index, generation, on)| {
                Command::SetSignal {
                    node: NodeId { index, generation },
                    on,
                }
            }),
            (any::<i32>(), any::<i32>()).prop_map(|(cx, cy)| Command::BuyChunk {
                coord: ChunkCoord::new(cx, cy),
            }),
            (zone, 0..=MAX_TAX_PERMILLE)
                .prop_map(|(zone, permille)| Command::SetTaxRate { zone, permille }),
            (1_u16..=48).prop_map(|m| Command::SetSimSpeed {
                speed: SimSpeed::new(m).unwrap(),
            }),
        ]
    }

    proptest! {
        #[test]
        fn roundtrip_any_log(steps in proptest::collection::vec((0_u64..3, arb_command()), 0..40)) {
            let mut log = CommandLog::new();
            let mut tick = 0;
            for (dt, c) in steps {
                tick += dt;
                log.record(Tick(tick), c).unwrap();
            }
            prop_assert_eq!(CommandLog::decode(&log.encode()), Ok(log));
        }

        #[test]
        fn decode_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..200)) {
            let _ = CommandLog::decode(&bytes);
        }

        #[test]
        fn decode_never_panics_after_header(tail in proptest::collection::vec(any::<u8>(), 0..200)) {
            let mut b = b"CHIZUREC".to_vec();
            b.extend_from_slice(&1_u32.to_le_bytes());
            b.extend_from_slice(&tail);
            let _ = CommandLog::decode(&b);
        }
    }
}
