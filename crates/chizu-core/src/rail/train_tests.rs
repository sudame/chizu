//! 路線と列車のテスト。街の中に道路、駅、線路を置き、路線を作って列車を長く回す。

use std::collections::BTreeMap;

use super::*;
use crate::command::{Command, RoadKind};
use crate::record::{CommandLog, replay};
use crate::tile::ChunkCoord;
use crate::units::{Money, Tick};
use crate::world::{ApplyOutcome, World};

fn at(x: i64, y: i64) -> TilePos {
    TilePos::new(x, y)
}

fn ok(w: &mut World, c: Command) {
    assert_eq!(w.apply(&c), ApplyOutcome::Applied, "{c:?}");
}

fn road(from: (i64, i64), to: (i64, i64)) -> Command {
    Command::BuildRoad {
        from: at(from.0, from.1),
        to: at(to.0, to.1),
        kind: RoadKind::Small,
    }
}

fn rail(from: (i64, i64), to: (i64, i64)) -> Command {
    Command::BuildRail {
        from: at(from.0, from.1),
        to: at(to.0, to.1),
    }
}

/// 駅を置いて ID を返す。`east` は駅の線路の東の端のマスで、`rotation` 2 なら南（y+）の道路、0 なら北の道路を向く。
fn station(w: &mut World, east: (i64, i64), rotation: u8) -> StationId {
    // 駅の線路は道路の側から 2 列目である。rotation 2 の at は東の端の 1 つ南、0 は西の端の 1 つ北である。
    let a = match rotation {
        2 => at(east.0, east.1 + 1),
        0 => at(east.0 - 9, east.1 - 1),
        _ => unreachable!(),
    };
    ok(
        w,
        Command::PlaceBuilding {
            kind: ServiceKind::Station,
            at: a,
            rotation,
        },
    );
    w.rail().station_at(at(east.0, east.1)).expect("駅がない")
}

fn create(w: &mut World, from: StationId, to: StationId) -> LineId {
    ok(w, Command::CreateLine { from, to });
    w.rail().lines().last().expect("路線がない").0
}

fn set_trains(w: &mut World, line: LineId, trains: u8) {
    ok(w, Command::SetLineTrains { line, trains });
}

fn rich_world() -> World {
    World::new_with_money(0, Money(1_000_000_000)).with_unlimited_land()
}

/// y = 10 の道路に面した駅 2 つ（駅の線路は y = 8 の x 3〜12 と x 53〜62）を、
/// 途中にノードのない線路（x 12〜53）で結んだ街。
fn shuttle_world() -> (World, StationId, StationId) {
    let mut w = rich_world();
    ok(&mut w, road((0, 10), (70, 10)));
    let a = station(&mut w, (12, 8), 2);
    let b = station(&mut w, (62, 8), 2);
    ok(&mut w, rail((12, 8), (53, 8)));
    (w, a, b)
}

/// 列車が押さえている閉塞が、ほかの列車と重ならないことを確かめる。
fn check_blocks(w: &World) {
    let rail = w.rail();
    let mut seen = BTreeMap::<Block, TrainId>::new();
    for (id, t) in rail.trains() {
        let line = rail.line(t.line).expect("路線がない");
        let s = line.step(t.pos);
        assert_eq!((s.track, s.dir), (t.track, t.dir), "{id:?}");
        assert!(t.offset >= 0 && t.offset <= s.len, "{id:?} {t:?}");
        for b in line.held_blocks(t) {
            if let Some(other) = seen.insert(b, id) {
                assert_eq!(other, id, "{b:?} を 2 本が押さえている");
            }
        }
    }
}

