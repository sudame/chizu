//! 住民の組ごとの更新と、出発の待ち行列。
//!
//! 住民は ID の添え字の連続した [`BLOCK_SIZE`] 人ずつを塊とし、塊の番号を [`BUCKET_COUNT`] で割った余りで
//! 組に分ける。tick を同じ数で割った余りの組をその tick に更新する。1 組の住民が塊ごとに列の上で
//! 連続するので、キャッシュラインを無駄に読まない。添え字は使い回しても変わらないので、住民の組は
//! 生きているあいだ変わらない。

use std::collections::BTreeSet;

#[cfg(feature = "parallel")]
use rayon::prelude::*;

use crate::citizens::{CitizenChange, CitizenRow, Population};
use crate::id::CitizenId;
use crate::state_hash::{StateHash, StateHasher};
use crate::units::Tick;

/// 住民を分ける組の数。等速（30 tick/秒）で、1 人あたり約 2 秒に 1 回の更新になる。
pub const BUCKET_COUNT: u32 = 64;

/// 組に分けるときの塊の人数。
pub const BLOCK_SIZE: u32 = 64;

/// 並列に分けたときに 1 本のスレッドが受け持つ塊の数の下限。塊がこの 2 倍に満たない組は
/// 1 本のスレッドで回す。1 人の更新は数十ナノ秒なので、これより小さく分けると、スレッドを起こして
/// 待ち合わせる時間のほうが長くなる。
///
/// 分け方で結果は変わらない。単体テストでは、少ない人数でも分けた道を通してスレッド数に依らないことを
/// 確かめられるように 1 にする。
#[cfg(feature = "parallel")]
const PAR_MIN_BLOCKS: usize = if cfg!(test) { 1 } else { 32 };

/// `tick` に更新する組。
pub fn bucket_at(tick: Tick) -> u32 {
    (tick.0 % u64::from(BUCKET_COUNT)) as u32
}

