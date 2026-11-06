//! 描画用のスナップショットを書く三重バッファ。
//!
//! 枠は WASM のメモリの中に置く。WASM のメモリは SharedArrayBuffer なので、メインスレッドは
//! [`TripleBuffer::byte_offset`] の位置に型つき配列を重ねて、そのまま読む。
//!
//! 3 つの枠は「書き込み中」「最新」「読み込み中」の役を入れ替えながら使う。
//! 書き込み中の枠の番号は書き手だけが、読み込み中の枠の番号は読み手だけが手元に持つ。
//! 共有するのは制御語 [`LATEST`] の 1 語だけで、最新の枠の番号と、読み手がまだ受け取っていない
//! ことを表す [`FRESH`] のビットを入れる。どちらの側も、この語を交換（exchange）で書き換える。
//!
//! - 書き手は書き込み中の枠を書き終えたら、`LATEST` を「その枠の番号 | FRESH」と交換し、
//!   返ってきた番号の枠を次の書き込み中の枠にする。待つことはない。
//! - 読み手は `LATEST` に FRESH が立っているときだけ、`LATEST` を「読み込み中の枠の番号」と
//!   交換し、返ってきた番号の枠を新しい読み込み中の枠にする。
//!
//! 語はすべて 32 ビットで、配置は `web/src/protocol/snapshot-layout.json` と一致させる。

use std::sync::atomic::{AtomicU32, Ordering};

/// 配置の版。配置を変えたら上げる。
pub const LAYOUT_VERSION: u32 = 5;

pub const SLOT_COUNT: u32 = 3;
/// 枠の前に置く制御の語の数。
pub const CONTROL_WORDS: usize = 4;
/// 制御の語のうち、最新の枠の番号と FRESH を入れる語。
pub const LATEST: usize = 0;
/// 制御の語のうち、配置の版を入れる語。
pub const VERSION: usize = 1;
/// 制御語の下位ビットのうち、枠の番号を表すビット。
pub const INDEX_MASK: u32 = 0b011;
/// 読み手がまだ受け取っていない枠が最新の枠にあることを表すビット。
pub const FRESH: u32 = 0b100;

/// 書き手、最新、読み手が最初に持つ枠の番号。
pub const INITIAL_WRITER: u32 = 0;
pub const INITIAL_LATEST: u32 = 1;
pub const INITIAL_READER: u32 = 2;

/// 枠の先頭の数値の、枠の中での語の位置。
pub mod header {
    pub const TICK_LO: usize = 0;
    pub const TICK_HI: usize = 1;
    pub const DAY: usize = 2;
    pub const SECOND_OF_DAY: usize = 3;
    pub const MONEY_LO: usize = 4;
    pub const MONEY_HI: usize = 5;
    pub const POPULATION: usize = 6;
    pub const SPEED: usize = 7;
    pub const PAUSED: usize = 8;
    pub const MOVER_COUNT: usize = 9;
    /// 移動体の座標の基準にするチャンクの座標（`i32` のビット列）。
    pub const BASE_CX: usize = 10;
    pub const BASE_CY: usize = 11;
    /// 列車の車両の数。車両は移動体の枠のあとに、移動体と同じ語の並びで置く。
    pub const TRAIN_CAR_COUNT: usize = 12;
    /// 車両の座標の基準にするチャンクの座標（`i32` のビット列）。移動体の基準とは別に決める。
    pub const TRAIN_BASE_CX: usize = 13;
    pub const TRAIN_BASE_CY: usize = 14;
}

/// 枠の先頭の数値に充てる語の数。移動体の配列の始まりが、枠の先頭から 16 バイトの倍数の位置に来るよう、
/// 4 の倍数にする。バッファそのものの先頭の位置は 4 バイト境界にしか乗せていない。
pub const SLOT_HEADER_WORDS: usize = 16;

/// 移動体 1 体の語の数（32 バイト）。中の語の位置は [`mover`] にある。
pub const MOVER_WORDS: usize = 8;

