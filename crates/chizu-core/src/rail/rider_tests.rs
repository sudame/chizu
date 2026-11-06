//! 乗る人のテスト。駅で待ち、列車に乗り、降りて歩き、着くまでと、途中で路線や駅がなくなったときを確かめる。

use super::*;
use crate::command::{Command, RoadKind, Zone};
use crate::units::Money;
use crate::world::{ApplyOutcome, World};

fn at(x: i64, y: i64) -> TilePos {
    TilePos::new(x, y)
}

#[track_caller]
fn ok(outcome: ApplyOutcome) {
    assert_eq!(outcome, ApplyOutcome::Applied);
}

/// 東西に約 2km 離れた住宅（西）と工場（東）を、y = 0 の通りで結んだ街。通りの南に駅を 2 つ置き、
/// 駅のあいだを y = 2 の線路で結んで、本数 `trains` の路線を走らせる。
pub(crate) fn rail_town(seed: u64, trains: u8) -> (World, LineId, StationId, StationId) {
    let (mut w, stations) = street_town(seed, &[-140, 125], &[(-131, 125)]);
    let (west, east) = (stations[0], stations[1]);
    ok(w.apply(&Command::CreateLine {
        from: west,
        to: east,
    }));
    let line = w.rail().lines().next().unwrap().0;
    ok(w.apply(&Command::SetLineTrains { line, trains }));
    (w, line, west, east)
}

/// [`rail_town`] の通りと区画に、駅を `stations`（駅の線路の西の端の x）に置き、`rails`（x の組）の y = 2 の線路で
/// 結んだ街。路線はまだない。駅は `stations` の順に返す。
fn street_town(seed: u64, stations: &[i64], rails: &[(i64, i64)]) -> (World, Vec<StationId>) {
    // 高速道路が北から入る種を選ぶ。南から入ると、入口から通りまでの道路が線路を横切る。
    let seed = (seed..)
        .find(|s| crate::world::highway_line(*s).start.y < 0)
        .expect("北から入る種がない");
    let mut w = World::new_with_money(seed, Money(1_000_000_000)).with_unlimited_land();
    // 内側から外へ、まわりのチャンクを買い広げる。
    for ring in 2_i32..=3 {
        for cy in -ring..=ring {
            for cx in -ring..=ring {
                if i32::max(cx.abs(), cy.abs()) == ring {
                    w.apply(&Command::BuyChunk {
                        coord: crate::tile::ChunkCoord::new(cx, cy),
                    });
                }
            }
        }
    }
    let road = |from: TilePos, to: TilePos| Command::BuildRoad {
        from,
        to,
        kind: RoadKind::Medium,
    };
    ok(w.apply(&road(at(-160, 0), at(160, 0))));
    // 高速道路の入口から通りまで道路を引き、外の世界とつなぐ。
    let inner = crate::world::highway_line(w.seed()).start;
    let corner = at(inner.x, 0);
    if inner != corner {
        ok(w.apply(&road(inner, corner)));
    }
    if !(-160..=160).contains(&corner.x) {
        ok(w.apply(&road(corner, at(corner.x.clamp(-160, 160), 0))));
    }
    // 駅の線路は、y = 2 の x から x + 9 まで。
    for x in stations {
        ok(w.apply(&Command::PlaceBuilding {
            kind: ServiceKind::Station,
            at: at(*x, 1),
            rotation: 0,
        }));
    }
    let ids: Vec<StationId> = stations
        .iter()
        .map(|x| w.rail().station_at(at(*x, 2)).expect("駅がない"))
        .collect();
    for (a, b) in rails {
        ok(w.apply(&Command::BuildRail {
            from: at(*a, 2),
            to: at(*b, 2),
        }));
    }
    for (area, zone) in [
        (((-160, -6), (-110, -1)), Zone::Residential),
        (((110, -6), (160, -1)), Zone::Industrial),
    ] {
        let c = w.apply(&Command::PaintZone {
            area: TileRect::from_corners(at(area.0.0, area.0.1), at(area.1.0, area.1.1)),
            zone: Some(zone),
        });
        assert!(matches!(
            c,
            ApplyOutcome::Applied | ApplyOutcome::AppliedPartly(_)
        ));
    }
    // 高速道路の入口から通りへ下りる道路を避けて、発電所と浄水場を置く。
    let xs: Vec<i64> = [-80, -60, -40, -20, 20, 40, 60, 80]
        .into_iter()
        .filter(|x| (x - inner.x).abs() > 14)
        .collect();
    for (kind, x) in [
        (ServiceKind::PowerPlant, xs[0]),
        (ServiceKind::WaterPump, xs[1]),
        (ServiceKind::WaterPump, xs[2]),
    ] {
        ok(w.apply(&Command::PlaceBuilding {
            kind,
            at: at(x, -1),
            rotation: 2,
        }));
    }
    (w, ids)
}

fn run(w: &mut World, ticks: u64) {
    for _ in 0..ticks {
        w.step();
        check(w);
    }
}