/// 住民の属する組。
pub fn bucket_of(id: CitizenId) -> u32 {
    id.index / BLOCK_SIZE % BUCKET_COUNT
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
    /// 組 `bucket` の住民を更新する。`bucket` が [`BUCKET_COUNT`] 以上なら panic する。
    ///
    /// `f` は表といまの行を読んで新しい値を返す。書き戻すのは全員の `f` が済んでからなので、`f` が読む表は
    /// 組の中の誰の更新にも左右されない。各住民の `f` は並列に呼び、
    /// 結果を ID の順に集めてから 1 本のスレッドで書き戻すので、結果はスレッド数に左右されない。
    pub fn update_bucket<F>(&mut self, bucket: u32, f: F)
    where
        F: Fn(&Population, CitizenId, CitizenRow) -> Option<CitizenChange> + Sync,
    {
        assert!(bucket < BUCKET_COUNT, "組の番号が範囲の外: {bucket}");
        let len = self.citizens.slots.len();
        let block = BLOCK_SIZE as usize;
        let stride = BUCKET_COUNT as usize;
        let first = bucket as usize;
        let blocks = len.div_ceil(block).saturating_sub(first).div_ceil(stride);
        let this = &*self;
        let citizens = &self.citizens;
        let block_slots = |k: usize| {
            let start = (first + k * stride) * block;
            start..(start + block).min(len)
        };
        let compute = |slot: usize| {
            let (index, generation) = citizens.slots.current(slot)?;
            let id = CitizenId { index, generation };
            f(this, id, citizens.row(slot)).map(|change| (id, change))
        };
        // 並列の collect は、変更のあった住民だけを元の順番（ID の順）のまま集める。
        #[cfg(feature = "parallel")]
        let changes: Vec<_> = (0..blocks)
            .into_par_iter()
            .with_min_len(PAR_MIN_BLOCKS)
            .flat_map_iter(|k| block_slots(k).filter_map(compute))
            .collect();
        #[cfg(not(feature = "parallel"))]
        let changes: Vec<_> = (0..blocks)
            .flat_map(|k| block_slots(k).filter_map(compute))
            .collect();
        for (id, change) in changes {
            self.apply_change(id, change);
        }
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
    ) -> impl Fn(&Population, CitizenId, CitizenRow) -> Option<CitizenChange> + Sync {
        move |_, id, row| {
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
        // 塊が 64 を超えて、組 0 に 2 つ目の塊が入る人数にする。
        let mut p = population(5_000);
        let bump = |_: &Population, _: CitizenId, row: CitizenRow| {
            let mut happiness = row.happiness;
            happiness.services += 1;
            Some(CitizenChange {
                schedule: row.schedule,
                happiness,
            })
        };
        for b in 0..BUCKET_COUNT {
            p.update_bucket(b, |p, id, row| {
                assert_eq!(bucket_of(id), b);
                bump(p, id, row)
            });
        }
        let ids: Vec<_> = p.citizen_ids().collect();
        assert_eq!(ids.len(), 5_000);
        for id in ids {
            assert_eq!(p.citizen(id).unwrap().happiness.services, 51, "{id:?}");
        }
        assert_eq!(bucket_at(Tick(64 * 5 + 3)), 3);
        assert_eq!(
            bucket_of(CitizenId {
                index: 64 * (64 * 2 + 9) + 3,
                generation: 2
            }),
            9
        );
    }

    #[test]
    fn small_tables_and_vacant_rows_are_updated_exactly_once() {
        for n in [0_u32, 1, 63, 64, 65, 130, 4_095, 4_096, 4_097, 9_000] {
            let mut p = population(n);
            // 消した行は飛ばす。
            let removed: Vec<_> = p.citizen_ids().step_by(5).collect();
            for id in &removed {
                p.remove_citizen(*id);
            }
            let calls = std::sync::atomic::AtomicU32::new(0);
            for b in 0..BUCKET_COUNT {
                p.update_bucket(b, |_, _, _| {
                    calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    None
                });
            }
            assert_eq!(calls.into_inner() as usize, p.citizen_count(), "{n} 人");
        }
    }

    #[test]
    #[should_panic(expected = "範囲の外")]
    fn out_of_range_bucket_panics() {
        population(100).update_bucket(BUCKET_COUNT, |_, _, _| None);
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
    }

    #[test]
    fn golden_hash_after_updates() {
        let mut p = population(2_000);
        run(&mut p, 128);
        assert_eq!(
            crate::state_hash::state_hash_of(&p),
            10_934_895_077_669_233_281
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

    #[test]
    fn world_step_updates_the_bucket_of_the_tick() {
        let mut w = crate::world::World::new(3);
        *w.population_mut() = population(10_000);
        for _ in 0..5 {
            w.step_with(|_, _, _| None);
        }
        let bump = |ctx: &crate::happiness::CitizenContext, _: CitizenId, row: CitizenRow| {
            let mut happiness = row.happiness;
            happiness.wealth = (ctx.tick.0 % 100) as u8;
            Some(CitizenChange {
                schedule: row.schedule,
                happiness,
            })
        };
        w.step_with(bump);
        assert_eq!(w.tick(), Tick(6));
        let p = w.population();
        for id in p.citizen_ids() {
            let wealth = p.citizen(id).unwrap().happiness.wealth;
            if bucket_of(id) == bucket_at(Tick(5)) {
                assert_eq!(wealth, 5, "{id:?}");
            } else {
                assert_eq!(wealth, crate::citizens::HappinessParts::NEUTRAL.wealth);
            }
        }
    }

    #[test]
    fn world_state_hash_includes_the_population() {
        let mut w = crate::world::World::new(3);
        let empty = w.state_hash();
        *w.population_mut() = population(100);
        let filled = w.state_hash();
        assert_ne!(filled, empty);
        let id = w.population().citizen_ids().nth(50).unwrap();
        w.population_mut().set_schedule(
            id,
            ScheduleState {
                next: Activity::Shop,
                depart_at: None,
            },
        );
        assert_ne!(w.state_hash(), filled);
    }
}
