//! 外からの移住、職場の割り当て、1 日の予定。
//!
//! - 移住：道路で「外の世界」とつながった住宅に空きがあり、住宅の需要があれば、
//!   世帯が外から来て入る。来る数は需要と空き家の数に応じて増える。家を失った世帯がいれば、
//!   新しい世帯より先に空き家へ入る。家のないまま [`HOMELESS_LEAVE_TICKS`] たった世帯は街を出ていく。
//! - 建物が消えたら、その tick のうちに住む世帯を家なしにし、働く住民の職場を外す。
//! - 出ていく：幸福度の平均が境目より低い世帯は、1 時間ごとに、下回った点数に比例する確率で街を出ていく。
//!   幸福度の平均は住宅の需要にも効く。
//! - 職場：[`assign_jobs`] で割り当て、職場ができた住民に通勤の予定を立てる。
//! - 予定：家にいる住民は朝に職場へ、職場にいる住民は夕方に家へ出発する。出発の tick は
//!   出発の待ち行列に積み、時刻が来たら移動を始め（`world::commute`）、着いたら居場所を切り替える。

use std::collections::{BTreeMap, BTreeSet};

use super::World;
use crate::building::{BASE_RESIDENTIAL_DEMAND, CityStats, happiness_pull};
use crate::citizens::HappinessParts;
use crate::citizens::{Activity, MAX_HOUSEHOLD_SIZE, ScheduleState, WORKING_AGES, draw_household};
use crate::clock::TICKS_PER_DAY;
use crate::command::ZoneKind;
use crate::happiness;
use crate::id::{BuildingId, CitizenId, HouseholdId};
use crate::jobs::{assign_jobs, workers_by_building};
use crate::rng::{RngPurpose, rng};
use crate::service::Components;
use crate::units::Tick;
use crate::world::economy::TICKS_PER_HOUR;

/// 移住を見る間隔（tick）。等速で 2 秒に 1 回である。
pub const MIGRATION_INTERVAL: u64 = 60;

/// 住みたい人がいるときに、1 回に外から来る世帯の数の下限。
pub const MIGRANTS_PER_INTERVAL: u64 = 4;

/// 1 回に来る世帯の数に、住宅の需要の何分の 1 を足すか。
pub const MIGRANTS_PER_DEMAND: u64 = 4;

/// 1 回に来る世帯の数に、空き家の何分の 1 を足すか。
pub const MIGRANTS_PER_VACANCY: u64 = 8;

/// 世帯の住民の幸福度の平均がこれより低いと、世帯は街を出ていくことがある。
pub const LEAVE_BELOW: u8 = 45;

/// 世帯の幸福度の平均が [`LEAVE_BELOW`] を 1 点下回るごとに、1 時間あたりに出ていく確率に足す値（‰）。
pub const LEAVE_PERMILLE_PER_POINT: u32 = 5;

/// 越してきた人の見込みの幸福度が [`LEAVE_BELOW`] をこの点数だけ上回れば、移住の数を減らさない。
/// 上回る点数がこれより少なければ、その割合まで減らす。
pub const APPEAL_FULL_MARGIN: u64 = 40;

