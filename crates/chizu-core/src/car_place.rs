//! 世帯ごとの、車を置く枠。
//!
//! 世帯の車は、乗っていないあいだ、世帯に配った枠に停まる。枠のない世帯は車を持たず、どの移動も歩く。
//! 住宅の駐車場に停められるのは、その建物の自宅の枠を持つ世帯の車だけである。
//! 決め方は `docs/design.md` の 3.7「共同の駐車場」の「3. 世帯と駐車場の結び付け」にある。
//!
//! 自宅の枠は、住宅の駐車場の台数（`Building::parking_spaces`）だけある。建物に住む世帯を ID の順に並べ、
//! 空いている枠を、枠を持たない世帯に順に配る。すでに枠を持つ世帯からは取り上げない。枠を取り上げると、
//! その世帯の停めてある車が、停めてよい台数を超えて残るためである。
//!
//! 自宅の枠のない世帯は、家から [`SHARED_LOT_RADIUS`] マスの中の共同の駐車場の枠を借りる（[`CarPlace::Shared`]）。
//! 借りる駐車場を探すのは `World` で、ここは結果を持つだけである。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::id::{BuildingId, HouseholdId};
use crate::movement::LotId;
use crate::state_hash::{StateHash, StateHasher};
use crate::tile::TilePos;
use crate::units::Money;

/// 自宅の枠のない世帯が共同の駐車場を探す範囲。家の出入口のマスから、駐車場の正面のマスまでの直線の距離（マス）である。
pub const SHARED_LOT_RADIUS: u64 = 32;

/// 共同の駐車場の、借りた枠 1 つあたりの 1 日の料金。料金を決めるコマンドを足すまでは 0 である。
pub const SHARED_LOT_FEE: i64 = 0;

/// 払える上限の基準にする料金（1 日）。
pub const BASE_SHARED_LOT_FEE: i64 = 10;

/// 蓄え `savings` の世帯が払える、借りた枠 1 つの 1 日の料金の上限。
/// 基準の料金 ×（100 ＋ 蓄えの割合）÷ 100 で、蓄えの割合は蓄えを `SAVINGS_FOR_FULL_BONUS` で割った百分率（0〜100）である。
pub fn fee_cap(savings: Money) -> i64 {
    let full = crate::happiness::SAVINGS_FOR_FULL_BONUS.0;
    let percent = savings.0.clamp(0, full) * 100 / full;
    BASE_SHARED_LOT_FEE * (100 + percent) / 100
}

/// 世帯の車を置く枠。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CarPlace {
    /// 住んでいる建物の駐車場の枠。
    Home,
    /// 共同の駐車場の枠。駐車場の占める範囲の最小の角のマスで指す。
    Shared(TilePos),
}

impl CarPlace {
    /// 枠 `place` のある駐車場。家が `home` の世帯の自宅の枠は、その建物の駐車場である。
    pub fn lot(place: Option<CarPlace>, home: Option<BuildingId>) -> Option<LotId> {
        match place? {
            CarPlace::Home => home.map(LotId::Building),
            CarPlace::Shared(c) => Some(LotId::Rented(c)),
        }
    }
}

impl StateHash for CarPlace {
    fn write_state(&self, h: &mut StateHasher) {
        match self {
            CarPlace::Home => h.write(&0_u8),
            CarPlace::Shared(c) => {
                h.write(&1_u8);
                h.write(&c.x);
                h.write(&c.y);
            }
        }
    }
}

/// 世帯ごとの枠。枠のない世帯は持たない。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CarPlaces {
    by_household: BTreeMap<HouseholdId, CarPlace>,
}

impl CarPlaces {
    /// 世帯 `h` の枠。
    pub fn get(&self, h: HouseholdId) -> Option<CarPlace> {
        self.by_household.get(&h).copied()
    }

