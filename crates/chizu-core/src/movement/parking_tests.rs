use super::parking::MissTable;
use super::*;
use crate::building::{Building, Buildings, ParkingCut};
use crate::car_place::CarPlace;
use crate::command::{RoadKind, Zone, ZoneKind};
use crate::id::BuildingId;
use crate::network::{Dir, Line};
use crate::rail::choice::WalkEnd;
use crate::rng::{RngPurpose, rng};
use crate::service::Facility;
use crate::state_hash::state_hash_of;
use crate::tile::TileRect;
use crate::units::Money;

/// 東西の道路と、その北側に並べた 1 マスの建物。
struct Street {
    net: Network,
    router: AstarRouter,
    buildings: Buildings,
    trips: Trips,
}

impl Street {
    /// 建物を `xs` のマスに建てる。どれも段階 1 で、駐車場は 1 台ぶんである。
    fn new(xs: &[i64]) -> (Street, Vec<BuildingId>) {
        Street::with_roads(xs, &[(0, 60)])
    }

    /// x が `roads` の各組の範囲の、東西の道路を引いて、建物を `xs` のマスに建てる。
    fn with_roads(xs: &[i64], roads: &[(i64, i64)]) -> (Street, Vec<BuildingId>) {
        let mut net = Network::new();
        for (a, b) in roads {
            let line = Line::between(TilePos::new(*a, 0), TilePos::new(*b, 0)).unwrap();
            net.build_road(&line, RoadKind::Small).unwrap();
        }
        let mut buildings = Buildings::default();
        let ids = xs
            .iter()
            .map(|x| {
                let front = TilePos::new(*x, -1);
                let segment = net.segments_through(TilePos::new(*x, 0))[0];
                // 西向きの歩道の始まりは、道路の東の端である。
                let east = net.segment_line(segment).unwrap().end().x;
                let entrance_at = LaneOffset(u32::try_from(east - x).unwrap() * 8_000);
                buildings.insert(Building {
                    zone: Zone::Commercial,
                    area: TileRect::from_corners(front, front),
                    front,
                    toward_road: (0, 1),
                    entrance: LinkId {
                        segment,
                        dir: Dir::Backward,
                        mode: LaneMode::Sidewalk,
                    },
                    level: 1,
                    entrance_at,
                    leveled_at: Tick::ZERO,
                    supply: Default::default(),
                    form: Default::default(),
                })
            })
            .collect();
        let router = AstarRouter::new(&net);
        let s = Street {
            net,
            router,
            buildings,
            trips: Trips::default(),
        };
        (s, ids)
    }

    fn drive(&mut self, from: BuildingId, to: BuildingId) -> Option<TripId> {
        let (net, router) = (&self.net, &mut self.router);
        self.trips
            .spawn_drive(net, router, (&self.buildings, &[]), (from, to), Tick::ZERO)
    }

    fn step(&mut self) -> Vec<TripId> {
        let ended = self.trips.step(&self.net, &mut self.router, Tick::ZERO);
        self.check();
        ended
    }

    /// 駐車場の整合と車間。停めてある台数と押さえた台数の和は台数を超えず、押さえた台数は向かっている
    /// 主体の数と同じで、駐車場は建っている建物にだけある。同じ車線の車は車間を保つ。
    fn check(&self) {
        let mut by_lane: Vec<(LaneId, LaneOffset)> = self
            .trips
            .iter()
            .filter(|(_, t)| car::on_road(t))
            .map(|(_, t)| (t.lane(), t.offset()))
            .collect();
        by_lane.sort();
        for w in by_lane.windows(2) {
            if w[0].0 == w[1].0 {
                assert!(w[1].1.0 - w[0].1.0 >= CAR_LENGTH + MIN_GAP, "{w:?}");
            }
        }
        let mut holding = std::collections::BTreeMap::<BuildingId, u32>::new();
        for (_, t) in self.trips.iter() {
            if let Some(lot) = t
                .then
                .lot()
                .and_then(LotId::building)
                .filter(|b| self.buildings.get(*b).is_some())
            {
                *holding.entry(lot).or_default() += 1;
            }
        }
        for (id, lot) in self.trips.lots() {
            let b = self.buildings.get(id).expect("消えた建物に駐車場が残った");
            assert!(
                u64::from(lot.parked + lot.reserved) <= b.parking_spaces(self.buildings.seed())
            );
            assert_eq!(lot.reserved, holding.remove(&id).unwrap_or(0), "{id:?}");
        }
        assert!(
            holding.is_empty(),
            "押さえた空きが数えられていない {holding:?}"
        );
    }

    /// `ids` の移動がどれも駐車場の出入りの位置まで歩いて車に乗るまで進める。
    fn board(&mut self, ids: &[TripId]) {
        for _ in 0..20 {
            let car = |id: &TripId| {
                self.trips
                    .get(*id)
                    .is_some_and(|t| t.mode() == LaneMode::Car)
            };
            if ids.iter().all(car) {
                return;
            }
            self.step();
        }
        panic!("車に乗らない");
    }

    /// 終わるまで進め、その tick ごとの手段の列（同じ手段が続く所は 1 つにまとめる）を返す。
    fn run(&mut self, id: TripId) -> Vec<LaneMode> {
        let mut modes = Vec::new();
        for _ in 0..1_000 {
            if let Some(t) = self.trips.get(id)
                && modes.last() != Some(&t.mode())
            {
                modes.push(t.mode());
            }
            if self.step().contains(&id) {
                return modes;
            }
        }
        panic!("着かない");
    }
}

impl Street {
    /// 車で走っている移動 `id` が車を降りて歩き出す（駐車場に着く）tick まで進める。
    fn run_until_parked(&mut self, id: TripId) {
        let mut drove = false;
        for _ in 0..1_000 {
            let mode = self.trips.get(id).expect("移動が終わった").mode();
            drove |= mode == LaneMode::Car;
            if drove && mode == LaneMode::Sidewalk {
                return;
            }
            self.step();
        }
        panic!("着かない");
    }
}

fn lot(s: &Street, b: BuildingId) -> (u32, u32) {
    let l = s.trips.lot(b);
    (l.parked, l.reserved)
}

#[test]
fn a_drive_walks_to_the_car_drives_parks_and_walks_on() {
    let (mut s, ids) = Street::new(&[50, 5]);
    let (home, shop) = (ids[0], ids[1]);
    let id = s.drive(home, shop).unwrap();
    assert_eq!(s.trips.get(id).unwrap().mode(), LaneMode::Sidewalk);
    assert_eq!(lot(&s, shop), (0, 1));
    let modes = s.run(id);
    assert_eq!(
        modes,
        [LaneMode::Sidewalk, LaneMode::Car, LaneMode::Sidewalk]
    );
    assert_eq!(lot(&s, shop), (1, 0));
    // 停めた車で帰ると、店の駐車場が空き、家の駐車場に停まる。
    let back = s.drive(shop, home).unwrap();
    assert_eq!((lot(&s, shop), lot(&s, home)), ((0, 0), (0, 1)));
    s.run(back);
    assert_eq!(lot(&s, home), (1, 0));
}

#[test]
fn a_full_destination_sends_the_car_to_the_nearest_free_lot() {
    let (mut s, ids) = Street::new(&[30, 5, 9, 1, 40]);
    let (home, shop, near, other, far) = (ids[0], ids[1], ids[2], ids[3], ids[4]);
    s.drive(home, shop).unwrap();
    // 店は押さえてあるので、4 マス先の 2 つのうち、番号の小さい建物にする。
    let second = s.drive(home, shop).unwrap();
    assert_eq!(lot(&s, near), (0, 1));
    s.drive(home, shop).unwrap();
    assert_eq!(lot(&s, other), (0, 1));
    // 遠い建物は範囲の外なので、どこも満車で車では行けない。満車で車をやめた数のほかは変わらない。
    let mut before = s.trips.clone();
    assert_eq!(s.drive(far, shop), None);
    before.parking_tally.lots_full += 1;
    before.lot_misses = s.trips.lot_misses.clone();
    assert_eq!(s.trips, before);
    let modes = s.run(second);
    assert_eq!(
        modes,
        [LaneMode::Sidewalk, LaneMode::Car, LaneMode::Sidewalk]
    );
    assert_eq!(lot(&s, near), (1, 0));
    let walked = s.trips.get(second);
    assert!(walked.is_none());
}

/// 満車でほかの駐車場に回っても、出発の tick は最初の区切りのものを引き継ぐ。通勤の時間はここから数える。
#[test]
fn a_redirected_drive_keeps_its_departure_tick() {
    let (mut s, ids) = Street::new(&[30, 5, 9, 1, 40]);
    let (home, shop, near) = (ids[0], ids[1], ids[2]);
    s.drive(home, shop).unwrap();
    let (net, router) = (&s.net, &mut s.router);
    let second = s
        .trips
        .spawn_drive(net, router, (&s.buildings, &[]), (home, shop), Tick(77))
        .unwrap();
    assert_eq!(lot(&s, near), (0, 1), "近くの駐車場に回っていない");
    let modes = s.run(second);
    assert_eq!(
        modes,
        [LaneMode::Sidewalk, LaneMode::Car, LaneMode::Sidewalk]
    );
    assert_eq!(
        s.trips.finished(second),
        Some(Finished {
            reached: true,
            departed: Tick(77),
            via: crate::movement::Via::Car,
        })
    );
}

#[test]
fn drives_sent_elsewhere_and_given_up_on_full_lots_are_counted() {
    let (mut s, ids) = Street::new(&[30, 5, 9, 1, 40]);
    let (home, shop, far) = (ids[0], ids[1], ids[4]);
    for _ in 0..3 {
        s.drive(home, shop).unwrap();
    }
    assert_eq!(s.drive(far, shop), None);
    let hash = state_hash_of(&s.trips);
    assert_eq!(
        s.trips.parking_tally(),
        ParkingTally {
            drives: 3,
            parked_elsewhere: 2,
            lots_full: 1,
            no_car: 0,
            no_car_in_use: 0,
            no_car_parked_away: 0,
            no_car_at_home: 0,
            no_car_other: 0,
            no_place: 0,
        }
    );
    // 数は状態ハッシュに入らない。
    let mut other = s.trips.clone();
    other.parking_tally = ParkingTally::default();
    other.lot_misses = Default::default();
    assert_eq!(state_hash_of(&other), hash);
}

#[test]
fn misses_are_counted_per_destination_building() {
    let (mut s, ids) = Street::new(&[30, 5, 9, 1, 40]);
    let (home, shop, near, far) = (ids[0], ids[1], ids[2], ids[4]);
    for _ in 0..3 {
        s.drive(home, shop).unwrap();
    }
    // 満車で車をやめる数は、出発した建物でなく、行き先の建物に数える。
    assert_eq!(s.drive(far, shop), None);
    assert_eq!(s.drive(far, shop), None);
    assert_eq!(s.drive(far, near), None);
    let missed: Vec<_> = s.trips.lot_misses(0).collect();
    let at = |id| missed.iter().find(|(b, _)| *b == id).map(|(_, m)| *m);
    assert_eq!(
        at(shop),
        Some(DailyLotMisses {
            today: LotMisses {
                lots_full: 2,
                parked_elsewhere: 2
            },
            yesterday: LotMisses::default(),
        })
    );
    assert_eq!(at(home), None);
    assert!(missed.windows(2).all(|w| w[0].0 < w[1].0), "番号の順");
    // 数は状態ハッシュに入らない。
    let hash = state_hash_of(&s.trips);
    let mut other = s.trips.clone();
    other.lot_misses = Default::default();
    assert_eq!(state_hash_of(&other), hash);
}

#[test]
fn misses_roll_over_by_day() {
    let mut t = MissTable::default();
    let b = BuildingId {
        index: 0,
        generation: 0,
    };
    t.add(b, 3, |m| m.lots_full += 2);
    let at = |t: &MissTable, day| t.at(day).map(|(_, m)| m).next();
    let two = LotMisses {
        lots_full: 2,
        parked_elsewhere: 0,
    };
    assert_eq!(
        at(&t, 3).map(|m| (m.today, m.yesterday)),
        Some((two, LotMisses::default()))
    );
    // 次の日には前の日に送られ、そのまた次の日には消える。
    assert_eq!(
        at(&t, 4).map(|m| (m.today, m.yesterday)),
        Some((LotMisses::default(), two))
    );
    assert_eq!(at(&t, 5), None);
    // 書き込みも日をまたぐと送る。
    t.add(b, 4, |m| m.parked_elsewhere += 1);
    let m = at(&t, 4).unwrap();
    assert_eq!((m.today.parked_elsewhere, m.yesterday), (1, two));
}

