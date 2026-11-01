//! 住民の予定と、通勤の移動のつなぎ。
//!
//! 出発の時刻が来た住民は、居場所をすぐには切り替えず、家と職場のあいだの移動を始める。
//! 移動が終わったら居場所を切り替え、次の出発を積む。移動を作れなかった住民は、その場で着いたものとする。
//! 網が変わって行けなくなり、移動が途中で消えた住民も、行き先に着いたものとする。
//! 移動のあいだに職場や家が変わって予定が立て直されていたら、着いても予定は変えない。

use super::World;
use crate::citizens::Activity;
use crate::id::{CitizenId, HouseholdId, TripId};
use crate::movement::{CommuteRequest, Commuter};

impl World {
    /// 住民 `due` の移動を始める。移動を作れなかった住民を ID の順に返す。
    pub(super) fn start_commutes(&mut self, due: Vec<CitizenId>) -> Vec<CitizenId> {
        let mut stuck = Vec::new();
        let mut requests = Vec::new();
        for id in due {
            let Some(row) = self.population.citizen(id) else {
                continue;
            };
            if let Some(old) = row.trip {
                self.trips.cancel(old);
                self.population.set_trip(id, None);
            }
            let home = self.population.home(row.household).flatten();
            let ends = match row.schedule.next {
                Activity::Work => home.zip(row.workplace),
                Activity::Home => row.workplace.zip(home),
                Activity::Shop => None,
            };
            match ends {
                Some((from, to)) => requests.push(CommuteRequest {
                    who: Commuter {
                        citizen: id,
                        to: row.schedule.next,
                    },
                    from,
                    to,
                }),
                None => stuck.push(id),
            }
        }
        let (net, router) = (&self.network, &mut self.router.router);
        let trips = self
            .trips
            .start_commutes(net, router, &self.buildings, &requests, self.tick);
        for (r, trip) in requests.iter().zip(trips) {
            match trip {
                Some(t) => self.population.set_trip(r.who.citizen, Some(t)),
                None => stuck.push(r.who.citizen),
            }
        }
        stuck.sort_unstable();
        stuck
    }

    /// 世帯を街から出す。移動している住民の移動もやめさせる。
    pub(super) fn remove_household(&mut self, h: HouseholdId) {
        let members = self.population.members(h).unwrap_or_default().to_vec();
        for id in members {
            if let Some(t) = self.population.citizen(id).and_then(|r| r.trip) {
                self.trips.cancel(t);
            }
        }
        self.population.remove_household(h);
    }

    /// 終わった移動 `ended` の住民の居場所を切り替える。
    pub(super) fn end_commutes(&mut self, ended: Vec<TripId>) {
        let mut arrived = Vec::new();
        for trip in ended {
            let Some(c) = self.trips.take_commuter(trip) else {
                continue;
            };
            let Some(row) = self.population.citizen(c.citizen) else {
                continue;
            };
            if row.trip != Some(trip) {
                continue;
            }
            self.population.set_trip(c.citizen, None);
            if row.schedule.next == c.to && row.schedule.depart_at.is_none() {
                arrived.push(c.citizen);
            }
        }
        arrived.sort_unstable();
        self.switch_places(arrived);
    }
}

#[cfg(all(test, feature = "parallel"))]
mod tests {
    use crate::clock::TICKS_PER_DAY;
    use crate::command::ZoneKind;
    use crate::world::people::WORK_START;
    use crate::world::people::tests::{city, run_days};

    /// 朝の通勤のさなかまで回した街の状態ハッシュと、移動している住民の数。
    fn morning(threads: usize) -> (u64, usize) {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        pool.install(|| {
            let mut w = city(&[ZoneKind::Residential, ZoneKind::Industrial]);
            run_days(&mut w, 1);
            while w.tick().0 < TICKS_PER_DAY + WORK_START + 150 {
                w.step();
            }
            let p = w.population();
            let moving = p
                .citizen_ids()
                .filter(|id| p.citizen(*id).unwrap().trip.is_some())
                .count();
            (w.state_hash(), moving)
        })
    }

    #[test]
    fn commuting_does_not_depend_on_the_thread_count() {
        let (one, moving) = morning(1);
        assert!(moving > 0, "朝に誰も移動していない");
        assert_eq!(morning(4).0, one);
        assert_eq!(morning(8).0, one);
    }
}
