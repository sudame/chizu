use super::*;
use crate::building::{Building, Buildings};
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
        let ended = self.trips.step(&self.net, &mut self.router);
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
    // 遠い建物は範囲の外なので、どこも満車で車では行けない。駐車場は変わらない。
    let before = s.trips.clone();
    assert_eq!(s.drive(far, shop), None);
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
    // 1 tick 目で 2 台とも車に乗り、車間の検査は `step` のたびに走る。
    s.step();
    let on_road = |id| s.trips.get(id).is_some_and(|t| t.mode() == LaneMode::Car);
    assert!(on_road(a) && on_road(b));
    let mut ended = Vec::new();
    for _ in 0..300 {
        ended.extend(s.step());
    }
    ended.sort();
    assert_eq!(ended, [a, b]);
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
    assert_eq!(s.step(), [id]);
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