/// 駅の徒歩圏の世帯の数は、家の正面のマスと駅の出入口のマスの直線で数える。
/// 車を持てない世帯の数は、車を持てない世帯の一覧（`carless_households`）と同じ決まりで数える。
#[test]
fn catchment_counts_households_by_the_door_tile() {
    let (mut w, _, west, east) = rail_town(5, 1);
    run(&mut w, 2 * crate::clock::TICKS_PER_DAY);
    let doors = [
        w.station_door(west).expect("西の駅の出入口がない"),
        w.station_door(east).expect("東の駅の出入口がない"),
    ];
    // 出入口は駅の敷地の外の道路側（y = 0 の通りの近く）にある
    assert!((-1..=1).contains(&doors[0].y), "{:?}", doors[0]);
    let got = w.catchments(&doors);
    let p = w.population();
    let mut want = [(0_u32, 0_u32); 2];
    for h in p.household_ids() {
        let Some(front) = p
            .home(h)
            .flatten()
            .and_then(|id| w.buildings().get(id))
            .map(|b| b.front)
        else {
            continue;
        };
        for (i, d) in doors.iter().enumerate() {
            if choice::in_walk_range(front, *d) {
                want[i].0 += 1;
                want[i].1 += u32::from(w.car_places().get(h).is_none());
            }
        }
    }
    assert!(want[0].0 > 0, "西の駅の徒歩圏に世帯がいない");
    for (g, (households, carless)) in got.iter().zip(want) {
        assert_eq!((g.households, g.carless), (households, carless));
    }
    // 東は工場だけなので、徒歩圏に家はない
    assert_eq!(got[1].households, 0);
    let all = w
        .carless_households()
        .iter()
        .filter(|c| choice::in_walk_range(c.front, doors[0]))
        .count();
    assert_eq!(got[0].carless as usize, all);
}

/// 重なる世帯は、ほかの駅の徒歩圏にも入る世帯である。同じ出入口を 2 つ並べると全員が重なり、遠い出入口は重ならない。
#[test]
fn catchment_counts_households_shared_with_other_doors() {
    let (mut w, _, west, _) = rail_town(5, 1);
    run(&mut w, 2 * crate::clock::TICKS_PER_DAY);
    let door = w.station_door(west).expect("西の駅の出入口がない");
    let far = at(door.x + 10_000, door.y);
    let alone = w.catchments(&[door]);
    let twin = w.catchments(&[door, door, far]);
    assert!(alone[0].households > 0, "徒歩圏に世帯がいない");
    assert_eq!(alone[0].shared, 0);
    assert_eq!(twin[0].shared, twin[0].households);
    assert_eq!(twin[1].shared, twin[1].households);
    assert_eq!((twin[2].households, twin[2].shared), (0, 0));
}

/// 置く前の出入口は、置いたあとの出入口と同じマスになる。置けない所は `None`。
#[test]
fn station_door_before_placing_matches_the_placed_one() {
    let (mut w, _, _, _) = rail_town(5, 1);
    // 通りの北側。南側は y = 2 の線路が通る
    let spot = at(-100, -1);
    let before = w.station_door_if_placed(spot, 2).expect("置ける所のはず");
    ok(w.apply(&Command::PlaceBuilding {
        kind: ServiceKind::Station,
        at: spot,
        rotation: 2,
    }));
    let id = w.rail().station_at(spot).expect("駅がない");
    assert_eq!(w.station_door(id), Some(before));
    assert_eq!(w.station_door_if_placed(spot, 2), None);
}

/// 乗る人と移動の整合。乗る人は休ませた移動を 1 つずつ持ち、その移動の住民がいる。待っている人は待ち行列に、
/// 乗っている人は列車の並びに 1 回ずつ入っている。列車は定員を超えない。
fn check(w: &World) {
    let rail = w.rail();
    let mut seen = std::collections::BTreeSet::new();
    for ((s, l, d), q) in rail.queues() {
        assert!(!q.is_empty());
        for id in q {
            let r = rail.rider(*id).expect("待ち行列にいない人がいる");
            assert_eq!(r.queue_key(), (*s, *l, *d));
            assert_eq!(r.train, None);
            assert!(seen.insert(*id));
        }
    }
    for (t, _) in rail.trains() {
        let list = rail.aboard(t);
        assert!(list.len() <= TRAIN_CAPACITY);
        for id in list {
            assert_eq!(rail.rider(*id).unwrap().train, Some(t));
            assert!(seen.insert(*id));
        }
    }
    assert_eq!(seen.len(), rail.rider_count(), "どこにもいない乗る人がいる");
    for (id, r) in rail.riders() {
        assert_eq!(rail.rider_of(r.trip), Some(id));
        assert!(w.trips().is_suspended(r.trip), "乗る人の移動が休んでいない");
        assert!(w.trips().commuter(r.trip).is_some(), "乗る人の住民がいない");
    }
}

/// 2km 離れた住宅から工場へ、駅まで歩き、列車に乗り、降りて歩いて着く。運賃は所持金に入る。
#[test]
fn commuters_ride_the_train_and_arrive() {
    let (mut w, _, _, _) = rail_town(5, 3);
    run(&mut w, 3 * crate::TICKS_PER_DAY);
    let t = w.rail().ride_tally().clone();
    assert!(t.boarded > 50, "乗った人が少ない: {t:?}");
    assert!(t.alighted > 0);
    assert_eq!(t.fares, t.boarded as i64 * i64::from(INITIAL_FARE));
    let y = w.traffic().yesterday.expect("前の日の集計がない");
    assert!(y.rode > 20, "鉄道で着いた移動が少ない: {y:?}");
    assert_eq!(y.walked + y.drove + y.rode, y.arrived);
    let day = w.tick().0 / crate::TICKS_PER_DAY;
    assert!(w.rail().ride_tally().fares_yesterday(day) > 0);
    assert_eq!(
        w.finances().fares.0,
        w.rail().ride_tally().fares_yesterday(day)
    );
}