/// 移動体の、1 体の中での語の位置。
pub mod mover {
    /// 基準のチャンク（[`header::BASE_CX`]）の原点からの x。マスを単位にした `f32` のビット列。
    pub const X: usize = 0;
    /// 基準のチャンクの原点からの y。マスを単位にした `f32` のビット列。
    pub const Y: usize = 1;
    /// 下位 16 ビットが向き、上位 16 ビットが種類と色。
    pub const HEADING_KIND: usize = 2;
    /// 移動の添え字。添え字は使い回されるので、世代と組にして移動を区別する。
    pub const ID: usize = 3;
    /// 移動の世代。
    pub const GENERATION: usize = 4;
    /// 描いた道筋の番号（`car_path::Paths`）。道筋の上に描いていなければ 0。
    pub const PATH: usize = 5;
    /// 前の tick の道筋からこの道筋までのあいだに、丸ごと通り抜けた道筋の番号。なければ 0。
    pub const VIA: usize = 6;
    /// 道筋の上で進んだ割合（0〜1）。`f32` のビット列。
    pub const FRAC: usize = 7;
}

/// 1 枠に入る移動体の上限。
pub const MAX_MOVERS: usize = 20_000;

/// 1 枠に入る車両の上限。路線の上限（64）× 1 路線の列車の上限（16）× 1 編成の両数（4）である。
/// 車両 1 両の語の並びは移動体と同じ（[`mover`]）。種類の上位ビットは列車（3）と路線の色の番号、
/// 移動の添え字は `列車の添え字 × 4 + 先頭からの順番`、世代は列車の世代を入れる。
pub const MAX_TRAIN_CARS: usize = 4_096;

/// 車両の並びが始まる、枠の中での語の位置。
pub const CARS_AT: usize = SLOT_HEADER_WORDS + MAX_MOVERS * MOVER_WORDS;

pub const SLOT_WORDS: usize = CARS_AT + MAX_TRAIN_CARS * MOVER_WORDS;
pub const TOTAL_WORDS: usize = CONTROL_WORDS + SLOT_COUNT as usize * SLOT_WORDS;

const _: () = assert!(SLOT_HEADER_WORDS.is_multiple_of(4));
const _: () = assert!(CONTROL_WORDS.is_multiple_of(4));
const _: () = assert!(INDEX_MASK & FRESH == 0 && SLOT_COUNT <= INDEX_MASK + 1);

/// 枠の先頭に置く数値。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Header {
    pub tick: u64,
    pub day: u32,
    pub second_of_day: u32,
    pub money: i64,
    pub population: u32,
    pub speed: u16,
    pub paused: bool,
    /// 移動体の座標の基準にするチャンク。
    pub base_cx: i32,
    pub base_cy: i32,
    /// 車両の座標の基準にするチャンク。
    pub train_base_cx: i32,
    pub train_base_cy: i32,
}

/// 描画用の移動体。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mover {
    pub x: f32,
    pub y: f32,
    pub heading: u16,
    pub kind: u16,
    pub id: u32,
    pub generation: u32,
    /// 描いた道筋の番号。道筋の上に描いていなければ 0。
    pub path: u32,
    /// 前の tick の道筋からこの道筋までのあいだに、丸ごと通り抜けた道筋の番号。なければ 0。
    pub via: u32,
    /// 道筋の上で進んだ割合。
    pub frac: f32,
}

/// 書き手の側の三重バッファ。
pub struct TripleBuffer {
    words: Box<[AtomicU32]>,
    writing: u32,
}

impl Default for TripleBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl TripleBuffer {
    pub fn new() -> TripleBuffer {
        let words: Box<[AtomicU32]> = (0..TOTAL_WORDS).map(|_| AtomicU32::new(0)).collect();
        words[LATEST].store(INITIAL_LATEST, Ordering::Relaxed);
        words[VERSION].store(LAYOUT_VERSION, Ordering::Relaxed);
        TripleBuffer {
            words,
            writing: INITIAL_WRITER,
        }
    }

    /// 先頭の語の、WASM のメモリの中でのバイト位置。値は作ってから変わらない。
    pub fn byte_offset(&self) -> usize {
        self.words.as_ptr() as usize
    }