/// 列車を `ticks` だけ回し、毎 tick 閉塞の重なりを確かめる。どの列車も、最後に駅を出てから（現れてから）
/// `limit` tick 以内にまた駅を出ることと、行き詰まりの輪が見つからないことも確かめる。
/// 回したあいだに見た列車の本数の最大を返す。
fn run(w: &mut World, ticks: u64, limit: u64) -> usize {
    let mut last = BTreeMap::<TrainId, (u64, TrainState)>::new();
    let mut most = 0;
    for _ in 0..ticks {
        w.step();
        let now = w.tick().0;
        check_blocks(w);
        let rail = w.rail();
        most = most.max(rail.train_count());
        let mut next = BTreeMap::new();
        for (id, t) in rail.trains() {
            // 走っていなかった列車が走り出したら、駅を出たことにする。
            let since = match last.get(&id) {
                Some((_, before))
                    if *before != TrainState::Running && t.state == TrainState::Running =>
                {
                    now
                }
                Some((since, _)) => *since,
                None => now,
            };
            assert!(
                now - since <= limit,
                "{id:?} が {} tick 駅を出ていない {t:?}",
                now - since
            );
            next.insert(id, (since, t.state));
        }
        last = next;
        if rail.tally().deadlock != 0 {
            for (id, t) in rail.trains() {
                eprintln!("{id:?} {t:?}");
            }
            for (id, l) in rail.lines() {
                for (i, s) in l.path.iter().enumerate() {
                    eprintln!("{id:?} {i} {s:?}");
                }
            }
        }
        assert_eq!(rail.tally().deadlock, 0, "tick {now} で行き詰まった");
    }
    most
}

#[test]
fn a_line_is_routed_between_two_stations_and_turns_back_at_both_ends() {
    let (mut w, a, b) = shuttle_world();
    let id = create(&mut w, a, b);
    let line = w.rail().line(id).unwrap();
    assert_eq!(line.stations, vec![a, b]);
    assert!(!line.looped && !line.stopped);
    let kinds: Vec<StepKind> = line.path.iter().map(|s| s.kind).collect();
    use StepKind::{Pass, Terminal};
    assert_eq!(kinds, [Pass, Pass, Terminal, Pass, Pass, Terminal]);
    // 行きと帰りは同じ線路を逆の向きに通る。
    for i in 0..3 {
        let (go, back) = (&line.path[i], &line.path[5 - i]);
        assert_eq!(go.track, back.track);
        assert_ne!(go.dir, back.dir);
    }
    assert!(line.path[0].depart && line.path[3].depart);
    assert_eq!(line.path[0].station, Some(a));
    assert_eq!(line.path[2].station, Some(b));
    // 駅の線路 4 つと、駅のあいだの線路の 2 つの向き。終点の両方のホームは 1 つに数える。
    assert_eq!(line.cap(), 3);
    assert_eq!(line.trains, 0);
}

#[test]
fn lines_need_existing_stations_and_a_route_without_reversing() {
    let (mut w, a, b) = shuttle_world();
    let missing = StationId {
        index: 9,
        generation: 0,
    };
    let deny = |w: &mut World, c: Command| w.apply(&c);
    assert_eq!(
        deny(
            &mut w,
            Command::CreateLine {
                from: a,
                to: missing
            }
        ),
        ApplyOutcome::Denied(DenyReason::NoSuchStation)
    );
    assert_eq!(
        deny(&mut w, Command::CreateLine { from: a, to: a }),
        ApplyOutcome::Denied(DenyReason::StationInLine)
    );
    // 線路でつながっていない駅。
    ok(&mut w, road((0, 30), (20, 30)));
    let c = station(&mut w, (12, 28), 2);
    assert_eq!(
        deny(&mut w, Command::CreateLine { from: a, to: c }),
        ApplyOutcome::Denied(DenyReason::RailNoRoute)
    );
    let id = create(&mut w, a, b);
    assert_eq!(
        deny(
            &mut w,
            Command::ExtendLine {
                line: id,
                station: b
            }
        ),
        ApplyOutcome::Denied(DenyReason::StationInLine)
    );
    assert_eq!(
        deny(
            &mut w,
            Command::ExtendLine {
                line: id,
                station: c
            }
        ),
        ApplyOutcome::Denied(DenyReason::RailNoRoute)
    );
    // 終点 b から先へ向きを変えずに行けない最初の駅 a は、環状にできない。
    assert_eq!(
        deny(
            &mut w,
            Command::ExtendLine {
                line: id,
                station: a
            }
        ),
        ApplyOutcome::Denied(DenyReason::RailNoRoute)
    );
    let gone = LineId {
        index: 5,
        generation: 0,
    };
    assert_eq!(
        deny(
            &mut w,
            Command::SetLineTrains {
                line: gone,
                trains: 1
            }
        ),
        ApplyOutcome::Denied(DenyReason::NoSuchLine)
    );
    assert_eq!(
        deny(
            &mut w,
            Command::SetLineTrains {
                line: id,
                trains: 17
            }
        ),
        ApplyOutcome::Rejected
    );
    ok(&mut w, Command::RemoveLine { line: id });
    assert_eq!(w.rail().lines().count(), 0);
    assert_eq!(
        deny(&mut w, Command::RemoveLine { line: id }),
        ApplyOutcome::Denied(DenyReason::NoSuchLine)
    );
}

