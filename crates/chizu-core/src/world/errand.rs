//! 買い物の外出。
//!
//! CS2 にならい、住民はときどき家の近くの商業の建物へ買い物に行き、短くいて帰る。
//!
//! - 働く住民：職場に着いたとき、[`WORKER_SHOP_CHANCE`] の確率で、夕方の帰りに店へ寄る予定にする（職場 → 店 → 家）。
//! - 働いていない住民：毎朝 [`PLAN_AT`] から [`PLAN_SPREAD`] のあいだに、住民の添え字で散らして 1 人ずつ見る。
//!   家にいて予定のない住民は [`IDLE_SHOP_CHANCE`] の確率で、昼のあいだに店へ出かける予定にする（家 → 店 → 家）。
//!
//! 店は家のまわり [`SHOP_SEARCH_TILES`] の範囲にある商業の建物のうち、近い [`SHOP_CANDIDATES`] 件から選び、
//! 近いほど選ばれやすくする。店には [`STAY_MIN`] から [`STAY_MIN`] ＋ [`STAY_SPREAD`] のあいだいる。
//! 移動は通勤と同じもの（`world::commute`）を使う。
//!
//! 行く先の店が消えたら、店へ向かう移動をやめさせて家に着いたものとし、店へ出るのを待っている住民は寄らずに帰る。
//! 職場の割り当てや建物の消滅で予定が立て直され、買い物の予定と合わなくなった住民の買い物は消す。

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use super::World;
use crate::citizens::{Activity, CitizenRow, ScheduleState};
use crate::clock::TICKS_PER_DAY;
use crate::command::ZoneKind;
use crate::id::{BuildingId, CitizenId};
use crate::rng::{Pcg32, RngPurpose, rng};
use crate::state_hash::{StateHash, StateHasher};
use crate::tile::{TilePos, TileRect};
use crate::units::Tick;

/// 働く住民が、仕事の帰りに店へ寄る確率（分子, 分母）。
pub const WORKER_SHOP_CHANCE: (u32, u32) = (1, 4);

/// 働いていない住民が、その日の昼に買い物へ出る確率（分子, 分母）。
pub const IDLE_SHOP_CHANCE: (u32, u32) = (1, 2);

/// 働いていない住民の買い物の予定を立て始める時刻（1 日の中の tick）。6 時である。
pub const PLAN_AT: u64 = TICKS_PER_DAY / 4;

/// 予定を立てる住民を散らす幅（tick）。3 時間で、添え字をこの数で割った余りの tick にその住民を見る。
pub const PLAN_SPREAD: u64 = TICKS_PER_DAY / 8;

/// 働いていない住民が買い物へ出る時刻の早いほうの端（1 日の中の tick）。9 時で、予定を立て終えたあとである。
pub const IDLE_LEAVE_FROM: u64 = TICKS_PER_DAY * 9 / 24;

/// 働いていない住民が買い物へ出る時刻の幅（tick）。8 時間で、17 時までに出る。
pub const IDLE_LEAVE_SPREAD: u32 = (TICKS_PER_DAY * 8 / 24) as u32;

/// 店にいる時間の下限（tick）。30 分である。
pub const STAY_MIN: u64 = TICKS_PER_DAY / 48;

/// 店にいる時間の幅（tick）。30 分で、長くても 1 時間でたつ。
pub const STAY_SPREAD: u32 = (TICKS_PER_DAY / 48) as u32;

/// 行き先の店を選ぶ候補の数。家から近い順にこの数だけ取り、近いほど重くする。
pub const SHOP_CANDIDATES: usize = 8;

/// 店を探す、家の正面のマスからの範囲（マス）。狭いほうで見つからなければ広いほうで探す。
pub const SHOP_SEARCH_TILES: [i64; 2] = [32, 128];

/// 買い物に出る住民の行き先。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Errand {
    shop: BuildingId,
    /// 店に着いて、家へ帰るのを待っているか。
    at_shop: bool,
}

impl StateHash for Errand {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.shop);
        h.write(&self.at_shop);
    }
}

/// 買い物に出る住民ごとの行き先。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Errands {
    by_citizen: BTreeMap<CitizenId, Errand>,
}

impl StateHash for Errands {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.by_citizen);
    }
}

impl Errands {
    /// 住民 `id` が向かう店。
    pub(super) fn shop(&self, id: CitizenId) -> Option<BuildingId> {
        self.by_citizen.get(&id).map(|e| e.shop)
    }