#[test]
fn cars_sent_to_another_lot_are_told_apart_until_they_park() {
    let (mut s, ids) = Street::new(&[30, 5, 9]);
    let (home, shop) = (ids[0], ids[1]);
    let first = s.drive(home, shop).unwrap();
    let second = s.drive(home, shop).unwrap();
    let mut seen = Vec::new();
    for _ in 0..1_000 {
        let (a, b) = (s.trips.get(first), s.trips.get(second));
        if a.is_none() && b.is_none() {
            break;
        }
        assert!(!a.is_some_and(|t| t.parks_elsewhere(&s.net, &s.buildings, &[], None)));
        if let Some(t) = b {
            seen.push((t.mode(), t.parks_elsewhere(&s.net, &s.buildings, &[], None)));
        }
        s.step();
    }
    seen.dedup();
    // 車まで歩くあいだと走るあいだは、ほかの建物へ回された車と分かる。停めて歩き出したら分からなくてよい。
    assert_eq!(
        seen,
        [
            (LaneMode::Sidewalk, true),
            (LaneMode::Car, true),
            (LaneMode::Sidewalk, false),
        ]
    );
}

#[test]
fn leaving_cars_do_not_free_spaces_held_for_arriving_cars() {
    let (mut s, ids) = Street::new(&[50, 5, 9]);
    let (home, shop, near) = (ids[0], ids[1], ids[2]);
    // 店の 1 台ぶんを、向かっている車のために押さえる。
    let coming = s.drive(home, shop).unwrap();
    // 店から出る車は、店に停めてある車がないので、押さえた空きを減らさない。
    s.drive(shop, home).unwrap();
    assert_eq!(lot(&s, shop), (0, 1));
    // 次に店へ向かう車は、近くの建物へ回る。
    s.drive(home, shop).unwrap();
    assert_eq!(lot(&s, near), (0, 1));
    s.run(coming);
    assert_eq!(lot(&s, shop), (1, 0));
}

#[test]
fn two_cars_leaving_one_building_in_the_same_tick_do_not_overlap() {
    let (mut s, ids) = Street::new(&[50, 5, 8]);
    let (home, shop) = (ids[0], ids[1]);
    let a = s.drive(home, shop).unwrap();
    let b = s.drive(home, shop).unwrap();
    // 同じ距離を歩いて、2 台とも同じ tick に車に乗る。車間の検査は `step` のたびに走る。
    s.board(&[a]);
    assert_eq!(s.trips.get(b).unwrap().mode(), LaneMode::Car);
    let mut ended = Vec::new();
    for _ in 0..300 {
        ended.extend(s.step());
    }
    ended.sort();
    assert_eq!(ended, [a, b]);
}

#[test]
fn cars_boarding_past_a_junction_leave_room_for_a_car_entering_from_it() {
    // 交差点 (10,0) から東へ延びる道路の (14,0) の建物から、4 台が同じ tick に車に乗る。
    let (mut s, ids) = Street::with_roads(&[14, 50, 52, 54, 56], &[(0, 10), (10, 60)]);
    let north = Line::between(TilePos::new(10, 0), TilePos::new(10, 10)).unwrap();
    s.net.build_road(&north, RoadKind::Small).unwrap();
    s.router = AstarRouter::new(&s.net);
    let on_street = |x| SidewalkPos {
        tile: TilePos::new(x, 0),
        step: (1, 0),
        along: LaneOffset(0),
    };
    // 西から交差点へ向かう車がいて、4 台が駐車場まで歩くあいだに交差点の手前まで来る。
    let approach = s.trips.spawn_drives(
        &s.net,
        &mut s.router,
        &[(on_street(6), on_street(59))],
        Tick::ZERO,
    )[0]
    .unwrap();
    let cars: Vec<TripId> = ids[1..]
        .iter()
        .map(|to| s.drive(ids[0], *to).unwrap())
        .collect();
    s.board(&cars);
    let rows: Vec<&Trip> = cars.iter().map(|id| s.trips.get(*id).unwrap()).collect();
    assert!(rows.iter().all(|t| t.mode() == LaneMode::Car));
    // 3 台は車 2 台ぶんまで下げて入り、4 台めは交差点から入る車の空きを取らずに路肩で待つ。
    let waiting: Vec<bool> = rows.iter().map(|t| t.waiting_to_depart()).collect();
    assert_eq!(waiting, [false, false, false, true]);
    let mut ended = Vec::new();
    for _ in 0..400 {
        ended.extend(s.step());
    }
    assert!(ended.contains(&approach));
    assert!(
        cars.iter().all(|id| s.trips.get(*id).is_none()),
        "着かない車がある"
    );
}

#[test]
fn an_unusable_nearest_lot_falls_back_to_the_next_one() {
    // 西の道路は東の道路とつながっていない。
    let (mut s, ids) = Street::with_roads(&[55, 40, 27, 54], &[(0, 28), (32, 60)]);
    let (home, shop, cut_off, usable) = (ids[0], ids[1], ids[2], ids[3]);
    s.drive(home, shop).unwrap();
    // 店は満車で、いちばん近い 13 マス先の建物には行けないので、14 マス先の建物にする。
    s.drive(home, shop).unwrap();
    assert_eq!((lot(&s, cut_off), lot(&s, usable)), ((0, 0), (0, 1)));
}

#[test]
fn a_trip_lost_on_reroute_returns_its_space() {
    let (mut s, ids) = Street::new(&[50, 5]);
    let (home, shop) = (ids[0], ids[1]);
    let id = s.drive(home, shop).unwrap();
    for _ in 0..3 {
        s.step();
    }
    let all = TileRect::from_corners(TilePos::new(0, 0), TilePos::new(60, 0));
    s.net.bulldoze(all).unwrap();
    s.router.sync(&s.net);
    assert_eq!(s.trips.reroute(&s.net, &mut s.router), [id]);
    assert_eq!(lot(&s, shop), (0, 0));
    s.check();
}

#[test]
fn a_trip_without_a_next_leg_returns_its_space() {
    let (mut s, ids) = Street::with_roads(&[50, 5], &[(0, 30), (30, 60)]);
    let (home, shop) = (ids[0], ids[1]);
    let id = s.drive(home, shop).unwrap();
    // 歩いて駐車場へ向かうあいだに、店の前の道路を消す。家の前の歩きは続けられる。
    let west = TileRect::from_corners(TilePos::new(5, 0), TilePos::new(5, 0));
    s.net.bulldoze(west).unwrap();
    s.router.sync(&s.net);
    assert_eq!(s.trips.reroute(&s.net, &mut s.router), []);
    let ended = (0..20).flat_map(|_| s.step()).collect::<Vec<_>>();
    assert_eq!(ended, [id]);
    assert_eq!(lot(&s, shop), (0, 0));
}

#[test]
fn a_drive_keeps_its_legs_when_the_road_is_split() {
    let (mut s, ids) = Street::new(&[50, 5]);
    let (home, shop) = (ids[0], ids[1]);
    let id = s.drive(home, shop).unwrap();
    for _ in 0..3 {
        s.step();
    }
    assert_eq!(s.trips.get(id).unwrap().mode(), LaneMode::Car);
    // 走っている道路の先を、交わる道路で分ける。
    let line = Line::between(TilePos::new(30, -5), TilePos::new(30, 5)).unwrap();
    s.net.build_road(&line, RoadKind::Small).unwrap();
    s.router.sync(&s.net);
    assert_eq!(s.trips.reroute(&s.net, &mut s.router), []);
    assert_eq!(s.run(id), [LaneMode::Car, LaneMode::Sidewalk]);
    assert_eq!(lot(&s, shop), (1, 0));
}

#[test]
fn removed_buildings_drop_their_parked_cars_and_reservations() {
    let (mut s, ids) = Street::new(&[50, 5, 20]);
    let (home, shop, mall) = (ids[0], ids[1], ids[2]);
    let first = s.drive(home, shop).unwrap();
    s.run(first);
    assert_eq!(lot(&s, shop), (1, 0));
    let to_mall = s.drive(home, mall).unwrap();
    for _ in 0..3 {
        s.step();
    }
    assert_eq!(lot(&s, mall), (0, 1));
    s.buildings.remove(mall);
    s.buildings.remove(shop);
    s.trips.forget_lots(&s.buildings);
    assert_eq!(s.trips.lots().count(), 0);
    // 消えた駐車場へ向かっていた車は、着いても停めずに、歩いて行き先へ向かう。
    let modes = s.run(to_mall);
    assert_eq!(modes, [LaneMode::Car, LaneMode::Sidewalk]);
    assert_eq!(s.trips.lots().count(), 0);
    assert_eq!(s.drive(home, mall), None);
}

/// 建物の並んだ道路で、種から決めた建物どうしを行き来させ、途中で建物を消す。作った移動の数、終わった順、
/// 状態ハッシュを返す。
fn busy_street() -> (u32, Vec<TripId>, u64) {
    let xs: Vec<i64> = (0..20).map(|k| 3 * k).collect();
    let (mut s, ids) = Street::new(&xs);
    let (mut ended, mut spawned) = (Vec::new(), 0);
    for t in 0..2_000_u64 {
        if t < 200 {
            let mut r = rng(7, RngPurpose::DailySchedule, 0, Tick(t));
            let pick = |r: &mut crate::rng::Pcg32| ids[r.below(20) as usize];
            let (a, b) = (pick(&mut r), pick(&mut r));
            if s.buildings.get(a).is_some() {
                spawned += u32::from(s.drive(a, b).is_some());
            }
        }
        if t == 100 {
            for id in ids.iter().step_by(3) {
                s.buildings.remove(*id);
            }
            s.trips.forget_lots(&s.buildings);
            s.check();
        }
        ended.extend(s.step());
    }
    assert!(s.trips.is_empty(), "着かずに残った移動がある");
    (spawned, ended, state_hash_of(&s.trips))
}

#[test]
fn parking_stays_consistent_and_does_not_depend_on_the_thread_count() {
    let one = busy_street();
    assert!(one.0 > 30, "作れた移動が少ない {}", one.0);
    assert_eq!(one.1.len(), one.0 as usize);
    assert_eq!(busy_street(), one);
    #[cfg(feature = "parallel")]
    for threads in [1, 3, 8] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        assert_eq!(pool.install(busy_street), one);
    }
}

#[test]
fn building_spots_sit_on_the_road_in_front() {
    let (s, ids) = Street::new(&[0, 7, 60]);
    for id in ids {
        let b = s.buildings.get(id).unwrap();
        let (link, off) = building_spot(&s.net, b).unwrap().resolve(&s.net).unwrap();
        assert_eq!(link, b.entrance);
        // 西向きの歩道なので、始まり（東の端）からの距離は (60 - x) マスである。
        let want = u32::try_from(60 - b.front.x).unwrap() * 8_000;
        assert_eq!(off, LaneOffset(want));
    }
}

#[test]
fn the_drawn_shift_slides_between_the_sidewalk_and_either_lane() {
    // 建物は道路の北側にある。西へ向かう車は建物の側の車線を、東へ向かう車は向こう側の車線を走る。
    // どちらでも、歩道から車線へ、車線から歩道へ移るときに、描く位置が横に飛ばない。
    for (from, to) in [(0, 1), (1, 0)] {
        let (mut s, ids) = Street::new(&[40, 10]);
        let id = s.drive(ids[from], ids[to]).unwrap();
        let shown = |s: &Street| {
            let min = WorldPos::new(Mm(-800_000), Mm(-800_000));
            let max = WorldPos::new(Mm(800_000), Mm(800_000));
            s.trips
                .movers_in(&s.net, min, max)
                .into_iter()
                .find(|m| m.id == id)
                .map(|m| ((m.pos.x.0, m.pos.y.0), (m.shift.0.0, m.shift.1.0), m.mode))
        };
        let mut before = shown(&s);
        let mut modes: Vec<LaneMode> = before.iter().map(|b| b.2).collect();
        for _ in 0..1_000 {
            if s.step().contains(&id) {
                break;
            }
            let after = shown(&s);
            if let (Some((p0, s0, _)), Some((p1, s1, mode))) = (before, after) {
                let dist = (p1.0 - p0.0).abs().max((p1.1 - p0.1).abs());
                let dshift = (s1.0 - s0.0).abs().max((s1.1 - s0.1).abs());
                assert!(
                    dshift <= 3 * dist + 50,
                    "{from}->{to}: {dist}mm 進むあいだに、ずれが {dshift}mm 変わった"
                );
                if modes.last() != Some(&mode) {
                    modes.push(mode);
                }
            }
            before = after.or(before);
        }
        assert_eq!(
            modes,
            [LaneMode::Sidewalk, LaneMode::Car, LaneMode::Sidewalk],
            "{from}->{to}"
        );
    }
}