/// 分岐で 135 度折り返さなければ届かない駅へは、路線を引けない。曲がりが 45 度までなら引ける。
#[test]
fn a_route_does_not_reverse_at_a_junction() {
    let mut w = rich_world();
    ok(&mut w, road((0, 10), (60, 10)));
    ok(&mut w, road((0, -30), (40, -30)));
    let a = station(&mut w, (12, 8), 2);
    let c = station(&mut w, (49, 8), 2);
    // 北の道路の南側の駅。駅の線路は y = -28 の x 3〜12 である。
    let b = station(&mut w, (12, -28), 0);
    ok(&mut w, rail((12, 8), (40, 8)));
    // (30, 8) で北西へ分かれる。西から来た列車は北西へ曲がれない（135 度）。東から来た列車は曲がれる。
    ok(&mut w, rail((30, 8), (22, 0)));
    ok(&mut w, rail((22, 0), (22, -20)));
    ok(&mut w, rail((22, -20), (14, -28)));
    ok(&mut w, rail((14, -28), (12, -28)));
    assert_eq!(
        w.apply(&Command::CreateLine { from: a, to: b }),
        ApplyOutcome::Denied(DenyReason::RailNoRoute)
    );
    let ac = create(&mut w, a, c);
    let bc = create(&mut w, b, c);
    // b から c へは分岐を東へ抜ける。分岐を通る要素は遅い速さに落とす。
    let line = w.rail().line(bc).unwrap();
    let j = w.rail().node_at(at(30, 8)).unwrap();
    assert!(line.path.iter().any(|s| s.junction == Some(j) && s.slow));
    // c に東向きで着く路線は、向きを変えなければ b へ行けない。
    assert_eq!(
        w.apply(&Command::ExtendLine {
            line: ac,
            station: b
        }),
        ApplyOutcome::Denied(DenyReason::RailNoRoute)
    );
    // 分岐の先の線路を取り壊すと、b を通る路線は止まり、列車は出ない。
    ok(
        &mut w,
        Command::Bulldoze {
            area: TileRect::from_corners(at(22, -10), at(22, -10)),
        },
    );
    assert!(w.rail().line(bc).unwrap().stopped);
    assert!(!w.rail().line(ac).unwrap().stopped);
    set_trains(&mut w, bc, 2);
    for _ in 0..500 {
        w.step();
    }
    assert!(w.rail().trains().all(|(_, t)| t.line != bc));
}

/// 駅 2 つの往復は、上限（3 本）まで走らせても行き詰まらない。指した本数は上限に抑える。
#[test]
fn a_two_station_shuttle_runs_up_to_the_cap_without_deadlock() {
    let (mut w, a, b) = shuttle_world();
    let id = create(&mut w, a, b);
    set_trains(&mut w, id, 4);
    assert_eq!(w.rail().line(id).unwrap().allowed(), 3);
    let most = run(&mut w, 20_000, 600);
    assert_eq!(most, 3);
    assert_eq!(w.rail().train_count(), 3);
}

/// 本数を減らすと、終点で折り返す列車から消える。0 にすれば列車はなくなる。
#[test]
fn fewer_trains_retire_at_the_terminal() {
    let (mut w, a, b) = shuttle_world();
    let id = create(&mut w, a, b);
    set_trains(&mut w, id, 3);
    run(&mut w, 2_000, 600);
    assert_eq!(w.rail().train_count(), 3);
    set_trains(&mut w, id, 1);
    run(&mut w, 2_000, 600);
    assert_eq!(w.rail().train_count(), 1);
    set_trains(&mut w, id, 0);
    run(&mut w, 2_000, 600);
    assert_eq!(w.rail().train_count(), 0);
}

