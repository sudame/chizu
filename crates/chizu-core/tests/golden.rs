//! ゴールデンのハッシュ。
//!
//! 決まった記録を再生した結果の状態ハッシュが、`tests/golden/<名前>.hash` と一致するかを見る。
//! 記録そのものも `tests/golden/<名前>.rec` に置き、書き出しの形式が変わっていないかも見る。
//!
//! シミュレーションや記録の形式を意図して変えたときは、`CHIZU_UPDATE_GOLDEN=1 cargo test -p chizu-core --test golden`
//! でファイルを書き直し、同じ PR に含める。

use std::path::PathBuf;

use chizu_core::{
    ChunkCoord, Command, CommandLog, RoadKind, SimSpeed, Tick, TilePos, TileRect, World, ZoneKind,
    replay,
};

struct Scenario {
    name: &'static str,
    seed: u64,
    ticks: u64,
    log: CommandLog,
}

fn scenarios() -> Vec<Scenario> {
    vec![Scenario {
        name: "clock_and_commands",
        seed: 42,
        ticks: 2 * chizu_core::TICKS_PER_DAY,
        log: clock_and_commands(),
    }]
}

/// 速度と税率を何度か変え、中身のまだないコマンドも混ぜた 2 日ぶんの記録。
fn clock_and_commands() -> CommandLog {
    let mut log = CommandLog::new();
    let mut rec = |tick: u64, c: Command| {
        log.record(Tick(tick), c).unwrap();
    };
    let speed = |m| Command::SetSimSpeed {
        speed: SimSpeed::new(m).unwrap(),
    };
    let tax = |zone, permille| Command::SetTaxRate { zone, permille };

    rec(
        0,
        Command::BuildRoad {
            from: TilePos::new(0, 0),
            to: TilePos::new(20, 0),
            kind: RoadKind::Small,
        },
    );
    rec(
        0,
        Command::PaintZone {
            area: TileRect::from_corners(TilePos::new(0, 1), TilePos::new(20, 4)),
            zone: ZoneKind::Residential,
        },
    );
    rec(30, tax(ZoneKind::Residential, 120));
    rec(30, tax(ZoneKind::Residential, 90));
    rec(600, speed(3));
    rec(2_400, speed(8));
    rec(2_400, tax(ZoneKind::Industrial, 150));
    rec(
        5_000,
        Command::BuyChunk {
            coord: ChunkCoord::new(1, 0),
        },
    );
    rec(7_200, speed(48));
    rec(9_000, tax(ZoneKind::Office, 0));
    rec(12_000, speed(1));
    // 再生する範囲より後なので適用されない。
    rec(20_000, speed(2));
    log
}

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

fn updating() -> bool {
    std::env::var_os("CHIZU_UPDATE_GOLDEN").is_some_and(|v| v == "1")
}

#[test]
fn golden_hashes_match() {
    let dir = golden_dir();
    let mut mismatches = Vec::new();
    for s in scenarios() {
        let rec_path = dir.join(format!("{}.rec", s.name));
        let hash_path = dir.join(format!("{}.hash", s.name));
        let encoded = s.log.encode();

        let mut world = World::new(s.seed);
        replay(&mut world, &s.log, s.ticks);
        let got = format!("{:016x}\n", world.state_hash());

        if updating() {
            std::fs::write(&rec_path, &encoded).unwrap();
            std::fs::write(&hash_path, &got).unwrap();
            continue;
        }

        let stored = std::fs::read(&rec_path)
            .unwrap_or_else(|e| panic!("{} を読めない: {e}", rec_path.display()));
        if stored != encoded {
            mismatches.push(format!("{}: 記録の書き出しが変わった", s.name));
        }
        // 保存した記録を読み戻して再生しても、同じ結果になる。
        let decoded = CommandLog::decode(&stored).unwrap();
        let mut from_file = World::new(s.seed);
        replay(&mut from_file, &decoded, s.ticks);
        assert_eq!(
            from_file, world,
            "{}: 読み戻した記録で結果が変わった",
            s.name
        );

        let expected = std::fs::read_to_string(&hash_path)
            .unwrap_or_else(|e| panic!("{} を読めない: {e}", hash_path.display()));
        if expected != got {
            mismatches.push(format!(
                "{}: ハッシュが {} から {} に変わった",
                s.name,
                expected.trim(),
                got.trim()
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "{}\n意図した変更なら CHIZU_UPDATE_GOLDEN=1 cargo test -p chizu-core --test golden で書き直す",
        mismatches.join("\n")
    );
}

#[test]
fn replaying_twice_gives_the_same_hash() {
    for s in scenarios() {
        let run = || {
            let mut w = World::new(s.seed);
            replay(&mut w, &s.log, s.ticks);
            w.state_hash()
        };
        assert_eq!(run(), run(), "{}", s.name);
    }
}

#[test]
fn seed_and_log_both_change_the_hash() {
    let s = &scenarios()[0];
    let run = |seed, log: &CommandLog| {
        let mut w = World::new(seed);
        replay(&mut w, log, s.ticks);
        w.state_hash()
    };
    let base = run(s.seed, &s.log);
    assert_ne!(run(s.seed + 1, &s.log), base);
    assert_ne!(run(s.seed, &CommandLog::new()), base);
}
