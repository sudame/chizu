//! 外からの移住、職場の割り当て、1 日の予定。
//!
//! - 移住：電気と水道が届き、道路で「外の世界」とつながった住宅に空きがあり、住宅の需要があれば、
//!   世帯が外から来て入る。家を失った世帯がいれば、新しい世帯より先に空き家へ入る。
//! - 職場：[`assign_jobs`] で割り当て、職場ができた住民に通勤の予定を立てる。
//! - 予定：家にいる住民は朝に職場へ、職場にいる住民は夕方に家へ出発する。出発の tick は
//!   出発の待ち行列に積み、時刻が来たら居場所を切り替える。移動そのものはまだ作らない。

use std::collections::BTreeMap;

use super::World;
use crate::building::{BASE_RESIDENTIAL_DEMAND, CityStats};
use crate::citizens::{Activity, MAX_HOUSEHOLD_SIZE, ScheduleState, WORKING_AGES, draw_household};
use crate::clock::TICKS_PER_DAY;
use crate::command::ZoneKind;
use crate::id::{BuildingId, CitizenId, HouseholdId};
use crate::jobs::{assign_jobs, workers_by_building};
use crate::rng::{RngPurpose, rng};
use crate::service::Components;
use crate::units::Tick;

/// 移住を見る間隔（tick）。等速で 2 秒に 1 回である。
pub const MIGRATION_INTERVAL: u64 = 60;

/// 1 回に外から来る世帯の数の上限。
pub const MIGRANTS_PER_INTERVAL: u64 = 4;

/// 職場の割り当てを見直す間隔（tick）。等速で 10 秒に 1 回である。
pub const JOB_INTERVAL: u64 = 300;

/// 職場へ出発する時刻（1 日の中の tick）。8 時である。
pub const WORK_START: u64 = TICKS_PER_DAY / 3;

/// 家へ出発する時刻（1 日の中の tick）。17 時である。
pub const WORK_END: u64 = TICKS_PER_DAY * 17 / 24;

/// 出発の時刻のばらつきの幅（tick）。1 時間である。
pub const DEPARTURE_SPREAD: u32 = (TICKS_PER_DAY / 24) as u32;

/// `after` より後で、1 日の中の時刻が `at_in_day` になる最初の tick。
fn next_time_of_day(after: Tick, at_in_day: u64) -> Tick {
    let day_start = after.0 - after.0 % TICKS_PER_DAY;
    let t = day_start + at_in_day;
    Tick(if t > after.0 { t } else { t + TICKS_PER_DAY })
}

impl World {
    /// 需要の式に使う、街の人口と働き口の数。
    pub fn city_stats(&self) -> CityStats {
        let p = &self.population;
        let cap = self.buildings.capacity();
        let mut s = CityStats {
            citizens: p.citizen_count() as u64,
            households: p
                .household_ids()
                .filter(|h| p.home(*h).flatten().is_some())
                .count() as u64,
            homes: cap.households,
            workplaces: cap.workplaces,
            ..CityStats::default()
        };
        for id in p.citizen_ids() {
            let row = p.citizen(id).expect("いる住民の行がない");
            if WORKING_AGES.contains(&row.age) {
                s.working_age += 1;
            }
            if let Some(b) = row.workplace.and_then(|w| self.buildings.get(w)) {
                s.employed += 1;
                s.filled[b.zone.index()] += 1;
            }
        }
        s
    }

    /// いま職場にいる住民の数。
    pub fn citizens_at_work(&self) -> usize {
        let p = &self.population;
        p.citizen_ids()
            .filter(|id| {
                p.citizen(*id)
                    .is_some_and(|r| r.workplace.is_some() && r.schedule.next == Activity::Home)
            })
            .count()
    }

    /// 1 tick ぶんの住民の処理。`step` から呼ぶ。
    pub(super) fn update_people(&mut self) {
        self.run_departures();
        let tick = self.tick.0;
        if tick.is_multiple_of(MIGRATION_INTERVAL) {
            self.migrate();
        }
        if tick.is_multiple_of(JOB_INTERVAL) {
            self.update_jobs();
        }
    }