#[test]
fn between_150_and_500_m_only_households_with_a_car_drive() {
    use crate::citizens::Activity;
    use crate::id::{CitizenId, HouseholdId};
    use crate::movement::parking::Then;
    // 30 マスは 240m である。
    let (mut s, ids) = Street::new(&[0, 30]);
    let who = |index| Commuter {
        citizen: CitizenId {
            index,
            generation: 0,
        },
        household: HouseholdId {
            index,
            generation: 0,
        },
        to: Activity::Work,
    };
    let request = |index, owns_car| CommuteRequest {
        who: who(index),
        from: ids[0],
        to: ids[1],
        home: None,
        place: Some(CarPlace::Home),
        owns_car,
        savings: Money::ZERO,
        parking_fee: 0,
    };
    let requests = [request(0, true), request(1, false)];
    let (net, router) = (&s.net, &mut s.router);
    let out = s
        .trips
        .start_commutes(net, router, (&s.buildings, &[]), &requests, Tick::ZERO);
    let drives = |i: usize| {
        let t = s.trips.get(out[i].unwrap()).unwrap();
        matches!(t.then, Then::Drive { .. })
    };
    assert!(drives(0), "車を持つ世帯が歩いた");
    assert!(!drives(1), "車を持たない世帯が車に乗った");
}

#[test]
fn walking_instead_of_driving_starts_on_the_building_side() {
    // 家の出入口は北の側の、西向きの歩道にある。東の店へ向かう車は、東向きの車線（建物と反対の向き）から出る。
    let (mut s, ids) = Street::new(&[5, 50]);
    let id = s.drive(ids[0], ids[1]).unwrap();
    for _ in 0..10 {
        if s.trips.get(id).is_some_and(|t| t.mode() == LaneMode::Car) {
            break;
        }
        s.step();
    }
    let car = s.trips.get(id).unwrap();
    assert_eq!(car.mode(), LaneMode::Car);
    assert_eq!(car.link().dir, Dir::Forward);
    assert!(car.walk.far);
    let mut ended = Vec::new();
    s.trips
        .walk_instead(&s.net, &mut s.router, vec![id.slot()], &mut ended);
    assert!(ended.is_empty());
    let walker = s.trips.get(id).unwrap();
    assert_eq!(walker.mode(), LaneMode::Sidewalk);
    // 建物の側（西向きの歩道のリンクの右）にいる。東へ歩くなら左の側である。
    assert_eq!(walker.link().dir == Dir::Backward, !walker.walk.far);
    assert_eq!(walker.walk.jay, None);
}

/// 東西の道路の北側の、`front_x` のマスを東の端にして西へ間口 `w` マス、奥へ `h` マス延びる建物。
fn wide(net: &Network, zone: ZoneKind, front_x: i64, (w, h): (i64, i64), level: u8) -> Building {
    let front = TilePos::new(front_x, -1);
    let segment = net.segments_through(TilePos::new(front_x, 0))[0];
    let east = net.segment_line(segment).unwrap().end().x;
    Building {
        zone: Zone::from(zone),
        area: TileRect::from_corners(front, TilePos::new(front_x - (w - 1), -h)),
        front,
        toward_road: (0, 1),
        entrance: LinkId {
            segment,
            dir: Dir::Backward,
            mode: LaneMode::Sidewalk,
        },
        entrance_at: LaneOffset(u32::try_from(east - front_x).unwrap() * 8_000),
        level,
        leveled_at: Tick::ZERO,
        supply: Default::default(),
        form: Default::default(),
    }
}

/// x が `xs` のマスで、東西の道路に南から道路が交わる。
fn street_with_junctions(xs: &[i64]) -> Network {
    let mut net = Network::new();
    let line = Line::between(TilePos::new(0, 0), TilePos::new(60, 0)).unwrap();
    net.build_road(&line, RoadKind::Small).unwrap();
    for x in xs {
        let side = Line::between(TilePos::new(*x, 0), TilePos::new(*x, 10)).unwrap();
        net.build_road(&side, RoadKind::Small).unwrap();
    }
    net
}

/// 出入りの位置の、x のワールド座標（mm）。
fn spot_x(net: &Network, b: &Building) -> i64 {
    let p = lot_spot(net, b, 0).unwrap();
    assert!(p.resolve_on(net, LaneMode::Car).is_some());
    assert!(p.resolve_on(net, LaneMode::Sidewalk).is_some());
    p.world().x.0
}

/// マス `x` の真ん中の x のワールド座標（mm）。
fn mid(x: i64) -> i64 {
    x * 8_000 + 4_000
}

#[test]
fn a_side_lot_opens_at_the_end_far_from_the_nearest_junction() {
    let net = street_with_junctions(&[10, 50]);
    // 4×4 の商業の段階 1 は 8 台で、端の 1 列を切り欠く。出入りの位置は、端の列のマスの真ん中である。
    let near_west = wide(&net, ZoneKind::Commercial, 14, (4, 4), 1);
    assert!(matches!(
        near_west.parking_layout(0).cut,
        ParkingCut::Side { columns: 1, .. }
    ));
    // 西の交差点から 1 マスの西の端（x = 11）より、4 マスの東の端（x = 14）が遠い。
    assert_eq!(spot_x(&net, &near_west), mid(14));
    // 東の交差点のそばでは、西の端（x = 46）にする。出入口（x = 49）とは別の所になる。
    let near_east = wide(&net, ZoneKind::Commercial, 49, (4, 4), 1);
    assert_eq!(spot_x(&net, &near_east), mid(46));
    assert_ne!(
        lot_spot(&net, &near_east, 0),
        building_spot(&net, &near_east)
    );
    // 2 つの交差点のうち近いほうまでの距離で比べる。x = 27〜30 なら、西の端は西の交差点から 17 マス、
    // 東の端は東の交差点から 20 マスで、東の端が遠い。x = 30〜33 なら逆になる。
    let middle = wide(&net, ZoneKind::Commercial, 30, (4, 4), 1);
    assert_eq!(spot_x(&net, &middle), mid(30));
    let middle = wide(&net, ZoneKind::Commercial, 33, (4, 4), 1);
    assert_eq!(spot_x(&net, &middle), mid(30));
}

#[test]
fn a_front_strip_opens_in_the_middle_of_its_slots() {
    let net = street_with_junctions(&[10]);
    // 1 マスの商業の段階 1 は 1 台で、帯は枠 1 つ（2.5m）。交差点から遠い東の辺に寄せ、その真ん中から出入りする。
    let one = wide(&net, ZoneKind::Commercial, 11, (1, 1), 1);
    assert_eq!(
        one.parking_layout(0).cut,
        ParkingCut::FrontStrip { slots: 1 }
    );
    assert_eq!(spot_x(&net, &one), mid(11) + 4_000 - 1_250);
    // 間口 2 マスの工業の段階 4 は 6 台で、帯は 15m。東の端の辺から 7.5m 内に入った所である。
    let two = wide(&net, ZoneKind::Industrial, 13, (2, 1), 4);
    assert_eq!(
        two.parking_layout(0).cut,
        ParkingCut::FrontStrip { slots: 6 }
    );
    assert_eq!(spot_x(&net, &two), mid(13) + 4_000 - 7_500);
}

#[test]
fn without_a_junction_the_lot_opens_at_the_larger_coordinate() {
    let net = street_with_junctions(&[]);
    let b = wide(&net, ZoneKind::Commercial, 30, (4, 4), 1);
    assert_eq!(spot_x(&net, &b), mid(30));
    let b = wide(&net, ZoneKind::Commercial, 30, (1, 1), 1);
    assert_eq!(spot_x(&net, &b), mid(30) + 2_750);
}

#[test]
fn a_lot_at_the_dead_end_of_a_road_stays_on_the_road() {
    // 道路の西の端（x = 0）のマスの建物。交差点から遠い西の側は道路の外になるので、マスの真ん中にする。
    let net = street_with_junctions(&[3]);
    let b = wide(&net, ZoneKind::Commercial, 0, (1, 1), 1);
    assert_eq!(spot_x(&net, &b), mid(0));
}

#[test]
fn arriving_cars_turn_into_the_lot_without_stopping_the_lane() {
    // 家から西へ、店（x = 20）へ向かう車と、その先（x = 5）へ向かう車が続いて走る。
    let (mut s, ids) = Street::new(&[50, 20, 5]);
    let (home, shop, far) = (ids[0], ids[1], ids[2]);
    let first = s.drive(home, shop).unwrap();
    s.board(&[first]);
    for _ in 0..3 {
        s.step();
    }
    let second = s.drive(home, far).unwrap();
    s.board(&[second]);
    let mut prev = std::collections::BTreeMap::new();
    let (mut first_min, mut follow_min) = (i32::MAX, i32::MAX);
    for _ in 0..300 {
        for id in [first, second] {
            let Some(t) = s.trips.get(id).filter(|t| car::on_road(t)) else {
                continue;
            };
            let v = t.speed().0;
            if let Some(before) = prev.insert(id, v) {
                assert!(
                    before - v <= CAR_DECEL,
                    "減速の上限を破った {before} -> {v}"
                );
            }
            if id == first && t.offset() > LaneOffset(16_000) {
                first_min = first_min.min(v);
            }
        }
        // 先の車が車線にいるあいだの、走り出したあとの後ろの車の速さ。
        if s.trips.get(first).is_some_and(car::on_road)
            && let Some(t) = s.trips.get(second).filter(|t| car::on_road(t))
            && (t.speed().0 > 0 || follow_min != i32::MAX)
        {
            follow_min = follow_min.min(t.speed().0);
        }
        if s.step().contains(&second) {
            break;
        }
    }
    // 先の車は、走り出したあとは入る速さより落とさずに駐車場へ入り、後ろの車も止まらない。
    assert_eq!(lot(&s, shop), (1, 0));
    assert!(first_min >= car::PARK_ENTRY_SPEED, "{first_min}");
    assert!(follow_min > 0, "後ろの車が止まった");
}

#[test]
fn a_lot_spanning_two_unjoined_roads_opens_onto_the_road_of_its_door() {
    // 西の道路（x = 0〜20）と東の道路（x = 21〜40）は隣り合うが、つながっていない。
    let (mut s, _) = Street::with_roads(&[], &[(0, 20), (21, 40)]);
    // 東の道路の x = 24 に交差点がある。x = 19〜22 の建物の出入口は東の道路の x = 22 にあり、
    // 交差点から遠い西の端（x = 19）は西の道路に面する。
    let side = Line::between(TilePos::new(24, 0), TilePos::new(24, 10)).unwrap();
    s.net.build_road(&side, RoadKind::Small).unwrap();
    s.router = AstarRouter::new(&s.net);
    let west = s.net.segments_through(TilePos::new(19, 0))[0];
    let east = s.net.segments_through(TilePos::new(22, 0))[0];
    assert_ne!(west, east);
    let b = wide(&s.net, ZoneKind::Commercial, 22, (4, 4), 1);
    let (link, _) = lot_spot(&s.net, &b, s.buildings.seed())
        .unwrap()
        .resolve_on(&s.net, LaneMode::Sidewalk)
        .unwrap();
    assert_eq!(link.segment, east);
    assert_eq!(spot_x(&s.net, &b), mid(22));
    // 出入口から出入りの位置まで歩いて、車で出られる。
    let shop = s.buildings.insert(b);
    let to = s
        .buildings
        .insert(wide(&s.net, ZoneKind::Commercial, 35, (1, 1), 1));
    let id = s.drive(shop, to).unwrap();
    s.board(&[id]);
    assert_eq!(s.run(id), [LaneMode::Car, LaneMode::Sidewalk]);
}

#[test]
fn a_lot_just_past_a_node_is_entered_by_slowing_on_the_link_before() {
    // x = 10 で 2 本に分かれた道路を東から西へ走り、つなぎ目を抜けてすぐの店（x = 9）に入る。
    // つなぎ目は交差点でないので、出入りの位置は座標の大きい東の辺の側で、最後のリンクの始まりから
    // 5.25m にある。最高速度から入る速さまで落とすには、手前のリンクから減速が要る。
    let (mut s, ids) = Street::with_roads(&[50, 9], &[(0, 10), (10, 60)]);
    let (home, shop) = (ids[0], ids[1]);
    let id = s.drive(home, shop).unwrap();
    s.board(&[id]);
    let last = {
        let t = s.trips.get(id).unwrap();
        assert!(t.route.links.len() >= 2, "交差点を抜けない経路");
        t.route.links.len() - 1
    };
    let (mut prev, mut slowed_before, mut speeds) = (None::<(u32, i32)>, false, Vec::new());
    for _ in 0..300 {
        let Some(t) = s.trips.get(id).filter(|t| car::on_road(t)) else {
            if s.trips
                .get(id)
                .is_some_and(|t| t.mode() == LaneMode::Sidewalk)
                && prev.is_some()
            {
                break;
            }
            s.step();
            continue;
        };
        let v = t.speed().0;
        if let Some((leg, before)) = prev {
            assert!(
                before - v <= CAR_DECEL,
                "減速の上限を破った {before} -> {v}"
            );
            // この tick の速さは、tick の始めにいたリンクの上で決めた。
            if (leg as usize) < last && v < before {
                slowed_before = true;
            }
        }
        speeds.push((t.leg, v));
        prev = Some((t.leg, v));
        s.step();
    }
    assert_eq!(lot(&s, shop), (1, 0), "{speeds:?}");
    assert!(slowed_before, "手前のリンクで減速しない {speeds:?}");
    let (_, arrive) = *speeds.last().unwrap();
    assert!(arrive >= car::PARK_ENTRY_SPEED, "{speeds:?}");
    // 落としすぎない。出入りの位置は最後のリンクの 5.25m 先なので、つなぎ目を抜けた tick には、
    // 入る速さより 1 tick の減速ぶん以上速い。
    let (_, entered) = *speeds
        .iter()
        .find(|(leg, _)| *leg as usize == last)
        .unwrap();
    assert!(
        entered > car::PARK_ENTRY_SPEED + CAR_DECEL,
        "手前で落としすぎた {speeds:?}"
    );
}