/// 駅まで歩いて着いた人を駅ごとに数え、実際に歩いた時間と、手段を選んだときの見積もりを比べられる。見積もりは
/// 歩く移動と同じ歩道の経路で測るので、ずれは通りを渡るときに待った分ほど（この街では 1 割あまり）に収まる。
#[test]
fn the_walk_to_each_station_is_measured_against_the_estimate() {
    let (mut w, _, west, _) = rail_town(5, 3);
    run(&mut w, 2 * crate::TICKS_PER_DAY);
    let walks = w.rail().ride_tally().station_walks.clone();
    let at_west = walks
        .get(&west)
        .copied()
        .expect("西の駅まで歩いた人がいない");
    assert!(at_west.walks > 50, "{at_west:?}");
    let walked = at_west.average_walked().unwrap();
    let gap = at_west.average_gap().unwrap();
    assert!(walked > 0);
    assert!(
        gap.unsigned_abs() * 4 <= walked,
        "歩いた平均 {walked}、ずれ {gap}"
    );
    // 乗り換えは歩いて着いた人に数えないので、歩いて着いた人は乗った回数を超えない。
    let total: u64 = walks.values().map(|s| s.walks).sum();
    let t = w.rail().ride_tally();
    assert!(total <= t.boarded + t.gave_up + t.evicted + w.rail().rider_count() as u64);
}

/// 列車を走らせない路線は、手段の候補にならない。鉄道のない街と同じく、誰も駅へ向かわない。
#[test]
fn a_line_without_trains_is_not_a_choice() {
    let (mut w, _, _, _) = rail_town(5, 0);
    run(&mut w, 2 * crate::TICKS_PER_DAY);
    assert_eq!(w.rail().ride_tally().boarded, 0);
    assert_eq!(w.rail().rider_count(), 0);
    assert!(w.trips().iter().all(|(_, t)| t.ride_plan().is_none()));
    assert!(w.traffic().yesterday.unwrap().drove > 0);
}

/// 乗っている途中で書き出して読み戻しても、同じ状態ハッシュから同じように進む。
#[test]
fn riders_round_trip_through_the_snapshot() {
    let (mut w, _, _, _) = rail_town(5, 3);
    for _ in 0..3 * crate::TICKS_PER_DAY {
        w.step();
        if w.rail()
            .trains()
            .any(|(id, _)| !w.rail().aboard(id).is_empty())
            && w.rail().queues().next().is_some()
        {
            break;
        }
    }
    assert!(w.rail().rider_count() > 0, "乗る人がいない");
    let mut back = World::from_snapshot(&w.to_snapshot()).unwrap();
    assert_eq!(back.state_hash(), w.state_hash());
    for _ in 0..500 {
        w.step();
        back.step();
    }
    assert_eq!(back.state_hash(), w.state_hash());
}

/// 乗っている人がいる路線をなくすと、乗っている人は次に止まる駅で降り、待っている人は駅から歩く。
/// 駅を取り壊しても同じに歩きに戻り、どちらでも乗る人は残らない。
#[test]
fn removing_the_line_or_a_station_sends_riders_walking() {
    for remove_station in [false, true] {
        let (mut w, line, west, _) = rail_town(5, 3);
        let busy = |w: &World| {
            w.rail()
                .trains()
                .any(|(id, _)| !w.rail().aboard(id).is_empty())
                && w.rail().queues().next().is_some()
        };
        let mut found = false;
        for _ in 0..3 * crate::TICKS_PER_DAY {
            w.step();
            if busy(&w) {
                found = true;
                break;
            }
        }
        assert!(found, "乗っている人と待っている人が同時にいない");
        let evicted = w.rail().ride_tally().evicted;
        if remove_station {
            let area = w.rail().station(west).unwrap().area;
            ok(w.apply(&Command::Bulldoze { area }));
        } else {
            ok(w.apply(&Command::RemoveLine { line }));
        }
        assert_eq!(w.rail().rider_count(), 0, "乗る人が残った");
        assert!(w.rail().ride_tally().evicted > evicted);
        check(&w);
        // 歩きに戻った人は、移動を作り直されて歩いている。
        let p = w.population();
        for id in p.citizen_ids() {
            if let Some(t) = p.citizen(id).unwrap().trip {
                assert!(!w.trips().is_suspended(t), "休んだままの移動がある");
            }
        }
        run(&mut w, 600);
    }
}

/// 待ち行列の先頭で長く待った人は、歩きに切り替える。列車が駅を出られない路線で確かめる。
#[test]
fn riders_give_up_after_waiting_too_long() {
    let (mut w, line, _, _) = rail_town(5, 1);
    let mut riding = false;
    for _ in 0..3 * crate::TICKS_PER_DAY {
        w.step();
        if w.rail().queues().next().is_some() {
            riding = true;
            break;
        }
    }
    assert!(riding, "待っている人がいない");
    // 列車の本数を 0 にすると、走っている列車は終点で消える。待っている人は乗れずに待ちきれなくなる。
    ok(w.apply(&Command::SetLineTrains { line, trains: 0 }));
    let before = w.rail().ride_tally().gave_up;
    run(&mut w, RAIL_GIVE_UP + 2_000);
    assert_eq!(w.rail().rider_count(), 0);
    assert!(
        w.rail().ride_tally().gave_up > before,
        "待ちきれずに歩いた人がいない"
    );
}

