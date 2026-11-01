//! ゴールデンのハッシュ。
//!
//! 決まった記録を再生し、途中の決まった時点の状態ハッシュの列が `tests/golden/<名前>.hash` と
//! 一致するかを見る。各行は「tick、その時点の状態ハッシュ、tick 0 からその時点までの
//! 毎 tick の状態ハッシュをまとめた値」である。まとめた値があるので、時点と時点のあいだで
//! 起きた違いも見逃さない。最後の行は再生の終わりの時点である。
//! 記録そのものも `tests/golden/<名前>.rec` に置き、書き出しの形式が変わっていないかも見る。
//!
//! シミュレーションや記録の形式を意図して変えたときは、
//! `CHIZU_UPDATE_GOLDEN=1 cargo test -p chizu-core --test golden` でファイルを書き直し、同じ PR に含める。

use std::path::PathBuf;

use chizu_core::{
    ChunkCoord, Command, CommandLog, RecordedCommand, RoadKind, SimSpeed, StateHasher, Tick,
    TilePos, TileRect, World, ZoneKind, replay,
};

/// 状態ハッシュの列に書き出す間隔（tick）。
const CHECKPOINT_INTERVAL: u64 = 1_000;

struct Scenario {
    name: &'static str,
    seed: u64,
    ticks: u64,
    log: CommandLog,
}

fn scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "clock_and_commands",
            seed: 42,
            ticks: 2 * chizu_core::TICKS_PER_DAY,
            log: clock_and_commands(),
        },
        Scenario {
            name: "land_purchase",
            seed: 42,
            ticks: 600,
            log: land_purchase(),
        },
    ]
}

/// 土地を買い進める記録。買える場合と、買った土地・離れた土地・角だけで接する土地・
/// 所持金の不足で却下される場合を混ぜる。
fn land_purchase() -> CommandLog {
    let mut log = CommandLog::new();
    let mut buy = |tick: u64, cx: i32, cy: i32| {
        log.record(
            Tick(tick),
            Command::BuyChunk {
                coord: ChunkCoord::new(cx, cy),
            },
        )
        .unwrap();
    };
    buy(10, 2, 0);
    buy(10, 2, 0);
    buy(20, 4, 4);
    buy(20, 1, 1);
    buy(30, 2, 2);
    buy(100, 0, -2);
    buy(100, 3, 0);
    buy(200, -2, 0);
    // 次の 1 件で所持金をちょうど使い切り、その次は却下される。
    buy(300, 0, 2);
    buy(300, 0, 3);
    buy(400, 0, -3);
    log
}

/// 速度と税率を何度か変え、中身のまだないコマンドも混ぜた 2 日ぶんの記録。
/// 終わりの直前にもコマンドを置き、速度は等速に戻さずに終える。
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
    rec(13_500, speed(12));
    rec(14_398, tax(ZoneKind::Commercial, 200));
    rec(14_399, speed(6));
    // 再生する範囲より後なので適用されない。
    rec(20_000, speed(2));
    log
}

/// 再生して、状態ハッシュの列を `.hash` の形式の文字列で返す。
fn checkpoints(seed: u64, log: &CommandLog, ticks: u64) -> String {
    checkpoints_and_world(seed, log, ticks).0
}

fn checkpoints_and_world(seed: u64, log: &CommandLog, ticks: u64) -> (String, World) {
    let mut world = World::new(seed);
    let mut digest = StateHasher::new();
    digest.write(&world.state_hash());
    let mut out = String::new();
    for _ in 0..ticks {
        replay(&mut world, log, 1);
        let h = world.state_hash();
        digest.write(&h);
        let t = world.tick().0;
        if t.is_multiple_of(CHECKPOINT_INTERVAL) || t == ticks {
            out.push_str(&format!("{t} {h:016x} {:016x}\n", digest.finish()));
        }
    }
    (out, world)
}

/// 買い進める記録の結果が、水辺の配置と購入の規則を通っていることを確かめる。
/// ここが崩れると、ゴールデンのハッシュがそれらを見ていないことになる。
#[test]
fn land_purchase_covers_water_and_the_rules() {
    let s = scenarios()
        .into_iter()
        .find(|s| s.name == "land_purchase")
        .unwrap();
    let mut world = World::new(s.seed);
    let stats = replay(&mut world, &s.log, s.ticks);
    assert_eq!((stats.applied, stats.denied), (5, 6));
    assert_eq!(world.money(), chizu_core::Money::ZERO);
    assert_eq!(world.chunks().filter(|c| c.owned()).count(), 9 + 5);
    let water: u32 = world.chunks().map(|c| c.terrain().water_count()).sum();
    assert!(water > 0, "買った土地に水辺が 1 マスもない");
    assert_eq!(checkpoints_and_world(s.seed, &s.log, s.ticks).1, world);
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
        let got = checkpoints(s.seed, &s.log, s.ticks);

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
        assert_eq!(
            checkpoints(s.seed, &decoded, s.ticks),
            got,
            "{}: 読み戻した記録で結果が変わった",
            s.name
        );

        let expected = std::fs::read_to_string(&hash_path)
            .unwrap_or_else(|e| panic!("{} を読めない: {e}", hash_path.display()));
        if let Some((e, g)) = expected.lines().zip(got.lines()).find(|(e, g)| e != g) {
            mismatches.push(format!("{}: {e} が {g} に変わった", s.name));
        } else if expected.lines().count() != got.lines().count() {
            mismatches.push(format!("{}: 時点の数が変わった", s.name));
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
        assert_eq!(
            checkpoints(s.seed, &s.log, s.ticks),
            checkpoints(s.seed, &s.log, s.ticks),
            "{}",
            s.name
        );
    }
}

/// 記録の各件を `f` で書き換えた記録。`None` を返した件は消す。
fn rewrite(log: &CommandLog, f: impl Fn(RecordedCommand) -> Option<RecordedCommand>) -> CommandLog {
    let mut out = CommandLog::new();
    for e in log.entries().iter().copied().filter_map(f) {
        out.push(e).unwrap();
    }
    out
}

#[test]
fn broken_records_change_the_checkpoints() {
    let s = &scenarios()[0];
    let base = checkpoints(s.seed, &s.log, s.ticks);
    let shift = |by: u64| {
        rewrite(&s.log, move |e| {
            Some(RecordedCommand {
                tick: if e.tick.0 < s.ticks {
                    e.tick + by
                } else {
                    e.tick
                },
                ..e
            })
        })
    };
    let broken = [
        ("種を変える", s.seed + 1, s.log.clone()),
        ("記録を空にする", s.seed, CommandLog::new()),
        (
            "SetSimSpeed を消す",
            s.seed,
            rewrite(&s.log, |e| {
                (!matches!(e.command, Command::SetSimSpeed { .. })).then_some(e)
            }),
        ),
        ("範囲内の tick を 1,000 ずらす", s.seed, shift(1_000)),
        ("範囲内の tick を 1 ずらす", s.seed, shift(1)),
        (
            "最後の 1 件を消す",
            s.seed,
            rewrite(&s.log, |e| (e.tick != Tick(14_399)).then_some(e)),
        ),
    ];
    for (what, seed, log) in broken {
        assert_ne!(checkpoints(seed, &log, s.ticks), base, "{what}");
    }
}