#[test]
fn a_car_bound_for_a_removed_building_is_not_counted_as_parking_elsewhere() {
    let (mut s, ids) = Street::new(&[50, 5]);
    let (home, shop) = (ids[0], ids[1]);
    let id = s.drive(home, shop).unwrap();
    s.board(&[id]);
    let t = s.trips.get(id).unwrap();
    assert!(!t.parks_elsewhere(&s.net, &s.buildings, &[], None));
    s.buildings.remove(shop);
    s.trips.forget_lots(&s.buildings);
    let t = s.trips.get(id).unwrap();
    assert!(!t.parks_elsewhere(&s.net, &s.buildings, &[], None));
}

#[test]
fn residential_lots_take_only_their_own_residents() {
    let (mut s, ids) = Street::new(&[30, 5]);
    let (work, shop) = (ids[0], ids[1]);
    let house = s
        .buildings
        .insert(wide(&s.net, ZoneKind::Residential, 9, (1, 1), 1));
    let other = s
        .buildings
        .insert(wide(&s.net, ZoneKind::Residential, 8, (1, 1), 1));
    assert!(s.buildings.parking_spaces(house) > 0);
    let drive = |s: &mut Street, home: Option<BuildingId>| {
        let (net, router) = (&s.net, &mut s.router);
        let id = s
            .trips
            .spawn_drive_taking(
                net,
                router,
                (&s.buildings, &[]),
                (work, house),
                None,
                (home.map(LotId::Building), home),
                Tick::ZERO,
            )
            .unwrap();
        s.trips.get(id).unwrap().parking_lot().unwrap()
    };
    // 住民でない人は、行き先の住宅にも、隣の住宅にも停めず、近くの店に停める。
    assert_eq!(drive(&mut s, None), LotId::Building(shop));
    assert_eq!(lot(&s, other), (0, 0));
    // 住民は、自分の家の駐車場に停める。隣の住宅の住民は、近くにある自分の家に停める。
    assert_eq!(drive(&mut s, Some(house)), LotId::Building(house));
    assert_eq!(drive(&mut s, Some(other)), LotId::Building(other));
}

/// 同じ世帯（番号 0）の住民が、`Street` の建物のあいだを通勤する。家は `home` である。
struct Household {
    home: BuildingId,
}

impl Household {
    const ID: HouseholdId = HouseholdId {
        index: 0,
        generation: 0,
    };

    fn citizen(index: u32) -> CitizenId {
        CitizenId {
            index,
            generation: 0,
        }
    }

    fn request(&self, index: u32, (from, to): (BuildingId, BuildingId)) -> CommuteRequest {
        CommuteRequest {
            who: Commuter {
                citizen: Household::citizen(index),
                household: Household::ID,
                to: crate::citizens::Activity::Work,
            },
            from,
            to,
            home: Some(self.home),
            place: Some(CarPlace::Home),
            owns_car: true,
            savings: Money::ZERO,
            parking_fee: 0,
        }
    }

    fn start(&self, s: &mut Street, requests: &[(u32, (BuildingId, BuildingId))]) -> Vec<TripId> {
        let requests: Vec<CommuteRequest> =
            requests.iter().map(|(i, e)| self.request(*i, *e)).collect();
        let (net, router) = (&s.net, &mut s.router);
        s.trips
            .start_commutes(net, router, (&s.buildings, &[]), &requests, Tick::ZERO)
            .into_iter()
            .map(|id| id.expect("移動を作れない"))
            .collect()
    }

    fn drives(s: &Street, id: TripId) -> bool {
        matches!(s.trips.get(id).unwrap().then, parking::Then::Drive { .. })
    }

    /// 住民 `index` が `from` から `to` へ車で行き、停めて着くまで進める。
    fn park(&self, s: &mut Street, index: u32, ends: (BuildingId, BuildingId)) {
        let id = self.start(s, &[(index, ends)])[0];
        assert!(Household::drives(s, id));
        s.run(id);
        s.trips.take_commuter(id);
        assert_eq!(
            s.trips.household_car(Household::ID),
            Some(HouseholdCar::Parked {
                lot: LotId::Building(ends.1),
                by: Household::citizen(index)
            })
        );
    }
}

use crate::id::{CitizenId, HouseholdId};

#[test]
fn a_household_shares_one_car() {
    // 30 マスは 240m で、車を持つ世帯なら車で向かう。
    let (mut s, ids) = Street::new(&[0, 30]);
    let (home, work) = (ids[0], ids[1]);
    let h = Household { home };
    // 同じ世帯の 2 人が同時に出ると、先の 1 人だけが車に乗り、もう 1 人は歩く。
    let out = h.start(&mut s, &[(0, (home, work)), (1, (home, work))]);
    assert!(Household::drives(&s, out[0]));
    assert!(!Household::drives(&s, out[1]), "2 台目の車に乗った");
    assert_eq!(s.trips.parking_tally().no_car, 1);
    s.run(out[0]);
    s.trips.take_commuter(out[0]);
    s.trips.cancel(out[1]);
    // 車が職場に停めてあるあいだ、家から出る人は車を使えない。
    let walk = h.start(&mut s, &[(1, (home, work))]);
    assert!(!Household::drives(&s, walk[0]));
    assert_eq!(s.trips.parking_tally().no_car, 2);
    s.trips.cancel(walk[0]);
    // 同じ職場で働く人も、ほかの人が職場に停めた車には乗れない。
    let other = h.start(&mut s, &[(1, (work, home))]);
    assert!(!Household::drives(&s, other[0]));
    s.trips.cancel(other[0]);
    // 停めた本人は、職場から車で帰れる。
    let back = h.start(&mut s, &[(0, (work, home))]);
    assert!(Household::drives(&s, back[0]));
    assert_eq!(lot(&s, work), (0, 0));
    assert_eq!(
        s.trips.household_car(Household::ID),
        Some(HouseholdCar::InUse(back[0]))
    );
    s.run(back[0]);
    s.trips.take_commuter(back[0]);
    // 家に停めた車は、世帯の誰でも家から乗れる。
    let next = h.start(&mut s, &[(1, (home, work))]);
    assert!(Household::drives(&s, next[0]));
    assert_eq!(lot(&s, home), (0, 0));
    // 停めずに移動が終わったら、車はどこにもないものとし、次に出る人の出発の建物から現れる。
    s.trips.cancel(next[0]);
    assert_eq!(s.trips.household_car(Household::ID), None);
    let again = h.start(&mut s, &[(0, (home, work))]);
    assert!(Household::drives(&s, again[0]));
}

/// 車を置く枠のない世帯は、車で向かうほど遠くても歩き、その数を `no_place` に数える。
/// 枠をなくす前に停めた車が残っていれば、駐車場から外す。
#[test]
fn a_household_without_a_car_place_walks() {
    let (mut s, ids) = Street::new(&[0, 30]);
    let (home, work) = (ids[0], ids[1]);
    let h = Household { home };
    h.park(&mut s, 0, (home, work));
    assert_eq!(lot(&s, work), (1, 0));
    let request = CommuteRequest {
        place: None,
        ..h.request(0, (work, home))
    };
    let (net, router) = (&s.net, &mut s.router);
    let out = s
        .trips
        .start_commutes(net, router, (&s.buildings, &[]), &[request], Tick::ZERO);
    let id = out[0].expect("歩く移動を作れない");
    assert!(!Household::drives(&s, id), "枠のない世帯が車に乗った");
    let t = s.trips.parking_tally();
    assert_eq!((t.no_place, t.no_car, t.lots_full), (1, 0, 0));
    assert_eq!(s.trips.household_car(Household::ID), None);
    assert_eq!(lot(&s, work), (0, 0));
}

/// 家に帰る移動でも、枠のない世帯は車に乗らない。枠を持つ世帯は、車で帰って自宅の駐車場に停める。
#[test]
fn going_home_without_a_place_walks_and_with_one_parks_at_home() {
    let (mut s, ids) = Street::new(&[0, 30]);
    let (home, work) = (ids[0], ids[1]);
    let h = Household { home };
    let request = CommuteRequest {
        place: None,
        ..h.request(0, (work, home))
    };
    let (net, router) = (&s.net, &mut s.router);
    let out = s
        .trips
        .start_commutes(net, router, (&s.buildings, &[]), &[request], Tick::ZERO);
    assert!(!Household::drives(&s, out[0].unwrap()));
    s.trips.cancel(out[0].unwrap());
    let back = h.start(&mut s, &[(0, (work, home))]);
    assert!(Household::drives(&s, back[0]));
    assert_eq!(
        s.trips.get(back[0]).unwrap().parking_lot(),
        Some(LotId::Building(home))
    );
}

/// 車で走っているあいだに枠をなくした世帯の車は、着いても駐車場に停めずに消え、押さえた空きも返す。
#[test]
fn a_car_dropped_while_driving_does_not_park() {
    let (mut s, ids) = Street::new(&[0, 30]);
    let (home, work) = (ids[0], ids[1]);
    let h = Household { home };
    h.park(&mut s, 0, (home, work));
    let back = h.start(&mut s, &[(0, (work, home))]);
    assert!(Household::drives(&s, back[0]));
    assert_eq!(
        s.trips.household_car(Household::ID),
        Some(HouseholdCar::InUse(back[0]))
    );
    s.trips.forget_car(Household::ID);
    s.run(back[0]);
    s.trips.take_commuter(back[0]);
    assert_eq!(lot(&s, home), (0, 0));
    assert_eq!(lot(&s, work), (0, 0));
    assert_eq!(s.trips.household_car(Household::ID), None);
}

#[test]
fn no_car_is_counted_by_reason() {
    let (mut s, ids) = Street::new(&[0, 30]);
    let (home, work) = (ids[0], ids[1]);
    let h = Household { home };
    let tally = |s: &Street| s.trips.parking_tally();
    let sum = |s: &Street| {
        let t = tally(s);
        t.no_car_in_use + t.no_car_parked_away + t.no_car_at_home + t.no_car_other
    };
    // 家族が乗っている。
    let out = h.start(&mut s, &[(0, (home, work)), (1, (home, work))]);
    assert_eq!(tally(&s).no_car_in_use, 1);
    s.run(out[0]);
    s.trips.take_commuter(out[0]);
    s.trips.cancel(out[1]);
    // 家族が職場に停めている。
    let walk = h.start(&mut s, &[(1, (home, work))]);
    assert_eq!(tally(&s).no_car_parked_away, 1);
    s.trips.cancel(walk[0]);
    // 家のない住民が、職場に停めてある車に乗れない。
    let request = CommuteRequest {
        home: None,
        ..h.request(1, (work, home))
    };
    let (net, router) = (&s.net, &mut s.router);
    let ids = s
        .trips
        .start_commutes(net, router, (&s.buildings, &[]), &[request], Tick::ZERO);
    assert_eq!(tally(&s).no_car_other, 1);
    for id in ids.into_iter().flatten() {
        s.trips.cancel(id);
    }
    // 車を家に停めた状態で、家にいない本人が家へ向かう。
    let back = h.start(&mut s, &[(0, (work, home))]);
    s.run(back[0]);
    s.trips.take_commuter(back[0]);
    assert_eq!(lot(&s, home), (1, 0));
    let walk = h.start(&mut s, &[(1, (work, home))]);
    assert_eq!(tally(&s).no_car_at_home, 1);
    s.trips.cancel(walk[0]);
    let t = tally(&s);
    assert_eq!(t.no_car, 4);
    assert_eq!(sum(&s), t.no_car);
}