    fn departure_jitter(&self, id: CitizenId) -> u64 {
        let mut r = rng(
            self.seed,
            RngPurpose::DailySchedule,
            u64::from(id.index),
            self.tick,
        );
        u64::from(r.below(DEPARTURE_SPREAD))
    }

    /// 出発の時刻が来た住民の居場所を切り替え、次の出発を積む。
    fn run_departures(&mut self) {
        for id in self.population.pop_departures(self.tick) {
            let row = self.population.citizen(id).expect("出発する住民がいない");
            let jitter = self.departure_jitter(id);
            let schedule = match (row.schedule.next, row.workplace) {
                // 職場に着いた。夕方に家へ向かう。
                (Activity::Work, Some(_)) => ScheduleState {
                    next: Activity::Home,
                    depart_at: Some(next_time_of_day(self.tick, WORK_END + jitter)),
                },
                // 家に着いた。職場があれば翌朝に職場へ向かう。
                (_, Some(_)) => ScheduleState {
                    next: Activity::Work,
                    depart_at: Some(next_time_of_day(self.tick, WORK_START + jitter)),
                },
                (_, None) => ScheduleState::default(),
            };
            self.population.set_schedule(id, schedule);
        }
    }

    /// 道路で「外の世界」とつながった、電気と水道の届く住宅のうち、空きのあるものと空きの数。建物の ID の順に並ぶ。
    fn vacant_homes(&self) -> Vec<(BuildingId, u64)> {
        let mut comps = Components::of(&self.network);
        let outside: Vec<usize> = self
            .network
            .nodes()
            .filter(|(_, n)| n.outside)
            .map(|(id, _)| comps.find(id.index as usize))
            .collect();
        let mut occupied = BTreeMap::<BuildingId, u64>::new();
        let p = &self.population;
        for h in p.household_ids() {
            if let Some(home) = p.home(h).flatten() {
                *occupied.entry(home).or_insert(0) += 1;
            }
        }
        self.buildings
            .iter()
            .filter(|(_, b)| b.zone == ZoneKind::Residential && b.supply.power && b.supply.water)
            .filter(|(_, b)| {
                comps
                    .of_link(&self.network, b.entrance)
                    .is_some_and(|c| outside.contains(&c))
            })
            .filter_map(|(id, b)| {
                let free = b
                    .households()
                    .saturating_sub(occupied.get(&id).copied().unwrap_or(0));
                (free > 0).then_some((id, free))
            })
            .collect()
    }

    /// 家を失った世帯を家なしにし、空き家に、家なしの世帯と外から来た世帯を入れる。
    fn migrate(&mut self) {
        let lost: Vec<HouseholdId> = self
            .population
            .household_ids()
            .filter(|h| {
                self.population.home(*h).flatten().is_some_and(|b| {
                    self.buildings
                        .get(b)
                        .is_none_or(|b| b.zone != ZoneKind::Residential)
                })
            })
            .collect();
        for h in lost {
            self.population.set_home(h, None);
            let members = self.population.members(h).expect("世帯がない").to_vec();
            for id in members {
                self.population.set_workplace(id, None);
                self.population.set_schedule(id, ScheduleState::default());
            }
        }
        let mut homes = self
            .vacant_homes()
            .into_iter()
            .flat_map(|(id, free)| (0..free).map(move |_| id));
        let homeless: Vec<HouseholdId> = self
            .population
            .household_ids()
            .filter(|h| self.population.home(*h) == Some(None))
            .collect();
        for h in homeless {
            let Some(home) = homes.next() else {
                return;
            };
            self.population.set_home(h, Some(home));
        }
        // 住宅の需要の式から空き家の項を除いた、住みたい人がいるかどうか。
        let s = self.city_stats();
        let jobs: u64 = s.workplaces.iter().sum();
        let filled: u64 = s.filled.iter().sum();
        let jobless = s.working_age.saturating_sub(s.employed);
        let want = BASE_RESIDENTIAL_DEMAND
            + i64::try_from(jobs.saturating_sub(filled)).expect("職場の数があふれた")
            - i64::try_from(jobless / 4).expect("住民の数があふれた");
        if want <= 0 {
            return;
        }
        for k in 0..MIGRANTS_PER_INTERVAL {
            let Some(home) = homes.next() else {
                return;
            };
            let mut r = rng(self.seed, RngPurpose::Migration, k, self.tick);
            let (money, ages) = draw_household(&mut r, MAX_HOUSEHOLD_SIZE);
            let h = self.population.add_household(money);
            self.population.set_home(h, Some(home));
            for age in ages {
                self.population
                    .add_citizen(h, age)
                    .expect("足したばかりの世帯がない");
            }
        }
    }

