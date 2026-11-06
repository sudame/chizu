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
pub(super) fn rail_town(seed: u64, trains: u8) -> (World, LineId, StationId, StationId) {
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
    // 駅の線路は、西が y = 2 の x -140〜-131、東が x 125〜134。
    for x in [-140, 125] {
        ok(w.apply(&Command::PlaceBuilding {
            kind: ServiceKind::Station,
            at: at(x, 1),
            rotation: 0,
        }));
    }
    let west = w.rail().station_at(at(-140, 2)).expect("西の駅がない");
    let east = w.rail().station_at(at(125, 2)).expect("東の駅がない");
    ok(w.apply(&Command::BuildRail {
        from: at(-131, 2),
        to: at(125, 2),
    }));
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
    ok(w.apply(&Command::CreateLine {
        from: west,
        to: east,
    }));
    let line = w.rail().lines().next().unwrap().0;
    ok(w.apply(&Command::SetLineTrains { line, trains }));
    (w, line, west, east)
}

fn run(w: &mut World, ticks: u64) {
    for _ in 0..ticks {
        w.step();
        check(w);
    }
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
            assert_eq!((r.plan.board, r.plan.line, r.plan.dir), (*s, *l, *d));
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
/// 手段ごとの移動、通勤、幸福度、車、止まった車、救済を出す。
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
                "trains {trains} day {} pop {} walk {} car {} rail {} commute_med {:?} p90 {:?} happy_x12 {} cars_x100 {} stopped_x100 {} teleported {} walked_instead {} gave_up {} fares {}",
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