#[test]
fn a_car_left_away_from_home_returns_when_its_driver_changes_jobs() {
    let (mut s, ids) = Street::new(&[0, 30]);
    let (home, work) = (ids[0], ids[1]);
    let h = Household { home };
    h.park(&mut s, 0, (home, work));
    // ほかの住民の分では戻さない。
    s.trips.return_car(
        Household::ID,
        Household::citizen(1),
        Some(LotId::Building(home)),
    );
    assert_eq!(lot(&s, work), (1, 0));
    s.trips.return_car(
        Household::ID,
        Household::citizen(0),
        Some(LotId::Building(home)),
    );
    assert_eq!(s.trips.household_car(Household::ID), None);
    assert_eq!(lot(&s, work), (0, 0));
    // 家に停めた車は戻さない。
    h.park(&mut s, 0, (work, home));
    s.trips.return_car(
        Household::ID,
        Household::citizen(0),
        Some(LotId::Building(home)),
    );
    assert_eq!(lot(&s, home), (1, 0));
}

#[test]
fn a_leaving_household_takes_its_parked_car() {
    let (mut s, ids) = Street::new(&[0, 30]);
    let (home, work) = (ids[0], ids[1]);
    let h = Household { home };
    h.park(&mut s, 0, (home, work));
    s.trips.forget_car(Household::ID);
    assert_eq!(s.trips.household_car(Household::ID), None);
    assert_eq!(lot(&s, work), (0, 0));
}

#[test]
fn walking_instead_of_driving_leaves_the_household_without_a_car() {
    let (mut s, ids) = Street::new(&[0, 30]);
    let (home, work) = (ids[0], ids[1]);
    let h = Household { home };
    let id = h.start(&mut s, &[(0, (home, work))])[0];
    s.board(&[id]);
    let mut ended = Vec::new();
    s.trips
        .walk_instead(&s.net, &mut s.router, vec![id.slot()], &mut ended);
    assert!(ended.is_empty());
    s.run(id);
    s.trips.take_commuter(id);
    assert_eq!(s.trips.household_car(Household::ID), None);
    assert_eq!(lot(&s, work), (0, 0));
}

#[test]
fn a_drive_lost_to_a_removed_road_leaves_the_household_without_a_car() {
    let (mut s, ids) = Street::new(&[0, 30]);
    let (home, work) = (ids[0], ids[1]);
    let h = Household { home };
    let id = h.start(&mut s, &[(0, (home, work))])[0];
    s.board(&[id]);
    s.net
        .bulldoze(TileRect::from_corners(
            TilePos::new(15, 0),
            TilePos::new(15, 0),
        ))
        .unwrap();
    s.router.rebuild(&s.net);
    assert_eq!(s.trips.reroute(&s.net, &mut s.router), vec![id]);
    s.trips.take_commuter(id);
    assert_eq!(s.trips.household_car(Household::ID), None);
    // 押さえていた職場の空きも返っている。
    assert_eq!(lot(&s, work), (0, 0));
}

/// 道路の南側に、`north` の向かいの建物を建てる。出入口は同じ x で、北側の建物と反対の歩道に付く。
fn build_opposite(s: &mut Street, north: BuildingId) -> BuildingId {
    let n = *s.buildings.get(north).unwrap();
    let front = TilePos::new(n.front.x, 1);
    let segment = n.entrance.segment;
    let start = s.net.segment_line(segment).unwrap().start.x;
    s.buildings.insert(Building {
        area: TileRect::from_corners(front, front),
        front,
        toward_road: (0, -1),
        entrance: LinkId {
            segment,
            dir: Dir::Forward,
            mode: LaneMode::Sidewalk,
        },
        entrance_at: LaneOffset(u32::try_from(n.front.x - start).unwrap() * 8_000),
        ..n
    })
}

/// 車線に出るのを待つ車は、待っている点ごとに数える。道路をはさんで向かい合う建物は同じ点になり、
/// 1 件にまとまって、2 つの建物が並ぶ。
#[test]
fn cars_waiting_at_the_curb_are_grouped_by_point_with_the_facing_buildings() {
    let (mut s, ids) = Street::with_roads(&[14, 40, 42, 44, 46, 48], &[(0, 10), (10, 60)]);
    let south = build_opposite(&mut s, ids[0]);
    let north = Line::between(TilePos::new(10, 0), TilePos::new(10, 10)).unwrap();
    s.net.build_road(&north, RoadKind::Small).unwrap();
    s.router = AstarRouter::new(&s.net);
    let on_street = |x| SidewalkPos {
        tile: TilePos::new(x, 0),
        step: (1, 0),
        along: LaneOffset(0),
    };
    // 西から交差点へ向かう車がいて、出発の車は交差点から入る車の空きを取らずに待つ。
    s.trips.spawn_drives(
        &s.net,
        &mut s.router,
        &[(on_street(6), on_street(59))],
        Tick::ZERO,
    )[0]
    .unwrap();
    let cars: Vec<TripId> = ids[1..]
        .iter()
        .enumerate()
        .map(|(i, to)| s.drive([ids[0], south][i % 2], *to).unwrap())
        .collect();
    s.board(&cars);
    let waits = |s: &Street, long| s.trips.curb_waits(&s.net, &s.buildings, long);
    let sample = crate::world::CarSample::of(&s.trips);
    let now = waits(&s, 0);
    // 向かい合う 2 つの建物は同じ点で、どちらも並ぶ。
    let here = |b: BuildingId| {
        lot_spot(&s.net, s.buildings.get(b).unwrap(), s.buildings.seed())
            .unwrap()
            .world()
    };
    assert_eq!(here(ids[0]), here(south));
    assert_eq!(now.len(), 1, "{now:?}");
    assert_eq!(now[0].at, here(ids[0]));
    assert_eq!(now[0].buildings, [ids[0], south]);
    // 台数の合計は、待っている車の数に等しい。
    let total: usize = now.iter().map(|w| w.waiting as usize).sum();
    assert_eq!(total, sample.waiting_at_curb());
    assert!(total > 0);
}

/// 長く待つ車は、待ち始めてからの tick 数が閾値に届いたところから `long` に入る。
#[test]
fn long_curb_waits_start_exactly_at_the_threshold() {
    let (mut s, ids) = Street::with_roads(&[14, 40, 42, 44, 46], &[(0, 10), (10, 60)]);
    let north = Line::between(TilePos::new(10, 0), TilePos::new(10, 10)).unwrap();
    s.net.build_road(&north, RoadKind::Small).unwrap();
    s.router = AstarRouter::new(&s.net);
    // 交差点の手前を通る車を、動かさずに置いて、出発の車を待たせ続ける。
    let on_street = |x| SidewalkPos {
        tile: TilePos::new(x, 0),
        step: (1, 0),
        along: LaneOffset(0),
    };
    let approach = s.trips.spawn_drives(
        &s.net,
        &mut s.router,
        &[(on_street(6), on_street(59))],
        Tick::ZERO,
    )[0]
    .unwrap();
    let cars: Vec<TripId> = ids[1..]
        .iter()
        .map(|to| s.drive(ids[0], *to).unwrap())
        .collect();
    s.board(&cars);
    let waiting_car = |s: &Street| {
        cars.iter()
            .copied()
            .find(|id| s.trips.get(*id).is_some_and(|t| t.waiting_to_depart()))
    };
    let car = waiting_car(&s).expect("待つ車がいない");
    let _ = approach;
    // 待ち始めてからの tick 数 `t` のとき、閾値 `t` なら長く待つ車に入り、`t + 1` なら入らない。
    let t = s.trips.get(car).unwrap().curb_wait_ticks();
    let waits = |s: &Street, long| s.trips.curb_waits(&s.net, &s.buildings, long);
    assert_eq!(waits(&s, t)[0].long, 1);
    assert_eq!(waits(&s, t + 1)[0].long, 0);
    // 待ち続けて数が増えれば、閾値を同じだけ上げても入る。
    for _ in 0..3 {
        s.step();
        let Some(car) = waiting_car(&s) else { return };
        let t = s.trips.get(car).unwrap().curb_wait_ticks();
        assert!(t > 0, "待つ tick 数が増えない");
        assert!(waits(&s, t)[0].long >= 1);
    }
}

/// 東西の道路の北側に、正面のマスの x が `front_x` の、2×2 マスの平面の共同の駐車場を置いた形。
fn shared_lot(s: &Street, front_x: i64) -> Facility {
    let front = TilePos::new(front_x, -1);
    let segment = s.net.segments_through(TilePos::new(front_x, 0))[0];
    Facility {
        kind: crate::command::ServiceKind::ParkingLot,
        area: TileRect::from_corners(front, TilePos::new(front_x + 1, -2)),
        front,
        toward_road: (0, 1),
        entrance: Some(LinkId {
            segment,
            dir: Dir::Backward,
            mode: LaneMode::Sidewalk,
        }),
    }
}

/// 共同の駐車場の枠を借りた世帯は、家と借りた枠のあいだを歩き、家に帰る車は借りた枠に停める。
/// 次に家から出るときは、借りた枠に停めた車に乗る。
#[test]
fn a_renter_walks_between_home_and_the_rented_space() {
    let (mut s, ids) = Street::new(&[50, 5]);
    let (home, work) = (ids[0], ids[1]);
    let facilities = [shared_lot(&s, 40)];
    let c = facilities[0].area.min;
    let spot = facility_spot(&s.net, &facilities[0]).expect("出入りの位置がない");
    s.trips
        .set_rented(&std::collections::BTreeMap::from([(c, 1)]));
    let h = Household { home };
    let start = |s: &mut Street, ends| {
        let r = CommuteRequest {
            place: Some(CarPlace::Shared(c)),
            ..h.request(0, ends)
        };
        let (net, router) = (&s.net, &mut s.router);
        let sites = (&s.buildings, facilities.as_slice());
        s.trips.start_commutes(net, router, sites, &[r], Tick::ZERO)[0].expect("移動を作れない")
    };
    // 車がまだどこにもなくても、家から出るときは借りた枠まで歩いて乗る。
    let out = start(&mut s, (home, work));
    assert!(Household::drives(&s, out));
    assert_eq!(s.trips.get(out).unwrap().dest, spot);
    s.run(out);
    s.trips.take_commuter(out);
    assert_eq!(lot(&s, work), (1, 0));
    // 家に帰る車は借りた枠に停め、そこから家まで歩く。借りた枠は押さえなくても空いている。
    let back = start(&mut s, (work, home));
    assert_eq!(
        s.trips.get(back).unwrap().parking_lot(),
        Some(LotId::Rented(c))
    );
    assert_eq!(s.trips.shared_lot(c).visitors, Lot::default());
    let modes = s.run(back);
    s.trips.take_commuter(back);
    assert_eq!(
        modes,
        [LaneMode::Sidewalk, LaneMode::Car, LaneMode::Sidewalk]
    );
    assert_eq!(
        s.trips.household_car(Household::ID),
        Some(HouseholdCar::Parked {
            lot: LotId::Rented(c),
            by: Household::citizen(0)
        })
    );
    assert_eq!(s.trips.shared_lot(c).rented_parked, 1);
    assert_eq!(s.trips.parking_tally().parked_elsewhere, 0);
    // 次に家から出るときは、借りた枠の車に乗る。
    let again = start(&mut s, (home, work));
    assert!(Household::drives(&s, again));
    assert_eq!(s.trips.get(again).unwrap().dest, spot);
    assert_eq!(s.trips.shared_lot(c).rented_parked, 0);
}

/// 行き先の駐車場が満車なら、近くの共同の駐車場の外来の空きに停める。貸した枠は、車がいなくても外来には使わせない。
#[test]
fn visitors_use_free_shared_spaces_but_not_rented_ones() {
    let (mut s, ids) = Street::new(&[50, 5, 48]);
    let (from, shop, other) = (ids[0], ids[1], ids[2]);
    let facilities = [shared_lot(&s, 8)];
    let c = facilities[0].area.min;
    let drive = |s: &mut Street, from| {
        let (net, router) = (&s.net, &mut s.router);
        let sites = (&s.buildings, facilities.as_slice());
        s.trips
            .spawn_drive(net, router, sites, (from, shop), Tick::ZERO)
    };
    let first = drive(&mut s, from).unwrap();
    assert_eq!(lot(&s, shop), (0, 1));
    let second = drive(&mut s, other).unwrap();
    assert_eq!(
        s.trips.get(second).unwrap().parking_lot(),
        Some(LotId::Shared(c))
    );
    assert_eq!(
        s.trips.shared_lot(c).visitors,
        Lot {
            parked: 0,
            reserved: 1
        }
    );
    s.run(second);
    assert_eq!(
        s.trips.shared_lot(c).visitors,
        Lot {
            parked: 1,
            reserved: 0
        }
    );
    // 残りの 11 台を貸すと、外来の停めた 1 台で埋まり、次の車は停める所がない。
    s.trips
        .set_rented(&std::collections::BTreeMap::from([(c, 11)]));
    let full = s.trips.parking_tally().lots_full;
    assert!(drive(&mut s, other).is_none());
    assert_eq!(s.trips.parking_tally().lots_full, full + 1);
    s.trips.cancel(first);
}