    /// 職場を割り当て直し、職場の変わった住民の予定を立て直す。
    fn update_jobs(&mut self) {
        for (id, workplace) in assign_jobs(&self.population, &self.buildings) {
            self.population.set_workplace(id, workplace);
            let schedule = match workplace {
                Some(_) => ScheduleState {
                    next: Activity::Work,
                    depart_at: Some(next_time_of_day(
                        self.tick,
                        WORK_START + self.departure_jitter(id),
                    )),
                },
                None => ScheduleState::default(),
            };
            self.population.set_schedule(id, schedule);
        }
    }

    /// 建物ごとの、住む世帯か働く住民の数。段階を上げるかの判定に使う。
    pub(super) fn occupants(&self) -> BTreeMap<BuildingId, u64> {
        let mut out = workers_by_building(&self.population, &self.buildings);
        let p = &self.population;
        for h in p.household_ids() {
            if let Some(home) = p.home(h).flatten() {
                *out.entry(home).or_insert(0) += 1;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::building::demand;
    use crate::command::{Command, RoadKind, ServiceKind};
    use crate::tile::{TilePos, TileRect};
    use crate::world::ApplyOutcome;

    fn apply(w: &mut World, c: Command) {
        assert_eq!(w.apply(&c), ApplyOutcome::Applied, "{c:?}");
    }

    fn road(w: &mut World, a: (i64, i64), b: (i64, i64)) {
        apply(
            w,
            Command::BuildRoad {
                from: TilePos::new(a.0, a.1),
                to: TilePos::new(b.0, b.1),
                kind: RoadKind::Small,
            },
        );
    }

    /// 2 本の通り（y = 10、40）に区画を塗り、y = -40 の通りに発電所と浄水場を並べ、外の世界とつないだ街。
    /// 区画の種類は、4 つの帯を `zones` の順に繰り返して塗る。
    fn city(zones: &[ZoneKind]) -> World {
        let mut w = World::new(3);
        for y in [-40, 10, 40] {
            road(&mut w, (-60, y), (20, y));
        }
        road(&mut w, (-60, -40), (-60, 10));
        road(&mut w, (-60, 10), (-60, 40));
        let inner = crate::world::highway_line(w.seed()).start;
        let corner = TilePos::new(inner.x, 10);
        if corner != inner {
            road(&mut w, (inner.x, inner.y), (corner.x, corner.y));
        }
        if !(-60..=20).contains(&corner.x) {
            road(&mut w, (corner.x, 10), (corner.x.clamp(-60, 20), 10));
        }
        for i in 0..10 {
            let plant = Command::PlaceBuilding {
                kind: ServiceKind::PowerPlant,
                at: TilePos::new(-56 + 4 * i, -41),
                rotation: 2,
            };
            apply(&mut w, plant);
        }
        for i in 0..14 {
            let pump = Command::PlaceBuilding {
                kind: ServiceKind::WaterPump,
                at: TilePos::new(-55 + 2 * i, -39),
                rotation: 0,
            };
            apply(&mut w, pump);
        }
        let bands = [(4, 9), (11, 16), (34, 39), (41, 46)];
        for (i, (y0, y1)) in bands.into_iter().enumerate() {
            let area = TileRect::from_corners(TilePos::new(-59, y0), TilePos::new(19, y1));
            let zone = Some(zones[i % zones.len()]);
            apply(&mut w, Command::PaintZone { area, zone });
        }
        w
    }

    fn run_days(w: &mut World, days: u64) {
        for _ in 0..days * TICKS_PER_DAY {
            w.step();
        }
    }

    #[test]
    fn next_time_of_day_is_strictly_later() {
        assert_eq!(next_time_of_day(Tick(0), WORK_START), Tick(WORK_START));
        assert_eq!(
            next_time_of_day(Tick(WORK_START), WORK_START),
            Tick(WORK_START + TICKS_PER_DAY)
        );
        assert_eq!(
            next_time_of_day(Tick(TICKS_PER_DAY + 1), 0),
            Tick(2 * TICKS_PER_DAY)
        );
    }

    #[test]
    fn migrants_take_jobs_and_commute_on_schedule() {
        let mut w = city(&[ZoneKind::Residential, ZoneKind::Industrial]);
        run_days(&mut w, 1);
        let p = w.population();
        assert!(p.household_count() > 0, "誰も移り住まない");
        // どの世帯も、電気と水道の届く住宅に住む。
        for h in p.household_ids() {
            let home = p.home(h).flatten().expect("家のない世帯がいる");
            let b = w.buildings().get(home).expect("消えた家に住んでいる");
            assert_eq!(b.zone, ZoneKind::Residential);
        }
        let stats = w.city_stats();
        assert!(stats.employed > 0, "誰も働いていない");
        // 職場のある住民は、出発の待ち行列にちょうど 1 回入っている。
        let workers = p
            .citizen_ids()
            .filter(|id| p.citizen(*id).unwrap().workplace.is_some())
            .count();
        assert_eq!(p.departures().len(), workers);
        // 昼には職場にいて、夜には家にいる。
        let day_start = w.tick().0 - w.tick().0 % TICKS_PER_DAY;
        while w.tick().0 < day_start + TICKS_PER_DAY + WORK_START + u64::from(DEPARTURE_SPREAD) {
            w.step();
        }
        assert!(
            w.citizens_at_work() * 10 >= workers * 9,
            "朝に職場へ向かわない"
        );
        while w.tick().0 < day_start + TICKS_PER_DAY + WORK_END + u64::from(DEPARTURE_SPREAD) {
            w.step();
        }
        assert!(w.citizens_at_work() * 10 <= workers, "夕方に家へ帰らない");
    }

    /// 区画の組み合わせごとに、早すぎる時点で止まらず、需要が 0 のまわりに収まっていくことを確かめる。
    #[test]
    fn demand_settles_for_every_zone_mix() {
        use ZoneKind::*;
        let mixes: [&[ZoneKind]; 4] = [
            &[Residential],
            &[Residential, Commercial],
            &[Residential, Industrial],
            &[Residential, Commercial, Industrial, Office],
        ];
        for mix in mixes {
            let mut w = city(mix);
            run_days(&mut w, 2);
            let early = w.city_stats();
            assert!(
                early.citizens >= 100,
                "{mix:?}：2 日で {} 人しか住まない",
                early.citizens
            );
            run_days(&mut w, 2);
            let d4 = demand(&w.city_stats());
            run_days(&mut w, 1);
            let late = w.city_stats();
            let d5 = demand(&late);
            assert!(late.citizens >= early.citizens, "{mix:?}：人口が減った");
            for (i, (a, b)) in d4.iter().zip(&d5).enumerate() {
                assert!(
                    b.abs() <= 150,
                    "{mix:?}：需要 {i} が {b} まで振れた（{late:?}）"
                );
                assert!(
                    *b <= (*a).max(0) + 30,
                    "{mix:?}：需要 {i} が {a} から {b} に伸び続けている"
                );
            }
        }
    }
}
