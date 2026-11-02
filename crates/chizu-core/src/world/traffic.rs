//! 通勤の移動の結果の集計と、住民の居場所の数え方。
//!
//! どちらも読み取りのための値で、状態ハッシュには入れない。集計は移動の結果から決まるので、
//! 同じ記録を再生すれば同じ値になる。

use super::World;
use crate::citizens::Activity;
use crate::clock::TICKS_PER_DAY;
use crate::movement::{Congestion, Finished};
use crate::network::SegmentId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::movement::Trips;
use crate::units::{Mm, WorldPos};
use crate::{LaneMode, TripId};

/// 1 日ぶんの、通勤の移動の結果。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DayTraffic {
    /// 数えた日（0 から数える）。
    pub day: u64,
    /// 行き先に着いた移動の数。
    pub arrived: u64,
    /// 途中で経路がなくなって消えた移動の数。
    pub abandoned: u64,
    /// 出発のときに経路が見つからず、移動を始められなかった数。
    pub no_route: u64,
    /// 着いた移動にかかった時間の合計（tick）。
    pub travel_ticks: u64,
    /// その日の tick ごとの、歩いている人の数の合計（人・tick）。通勤に限らず、歩いているすべての人を数える。
    /// 横断歩道を外して遠回りが増えると増える。
    pub walking_ticks: u64,
}

/// 通勤の移動の結果の集計。今日と前の日だけを持つ。状態ハッシュには入れない。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct TrafficTally {
    today: DayTraffic,
    yesterday: Option<DayTraffic>,
}

impl TrafficTally {
    /// `day` の欄を返す。日が変わっていたら、前の日に送る。
    fn day_mut(&mut self, day: u64) -> &mut DayTraffic {
        if self.today.day != day {
            self.yesterday = (self.today.day + 1 == day).then_some(self.today);
            self.today = DayTraffic {
                day,
                ..DayTraffic::default()
            };
        }
        &mut self.today
    }
}

/// 通勤の移動の結果。今日と前の日。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Traffic {
    pub today: DayTraffic,
    /// 前の日の集計。始まってまだ 1 日たっていないときは `None`。
    pub yesterday: Option<DayTraffic>,
}

/// 同じ位置とみなすずれ（mm）。道路が分けられたときの、車線の中の位置の丸めを許す。
const SAME_SPOT_MM: i64 = 5;

/// ある tick の、走っている車のワールド座標の位置。止まっている車を見分けるのに使う。
/// 車線の ID は、道路が分けられると変わるので、位置は車線によらない座標で持つ。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CarSample(BTreeMap<TripId, WorldPos>);

impl CarSample {
    pub(crate) fn of(trips: &Trips) -> CarSample {
        CarSample(
            trips
                .iter()
                .filter(|(_, t)| t.mode() == LaneMode::Car)
                .map(|(id, t)| (id, t.position().world()))
                .collect(),
        )
    }

    /// 車の台数。車線に出る順番を待っている車も含む。
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// 同じ場所（ずれは数 mm まで）にいる車の台数。`earlier` にいなかった車は、動いたものとして数えない。
    pub fn unmoved_since(&self, earlier: &CarSample) -> usize {
        let near = |a: Mm, b: Mm| a.0.abs_diff(b.0) <= SAME_SPOT_MM.unsigned_abs();
        self.0
            .iter()
            .filter(|(id, at)| {
                earlier
                    .0
                    .get(id)
                    .is_some_and(|p| near(p.x, at.x) && near(p.y, at.y))
            })
            .count()
    }
}

/// 住民の居場所ごとの人数。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CitizenPlaces {
    /// 家にいる。
    pub at_home: u64,
    /// 職場または学校にいる。
    pub at_work: u64,
    /// 帰りに寄った店にいる。
    pub at_shop: u64,
    /// 歩いて移動している。車を降りて歩く区切りも含む。
    pub walking: u64,
    /// 車に乗って移動している。
    pub driving: u64,
    /// 家がない世帯の住民。
    pub homeless: u64,
}

impl World {
    /// 通勤の移動の結果の集計。今日と前の日。状態は変えない。
    pub fn traffic(&self) -> Traffic {
        let day = self.tick.0 / TICKS_PER_DAY;
        let t = &self.traffic;
        let today = if t.today.day == day {
            t.today
        } else {
            DayTraffic {
                day,
                ..DayTraffic::default()
            }
        };
        let yesterday = if t.today.day == day {
            t.yesterday
        } else {
            (t.today.day + 1 == day).then_some(t.today)
        };
        Traffic { today, yesterday }
    }