/// 運賃は 0〜20 で決められ、運賃だけを変えた街も状態ハッシュに映る。
#[test]
fn the_fare_is_set_and_hashed() {
    let mut w = World::new(1);
    let before = w.state_hash();
    ok(w.apply(&Command::SetFare { fare: 12 }));
    assert_eq!(w.rail().fare(), 12);
    assert_ne!(w.state_hash(), before);
    assert_eq!(
        w.apply(&Command::SetFare { fare: 21 }),
        ApplyOutcome::Rejected
    );
    ok(w.apply(&Command::SetFare { fare: INITIAL_FARE }));
    assert_eq!(w.state_hash(), before);
}

/// 進路から求める止まる所と、駅の並びから作る止まる所は同じになる（ベンチマークの形が実際の路線と同じ）。
#[test]
fn stops_from_the_path_match_the_station_order() {
    let (w, line, _, _) = rail_town(5, 0);
    let l = w.rail().line(line).unwrap();
    assert_eq!(
        choice::line_stops(l),
        choice::line_stops_of(&l.stations, l.looped)
    );
    assert_eq!(l.segment_ticks().len(), l.stops().len());
    assert_eq!(l.measured_round_trip_ticks(), l.round_trip_ticks());
}

/// 乗る人を測る街（[`rail_town`]）を、列車を走らせない版と 3 本走らせる版で回し、4 日目からの 3 日の
/// 手段ごとの移動、通勤、幸福度、車、止まった車、救済と、列車にいちばん多く乗った人数と駅でいちばん長い待ち行列を出す。
/// `cargo test -p chizu-core --release measure_riders -- --ignored --nocapture` で回す。
#[test]
#[ignore]
fn measure_riders() {
    use crate::world::CarSample;
    for trains in [0, 3] {
        let (mut w, _, _, _) = rail_town(5, trains);
        for _ in 0..3 * crate::TICKS_PER_DAY {
            w.step();
        }
        for _ in 0..3 {
            let r0 = w.trips().rescues();
            let (mut cars, mut stopped, mut samples, mut happy) = (0_u64, 0_u64, 0_u64, 0_u64);
            let (mut max_aboard, mut max_queue) = (0, 0);
            let mut before = CarSample::default();
            for k in 0..crate::TICKS_PER_DAY {
                w.step();
                let rail = w.rail();
                for (t, _) in rail.trains() {
                    max_aboard = max_aboard.max(rail.aboard(t).len());
                }
                for (_, q) in rail.queues() {
                    max_queue = max_queue.max(q.len());
                }
                if k % 100 == 0 {
                    let s = w.car_sample();
                    stopped += s.stopped_on_road_since(&before) as u64;
                    cars += s.len() as u64;
                    samples += 1;
                    before = s;
                }
                if k % 600 == 0 {
                    happy += u64::from(w.happiness_summary().0);
                }
            }
            let y = w.traffic().yesterday.unwrap();
            let ct = w.commute_times();
            let r1 = w.trips().rescues();
            println!(
                "trains {trains} day {} pop {} walk {} car {} rail {} commute_med {:?} p90 {:?} happy_x12 {} cars_x100 {} stopped_x100 {} teleported {} walked_instead {} gave_up {} fares {} max_aboard {max_aboard} max_queue {max_queue}",
                w.tick().0 / crate::TICKS_PER_DAY,
                w.population().citizen_count(),
                y.walked,
                y.drove,
                y.rode,
                ct.median_seconds,
                ct.p90_seconds,
                happy,
                cars * 100 / samples,
                stopped * 100 / samples,
                r1.teleported - r0.teleported,
                r1.walked_instead - r0.walked_instead,
                w.rail().ride_tally().gave_up,
                w.rail().ride_tally().fares,
            );
        }
    }
}

/// [`rail_town`] の東の駅の先（x 200〜209）に 3 つ目の駅を置き、東の駅から線路でつないだ街。
/// `extend` なら路線をその駅まで延ばす。
fn far_town(trains: u8, extend: bool) -> (World, LineId, [StationId; 3]) {
    let (mut w, line, west, east) = rail_town(5, trains);
    let c = w.apply(&Command::BuildRoad {
        from: at(160, 0),
        to: at(220, 0),
        kind: RoadKind::Medium,
    });
    assert!(matches!(
        c,
        ApplyOutcome::Applied | ApplyOutcome::AppliedPartly(_)
    ));
    ok(w.apply(&Command::PlaceBuilding {
        kind: ServiceKind::Station,
        at: at(200, 1),
        rotation: 0,
    }));
    let far = w.rail().station_at(at(200, 2)).expect("3 つ目の駅がない");
    ok(w.apply(&Command::BuildRail {
        from: at(134, 2),
        to: at(200, 2),
    }));
    if extend {
        ok(w.apply(&Command::ExtendLine { line, station: far }));
    }
    (w, line, [west, east, far])
}

