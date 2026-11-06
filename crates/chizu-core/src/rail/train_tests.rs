//! 路線と列車のテスト。街の中に道路、駅、線路を置き、路線を作って列車を長く回す。

use std::collections::BTreeMap;

use super::train::{TRAIN_BLOCK_LEN_MM, TRAIN_UPKEEP_PER_DAY};
use super::*;
use crate::command::{Command, RoadKind};
use crate::network::Dir;
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

/// 行き詰まりを防ぐ上限まで列車を走らせて確かめるので、終点の駅で折り返せる本数では抑えない。
fn rich_world() -> World {
    let mut w = World::new_with_money(0, Money(1_000_000_000)).with_unlimited_land();
    w.lift_turn_caps(true);
    w
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
        // 駅で止まっている列車は、駅の線路の先の端にいる。
        let at_end = t.offset == s.len;
        match t.state {
            TrainState::Running => {}
            TrainState::Dwell { .. } | TrainState::Turnback { .. } => {
                assert!(s.is_stop() && at_end, "{id:?} {t:?}");
            }
            TrainState::Ready => assert!(s.depart && at_end, "{id:?} {t:?}"),
        }
        for b in line.held_blocks(t) {
            if let Block::Track(track, _) = b {
                assert!(
                    rail.track(track).is_some(),
                    "{id:?} が消えた線路を押さえている {t:?}"
                );
            }
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
    run_lines(w, ticks, limit).0
}

/// [`run`] と同じだが、路線ごとの列車の本数の最大も返す。
fn run_lines(w: &mut World, ticks: u64, limit: u64) -> (usize, BTreeMap<LineId, usize>) {
    let mut last = BTreeMap::<TrainId, (u64, TrainState)>::new();
    let mut most = 0;
    let mut per_line = BTreeMap::<LineId, usize>::new();
    for _ in 0..ticks {
        w.step();
        let now = w.tick().0;
        check_blocks(w);
        let rail = w.rail();
        most = most.max(rail.train_count());
        let mut counts = BTreeMap::<LineId, usize>::new();
        for (_, t) in rail.trains() {
            *counts.entry(t.line).or_default() += 1;
        }
        for (l, c) in counts {
            let m = per_line.entry(l).or_default();
            *m = (*m).max(c);
        }
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
    (most, per_line)
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
    // 駅のあいだの線路の 2 つの向きと、両端の駅。駅の線路をほかの路線が通らないので、どちらの駅も
    // ホーム 2 つに列車が入れ、2 つに数える。
    assert_eq!(line.cap(), 5);
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

/// 駅 2 つの往復は、上限（5 本）まで走らせても行き詰まらない。指した本数は上限に抑える。
#[test]
fn a_two_station_shuttle_runs_up_to_the_cap_without_deadlock() {
    let (mut w, a, b) = shuttle_world();
    let id = create(&mut w, a, b);
    set_trains(&mut w, id, 6);
    assert_eq!(w.rail().line(id).unwrap().allowed(), 5);
    let most = run(&mut w, 20_000, 600);
    assert_eq!(most, 5);
    assert_eq!(w.rail().train_count(), 5);
}

/// 区画と区間の枠を譲る決まり：列車が、先に枠を待った同じ路線の列車の印を無視するのは、その列車がこちらを待って
/// いるときだけである。待っていなければ、印を守って枠を譲る。
#[test]
fn a_claimed_quota_is_ignored_only_when_the_claimant_waits_on_us() {
    let (mut w, [west, mid, east, north]) = two_line_world();
    let one = line_through(&mut w, &[west, mid, east]);
    let two = line_through(&mut w, &[north, mid, west]);
    set_trains(&mut w, one, 4);
    set_trains(&mut w, two, 4);
    let (mut kept, mut ignored) = (0, 0);
    for _ in 0..3_000 {
        w.step();
        // 枠は路線ごとなので、枠を待つのは同じ路線の列車である。
        let ids: Vec<(TrainId, LineId)> = w.rail().trains().map(|(i, t)| (i, t.line)).collect();
        for (x, lx) in &ids {
            for (y, _) in ids.iter().filter(|(y, ly)| y != x && ly == lx) {
                if w.rail().ignores_claim(*x, *y, false) == Some(false) {
                    kept += 1;
                }
                if w.rail().ignores_claim(*x, *y, true) == Some(true) {
                    ignored += 1;
                }
            }
        }
    }
    assert!(kept > 0, "こちらを待たない列車の印を守る形がない");
    assert!(ignored > 0, "こちらを待つ列車の印を無視する形がない");
}

/// 駅の線路をほかの路線が通らない終点では、着くホームがふさがっていれば、列車はもう 1 つのホームに入り、2 本が
/// 同時に止まって折り返す（#371）。2 本は別々のホームを押さえる。
#[test]
fn a_terminal_turns_two_trains_at_once() {
    let (mut w, a, b) = shuttle_world();
    let id = create(&mut w, a, b);
    set_trains(&mut w, id, 5);
    let mut both = 0;
    for _ in 0..5_000 {
        w.step();
        check_blocks(&w);
        let rail = w.rail();
        let line = rail.line(id).unwrap();
        let mut at = BTreeMap::<u32, Vec<u8>>::new();
        for (_, t) in rail.trains() {
            if let Some(k) = line.pocket(t.pos)
                && t.state != TrainState::Running
            {
                at.entry(k).or_default().push(t.bays & line.bay_bit(k));
            }
        }
        for bays in at.values() {
            assert!(bays.len() <= 2, "{bays:?}");
            if bays.len() == 2 {
                both += 1;
                assert_ne!(bays[0], bays[1], "2 本が同じホームにいる");
            }
        }
    }
    assert!(both > 500, "{both}");
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

/// 終点で折り返せる本数（`turn_cap`）で抑える路線でも、本数を減らした分はその場では消えず、終点で消える。
#[test]
fn fewer_trains_under_a_turn_cap_retire_at_the_terminal() {
    let (mut w, a, b) = shuttle_world();
    w.lift_turn_caps(false);
    let id = create(&mut w, a, b);
    set_trains(&mut w, id, 2);
    assert!(w.rail().line(id).unwrap().turn_cap() < line::MAX_LINE_TRAINS);
    run(&mut w, 1_000, 600);
    assert_eq!(w.rail().train_count(), 2);
    set_trains(&mut w, id, 1);
    assert_eq!(w.rail().train_count(), 2, "本数を減らしたその場で消えた");
    let mut last = BTreeMap::new();
    for _ in 0..2_000 {
        last = w.rail().trains().map(|(t, s)| (t, s.pos)).collect();
        w.step();
        if w.rail().train_count() == 1 {
            break;
        }
    }
    assert_eq!(w.rail().train_count(), 1);
    let line = w.rail().line(id).unwrap();
    let (gone, pos) = last
        .iter()
        .find(|(t, _)| w.rail().trains().all(|(o, _)| o != **t))
        .expect("消えた列車がない");
    assert_eq!(
        line.step(*pos).kind,
        StepKind::Terminal,
        "{gone:?} が終点の外で消えた"
    );
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
    w.lift_turn_caps(true);
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
    // 最初の駅（終点）を出る列車は、前の列車が出てから、測った 1 周を本数で割った値の 9 割を空ける。
    let line = w.rail().line(id).unwrap();
    let gap = line.departure_gap(0);
    assert_eq!(gap, line.measured_round_trip_ticks() * 9 / 10 / 16);
    assert!(gap > line.headway_ticks().unwrap() / 2);
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

/// 3 つの決まりでは起きないはずの輪を手で作ると、備えが輪の中の ID のいちばん大きい列車を 1 本消し、
/// 列車はまた走り出す。駅 2 つの往復の 4 つの待てる所に 1 本ずつ置き、両端の駅のもう 1 つのホームにも 1 本ずつ
/// 置いて、どの列車も次の待てる所を待つ形にする。
#[test]
fn the_backstop_untangles_a_cycle_of_waiting_trains() {
    let (mut w, a, b) = shuttle_world();
    let id = create(&mut w, a, b);
    set_trains(&mut w, id, 5);
    let mut rail = w.rail().clone();
    let line = rail.line(id).unwrap().clone();
    let waits: Vec<u32> = (0..line.len()).filter(|i| line.step(*i).wait).collect();
    assert_eq!(waits, [0, 1, 3, 4]);
    let mut ids = Vec::new();
    // 駅のホームには、着くホームと、もう 1 つのホームに 1 本ずつ置く。
    let places: Vec<(u32, u8)> = waits
        .iter()
        .flat_map(|i| {
            let i = *i;
            let alt = line
                .pocket(i)
                .filter(|k| line.alt_ok(*k))
                .map(|k| (i, line.bay_bit(k)));
            [(i, 0)].into_iter().chain(alt)
        })
        .collect();
    assert_eq!(places.len(), 6);
    for (i, bays) in places {
        let s = line.step(i);
        ids.push(rail.trains.insert(Train {
            line: id,
            pos: i,
            track: s.track,
            dir: s.dir,
            offset: s.len,
            speed: 0,
            ahead: 0,
            state: if s.depart {
                TrainState::Ready
            } else {
                TrainState::Running
            },
            waited: 0,
            bays,
        }));
    }
    rail.step(Tick(1));
    assert_eq!(rail.tally().deadlock, 1);
    assert_eq!(rail.train_count(), 5);
    assert!(rail.train(*ids.iter().max().unwrap()).is_none());
    let mut departures = 0;
    for tick in 2..5_000 {
        let before: Vec<TrainState> = rail.trains().map(|(_, t)| t.state).collect();
        rail.step(Tick(tick));
        departures += rail
            .trains()
            .zip(before)
            .filter(|((_, t), b)| *b != TrainState::Running && t.state == TrainState::Running)
            .count();
    }
    assert_eq!(rail.tally().deadlock, 1);
    assert!(departures > 50, "{departures}");
}

/// 2 つの路線が、分岐と幹の線路と終点の駅を共有し、幹の上を逆の向きにも走る。
/// どちらも上限まで走らせて行き詰まらない。
///
/// 共有する待てる所（西の駅のホームと、分岐までの線路の 2 つの向き）は共有の区画になる。区画の中に輪は
/// ないので枠に上限はなく、路線の上限は自分だけの区間（分岐から先）の待てる所の数で決まる。
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
        // 自分だけの区間は、分岐から先の長い線路の 2 つの向き、途中の駅の 2 つのホーム、その先の線路の
        // 2 つの向き、東か北の終点の 6 つである。東と北の終点は、駅の線路をほかの路線が通らないので
        // ホーム 2 つに列車が入れ、2 つに数える。北の路線の (40, -2)–(45, -2) は 40m しかないので待てる所にしない。
        assert!(
            line.path
                .iter()
                .all(|s| !s.wait || s.len >= train::TRAIN_BLOCK_LEN_MM)
        );
        assert_eq!(line.plan.runs, vec![8], "{id:?}");
        assert_eq!(line.cap(), 8, "{id:?}");
    }
    set_trains(&mut w, one, 16);
    set_trains(&mut w, two, 16);
    let most = run(&mut w, 30_000, 2_000);
    assert_eq!(most, 16);
}

/// 2 つの路線が、向かい合う終点として同じ駅を使う。列車は着いたホームで折り返して同じホームから出るので、
/// 2 つの路線は駅の別々のホームを使い、互いに相手のホームを待って止まることがない。
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
        // 待てる所は、自分の端の駅（ホーム 2 つ）、行きと帰りの線路、駅 b の着くホームの 5 つである。
        // 駅 b の線路はもう一方の路線も使うので、駅 b ではもう 1 つのホームを使わない。
        let line = w.rail().line(id).unwrap();
        assert!(line.plan.zone.iter().all(Option::is_none), "{id:?}");
        assert_eq!(line.cap(), 4, "{id:?}");
        set_trains(&mut w, id, 16);
    }
    let most = run(&mut w, 20_000, 1_000);
    assert_eq!(most, 8);
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
    assert_eq!(
        w.finances().rail_upkeep,
        base + Money(TRAIN_UPKEEP_PER_DAY.0 * 2)
    );
}

/// 路線と列車の記録は、分けて再生しても、書き出して読み戻しても同じ街になる。
#[test]
fn lines_replay_and_round_trip_through_the_snapshot() {
    let (w0, a, b) = {
        let (mut w, a, b) = shuttle_world();
        // 印は書き出さないので、読み戻した街と比べられるよう、ふだんどおりにする。
        w.lift_turn_caps(false);
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

/// 駅 a、b、c を一列に並べ、a–b と b–c を線路で結んだ街。
fn three_in_a_row() -> (World, StationId, StationId, StationId) {
    let mut w = rich_world();
    ok(&mut w, road((0, 10), (100, 10)));
    let a = station(&mut w, (12, 8), 2);
    let b = station(&mut w, (62, 8), 2);
    let c = station(&mut w, (92, 8), 2);
    ok(&mut w, rail((12, 8), (53, 8)));
    ok(&mut w, rail((62, 8), (83, 8)));
    (w, a, b, c)
}

/// a–b の往復を 1 本で走らせ、`pos` の要素で `Turnback { left }` になるまで回す。
fn turn_back_at(w: &mut World, pos: u32, left: u16) {
    for _ in 0..3_000 {
        w.step();
        let (_, t) = w.rail().trains().next().unwrap();
        if t.pos == pos && t.state == (TrainState::Turnback { left }) {
            return;
        }
    }
    panic!("折り返さない");
}

/// 最初の駅で折り返し終える直前に路線を延ばしても、列車は折り返しをやり直して走り続ける。
#[test]
fn extending_a_line_while_turning_back_at_the_first_station_keeps_the_train() {
    let (mut w, a, b, c) = three_in_a_row();
    let id = create(&mut w, a, b);
    set_trains(&mut w, id, 1);
    turn_back_at(&mut w, 5, 1);
    ok(
        &mut w,
        Command::ExtendLine {
            line: id,
            station: c,
        },
    );
    run(&mut w, 3_000, 1_000);
    assert_eq!(w.rail().train_count(), 1);
}

/// 終点で折り返しているあいだに路線を延ばすと、終点は途中の駅になる。列車は折り返さずに、
/// 先の閉塞を押さえてから駅を出る。
#[test]
fn extending_a_line_while_turning_back_at_the_far_end_keeps_the_train_at_the_station() {
    let (mut w, a, b, c) = three_in_a_row();
    let id = create(&mut w, a, b);
    set_trains(&mut w, id, 1);
    turn_back_at(&mut w, 2, 15);
    ok(
        &mut w,
        Command::ExtendLine {
            line: id,
            station: c,
        },
    );
    let (_, t) = w.rail().trains().next().unwrap();
    assert_eq!((t.pos, t.ahead), (2, 0));
    run(&mut w, 3_000, 1_000);
    assert_eq!(w.rail().train_count(), 1);
}

/// 先に押さえた線路を取り壊すと、その列車は消える。
#[test]
fn bulldozing_a_track_reserved_ahead_removes_the_train() {
    let mut w = rich_world();
    ok(&mut w, road((0, 10), (120, 10)));
    let a = station(&mut w, (12, 8), 2);
    let b = station(&mut w, (112, 8), 2);
    // 途中にノードのない長い線路は 50 マスごとに区切るので、駅のあいだは 2 本になる。
    ok(&mut w, rail((12, 8), (103, 8)));
    let id = create(&mut w, a, b);
    set_trains(&mut w, id, 1);
    let mut found = false;
    for _ in 0..3_000 {
        w.step();
        let (_, t) = w.rail().trains().next().unwrap();
        if t.pos == 1 && t.ahead >= 1 && t.state == TrainState::Running {
            found = true;
            break;
        }
    }
    assert!(found);
    ok(
        &mut w,
        Command::Bulldoze {
            area: TileRect::from_corners(at(80, 8), at(80, 8)),
        },
    );
    assert_eq!(w.rail().train_count(), 0);
    for _ in 0..200 {
        w.step();
        check_blocks(&w);
    }
}

/// 駅 a、b を結ぶ線路の、b の端から `gap` マス手前に、使われない短い枝を付けた往復の路線を回し、
/// 行き詰まりの輪を見つけた回数を返す。
fn spur_near_terminal(gap: i64, trains: u8) -> u64 {
    let mut w = rich_world();
    ok(&mut w, road((0, 10), (70, 10)));
    let a = station(&mut w, (12, 8), 2);
    let b = station(&mut w, (62, 8), 2);
    ok(&mut w, rail((12, 8), (53, 8)));
    let j = 53 - gap;
    ok(&mut w, rail((j, 8), (j - 5, 3)));
    let id = create(&mut w, a, b);
    set_trains(&mut w, id, trains);
    for _ in 0..10_000 {
        w.step();
        check_blocks(&w);
    }
    w.rail().tally().deadlock
}

/// 分岐のすぐ先に列車が入りきらない短い線路があっても、列車はそこで待たないので行き詰まらない。
#[test]
fn a_short_track_right_after_a_junction_does_not_deadlock() {
    for gap in [1, 3, 5, 8, 9, 20] {
        for trains in [2, 3, 16] {
            assert_eq!(
                spur_near_terminal(gap, trains),
                0,
                "gap {gap} trains {trains}"
            );
        }
    }
}

/// 駅 W、M、E を y = 8 に、駅 N を y = −2 に置き、W–M、M–E を結び、M と E のあいだの (45, 8) から
/// N へ分かれる街。
fn two_line_world() -> (World, [StationId; 4]) {
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
    (w, [west, mid, east, north])
}

fn line_through(w: &mut World, stations: &[StationId]) -> LineId {
    let id = create(w, stations[0], stations[1]);
    for s in &stations[2..] {
        ok(
            w,
            Command::ExtendLine {
                line: id,
                station: *s,
            },
        );
    }
    id
}

/// 途中の駅 M と端の駅 W を 2 つの路線が共有しても、行き詰まらない。形 A は W–M–E と N–M–W、形 B は W–M–E と
/// W–M–N である。本数を 1〜6 の組で 20,000 tick ずつ回す。
///
/// 共有する W、W–M の線路、M は共有の区画になり、列車はそこでも止まって待てる。区画の中に輪はないので枠に
/// 上限はない。どちらの路線も、自分だけの区間（M の先の線路の 2 つの向きと、ホーム 2 つに列車が入れる終点）の
/// 待てる所の数 4 まで走る。
#[test]
fn lines_sharing_a_middle_station_and_a_terminal_do_not_deadlock() {
    for form_b in [false, true] {
        for (a, b) in (1..=6).flat_map(|a| (1..=6).map(move |b| (a, b))) {
            let (mut w, [west, mid, east, north]) = two_line_world();
            let one = line_through(&mut w, &[west, mid, east]);
            let two = if form_b {
                line_through(&mut w, &[west, mid, north])
            } else {
                line_through(&mut w, &[north, mid, west])
            };
            assert_eq!(w.rail().line(one).unwrap().cap(), 4);
            assert_eq!(w.rail().line(two).unwrap().cap(), 4);
            set_trains(&mut w, one, a);
            set_trains(&mut w, two, b);
            let (most, per_line) = run_lines(&mut w, 20_000, 3_000);
            let rail = w.rail();
            let allowed = rail.line(one).unwrap().allowed() + rail.line(two).unwrap().allowed();
            assert_eq!(most, usize::from(allowed), "form_b {form_b} {a}/{b}");
            assert_eq!(per_line.len(), 2, "form_b {form_b} {a}/{b}");
        }
    }
}

/// 2 つの路線の一方が、もう一方の進路にすっぽり入る形でも、どちらも走り、行き詰まらない。
/// `inner` の駅の並びの路線を、W–M–E の路線と同時に、本数の組を変えながら 20,000 tick ずつ回す。
/// 走った本数の最大を、路線の順に返す。
fn run_inside(inner: &[usize], counts: &[(u8, u8)]) -> Vec<(u8, u8)> {
    let mut out = Vec::new();
    for (a, b) in counts {
        let (mut w, st) = two_line_world();
        let outer = line_through(&mut w, &[st[0], st[1], st[2]]);
        let stations: Vec<StationId> = inner.iter().map(|i| st[*i]).collect();
        let inner_line = line_through(&mut w, &stations);
        assert!(w.rail().line(inner_line).unwrap().cap() > 0, "{inner:?}");
        set_trains(&mut w, outer, *a);
        set_trains(&mut w, inner_line, *b);
        let (_, per_line) = run_lines(&mut w, 20_000, 3_000);
        let most = |l: LineId| u8::try_from(per_line.get(&l).copied().unwrap_or(0)).unwrap();
        assert!(
            most(outer) >= 1 && most(inner_line) >= 1,
            "{inner:?} {a}/{b}"
        );
        out.push((most(outer), most(inner_line)));
    }
    out
}

const COUNTS: [(u8, u8); 7] = [(1, 1), (1, 6), (6, 1), (3, 3), (6, 6), (16, 16), (2, 16)];

/// 区間運転（W–M–E があるときの W–M）が走る。W–M は自分だけの待てる所を持たず、W、W–M の線路、M の
/// 待てる所の輪（4 つ）が共有の区画の中の輪になる。区画の枠は 3 で、本数が 1 以上の路線で分ける。
/// W–M の本数が 0 のうちは、W–M–E が 3 つとも使い、上限は自分だけの区間の待てる所の数 7 である（E は
/// ホーム 2 つに列車が入れ、M の西向きのホームは、W–M が M の東向きのホームで折り返すので W–M–E だけが使う）。W–M の上限は、
/// 本数を 1 以上にしたら配られる分の 2 を見せる。
/// W–M を走らせると、W–M に 2、W–M–E に 1 を分ける。W–M–E の列車は 1 周の 3 分の 1 以上を区画の中で過ごすので、
/// 区画の分 1 で運べるのは 3 本の分である。上限は 3 になる。
#[test]
fn a_short_turn_line_runs_inside_another() {
    let (mut w, st) = two_line_world();
    let outer = line_through(&mut w, &[st[0], st[1], st[2]]);
    let inner = line_through(&mut w, &[st[0], st[1]]);
    set_trains(&mut w, outer, 5);
    let rail = w.rail();
    assert!(rail.line(inner).unwrap().plan.runs.is_empty());
    assert_eq!(rail.line(inner).unwrap().cap(), 2);
    assert_eq!(rail.line(inner).unwrap().plan.quota[&0], 0);
    assert_eq!(rail.line(outer).unwrap().plan.runs, vec![7]);
    assert_eq!(rail.line(outer).unwrap().plan.quota[&0], 3);
    assert_eq!(rail.line(outer).unwrap().cap(), 7);
    set_trains(&mut w, inner, 1);
    let rail = w.rail();
    assert_eq!(rail.line(inner).unwrap().plan.quota[&0], 2);
    assert_eq!(rail.line(inner).unwrap().cap(), 2);
    assert_eq!(rail.line(outer).unwrap().plan.quota[&0], 1);
    assert_eq!(rail.line(outer).unwrap().cap(), 3);
    for (o, i) in run_inside(&[0, 1], &COUNTS) {
        assert!(i <= 2 && o <= 3, "{o}/{i}");
    }
}

/// 本数 0 の区間運転は、区画の枠を押さえない（#344）。W–M–E を 5 本走らせ、W–M を 0 本にしておくと、
/// W–M–E は区画の枠 3 をすべて使って 5 本走る。W–M を走らせると W–M–E の上限は 3 に下がり、超えた列車は
/// すぐに消える。W–M を 0 本に戻すと、W–M の列車はすぐに消え（減った枠を待って止まらないように）、
/// W–M–E は 5 本まで出直す（上限は 7 に戻る）。
#[test]
fn a_short_turn_line_without_trains_leaves_the_zone_to_the_main_line() {
    let (mut w, st) = two_line_world();
    let outer = line_through(&mut w, &[st[0], st[1], st[2]]);
    let inner = line_through(&mut w, &[st[0], st[1]]);
    set_trains(&mut w, outer, 5);
    let count = |w: &World, id: LineId| w.rail().trains().filter(|(_, t)| t.line == id).count();
    let (_, per_line) = run_lines(&mut w, 3_000, 3_000);
    assert_eq!(per_line[&outer], 5);
    set_trains(&mut w, inner, 2);
    assert_eq!(w.rail().line(outer).unwrap().cap(), 3);
    assert!(count(&w, outer) <= 3);
    let (_, per_line) = run_lines(&mut w, 3_000, 3_000);
    assert_eq!(per_line[&inner], 2);
    // 区画の分 1 を待つ列車が先に区画へ入るので、最初の駅に新しい列車が出る隙は少ない。
    assert!((2..=3).contains(&per_line[&outer]), "{per_line:?}");
    set_trains(&mut w, inner, 0);
    assert_eq!(count(&w, inner), 0);
    assert_eq!(w.rail().line(outer).unwrap().cap(), 7);
    run(&mut w, 3_000, 3_000);
    assert_eq!(count(&w, outer), 5);
}

/// W–M–E を 1 本走らせ、E で折り返して M へ戻る途中（自分だけの区間）まで進めた街。
fn outer_train_heading_back() -> (World, [StationId; 4], LineId) {
    let (mut w, st) = two_line_world();
    let outer = line_through(&mut w, &[st[0], st[1], st[2]]);
    set_trains(&mut w, outer, 1);
    let line = w.rail().line(outer).unwrap();
    let east = line
        .path
        .iter()
        .position(|s| s.kind == StepKind::Terminal && s.station == Some(st[2]))
        .unwrap();
    let east = u32::try_from(east).unwrap();
    let len = line.len();
    for _ in 0..20_000 {
        w.step();
        let found = w.rail().trains().any(|(_, t)| {
            t.line == outer && t.state == TrainState::Running && t.pos > east && t.pos + 3 < len
        });
        if found {
            return (w, st, outer);
        }
    }
    panic!("M へ戻る途中の列車がない");
}

/// 本数 0 にした路線の列車は、区分けが変わらず終点まで残る。その途中で区画ができ、その路線の分が 0 になったら、
/// 列車をすぐに消す。残すと、区画の手前で止まり続ける。
#[test]
fn a_zero_line_train_is_dropped_when_a_zone_shuts_it_out() {
    let (mut w, st, outer) = outer_train_heading_back();
    set_trains(&mut w, outer, 0);
    assert_eq!(
        w.rail().trains().filter(|(_, t)| t.line == outer).count(),
        1
    );
    line_through(&mut w, &[st[0], st[1]]);
    assert!(
        w.rail()
            .line(outer)
            .unwrap()
            .plan
            .quota
            .values()
            .any(|q| *q == 0)
    );
    assert!(w.rail().trains().all(|(_, t)| t.line != outer));
    run(&mut w, 3_000, 3_000);
    assert!(w.rail().trains().all(|(_, t)| t.line != outer));
}

/// 前の規則で、本数 0 の路線が区画の分を持っていたときの保存データを読むと、読み直した区分けではその路線の分が
/// 0 になる。そのとき残っている列車も、次の tick で消す。
#[test]
fn a_zero_line_train_from_an_old_save_is_dropped() {
    let (mut w, st, outer) = outer_train_heading_back();
    line_through(&mut w, &[st[0], st[1]]);
    let mut rail = w.rail().clone();
    // 本数だけを 0 にし、区分けを作り直す。読み込みの `restore_plans` と同じである。
    rail.lines.get_mut(outer).unwrap().trains = 0;
    rail.restore_plans();
    assert!(
        rail.line(outer)
            .unwrap()
            .plan
            .quota
            .values()
            .any(|q| *q == 0)
    );
    assert_eq!(rail.trains().filter(|(_, t)| t.line == outer).count(), 1);
    rail.step(w.tick());
    assert!(rail.trains().all(|(_, t)| t.line != outer));
}

/// 同じ駅を並べた 2 つの路線（緩行と快速）と、途中の駅を通り過ぎる路線が走る。どちらの路線も進路のすべてを
/// 共有するので、区画の枠（輪の待てる所の数 − 1）を 2 つの路線で分ける。
#[test]
fn lines_on_the_same_stations_both_run() {
    for inner in [&[0, 1, 2][..], &[0, 2]] {
        let (mut w, st) = two_line_world();
        let one = line_through(&mut w, &[st[0], st[1], st[2]]);
        let stations: Vec<StationId> = inner.iter().map(|i| st[*i]).collect();
        let two = line_through(&mut w, &stations);
        let rail = w.rail();
        let (c1, c2) = (rail.line(one).unwrap().cap(), rail.line(two).unwrap().cap());
        assert!(c1 >= 1 && c2 >= 1, "{inner:?} {c1}/{c2}");
        run_inside(inner, &COUNTS);
    }
}

/// 自分だけの部分が短い枝の路線（N–M）が、W–M–E と M の前後を共有して走る。
#[test]
fn a_short_branch_line_runs() {
    run_inside(&[3, 1], &COUNTS);
}

/// 共有の区画の枠を、区画を使う路線で分けきれない本数を決めると、前からある路線の上限が 0 になるので断る。
/// 区間運転 W–M が作る区画（枠 3）を、W–M、W–M–E、W–M–N が 1 つずつ使っているところへ、M–W を足す。
/// 本数 0 の M–W は枠を使わないので、作るのは受け付ける。M–W を走らせようとすると、余りを先に配る区間運転の
/// M–W に 1 つ回り、W–M–N の分が 0 になるので断る。
#[test]
fn a_line_that_would_starve_another_is_denied() {
    let (mut w, [west, mid, east, north]) = two_line_world();
    let short = line_through(&mut w, &[west, mid]);
    let one = line_through(&mut w, &[west, mid, east]);
    let two = line_through(&mut w, &[west, mid, north]);
    for id in [short, one, two] {
        assert!(w.rail().line(id).unwrap().cap() >= 1);
        set_trains(&mut w, id, 3);
    }
    run(&mut w, 2_000, 3_000);
    let back = create(&mut w, mid, west);
    for id in [short, one, two] {
        assert_eq!(w.rail().line(id).unwrap().plan.quota[&0], 1, "{id:?}");
    }
    assert_eq!(w.rail().line(back).unwrap().cap(), 1);
    let before = w.state_hash();
    let command = Command::SetLineTrains {
        line: back,
        trains: 2,
    };
    assert_eq!(
        w.apply(&command),
        ApplyOutcome::Denied(DenyReason::LineStarvesOther)
    );
    assert_eq!(w.state_hash(), before);
    assert_eq!(w.rail().line(back).unwrap().trains, 0);
    // 断る原因の路線を、状態を変えずに読める。
    assert_eq!(w.rail().starved_by_command(&command), Some(two));
    assert_eq!(
        w.rail()
            .starved_by_command(&Command::RemoveLine { line: short }),
        None
    );
    assert_eq!(w.state_hash(), before);
    run(&mut w, 3_000, 3_000);
}

/// 駅 A、M、E、F を一列に並べ（駅の線路は y = 8 の x 3〜12、33〜42、63〜72、93〜102）、隣り合う駅を
/// 線路で結んだ街に、A–M、M–E、E–F の 3 本を本数 3 で走らせる（#356）。M と E のあいだだけは x 52 に
/// ノードを置いて 2 本に分け、M–E の自分だけの区間（行きと帰り）の待てる所を 2 つずつにする。
fn row_world() -> (World, [StationId; 4], [LineId; 3]) {
    let mut w = rich_world();
    ok(&mut w, road((0, 10), (110, 10)));
    let st = [12, 42, 72, 102].map(|x| station(&mut w, (x, 8), 2));
    for (from, to) in [(12, 33), (42, 52), (52, 63), (72, 93)] {
        ok(&mut w, rail((from, 8), (to, 8)));
    }
    let [a, m, e, f] = st;
    let lines = [[a, m], [m, e], [e, f]].map(|s| line_through(&mut w, &s));
    // 列車は着いたホームで折り返すので、M と E では隣の路線と別のホームを使い、待てる所を共有しない。
    // 両端の A と F は、駅の線路をほかの路線が通らないので、ホーム 2 つに列車が入れる。
    assert_eq!(lines.map(|id| w.rail().line(id).unwrap().cap()), [4, 5, 4]);
    for id in lines {
        set_trains(&mut w, id, 3);
    }
    run(&mut w, 1_000, 3_000);
    (w, st, lines)
}

/// 区画の枠を分けるのは、辺をたどって互いに行き来できる共有の待てる所だけである（#356）。A–E を足すと、
/// A–M、M–E、A–E の待てる所が輪のある区画（枠 3）になる。E–F が折り返す E の西向きのホームは、M–E と A–E が
/// 着いて折り返す東向きのホームと別なので、E–F は区画の枠を分けない。A–E は E–F を走れなくしないので断らない。
/// 本数 0 の A–E は枠を使わず、A–M と M–E で 2 と 1 に分ける。A–E を走らせると 1 つずつになる。
#[test]
fn a_line_ending_where_another_starts_does_not_starve_it() {
    let (mut w, [a, _, e, _], [am, me, ef]) = row_world();
    let ae = create(&mut w, a, e);
    let caps = |w: &World| [am, me, ae, ef].map(|id| w.rail().line(id).unwrap().cap());
    assert_eq!(caps(&w), [2, 1, 1, 4]);
    let command = Command::SetLineTrains {
        line: ae,
        trains: 3,
    };
    assert_eq!(w.rail().lowered_by_command(&command), vec![(am, 2, 1)]);
    ok(&mut w, command);
    assert_eq!(caps(&w), [1, 1, 1, 4]);
    run(&mut w, 3_000, 3_000);
}

/// 自分の上限が 0 になる路線で、ほかの路線の上限を下げるなら断る（#356）。A–F は A–M、M–E、E–F の待てる所を
/// すべて通り、4 本で区画の枠 3 を分けると A–F の分け前が 0 になる。作っても A–F は走らず、ほかの 3 本の
/// 上限が 1 に下がって列車が消えるだけなので、断って状態を変えない。
#[test]
fn a_line_that_cannot_run_and_lowers_others_is_denied() {
    let (mut w, [a, _, _, f], lines) = row_world();
    let before = w.state_hash();
    assert_eq!(
        w.apply(&Command::CreateLine { from: a, to: f }),
        ApplyOutcome::Denied(DenyReason::LineStarvesSelf)
    );
    assert_eq!(w.state_hash(), before);
    assert_eq!(lines.map(|id| w.rail().line(id).unwrap().cap()), [4, 5, 4]);
}

/// 路線を作って走らせ、上限が下がり列車が消えた路線も、その路線を消して上限が戻れば、決めた本数まで最初の駅から
/// 出直す（#356）。
#[test]
fn trains_come_back_when_the_cap_comes_back() {
    let (mut w, [a, _, e, _], lines) = row_world();
    let ae = create(&mut w, a, e);
    set_trains(&mut w, ae, 3);
    let count = |w: &World, id: LineId| w.rail().trains().filter(|(_, t)| t.line == id).count();
    assert_eq!(count(&w, lines[0]), 1);
    ok(&mut w, Command::RemoveLine { line: ae });
    assert_eq!(lines.map(|id| w.rail().line(id).unwrap().cap()), [4, 5, 4]);
    run(&mut w, 1_000, 3_000);
    for id in lines {
        assert_eq!(count(&w, id), 3, "{id:?}");
    }
}

/// 上限と、0 のときの理由を読める。共有の区画の枠を、区画を使う路線で分けきれないと、新しい路線の上限が 0 になる。
/// W–M の区間運転を 4 本作り、前の 3 本を走らせると、区画の枠 3 を前の 3 本が 1 つずつ使い、4 本目は 0 になる。
/// 走らせる前は、どの路線も、自分だけを走らせたら配られる 3 を上限に見せる。
#[test]
fn line_capacity_shows_why_the_cap_is_zero() {
    let (mut w, [west, mid, _, _]) = two_line_world();
    let ids: Vec<LineId> = (0..4).map(|_| line_through(&mut w, &[west, mid])).collect();
    for id in &ids {
        assert_eq!(w.rail().line(*id).unwrap().cap(), 3);
    }
    for id in &ids[..3] {
        set_trains(&mut w, *id, 1);
    }
    let rail = w.rail();
    let c = rail.line_capacity(ids[0]).unwrap();
    assert_eq!(c.cap, 1);
    assert_eq!(c.wait_places, 4);
    assert_eq!(c.shared_with, ids[1..].to_vec());
    let c = rail.line_capacity(ids[3]).unwrap();
    assert_eq!(c.cap, 0);
    assert_eq!(c.wait_places, 4);
    assert_eq!(c.shared_with, ids[..3].to_vec());
}

/// 駅 W、M、E、N の街に、駅を 2〜3 つ選んだ路線を 2〜4 本と、本数 1〜16 を種で選んで 20,000 tick 回す。
/// 区間運転、同じ駅を並べた路線、分岐の先の路線がまざる。閉塞が重ならず、備えが働かず、どの列車も
/// 3,000 tick 以内に駅を出る。
fn four_station_fuzz(seed: u64) {
    let mut r = Xorshift(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let (mut w, st) = two_line_world();
    for _ in 0..2 + r.below(3) {
        let mut pool: Vec<StationId> = st.to_vec();
        let mut pick = Vec::new();
        for _ in 0..2 + r.below(2) {
            pick.push(pool.remove(r.below(pool.len() as u64) as usize));
        }
        if w.apply(&Command::CreateLine {
            from: pick[0],
            to: pick[1],
        }) != ApplyOutcome::Applied
        {
            continue;
        }
        let id = w.rail().lines().last().unwrap().0;
        if pick.len() == 3 {
            let _ = w.apply(&Command::ExtendLine {
                line: id,
                station: pick[2],
            });
        }
        let _ = w.apply(&Command::SetLineTrains {
            line: id,
            trains: 1 + r.below(16) as u8,
        });
    }
    run(&mut w, 20_000, 3_000);
}

#[test]
fn mixed_lines_on_four_stations_do_not_deadlock() {
    for seed in 0..40 {
        four_station_fuzz(seed);
    }
}

/// 路線が止まると、その路線の列車は消えて閉塞を返す。同じ駅を使うほかの路線は走り続ける。
#[test]
fn a_stopped_line_lets_go_of_a_shared_station() {
    let (mut w, [west, mid, east, north]) = two_line_world();
    let one = line_through(&mut w, &[west, mid, east]);
    let two = line_through(&mut w, &[west, mid, north]);
    set_trains(&mut w, one, 2);
    set_trains(&mut w, two, 2);
    run(&mut w, 2_000, 3_000);
    assert!(w.rail().trains().any(|(_, t)| t.line == two));
    // N へ向かう線路を取り壊すと、W–M–N の路線は進路を選べずに止まる。
    ok(
        &mut w,
        Command::Bulldoze {
            area: TileRect::from_corners(at(58, -2), at(58, -2)),
        },
    );
    assert!(w.rail().line(two).unwrap().stopped);
    assert!(!w.rail().line(one).unwrap().stopped);
    assert!(w.rail().trains().all(|(_, t)| t.line == one));
    let most = run(&mut w, 10_000, 3_000);
    assert!(w.rail().trains().all(|(_, t)| t.line == one));
    assert_eq!(most, usize::from(w.rail().line(one).unwrap().allowed()));
}

/// 上限が下がるが 0 にはならない路線は受け付ける。超えた分の列車はすぐに消え、残りは行き詰まらずに走り続ける。
/// 共有するようになった駅で先を押さえ直せない列車も消えるが、路線は上限まで出直す。
#[test]
fn a_line_lowering_another_cap_removes_only_the_excess() {
    let (mut w, [west, mid, east, north]) = two_line_world();
    let one = line_through(&mut w, &[west, mid, east]);
    set_trains(&mut w, one, 16);
    let cap_before = w.rail().line(one).unwrap().cap();
    for _ in 0..20_000 {
        w.step();
        if w.rail().train_count() == usize::from(cap_before) {
            break;
        }
    }
    assert_eq!(w.rail().train_count(), usize::from(cap_before));
    create(&mut w, west, north);
    let cap_after = w.rail().line(one).unwrap().cap();
    assert!(
        0 < cap_after && cap_after < cap_before,
        "{cap_before} → {cap_after}"
    );
    let count = |w: &World| w.rail().trains().filter(|(_, t)| t.line == one).count();
    assert!((1..=usize::from(cap_after)).contains(&count(&w)));
    run(&mut w, 10_000, 3_000);
    assert_eq!(count(&w), usize::from(cap_after));
}

/// 上りと下りですれ違う 2 本の下で線路を分けると、分けた所の分岐に 2 本がかかる。ID の大きいほうを消す。
#[test]
fn splitting_a_track_under_two_passing_trains_keeps_blocks_apart() {
    let mut w = rich_world();
    ok(&mut w, road((0, 10), (120, 10)));
    let a = station(&mut w, (12, 8), 2);
    let b = station(&mut w, (112, 8), 2);
    ok(&mut w, rail((12, 8), (103, 8)));
    let id = create(&mut w, a, b);
    set_trains(&mut w, id, 3);
    // 2 本が逆の向きに同じ線路の同じ所にいる tick を探し、その下で線路を分ける。
    for _ in 0..20_000 {
        w.step();
        let heads: Vec<(TrackId, Dir, i64)> = w
            .rail()
            .trains()
            .map(|(_, t)| (t.track, t.dir, t.offset))
            .collect();
        let found = heads.iter().enumerate().find_map(|(i, p)| {
            heads[i + 1..]
                .iter()
                .find(|q| q.0 == p.0 && q.1 != p.1)
                .map(|q| (*p, *q))
        });
        let Some((p, q)) = found else { continue };
        let line = w.rail().track_line(p.0).unwrap();
        let len = line.length().0;
        // 前向きの距離にそろえる。
        let fwd = |h: (TrackId, Dir, i64)| match h.1 {
            Dir::Forward => h.2,
            Dir::Backward => len - h.2,
        };
        let (lo, hi) = (fwd(p).min(fwd(q)), fwd(p).max(fwd(q)));
        // 2 本の先頭のあいだが 1 両より短ければ、そのあいだのマスで分けると両方の体がかかる。
        if hi - lo > TRAIN_BLOCK_LEN_MM || hi - lo < 2 * crate::network::TILE_MM {
            continue;
        }
        let k = u64::try_from((lo + hi) / 2 / crate::network::TILE_MM).unwrap();
        if k == 0 || k >= line.len {
            continue;
        }
        let t = line.tile(k);
        if w.apply(&rail((t.x, t.y), (t.x - 5, t.y - 5))) != ApplyOutcome::Applied {
            continue;
        }
        check_blocks(&w);
        run(&mut w, 5_000, 2_000);
        return;
    }
    panic!("すれ違う所が見つからない");
}

/// レビューで使った、線路のつながった駅の並びに路線と列車を置き、途中で線路と路線を変える形。
/// 線路を変えた直後に備えが働いた種を回し、閉塞が重ならず、備えが一度も働かないことを確かめる。
struct Xorshift(u64);

impl Xorshift {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n
    }
}

fn rebuilt_while_running(seed: u64) {
    let mut r = Xorshift(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut w = World::new_with_money(6, Money(1_000_000_000_000)).with_unlimited_land();
    w.lift_turn_caps(true);
    for cx in -1..=6 {
        for cy in -1..=1 {
            let _ = w.apply(&Command::BuyChunk {
                coord: ChunkCoord::new(cx, cy),
            });
        }
    }
    ok(&mut w, road((0, 10), (400, 10)));
    let k = 2 + r.below(4) as usize;
    let gaps = [1_i64, 2, 3, 5, 8, 9, 10, 15, 30, 50];
    let mut x = 3;
    let mut ends = Vec::new();
    let mut st = Vec::new();
    for _ in 0..k {
        st.push(station(&mut w, (x + 9, 8), 2));
        ends.push((x, x + 9));
        x += 9 + gaps[r.below(gaps.len() as u64) as usize];
    }
    for i in 0..k - 1 {
        ok(&mut w, rail((ends[i].1, 8), (ends[i + 1].0, 8)));
    }
    for i in 0..k - 1 {
        let (e, nw) = (ends[i].1, ends[i + 1].0);
        if r.below(100) < 60 {
            let (lo, hi) = (e + 1, nw - 1);
            if hi >= lo {
                let p = lo + r.below((hi - lo + 1) as u64) as i64;
                let to = if r.below(2) == 0 {
                    (p + 5, 3)
                } else {
                    (p - 5, 3)
                };
                let _ = w.apply(&rail((p, 8), to));
            }
        }
        if r.below(100) < 30 {
            let _ = w.apply(&rail((e, 8), (e + 5, 3)));
        }
        if r.below(100) < 30 {
            let _ = w.apply(&rail((nw, 8), (nw - 5, 3)));
        }
    }
    let mut r = Xorshift(seed.wrapping_mul(77) + 3);
    let mut ids = Vec::new();
    for _ in 0..2 + r.below(3) {
        let mut pick: Vec<StationId> = st.iter().copied().filter(|_| r.below(2) == 0).collect();
        if pick.len() < 2 {
            continue;
        }
        if r.below(2) == 0 {
            pick.reverse();
        }
        if w.apply(&Command::CreateLine {
            from: pick[0],
            to: pick[1],
        }) != ApplyOutcome::Applied
        {
            continue;
        }
        let id = w.rail().lines().last().unwrap().0;
        for s in &pick[2..] {
            let _ = w.apply(&Command::ExtendLine {
                line: id,
                station: *s,
            });
        }
        let trains = 1 + r.below(16) as u8;
        let _ = w.apply(&Command::SetLineTrains { line: id, trains });
        ids.push(id);
    }
    for tick in 0..8_000 {
        if tick % 1_500 == 700 {
            let mut kind = r.below(5);
            let x = 3 + r.below(150) as i64;
            if ids.is_empty() && (kind == 2 || kind == 3) {
                kind = 0;
            }
            let c = match kind {
                0 => rail((x, 8), (x + 5, 3)),
                1 => Command::Bulldoze {
                    area: TileRect::from_corners(at(x, 3), at(x + 5, 5)),
                },
                2 => Command::SetLineTrains {
                    line: ids[r.below(ids.len() as u64) as usize],
                    trains: r.below(17) as u8,
                },
                3 => Command::ExtendLine {
                    line: ids[r.below(ids.len() as u64) as usize],
                    station: st[r.below(st.len() as u64) as usize],
                },
                _ => Command::Bulldoze {
                    area: TileRect::from_corners(at(x, 8), at(x, 8)),
                },
            };
            let _ = w.apply(&c);
            check_blocks(&w);
        }
        w.step();
        check_blocks(&w);
        assert_eq!(
            w.rail().tally().deadlock,
            0,
            "種 {seed} の tick {tick} で行き詰まった"
        );
    }
}

/// 線路を分けて分岐ができた直後や、駅を取り壊して進路が短くなった直後に、待てる所の外で止まった列車と、
/// 上限を超えた列車と、分けた所の分岐に重なる列車を消すので、備えは働かない。
/// 4287 は線路を分けた直後、4151 は駅を取り壊した直後、5753 はほかの路線の上限を下げた直後、
/// 7277 はすれ違う 2 本の下で線路を分けた直後に、直す前は備えが働くか閉塞が重なった。
#[test]
fn rebuilding_while_trains_run_does_not_deadlock() {
    for seed in [4151, 4287, 5611, 5753, 7277] {
        rebuilt_while_running(seed);
    }
}

/// 押さえ直せない列車を消すとき、ID の小さい列車が、あとで消える大きい列車の閉塞のせいで巻き込まれて
/// 消えない。消した列車の押さえ直す先と体の閉塞が、最後の held ですべて空いていたことは一度もない。
/// 種 6028 では、直す前に ID の小さい列車が余計に消えた。
#[test]
fn settle_does_not_remove_more_trains_than_needed() {
    use super::train::OVERDELETED;
    use std::sync::atomic::Ordering;
    for seed in [6028, 4151, 4287, 5611, 5753, 7277] {
        rebuilt_while_running(seed);
    }
    assert_eq!(OVERDELETED.load(Ordering::Relaxed), 0);
}

mod ring;

/// 共有の区画の枠を分けきれず上限が 0 になっていた路線は、ほかの路線を消せば、上限が戻る。
#[test]
fn removing_a_line_restores_the_cap_of_a_starved_line() {
    let (mut w, [west, mid, _, _]) = two_line_world();
    let ids: Vec<LineId> = (0..4).map(|_| line_through(&mut w, &[west, mid])).collect();
    for id in &ids[..3] {
        set_trains(&mut w, *id, 1);
    }
    assert_eq!(w.rail().line(ids[3]).unwrap().cap(), 0);
    ok(&mut w, Command::RemoveLine { line: ids[0] });
    assert!(w.rail().line(ids[3]).unwrap().cap() > 0);
    assert!(w.rail().line_capacity(ids[3]).unwrap().cap > 0);
}

/// 路線と線路のコマンドをどう打っても、路線の上限は、書き出して読み戻して求め直した上限と同じになる
/// （求め直し忘れがない）。区画の分は終点で折り返せる本数で変わる（#377）ので、折り返せる本数で抑える形で確かめる
/// （抑えを外す印は書き出さない）。
#[test]
fn line_caps_always_match_a_fresh_plan() {
    for seed in 0..60u64 {
        let mut r = Xorshift(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let (mut w, st) = two_line_world();
        w.lift_turn_caps(false);
        let mut ids: Vec<LineId> = Vec::new();
        for step in 0..30 {
            let cmd = match r.below(7) {
                0 | 1 => Command::CreateLine {
                    from: st[r.below(4) as usize],
                    to: st[r.below(4) as usize],
                },
                2 if !ids.is_empty() => Command::ExtendLine {
                    line: ids[r.below(ids.len() as u64) as usize],
                    station: st[r.below(4) as usize],
                },
                5 if !ids.is_empty() => Command::InsertLineStation {
                    line: ids[r.below(ids.len() as u64) as usize],
                    after_index: r.below(4) as u8,
                    station: st[r.below(4) as usize],
                },
                6 if !ids.is_empty() => Command::SetLineTrains {
                    line: ids[r.below(ids.len() as u64) as usize],
                    trains: r.below(17) as u8,
                },
                3 if !ids.is_empty() => Command::RemoveLine {
                    line: ids[r.below(ids.len() as u64) as usize],
                },
                _ => Command::Bulldoze {
                    area: TileRect::from_corners(
                        at(r.below(80) as i64, 8),
                        at(r.below(80) as i64, 8),
                    ),
                },
            };
            let _ = w.apply(&cmd);
            ids = w.rail().lines().map(|(i, _)| i).collect();
            let c = World::from_snapshot(&w.to_snapshot()).unwrap();
            for (id, l) in w.rail().lines() {
                assert_eq!(
                    l.cap(),
                    c.rail().line(id).unwrap().cap(),
                    "seed {seed} step {step} {cmd:?}"
                );
            }
        }
    }
}

/// 路線の途中に駅を挿せる。a–c の路線に b を挿すと a、b、c の並びになる。
#[test]
fn inserting_a_station_puts_it_in_the_middle_of_the_line() {
    let (mut w, a, b, c) = three_in_a_row();
    let id = create(&mut w, a, c);
    ok(
        &mut w,
        Command::InsertLineStation {
            line: id,
            after_index: 0,
            station: b,
        },
    );
    assert_eq!(w.rail().line(id).unwrap().stations, vec![a, b, c]);
    assert!(!w.rail().line(id).unwrap().stopped);
}

/// 途中の駅を取り壊すと路線の並びから外れる。建て直したあと、挿せば元の位置に戻る。
#[test]
fn a_rebuilt_station_can_be_inserted_back_into_the_line() {
    let (mut w, a, b, c) = three_in_a_row();
    let id = line_through(&mut w, &[a, b, c]);
    let area = w.rail().station(b).unwrap().area;
    ok(&mut w, Command::Bulldoze { area });
    assert_eq!(w.rail().line(id).unwrap().stations, vec![a, c]);
    assert!(w.rail().line(id).unwrap().stopped);
    let b2 = station(&mut w, (62, 8), 2);
    // 駅を壊すと、駅の端につないだ外の線路も、範囲にかかったものは消えるので引きなおす。
    ok(&mut w, rail((12, 8), (53, 8)));
    ok(&mut w, rail((62, 8), (83, 8)));
    assert_eq!(w.rail().line(id).unwrap().stations, vec![a, c]);
    ok(
        &mut w,
        Command::InsertLineStation {
            line: id,
            after_index: 0,
            station: b2,
        },
    );
    assert_eq!(w.rail().line(id).unwrap().stations, vec![a, b2, c]);
    assert!(!w.rail().line(id).unwrap().stopped);
}

/// 挿せないときは `ExtendLine` と同じ理由で断る。位置が並びの外なら `NoSuchStation` である。
#[test]
fn inserting_a_station_is_denied_like_extending() {
    let (mut w, a, b, c) = three_in_a_row();
    let id = create(&mut w, a, b);
    let insert = |after_index, station, line| Command::InsertLineStation {
        line,
        after_index,
        station,
    };
    let deny = |w: &mut World, c: Command| w.apply(&c);
    assert_eq!(
        deny(&mut w, insert(0, b, id)),
        ApplyOutcome::Denied(DenyReason::StationInLine)
    );
    assert_eq!(
        deny(&mut w, insert(2, c, id)),
        ApplyOutcome::Denied(DenyReason::NoSuchStation)
    );
    assert_eq!(
        deny(
            &mut w,
            insert(
                0,
                c,
                LineId {
                    index: 9,
                    generation: 0
                }
            )
        ),
        ApplyOutcome::Denied(DenyReason::NoSuchLine)
    );
    // 線路でつながっていない駅。
    ok(&mut w, road((0, 30), (20, 30)));
    let d = station(&mut w, (12, 28), 2);
    assert_eq!(
        deny(&mut w, insert(0, d, id)),
        ApplyOutcome::Denied(DenyReason::RailNoRoute)
    );
    // 並びの最後の駅の次は、足すのと同じ。
    ok(&mut w, insert(1, c, id));
    assert_eq!(w.rail().line(id).unwrap().stations, vec![a, b, c]);
}
