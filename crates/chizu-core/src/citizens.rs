//! 住民データ層の表。
//!
//! 住民と世帯は列ごとの `Vec`（SoA）で持ち、行の添え字は [`crate::id::Slots`] で割り当てる。
//! 消した行の列は [`CitizenRow::VACANT`] などの決まった値に戻すので、状態ハッシュは列をそのままなめればよい。

use smallvec::SmallVec;

use crate::id::{BuildingId, CitizenId, HouseholdId, Slots, TripId};
use crate::rng::Pcg32;
use crate::schedule::DepartureQueue;
use crate::state_hash::{StateHash, StateHasher};
use crate::units::{Money, Tick};

/// 1 世帯の住民の数の上限。
pub const MAX_HOUSEHOLD_SIZE: u32 = 4;

/// 働く年齢の範囲（歳）。
pub const WORKING_AGES: core::ops::RangeInclusive<u8> = 18..=64;

/// 1 つの世帯の所持金と住民の年齢を引く。人数は 1〜[`MAX_HOUSEHOLD_SIZE`] 人を引いてから `limit` で削る。
/// 最初の 1 人は 18〜79 歳、残りは 0〜79 歳である。引く順番は人数、所持金、年齢の順に決めてある。
pub fn draw_household(r: &mut Pcg32, limit: u32) -> (Money, SmallVec<[u8; 4]>) {
    let size = (1 + r.below(MAX_HOUSEHOLD_SIZE)).min(limit);
    let money = Money(i64::from(r.below(100_000)));
    let ages = (0..size)
        .map(|i| if i == 0 { 18 + r.below(62) } else { r.below(80) } as u8)
        .collect();
    (money, ages)
}

/// 幸福度の要素の点数の上限。点数は `0..=HAPPINESS_MAX` である。
pub const HAPPINESS_MAX: u8 = 100;

/// 住民が次に向かう先。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum Activity {
    #[default]
    Home = 0,
    /// 職場または学校。
    Work = 1,
    Shop = 2,
}

/// 住民の予定の状態。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScheduleState {
    pub next: Activity,
    /// 出発を待っているなら、出発する tick。出発の待ち行列にも同じ値が入っている。
    pub depart_at: Option<Tick>,
}

/// 幸福度の 3 つの要素。重みを付けて足すのは世帯の属性が要るので、ここでは要素の点数だけを持つ。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HappinessParts {
    pub commute: u8,
    pub services: u8,
    pub wealth: u8,
}

impl HappinessParts {
    pub const NEUTRAL: HappinessParts = HappinessParts {
        commute: HAPPINESS_MAX / 2,
        services: HAPPINESS_MAX / 2,
        wealth: HAPPINESS_MAX / 2,
    };
}

/// 住民 1 人ぶんの列の値。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CitizenRow {
    pub household: HouseholdId,
    /// 年齢（歳）。
    pub age: u8,
    pub workplace: Option<BuildingId>,
    pub schedule: ScheduleState,
    pub happiness: HappinessParts,
    pub trip: Option<TripId>,
}

impl CitizenRow {
    /// 消した行に入れておく値。
    pub const VACANT: CitizenRow = CitizenRow {
        household: HouseholdId {
            index: u32::MAX,
            generation: 0,
        },
        age: 0,
        workplace: None,
        schedule: ScheduleState {
            next: Activity::Home,
            depart_at: None,
        },
        happiness: HappinessParts::NEUTRAL,
        trip: None,
    };
}

/// 組ごとの更新が 1 人の住民に書き戻す値。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CitizenChange {
    pub schedule: ScheduleState,
    pub happiness: HappinessParts,
}

/// 住民の表。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Citizens {
    pub(crate) slots: Slots,
    household: Vec<HouseholdId>,
    age: Vec<u8>,
    workplace: Vec<Option<BuildingId>>,
    schedule: Vec<ScheduleState>,
    happiness: Vec<HappinessParts>,
    trip: Vec<Option<TripId>>,
}

