//! 街の状態の書き出しと読み戻し。
//!
//! ゴールデンの記録（`tests/golden/*.rec`）を再生し、途中の時点で書き出して読み戻す。
//! 時点には、記録のコマンドを適用した直後（網を変えてまだ step していない時点）を含める。

use std::path::PathBuf;

use chizu_core::{CommandLog, SNAPSHOT_VERSION, SnapshotError, World, replay};

/// ゴールデンの記録はすべて種 42 で作ってある。
const SEED: u64 = 42;

/// 記録の最後のコマンドのあとに進める tick。
const AFTER_LAST: u64 = 1_500;

fn golden_logs() -> Vec<(String, CommandLog)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "rec"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "ゴールデンの記録がない");
    paths
        .into_iter()
        .map(|p| {
            let name = p.file_stem().unwrap().to_string_lossy().into_owned();
            let log = CommandLog::decode(&std::fs::read(&p).unwrap()).unwrap();
            (name, log)
        })
        .collect()
}

/// 書き出す時点。tick 0、コマンドを適用した各 tick、その少しあと、最後の tick である。
fn points(log: &CommandLog) -> (Vec<u64>, u64) {
    let last = log.entries().last().map_or(0, |e| e.tick.0);
    let end = last + AFTER_LAST;
    let mut points: Vec<u64> = log
        .entries()
        .iter()
        .flat_map(|e| [e.tick.0, e.tick.0 + 7])
        .chain([0, last + AFTER_LAST / 2, end])
        .collect();
    points.sort_unstable();
    points.dedup();
    (points, end)
}

fn round_trip(world: &World) -> World {
    World::from_snapshot(&world.to_snapshot()).expect("読み戻せない")
}

#[test]
fn snapshot_round_trip_keeps_the_hash() {
    for (name, log) in golden_logs() {
        let (points, _) = points(&log);
        let mut world = World::new(SEED);
        replay(&mut world, &log, 0);
        for p in points {
            let ticks = p - world.tick().0;
            replay(&mut world, &log, ticks);
            let bytes = world.to_snapshot();
            let back = World::from_snapshot(&bytes).expect("読み戻せない");
            assert_eq!(back.state_hash(), world.state_hash(), "{name} の tick {p}");
            assert!(
                back == world,
                "{name} の tick {p}：ハッシュに入らない欄が違う"
            );
            assert_eq!(
                back.to_snapshot(),
                bytes,
                "{name} の tick {p}：書き出し直すと違う"
            );
        }
    }
}

#[test]
fn continuing_from_a_snapshot_matches_the_replay() {
    for (name, log) in golden_logs() {
        let (points, end) = points(&log);
        let mut through = World::new(SEED);
        replay(&mut through, &log, end);
        let mut world = World::new(SEED);
        replay(&mut world, &log, 0);
        for p in points {
            let ticks = p - world.tick().0;
            replay(&mut world, &log, ticks);
            let mut back = round_trip(&world);
            replay(&mut back, &log, end - p);
            assert_eq!(
                back.state_hash(),
                through.state_hash(),
                "{name}：tick {p} で読み戻して tick {end} まで進めると、記録から作り直した街と違う"
            );
            assert!(
                back == through,
                "{name} の tick {p}：ハッシュに入らない欄が違う"
            );
        }
    }
}

#[test]
fn other_snapshot_versions_are_refused() {
    let mut world = World::new(SEED);
    world.step();
    let good = world.to_snapshot();
    assert_eq!(&good[8..12], &SNAPSHOT_VERSION.to_le_bytes());

    let mut other = good.clone();
    other[8..12].copy_from_slice(&(SNAPSHOT_VERSION + 1).to_le_bytes());
    assert_eq!(
        World::from_snapshot(&other),
        Err(SnapshotError::UnsupportedVersion(SNAPSHOT_VERSION + 1))
    );

    let mut magic = good.clone();
    magic[0] = b'X';
    assert_eq!(World::from_snapshot(&magic), Err(SnapshotError::BadMagic));
    assert!(matches!(
        World::from_snapshot(&good[..10]),
        Err(SnapshotError::Decode(_))
    ));
    assert!(matches!(
        World::from_snapshot(&good[..good.len() - 1]),
        Err(SnapshotError::Decode(_))
    ));
    let mut longer = good.clone();
    longer.push(0);
    assert_eq!(
        World::from_snapshot(&longer),
        Err(SnapshotError::TrailingBytes(1))
    );
}