    /// 枠を持つ世帯を ID の順に返す。
    pub fn iter(&self) -> impl Iterator<Item = (HouseholdId, CarPlace)> + '_ {
        self.by_household.iter().map(|(h, p)| (*h, *p))
    }

    /// 世帯 `h` の枠を返させ、持っていた枠を返す。
    pub(crate) fn remove(&mut self, h: HouseholdId) -> Option<CarPlace> {
        self.by_household.remove(&h)
    }

    /// 世帯 `h` に共同の駐車場 `corner` の枠を貸す。
    pub(crate) fn rent(&mut self, h: HouseholdId, corner: TilePos) {
        self.by_household.insert(h, CarPlace::Shared(corner));
    }

    /// 共同の駐車場ごとの、貸した枠の数。角のマスの順に並ぶ。
    pub fn rented_counts(&self) -> BTreeMap<TilePos, u32> {
        let mut out: BTreeMap<TilePos, u32> = BTreeMap::new();
        for place in self.by_household.values() {
            if let CarPlace::Shared(c) = place {
                let n = out.entry(*c).or_insert(0);
                *n = n.checked_add(1).expect("貸した枠の数があふれた");
            }
        }
        out
    }

    /// 自宅の枠を配り直し、枠をなくした世帯と、共同の駐車場の枠から自宅の枠に移った世帯を ID の順に返す。
    ///
    /// `homes` は、いる世帯と住んでいる建物の組で、`spaces` は建物の自宅の枠の数である。
    /// 自宅の枠を持つ世帯は、同じ建物に住み続けるかぎり枠を持ち続ける。ただし、建物の枠の数より多くの世帯が
    /// 枠を持っていれば、ID の大きい世帯から返させる。空いた枠は、枠を持たない世帯に ID の順に配り、
    /// まだ余れば、共同の駐車場の枠を借りている世帯に ID の順に配る。借りていた枠は返す。
    /// 自宅の枠をもらえなかった借りている世帯は、家があるかぎり借りた枠を持ち続ける。
    /// 家のない世帯と、いなくなった世帯は枠をなくす。
    pub(crate) fn assign_homes(
        &mut self,
        homes: impl IntoIterator<Item = (HouseholdId, Option<BuildingId>)>,
        spaces: impl Fn(BuildingId) -> u64,
    ) -> Vec<HouseholdId> {
        let mut by_building: BTreeMap<BuildingId, Vec<HouseholdId>> = BTreeMap::new();
        for (h, home) in homes {
            if let Some(b) = home {
                by_building.entry(b).or_default().push(h);
            }
        }
        let mut next: BTreeMap<HouseholdId, CarPlace> = BTreeMap::new();
        for (b, mut households) in by_building {
            households.sort_unstable();
            let mut room = spaces(b);
            // 自宅の枠を持つ世帯、枠を持たない世帯、共同の駐車場の枠を借りている世帯の順に、それぞれ ID の順に並べる。
            let rank = |h: &HouseholdId| match self.by_household.get(h) {
                Some(CarPlace::Home) => 0,
                None => 1,
                Some(CarPlace::Shared(_)) => 2,
            };
            households.sort_by_key(|h| (rank(h), *h));
            for h in households {
                if room > 0 {
                    room -= 1;
                    next.insert(h, CarPlace::Home);
                } else if let Some(shared @ CarPlace::Shared(_)) = self.by_household.get(&h) {
                    next.insert(h, *shared);
                }
            }
        }
        let lost = self
            .by_household
            .iter()
            .filter(|(h, place)| next.get(h) != Some(place))
            .map(|(h, _)| *h)
            .collect();
        self.by_household = next;
        lost
    }
}

impl StateHash for CarPlaces {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.by_household);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(index: u32) -> HouseholdId {
        HouseholdId {
            index,
            generation: 0,
        }
    }

    fn b(index: u32) -> BuildingId {
        BuildingId {
            index,
            generation: 0,
        }
    }

    fn homes(places: &CarPlaces) -> Vec<u32> {
        places.iter().map(|(h, _)| h.index).collect()
    }

    #[test]
    fn home_places_go_to_households_in_id_order_up_to_the_spaces() {
        let mut p = CarPlaces::default();
        let lost = p.assign_homes(
            [(h(3), Some(b(1))), (h(1), Some(b(1))), (h(2), Some(b(1)))],
            |_| 2,
        );
        assert!(lost.is_empty());
        assert_eq!(homes(&p), vec![1, 2]);
    }

    #[test]
    fn holders_keep_their_place_when_a_smaller_id_moves_in() {
        let mut p = CarPlaces::default();
        p.assign_homes([(h(5), Some(b(1)))], |_| 1);
        let lost = p.assign_homes([(h(2), Some(b(1))), (h(5), Some(b(1)))], |_| 1);
        assert!(lost.is_empty());
        assert_eq!(homes(&p), vec![5]);
    }

    #[test]
    fn more_spaces_go_to_those_without_a_place() {
        let mut p = CarPlaces::default();
        let all = [(h(1), Some(b(1))), (h(2), Some(b(1))), (h(3), Some(b(1)))];
        p.assign_homes(all, |_| 1);
        assert_eq!(homes(&p), vec![1]);
        p.assign_homes(all, |_| 2);
        assert_eq!(homes(&p), vec![1, 2]);
    }

    #[test]
    fn homeless_and_gone_households_lose_their_place() {
        let mut p = CarPlaces::default();
        p.assign_homes(
            [(h(1), Some(b(1))), (h(2), Some(b(1))), (h(3), Some(b(2)))],
            |_| 4,
        );
        let lost = p.assign_homes([(h(1), None), (h(3), Some(b(2)))], |_| 4);
        assert_eq!(lost, vec![h(1), h(2)]);
        assert_eq!(homes(&p), vec![3]);
    }

    #[test]
    fn fewer_spaces_take_places_back_from_the_largest_ids() {
        let mut p = CarPlaces::default();
        let all = [(h(1), Some(b(1))), (h(2), Some(b(1))), (h(3), Some(b(1)))];
        p.assign_homes(all, |_| 3);
        let lost = p.assign_homes(all, |_| 1);
        assert_eq!(lost, vec![h(2), h(3)]);
        assert_eq!(homes(&p), vec![1]);
    }
}