/// 取り壊した共同の駐車場に向かっていた車は、着いても停めずに消え、同じ角に置き直した駐車場の台数を崩さない。
#[test]
fn a_car_heading_to_a_removed_shared_lot_does_not_park() {
    let (mut s, ids) = Street::new(&[50, 5, 48]);
    let (from, shop, other) = (ids[0], ids[1], ids[2]);
    let facilities = [shared_lot(&s, 8)];
    let c = facilities[0].area.min;
    let sites = (&s.buildings, facilities.as_slice());
    let (net, router) = (&s.net, &mut s.router);
    s.trips
        .spawn_drive(net, router, sites, (from, shop), Tick::ZERO)
        .unwrap();
    let to_shared = s
        .trips
        .spawn_drive(net, router, sites, (other, shop), Tick::ZERO)
        .unwrap();
    assert_eq!(
        s.trips.get(to_shared).unwrap().parking_lot(),
        Some(LotId::Shared(c))
    );
    s.trips.forget_shared_lot(c);
    // 置き直した駐車場の枠を、1 つ貸してある。
    s.trips
        .set_rented(&std::collections::BTreeMap::from([(c, 1)]));
    s.run(to_shared);
    let lot = s.trips.shared_lot(c);
    assert_eq!((lot.rented, lot.visitors), (1, Lot::default()));
}

/// 家に帰って借りた枠に停める車は、`parked_elsewhere` の積み上げにも、車ごとの `parks_elsewhere` にも数えない。
/// 職場が満車で借りた枠に回る車と、外来の枠に回る車は、どちらにも数える。外来の枠は、その駐車場が取り壊されれば
/// `parks_elsewhere` は数えなくなる。
#[test]
fn parking_elsewhere_is_counted_alike_by_the_tally_and_by_each_trip() {
    let (mut s, ids) = Street::new(&[50, 5, 30, 28]);
    let (home, work, filler, visitor) = (ids[0], ids[1], ids[2], ids[3]);
    let facilities = [shared_lot(&s, 8)];
    let c = facilities[0].area.min;
    s.trips
        .set_rented(&std::collections::BTreeMap::from([(c, 1)]));
    // 職場の駐車場を埋める。
    let (net, router) = (&s.net, &mut s.router);
    let sites = (&s.buildings, facilities.as_slice());
    s.trips
        .spawn_drive(net, router, sites, (filler, work), Tick::ZERO)
        .unwrap();
    let h = Household { home };
    let start = |s: &mut Street, ends| {
        let request = CommuteRequest {
            place: Some(CarPlace::Shared(c)),
            ..h.request(0, ends)
        };
        let (net, router) = (&s.net, &mut s.router);
        let sites = (&s.buildings, facilities.as_slice());
        s.trips
            .start_commutes(net, router, sites, &[request], Tick::ZERO)[0]
            .expect("移動を作れない")
    };
    let elsewhere = |s: &Street, id, home| {
        s.trips
            .get(id)
            .unwrap()
            .parks_elsewhere(&s.net, &s.buildings, &facilities, home)
    };
    // 職場が満車で借りた枠に回る車は数える。
    let before = s.trips.parking_tally().parked_elsewhere;
    let out = start(&mut s, (home, work));
    assert_eq!(
        s.trips.get(out).unwrap().parking_lot(),
        Some(LotId::Rented(c))
    );
    assert!(elsewhere(&s, out, Some(home)));
    assert_eq!(s.trips.parking_tally().parked_elsewhere, before + 1);
    s.trips.cancel(out);
    // 家に帰って借りた枠に停める車は数えない。
    let before = s.trips.parking_tally().parked_elsewhere;
    let back = start(&mut s, (work, home));
    assert_eq!(
        s.trips.get(back).unwrap().parking_lot(),
        Some(LotId::Rented(c))
    );
    assert!(!elsewhere(&s, back, Some(home)));
    assert_eq!(s.trips.parking_tally().parked_elsewhere, before);
    s.trips.cancel(back);
    // 借りていない車は外来の枠に回り、積み上げにも数える。
    let (net, router) = (&s.net, &mut s.router);
    let sites = (&s.buildings, facilities.as_slice());
    let v = s
        .trips
        .spawn_drive(net, router, sites, (visitor, work), Tick::ZERO)
        .unwrap();
    assert_eq!(
        s.trips.get(v).unwrap().parking_lot(),
        Some(LotId::Shared(c))
    );
    assert!(elsewhere(&s, v, None));
    assert_eq!(s.trips.parking_tally().parked_elsewhere, before + 1);
    // 駐車場が取り壊されたら数えない。
    let t = s.trips.get(v).unwrap();
    assert!(!t.parks_elsewhere(&s.net, &s.buildings, &[], None));
}

/// 共同の駐車場 A に向かう車が、A を取り壊して同じ角に置き直した B に、あとから向かった別の車の枠を横取りしない。
/// 先に着いた車は停めずに消え、B の押さえた空きはあとの車のものとして残る。
#[test]
fn a_car_heading_to_a_replaced_shared_lot_leaves_the_new_reservation() {
    let (mut s, ids) = Street::new(&[50, 5, 48, 46]);
    // 先に向かう車の出発点を、あとの車より共同の駐車場の近くにする。
    let (from, shop, other, later) = (ids[0], ids[1], ids[3], ids[2]);
    let facilities = [shared_lot(&s, 8)];
    let c = facilities[0].area.min;
    let sites = (&s.buildings, facilities.as_slice());
    let (net, router) = (&s.net, &mut s.router);
    // 店の駐車場を埋めて、あとの車を共同の駐車場に回す。
    s.trips
        .spawn_drive(net, router, sites, (from, shop), Tick::ZERO)
        .unwrap();
    let v1 = s
        .trips
        .spawn_drive(net, router, sites, (other, shop), Tick::ZERO)
        .unwrap();
    assert_eq!(
        s.trips.get(v1).unwrap().parking_lot(),
        Some(LotId::Shared(c))
    );
    // A を取り壊して、同じ角に B を置く。
    s.trips.forget_shared_lot(c);
    let (net, router) = (&s.net, &mut s.router);
    let sites = (&s.buildings, facilities.as_slice());
    let v2 = s
        .trips
        .spawn_drive(net, router, sites, (later, shop), Tick::ZERO)
        .unwrap();
    assert_eq!(
        s.trips.get(v2).unwrap().parking_lot(),
        Some(LotId::Shared(c))
    );
    let reserved = Lot {
        parked: 0,
        reserved: 1,
    };
    assert_eq!(s.trips.shared_lot(c).visitors, reserved);
    // V1 が先に着いても、B には停まらず、V2 の押さえた空きもそのまま残る。
    s.run_until_parked(v1);
    assert_eq!(s.trips.get(v2).unwrap().mode(), LaneMode::Car);
    assert_eq!(s.trips.shared_lot(c).visitors, reserved);
    s.run(v2);
    let parked = Lot {
        parked: 1,
        reserved: 0,
    };
    assert_eq!(s.trips.shared_lot(c).visitors, parked);
}

/// 世帯の車で共同の駐車場の外来の枠へ向かう途中に駐車場を取り壊して置き直しても、
/// 世帯の車は B に停まらず、あとから向かった別の車の押さえた空きを横取りしない。
#[test]
fn a_household_car_heading_to_a_replaced_shared_lot_leaves_the_new_reservation() {
    let (mut s, ids) = Street::new(&[50, 5, 48, 46]);
    // 世帯の車の出発点を、あとの車より共同の駐車場の近くにする。
    let (home, shop, other, later) = (ids[0], ids[1], ids[3], ids[2]);
    let facilities = [shared_lot(&s, 8)];
    let c = facilities[0].area.min;
    let (net, router) = (&s.net, &mut s.router);
    let sites = (&s.buildings, facilities.as_slice());
    s.trips
        .spawn_drive(net, router, sites, (home, shop), Tick::ZERO)
        .unwrap();
    let h = Household { home };
    let request = h.request(0, (other, shop));
    let v1 = s
        .trips
        .start_commutes(net, router, sites, &[request], Tick::ZERO)[0]
        .expect("移動を作れない");
    assert_eq!(
        s.trips.get(v1).unwrap().parking_lot(),
        Some(LotId::Shared(c))
    );
    assert_eq!(
        s.trips.household_car(Household::ID),
        Some(HouseholdCar::InUse(v1))
    );
    s.trips.forget_shared_lot(c);
    let (net, router) = (&s.net, &mut s.router);
    let v2 = s
        .trips
        .spawn_drive(net, router, sites, (later, shop), Tick::ZERO)
        .unwrap();
    assert_eq!(
        s.trips.get(v2).unwrap().parking_lot(),
        Some(LotId::Shared(c))
    );
    let reserved = Lot {
        parked: 0,
        reserved: 1,
    };
    s.run_until_parked(v1);
    assert_eq!(s.trips.get(v2).unwrap().mode(), LaneMode::Car);
    assert_eq!(s.trips.shared_lot(c).visitors, reserved);
    s.run(v1);
    s.trips.take_commuter(v1);
    assert_eq!(s.trips.household_car(Household::ID), None);
    s.run(v2);
    assert_eq!(s.trips.shared_lot(c).visitors.parked, 1);
}

/// 借りた枠に停めた車は、停めた住民が職場から家に帰るときに取りに行かない。枠が職場の近くにあると、
/// 枠まで歩いて 0m だけ乗り、家まで歩くことになる。家にいる世帯の誰かが出るときは、いつもどおり乗れる。
#[test]
fn a_car_in_the_rented_place_is_not_fetched_to_drive_home() {
    let (mut s, ids) = Street::new(&[50, 28]);
    let (home, work) = (ids[0], ids[1]);
    let facilities = [shared_lot(&s, 40)];
    let c = facilities[0].area.min;
    s.trips
        .set_rented(&std::collections::BTreeMap::from([(c, 1)]));
    s.trips
        .place_car(Household::ID, Household::citizen(0), LotId::Rented(c));
    let h = Household { home };
    let start = |s: &mut Street, index, ends, to| {
        let r = h.request(index, ends);
        let r = CommuteRequest {
            who: Commuter { to, ..r.who },
            place: Some(CarPlace::Shared(c)),
            ..r
        };
        let (net, router) = (&s.net, &mut s.router);
        let sites = (&s.buildings, facilities.as_slice());
        s.trips.start_commutes(net, router, sites, &[r], Tick::ZERO)[0].expect("移動を作れない")
    };
    let home_bound = start(&mut s, 0, (work, home), crate::citizens::Activity::Home);
    assert!(!Household::drives(&s, home_bound));
    assert_eq!(s.trips.parking_tally().no_car_at_home, 1);
    assert_eq!(s.trips.parking_tally().drives, 0);
    s.trips.cancel(home_bound);
    let from_home = start(&mut s, 1, (home, work), crate::citizens::Activity::Work);
    assert!(Household::drives(&s, from_home));
}

/// 家からも職場からも離れた所に停めて置き去りになった車は、外す。近くにあれば残す。
#[test]
fn a_stranded_car_is_returned_only_when_out_of_reach() {
    let (mut s, ids) = Street::new(&[0, 20, 58]);
    let (home, work, shop) = (ids[0], ids[1], ids[2]);
    let h = Household { home };
    h.park(&mut s, 0, (home, shop));
    let own = Some(LotId::Building(home));
    let sites = (&s.buildings, &[][..]);
    let c = Household::citizen(0);
    // 停めた本人でなければ外さない。
    assert!(!s.trips.return_stranded_car(
        Household::ID,
        Household::citizen(1),
        own,
        sites,
        [Some(home), Some(work)]
    ));
    // 店のそばに職場があれば外さない。
    assert!(
        !s.trips
            .return_stranded_car(Household::ID, c, own, sites, [Some(home), Some(shop)])
    );
    assert_eq!(lot(&s, shop), (1, 0));
    assert!(
        s.trips
            .return_stranded_car(Household::ID, c, own, sites, [Some(home), Some(work)])
    );
    assert_eq!(s.trips.household_car(Household::ID), None);
    assert_eq!(lot(&s, shop), (0, 0));
}