/// 列車は駅に止まり、終点で折り返して、最高の速さを超えない。
#[test]
fn a_train_stops_turns_back_and_keeps_to_the_speed_limit() {
    let (mut w, a, b) = shuttle_world();
    let id = create(&mut w, a, b);
    set_trains(&mut w, id, 1);
    let mut seen = Vec::new();
    for _ in 0..1_000 {
        w.step();
        let (_, t) = w.rail().trains().next().expect("列車が現れない");
        assert!(t.speed <= train::TRAIN_MAX_SPEED);
        if seen.last() != Some(&(t.pos, t.state)) {
            seen.push((t.pos, t.state));
        }
    }
    let states: Vec<u32> = seen
        .iter()
        .filter(|(_, s)| matches!(s, TrainState::Turnback { left: 30 }))
        .map(|(p, _)| *p)
        .collect();
    // 両方の終点で折り返す。
    assert!(states.contains(&2) && states.contains(&5), "{seen:?}");
}

/// 環状の路線は、1 周の閉塞の数 − 1 まで走らせても行き詰まらない。
#[test]
fn a_ring_line_runs_up_to_the_cap_without_deadlock() {
    let mut w = rich_world();
    ok(&mut w, road((0, 10), (20, 10)));
    ok(&mut w, road((0, -50), (20, -50)));
    let a = station(&mut w, (12, 8), 2);
    let b = station(&mut w, (12, -48), 0);
    for (from, to) in [
        ((12, 8), (22, 8)),
        ((22, 8), (32, -2)),
        ((32, -2), (32, -38)),
        ((32, -38), (22, -48)),
        ((22, -48), (12, -48)),
        ((3, -48), (-7, -48)),
        ((-7, -48), (-17, -38)),
        ((-17, -38), (-17, -2)),
        ((-17, -2), (-7, 8)),
        ((-7, 8), (3, 8)),
    ] {
        ok(&mut w, rail(from, to));
    }
    let id = create(&mut w, a, b);
    ok(
        &mut w,
        Command::ExtendLine {
            line: id,
            station: a,
        },
    );
    let line = w.rail().line(id).unwrap();
    assert!(line.looped);
    assert!(line.path.iter().all(|s| s.kind != StepKind::Terminal));
    // 駅の線路 2 つと、駅の外の線路 10 本。
    assert_eq!(line.cap(), 11);
    set_trains(&mut w, id, 16);
    let most = run(&mut w, 20_000, 1_000);
    assert_eq!(most, 11);
}

/// 本数の多い長い往復。16 本を走らせても、終点の折り返しで行き詰まらない。
#[test]
fn a_long_shuttle_with_sixteen_trains_turns_back_without_deadlock() {
    // 種 0 の街は y = 10 の x 0〜410 に水辺があるので、陸の続く種 6 を使う。
    let mut w = World::new_with_money(6, Money(1_000_000_000)).with_unlimited_land();
    for cx in 2..=6 {
        ok(
            &mut w,
            Command::BuyChunk {
                coord: ChunkCoord::new(cx, 0),
            },
        );
    }
    ok(&mut w, road((0, 10), (410, 10)));
    let a = station(&mut w, (12, 8), 2);
    let b = station(&mut w, (402, 8), 2);
    ok(&mut w, rail((12, 8), (393, 8)));
    let id = create(&mut w, a, b);
    assert!(w.rail().line(id).unwrap().cap() >= 16);
    set_trains(&mut w, id, 16);
    // 最初の駅を出る列車は、前の列車が出てから運転間隔の半分を空ける。
    let gap = w.rail().line(id).unwrap().departure_gap();
    assert!(gap > 0);
    let mut last = None;
    for _ in 0..3_000 {
        w.step();
        let at_first = w.rail().line(id).unwrap().departed.get(&0).copied();
        if at_first != last {
            if let (Some(before), Some(now)) = (last, at_first) {
                assert!(now - before >= gap, "{before} → {now}");
            }
            last = at_first;
        }
    }
    let most = run(&mut w, 30_000, 2_000);
    assert_eq!(most, 16);
}