    /// 書き込み中の枠に書き、最新の枠として渡す。`movers` と `cars` が上限を超えた分は捨てる。
    /// `cars` は列車の車両で、移動体と同じ形で書く。
    pub fn publish(&mut self, header: &Header, movers: &[Mover], cars: &[Mover]) {
        let movers = &movers[..movers.len().min(MAX_MOVERS)];
        let cars = &cars[..cars.len().min(MAX_TRAIN_CARS)];
        let base = CONTROL_WORDS + self.writing as usize * SLOT_WORDS;
        let slot = &self.words[base..base + SLOT_WORDS];
        let put = |i: usize, v: u32| slot[i].store(v, Ordering::Relaxed);

        put(header::TICK_LO, header.tick as u32);
        put(header::TICK_HI, (header.tick >> 32) as u32);
        put(header::DAY, header.day);
        put(header::SECOND_OF_DAY, header.second_of_day);
        put(header::MONEY_LO, header.money as u32);
        put(header::MONEY_HI, (header.money >> 32) as u32);
        put(header::POPULATION, header.population);
        put(header::SPEED, u32::from(header.speed));
        put(header::PAUSED, u32::from(header.paused));
        // 上限で切ってあるので u32 に収まる。
        put(header::MOVER_COUNT, movers.len() as u32);
        put(header::BASE_CX, header.base_cx as u32);
        put(header::BASE_CY, header.base_cy as u32);
        // 上限で切ってあるので u32 に収まる。
        put(header::TRAIN_CAR_COUNT, cars.len() as u32);
        put(header::TRAIN_BASE_CX, header.train_base_cx as u32);
        put(header::TRAIN_BASE_CY, header.train_base_cy as u32);

        let write = |at: usize, m: &Mover| {
            put(at + mover::X, m.x.to_bits());
            put(at + mover::Y, m.y.to_bits());
            put(
                at + mover::HEADING_KIND,
                u32::from(m.heading) | (u32::from(m.kind) << 16),
            );
            put(at + mover::ID, m.id);
            put(at + mover::GENERATION, m.generation);
            put(at + mover::PATH, m.path);
            put(at + mover::VIA, m.via);
            put(at + mover::FRAC, m.frac.to_bits());
        };
        for (i, m) in movers.iter().enumerate() {
            write(SLOT_HEADER_WORDS + i * MOVER_WORDS, m);
        }
        for (i, m) in cars.iter().enumerate() {
            write(CARS_AT + i * MOVER_WORDS, m);
        }

        let previous = self.words[LATEST].swap(self.writing | FRESH, Ordering::AcqRel);
        self.writing = previous & INDEX_MASK;
    }
}