/// 2km の移動で、鉄道の費用が車より大きい所では、車を持つ世帯は車に乗り、車を持たない世帯も仮の扱いの車に乗る。
/// 鉄道の費用が小さい所では、どちらも駅へ歩いて鉄道に乗る（設計 3.10 の 4）。
#[test]
fn at_2km_the_cheaper_of_car_and_rail_wins() {
    use crate::rail::choice::{LineLive, RailPlanner, RideChoice, line_stops_of};
    use crate::rail::{LineId, StationId};
    use std::collections::{BTreeMap, BTreeSet};

    // 家（x = 0）と職場（x = 250、2km 先）と、駅の出入口に使う建物（x = 10 と x = 240）。
    let (mut s, ids) = Street::with_roads(&[0, 250, 10, 240], &[(0, 260)]);
    let (home, work) = (ids[0], ids[1]);
    let door = |b: BuildingId| building_spot(&s.net, s.buildings.get(b).unwrap()).unwrap();
    let station = |index| StationId {
        index,
        generation: 0,
    };
    let line = LineId {
        index: 0,
        generation: 0,
    };
    let doors = vec![
        (station(0), door(ids[2]).tile, door(ids[2])),
        (station(1), door(ids[3]).tile, door(ids[3])),
    ];
    let planner = RailPlanner::from_parts(
        doors,
        &[(line, line_stops_of(&[station(0), station(1)], false))],
    );
    for (seg, rail_wins) in [(100, true), (3_000, false)] {
        for owns_car in [true, false] {
            let live = BTreeMap::from([(line, LineLive::new(2, &[seg, seg]))]);
            let choice = RideChoice::from_parts(&planner, live, BTreeSet::new(), 5);
            let offer = choice
                .offer(
                    &WalkEnd::at(s.buildings.get(home).unwrap().front),
                    &WalkEnd::at(s.buildings.get(work).unwrap().front),
                )
                .expect("鉄道の候補がない");
            assert_eq!(offer.board, station(0));
            assert_eq!(offer.alight, station(1));
            let request = CommuteRequest {
                who: Commuter {
                    citizen: CitizenId {
                        index: 0,
                        generation: 0,
                    },
                    household: HouseholdId {
                        index: u32::from(owns_car),
                        generation: 0,
                    },
                    to: crate::citizens::Activity::Work,
                },
                from: home,
                to: work,
                home: Some(home),
                place: Some(CarPlace::Home),
                owns_car,
                savings: Money::ZERO,
                parking_fee: 0,
            };
            let (net, router) = (&s.net, &mut s.router);
            let id = s.trips.start_commutes_with_rides(
                net,
                router,
                (&s.buildings, &[]),
                &[request],
                Tick::ZERO,
                Some(&choice),
            )[0]
            .expect("移動を作れない");
            let t = s.trips.get(id).unwrap();
            let rides = t.ride_plan().is_some();
            let drives = matches!(t.then, parking::Then::Drive { .. });
            assert_eq!(
                (rides, drives),
                (rail_wins, !rail_wins),
                "区間 {seg} tick、車を持つ {owns_car}、鉄道の費用 {}",
                offer.cost
            );
            if rides {
                assert_eq!(t.via(), Via::Walk);
            }
            s.trips.cancel(id);
        }
    }
}

/// 鉄道の候補を確かめる駅。`Street` の建物 `doors` の出入口を、駅 0、1、… の出入口にする。
/// 駅を順に結ぶ往復の路線（ID 0）を 1 本置く。
fn rail_planner(
    s: &Street,
    doors: &[BuildingId],
) -> (crate::rail::choice::RailPlanner, crate::rail::LineId, usize) {
    use crate::rail::choice::{RailPlanner, line_stops_of};
    use crate::rail::{LineId, StationId};
    let station = |index| StationId {
        index,
        generation: 0,
    };
    let line = LineId {
        index: 0,
        generation: 0,
    };
    let list: Vec<_> = doors
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let door = building_spot(&s.net, s.buildings.get(*b).unwrap()).unwrap();
            (station(u32::try_from(i).unwrap()), door.tile, door)
        })
        .collect();
    let stations: Vec<StationId> = list.iter().map(|(s, _, _)| *s).collect();
    let stops = line_stops_of(&stations, false);
    let n = stops.len();
    (RailPlanner::from_parts(list, &[(line, stops)]), line, n)
}

/// 列車 1 本が、止まる所ごとの区間の時間 `segs` で走る路線の、運賃 5 の鉄道の候補。
fn rail_choice<'a>(
    planner: &'a crate::rail::choice::RailPlanner,
    line: crate::rail::LineId,
    segs: &[u32],
) -> crate::rail::choice::RideChoice<'a> {
    use crate::rail::choice::{LineLive, RideChoice};
    use std::collections::{BTreeMap, BTreeSet};
    let live = BTreeMap::from([(line, LineLive::new(1, segs))]);
    RideChoice::from_parts(planner, live, BTreeSet::new(), 5)
}

/// 鉄道の候補 `choice` があるときに、`request` の移動を作る。
fn start_with_rides(
    s: &mut Street,
    request: CommuteRequest,
    choice: &crate::rail::choice::RideChoice<'_>,
) -> Option<TripId> {
    let (net, router) = (&s.net, &mut s.router);
    s.trips.start_commutes_with_rides(
        net,
        router,
        (&s.buildings, &[]),
        &[request],
        Tick::ZERO,
        Some(choice),
    )[0]
}

/// 移動 `id` の手段。駅へ歩くなら "rail"、車なら "car"、行き先へ歩くなら "walk"。
fn mode_of(s: &Street, id: TripId) -> &'static str {
    let t = s.trips.get(id).unwrap();
    if t.ride_plan().is_some() {
        "rail"
    } else if matches!(t.then, parking::Then::Drive { .. }) {
        "car"
    } else {
        "walk"
    }
}

/// 家の枠でない駐車場（職場）に停めた車で出る移動は、鉄道のほうが速くても鉄道と比べず、車で出る。職場から店へ
/// 向かう移動も、職場から家へ帰る移動と同じに扱う。鉄道に乗ると、車が職場に残るためである。
/// 家の枠に停めた車なら、鉄道に乗っても車は家に残るので、鉄道と比べる。
#[test]
fn a_car_parked_away_from_home_is_not_left_there_for_the_train() {
    // 家（x = 0）、職場（x = 250、2km 先）、駅の出入口に使う建物（x = 10 と x = 240）、家の近くの店（x = 5）。
    let (mut s, ids) = Street::with_roads(&[0, 250, 10, 240, 5], &[(0, 260)]);
    let (home, work, shop) = (ids[0], ids[1], ids[4]);
    let (planner, line, _) = rail_planner(&s, &[ids[2], ids[3]]);
    let choice = rail_choice(&planner, line, &[100, 100]);
    let h = Household { home };
    h.park(&mut s, 0, (home, work));
    for (to, activity) in [
        (shop, crate::citizens::Activity::Shop),
        (home, crate::citizens::Activity::Home),
    ] {
        let mut r = h.request(0, (work, to));
        r.who.to = activity;
        let id = start_with_rides(&mut s, r, &choice).expect("移動を作れない");
        assert_eq!(mode_of(&s, id), "car", "{activity:?} へ鉄道で向かった");
        // 車に乗ったまま取りやめたので、車はどこにもない。職場に停め直してから次を試す。
        s.trips.cancel(id);
        h.park(&mut s, 0, (home, work));
    }
    // 家の枠に停めた車なら、家から鉄道に乗ってよい。
    let (mut s, ids) = Street::with_roads(&[0, 250, 10, 240], &[(0, 260)]);
    let (home, work) = (ids[0], ids[1]);
    let (planner, line, _) = rail_planner(&s, &[ids[2], ids[3]]);
    let choice = rail_choice(&planner, line, &[100, 100]);
    let h = Household { home };
    s.trips
        .place_car(Household::ID, Household::citizen(0), LotId::Building(home));
    let id = start_with_rides(&mut s, h.request(0, (home, work)), &choice).unwrap();
    assert_eq!(mode_of(&s, id), "rail");
}

/// 鉄道で通う人の車は家の枠に残るので、帰りは車を使えないが、鉄道に乗るなら車がない移動（`no_car`）に数えない。
/// 鉄道が遅く、歩いて帰るなら、これまでどおり数える。
#[test]
fn riding_home_is_not_counted_as_missing_the_car() {
    let (mut s, ids) = Street::with_roads(&[0, 250, 10, 240], &[(0, 260)]);
    let (home, work) = (ids[0], ids[1]);
    let (planner, line, _) = rail_planner(&s, &[ids[2], ids[3]]);
    let h = Household { home };
    s.trips
        .place_car(Household::ID, Household::citizen(0), LotId::Building(home));
    let mut back = h.request(0, (work, home));
    back.who.to = crate::citizens::Activity::Home;
    let fast = rail_choice(&planner, line, &[100, 100]);
    let id = start_with_rides(&mut s, back, &fast).unwrap();
    assert_eq!(mode_of(&s, id), "rail");
    assert_eq!(s.trips.parking_tally().no_car, 0);
    assert_eq!(s.trips.parking_tally().no_car_at_home, 0);
    s.trips.cancel(id);
    let slow = rail_choice(&planner, line, &[30_000, 30_000]);
    let id = start_with_rides(&mut s, back, &slow).unwrap();
    assert_eq!(mode_of(&s, id), "walk");
    assert_eq!(s.trips.parking_tally().no_car, 1);
    assert_eq!(s.trips.parking_tally().no_car_at_home, 1);
}

/// 150m より近い移動は、車を持つかによらず鉄道と比べずに歩く。150m 以上なら、車を持たない世帯の 500m より
/// 近い移動も、鉄道が速ければ鉄道に乗る。
#[test]
fn trips_under_150m_walk_without_the_train() {
    // 家（x = 0）、120m 先（x = 15）、200m 先（x = 25）と、それぞれの隣の駅の出入口（x = 1、14、24）。
    let (mut s, ids) = Street::with_roads(&[0, 15, 25, 1, 14, 24], &[(0, 60)]);
    let (home, near, far) = (ids[0], ids[1], ids[2]);
    let (planner, line, n) = rail_planner(&s, &[ids[3], ids[4], ids[5]]);
    let choice = rail_choice(&planner, line, &vec![1; n]);
    let h = Household { home };
    for (to, owns_car, want) in [
        (near, true, "walk"),
        (near, false, "walk"),
        (far, false, "rail"),
    ] {
        let mut r = h.request(0, (home, to));
        r.owns_car = owns_car;
        let id = start_with_rides(&mut s, r, &choice).unwrap();
        assert_eq!(mode_of(&s, id), want, "{to:?} 車を持つ {owns_car}");
        s.trips.cancel(id);
    }
}

/// 鉄道は、今の決まりで選んだ手段（歩きか車）の見積もりより費用が小さいときだけ選ぶ。費用が同じなら今の手段を残す。
/// 歩きを選んだ移動は歩きの見積もりと、車を選んだ移動は車の見積もりと比べる。
#[test]
fn the_train_wins_only_when_strictly_cheaper() {
    use crate::rail::choice::{car_seconds, tile_distance_mm, walk_seconds};
    // 家（x = 0）、300m 先（x = 38）、2km 先（x = 250）と、駅の出入口（x = 1、37、249）。
    let (mut s, ids) = Street::with_roads(&[0, 38, 250, 1, 37, 249], &[(0, 260)]);
    let home = ids[0];
    let h = Household { home };
    for (to, base, want) in [
        (ids[1], walk_seconds as fn(u64) -> u64, "walk"),
        (ids[2], car_seconds as fn(u64) -> u64, "car"),
    ] {
        let doors = if to == ids[1] {
            [ids[3], ids[4]]
        } else {
            [ids[3], ids[5]]
        };
        let (planner, line, _) = rail_planner(&s, &doors);
        let front = |b: BuildingId| s.buildings.get(b).unwrap().front;
        let (from_tile, to_tile) = (front(home), front(to));
        // 車を持たない世帯で、300m は歩き、2km は仮の扱いの車になる。
        let mut r = h.request(0, (home, to));
        r.owns_car = false;
        // 車の見積もりには、停める手間を足して比べる。
        let extra = if want == "car" {
            s.trips.parking_seconds(&r, (&s.buildings, &[]))
        } else {
            0
        };
        let target = base(tile_distance_mm(from_tile, to_tile)) + extra;
        // 費用がちょうど `target` と `target - 1` になる区間の時間を探す。運賃は依頼の世帯の重さで足す。
        let segs_for = |cost: u64| {
            (0..20_000_u32)
                .flat_map(|a| (0..4_u32).map(move |b| [a, b]))
                .find(|segs| {
                    rail_choice(&planner, line, segs)
                        .offer_for(&WalkEnd::at(from_tile), &WalkEnd::at(to_tile), r.savings)
                        .is_some_and(|o| o.cost == cost)
                })
                .expect("費用の合う区間の時間がない")
        };
        for (cost, expect) in [(target, want), (target - 1, "rail")] {
            let segs = segs_for(cost);
            let choice = rail_choice(&planner, line, &segs);
            let id = start_with_rides(&mut s, r, &choice).unwrap();
            assert_eq!(
                mode_of(&s, id),
                expect,
                "費用 {cost}、今の手段の見積もり {target}"
            );
            s.trips.cancel(id);
        }
    }
}