/// 駅 `s` の出入口。
fn station_door(w: &World, s: StationId) -> crate::movement::SidewalkPos {
    let site = w.rail().station(s).expect("駅がない").site();
    crate::movement::facility_spot(w.network(), &site).expect("駅の出入口がない")
}

/// `rail` の列車 `train` に、駅 `board` から向き `dir` で乗って駅 `alight` で降りる人を 1 人乗せ、その移動の ID を返す。
/// 移動は作らず、鉄道の中だけで確かめる。
fn put_aboard(
    rail: &mut Rail,
    train: TrainId,
    (board, line, dir, alight): (StationId, LineId, u8, StationId),
    door: crate::movement::SidewalkPos,
) -> crate::id::TripId {
    let trip = crate::id::TripId {
        index: 1_000_000 + u32::try_from(rail.rider_count()).unwrap(),
        generation: 0,
    };
    let id = rail.riders.insert(Rider {
        trip,
        started: crate::units::Tick::ZERO,
        plan: RidePlan {
            board,
            line,
            dir,
            alight,
            transfer: None,
            to: door,
            walk_estimate: 0,
        },
        leg: 0,
        door,
        since: 0,
        train: Some(train),
    });
    rail.by_trip.insert(trip, id);
    rail.aboard.entry(train).or_default().push(id);
    trip
}

/// 移動 `trip` の人が鉄道を離れたときの [`Exit`]。離れていなければ `None`。
fn exit_of(rail: &mut Rail, trip: crate::id::TripId) -> Option<Exit> {
    rail.take_exits().into_iter().find(|e| e.trip == trip)
}

/// 最初の駅で出るのを待つ列車（終点で折り返したあとの要素 0 にいる）が路線ごと消えると、乗っていた人は
/// その駅で降りる。反対の終点まで運ばれない。
#[test]
fn riders_of_a_train_ready_at_the_first_station_get_off_there() {
    let (mut w, line, west, east) = rail_town(5, 16);
    let train = (0..2_000)
        .find_map(|_| {
            w.step();
            w.rail()
                .trains()
                .find(|(_, t)| t.pos == 0 && t.state == TrainState::Ready)
                .map(|(id, _)| id)
        })
        .expect("最初の駅で待つ列車がない");
    let mut rail = w.rail().clone();
    let door = station_door(&w, west);
    let trip = put_aboard(&mut rail, train, (west, line, 0, east), door);
    rail.remove_line(line).unwrap();
    let e = exit_of(&mut rail, trip).expect("降りていない");
    assert_eq!(e.from, ExitFrom::Station(west));
    assert!(e.rode);
}

/// 終点の駅を取り壊して路線が縮み、新しい進路に置けずに消える列車の乗客は、前の進路で次に止まる駅で降りる。
#[test]
fn riders_of_a_train_cut_off_by_a_shorter_line_get_off_at_its_next_stop() {
    let (mut w, line, [west, east, far]) = far_town(3, true);
    // 帰り（向き 1）に、3 つ目の駅から東の駅へ向かって、駅のあいだの東の駅に近い線路（x 134〜167）を走る列車。
    // 3 つ目の駅に接する線路（x 167〜200）は駅と一緒に取り壊され、そこに体のかかる列車は進路を選び直す前に
    // 消えるので、尾がその線路を抜けた列車を選ぶ。
    let near_east = |w: &World, t: &Train| {
        let rail = w.rail();
        let l = rail.line(t.line).unwrap();
        let tr = rail.track_line(t.track).unwrap();
        rail.track(t.track).unwrap().station.is_none()
            && l.half(t.pos) == 1
            && t.state == TrainState::Running
            && t.offset > 2 * TRAIN_BLOCK_LEN_MM
            && rail
                .reserved_steps(l, t)
                .iter()
                .all(|i| rail.track_line(l.step(*i).track).unwrap().end().x <= 167)
            && [tr.start, tr.end()]
                .iter()
                .all(|p| (134..=170).contains(&p.x))
    };
    let train = (0..3 * crate::TICKS_PER_DAY)
        .find_map(|_| {
            w.step();
            w.rail()
                .trains()
                .find(|(_, t)| near_east(&w, t))
                .map(|(id, _)| id)
        })
        .expect("東の駅へ向かう列車がない");
    let mut rail = w.rail().clone();
    let door = station_door(&w, far);
    let trip = put_aboard(&mut rail, train, (far, line, 1, west), door);
    let area = rail.station(far).unwrap().area;
    rail.bulldoze(area);
    assert!(rail.train(train).is_none(), "列車が残った");
    let e = exit_of(&mut rail, trip).expect("降りていない");
    assert_eq!(e.from, ExitFrom::Station(east));
    assert!(e.rode);
}

/// 終点に止まる列車に、折り返して戻る向きの人が乗っているときに路線を延ばすと、列車は新しい終点へ進むので、
/// その人はいま止まっている駅で降りる。逆の向きへ運ばれない。
#[test]
fn riders_heading_back_get_off_when_the_line_is_extended_past_their_terminal() {
    let (mut w, line, [west, east, far]) = far_town(3, false);
    let train = (0..3 * crate::TICKS_PER_DAY)
        .find_map(|_| {
            w.step();
            let rail = w.rail();
            rail.trains()
                .find(|(_, t)| {
                    let s = rail.line(t.line).unwrap().step(t.pos);
                    s.kind == StepKind::Terminal
                        && s.station == Some(east)
                        && matches!(t.state, TrainState::Dwell { .. })
                })
                .map(|(id, _)| id)
        })
        .expect("東の終点に止まる列車がない");
    let mut rail = w.rail().clone();
    let door = station_door(&w, east);
    let back = put_aboard(&mut rail, train, (east, line, 1, west), door);
    rail.extend_line(line, far).unwrap();
    assert!(rail.train(train).is_some(), "列車が消えた");
    let e = exit_of(&mut rail, back).expect("降りていない");
    assert_eq!(e.from, ExitFrom::Station(east));
    assert!(e.rode);
}

