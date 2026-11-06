//! 街の状態の書き出しと読み戻し。
//!
//! ゴールデンの記録（`tests/golden/*.rec`）を再生し、途中の時点で書き出して読み戻す。
//! 時点には、記録のコマンドを適用した直後（網を変えてまだ step していない時点）を含める。

use std::path::PathBuf;

use chizu_core::{CommandLog, HouseholdCar, Money, SNAPSHOT_VERSION, SnapshotError, World, replay};

/// ゴールデンの記録のほとんどは種 42 で作ってある。
const SEED: u64 = 42;

/// 記録 `name` を作ったときと同じ種と所持金で始めた街（`tests/golden.rs` の `scenarios` と合わせる）。
/// 記録と違う街で再生すると、駅や区画を置けずに、確かめたい形（乗る人など）が出ない。
fn start(name: &str) -> World {
    let rail_riders_seed = (0..)
        .find(|s| chizu_core::world::highway_line(*s).start.y < 0)
        .unwrap();
    match name {
        "rail_lines" | "rail_crossing" => World::new(0),
        "rail_riders" => World::new_with_money(rail_riders_seed, Money(10_000_000)),
        "land_slots" => World::new_with_money(SEED, Money(10_000_000)),
        _ => World::new(SEED),
    }
}

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
    let mut riders = 0;
    for (name, log) in golden_logs() {
        let (points, _) = points(&log);
        let mut world = start(&name);
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
            riders += world.rail().rider_count();
        }
    }
    assert!(
        riders > 0,
        "駅で待つ人か列車に乗る人のいる時点で書き出していない"
    );
}

#[test]
fn continuing_from_a_snapshot_matches_the_replay() {
    for (name, log) in golden_logs() {
        let (points, end) = points(&log);
        let mut through = start(&name);
        replay(&mut through, &log, end);
        let mut world = start(&name);
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
        World::from_snapshot(&good[..16]),
        Err(SnapshotError::Decode(_))
    ));
    assert_eq!(
        World::from_snapshot(&good[..good.len() - 1]),
        Err(SnapshotError::Checksum)
    );
    let mut longer = good.clone();
    longer.push(0);
    assert_eq!(World::from_snapshot(&longer), Err(SnapshotError::Checksum));
}

/// 中身の整合が崩れたバイト列から街を作り直すと、経路探索のグラフを作るところで panic しうる。
/// 1 バイトでも壊れていれば、作り直す前に断ることを確かめる。
#[test]
fn a_corrupted_snapshot_is_refused_without_panicking() {
    let (_, log) = golden_logs()
        .into_iter()
        .find(|(name, _)| name == "commute")
        .expect("commute の記録がない");
    let mut world = World::new(SEED);
    replay(&mut world, &log, 3_000);
    let good = world.to_snapshot();
    let mut bytes = good.clone();
    for i in 0..bytes.len() {
        for bit in [0x01, 0x80] {
            bytes[i] ^= bit;
            assert!(
                World::from_snapshot(&bytes).is_err(),
                "{i} バイト目を {bit:#x} で反転しても読めた"
            );
            bytes[i] ^= bit;
        }
    }
    for len in (0..good.len()).step_by(97) {
        assert!(
            World::from_snapshot(&good[..len]).is_err(),
            "{len} バイトで切った"
        );
    }
}

#[test]
fn a_snapshot_the_caller_rejects_is_refused() {
    let mut world = World::new(SEED);
    world.step();
    let bytes = world.to_snapshot();
    assert_eq!(
        World::from_snapshot_where(&bytes, |w| w.tick().0 == 2),
        Err(SnapshotError::Rejected)
    );
    let back = World::from_snapshot_where(&bytes, |w| w.tick().0 == 1).unwrap();
    assert!(back == world);
}

/// 世帯の車が移動に使われているあいだに書き出して読み戻しても、車のいる所と、その後の街が変わらない。
#[test]
fn snapshot_keeps_household_cars_in_use() {
    let (_, log) = golden_logs()
        .into_iter()
        .find(|(name, _)| name == "commute")
        .expect("commute の記録がない");
    let in_use = |w: &World| {
        w.population()
            .household_ids()
            .filter(|h| matches!(w.trips().household_car(*h), Some(HouseholdCar::InUse(_))))
            .count()
    };
    let mut world = World::new(SEED);
    replay(&mut world, &log, 0);
    for _ in 0..20_000 {
        if in_use(&world) > 0 {
            break;
        }
        replay(&mut world, &log, 1);
    }
    assert!(in_use(&world) > 0, "車を使う世帯が出てこない");
    let mut back = round_trip(&world);
    assert!(back == world);
    for h in world.population().household_ids() {
        assert_eq!(
            back.trips().household_car(h),
            world.trips().household_car(h)
        );
    }
    replay(&mut world, &log, 500);
    replay(&mut back, &log, 500);
    assert_eq!(back.state_hash(), world.state_hash());
}
