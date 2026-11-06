//! 住民の予定と、通勤と買い物の移動のつなぎ。
//!
//! 出発の時刻が来た住民は、居場所をすぐには切り替えず、家と職場のあいだの移動を始める。
//! 移動が終わったら居場所を切り替え、次の出発を積む。移動を作れなかった住民は、その場で着いたものとする。
//! 網が変わって行けなくなり、移動が途中で消えた住民も、行き先に着いたものとする。
//! 移動のあいだに職場や家が変わって予定が立て直されていたら、着いても予定は変えない。
//! 家から職場に着いた移動は、かかった時間を住民の通勤の時間に入れ、幸福度の通勤の点に使う。

use super::World;
use crate::car_place::{CarPlace, CarPlaces, SHARED_LOT_RADIUS, fee_cap};
use crate::citizens::Activity;
use crate::id::{CitizenId, HouseholdId, TripId};
use crate::movement::{CommuteRequest, Commuter, LotId};
use crate::rng::{RngPurpose, mix_words};
use crate::tile::TilePos;
use crate::units::Money;

/// 車を持つ世帯の割合（百分率）。
pub const CAR_OWNERSHIP_PERCENT: u64 = 70;

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
                Activity::Work => {
                    self.errands.forget(id);
                    home.zip(row.workplace)
                }
                Activity::Home => self.errands.at_shop(id).or(row.workplace).zip(home),
                Activity::Shop => row.workplace.or(home).zip(self.errands.shop(id)),
            };
            match ends {
                Some((from, to)) => requests.push(CommuteRequest {
                    who: Commuter {
                        citizen: id,
                        household: row.household,
                        to: row.schedule.next,
                    },
                    from,
                    to,
                    home,
                    place: self.car_places.get(row.household),
                    owns_car: self.owns_car(row.household),
                }),
                None => stuck.push(id),
            }
        }
        let (net, router) = (&self.network, &mut self.router.router);
        let sites = (&self.buildings, self.facilities.as_slice());
        let trips = self
            .trips
            .start_commutes(net, router, sites, &requests, self.tick);
        let mut no_route = 0;
        for (r, trip) in requests.iter().zip(trips) {
            match trip {
                Some(t) => self.population.set_trip(r.who.citizen, Some(t)),
                None => {
                    no_route += 1;
                    stuck.push(r.who.citizen);
                }
            }
        }
        self.tally_no_route(no_route);
        stuck.sort_unstable();
        stuck
    }

    /// 世帯 `h` が車を持つか。種と世帯の ID だけで決まり、状態には持たない。
    fn owns_car(&self, h: HouseholdId) -> bool {
        let words = [u64::from(h.index), u64::from(h.generation)];
        mix_words(self.seed, RngPurpose::CarOwnership, &words) % 100 < CAR_OWNERSHIP_PERCENT
    }

    /// 世帯を街から出す。移動している住民の移動をやめさせ、停めてある車も駐車場から外す。
    pub(super) fn remove_household(&mut self, h: HouseholdId) {
        let members = self.population.members(h).unwrap_or_default().to_vec();
        for id in members {
            self.errands.forget(id);
            if let Some(t) = self.population.citizen(id).and_then(|r| r.trip) {
                self.trips.cancel(t);
            }
        }
        self.trips.forget_car(h);
        if let Some(CarPlace::Shared(_)) = self.car_places.remove(h) {
            self.sync_rented();
        }
        self.population.remove_household(h);
    }

    /// 世帯 `h` の枠のある駐車場。
    pub(super) fn own_lot(&self, h: HouseholdId) -> Option<LotId> {
        CarPlace::lot(self.car_places.get(h), self.population.home(h).flatten())
    }

    /// 共同の駐車場ごとの貸した枠の数を、世帯の枠に合わせる。世帯の枠を変えたあとに呼ぶ。
    pub(super) fn sync_rented(&mut self) {
        self.trips.set_rented(&self.car_places.rented_counts());
    }

    /// 世帯ごとの、車を置く枠。
    pub fn car_places(&self) -> &CarPlaces {
        &self.car_places
    }

    /// 自宅の枠を、いまの家と建物の台数に合わせて配り直し（[`CarPlaces::assign_homes`]）、
    /// 自宅の枠のない世帯に共同の駐車場の枠を貸す（[`World::rent_shared_lots`]）。
    /// 世帯が越してきたり、家をなくしたり、住宅の段階が上がったりしたあとに呼ぶ。
    /// 枠をなくした世帯と、借りた枠から自宅の枠に移った世帯の停めてある車は、駐車場から外す。
    pub(super) fn assign_car_places(&mut self) {
        let p = &self.population;
        let homes: Vec<_> = p
            .household_ids()
            .map(|h| (h, p.home(h).flatten()))
            .collect();
        let buildings = &self.buildings;
        let lost = self.car_places.assign_homes(homes, |b| {
            let spaces = buildings.parking_spaces(b);
            home_spaces_limit().map_or(spaces, |limit| spaces.min(limit))
        });
        for h in lost {
            self.trips.forget_car(h);
        }
        self.rent_shared_lots();
        self.sync_rented();
    }

    /// 自宅の枠のない、家のある世帯に、共同の駐車場の枠を世帯の ID の順に貸す。家の出入口のマスから
    /// [`SHARED_LOT_RADIUS`] マスの中の駐車場を、直線の距離の近い順（同じなら角のマスの順）に試し、
    /// 空きがあって料金が払える上限（[`fee_cap`]）以下なら借りる。どこも借りられなければ枠を持たない。
    /// 面した道路のない駐車場は貸さない。
    fn rent_shared_lots(&mut self) {
        let lots = self.shared_lot_sites();
        if lots.is_empty() {
            return;
        }
        let mut rented = self.car_places.rented_counts();
        let p = &self.population;
        let fee = i64::from(self.parking_fee);
        let mut renting = Vec::new();
        for h in p.household_ids() {
            if self.car_places.get(h).is_some() {
                continue;
            }
            let Some(home) = p.home(h).flatten().and_then(|b| self.buildings.get(b)) else {
                continue;
            };
            let savings = p.household_money(h).unwrap_or(Money::ZERO);
            if fee > fee_cap(savings) {
                continue;
            }
            let found = lots_in_range(&lots, home.front)
                .into_iter()
                .find(|(_, c, spaces)| {
                    let mut lot = self.trips.shared_lot(*c);
                    lot.rented = rented.get(c).copied().unwrap_or(0);
                    lot.has_room(*spaces)
                });
            if let Some((_, c, _)) = found {
                let n = rented.entry(c).or_insert(0);
                *n = n.checked_add(1).expect("貸した枠の数があふれた");
                renting.push((h, c));
            }
        }
        for (h, c) in renting {
            self.car_places.rent(h, c);
        }
    }

    /// 貸せる共同の駐車場。駐車場ごとに、角のマス、正面のマス、台数を施設を置いた順に並べる。
    /// 面した道路のない駐車場は貸さない。
    pub(super) fn shared_lot_sites(&self) -> Vec<SharedLotSite> {
        self.facilities
            .iter()
            .filter(|f| f.kind.is_parking() && f.entrance.is_some())
            .map(|f| (f.area.min, f.front, f.kind.parking_spaces()))
            .collect()
    }

    /// 道路に面していない共同の駐車場。貸せないが、範囲の中にあるかを理由の判定に使う。並びは [`Self::shared_lot_sites`] と同じ。
    pub(super) fn unconnected_lot_sites(&self) -> Vec<SharedLotSite> {
        self.facilities
            .iter()
            .filter(|f| f.kind.is_parking() && f.entrance.is_none())
            .map(|f| (f.area.min, f.front, f.kind.parking_spaces()))
            .collect()
    }

    /// ゲーム内の 1 日に 1 回、料金が払える上限を超えた世帯に借りた枠を返させ、枠のない世帯に貸し直す。
    /// 借りている世帯は、より近い駐車場ができても移らない。共同の駐車場がなければ何もしない。
    pub(super) fn review_shared_lots(&mut self) {
        if !self.facilities.iter().any(|f| f.kind.is_parking()) {
            return;
        }
        let p = &self.population;
        let fee = i64::from(self.parking_fee);
        let over: Vec<HouseholdId> = self
            .car_places
            .iter()
            .filter(|(h, place)| {
                matches!(place, CarPlace::Shared(_))
                    && fee > fee_cap(p.household_money(*h).unwrap_or(Money::ZERO))
            })
            .map(|(h, _)| h)
            .collect();
        for h in over {
            self.trips.forget_car(h);
            self.car_places.remove(h);
        }
        self.rent_shared_lots();
        self.sync_rented();
    }

    /// 取り壊した共同の駐車場 `corner` を借りていた世帯の枠をなくし、停まっていた車も消す。
    /// 枠をなくした世帯は、次の決め直しでほかの駐車場を探す。
    pub(super) fn forget_shared_lot(&mut self, corner: TilePos) {
        let renters: Vec<HouseholdId> = self
            .car_places
            .iter()
            .filter(|(_, place)| *place == CarPlace::Shared(corner))
            .map(|(h, _)| h)
            .collect();
        for h in renters {
            self.trips.forget_car(h);
            self.car_places.remove(h);
        }
        self.trips.forget_shared_lot(corner);
        self.sync_rented();
    }

    /// 終わった移動 `ended` の住民の居場所を切り替える。
    pub(super) fn end_commutes(&mut self, ended: Vec<TripId>) {
        let mut arrived = Vec::new();
        for trip in ended {
            let Some(c) = self.trips.take_commuter(trip) else {
                continue;
            };
            let finished = self.trips.finished(trip);
            if let Some(f) = finished {
                self.tally_finished(f);
            }
            let Some(row) = self.population.citizen(c.citizen) else {
                continue;
            };
            if row.trip != Some(trip) {
                continue;
            }
            self.population.set_trip(c.citizen, None);
            if row.schedule.next == c.to && row.schedule.depart_at.is_none() {
                // 職場へは家から向かうので、着いた移動の時間が、家を出てから職場に着くまでの時間になる。
                // 向かっているあいだに職場が別の建物に変わったら、予定が立て直されて上の条件で外れる。
                // 職をなくしただけなら予定は残るので、職のない住民には記録しない。
                if c.to == Activity::Work
                    && row.workplace.is_some()
                    && let Some(f) = finished.filter(|f| f.reached)
                {
                    let took = self.tick.0.saturating_sub(f.departed.0);
                    self.population.record_commute(c.citizen, took);
                }
                arrived.push(c.citizen);
            } else {
                // 向かっているあいだに職場が変わるかなくなり、着いても予定どおりの居場所にならない。
                // 着いた所に停めた車は、本人がもうそこから出ないので、どこにもないものに戻す。
                let own = self.own_lot(row.household);
                self.trips.return_car(row.household, c.citizen, own);
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
    use crate::world::economy::TICKS_PER_HOUR;
    use crate::world::people::Shift;
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
            let peak = Shift::of(ZoneKind::Industrial).leave_home;
            while w.tick().0 < TICKS_PER_DAY + peak + TICKS_PER_HOUR / 2 {
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

/// 貸せる共同の駐車場 1 つ。角のマス、正面のマス、台数。
pub(super) type SharedLotSite = (TilePos, TilePos, u32);

/// 家の正面のマス `from` から [`SHARED_LOT_RADIUS`] マスの中にある駐車場を、直線の距離の近い順（同じなら角のマスの順）に並べる。
/// 距離は 2 乗で、駐車場の台数が空いているかは見ない。
pub(super) fn lots_in_range(lots: &[SharedLotSite], from: TilePos) -> Vec<(i128, TilePos, u32)> {
    let radius = i128::from(SHARED_LOT_RADIUS);
    let dist2 = |a: TilePos, b: TilePos| {
        let d = |p: i64, q: i64| i128::from(p) - i128::from(q);
        let (dx, dy) = (d(a.x, b.x), d(a.y, b.y));
        dx * dx + dy * dy
    };
    let mut near: Vec<(i128, TilePos, u32)> = lots
        .iter()
        .map(|(c, front, spaces)| (dist2(from, *front), *c, *spaces))
        .filter(|(d, _, _)| *d <= radius * radius)
        .collect();
    near.sort_unstable();
    near
}

/// 建物の自宅の枠の数の上限。テストで自宅の枠が足りない街を作るのに使い、ふつうは `None` である。
#[cfg(not(any(test, feature = "test-util")))]
fn home_spaces_limit() -> Option<u64> {
    None
}

#[cfg(any(test, feature = "test-util"))]
fn home_spaces_limit() -> Option<u64> {
    crate::car_place::testing::HOME_SPACES_LIMIT.with(std::cell::Cell::get)
}

#[cfg(test)]
mod car_tests {
    use super::CAR_OWNERSHIP_PERCENT;
    use crate::command::ZoneKind;
    use crate::world::people::tests::{city, run_days};

    #[test]
    fn about_the_set_share_of_households_own_a_car() {
        let mut w = city(&[ZoneKind::Residential, ZoneKind::Industrial]);
        run_days(&mut w, 3);
        let ids: Vec<_> = w.population().household_ids().collect();
        assert!(ids.len() > 20, "世帯が少なすぎる: {}", ids.len());
        let owners = ids.iter().filter(|h| w.owns_car(**h)).count() as u64;
        let percent = owners * 100 / ids.len() as u64;
        assert!(
            percent.abs_diff(CAR_OWNERSHIP_PERCENT) <= 20,
            "車を持つ世帯が {percent}%"
        );
    }
}

#[cfg(test)]
mod car_place_tests {
    use std::collections::BTreeMap;

    use crate::car_place::{CarPlace, SHARED_LOT_RADIUS};
    use crate::clock::TICKS_PER_DAY;
    use crate::command::ServiceKind;
    use crate::command::{Command, ZoneKind};
    use crate::id::BuildingId;
    use crate::movement::HouseholdCar;
    use crate::movement::{LotId, SharedLot, shared_facility};
    use crate::tile::TilePos;
    use crate::world::ApplyOutcome;
    use crate::world::World;
    use crate::world::people::tests::city;

    /// 建物ごとの、自宅の枠を持つ世帯の数。
    fn holders(w: &World) -> BTreeMap<BuildingId, u64> {
        let mut out = BTreeMap::new();
        for (h, place) in w.car_places().iter() {
            assert_eq!(place, CarPlace::Home);
            let home = w
                .population()
                .home(h)
                .flatten()
                .expect("家のない世帯が枠を持つ");
            *out.entry(home).or_insert(0) += 1;
        }
        out
    }

    /// 建物ごとの、世帯が車をなくしたまま向かっている移動の数（[`crate::movement::Trips::car_dropped`]）。
    /// この移動は着いても停めずに、押さえた空きを返す。
    fn dropped(w: &World) -> BTreeMap<BuildingId, u64> {
        let mut out = BTreeMap::new();
        for (id, t) in w.trips().iter() {
            if let Some(b) = t.parking_lot().and_then(LotId::building)
                && w.trips().car_dropped(id)
            {
                *out.entry(b).or_insert(0) += 1;
            }
        }
        out
    }

    /// 枠は家のある世帯だけが持ち、建物の台数を超えない。台数が世帯の数に足りる建物では、どの世帯も枠を持つ。
    /// 住宅の駐車場に停めてある台数と押さえた台数は、枠を持つ世帯の数を超えない。ただし、枠をなくした世帯の
    /// 向かっている車が押さえた空きは、着くまで残る。
    fn check(w: &World) {
        let dropped = dropped(w);
        let held = holders(w);
        let p = w.population();
        let mut living: BTreeMap<BuildingId, u64> = BTreeMap::new();
        for h in p.household_ids() {
            if let Some(home) = p.home(h).flatten() {
                *living.entry(home).or_insert(0) += 1;
            }
        }
        for (id, n) in &living {
            let spaces = w.buildings().parking_spaces(*id);
            let got = held.get(id).copied().unwrap_or(0);
            assert_eq!(got, (*n).min(spaces), "建物 {id:?} の枠の数");
        }
        for (id, b) in w.buildings().iter() {
            if b.zone.kind() != ZoneKind::Residential {
                continue;
            }
            let lot = w.trips().lot(id);
            let used = u64::from(lot.parked) + u64::from(lot.reserved)
                - dropped.get(&id).copied().unwrap_or(0);
            assert!(
                used <= held.get(&id).copied().unwrap_or(0),
                "建物 {id:?} に枠より多く停まっている: {lot:?}"
            );
        }
    }

    /// 越してきた世帯、段階の上がった住宅、取り壊した住宅の、どの場面でも枠を配り直す。
    #[test]
    fn home_places_follow_moves_level_ups_and_removals() {
        let mut w = city(&[ZoneKind::Residential, ZoneKind::Industrial]);
        let mut leveled = false;
        while w.tick.0 < 3 * TICKS_PER_DAY {
            w.step();
            if w.tick.0 % 60 == 1 {
                check(&w);
                leveled |= w
                    .buildings()
                    .iter()
                    .any(|(_, b)| b.zone.kind() == ZoneKind::Residential && b.level > 1);
            }
        }
        assert!(leveled, "段階の上がった住宅がない");
        assert!(
            holders(&w).values().sum::<u64>() > 20,
            "枠を持つ世帯が少ない"
        );
        // 自宅の駐車場を持たない戸建ての世帯は枠を持たないが、そのほかの世帯はみな枠を持つ（`check`）。
        // 世帯の住む住宅を 1 つ取り壊すと、その世帯は枠をなくし、家でない所（職場など）に停めてあった車も
        // その駐車場から外れる。家の駐車場は建物ごと消える。
        let away = |w: &World| {
            let p = w.population();
            w.car_places().iter().find_map(|(h, _)| {
                let home = p.home(h).flatten()?;
                match w.trips().household_car(h) {
                    Some(HouseholdCar::Parked { lot, .. })
                        if lot != crate::movement::LotId::Building(home) =>
                    {
                        Some(home)
                    }
                    _ => None,
                }
            })
        };
        for _ in 0..TICKS_PER_DAY {
            if away(&w).is_some() {
                break;
            }
            w.step();
        }
        let gone = away(&w).expect("家でない所に停めた車がない");
        let b = *w.buildings().get(gone).unwrap();
        let lost: Vec<_> = w
            .car_places()
            .iter()
            .map(|(h, _)| h)
            .filter(|h| w.population().home(*h).flatten() == Some(gone))
            .collect();
        // 取り壊す家の世帯が、家でない所に停めている車の、駐車場ごとの台数。
        let mut parked_away: BTreeMap<BuildingId, u32> = BTreeMap::new();
        for h in &lost {
            if let Some(HouseholdCar::Parked { lot, .. }) = w.trips().household_car(*h)
                && let Some(lot) = lot.building()
                && lot != gone
            {
                *parked_away.entry(lot).or_insert(0) += 1;
            }
        }
        assert!(!parked_away.is_empty());
        let before: BTreeMap<BuildingId, u32> = parked_away
            .keys()
            .map(|lot| (*lot, w.trips().lot(*lot).parked))
            .collect();
        assert_eq!(
            w.apply(&Command::Bulldoze { area: b.area }),
            crate::world::ApplyOutcome::Applied
        );
        for h in lost {
            assert_eq!(w.car_places().get(h), None);
            assert!(
                !matches!(
                    w.trips().household_car(h),
                    Some(HouseholdCar::Parked { .. })
                ),
                "枠をなくした世帯の車が残っている"
            );
        }
        for (lot, n) in parked_away {
            assert_eq!(
                w.trips().lot(lot).parked,
                before[&lot] - n,
                "駐車場 {lot:?} の台数が減っていない"
            );
        }
        check(&w);
    }

    /// 戸建てが集合住宅に上がって自宅の枠が減ると、ID の大きい世帯から枠を返す。返した世帯の車は、停めてあれば
    /// 駐車場から外し、家へ向かっていれば着いても停めずに、押さえた空きを返す。
    #[test]
    fn houses_that_become_apartments_take_back_home_places() {
        let mut w = city(&[ZoneKind::Residential, ZoneKind::Industrial]);
        // 3 マス以上の段階 2 の戸建てで、段階 3 の台数より多く停めてあるか押さえてあり、段階 3 で枠を返す世帯の
        // 車が、その建物へ向かっているもの。枠を返すのは、自宅の枠を持つ世帯のうち、ID の順で段階 3 の台数を
        // 超える分である。
        let pick = |w: &World| {
            w.buildings().iter().find_map(|(id, b)| {
                let mut up = *b;
                up.level = 3;
                let spaces = up.parking_spaces(w.seed());
                let lot = w.trips().lot(id);
                if !(b.zone == crate::command::Zone::Residential
                    && b.level == 2
                    && b.tile_count() >= 3
                    && u64::from(lot.parked + lot.reserved) > spaces)
                {
                    return None;
                }
                let mut holders: Vec<_> = w
                    .car_places()
                    .iter()
                    .filter(|(h, place)| {
                        *place == CarPlace::Home && w.population().home(*h).flatten() == Some(id)
                    })
                    .map(|(h, _)| h)
                    .collect();
                holders.sort_unstable();
                let skip = usize::try_from(spaces).unwrap_or(usize::MAX);
                let heading = holders.into_iter().skip(skip).any(|h| {
                    matches!(
                        w.trips().household_car(h),
                        Some(HouseholdCar::InUse(t))
                            if w.trips().iter().any(|(tid, trip)| {
                                tid == t && trip.parking_lot() == Some(LotId::Building(id))
                            })
                    )
                });
                heading.then_some(id)
            })
        };
        let mut found = None;
        while w.tick.0 < 14 * TICKS_PER_DAY && found.is_none() {
            // 戸建てが段階 3 に上がる前に、車が家へ向かう場面を作るため、段階 2 の戸建てを段階 2 に留める。
            let hold: Vec<BuildingId> = w
                .buildings()
                .iter()
                .filter(|(_, b)| b.zone == crate::command::Zone::Residential && b.level == 2)
                .map(|(id, _)| id)
                .collect();
            for id in hold {
                w.buildings.get_mut(id).unwrap().leveled_at = w.tick;
            }
            w.step();
            found = pick(&w);
        }
        let id = found.expect("枠を返す世帯の車が家へ向かっている戸建てがない");
        w.buildings.get_mut(id).unwrap().level = 3;
        w.assign_car_places();
        let spaces = w.buildings().parking_spaces(id);
        assert_eq!(holders(&w).get(&id).copied().unwrap_or(0), spaces);
        assert!(
            dropped(&w).contains_key(&id),
            "家へ向かう車をなくした世帯がない"
        );
        check(&w);
        // 枠を返した世帯の車は、着いても停めない。向かっている車がみな着けば、台数に収まる。
        let living = |w: &World, h| w.population().home(h).flatten() == Some(id);
        for _ in 0..TICKS_PER_DAY {
            w.step();
            check(&w);
            let p = w.population();
            for h in p.household_ids().filter(|h| living(&w, *h)) {
                if let Some(HouseholdCar::Parked { lot, .. }) = w.trips().household_car(h) {
                    assert!(
                        w.car_places().get(h).is_some(),
                        "枠のない世帯の車が {lot:?} に停まっている"
                    );
                }
            }
        }
        let lot = w.trips().lot(id);
        assert!(u64::from(lot.parked + lot.reserved) <= w.buildings().parking_spaces(id));
    }

    use crate::car_place::testing::HOME_SPACES_LIMIT;

    /// 共同の駐車場の枠を借りた世帯を、駐車場の角のマスごとに数える。どの世帯も家から範囲の中の駐車場を借り、
    /// 貸した枠と外来の台数の和は台数を超えず、駐車場の台数の表と世帯の枠が合う。
    /// 借りた世帯の車が停まっている台数は、貸した枠の数を超えない。
    fn check_rented(w: &World) -> BTreeMap<TilePos, u32> {
        let counts = w.car_places().rented_counts();
        for (h, place) in w.car_places().iter() {
            let CarPlace::Shared(c) = place else {
                continue;
            };
            let f = shared_facility(w.facilities(), c).expect("ない駐車場を借りている");
            let home = w
                .population()
                .home(h)
                .flatten()
                .expect("家のない世帯が借りている");
            let home = w.buildings().get(home).unwrap();
            let (dx, dy) = (home.front.x - f.front.x, home.front.y - f.front.y);
            let r = SHARED_LOT_RADIUS as i64;
            assert!(dx * dx + dy * dy <= r * r, "遠い駐車場を借りている");
        }
        for f in w.facilities().iter().filter(|f| f.kind.is_parking()) {
            let c = f.area.min;
            let lot = w.trips().shared_lot(c);
            assert_eq!(lot.rented, counts.get(&c).copied().unwrap_or(0), "{c:?}");
            assert!(lot.rented_parked <= lot.rented, "{c:?}: {lot:?}");
            assert!(
                lot.rented + lot.visitors.parked + lot.visitors.reserved <= f.kind.parking_spaces(),
                "{c:?}: {lot:?}"
            );
        }
        counts
    }

    fn place(w: &mut World, kind: ServiceKind, at: (i64, i64)) {
        let command = Command::PlaceBuilding {
            kind,
            at: TilePos::new(at.0, at.1),
            rotation: 2,
        };
        assert_eq!(w.apply(&command), ApplyOutcome::Applied);
    }

    /// 自宅の枠が足りない街では、枠のない世帯が近くの共同の駐車場を借り、車で出入りする。
    /// 取り壊すと、借りていた世帯は枠をなくし、停まっていた車も消える。次の決め直しで、ほかの駐車場を借りる。
    #[test]
    fn households_without_home_places_rent_shared_lots() {
        HOME_SPACES_LIMIT.with(|l| l.set(Some(0)));
        let mut w = city(&[ZoneKind::Residential, ZoneKind::Industrial]);
        // 住宅の帯（y が 4〜9）の南の道路に面して、平面の駐車場を置く。
        place(&mut w, ServiceKind::ParkingLot, (-20, 9));
        let corner = w.facilities().last().unwrap().area.min;
        let (mut parked, mut driving) = (false, false);
        while w.tick.0 < 2 * TICKS_PER_DAY {
            w.step();
            if w.tick.0 % 30 == 0 {
                let counts = check_rented(&w);
                assert!(counts.get(&corner).copied().unwrap_or(0) <= 12);
                for (h, _) in w.car_places().iter() {
                    match w.trips().household_car(h) {
                        Some(HouseholdCar::Parked { lot, .. }) => {
                            parked |= lot == LotId::Rented(corner);
                        }
                        Some(HouseholdCar::InUse(_)) => driving = true,
                        None => {}
                    }
                }
            }
        }
        let counts = check_rented(&w);
        assert_eq!(counts.get(&corner), Some(&12), "枠がすべて借りられていない");
        assert!(
            w.car_places().iter().all(|(_, p)| p != CarPlace::Home),
            "自宅の枠を持つ世帯がいる"
        );
        assert!(parked, "借りた枠に停まった車がない");
        assert!(driving, "借りた世帯の車が走っていない");
        assert!(
            w.trips().parking_tally().no_place > 0,
            "枠のない世帯が出ない"
        );
        // 状態の書き出しと読み戻しで、借りた枠と駐車場の台数が残る。
        let back = World::from_snapshot(&w.to_snapshot()).unwrap();
        assert_eq!(back.state_hash(), w.state_hash());
        assert_eq!(back.car_places(), w.car_places());
        // 取り壊すと、借りていた世帯は枠をなくし、停まっていた車も消える。
        let renters: Vec<_> = w.car_places().iter().map(|(h, _)| h).collect();
        let area = w.facilities().last().unwrap().area;
        assert_eq!(w.apply(&Command::Bulldoze { area }), ApplyOutcome::Applied);
        assert!(w.car_places().iter().next().is_none());
        assert_eq!(w.trips().shared_lot(corner), SharedLot::default());
        for h in renters {
            assert!(
                !matches!(
                    w.trips().household_car(h),
                    Some(HouseholdCar::Parked { lot, .. }) if lot.shared_corner() == Some(corner)
                ),
                "取り壊した駐車場に車が残っている"
            );
        }
        // 駐車場ビルを置くと、次の決め直しで借りる。
        place(&mut w, ServiceKind::ParkingGarage, (-20, 9));
        let garage = w.facilities().last().unwrap().area.min;
        for _ in 0..TICKS_PER_DAY {
            w.step();
        }
        let counts = check_rented(&w);
        assert!(
            counts.get(&garage).copied().unwrap_or(0) > 12,
            "駐車場ビルを借りた世帯が少ない: {counts:?}"
        );
        HOME_SPACES_LIMIT.with(|l| l.set(None));
    }
}

#[cfg(test)]
mod commute_time_tests {
    use crate::command::ZoneKind;
    use crate::happiness::{self, CitizenContext, commute_score};
    use crate::world::people::tests::{city, run_days};

    /// 職場に着いた住民は通勤の時間を持ち、通勤の点はその時間から決まる。
    #[test]
    fn the_commute_score_follows_the_measured_commute() {
        let mut w = city(&[ZoneKind::Residential, ZoneKind::Industrial]);
        run_days(&mut w, 3);
        let ctx = CitizenContext {
            seed: w.seed,
            tick: w.tick,
            population: &w.population,
            buildings: &w.buildings,
            tax_permille: w.tax_permille,
            car_places: &w.car_places,
            facilities: &w.facilities,
            parking_fee: w.parking_fee,
        };
        let p = &w.population;
        let mut measured = 0;
        for id in p.citizen_ids() {
            let row = p.citizen(id).unwrap();
            if row.workplace.is_none() {
                assert_eq!(row.commute, None, "{id:?}");
                continue;
            }
            if let Some(t) = row.commute {
                measured += 1;
                assert!(t > 0, "{id:?}");
                let parts = happiness::parts_of(&ctx, &row);
                assert_eq!(parts.commute, commute_score(u64::from(t)), "{id:?}");
            }
        }
        assert!(measured > 20, "通勤の時間を測った住民が少ない: {measured}");
    }

    /// 通勤の時間は、職場に着いた移動の、出発から着くまでの tick 数で書き換わり、ほかでは変わらない。
    /// 朝の通勤のさなかに道路を壊し、途中で消える移動と、経路がなく始められない住民を出して確かめる。
    #[test]
    fn only_arrivals_at_work_record_the_commute() {
        use crate::citizens::{Activity, COMMUTE_SMOOTHING};
        use crate::clock::TICKS_PER_DAY;
        use crate::command::Command;
        use crate::id::{CitizenId, TripId};
        use crate::tile::{TilePos, TileRect};
        use crate::units::Tick;
        use crate::world::people::Shift;
        use std::collections::BTreeMap;

        let mut w = city(&[ZoneKind::Residential, ZoneKind::Industrial]);
        run_days(&mut w, 1);
        let cut = TICKS_PER_DAY + Shift::of(ZoneKind::Industrial).leave_home + 150;
        let (mut arrived, mut lost) = (0, 0);
        let mut cut_done = false;
        while w.tick.0 < 2 * TICKS_PER_DAY {
            let to_work: BTreeMap<TripId, (CitizenId, Tick)> = w
                .trips
                .iter()
                .filter_map(|(id, t)| {
                    let c = w.trips.commuter(id).filter(|c| c.to == Activity::Work)?;
                    Some((id, (c.citizen, t.departed())))
                })
                .collect();
            let before: BTreeMap<CitizenId, Option<u32>> = w
                .population
                .citizen_ids()
                .map(|id| (id, w.population.citizen(id).unwrap().commute))
                .collect();
            // 着いたかは step の中で、その tick（進める前の tick）で決まる。前の日の集計と同じ数え方である。
            let now = w.tick.0;
            if now == cut && !cut_done {
                // 住宅の帯と工業の帯をつなぐ、西の縦の道路を壊す。通れなくなった移動は、次の step の初めに
                // 探し直して消える。その結果は step の中で消されるので、ここで探し直して確かめる。
                cut_done = true;
                let area = TileRect::from_corners(TilePos::new(-60, 20), TilePos::new(-60, 30));
                assert_eq!(
                    w.apply(&Command::Bulldoze { area }),
                    crate::world::ApplyOutcome::Applied
                );
                w.sync_router();
            } else {
                w.step();
            }
            let mut expected = before.clone();
            for (trip, (citizen, departed)) in &to_work {
                if w.trips.get(*trip).is_some() {
                    continue;
                }
                // 取りやめた移動（家を失った世帯など）は結果を残さない。通勤の時間も変わらない。
                let Some(f) = w.trips.finished(*trip) else {
                    continue;
                };
                assert_eq!(f.departed, *departed, "出発の tick が引き継がれていない");
                if !f.reached {
                    lost += 1;
                    continue;
                }
                arrived += 1;
                let took = u32::try_from(now - departed.0).unwrap();
                let old = before[citizen];
                let n = COMMUTE_SMOOTHING as u32;
                let mixed = old.map_or(took, |o| (o * (n - 1) + took + n / 2) / n);
                expected.insert(*citizen, Some(mixed));
            }
            for (id, want) in expected {
                let Some(row) = w.population.citizen(id) else {
                    continue;
                };
                // 職場か家が変われば捨てる。それ以外は、職場に着いた住民だけが書き換わる。
                if row.commute.is_some() || want.is_none() {
                    assert_eq!(row.commute, want, "{id:?} tick {now}");
                }
            }
        }
        let t = w.traffic();
        let today = t.yesterday.expect("前の日の集計がない");
        assert!(arrived > 50, "職場に着いた移動が少ない: {arrived}");
        assert!(lost > 0, "途中で消えた通勤がない");
        assert!(
            today.no_route > 0,
            "経路がなく始められなかった移動がない: {today:?}"
        );
    }
}