    /// 消えた通勤の移動の結果を集計に足す。
    pub(super) fn tally_finished(&mut self, f: Finished) {
        let now = self.tick;
        let day = self.traffic.day_mut(now.0 / TICKS_PER_DAY);
        if f.reached {
            day.arrived += 1;
            day.travel_ticks += now.0.saturating_sub(f.departed.0);
        } else {
            day.abandoned += 1;
        }
    }

    /// この tick に歩いている人の数を足す。
    pub(super) fn tally_walking(&mut self) {
        let walking = self
            .trips
            .iter()
            .filter(|(_, t)| t.mode() == LaneMode::Sidewalk)
            .count();
        let day = self.tick.0 / TICKS_PER_DAY;
        self.traffic.day_mut(day).walking_ticks += walking as u64;
    }

    /// 経路が見つからず、移動を始められなかった通勤を足す。
    pub(super) fn tally_no_route(&mut self, n: u64) {
        let day = self.tick.0 / TICKS_PER_DAY;
        self.traffic.day_mut(day).no_route += n;
    }

    /// いま走っている車の位置。状態は変えない。
    pub fn car_sample(&self) -> CarSample {
        CarSample::of(&self.trips)
    }

    /// 車のいる区間の混み具合。空いている区間は載せない。状態は変えず、状態ハッシュにも入れない。
    pub fn road_congestion(&self) -> BTreeMap<SegmentId, Congestion> {
        self.trips.congestion(&self.network)
    }