/// 列車が駅を出るときに乗りきれずに人が残ると、その駅と向きを覚え、鉄道の候補の待ちに運転間隔を 1 つ足す。
/// 次に出た列車のあとに人が残らなければ外す。
#[test]
fn riders_left_behind_add_a_headway_to_the_wait() {
    let (mut w, line, west, east) = rail_town(5, 3);
    let train = (0..3 * crate::TICKS_PER_DAY)
        .find_map(|_| {
            w.step();
            let rail = w.rail();
            rail.trains()
                .find(|(_, t)| {
                    let l = rail.line(t.line).unwrap();
                    l.step(t.pos).station == Some(west)
                        && l.half(t.pos) == 0
                        && t.state == TrainState::Ready
                })
                .map(|(id, _)| id)
        })
        .expect("西の駅を出る列車がない");
    let planner = w.rail_planner().unwrap().clone();
    let end = |s| {
        let door = station_door(&w, s);
        choice::WalkEnd::new(w.network(), door.tile, door)
    };
    let (from, to) = (end(west), end(east));
    let before = choice::RideChoice::new(&planner, w.rail())
        .offer(&from, &to)
        .unwrap();
    let mut rail = w.rail().clone();
    let now = w.tick().0;
    let door = station_door(&w, west);
    let key = (west, line, 0);
    let waiting = rail.queues.get(&key).map_or(0, |q| q.len());
    let room = TRAIN_CAPACITY - rail.aboard(train).len();
    for k in 0..room + 100 {
        let trip = crate::id::TripId {
            index: 1_000_000 + u32::try_from(k).unwrap(),
            generation: 0,
        };
        let plan = RidePlan {
            board: west,
            line,
            dir: 0,
            alight: east,
            transfer: None,
            to: door,
            walk_estimate: 0,
        };
        rail.add_rider(trip, crate::units::Tick(now), plan, door, now);
    }
    rail.serve_departure(train, now);
    assert_eq!(rail.aboard(train).len(), TRAIN_CAPACITY);
    assert_eq!(rail.queues.get(&key).map_or(0, |q| q.len()), waiting + 100);
    assert!(rail.left_behind().contains(&key));
    let after = choice::RideChoice::new(&planner, &rail)
        .offer(&from, &to)
        .unwrap();
    let l = rail.line(line).unwrap();
    let running = rail.trains().filter(|(_, t)| t.line == line).count() as u64;
    let headway = l.segment_ticks().iter().map(|s| u64::from(*s)).sum::<u64>() / running;
    assert_eq!(after.cost, before.cost + headway);
    // 残った人がいなくなってから列車が出れば、外す。
    rail.queues.remove(&key);
    rail.serve_departure(train, now);
    assert!(!rail.left_behind().contains(&key));
}

/// [`rail_town`] の通りの南に、駅を西（x -140）、真ん中（x -5）、東（x 125）の 3 つ置き、西と真ん中を結ぶ路線と、
/// 真ん中と東を結ぶ路線を、それぞれ本数 `trains` で走らせる街。西の住宅から東の工場へは、真ん中の駅で
/// 乗り換えないと鉄道で行けない。路線は作った順に、駅は西から順に返す。
pub(crate) fn transfer_town(trains: u8) -> (World, [LineId; 2], [StationId; 3]) {
    let (mut w, s) = street_town(5, &[-140, -5, 125], &[(-131, -5), (4, 125)]);
    let mut lines = Vec::new();
    for (from, to) in [(s[0], s[1]), (s[1], s[2])] {
        ok(w.apply(&Command::CreateLine { from, to }));
        let line = w.rail().lines().last().unwrap().0;
        ok(w.apply(&Command::SetLineTrains { line, trains }));
        lines.push(line);
    }
    (w, [lines[0], lines[1]], [s[0], s[1], s[2]])
}

/// 本数を変えても、上限で抑えられて走らせる本数が変わらなければ、駅の組の表を捨てない。走らせる本数が変われば
/// 捨てる。
#[test]
fn setting_trains_above_the_cap_keeps_the_table() {
    let (mut w, [line, _], _) = transfer_town(3);
    let cap = w.rail().line(line).unwrap().cap();
    assert!(cap >= 2, "上限が小さすぎる: {cap}");
    ok(w.apply(&Command::SetLineTrains { line, trains: cap }));
    assert!(w.rail_planner().is_some());
    ok(w.apply(&Command::SetLineTrains {
        line,
        trains: crate::rail::line::MAX_LINE_TRAINS,
    }));
    assert_eq!(w.rail().line(line).unwrap().allowed(), cap);
    assert!(w.has_rail_planner(), "走らせる本数が同じなのに表を捨てた");
    ok(w.apply(&Command::SetLineTrains {
        line,
        trains: cap - 1,
    }));
    assert!(
        !w.has_rail_planner(),
        "走らせる本数が変わったのに表を捨てなかった"
    );
}

