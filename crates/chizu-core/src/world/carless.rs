//! 車を持てない世帯の読み取り。画面の知らせと置く道具、observe が、どこで何世帯が車を持てないかを知るために使う。
//!
//! 車を持てない理由は状態に持たない。読み出すときに、共同の駐車場を借りる決まり
//! （[`World::rent_shared_lots`](super::World) と同じ探し方）を読み取りだけで回して求める。
//! 状態は変えず、状態ハッシュにも入れない。

use std::collections::BTreeMap;

use super::World;
use super::commute::lots_in_range;
use crate::car_place::{SHARED_LOT_FEE, fee_cap};
use crate::id::{BuildingId, HouseholdId};
use crate::tile::TilePos;
use crate::units::Money;

/// 車を持てない理由。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CarlessReason {
    /// 家から [`SHARED_LOT_RADIUS`](crate::car_place::SHARED_LOT_RADIUS) マスの中に、共同の駐車場が 1 つもない。
    NoLotInRange,
    /// 範囲の中に共同の駐車場はあるが、どれも空きがない。
    Full,
    /// 範囲の中に共同の駐車場はあるが、料金が世帯の払える上限を超える。
    FeeTooHigh,
    /// 範囲の中に空きのある共同の駐車場があり、料金も払える。次の決め直し（移住を見る間隔ごと）で借りる。
    Waiting,
    /// 範囲の中の共同の駐車場が、どれも道路に面していない（前の道路が消えた）。道路に面していない駐車場は貸さない。
    LotNotOnRoad,
}

impl CarlessReason {
    pub const ALL: [CarlessReason; 5] = [
        CarlessReason::NoLotInRange,
        CarlessReason::Full,
        CarlessReason::FeeTooHigh,
        CarlessReason::Waiting,
        CarlessReason::LotNotOnRoad,
    ];

    /// `ALL` の中での位置。
    pub fn index(self) -> usize {
        self as usize
    }
}

/// 車を持てない世帯 1 つ。家のある世帯のうち、車を置く枠（自宅の枠も共同の駐車場の枠も）がない世帯である。
/// 家のない世帯は、車を持てない世帯に数えない。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Carless {
    pub household: HouseholdId,
    pub home: BuildingId,
    /// 家の正面のマス。共同の駐車場を探す範囲は、このマスから測る。
    pub front: TilePos,
    pub reason: CarlessReason,
}

/// 車を持てない世帯を数える区切りの 1 辺（マス）。
pub const CARLESS_AREA_SIDE: i64 = 8;

/// 区切り 1 つの、車を持てない世帯の数。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CarlessArea {
    /// 区切りの最小の角のマス。座標は [`CARLESS_AREA_SIDE`] の倍数である。
    pub min: TilePos,
    /// 車を持てない世帯の数。
    pub households: u32,
    /// 理由ごとの数。並びは [`CarlessReason::ALL`] である。
    pub by_reason: [u32; 5],
}

impl CarlessArea {
    /// 区切りの真ん中のマス。画面がカメラを寄せる先にする。
    pub fn center(&self) -> TilePos {
        TilePos::new(
            self.min.x + CARLESS_AREA_SIDE / 2,
            self.min.y + CARLESS_AREA_SIDE / 2,
        )
    }
}

/// 車を持てない世帯を 8×8 マスの区切りで数え、多い順に上位 `limit` 件を返す。同じ数なら区切りの座標（x、y の順）が小さいほうが先である。
pub fn carless_areas(carless: &[Carless], limit: usize) -> Vec<CarlessArea> {
    let mut cells: BTreeMap<(i64, i64), CarlessArea> = BTreeMap::new();
    for c in carless {
        let key = (
            c.front.x.div_euclid(CARLESS_AREA_SIDE),
            c.front.y.div_euclid(CARLESS_AREA_SIDE),
        );
        let area = cells.entry(key).or_insert(CarlessArea {
            min: TilePos::new(key.0 * CARLESS_AREA_SIDE, key.1 * CARLESS_AREA_SIDE),
            households: 0,
            by_reason: [0; 5],
        });
        area.households += 1;
        area.by_reason[c.reason.index()] += 1;
    }
    let mut out: Vec<CarlessArea> = cells.into_values().collect();
    out.sort_by(|a, b| {
        b.households
            .cmp(&a.households)
            .then((a.min.x, a.min.y).cmp(&(b.min.x, b.min.y)))
    });
    out.truncate(limit);
    out
}