    /// 住民 `id` が店にいるなら、その店。
    pub(super) fn at_shop(&self, id: CitizenId) -> Option<BuildingId> {
        self.by_citizen
            .get(&id)
            .filter(|e| e.at_shop)
            .map(|e| e.shop)
    }

    pub(super) fn forget(&mut self, id: CitizenId) {
        self.by_citizen.remove(&id);
    }
}

/// 2 つのマスの距離の 2 乗。
fn dist_sq(a: TilePos, b: TilePos) -> i128 {
    let d = |p: i64, q: i64| i128::from(p.checked_sub(q).expect("距離があふれた"));
    let (x, y) = (d(a.x, b.x), d(a.y, b.y));
    x.checked_mul(x)
        .zip(y.checked_mul(y))
        .and_then(|(x, y)| x.checked_add(y))
        .expect("距離があふれた")
}

/// `shops` のうち `from` に近い [`SHOP_CANDIDATES`] 件から、近いほど選ばれやすく 1 件を選ぶ。
/// 近い順に [`SHOP_CANDIDATES`]、…、1 の重みを付ける。距離が同じなら建物の ID の小さいほうを近いとする。
fn pick_shop(shops: &[(BuildingId, TilePos)], from: TilePos, r: &mut Pcg32) -> Option<BuildingId> {
    let mut near: Vec<(i128, BuildingId)> = Vec::with_capacity(SHOP_CANDIDATES + 1);
    for (id, at) in shops {
        let key = (dist_sq(from, *at), *id);
        let pos = near.partition_point(|k| *k < key);
        if pos < SHOP_CANDIDATES {
            near.insert(pos, key);
            near.truncate(SHOP_CANDIDATES);
        }
    }
    let n = u32::try_from(near.len()).expect("候補の数があふれた");
    if n == 0 {
        return None;
    }
    let total = n.checked_mul(n.checked_add(1)?)? / 2;
    let mut roll = r.below(total);
    for (rank, (_, id)) in (0_u32..).zip(&near) {
        let weight = n.checked_sub(rank)?;
        if roll < weight {
            return Some(*id);
        }
        roll = roll.checked_sub(weight)?;
    }
    None
}