impl Citizens {
    pub(crate) fn row(&self, slot: usize) -> CitizenRow {
        CitizenRow {
            household: self.household[slot],
            age: self.age[slot],
            workplace: self.workplace[slot],
            schedule: self.schedule[slot],
            happiness: self.happiness[slot],
            trip: self.trip[slot],
        }
    }

    fn set_row(&mut self, slot: usize, row: CitizenRow) {
        self.household[slot] = row.household;
        self.age[slot] = row.age;
        self.workplace[slot] = row.workplace;
        self.schedule[slot] = row.schedule;
        self.happiness[slot] = row.happiness;
        self.trip[slot] = row.trip;
    }

    fn insert(&mut self, row: CitizenRow) -> CitizenId {
        let ((index, generation), grew) = self.slots.alloc();
        if grew {
            self.household.push(row.household);
            self.age.push(row.age);
            self.workplace.push(row.workplace);
            self.schedule.push(row.schedule);
            self.happiness.push(row.happiness);
            self.trip.push(row.trip);
        } else {
            self.set_row(index as usize, row);
        }
        CitizenId { index, generation }
    }
}

/// 世帯の表。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Households {
    slots: Slots,
    /// 住んでいる建物。建物の表ができるまでは、どの世帯も `None` である。
    home: Vec<Option<BuildingId>>,
    money: Vec<Money>,
    /// 世帯の住民。加わった順に並ぶ。
    members: Vec<SmallVec<[CitizenId; 4]>>,
}

/// 住民データ層。住民と世帯の表と、出発の待ち行列をまとめて持ち、互いの整合を保つ。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Population {
    pub(crate) citizens: Citizens,
    households: Households,
    pub(crate) departures: DepartureQueue,
}

impl Population {
    pub fn citizen_count(&self) -> usize {
        self.citizens.slots.live_count()
    }

    pub fn household_count(&self) -> usize {
        self.households.slots.live_count()
    }

    pub fn contains_citizen(&self, id: CitizenId) -> bool {
        self.citizens.slots.contains(id.index, id.generation)
    }

    pub fn contains_household(&self, id: HouseholdId) -> bool {
        self.households.slots.contains(id.index, id.generation)
    }

    pub fn citizen(&self, id: CitizenId) -> Option<CitizenRow> {
        self.contains_citizen(id)
            .then(|| self.citizens.row(id.slot()))
    }