/// 駅まで歩く表は、路線と本数だけを変えたときは使い回し、道路の網が変わったとき（横断歩道を外したときも）は
/// 作り直す。どちらでも、いまの網と駅から作った表と同じになる。
#[test]
fn walk_tables_are_reused_until_the_network_changes() {
    let (mut w, [line, _], _) = transfer_town(3);
    let fresh = |w: &World| {
        let walk = crate::routing::Graph::build(w.network(), crate::network::LaneMode::Sidewalk);
        choice::RailPlanner::build(w.rail(), w.network(), &walk)
    };
    let same = |w: &mut World| {
        let want = fresh(w);
        let got = w.rail_planner().unwrap().clone();
        assert_eq!(got, want, "使い回した表が作り直した表と違う");
    };
    same(&mut w);
    let built = w.walk_table_builds();
    ok(w.apply(&Command::SetLineTrains { line, trains: 1 }));
    assert!(!w.has_rail_planner());
    same(&mut w);
    assert_eq!(
        w.walk_table_builds(),
        built,
        "路線だけを変えたのに作り直した"
    );
    // 道路を引くと作り直す。
    ok(w.apply(&Command::BuildRoad {
        from: at(-150, 0),
        to: at(-150, -8),
        kind: RoadKind::Small,
    }));
    same(&mut w);
    assert_eq!(w.walk_table_builds(), built + 1);
    // 横断歩道を外すコマンドは表を捨てないが、網の版が変わるので作り直す。
    let node = w.network().node_at(at(-150, 0)).expect("交差点がない");
    ok(w.apply(&Command::SetCrosswalk {
        node,
        arm: crate::command::Heading::E,
        on: false,
    }));
    same(&mut w);
    assert_eq!(w.walk_table_builds(), built + 2);
}

/// 乗り換える駅で待っている人（2 回目の乗車）と、乗り換える前の路線の列車に乗っていて、同じ路線へ乗り換える人。
/// 移動の ID と、乗り換え先の路線である。
fn transfer_pair(rail: &Rail) -> Option<(crate::id::TripId, crate::id::TripId, LineId)> {
    rail.riders().find_map(|(_, waiting)| {
        if waiting.leg != 1 || waiting.train.is_some() {
            return None;
        }
        let line = waiting.leg_route().1;
        let aboard = rail.riders().find(|(_, r)| {
            r.leg == 0 && r.train.is_some() && r.plan.transfer.is_some_and(|t| t.line == line)
        })?;
        Some((waiting.trip, aboard.1.trip, line))
    })
}

/// 乗り換えの組（[`transfer_pair`]）ができるまで回す。
fn run_until_transfer_pair(w: &mut World) -> (crate::id::TripId, crate::id::TripId, LineId) {
    for _ in 0..3 * crate::TICKS_PER_DAY {
        w.step();
        if let Some(p) = transfer_pair(w.rail()) {
            return p;
        }
    }
    panic!("乗り換える駅で待つ人と、乗り換える前の人が同時にいない");
}

/// 西の住宅から東の工場へ、真ん中の駅で乗り換えて着く。乗り換えは真ん中の駅でだけ起き、運賃は乗り換えても
/// 1 回分である。
#[test]
fn commuters_transfer_at_the_shared_station_and_arrive() {
    let (mut w, [first, second], [_, mid, _]) = transfer_town(3);
    run(&mut w, 3 * crate::TICKS_PER_DAY);
    let t = w.rail().ride_tally().clone();
    assert!(t.transfers > 50, "乗り換えた人が少ない: {t:?}");
    assert_eq!(t.station_transfers.get(&mid).copied(), Some(t.transfers));
    assert_eq!(t.station_transfers.len(), 1);
    let on = |l| t.line_transfers.get(&l).copied().unwrap_or(0);
    assert_eq!(on(first) + on(second), t.transfers);
    // 朝は 2 本目に、夕方は 1 本目に乗り換える。
    assert!(on(first) > 0 && on(second) > 0, "{t:?}");
    let fare = i64::from(INITIAL_FARE);
    let boarded = i64::try_from(t.boarded).unwrap();
    let transfers = i64::try_from(t.transfers).unwrap();
    assert!(t.fares < boarded * fare, "乗り換えで運賃を払った: {t:?}");
    assert!(t.fares >= (boarded - transfers) * fare, "{t:?}");
    let y = w.traffic().yesterday.expect("前の日の集計がない");
    assert!(y.rode > 20, "鉄道で着いた移動が少ない: {y:?}");
}

