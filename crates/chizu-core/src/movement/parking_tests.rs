use super::*;
use crate::building::{Building, Buildings, ParkingCut};
use crate::command::{RoadKind, ZoneKind};
use crate::id::BuildingId;
use crate::network::{Dir, Line};
use crate::rng::{RngPurpose, rng};
use crate::state_hash::state_hash_of;
use crate::tile::TileRect;

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
                    zone: ZoneKind::Commercial,
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
            .spawn_drive(net, router, &self.buildings, (from, to), Tick::ZERO)
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
            if let Some(lot) = t.then.lot().filter(|b| self.buildings.get(*b).is_some()) {
                *holding.entry(lot).or_default() += 1;
            }
        }
        for (id, lot) in self.trips.lots() {
            let b = self.buildings.get(id).expect("消えた建物に駐車場が残った");
            assert!(u64::from(lot.parked + lot.reserved) <= b.parking_spaces());
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
            let car = |id: &TripId| self.trips.get(*id).is_some_and(|t| t.mode() == LaneMode::Car);
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
        }
    );
    // 数は状態ハッシュに入らない。
    let mut other = s.trips.clone();
    other.parking_tally = ParkingTally::default();
    assert_eq!(state_hash_of(&other), hash);
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
        assert!(!a.is_some_and(|t| t.parks_elsewhere(&s.net, &s.buildings)));
        if let Some(t) = b {
            seen.push((t.mode(), t.parks_elsewhere(&s.net, &s.buildings)));
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
    use crate::id::CitizenId;
    use crate::movement::parking::Then;
    // 30 マスは 240m である。
    let (mut s, ids) = Street::new(&[0, 30]);
    let who = |index| Commuter {
        citizen: CitizenId {
            index,
            generation: 0,
        },
        to: Activity::Work,
    };
    let request = |index, owns_car| CommuteRequest {
        who: who(index),
        from: ids[0],
        to: ids[1],
        owns_car,
    };
    let requests = [request(0, true), request(1, false)];
    let (net, router) = (&s.net, &mut s.router);
    let out = s
        .trips
        .start_commutes(net, router, &s.buildings, &requests, Tick::ZERO);
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
        zone,
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
    let p = lot_spot(net, b).unwrap();
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
        near_west.parking_layout().cut,
        ParkingCut::Side { columns: 1, .. }
    ));
    // 西の交差点から 1 マスの西の端（x = 11）より、4 マスの東の端（x = 14）が遠い。
    assert_eq!(spot_x(&net, &near_west), mid(14));
    // 東の交差点のそばでは、西の端（x = 46）にする。出入口（x = 49）とは別の所になる。
    let near_east = wide(&net, ZoneKind::Commercial, 49, (4, 4), 1);
    assert_eq!(spot_x(&net, &near_east), mid(46));
    assert_ne!(lot_spot(&net, &near_east), building_spot(&net, &near_east));
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
    assert_eq!(one.parking_layout().cut, ParkingCut::FrontStrip { slots: 1 });
    assert_eq!(spot_x(&net, &one), mid(11) + 4_000 - 1_250);
    // 間口 2 マスの工業の段階 4 は 6 台で、帯は 15m。東の端の辺から 7.5m 内に入った所である。
    let two = wide(&net, ZoneKind::Industrial, 13, (2, 1), 4);
    assert_eq!(two.parking_layout().cut, ParkingCut::FrontStrip { slots: 6 });
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
                assert!(before - v <= CAR_DECEL, "減速の上限を破った {before} -> {v}");
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