    /// いる住民の ID を ID の順に返す。
    pub fn citizen_ids(&self) -> impl Iterator<Item = CitizenId> + '_ {
        let slots = &self.citizens.slots;
        (0..slots.len()).filter_map(|slot| {
            let (index, generation) = slots.current(slot)?;
            Some(CitizenId { index, generation })
        })
    }

    /// いる世帯の ID を ID の順に返す。
    pub fn household_ids(&self) -> impl Iterator<Item = HouseholdId> + '_ {
        let slots = &self.households.slots;
        (0..slots.len()).filter_map(|slot| {
            let (index, generation) = slots.current(slot)?;
            Some(HouseholdId { index, generation })
        })
    }

    /// 世帯の住む家を決める。世帯がなければ何もしない。
    pub(crate) fn set_home(&mut self, id: HouseholdId, home: Option<BuildingId>) {
        if self.contains_household(id) {
            self.households.home[id.slot()] = home;
        }
    }

    /// 住民の職場を決める。住民がいなければ何もしない。
    pub(crate) fn set_workplace(&mut self, id: CitizenId, workplace: Option<BuildingId>) {
        if self.contains_citizen(id) {
            self.citizens.workplace[id.slot()] = workplace;
        }
    }

    pub fn members(&self, id: HouseholdId) -> Option<&[CitizenId]> {
        self.contains_household(id)
            .then(|| self.households.members[id.slot()].as_slice())
    }

    pub fn household_money(&self, id: HouseholdId) -> Option<Money> {
        self.contains_household(id)
            .then(|| self.households.money[id.slot()])
    }

    pub fn home(&self, id: HouseholdId) -> Option<Option<BuildingId>> {
        self.contains_household(id)
            .then(|| self.households.home[id.slot()])
    }

    pub fn departures(&self) -> &DepartureQueue {
        &self.departures
    }

    /// 住む家のまだない世帯を足す。
    pub fn add_household(&mut self, money: Money) -> HouseholdId {
        let h = &mut self.households;
        let ((index, generation), grew) = h.slots.alloc();
        if grew {
            h.home.push(None);
            h.money.push(money);
            h.members.push(SmallVec::new());
        } else {
            h.money[index as usize] = money;
        }
        HouseholdId { index, generation }
    }

    /// 世帯を、その住民ごと消す。
    pub fn remove_household(&mut self, id: HouseholdId) -> bool {
        if !self.contains_household(id) {
            return false;
        }
        let slot = id.slot();
        for member in core::mem::take(&mut self.households.members[slot]) {
            self.remove_citizen(member);
        }
        let h = &mut self.households;
        h.slots.release(id.index, id.generation);
        h.home[slot] = None;
        h.money[slot] = Money::ZERO;
        true
    }

    /// 世帯に住民を 1 人足す。世帯がなければ `None` を返す。
    pub fn add_citizen(&mut self, household: HouseholdId, age: u8) -> Option<CitizenId> {
        if !self.contains_household(household) {
            return None;
        }
        let id = self.citizens.insert(CitizenRow {
            household,
            age,
            ..CitizenRow::VACANT
        });
        self.households.members[household.slot()].push(id);
        Some(id)
    }

    /// 住民を消す。世帯の住民の一覧と、出発の待ち行列からも外す。世帯は残す。
    pub fn remove_citizen(&mut self, id: CitizenId) -> bool {
        let Some(row) = self.citizen(id) else {
            return false;
        };
        if let Some(t) = row.schedule.depart_at {
            self.departures.remove(t, id);
        }
        if self.contains_household(row.household) {
            self.households.members[row.household.slot()].retain(|m| *m != id);
        }
        self.citizens.slots.release(id.index, id.generation);
        self.citizens.set_row(id.slot(), CitizenRow::VACANT);
        true
    }

    /// 住民の予定を変え、出発の待ち行列を合わせる。住民がいなければ `false` を返す。
    pub fn set_schedule(&mut self, id: CitizenId, schedule: ScheduleState) -> bool {
        let Some(row) = self.citizen(id) else {
            return false;
        };
        if row.schedule.depart_at != schedule.depart_at {
            if let Some(t) = row.schedule.depart_at {
                self.departures.remove(t, id);
            }
            if let Some(t) = schedule.depart_at {
                self.departures.insert(t, id);
            }
        }
        self.citizens.schedule[id.slot()] = schedule;
        true
    }

    #[track_caller]
    pub(crate) fn apply_change(&mut self, id: CitizenId, change: CitizenChange) {
        let h = change.happiness;
        assert!(
            h.commute <= HAPPINESS_MAX && h.services <= HAPPINESS_MAX && h.wealth <= HAPPINESS_MAX,
            "幸福度の点数が上限を超えた: {h:?}"
        );
        self.set_schedule(id, change.schedule);
        self.citizens.happiness[id.slot()] = change.happiness;
    }

    /// `now` までに出発する住民を、出発の tick の順（同じ tick は ID の順）に取り出す。
    /// 取り出した住民の予定からは出発の tick を消す。
    pub fn pop_departures(&mut self, now: Tick) -> Vec<CitizenId> {
        let due = self.departures.pop_due(now);
        for id in &due {
            self.citizens.schedule[id.slot()].depart_at = None;
        }
        due
    }
}

impl StateHash for Activity {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&(*self as u8));
    }
}

impl StateHash for ScheduleState {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.next);
        h.write(&self.depart_at);
    }
}

impl StateHash for HappinessParts {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&[self.commute, self.services, self.wealth]);
    }
}

impl StateHash for Citizens {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.slots);
        h.write(&self.household);
        h.write(&self.age);
        h.write(&self.workplace);
        h.write(&self.schedule);
        h.write(&self.happiness);
        h.write(&self.trip);
    }
}