impl World {
    /// 車を持てない世帯を、世帯の ID の順に返す。理由は共同の駐車場の探し方から求める。状態は変えない。
    /// 空きがあるかは世帯ごとに見るので、同じ空きを待つ世帯が何世帯いても、どれも `Waiting` になる。
    pub fn carless_households(&self) -> Vec<Carless> {
        let p = &self.population;
        let lots = self.shared_lot_sites();
        let unconnected = self.unconnected_lot_sites();
        let rented = self.car_places.rented_counts();
        let mut out = Vec::new();
        for h in p.household_ids() {
            if self.car_places.get(h).is_some() {
                continue;
            }
            let Some((home, b)) = p
                .home(h)
                .flatten()
                .and_then(|id| self.buildings.get(id).map(|b| (id, b)))
            else {
                continue;
            };
            let near = lots_in_range(&lots, b.front);
            let savings = p.household_money(h).unwrap_or(Money::ZERO);
            let reason = if near.is_empty() {
                if lots_in_range(&unconnected, b.front).is_empty() {
                    CarlessReason::NoLotInRange
                } else {
                    CarlessReason::LotNotOnRoad
                }
            } else if SHARED_LOT_FEE > fee_cap(savings) {
                CarlessReason::FeeTooHigh
            } else {
                let has_room = near.iter().any(|(_, c, spaces)| {
                    let mut lot = self.trips.shared_lot(*c);
                    lot.rented = rented.get(c).copied().unwrap_or(0);
                    lot.has_room(*spaces)
                });
                if has_room {
                    CarlessReason::Waiting
                } else {
                    CarlessReason::Full
                }
            };
            out.push(Carless {
                household: h,
                home,
                front: b.front,
                reason,
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::car_place::testing::set_home_spaces_limit;
    use crate::clock::TICKS_PER_DAY;
    use crate::command::{Command, ServiceKind, ZoneKind};
    use crate::world::ApplyOutcome;
    use crate::world::people::tests::city;

    fn housed(w: &World) -> usize {
        let p = w.population();
        p.household_ids()
            .filter(|h| {
                p.home(*h)
                    .flatten()
                    .is_some_and(|b| w.buildings().get(b).is_some())
            })
            .count()
    }

    fn run(w: &mut World, ticks: u64) {
        for _ in 0..ticks {
            w.step();
        }
    }

    fn renters(w: &World) -> usize {
        w.car_places()
            .rented_counts()
            .values()
            .map(|n| *n as usize)
            .sum()
    }

    fn count(carless: &[Carless], reason: CarlessReason) -> usize {
        carless.iter().filter(|c| c.reason == reason).count()
    }

    /// 自宅の枠がなく共同の駐車場もない街では、家のある世帯がすべて、範囲に駐車場がない理由で車を持てない。
    /// 駐車場を置いて借りる世帯が出ても、車を持てない世帯と枠を持つ世帯で、家のある世帯が過不足なく分かれる。
    /// 満車になれば、範囲の中の世帯は満車、外の世帯は範囲に駐車場がない理由になる。読み取りは状態を変えない。
    #[test]
    fn carless_households_are_found_with_their_reasons() {
        set_home_spaces_limit(Some(0));
        let mut w = city(&[ZoneKind::Residential, ZoneKind::Industrial]);
        run(&mut w, TICKS_PER_DAY);
        let carless = w.carless_households();
        let total = housed(&w);
        assert!(total > 12, "世帯が足りない: {total}");
        assert_eq!(carless.len(), total);
        assert_eq!(count(&carless, CarlessReason::NoLotInRange), total);
        assert!(carless.windows(2).all(|p| p[0].household < p[1].household));
        let p = w.population();
        assert!(
            carless
                .iter()
                .all(|c| p.home(c.household).flatten() == Some(c.home))
        );

        let command = Command::PlaceBuilding {
            kind: ServiceKind::ParkingLot,
            at: TilePos::new(-20, 9),
            rotation: 2,
        };
        assert_eq!(w.apply(&command), ApplyOutcome::Applied);
        let lot = *w.facilities().last().unwrap();
        let hash = w.state_hash();
        let after = w.carless_households();
        assert_eq!(w.state_hash(), hash, "読み取りが状態を変えた");
        // 駐車場の下の建物は取り壊されるので、家をなくした世帯は数えない
        assert!(after.len() < total);
        assert_eq!(renters(&w), 12, "枠がすべて借りられていない");
        assert_eq!(
            after.len() + renters(&w),
            housed(&w),
            "世帯の分かれ方が合わない"
        );
        // 範囲の中の世帯は満車、外の世帯は範囲に駐車場がない
        let within = |c: &Carless| {
            let (dx, dy) = (c.front.x - lot.front.x, c.front.y - lot.front.y);
            dx * dx + dy * dy <= 32 * 32
        };
        assert!(
            after.iter().any(within) && after.iter().any(|c| !within(c)),
            "範囲の内と外の世帯がそろっていない"
        );
        for c in &after {
            let want = if within(c) {
                CarlessReason::Full
            } else {
                CarlessReason::NoLotInRange
            };
            assert_eq!(c.reason, want, "{c:?}");
        }

        // 借りていた世帯が出て枠が空くと、次の決め直しまでは、範囲の中の世帯は借りるのを待っている
        let renter = w
            .car_places()
            .iter()
            .map(|(h, _)| h)
            .next()
            .expect("借りた世帯がいない");
        w.car_places.remove(renter);
        w.sync_rented();
        let waiting = w.carless_households();
        assert_eq!(count(&waiting, CarlessReason::Full), 0);
        assert!(count(&waiting, CarlessReason::Waiting) > 0);
        for c in &waiting {
            let want = if within(c) {
                CarlessReason::Waiting
            } else {
                CarlessReason::NoLotInRange
            };
            assert_eq!(c.reason, want, "{c:?}");
        }
        // 日が進むと、空いた枠は借りる世帯に埋まる
        run(&mut w, TICKS_PER_DAY);
        assert_eq!(renters(&w), 12);
        assert_eq!(count(&w.carless_households(), CarlessReason::Waiting), 0);
        set_home_spaces_limit(None);
    }

    fn at(x: i64, y: i64, reason: CarlessReason) -> Carless {
        Carless {
            household: crate::id::HouseholdId {
                index: 0,
                generation: 0,
            },
            home: crate::id::BuildingId {
                index: 0,
                generation: 0,
            },
            front: TilePos::new(x, y),
            reason,
        }
    }

    #[test]
    fn areas_count_per_eight_by_eight_cell_and_rank_by_count() {
        use CarlessReason::{Full, NoLotInRange};
        let carless = [
            at(0, 0, NoLotInRange),
            at(7, 7, Full),
            at(8, 0, Full),
            at(-1, -1, NoLotInRange),
            at(-8, -8, NoLotInRange),
            at(-9, -9, NoLotInRange),
            at(3, 3, NoLotInRange),
        ];
        let areas = carless_areas(&carless, 10);
        // 区切りの境は負の座標でも 8 の倍数で、(-1, -1) と (-8, -8) は同じ区切りに入る
        let cells: Vec<_> = areas
            .iter()
            .map(|a| (a.min.x, a.min.y, a.households))
            .collect();
        assert_eq!(cells, [(0, 0, 3), (-8, -8, 2), (-16, -16, 1), (8, 0, 1)]);
        assert_eq!(areas[0].by_reason, [2, 1, 0, 0, 0]);
        assert_eq!(areas[0].center(), TilePos::new(4, 4));
        assert_eq!(carless_areas(&carless, 2).len(), 2);
        assert!(carless_areas(&[], 10).is_empty());
    }
}