/// 配置を JSON で表す。`web/src/protocol/snapshot-layout.json` はこの出力そのものである。
pub fn layout_json() -> String {
    format!(
        r#"{{
  "version": {LAYOUT_VERSION},
  "slotCount": {SLOT_COUNT},
  "controlWords": {CONTROL_WORDS},
  "control": {{
    "latest": {LATEST},
    "version": {VERSION}
  }},
  "indexMask": {INDEX_MASK},
  "fresh": {FRESH},
  "initial": {{
    "writer": {INITIAL_WRITER},
    "latest": {INITIAL_LATEST},
    "reader": {INITIAL_READER}
  }},
  "slotHeaderWords": {SLOT_HEADER_WORDS},
  "header": {{
    "tickLo": {},
    "tickHi": {},
    "day": {},
    "secondOfDay": {},
    "moneyLo": {},
    "moneyHi": {},
    "population": {},
    "speed": {},
    "paused": {},
    "moverCount": {},
    "baseCx": {},
    "baseCy": {},
    "trainCarCount": {},
    "trainBaseCx": {},
    "trainBaseCy": {}
  }},
  "moverWords": {MOVER_WORDS},
  "mover": {{
    "x": {},
    "y": {},
    "headingKind": {},
    "id": {},
    "generation": {},
    "path": {},
    "via": {},
    "frac": {}
  }},
  "maxMovers": {MAX_MOVERS},
  "maxTrainCars": {MAX_TRAIN_CARS},
  "carsStart": {CARS_AT},
  "slotWords": {SLOT_WORDS},
  "totalWords": {TOTAL_WORDS}
}}
"#,
        header::TICK_LO,
        header::TICK_HI,
        header::DAY,
        header::SECOND_OF_DAY,
        header::MONEY_LO,
        header::MONEY_HI,
        header::POPULATION,
        header::SPEED,
        header::PAUSED,
        header::MOVER_COUNT,
        header::BASE_CX,
        header::BASE_CY,
        header::TRAIN_CAR_COUNT,
        header::TRAIN_BASE_CX,
        header::TRAIN_BASE_CY,
        mover::X,
        mover::Y,
        mover::HEADING_KIND,
        mover::ID,
        mover::GENERATION,
        mover::PATH,
        mover::VIA,
        mover::FRAC,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAYOUT_FILE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../web/src/protocol/snapshot-layout.json"
    );

    #[test]
    fn layout_file_matches() {
        let expected = layout_json();
        if std::env::var_os("CHIZU_UPDATE_LAYOUT").is_some() {
            std::fs::write(LAYOUT_FILE, &expected).unwrap();
        }
        let actual = std::fs::read_to_string(LAYOUT_FILE).unwrap();
        assert!(
            actual == expected,
            "{LAYOUT_FILE} が Rust 側の配置と食い違う。\
             CHIZU_UPDATE_LAYOUT=1 cargo test -p chizu-wasm で書き直す"
        );
    }

    const FIXTURE_FILE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../web/src/protocol/snapshot-fixture.bin"
    );

    /// 画面の側のテストが読む枠の値。`web/src/protocol/snapshot.test.ts` にも同じ値を書く。
    fn fixture_header() -> Header {
        Header {
            tick: (1 << 32) + 7,
            day: 3,
            second_of_day: 45_672,
            money: -1_234_567_890_123,
            population: 42,
            speed: 48,
            paused: true,
            base_cx: -3,
            base_cy: 5,
            train_base_cx: 2,
            train_base_cy: -7,
        }
    }

    /// 2 回書いたあとの、制御の語と各枠の先頭の数値を、リトルエンディアンで並べる。
    /// 読み手は最初の枠を持ったまま、2 回目に書いた枠を受け取ることになる。
    fn fixture_bytes() -> Vec<u8> {
        let mut buf = TripleBuffer::new();
        buf.publish(&header(1), &[], &[]);
        buf.publish(&fixture_header(), &[], &[]);
        let mut words: Vec<u32> = (0..CONTROL_WORDS)
            .map(|i| buf.words[i].load(Ordering::Relaxed))
            .collect();
        for slot in 0..SLOT_COUNT as usize {
            let base = CONTROL_WORDS + slot * SLOT_WORDS;
            words.extend(
                (0..SLOT_HEADER_WORDS).map(|i| buf.words[base + i].load(Ordering::Relaxed)),
            );
        }
        words.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    #[test]
    fn fixture_file_matches() {
        let expected = fixture_bytes();
        if std::env::var_os("CHIZU_UPDATE_LAYOUT").is_some() {
            std::fs::write(FIXTURE_FILE, &expected).unwrap();
        }
        let actual = std::fs::read(FIXTURE_FILE).unwrap();
        assert!(
            actual == expected,
            "{FIXTURE_FILE} が Rust 側の書き方と食い違う。\
             CHIZU_UPDATE_LAYOUT=1 cargo test -p chizu-wasm で書き直す"
        );
    }

    /// 読み手の手順を Rust で写したもの。
    struct Reader {
        reading: u32,
    }

    impl Reader {
        fn acquire(&mut self, buf: &TripleBuffer) -> u32 {
            if buf.words[LATEST].load(Ordering::Acquire) & FRESH != 0 {
                self.reading = buf.words[LATEST].swap(self.reading, Ordering::AcqRel) & INDEX_MASK;
            }
            self.reading
        }

        fn word(&self, buf: &TripleBuffer, i: usize) -> u32 {
            buf.words[CONTROL_WORDS + self.reading as usize * SLOT_WORDS + i]
                .load(Ordering::Relaxed)
        }
    }

    fn header(tick: u64) -> Header {
        Header {
            tick,
            money: -5,
            ..Header::default()
        }
    }

    #[test]
    fn reader_sees_latest_publish_and_never_the_writing_slot() {
        let mut buf = TripleBuffer::new();
        let mut r = Reader {
            reading: INITIAL_READER,
        };
        assert_eq!(r.acquire(&buf), INITIAL_READER);

        for tick in 1..=10_u64 {
            buf.publish(&header(tick), &[], &[]);
            if tick % 3 == 0 {
                continue;
            }
            let slot = r.acquire(&buf);
            assert_ne!(slot, buf.writing);
            assert_eq!(r.word(&buf, header::TICK_LO), tick as u32);
            assert_eq!(r.word(&buf, header::MONEY_LO), -5_i64 as u32);
            assert_eq!(r.word(&buf, header::MONEY_HI), u32::MAX);
            // 新しく書かれていなければ、同じ枠を読み続ける。
            assert_eq!(r.acquire(&buf), slot);
        }
    }

    #[test]
    fn writes_movers() {
        let mut buf = TripleBuffer::new();
        let m = Mover {
            x: 1.5,
            y: -2.0,
            heading: 0x1234,
            kind: 0xabcd,
            id: 7,
            generation: 3,
            path: 11,
            via: 5,
            frac: 0.25,
        };
        buf.publish(&header(0), &[m], &[]);
        let mut r = Reader {
            reading: INITIAL_READER,
        };
        r.acquire(&buf);
        assert_eq!(r.word(&buf, header::MOVER_COUNT), 1);
        let at = SLOT_HEADER_WORDS;
        assert_eq!(f32::from_bits(r.word(&buf, at + mover::X)), 1.5);
        assert_eq!(f32::from_bits(r.word(&buf, at + mover::Y)), -2.0);
        assert_eq!(r.word(&buf, at + mover::HEADING_KIND), 0xabcd_1234);
        assert_eq!(r.word(&buf, at + mover::ID), 7);
        assert_eq!(r.word(&buf, at + mover::GENERATION), 3);
        assert_eq!(r.word(&buf, at + mover::PATH), 11);
        assert_eq!(r.word(&buf, at + mover::VIA), 5);
        assert_eq!(f32::from_bits(r.word(&buf, at + mover::FRAC)), 0.25);
    }

    #[test]
    fn writes_cars_after_the_movers_with_their_own_base() {
        let mut buf = TripleBuffer::new();
        let mover = Mover {
            x: 1.0,
            y: 2.0,
            heading: 3,
            kind: 2,
            id: 9,
            generation: 0,
            path: 0,
            via: 0,
            frac: 0.0,
        };
        let car = Mover {
            x: -4.5,
            y: 6.25,
            heading: 0x4000,
            kind: 3 | (5 << 2),
            id: 13,
            generation: 2,
            path: 0,
            via: 0,
            frac: 0.0,
        };
        let header = Header {
            train_base_cx: -2,
            train_base_cy: 7,
            ..header(0)
        };
        buf.publish(&header, &[mover], &[car, car]);
        let mut r = Reader {
            reading: INITIAL_READER,
        };
        r.acquire(&buf);
        assert_eq!(r.word(&buf, header::MOVER_COUNT), 1);
        assert_eq!(r.word(&buf, header::TRAIN_CAR_COUNT), 2);
        assert_eq!(r.word(&buf, header::TRAIN_BASE_CX) as i32, -2);
        assert_eq!(r.word(&buf, header::TRAIN_BASE_CY) as i32, 7);
        for i in 0..2 {
            let at = CARS_AT + i * MOVER_WORDS;
            assert_eq!(f32::from_bits(r.word(&buf, at + mover::X)), -4.5);
            assert_eq!(f32::from_bits(r.word(&buf, at + mover::Y)), 6.25);
            assert_eq!(r.word(&buf, at + mover::HEADING_KIND), (23 << 16) | 0x4000);
            assert_eq!(r.word(&buf, at + mover::ID), 13);
            assert_eq!(r.word(&buf, at + mover::GENERATION), 2);
        }
        // 移動体の欄は車両に上書きされない
        assert_eq!(r.word(&buf, SLOT_HEADER_WORDS + mover::ID), 9);
    }

    #[test]
    fn cars_over_the_limit_are_dropped() {
        let mut buf = TripleBuffer::new();
        let car = Mover {
            x: 0.0,
            y: 0.0,
            heading: 0,
            kind: 3,
            id: 0,
            generation: 0,
            path: 0,
            via: 0,
            frac: 0.0,
        };
        buf.publish(&header(0), &[], &vec![car; MAX_TRAIN_CARS + 10]);
        let mut r = Reader {
            reading: INITIAL_READER,
        };
        r.acquire(&buf);
        assert_eq!(
            r.word(&buf, header::TRAIN_CAR_COUNT) as usize,
            MAX_TRAIN_CARS
        );
    }
}