/// 途中の駅を 2 つの路線が両方の向きに共有すると、路線ごとの上限を守っても、路線をまたぐ輪で
/// 行き詰まることがある。そのときは備えが輪の中の列車を 1 本消し、列車はまた走り出す。
#[test]
fn the_backstop_untangles_lines_crossing_at_a_shared_middle_station() {
    let mut w = rich_world();
    ok(&mut w, road((0, 10), (90, 10)));
    ok(&mut w, road((58, 0), (80, 0)));
    let west = station(&mut w, (12, 8), 2);
    let mid = station(&mut w, (32, 8), 2);
    let east = station(&mut w, (79, 8), 2);
    let north = station(&mut w, (71, -2), 2);
    ok(&mut w, rail((12, 8), (23, 8)));
    ok(&mut w, rail((32, 8), (70, 8)));
    ok(&mut w, rail((45, 8), (55, -2)));
    ok(&mut w, rail((55, -2), (62, -2)));
    let one = create(&mut w, west, mid);
    ok(
        &mut w,
        Command::ExtendLine {
            line: one,
            station: east,
        },
    );
    let two = create(&mut w, north, mid);
    ok(
        &mut w,
        Command::ExtendLine {
            line: two,
            station: west,
        },
    );
    set_trains(&mut w, one, 16);
    set_trains(&mut w, two, 16);
    let mut departures = 0;
    for _ in 0..20_000 {
        let before: Vec<TrainState> = w.rail().trains().map(|(_, t)| t.state).collect();
        w.step();
        check_blocks(&w);
        departures += w
            .rail()
            .trains()
            .zip(before)
            .filter(|((_, t), b)| *b != TrainState::Running && t.state == TrainState::Running)
            .count();
    }
    assert!(w.rail().tally().deadlock > 0);
    assert!(departures > 500, "{departures}");
}

/// 2 つの路線が、分岐と幹の線路と終点の駅を共有し、幹の上を逆の向きにも走る。
/// どちらも上限まで走らせて行き詰まらない。
///
/// 路線ごとの上限は、ほかの路線と共有する閉塞を数えに入れない。共有する駅が終点の 1 つだけなら、
/// 路線をまたぐ輪は終点を通る 1 本の路線の輪になり、上限で防げる。
#[test]
fn lines_sharing_a_junction_and_a_terminal_run_without_deadlock() {
    let mut w = rich_world();
    ok(&mut w, road((0, 10), (90, 10)));
    ok(&mut w, road((40, 0), (90, 0)));
    let west = station(&mut w, (12, 8), 2);
    let mid = station(&mut w, (49, 8), 2);
    let east = station(&mut w, (79, 8), 2);
    let north_mid = station(&mut w, (54, -2), 2);
    let north = station(&mut w, (73, -2), 2);
    ok(&mut w, rail((12, 8), (40, 8)));
    ok(&mut w, rail((49, 8), (70, 8)));
    // (30, 8) で北東へ分かれる。
    ok(&mut w, rail((30, 8), (40, -2)));
    ok(&mut w, rail((40, -2), (45, -2)));
    ok(&mut w, rail((54, -2), (64, -2)));
    let one = create(&mut w, west, mid);
    ok(
        &mut w,
        Command::ExtendLine {
            line: one,
            station: east,
        },
    );
    let two = create(&mut w, north, north_mid);
    ok(
        &mut w,
        Command::ExtendLine {
            line: two,
            station: west,
        },
    );
    let rail_ = w.rail();
    let j = rail_.node_at(at(30, 8)).unwrap();
    for id in [one, two] {
        let line = rail_.line(id).unwrap();
        assert_eq!(
            line.path.iter().filter(|s| s.junction == Some(j)).count(),
            2,
            "{id:?}"
        );
        // 西の終点との行き来だけが、共有する区間である。
        let shared: Vec<bool> = line
            .path
            .iter()
            .filter(|s| s.depart)
            .map(|s| s.shared)
            .collect();
        assert_eq!(shared.iter().filter(|s| **s).count(), 2, "{id:?}");
        // 駅のホーム 6 つ（終点は 1 つに数えて 4 つ）と、共有しない区間の線路の 2 つの向き。
        assert_eq!(line.cap(), 5, "{id:?}");
    }
    set_trains(&mut w, one, 16);
    set_trains(&mut w, two, 16);
    let most = run(&mut w, 30_000, 1_500);
    assert_eq!(most, 10);
}