/// 乗り換え先の路線をなくすと、乗り換える駅で待っていた人はその駅から歩く。乗り換える前の路線に乗っていた人は、
/// 乗り換える駅まで乗り、そこから歩く。
#[test]
fn riders_walk_from_the_transfer_station_when_the_next_line_is_removed() {
    let (mut w, _, [_, mid, _]) = transfer_town(3);
    let (waiting, aboard, line) = run_until_transfer_pair(&mut w);
    let mut rail = w.rail().clone();
    rail.remove_line(line).unwrap();
    let mut exits = rail.take_exits();
    let e = *exits
        .iter()
        .find(|e| e.trip == waiting)
        .expect("待っていた人が歩いていない");
    assert_eq!(e.from, ExitFrom::Station(mid));
    // 1 本目には乗ったので、鉄道で移動したと数える。
    assert!(e.rode);
    assert!(rail.rider_of(aboard).is_some(), "乗り換える前の人が降りた");
    let mut tick = w.tick();
    for _ in 0..2_000 {
        if exits.iter().any(|e| e.trip == aboard) {
            break;
        }
        tick = tick.next();
        rail.step(tick);
        exits.extend(rail.take_exits());
    }
    let e = *exits
        .iter()
        .find(|e| e.trip == aboard)
        .expect("乗り換える前の人が降りていない");
    assert_eq!(e.from, ExitFrom::Station(mid));
    assert!(e.rode);
    assert_eq!(rail.rider_of(aboard), None);
    // 街の中でも、乗り換えられなくなった人は歩きに戻り、乗る人の整合は崩れない。
    ok(w.apply(&Command::RemoveLine { line }));
    check(&w);
    run(&mut w, 1_500);
    assert!(
        w.rail().riders().all(|(_, r)| r.leg_route().1 != line),
        "なくした路線を待つ人が残った"
    );
}

/// 乗り換える駅を取り壊すと、そこで乗り換えを待っていた人は、取り壊した駅の敷地の出入口から歩く。
#[test]
fn riders_waiting_at_a_bulldozed_transfer_station_walk_from_its_site() {
    let (mut w, _, [_, mid, _]) = transfer_town(3);
    let (waiting, _, _) = run_until_transfer_pair(&mut w);
    let mut rail = w.rail().clone();
    let site = rail.station(mid).unwrap().site();
    rail.bulldoze(site.area);
    let e = rail
        .take_exits()
        .into_iter()
        .find(|e| e.trip == waiting)
        .expect("待っていた人が歩いていない");
    assert_eq!(e.from, ExitFrom::Site(site));
    assert!(e.rode);
    ok(w.apply(&Command::Bulldoze { area: site.area }));
    check(&w);
    run(&mut w, 600);
}

/// 乗り換える駅で待つ人がいるときに書き出して読み戻しても、同じ状態ハッシュから同じように進む。
#[test]
fn transfer_riders_round_trip_through_the_snapshot() {
    let (mut w, _, _) = transfer_town(3);
    run_until_transfer_pair(&mut w);
    let mut back = World::from_snapshot(&w.to_snapshot()).unwrap();
    assert_eq!(back.state_hash(), w.state_hash());
    let waiting = |w: &World| {
        w.rail()
            .riders()
            .filter(|(_, r)| r.leg == 1 && r.train.is_none())
            .map(|(id, r)| (id, *r))
            .collect::<Vec<_>>()
    };
    assert!(!waiting(&w).is_empty());
    assert_eq!(waiting(&back), waiting(&w));
    for _ in 0..500 {
        w.step();
        back.step();
    }
    assert_eq!(back.state_hash(), w.state_hash());
    assert_eq!(
        back.rail().ride_tally().transfers,
        w.rail().ride_tally().transfers
    );
}

/// 乗り換えの街（[`transfer_town`]）を、乗り換えを使わない版と使う版で回し、4 日目からの 3 日の手段ごとの移動、
/// 乗り換えの数、通勤、幸福度、車、止まった車、救済を出す。乗り換えを使わない版は、直接行ける路線がないので、
/// 鉄道のない街と同じ移動になる。
/// `cargo test -p chizu-core --release measure_transfers -- --ignored --nocapture` で回す。
#[test]
#[ignore]
fn measure_transfers() {
    use crate::world::CarSample;
    for transfers in [false, true] {
        let (mut w, _, _) = transfer_town(3);
        if !transfers {
            w.disable_rail_transfers();
        }
        for _ in 0..3 * crate::TICKS_PER_DAY {
            w.step();
        }
        for _ in 0..3 {
            let r0 = w.trips().rescues();
            let x0 = w.rail().ride_tally().transfers;
            let (mut cars, mut stopped, mut samples, mut happy) = (0_u64, 0_u64, 0_u64, 0_u64);
            let mut before = CarSample::default();
            for k in 0..crate::TICKS_PER_DAY {
                w.step();
                if k % 100 == 0 {
                    let s = w.car_sample();
                    stopped += s.stopped_on_road_since(&before) as u64;
                    cars += s.len() as u64;
                    samples += 1;
                    before = s;
                }
                if k % 600 == 0 {
                    happy += u64::from(w.happiness_summary().0);
                }
            }
            let y = w.traffic().yesterday.unwrap();
            let ct = w.commute_times();
            let r1 = w.trips().rescues();
            println!(
                "transfers {transfers} day {} pop {} walk {} car {} rail {} transferred {} commute_med {:?} p90 {:?} happy_x12 {} cars_x100 {} stopped_x100 {} teleported {} walked_instead {} gave_up {}",
                w.tick().0 / crate::TICKS_PER_DAY,
                w.population().citizen_count(),
                y.walked,
                y.drove,
                y.rode,
                w.rail().ride_tally().transfers - x0,
                ct.median_seconds,
                ct.p90_seconds,
                happy,
                cars * 100 / samples,
                stopped * 100 / samples,
                r1.teleported - r0.teleported,
                r1.walked_instead - r0.walked_instead,
                w.rail().ride_tally().gave_up,
            );
        }
    }
}