/// 家のない世帯が街を出ていくまでの時間（tick）。ゲーム内の 1 日である。
pub const HOMELESS_LEAVE_TICKS: u64 = TICKS_PER_DAY;

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
    ///
    /// 住める世帯の数は、家のある世帯の数に、移り住める空き家（[`World::vacant_homes`]）の数を足したものとする。
    /// 外の世界とつながらない家の空きを数えると、埋まらない空き家のせいで
    /// 住宅の需要が負のまま動かなくなるためである。
    pub fn city_stats(&self) -> CityStats {
        let p = &self.population;
        let cap = self.buildings.capacity();
        let households = p
            .household_ids()
            .filter(|h| {
                p.home(*h)
                    .flatten()
                    .is_some_and(|b| self.buildings.get(b).is_some())
            })
            .count() as u64;
        let vacant: u64 = self.vacant_homes().iter().map(|(_, n)| n).sum();
        let mut s = CityStats {
            citizens: p.citizen_count() as u64,
            households,
            homes: households + vacant,
            workplaces: cap.workplaces,
            ..CityStats::default()
        };
        for zone in ZoneKind::ALL {
            let built = match zone {
                ZoneKind::Residential => cap.households > 0,
                _ => cap.workplaces[zone.index()] > 0,
            };
            s.zoned[zone.index()] = built || self.lots.has_zone(zone);
        }
        let (mut happy_sum, mut housed) = (0_u64, 0_u64);
        for id in p.citizen_ids() {
            let row = p.citizen(id).expect("いる住民の行がない");
            if WORKING_AGES.contains(&row.age) {
                s.working_age += 1;
            }
            let home = p.home(row.household).flatten();
            if home.is_some_and(|b| self.buildings.get(b).is_some()) {
                happy_sum += u64::from(happiness::total(row.happiness));
                housed += 1;
            }
            if let Some(b) = row.workplace.and_then(|w| self.buildings.get(w)) {
                s.employed += 1;
                s.filled[b.zone.index()] += 1;
            }
        }
        s.happiness = match housed {
            0 => happiness::total(HappinessParts::NEUTRAL),
            n => (happy_sum / n) as u8,
        };
        s
    }

    /// いま職場にいる住民の数。移動している住民と、帰りに寄った店にいる住民は数えない。
    pub fn citizens_at_work(&self) -> usize {
        let p = &self.population;
        p.citizen_ids()
            .filter(|id| {
                p.citizen(*id).is_some_and(|r| {
                    let here = match r.schedule.next {
                        Activity::Home => self.errands.at_shop(*id).is_none(),
                        Activity::Shop => true,
                        Activity::Work => false,
                    };
                    r.workplace.is_some() && here && r.trip.is_none()
                })
            })
            .count()
    }

    /// 1 tick ぶんの住民の処理。`step` から呼ぶ。
    pub(super) fn update_people(&mut self) {
        self.run_departures();
        self.plan_daytime_errands();
        let tick = self.tick.0;
        if tick.is_multiple_of(MIGRATION_INTERVAL) {
            self.migrate();
        }
        if tick.is_multiple_of(JOB_INTERVAL) {
            self.update_jobs();
            self.drop_stale_errands();
        }
        if tick.is_multiple_of(TICKS_PER_HOUR) {
            self.emigrate();
        }
    }

    /// 家のある世帯のうち、住民の幸福度の平均が [`LEAVE_BELOW`] より低い世帯を、下回った点数に比例する確率で
    /// 街から出す。1 時間ごとに呼ばれる。職場の割り当てや移住の直後でも正しく判定できるよう、点数は
    /// 組ごとの更新で書いた値ではなく、いまの状態から求め直す。
    fn emigrate(&mut self) {
        let p = &self.population;
        let ctx = happiness::CitizenContext {
            seed: self.seed,
            tick: self.tick,
            population: p,
            buildings: &self.buildings,
            tax_permille: self.tax_permille,
        };
        let leaving: Vec<HouseholdId> = p
            .household_ids()
            .filter(|h| p.home(*h).flatten().is_some())
            .filter(|h| {
                let members = p.members(*h).expect("世帯がない");
                let n = members.len() as u64;
                if n == 0 {
                    return false;
                }
                let sum: u64 = members
                    .iter()
                    .map(|id| {
                        let row = p.citizen(*id).expect("住民がない");
                        u64::from(happiness::total(happiness::parts_of(&ctx, &row)))
                    })
                    .sum();
                let below = (u64::from(LEAVE_BELOW) * n).saturating_sub(sum).div_ceil(n);
                let permille = (below * u64::from(LEAVE_PERMILLE_PER_POINT)).min(1_000) as u32;
                permille > 0
                    && rng(
                        self.seed,
                        RngPurpose::Emigration,
                        u64::from(h.index),
                        self.tick,
                    )
                    .chance(permille, 1_000)
            })
            .collect();
        for h in leaving {
            self.remove_household(h);
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

    /// 出発の時刻が来た住民の移動を始める。移動を作れなかった住民は、その場で居場所を切り替える。
    fn run_departures(&mut self) {
        let due = self.population.pop_departures(self.tick);
        let stuck = self.start_commutes(due);
        self.switch_places(stuck);
    }

    /// 行き先に着いた住民の居場所を切り替え、次の出発を積む。
    pub(super) fn switch_places(&mut self, ids: Vec<CitizenId>) {
        for id in ids {
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
            let schedule = self.plan_errand(id, &row, schedule);
            self.population.set_schedule(id, schedule);
        }
    }

    /// 道路で「外の世界」とつながった住宅のうち、空きのあるものと空きの数。建物の ID の順に並ぶ。
    /// CS2 にならい、電気や水道が届いていない家にも移り住む。届かないぶんは生活サービスの点数が下がる。
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
            .filter(|(_, b)| b.zone == ZoneKind::Residential)
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

    /// 消えた建物に住む世帯を家なしにし、そこで働く住民の職場を外す。
    pub(super) fn vacate(&mut self, removed: &[BuildingId]) {
        if removed.is_empty() {
            return;
        }
        let removed: BTreeSet<BuildingId> = removed.iter().copied().collect();
        let gone = |b: Option<BuildingId>| b.is_some_and(|b| removed.contains(&b));
        let lost: Vec<HouseholdId> = self
            .population
            .household_ids()
            .filter(|h| gone(self.population.home(*h).flatten()))
            .collect();
        for h in lost {
            self.population.set_home(h, None, self.tick);
            let members = self.population.members(h).expect("世帯がない").to_vec();
            for id in members {
                self.population.set_workplace(id, None);
                self.population.set_schedule(id, ScheduleState::default());
            }
        }
        let jobless: Vec<CitizenId> = self
            .population
            .citizen_ids()
            .filter(|id| gone(self.population.citizen(*id).and_then(|r| r.workplace)))
            .collect();
        for id in jobless {
            self.population.set_workplace(id, None);
            self.population.set_schedule(id, ScheduleState::default());
        }
    }

    /// 家のないまま長くたった世帯を街から出し、空き家に、家なしの世帯と外から来た世帯を入れる。
    fn migrate(&mut self) {
        let homeless: Vec<HouseholdId> = self
            .population
            .household_ids()
            .filter(|h| self.population.home(*h) == Some(None))
            .collect();
        let mut staying = Vec::new();
        for h in homeless {
            match self.population.homeless_since(h).flatten() {
                Some(since) if self.tick.since(since) >= HOMELESS_LEAVE_TICKS => {
                    self.remove_household(h);
                }
                _ => {
                    self.population.set_home(h, None, self.tick);
                    staying.push(h);
                }
            }
        }
        let vacant = self.vacant_homes();
        let free: u64 = vacant.iter().map(|(_, n)| n).sum();
        let mut homes = vacant
            .into_iter()
            .flat_map(|(id, free)| (0..free).map(move |_| id));
        for h in staying {
            let Some(home) = homes.next() else {
                return;
            };
            self.population.set_home(h, Some(home), self.tick);
        }
        // 住宅の需要の式から空き家の項を除いた、住みたい人がいるかどうか。
        let s = self.city_stats();
        let jobs: u64 = s.workplaces.iter().sum();
        let filled: u64 = s.filled.iter().sum();
        let jobless = s.working_age.saturating_sub(s.employed);
        let want = BASE_RESIDENTIAL_DEMAND
            + happiness_pull(s.happiness)
            + i64::try_from(jobs.saturating_sub(filled)).expect("職場の数があふれた")
            - i64::try_from(jobless / 4).expect("住民の数があふれた");
        if want <= 0 {
            return;
        }
        let pull = u64::try_from(want).expect("正の需要") / MIGRANTS_PER_DEMAND;
        let count = MIGRANTS_PER_INTERVAL + pull + free / MIGRANTS_PER_VACANCY;
        // 越してきた人の見込みの幸福度が、出ていく境目に近いほど来る数を減らす。
        let tax = self.tax_permille[ZoneKind::Residential.index()];
        let margin = u64::from(happiness::newcomer_happiness(tax).saturating_sub(LEAVE_BELOW));
        let count = count * margin.min(APPEAL_FULL_MARGIN) / APPEAL_FULL_MARGIN;
        for k in 0..count {
            let Some(home) = homes.next() else {
                return;
            };
            let mut r = rng(self.seed, RngPurpose::Migration, k, self.tick);
            let (money, ages) = draw_household(&mut r, MAX_HOUSEHOLD_SIZE);
            let h = self.population.add_household(money);
            self.population.set_home(h, Some(home), self.tick);
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
pub(crate) mod tests {
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
    pub(crate) fn city(zones: &[ZoneKind]) -> World {
        city_with(zones, true)
    }

    /// [`city`] と同じ街で、`services` が偽なら発電所と浄水場を置かない。
    fn city_with(zones: &[ZoneKind], services: bool) -> World {
        let mut w = World::new_with_money(3, crate::units::Money(100_000_000));
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
        for i in (0..10).filter(|_| services) {
            let plant = Command::PlaceBuilding {
                kind: ServiceKind::PowerPlant,
                at: TilePos::new(-56 + 4 * i, -41),
                rotation: 2,
            };
            apply(&mut w, plant);
        }
        for i in (0..14).filter(|_| services) {
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

    pub(crate) fn run_days(w: &mut World, days: u64) {
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
        // どの世帯も、残っている住宅に住む。
        for h in p.household_ids() {
            let home = p.home(h).flatten().expect("家のない世帯がいる");
            let b = w.buildings().get(home).expect("消えた家に住んでいる");
            assert_eq!(b.zone, ZoneKind::Residential);
        }
        let stats = w.city_stats();
        assert!(stats.employed > 0, "誰も働いていない");
        // 職場のある住民は、移動しているか、出発の待ち行列にちょうど 1 回入っている。
        let workers = p
            .citizen_ids()
            .filter(|id| p.citizen(*id).unwrap().workplace.is_some())
            .count();
        let moving = p
            .citizen_ids()
            .filter(|id| p.citizen(*id).unwrap().trip.is_some())
            .count();
        assert_eq!(p.departures().len() + moving, workers);
        // 昼には職場にいて、夜には家にいる。出発から 4 時間待つ。移動は 1 tick に移動の 1 秒しか進まず、
        // 歩道は片側を一方の向きにしか歩けないので、この街の徒歩の通勤は回り道で 1,000 tick 近くかかる。
        let day_start = w.tick().0 - w.tick().0 % TICKS_PER_DAY;
        let wait = 4 * u64::from(DEPARTURE_SPREAD);
        // 母数は判定する時点の働く住民である。朝の出発のあとに職場が決まり、次の朝を待っている住民は除く。
        let workers_now = |w: &World| {
            let p = w.population();
            p.citizen_ids()
                .map(|id| p.citizen(id).unwrap())
                .filter(|r| r.workplace.is_some())
                .filter(|r| r.schedule.next != Activity::Work || r.schedule.depart_at.is_none())
                .count()
        };
        while w.tick().0 < day_start + TICKS_PER_DAY + WORK_START + wait {
            w.step();
        }
        let workers = workers_now(&w);
        assert!(
            w.citizens_at_work() * 10 >= workers * 9,
            "朝に職場へ向かわない：{} / {workers}",
            w.citizens_at_work()
        );
        while w.tick().0 < day_start + TICKS_PER_DAY + WORK_END + wait {
            w.step();
        }
        let workers = workers_now(&w);
        assert!(
            w.citizens_at_work() * 10 <= workers,
            "夕方に家へ帰らない：{} / {workers}",
            w.citizens_at_work()
        );
    }

    /// 区画の組み合わせごとに、早すぎる時点で止まらず、需要が 0 のまわりに収まっていくことを確かめる。
    #[test]
    fn demand_settles_for_every_zone_mix() {
        use ZoneKind::*;
        let mixes: [&[ZoneKind]; 6] = [
            &[Residential],
            &[Residential, Commercial],
            &[Residential, Industrial],
            &[Residential, Commercial, Industrial],
            &[Residential, Industrial, Office],
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
            // 工業もオフィスもない街は職のない住民が多く、暮らし向きの低い世帯が出ていくので、人口が減ることがある。
            if mix.contains(&Industrial) || mix.contains(&Office) {
                assert!(late.citizens >= early.citizens, "{mix:?}：人口が減った");
            }
            // 働き口のある区画が 2 種類以上ある街は、働き口と住まいが互いを呼び合って、2 日目のあとも育ち続ける。
            if mix.len() >= 3 {
                let (num, den) = if mix.len() == 4 { (3, 2) } else { (5, 4) };
                assert!(
                    late.citizens * den > early.citizens * num,
                    "{mix:?}：{early:?} → {late:?}"
                );
            }
            // 工業かオフィスがあれば、塗られた区画で働き手を吸い切り、職のない住民が溜まらない。
            if mix.contains(&Industrial) {
                let jobless = late.working_age - late.employed;
                assert!(jobless * 10 <= late.working_age, "{mix:?}：{late:?}");
            }
            // 塗っていない区画の需要は建物にならないので見ない。
            for (i, (a, b)) in d4.iter().zip(&d5).enumerate() {
                if !late.zoned[i] {
                    continue;
                }
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

    /// 建物が消えたその tick に、住む世帯は家なしになり、働く住民は職場を失う。
    /// 家なしのまま 1 日たった世帯は街を出ていく。
    #[test]
    fn removed_buildings_free_their_households_and_workers_at_once() {
        let mut w = city(&[ZoneKind::Residential, ZoneKind::Industrial]);
        run_days(&mut w, 1);
        let before = w.city_stats();
        assert!(before.households > 0 && before.employed > 0);
        // 区画を消すと、建物がすべて消える。
        let area = TileRect::from_corners(TilePos::new(-59, 4), TilePos::new(19, 46));
        apply(&mut w, Command::PaintZone { area, zone: None });
        assert!(w.buildings().is_empty());
        let p = w.population();
        assert!(p.household_ids().all(|h| p.home(h) == Some(None)));
        assert!(
            p.citizen_ids()
                .all(|id| p.citizen(id).unwrap().workplace.is_none())
        );
        assert!(p.departures().is_empty());
        let s = w.city_stats();
        assert_eq!((s.households, s.employed), (0, 0));
        assert_eq!(s.citizens, before.citizens);
        for _ in 0..TICKS_PER_DAY + MIGRATION_INTERVAL {
            w.step();
        }
        assert_eq!(w.population().household_count(), 0, "家なしの世帯が残る");
    }

    /// [`city`] に、y = 70 の通りを足して発電所と浄水場を並べ、区画を埋め切っても停電しない街。
    fn served_city(zones: &[ZoneKind]) -> World {
        let mut w = city(zones);
        road(&mut w, (-60, 70), (20, 70));
        road(&mut w, (-60, 40), (-60, 70));
        for i in 0..19 {
            let plant = Command::PlaceBuilding {
                kind: ServiceKind::PowerPlant,
                at: TilePos::new(-53 + 4 * i, 69),
                rotation: 2,
            };
            apply(&mut w, plant);
        }
        for i in 0..39 {
            let pump = Command::PlaceBuilding {
                kind: ServiceKind::WaterPump,
                at: TilePos::new(-59 + 2 * i, 71),
                rotation: 0,
            };
            apply(&mut w, pump);
        }
        w
    }

    fn set_tax(w: &mut World, permille: u16) {
        let zone = ZoneKind::Residential;
        apply(w, Command::SetTaxRate { zone, permille });
    }

    /// 住民の幸福度は組ごとの更新で決まり、住宅の税率を上げると暮らし向きが下がって世帯が出ていく。
    /// 税率を戻すと、また人が移り住む。
    #[test]
    fn raising_the_tax_drives_households_away() {
        let mut w = served_city(&[ZoneKind::Residential, ZoneKind::Industrial]);
        run_days(&mut w, 3);
        let before = w.city_stats();
        assert!(before.happiness >= 60, "{before:?}");
        let p = w.population();
        for id in p.citizen_ids() {
            let row = p.citizen(id).unwrap();
            assert_ne!(
                row.happiness,
                HappinessParts::NEUTRAL,
                "更新されない住民がいる"
            );
            assert_eq!(row.happiness.services, crate::citizens::HAPPINESS_MAX);
        }
        set_tax(&mut w, crate::command::MAX_TAX_PERMILLE);
        run_days(&mut w, 2);
        let after = w.city_stats();
        assert!(after.happiness < LEAVE_BELOW, "{after:?}");
        assert!(
            after.citizens * 2 < before.citizens,
            "{} 人から {} 人",
            before.citizens,
            after.citizens
        );
        set_tax(&mut w, crate::world::INITIAL_TAX_PERMILLE);
        run_days(&mut w, 1);
        let back = w.city_stats();
        assert!(
            back.citizens * 10 >= before.citizens * 9,
            "{} 人から {} 人に減り、戻して {} 人",
            before.citizens,
            after.citizens,
            back.citizens
        );
    }

    /// 税率を少しずつ上げると、人口は段差を作らずになだらかに減っていく。
    #[test]
    fn population_falls_gradually_as_the_tax_rises() {
        let mut w = served_city(&[ZoneKind::Residential, ZoneKind::Industrial]);
        run_days(&mut w, 3);
        let mut counts = vec![w.city_stats().citizens];
        for permille in (150..=300).step_by(25) {
            set_tax(&mut w, permille);
            run_days(&mut w, 1);
            counts.push(w.city_stats().citizens);
        }
        let peak = counts.iter().copied().max().unwrap();
        assert!(counts[counts.len() - 1] * 4 < peak, "{counts:?}");
        // 減り始めてからも、どの 1 段でもその時点の人口の半分より多くは減らず、何段にも分かれて減る。
        let mut falling = 0;
        for pair in counts.windows(2) {
            let drop = pair[0].saturating_sub(pair[1]);
            assert!(drop * 2 <= pair[0], "{counts:?}");
            falling += usize::from(drop * 10 > pair[0]);
        }
        assert!(falling >= 3, "{counts:?}");
    }

    /// 出ていくかは、組ごとの更新で書いた古い点数ではなく、いまの状態から決める。
    #[test]
    fn emigration_uses_the_current_scores() {
        let mut w = served_city(&[ZoneKind::Residential, ZoneKind::Industrial]);
        run_days(&mut w, 2);
        // 全員の書いてある点数を 0 にする。いまの状態から求めれば、職も家もある住民の幸福度は高い。
        let ids: Vec<_> = w.population().citizen_ids().collect();
        for id in ids {
            let row = w.population().citizen(id).unwrap();
            let zero = HappinessParts {
                commute: 0,
                services: 0,
                wealth: 0,
            };
            w.population_mut().apply_change(
                id,
                crate::citizens::CitizenChange {
                    schedule: row.schedule,
                    happiness: zero,
                },
            );
        }
        let before = w.population().household_count();
        w.emigrate();
        assert_eq!(w.population().household_count(), before);
    }

    /// 電気も水道もない街は、移り住んでもすぐ出ていくので、人口が伸び続けない。
    #[test]
    fn a_city_without_services_does_not_keep_growing() {
        let zones = [ZoneKind::Residential, ZoneKind::Industrial];
        let mut w = city_with(&zones, false);
        run_days(&mut w, 2);
        let early = w.city_stats();
        assert!(early.happiness < LEAVE_BELOW, "{early:?}");
        run_days(&mut w, 2);
        let late = w.city_stats();
        assert!(late.citizens <= early.citizens, "{early:?} → {late:?}");
        // 施設のある同じ街は、ずっと大きく育つ。
        let mut served = city(&zones);
        run_days(&mut served, 4);
        assert!(served.city_stats().citizens > late.citizens * 2);
    }
}
