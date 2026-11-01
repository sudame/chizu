//! 住民の組ごとの更新と、出発の待ち行列。
//!
//! 住民は ID の添え字を [`BUCKET_COUNT`] で割った余りで組に分け、tick を同じ数で割った余りの組を
//! その tick に更新する。添え字は使い回しても変わらないので、住民の組は生きているあいだ変わらない。

use std::collections::BTreeSet;

#[cfg(feature = "parallel")]
use rayon::prelude::*;

use crate::citizens::{CitizenChange, CitizenRow, Population};
use crate::id::CitizenId;
use crate::state_hash::{StateHash, StateHasher};
use crate::units::Tick;

/// 住民を分ける組の数。等速（30 tick/秒）で、1 人あたり約 2 秒に 1 回の更新になる。
pub const BUCKET_COUNT: u32 = 64;

/// `tick` に更新する組。
pub fn bucket_at(tick: Tick) -> u32 {
    (tick.0 % u64::from(BUCKET_COUNT)) as u32
}

/// 住民の属する組。
pub fn bucket_of(id: CitizenId) -> u32 {
    id.index % BUCKET_COUNT
}

/// 組ごとの更新で 1 人の住民に行う処理。いまは何も変えない。
pub fn update_citizen(
    _seed: u64,
    _tick: Tick,
    _id: CitizenId,
    _row: CitizenRow,
) -> Option<CitizenChange> {
    None
}

/// 出発の待ち行列。`(出発 tick, CitizenId)` の順に並ぶので、同じ tick の出発は ID の順に出る。
///
/// 住民 1 人が入るのは高々 1 か所で、その tick は住民の予定の `depart_at` と一致する。
/// この整合は [`Population`] が保つ。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DepartureQueue {
    entries: BTreeSet<(Tick, CitizenId)>,
}

impl DepartureQueue {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 次に出発する住民。
    pub fn peek(&self) -> Option<(Tick, CitizenId)> {
        self.entries.first().copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = (Tick, CitizenId)> + '_ {
        self.entries.iter().copied()
    }

    pub(crate) fn insert(&mut self, at: Tick, id: CitizenId) {
        self.entries.insert((at, id));
    }

    pub(crate) fn remove(&mut self, at: Tick, id: CitizenId) {
        self.entries.remove(&(at, id));
    }

    pub(crate) fn pop_due(&mut self, now: Tick) -> Vec<CitizenId> {
        let mut due = Vec::new();
        while let Some(&(at, id)) = self.entries.first() {
            if at > now {
                break;
            }
            self.entries.pop_first();
            due.push(id);
        }
        due
    }
}

impl StateHash for DepartureQueue {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.entries);
    }
}

impl Population {
    /// 組 `bucket` の住民を更新する。
    ///
    /// `f` はいまの行だけを読んで新しい値を返し、他の住民の行は読まない。各住民の `f` は並列に呼び、
    /// 結果を ID の順に集めてから 1 本のスレッドで書き戻すので、結果はスレッド数に左右されない。
    pub fn update_bucket<F>(&mut self, bucket: u32, f: F)
    where
        F: Fn(CitizenId, CitizenRow) -> Option<CitizenChange> + Sync,
    {
        let start = bucket as usize;
        let stride = BUCKET_COUNT as usize;
        let count = self
            .citizens
            .slots
            .len()
            .saturating_sub(start)
            .div_ceil(stride);
        let citizens = &self.citizens;
        let compute = |k: usize| {
            let slot = start + k * stride;
            let (index, generation) = citizens.slots.current(slot)?;
            let id = CitizenId { index, generation };
            f(id, citizens.row(slot)).map(|change| (id, change))
        };
        #[cfg(feature = "parallel")]
        let changes: Vec<_> = (0..count).into_par_iter().map(compute).collect();
        #[cfg(not(feature = "parallel"))]
        let changes: Vec<_> = (0..count).map(compute).collect();
        for (id, change) in changes.into_iter().flatten() {
            self.apply_change(id, change);
        }
    }