/// 車で行く時間の見込みが長い時間帯に出る移動は、測る前なら車を選ぶ鉄道の費用でも、鉄道を選ぶ。
/// 見込みを測っていない時間帯の移動は、今までどおり車を選ぶ。
#[test]
fn a_slow_hour_for_cars_sends_drivers_to_the_train() {
    use crate::clock::TICKS_PER_DAY;
    use crate::rail::choice::{car_seconds, tile_distance_mm};
    // 家（x = 0）、2km 先の職場（x = 250）と、駅の出入口（x = 1、249）。
    let (mut s, ids) = Street::with_roads(&[0, 250, 1, 249], &[(0, 260)]);
    let (home, work) = (ids[0], ids[1]);
    let r = Household { home }.request(0, (home, work));
    let (planner, line, _) = rail_planner(&s, &[ids[2], ids[3]]);
    let front = |b: BuildingId| s.buildings.get(b).unwrap().front;
    let (from_tile, to_tile) = (front(home), front(work));
    let mm = tile_distance_mm(from_tile, to_tile);
    let car = car_seconds(mm) + s.trips.parking_seconds(&r, (&s.buildings, &[]));
    // 鉄道の費用を、測る前の車の見積もりより 100 秒大きくする。
    let segs = (0..20_000_u32)
        .flat_map(|a| (0..4_u32).map(move |b| [a, b]))
        .find(|segs| {
            rail_choice(&planner, line, segs)
                .offer_for(&WalkEnd::at(from_tile), &WalkEnd::at(to_tile), r.savings)
                .is_some_and(|o| o.cost == car + 100)
        })
        .expect("費用の合う区間の時間がない");
    let choice = rail_choice(&planner, line, &segs);
    let hour = TICKS_PER_DAY / 24;
    let start_at = |s: &mut Street, at: Tick| {
        let (net, router) = (&s.net, &mut s.router);
        let id = s.trips.start_commutes_with_rides(
            net,
            router,
            (&s.buildings, &[]),
            &[r],
            at,
            Some(&choice),
        )[0]
        .unwrap();
        let mode = mode_of(s, id);
        s.trips.cancel(id);
        mode
    };
    assert_eq!(start_at(&mut s, Tick(7 * hour)), "car");
    // 0 日目の 7 時台に、測る前の見積もりの 3 倍かかった車の移動を測る。見込みを変えるのは次の日からである。
    let pace = s.trips.car_pace_mut();
    pace.learn(Tick(7 * hour), Tick(7 * hour + car_seconds(mm) * 3), mm);
    assert_eq!(start_at(&mut s, Tick(7 * hour + 10)), "car");
    assert_eq!(start_at(&mut s, Tick(TICKS_PER_DAY + 7 * hour)), "rail");
    assert_eq!(start_at(&mut s, Tick(TICKS_PER_DAY + 12 * hour)), "car");
}

/// 西の道路（x 0〜100）と東の道路（x 102〜260）を、北へ `detour` マス回る道でつないだ街。建物は `xs` に建てる。
fn detour_street(xs: &[i64], detour: i64) -> (Street, Vec<BuildingId>) {
    let (mut s, ids) = Street::with_roads(xs, &[(0, 100), (102, 260)]);
    for (a, b) in [
        ((100, 0), (100, -detour)),
        ((100, -detour), (102, -detour)),
        ((102, -detour), (102, 0)),
    ] {
        let line = Line::between(TilePos::new(a.0, a.1), TilePos::new(b.0, b.1)).unwrap();
        s.net.build_road(&line, RoadKind::Small).unwrap();
    }
    s.router = AstarRouter::new(&s.net);
    (s, ids)
}

/// 駅まで歩く時間は、直線ではなく歩道の経路で見積もる。駅が直線では近くても、道路が遠回りなら見積もりも長くなり、
/// 鉄道を選んだ人が実際に駅まで歩いた時間と合う。
#[test]
fn the_walk_to_the_station_follows_the_sidewalks() {
    // 家（x = 104）は東の道路に、乗る駅の出入口（x = 98）は西の道路にあり、北へ 20 マス回ってつながる。
    let (mut s, ids) = detour_street(&[104, 250, 98, 240], 20);
    let (home, work) = (ids[0], ids[1]);
    let (planner, line, _) = rail_planner(&s, &[ids[2], ids[3]]);
    let planner = planner.with_walks(&s.net, s.router.graph(LaneMode::Sidewalk));
    let choice = rail_choice(&planner, line, &[1, 1]);
    let end = |b: BuildingId| {
        let bb = s.buildings.get(b).unwrap();
        WalkEnd::new(&s.net, bb.front, building_spot(&s.net, bb).unwrap())
    };
    let offer = choice
        .offer(&end(home), &end(work))
        .expect("鉄道の候補がない");
    // 東へ 2 マス、北へ 20 マス、西へ 2 マス、南へ 20 マス、西へ 2 マスほど歩く。直線の 6 マスよりずっと長い。
    let straight = crate::rail::choice::walk_seconds(6 * 8_000);
    assert!(offer.walk_to_board > 5 * straight, "{offer:?}");
    // 鉄道を選ばせ、駅に着くまでの tick を見積もりと比べる。
    let h = Household { home };
    let mut r = h.request(0, (home, work));
    r.owns_car = false;
    r.place = None;
    let id = start_with_rides(&mut s, r, &choice).expect("移動を作れない");
    assert_eq!(mode_of(&s, id), "rail");
    let mut ticks = 0_u64;
    while s.trips.take_boarding().is_empty() {
        s.step();
        ticks += 1;
        assert!(ticks < 2_000, "駅に着かない");
    }
    let gap = ticks.abs_diff(offer.walk_to_board);
    assert!(
        gap * 10 <= offer.walk_to_board,
        "見積もり {}、歩いた {ticks}",
        offer.walk_to_board
    );
}

/// 歩道で駅に着けないか、着くまでの道のりが歩く表の上限より長い駅は、直線では徒歩圏でも候補にしない。
#[test]
fn a_station_beyond_the_walk_limit_is_not_offered() {
    use crate::rail::choice::STATION_WALK_LIMIT_MM;
    let limit_tiles = i64::try_from(STATION_WALK_LIMIT_MM / 8_000).unwrap();
    for (detour, offered) in [
        (Some(20), true),
        (Some(limit_tiles / 2 + 1), false),
        (None, false),
    ] {
        let (s, ids) = match detour {
            Some(d) => detour_street(&[104, 250, 98, 240], d),
            None => Street::with_roads(&[104, 250, 98, 240], &[(0, 100), (102, 260)]),
        };
        let (planner, line, _) = rail_planner(&s, &[ids[2], ids[3]]);
        let planner = planner.with_walks(&s.net, s.router.graph(LaneMode::Sidewalk));
        let choice = rail_choice(&planner, line, &[1, 1]);
        let end = |b: BuildingId| {
            let bb = s.buildings.get(b).unwrap();
            WalkEnd::new(&s.net, bb.front, building_spot(&s.net, bb).unwrap())
        };
        assert_eq!(
            choice.offer(&end(ids[0]), &end(ids[1])).is_some(),
            offered,
            "回り道 {detour:?}"
        );
    }
}

/// 鉄道を選んでも駅まで歩けなければ（駅の出入口が別の道路網にある）、行き先へ歩く。
#[test]
fn a_rider_who_cannot_reach_the_station_walks_instead() {
    // 家（x = 104）と職場（x = 250）は東の道路に、乗る駅の出入口（x = 98）は西の道路にある。
    let (mut s, ids) = Street::with_roads(&[104, 250, 98, 240], &[(0, 100), (102, 260)]);
    let (home, work) = (ids[0], ids[1]);
    let (planner, line, _) = rail_planner(&s, &[ids[2], ids[3]]);
    let choice = rail_choice(&planner, line, &[100, 100]);
    let front = |b: BuildingId| s.buildings.get(b).unwrap().front;
    let offer = choice
        .offer(&WalkEnd::at(front(home)), &WalkEnd::at(front(work)))
        .expect("鉄道の候補がない");
    assert_eq!(offer.board.index, 0);
    let h = Household { home };
    let mut r = h.request(0, (home, work));
    r.owns_car = false;
    r.place = None;
    let id = start_with_rides(&mut s, r, &choice).expect("移動を作れない");
    assert_eq!(mode_of(&s, id), "walk");
}

/// 同じ移動でも、蓄えの多い世帯は運賃を軽く感じて鉄道に乗り、蓄えのない世帯は歩く。
#[test]
fn poorer_households_feel_the_fare_more() {
    use crate::happiness::SAVINGS_FOR_FULL_BONUS;
    use crate::rail::choice::{tile_distance_mm, walk_seconds};
    // 家（x = 0）、300m 先の職場（x = 38）と、駅の出入口（x = 1、37）。
    let (mut s, ids) = Street::with_roads(&[0, 38, 1, 37], &[(0, 60)]);
    let (home, work) = (ids[0], ids[1]);
    let (planner, line, _) = rail_planner(&s, &[ids[2], ids[3]]);
    let front = |b: BuildingId| s.buildings.get(b).unwrap().front;
    let (a, b) = (front(home), front(work));
    let target = walk_seconds(tile_distance_mm(a, b));
    let (poor, rich) = (Money::ZERO, SAVINGS_FOR_FULL_BONUS);
    // 蓄えの多い世帯には歩きより安く、蓄えのない世帯には歩きより高い区間の時間を探す。
    let segs = (0..20_000_u32)
        .map(|t| [t, 1])
        .find(|segs| {
            let c = rail_choice(&planner, line, segs);
            let cost = |m| {
                c.offer_for(&WalkEnd::at(a), &WalkEnd::at(b), m)
                    .unwrap()
                    .cost
            };
            cost(rich) < target && cost(poor) >= target
        })
        .expect("運賃の重さで手段の分かれる区間の時間がない");
    let choice = rail_choice(&planner, line, &segs);
    let h = Household { home };
    for (savings, want) in [(rich, "rail"), (poor, "walk")] {
        let mut r = h.request(0, (home, work));
        r.owns_car = false;
        r.savings = savings;
        let id = start_with_rides(&mut s, r, &choice).unwrap();
        assert_eq!(mode_of(&s, id), want, "蓄え {savings:?}");
        s.trips.cancel(id);
    }
}

/// 車の費用に足す停める手間は、行き先の駐車場に空きがあれば [`PARK_SECONDS`] だけで、満車なら近くの駐車場から
/// 歩く分が足される。
#[test]
fn a_full_lot_at_the_destination_adds_to_the_car_cost() {
    use crate::rail::choice::{PARK_SECONDS, walk_seconds};
    let (mut s, ids) = Street::with_roads(&[0, 250], &[(0, 260)]);
    let (home, work) = (ids[0], ids[1]);
    let h = Household { home };
    let r = h.request(0, (home, work));
    assert_eq!(
        s.trips.parking_seconds(&r, (&s.buildings, &[])),
        PARK_SECONDS
    );
    let spaces = s
        .buildings
        .get(work)
        .unwrap()
        .parking_spaces(s.buildings.seed());
    assert!(spaces > 0, "職場に駐車場がない");
    for _ in 0..spaces {
        s.trips.parking.reserve(LotId::Building(work));
    }
    assert_eq!(
        s.trips.parking_seconds(&r, (&s.buildings, &[])),
        PARK_SECONDS + walk_seconds(PARK_RADIUS * 8_000 / 2)
    );
}

/// 共同の駐車場の枠を借りた世帯は、家から出るときと家に帰るときだけ、家と駐車場のあいだを歩く時間と、1 日の料金の
/// 半分を蓄えの重さで秒に直した分を、車の費用に足す。家に関わらない移動には足さない。
#[test]
fn a_renter_adds_the_walk_and_half_the_fee_to_the_car_cost() {
    use crate::rail::choice::{PARK_SECONDS, tile_distance_mm, walk_seconds};
    use crate::rail::money_seconds;
    let (s, ids) = Street::new(&[50, 5, 10]);
    let (home, work, shop) = (ids[0], ids[1], ids[2]);
    let facilities = [shared_lot(&s, 40)];
    let c = facilities[0].area.min;
    let h = Household { home };
    let request = |ends| CommuteRequest {
        place: Some(CarPlace::Shared(c)),
        parking_fee: 30,
        ..h.request(0, ends)
    };
    let sites = (&s.buildings, facilities.as_slice());
    let walk = walk_seconds(tile_distance_mm(
        s.buildings.get(home).unwrap().front,
        facilities[0].front,
    ));
    let fee = money_seconds(30, Money::ZERO) / 2;
    assert!(walk > 0 && fee > 0);
    assert_eq!(
        s.trips.parking_seconds(&request((home, work)), sites),
        PARK_SECONDS + walk + fee
    );
    // 家に帰る車は借りた枠に停めるので、満車の分は足さない。
    assert_eq!(
        s.trips.parking_seconds(&request((work, home)), sites),
        PARK_SECONDS + walk + fee
    );
    // 家に関わらない移動は、停めて出す手間だけである。
    assert_eq!(
        s.trips.parking_seconds(&request((work, shop)), sites),
        PARK_SECONDS
    );
}