impl StateHash for Households {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.slots);
        h.write(&self.home);
        h.write(&self.money);
        h.write_len(self.members.len());
        for m in &self.members {
            h.write(m.as_slice());
        }
    }
}

impl StateHash for Population {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.citizens);
        h.write(&self.households);
        h.write(&self.departures);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state_hash::state_hash_of;

    #[test]
    fn add_and_remove_keep_households_and_departures_consistent() {
        let mut p = Population::default();
        let h = p.add_household(Money(1_000));
        let a = p.add_citizen(h, 30).unwrap();
        let b = p.add_citizen(h, 5).unwrap();
        assert_eq!(p.members(h), Some(&[a, b][..]));
        assert_eq!(p.citizen(b).unwrap().age, 5);
        assert_eq!(p.citizen(b).unwrap().household, h);

        let leave = ScheduleState {
            next: Activity::Work,
            depart_at: Some(Tick(10)),
        };
        assert!(p.set_schedule(a, leave));
        assert_eq!(p.departures().len(), 1);
        assert!(p.remove_citizen(a));
        assert!(!p.remove_citizen(a));
        assert_eq!(p.citizen(a), None);
        assert_eq!(p.departures().len(), 0);
        assert_eq!(p.members(h), Some(&[b][..]));

        // 消した住民の添え字は、世代を上げて使い回す。
        let c = p.add_citizen(h, 40).unwrap();
        assert_eq!((c.index, c.generation), (a.index, a.generation + 1));
        assert_eq!(p.citizen_ids().collect::<Vec<_>>(), [c, b]);

        assert!(p.remove_household(h));
        assert_eq!((p.citizen_count(), p.household_count()), (0, 0));
        assert_eq!(p.add_citizen(h, 1), None);
    }

    #[test]
    fn the_same_operations_give_the_same_hash() {
        let build = || {
            let mut p = Population::default();
            let h = p.add_household(Money(5));
            let a = p.add_citizen(h, 1).unwrap();
            p.add_citizen(h, 2).unwrap();
            p.remove_citizen(a);
            p
        };
        let mut other = build();
        assert_eq!(state_hash_of(&build()), state_hash_of(&other));
        let before = state_hash_of(&other);
        other.add_household(Money(5));
        assert_ne!(state_hash_of(&other), before);
    }

    #[test]
    fn popping_departures_clears_the_departure_tick() {
        let mut p = Population::default();
        let h = p.add_household(Money::ZERO);
        let ids: Vec<_> = (0..3).map(|_| p.add_citizen(h, 20).unwrap()).collect();
        for (id, t) in ids.iter().zip([7, 5, 5]).rev() {
            p.set_schedule(
                *id,
                ScheduleState {
                    next: Activity::Shop,
                    depart_at: Some(Tick(t)),
                },
            );
        }
        assert_eq!(p.pop_departures(Tick(6)), [ids[1], ids[2]]);
        assert_eq!(p.citizen(ids[1]).unwrap().schedule.depart_at, None);
        assert_eq!(p.pop_departures(Tick(7)), [ids[0]]);
        assert!(p.departures().is_empty());
    }

    #[test]
    fn reused_household_starts_empty_and_its_removal_clears_departures() {
        let mut p = Population::default();
        let h = p.add_household(Money(9));
        let a = p.add_citizen(h, 30).unwrap();
        p.set_schedule(
            a,
            ScheduleState {
                next: Activity::Work,
                depart_at: Some(Tick(3)),
            },
        );
        p.households.home[h.slot()] = Some(BuildingId {
            index: 1,
            generation: 0,
        });
        assert!(p.remove_household(h));
        assert!(p.departures().is_empty());
        assert_eq!(p.citizen(a), None);

        let g = p.add_household(Money(4));
        assert_eq!((g.index, g.generation), (h.index, h.generation + 1));
        assert_eq!(p.members(g), Some(&[][..]));
        assert_eq!(p.home(g), Some(None));
        assert_eq!(p.household_money(g), Some(Money(4)));
        assert_eq!(p.members(h), None);
    }
}