/// 2 つの路線が、向かい合う終点として同じ駅を使う。両方のホームをまとめて押さえて入るので、
/// 互いに相手の折り返す先のホームを待って止まることがない。
#[test]
fn lines_ending_at_the_same_station_from_both_sides_run_without_deadlock() {
    let mut w = rich_world();
    ok(&mut w, road((0, 10), (90, 10)));
    let a = station(&mut w, (12, 8), 2);
    let b = station(&mut w, (42, 8), 2);
    let c = station(&mut w, (72, 8), 2);
    ok(&mut w, rail((12, 8), (33, 8)));
    ok(&mut w, rail((42, 8), (63, 8)));
    let left = create(&mut w, a, b);
    let right = create(&mut w, c, b);
    for id in [left, right] {
        // 両端の駅（2 つに数える）と、共有しない帰りの線路。行きは共有する駅へ向かうので、途中で待たない。
        assert_eq!(w.rail().line(id).unwrap().cap(), 2, "{id:?}");
        set_trains(&mut w, id, 16);
    }
    let most = run(&mut w, 20_000, 1_000);
    assert_eq!(most, 4);
}

/// 線路を分けても、走っている列車はそのまま同じ位置で走り続ける。
#[test]
fn trains_keep_running_when_a_track_is_split() {
    let mut w = rich_world();
    ok(&mut w, road((0, 10), (120, 10)));
    let a = station(&mut w, (12, 8), 2);
    let b = station(&mut w, (112, 8), 2);
    ok(&mut w, rail((12, 8), (103, 8)));
    let id = create(&mut w, a, b);
    set_trains(&mut w, id, 3);
    run(&mut w, 600, 1_000);
    let before = w.rail().train_count();
    assert!(before > 0);
    // 線路の途中から分岐を引くと、線路が分かれて ID が変わる。
    ok(&mut w, rail((60, 8), (70, -2)));
    assert_eq!(w.rail().train_count(), before);
    run(&mut w, 5_000, 1_000);
    assert_eq!(w.rail().train_count(), 3);
    // 列車の下の線路を取り壊すと、その列車は消え、残った路線は止まる。
    ok(
        &mut w,
        Command::Bulldoze {
            area: TileRect::from_corners(at(50, 8), at(50, 8)),
        },
    );
    assert!(w.rail().line(id).unwrap().stopped);
    for _ in 0..2_000 {
        w.step();
        check_blocks(&w);
    }
    // 駅を取り壊すと、駅が 2 つ未満になった路線はなくなり、列車も消える。
    ok(
        &mut w,
        Command::Bulldoze {
            area: TileRect::from_corners(at(5, 8), at(5, 8)),
        },
    );
    assert_eq!(w.rail().lines().count(), 0);
    assert_eq!(w.rail().train_count(), 0);
}

/// 列車の維持費は、走っている 1 本ごとに 1 日 600 である。
#[test]
fn trains_cost_upkeep() {
    let (mut w, a, b) = shuttle_world();
    let id = create(&mut w, a, b);
    let base = w.finances().rail_upkeep;
    set_trains(&mut w, id, 2);
    run(&mut w, 1_000, 600);
    assert_eq!(w.rail().train_count(), 2);
    assert_eq!(w.finances().rail_upkeep, base + Money(1_200));
}

/// 路線と列車の記録は、分けて再生しても、書き出して読み戻しても同じ街になる。
#[test]
fn lines_replay_and_round_trip_through_the_snapshot() {
    let (w0, a, b) = {
        let (w, a, b) = shuttle_world();
        (w, a, b)
    };
    let line = LineId {
        index: 0,
        generation: 0,
    };
    let mut log = CommandLog::new();
    log.record(Tick(1), Command::CreateLine { from: a, to: b })
        .unwrap();
    log.record(Tick(2), Command::SetLineTrains { line, trains: 3 })
        .unwrap();
    log.record(Tick(900), Command::SetLineTrains { line, trains: 1 })
        .unwrap();
    let mut whole = w0.clone();
    replay(&mut whole, &log, 1_500);
    let mut pieces = w0.clone();
    for _ in 0..15 {
        replay(&mut pieces, &log, 100);
    }
    assert_eq!(whole, pieces);
    assert_eq!(whole.state_hash(), pieces.state_hash());
    assert!(whole.rail().train_count() > 0);
    let mut back = World::from_snapshot(&whole.to_snapshot()).unwrap();
    assert_eq!(back, whole);
    assert_eq!(back.state_hash(), whole.state_hash());
    for w in [&mut whole, &mut back] {
        for _ in 0..500 {
            w.step();
        }
    }
    assert_eq!(back, whole);
    assert_eq!(back.state_hash(), whole.state_hash());
}