    /// `tick` の組を、標準の処理 [`update_citizen`] で更新する。
    pub fn step(&mut self, seed: u64, tick: Tick) {
        self.update_bucket(bucket_at(tick), |id, row| {
            update_citizen(seed, tick, id, row)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::citizens::{Activity, ScheduleState};
    use crate::rng::{RngPurpose, rng};
    use crate::units::Money;

    fn population(n: u32) -> Population {
        let mut p = Population::default();
        let mut h = p.add_household(Money::ZERO);
        for i in 0..n {
            if i % 3 == 0 {
                h = p.add_household(Money(i64::from(i)));
            }
            p.add_citizen(h, (i % 90) as u8).unwrap();
        }
        p
    }

    /// 乱数で幸福度と出発を変える更新。並列に回したときの決定論を見るために使う。
    fn noisy(
        seed: u64,
        tick: Tick,
    ) -> impl Fn(CitizenId, CitizenRow) -> Option<CitizenChange> + Sync {
        move |id, row| {
            let key = (u64::from(id.index) << 16) | u64::from(id.generation);
            let mut r = rng(seed, RngPurpose::DailySchedule, key, tick);
            if r.chance(1, 4) {
                return None;
            }
            let mut happiness = row.happiness;
            happiness.commute = r.below(101) as u8;
            let depart_at = r.chance(1, 2).then(|| tick + 1 + u64::from(r.below(200)));
            Some(CitizenChange {
                schedule: ScheduleState {
                    next: Activity::Work,
                    depart_at,
                },
                happiness,
            })
        }
    }

    fn run(p: &mut Population, ticks: u64) {
        for t in 0..ticks {
            let tick = Tick(t);
            p.update_bucket(bucket_at(tick), noisy(7, tick));
            if t % 10 == 0 {
                p.pop_departures(tick);
            }
        }
    }

    #[test]
    fn each_citizen_is_updated_once_every_bucket_count_ticks() {
        let mut p = population(1_000);
        let bump = |_: CitizenId, row: CitizenRow| {
            let mut happiness = row.happiness;
            happiness.services += 1;
            Some(CitizenChange {
                schedule: row.schedule,
                happiness,
            })
        };
        for t in 0..u64::from(BUCKET_COUNT) {
            p.update_bucket(bucket_at(Tick(t)), bump);
        }
        let ids: Vec<_> = p.citizen_ids().collect();
        assert_eq!(ids.len(), 1_000);
        for id in ids {
            assert_eq!(p.citizen(id).unwrap().happiness.services, 51, "{id:?}");
        }
        assert_eq!(bucket_at(Tick(64 * 5 + 3)), 3);
        assert_eq!(
            bucket_of(CitizenId {
                index: 64 * 9 + 3,
                generation: 2
            }),
            3
        );
    }

    #[test]
    fn departures_follow_the_schedule() {
        let mut p = population(3_000);
        run(&mut p, 300);
        let queued: Vec<_> = p.departures().iter().collect();
        assert!(!queued.is_empty());
        for (at, id) in &queued {
            assert_eq!(p.citizen(*id).unwrap().schedule.depart_at, Some(*at));
        }
        let scheduled = p
            .citizen_ids()
            .filter(|id| p.citizen(*id).unwrap().schedule.depart_at.is_some())
            .count();
        assert_eq!(scheduled, queued.len());
        assert!(queued.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn golden_hash_after_updates() {
        let mut p = population(2_000);
        run(&mut p, 128);
        assert_eq!(
            crate::state_hash::state_hash_of(&p),
            11_377_169_248_151_745_310
        );
    }

    #[cfg(feature = "parallel")]
    #[test]
    fn result_does_not_depend_on_the_thread_count() {
        let hash_with = |threads: usize| {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap();
            pool.install(|| {
                let mut p = population(20_000);
                run(&mut p, 200);
                crate::state_hash::state_hash_of(&p)
            })
        };
        let one = hash_with(1);
        assert_eq!(one, hash_with(8));
        assert_eq!(one, hash_with(3));
        assert_ne!(one, crate::state_hash::state_hash_of(&population(20_000)));
    }
}