    /// 住民の居場所ごとの人数。状態は変えない。
    pub fn citizen_places(&self) -> CitizenPlaces {
        let p = &self.population;
        let mut out = CitizenPlaces::default();
        for id in p.citizen_ids() {
            let row = p.citizen(id).expect("いる住民の行がない");
            if let Some(t) = row.trip.and_then(|t| self.trips.get(t)) {
                match t.mode() {
                    LaneMode::Car => out.driving += 1,
                    LaneMode::Sidewalk => out.walking += 1,
                }
                continue;
            }
            if p.home(row.household).flatten().is_none() {
                out.homeless += 1;
                continue;
            }
            let at_shop = self.errands.at_shop(id).is_some();
            let at_work = row.workplace.is_some();
            match row.schedule.next {
                Activity::Work => out.at_home += 1,
                Activity::Home if at_shop => out.at_shop += 1,
                Activity::Home | Activity::Shop if at_work => out.at_work += 1,
                Activity::Home | Activity::Shop => out.at_home += 1,
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use crate::clock::TICKS_PER_DAY;
    use crate::command::{Command, ZoneKind};
    use crate::tile::{TilePos, TileRect};
    use crate::world::ApplyOutcome;
    use crate::world::economy::TICKS_PER_HOUR;
    use crate::world::people::Shift;
    use crate::world::people::tests::{city, run_days};

    /// 住民の居場所の人数は、住民の数と合う。
    #[test]
    fn places_add_up_to_the_citizens() {
        let mut w = city(&[ZoneKind::Residential, ZoneKind::Industrial]);
        run_days(&mut w, 2);
        let peak = Shift::of(ZoneKind::Industrial).leave_home;
        while w.tick().0 < 2 * TICKS_PER_DAY + peak + TICKS_PER_HOUR / 2 {
            w.step();
        }
        let p = w.citizen_places();
        let sum = p.at_home + p.at_work + p.at_shop + p.walking + p.driving + p.homeless;
        assert_eq!(sum, w.population().citizen_count() as u64);
        assert!(p.walking + p.driving > 0, "朝の通勤で誰も移動していない");
        assert!(p.at_home > 0 && p.at_work > 0);
    }

    /// 着いた移動の数と平均の移動時間が日ごとに溜まり、日が変わると前の日に送られる。
    #[test]
    fn arrivals_are_tallied_per_day() {
        let mut w = city(&[ZoneKind::Residential, ZoneKind::Industrial]);
        run_days(&mut w, 3);
        let t = w.traffic();
        assert_eq!(t.today.day, 3);
        let y = t.yesterday.expect("前の日の集計がない");
        assert_eq!(y.day, 2);
        assert!(y.arrived > 0, "前の日に誰も着いていない");
        assert!(y.travel_ticks > 0);
        assert_eq!((y.abandoned, y.no_route), (0, 0), "つながった街で着けない");
    }

    /// 道が切れると、移動中の人は着けずに消え、`abandoned` に数えられる。
    #[test]
    fn a_cut_road_shows_up_as_unreachable() {
        use crate::world::people::JOB_INTERVAL;
        let mut w = city(&[ZoneKind::Residential, ZoneKind::Industrial]);
        run_days(&mut w, 2);
        // 朝の出発のさなかで、職場の割り当てを見直した直後まで進める。
        let peak = Shift::of(ZoneKind::Industrial).leave_home;
        while w.tick().0 < 2 * TICKS_PER_DAY + peak - TICKS_PER_HOUR / 2
            || w.tick().0 % JOB_INTERVAL != 1
        {
            w.step();
        }
        let before = w.traffic().today;
        assert_eq!((before.abandoned, before.no_route), (0, 0));
        // 通りを何か所か取り壊して、細かく分ける。
        for x in [-50, -35, -20, -5, 10] {
            let gap = TileRect::from_corners(TilePos::new(x, 10), TilePos::new(x + 1, 10));
            w.apply(&Command::Bulldoze { area: gap });
        }
        for _ in 0..JOB_INTERVAL - 2 {
            w.step();
        }
        let after = w.traffic().today;
        assert!(after.abandoned > 0, "切れた道で着けなくなった移動がない");
    }

    /// 行き先に経路が引けない通勤は、始められずに数える。
    #[test]
    fn departures_without_a_route_are_counted() {
        use crate::id::BuildingId;
        let mut w = city(&[ZoneKind::Residential, ZoneKind::Industrial]);
        run_days(&mut w, 1);
        let ids: Vec<_> = w.population().citizen_ids().take(5).collect();
        // 消えた建物を職場にして、経路を引けなくする。
        let gone = BuildingId {
            index: u32::MAX,
            generation: 0,
        };
        for id in &ids {
            w.population_mut().set_workplace(*id, Some(gone));
            let mut row = w.population().citizen(*id).unwrap().schedule;
            row.next = crate::citizens::Activity::Work;
            w.population_mut().set_schedule(*id, row);
        }
        let stuck = w.start_commutes(ids.clone());
        assert_eq!(stuck, ids);
        assert_eq!(w.traffic().today.no_route, ids.len() as u64);
    }

    /// 走り出す前の車は動いていない。進めると動いた車になる。
    #[test]
    fn cars_that_did_not_move_are_counted() {
        let mut w = city(&[ZoneKind::Residential, ZoneKind::Industrial]);
        run_days(&mut w, 2);
        let ids: Vec<_> = w.buildings().iter().map(|(id, _)| id).collect();
        let (from, to) = (ids[0], ids[ids.len() - 1]);
        let id = w.spawn_drive(from, to).expect("車の移動を作れない");
        // 歩く区切りを先に済ませる。
        while w
            .trips()
            .get(id)
            .is_some_and(|t| t.mode() != crate::LaneMode::Car)
        {
            w.step();
        }
        let before = w.car_sample();
        assert!(!before.is_empty());
        // 進めていないので、車は動いていない。
        assert_eq!(w.car_sample().unmoved_since(&before), before.len());
        for _ in 0..20 {
            w.step();
        }
        let after = w.car_sample();
        assert!(after.unmoved_since(&before) < before.len(), "車が動かない");
    }

    /// 転入の見込みの数は、世帯の数と食い違わない。
    #[test]
    fn migration_outlook_counts_homeless_households() {
        let mut w = city(&[ZoneKind::Residential, ZoneKind::Industrial]);
        run_days(&mut w, 1);
        let o = w.migration_outlook();
        let room = o.vacant_homes.saturating_sub(o.homeless_households);
        assert_eq!(
            o.blocked_households,
            o.wanting_households.saturating_sub(room)
        );
        assert_eq!(o.homeless_households, 0);
        assert_eq!(o.homeless_citizens, 0);
        // 住宅をすべて取り壊すと、住んでいた世帯は家なしになって空き家を待つ。
        let all = TileRect::from_corners(TilePos::new(-59, 4), TilePos::new(19, 9));
        assert_eq!(
            w.apply(&Command::PaintZone {
                area: all,
                zone: None
            }),
            ApplyOutcome::Applied
        );
        let road = TileRect::from_corners(TilePos::new(-60, 10), TilePos::new(20, 10));
        w.apply(&Command::Bulldoze { area: road });
        w.step();
        let o = w.migration_outlook();
        assert!(o.homeless_households > 0, "家なしの世帯がいない");
        assert!(o.homeless_citizens >= o.homeless_households);
        assert_eq!(o.vacant_homes, 0);
    }
}