impl World {
    /// 住民と、向かう店か、いる店。住民の ID の順に並ぶ。
    pub fn errands(&self) -> impl Iterator<Item = (CitizenId, BuildingId)> + '_ {
        self.errands.by_citizen.iter().map(|(id, e)| (*id, e.shop))
    }

    /// 住民 `row` の家のまわりの店から 1 件を選ぶ。
    fn shop_near_home(&self, row: &CitizenRow, r: &mut Pcg32) -> Option<BuildingId> {
        let home = self.population.home(row.household).flatten()?;
        let at = self.buildings.get(home)?.front;
        for reach in SHOP_SEARCH_TILES {
            let corner = |s: i64| {
                let x = at.x.checked_add(s.checked_mul(reach)?)?;
                let y = at.y.checked_add(s.checked_mul(reach)?)?;
                Some(TilePos::new(x, y))
            };
            let area = TileRect::from_corners(corner(-1)?, corner(1)?);
            let shops: Vec<(BuildingId, TilePos)> = self
                .buildings
                .fronts_in(area)
                .filter_map(|id| {
                    let b = self.buildings.get(id)?;
                    (b.zone == ZoneKind::Commercial).then_some((id, b.front))
                })
                .collect();
            if !shops.is_empty() {
                return pick_shop(&shops, at, r);
            }
        }
        None
    }

    fn errand_rng(&self, id: CitizenId) -> Pcg32 {
        rng(
            self.seed,
            RngPurpose::Errand,
            u64::from(id.index),
            self.tick,
        )
    }

    /// 行き先に着いた住民 `id` の次の予定。`planned` は通勤だけを見て立てた予定である。
    /// 店に着いたら、しばらくして家へ向かう。家に着いたら買い物を終える。
    /// 職場に着いたら、ときどき帰りに店へ寄る予定にする。
    pub(super) fn plan_errand(
        &mut self,
        id: CitizenId,
        row: &CitizenRow,
        planned: ScheduleState,
    ) -> ScheduleState {
        match row.schedule.next {
            Activity::Shop => {
                let stay = u64::from(self.errand_rng(id).below(STAY_SPREAD));
                let Some(e) = self.errands.by_citizen.get_mut(&id) else {
                    return planned;
                };
                e.at_shop = true;
                let at = self
                    .tick
                    .0
                    .checked_add(STAY_MIN)
                    .and_then(|t| t.checked_add(stay))
                    .expect("tick があふれた");
                ScheduleState {
                    next: Activity::Home,
                    depart_at: Some(Tick(at)),
                }
            }
            Activity::Home => {
                self.errands.forget(id);
                planned
            }
            Activity::Work => {
                let mut r = self.errand_rng(id);
                let (num, den) = WORKER_SHOP_CHANCE;
                if planned.next != Activity::Home || !r.chance(num, den) {
                    return planned;
                }
                let Some(shop) = self.shop_near_home(row, &mut r) else {
                    return planned;
                };
                self.errands.by_citizen.insert(
                    id,
                    Errand {
                        shop,
                        at_shop: false,
                    },
                );
                ScheduleState {
                    next: Activity::Shop,
                    ..planned
                }
            }
        }
    }

    /// 朝の [`PLAN_AT`] から [`PLAN_SPREAD`] のあいだ、添え字をその幅で割った余りがいまの tick に当たる住民のうち、
    /// 家にいて予定のない、働いていない住民の買い物の予定を立てる。
    pub(super) fn plan_daytime_errands(&mut self) {
        let Some(offset) = (self.tick.0 % TICKS_PER_DAY).checked_sub(PLAN_AT) else {
            return;
        };
        if offset >= PLAN_SPREAD {
            return;
        }
        let day_start = self.tick.0 - self.tick.0 % TICKS_PER_DAY;
        let slots = &self.population.citizens.slots;
        let stride = usize::try_from(PLAN_SPREAD).expect("幅があふれた");
        let first = usize::try_from(offset).expect("添え字があふれた");
        let mut plans = Vec::new();
        for slot in (first..slots.len()).step_by(stride) {
            let Some((index, generation)) = slots.current(slot) else {
                continue;
            };
            let id = CitizenId { index, generation };
            let Some(row) = self.population.citizen(id) else {
                continue;
            };
            if row.workplace.is_some()
                || row.trip.is_some()
                || row.schedule != ScheduleState::default()
            {
                continue;
            }
            let mut r = self.errand_rng(id);
            let (num, den) = IDLE_SHOP_CHANCE;
            if !r.chance(num, den) {
                continue;
            }
            let Some(shop) = self.shop_near_home(&row, &mut r) else {
                continue;
            };
            let at = day_start
                .checked_add(IDLE_LEAVE_FROM)
                .and_then(|t| t.checked_add(u64::from(r.below(IDLE_LEAVE_SPREAD))))
                .expect("tick があふれた");
            plans.push((id, shop, Tick(at)));
        }
        for (id, shop, at) in plans {
            self.population.set_schedule(
                id,
                ScheduleState {
                    next: Activity::Shop,
                    depart_at: Some(at),
                },
            );
            self.errands.by_citizen.insert(
                id,
                Errand {
                    shop,
                    at_shop: false,
                },
            );
        }
    }

    /// 消えた建物 `removed` を行き先にしていた住民の買い物をやめさせ、予定と合わなくなった買い物を消す。
    ///
    /// 店へ向かって移動している住民は、移動をやめさせて家に着いたものとする。店へ出るのを待っている住民は、
    /// 寄らずに帰る（働く住民は職場から同じ時刻に家へ、働いていない住民は家にいるまま）。店で帰りを待っている
    /// 住民は家に着いたものとし、店から家へ向かっている住民はそのまま帰らせる。
    /// 判定は住民の予定と移動の向かう先だけで決めるので、同じ tick に家や職場が消えて予定が立て直されていても食い違わない。
    pub(super) fn vacate_shops(&mut self, removed: &[BuildingId]) {
        let removed: BTreeSet<BuildingId> = removed.iter().copied().collect();
        let lost: Vec<(CitizenId, Errand)> = self
            .errands
            .by_citizen
            .iter()
            .filter(|(_, e)| removed.contains(&e.shop))
            .map(|(id, e)| (*id, *e))
            .collect();
        let mut arrived = Vec::new();
        for (id, e) in lost {
            self.errands.forget(id);
            let Some(row) = self.population.citizen(id) else {
                continue;
            };
            let heading = row.trip.and_then(|t| self.trips.commuter(t)).map(|c| c.to);
            if heading == Some(Activity::Shop)
                && let Some(t) = row.trip
            {
                self.trips.cancel(t);
                self.population.set_trip(id, None);
            }
            let next = row.schedule.next;
            if next == Activity::Shop && heading.is_none() {
                let schedule = match row.workplace {
                    Some(_) => ScheduleState {
                        next: Activity::Home,
                        ..row.schedule
                    },
                    None => ScheduleState::default(),
                };
                self.population.set_schedule(id, schedule);
            } else if (next == Activity::Shop && heading == Some(Activity::Shop))
                || (next == Activity::Home && e.at_shop && heading.is_none())
            {
                self.population.set_schedule(
                    id,
                    ScheduleState {
                        next: Activity::Home,
                        depart_at: None,
                    },
                );
                arrived.push(id);
            }
        }
        self.switch_places(arrived);
        self.drop_stale_errands();
    }

    /// 予定と合わなくなった買い物を消す。店へ向かう買い物は、予定の次の行き先が店のときだけ残す。
    /// 店にいる買い物は、次の行き先が家で、帰りの出発を待っているか移動しているときだけ残す。
    pub(super) fn drop_stale_errands(&mut self) {
        let p = &self.population;
        self.errands.by_citizen.retain(|id, e| {
            p.citizen(*id)
                .is_some_and(|row| match (row.schedule.next, e.at_shop) {
                    (Activity::Shop, false) => true,
                    (Activity::Home, true) => {
                        row.schedule.depart_at.is_some() || row.trip.is_some()
                    }
                    _ => false,
                })
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shops(n: u32) -> Vec<(BuildingId, TilePos)> {
        (0..n)
            .map(|i| {
                let id = BuildingId {
                    index: i,
                    generation: 0,
                };
                (id, TilePos::new(i64::from(i) * 10, 0))
            })
            .collect()
    }

    #[test]
    fn nearer_shops_are_picked_more_often() {
        let shops = shops(20);
        let mut count = [0_u32; 20];
        for k in 0..4_000 {
            let mut r = rng(1, RngPurpose::Errand, k, Tick(0));
            let id = pick_shop(&shops, TilePos::new(0, 0), &mut r).unwrap();
            count[id.index as usize] += 1;
        }
        assert!(
            count[SHOP_CANDIDATES..].iter().all(|c| *c == 0),
            "{count:?}"
        );
        assert!(count[0] > count[3] && count[3] > count[7], "{count:?}");
        assert!(count[7] > 0, "{count:?}");
    }

    #[test]
    fn no_shop_no_errand() {
        let mut r = rng(1, RngPurpose::Errand, 0, Tick(0));
        assert_eq!(pick_shop(&[], TilePos::new(0, 0), &mut r), None);
        let one = shops(1);
        assert_eq!(pick_shop(&one, TilePos::new(5, 5), &mut r), Some(one[0].0));
    }
}

#[cfg(all(test, feature = "parallel"))]
mod thread_tests {
    use crate::citizens::Activity;
    use crate::clock::TICKS_PER_DAY;
    use crate::command::ZoneKind;
    use crate::world::people::tests::{city, run_days};

    /// 2 日目の正午まで回した街の状態ハッシュと、店へ向かう移動の延べの数。
    fn noon(threads: usize) -> (u64, usize) {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        pool.install(|| {
            let zones = [
                ZoneKind::Residential,
                ZoneKind::Commercial,
                ZoneKind::Industrial,
            ];
            let mut w = city(&zones);
            run_days(&mut w, 1);
            let mut shopping = 0;
            while w.tick().0 < TICKS_PER_DAY + TICKS_PER_DAY / 2 {
                w.step();
                let trips = w.trips();
                shopping += trips
                    .iter()
                    .filter(|(id, _)| trips.commuter(*id).is_some_and(|c| c.to == Activity::Shop))
                    .count();
            }
            (w.state_hash(), shopping)
        })
    }

    #[test]
    fn errands_do_not_depend_on_the_thread_count() {
        let (one, shopping) = noon(1);
        assert!(shopping > 0, "昼に誰も店へ向かわない");
        assert_eq!(noon(4).0, one);
        assert_eq!(noon(8).0, one);
    }
}
