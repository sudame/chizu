//! 財政と人口のマイルストーン。
//!
//! - 収支：住民の賃金と職場の利益に区画の種類ごとの税率をかけた税収と、共同の駐車場の料金から、
//!   道路と施設の維持費を引く。
//!   1 日あたりの見込みを [`World::finances`] で出し、ゲーム内の 1 時間ごとにその 1/24 を所持金に足す。
//!   24 で割り切れない端数は持ち越すので、何時間たっても 1 日あたりの見込みとずれない。
//!   所持金は負になってもよい。
//! - マイルストーン：家のある住民の数が段階に届くと、買えるチャンクの枠が増える。一度届いた段階は人口が減っても戻らない。

use super::World;
use crate::car_place::CarPlace;
use crate::clock::TICKS_PER_DAY;
use crate::command::ZoneKind;
use crate::network::{SegmentKind, TILE_MM, road_cost_per_tile};
use crate::round;
use crate::units::Money;

/// 1 日の時間の数。
pub const HOURS_PER_DAY: u64 = 24;

/// ゲーム内の 1 時間の tick 数。
pub const TICKS_PER_HOUR: u64 = TICKS_PER_DAY / HOURS_PER_DAY;

/// 働く住民 1 人の 1 日の賃金の、職場の段階によらない部分。
pub const BASE_WAGE_PER_DAY: i64 = 80;

/// 職場の段階が 1 上がるごとに、1 日の賃金に足す額。
pub const WAGE_PER_LEVEL: i64 = 20;

/// 職場の区画の種類ごとの、働く住民 1 人あたりの 1 日の利益。添え字は [`ZoneKind::index`]。住宅の欄は使わない。
pub const PROFIT_PER_WORKER_PER_DAY: [i64; 4] = [0, 60, 70, 90];

/// 道路の 1 日の維持費の、建設費に対する割合（‰）。
pub const ROAD_UPKEEP_PERMILLE_PER_DAY: i64 = 10;

/// 始めたときに買えるチャンクの数。最初から買ってあるチャンクは数えない。
pub const INITIAL_CHUNK_SLOTS: u32 = 6;

/// 人口のマイルストーン。届く人口と、届いたあとに買えるチャンクの数の合計。人口の小さい順に並ぶ。
pub const MILESTONES: [(u64, u32); 10] = [
    (250, 9),
    (600, 13),
    (1_200, 18),
    (2_500, 24),
    (5_000, 31),
    (10_000, 39),
    (20_000, 48),
    (40_000, 58),
    (70_000, 69),
    (100_000, 81),
];

/// 表の最後のマイルストーンより先は、この人口が増えるごとに段階を 1 つ上げる。広さに上限を設けないためである。
pub const MILESTONE_POPULATION_STEP: u64 = 50_000;

/// 表の最後のマイルストーンより先の段階 1 つで増える枠。
pub const MILESTONE_SLOT_STEP: u32 = 12;

/// 段階 `k`（1 から数える）に届く人口。
pub fn milestone_population(k: u32) -> u64 {
    assert!(k > 0, "段階は 1 から数える");
    let last = MILESTONES.len() as u32;
    match MILESTONES.get(k as usize - 1) {
        Some((pop, _)) => *pop,
        None => MILESTONES[MILESTONES.len() - 1]
            .0
            .checked_add(
                u64::from(k - last)
                    .checked_mul(MILESTONE_POPULATION_STEP)
                    .expect("人口があふれた"),
            )
            .expect("人口があふれた"),
    }
}

/// 段階 `k` まで届いたときに買えるチャンクの数の合計。`k` が 0 なら始めたときの枠である。
pub fn chunk_slots(k: u32) -> u32 {
    let last = MILESTONES.len() as u32;
    match k {
        0 => INITIAL_CHUNK_SLOTS,
        _ if k <= last => MILESTONES[k as usize - 1].1,
        _ => (k - last)
            .checked_mul(MILESTONE_SLOT_STEP)
            .and_then(|extra| extra.checked_add(MILESTONES[MILESTONES.len() - 1].1))
            .expect("枠の数があふれた"),
    }
}

/// 職場の段階ごとの、働く住民 1 人の 1 日の賃金。
pub fn wage_per_day(level: u8) -> i64 {
    BASE_WAGE_PER_DAY + WAGE_PER_LEVEL * i64::from(level)
}

/// 1 日あたりの収支の見込み。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Finances {
    /// 区画の種類ごとの税収。住宅は住民の賃金から、ほかは職場の利益から取る。添え字は [`ZoneKind::index`]。
    pub taxes: [Money; 4],
    /// 共同の駐車場の料金の収入。借りられた枠の数 × 料金である。
    pub parking_fees: Money,
    /// 道路の維持費。外部接続の高速道路は含めない。
    pub road_upkeep: Money,
    /// 発電所と浄水場の維持費。添え字は [`ServiceKind::utility_slot`]。
    pub facility_upkeep: [Money; 2],
    /// 共同の駐車場の維持費。
    pub parking_upkeep: Money,
}

impl Finances {
    pub fn income(&self) -> Money {
        self.taxes.iter().fold(self.parking_fees, |a, b| a + *b)
    }

    pub fn expenses(&self) -> Money {
        self.facility_upkeep
            .iter()
            .fold(self.road_upkeep + self.parking_upkeep, |a, b| a + *b)
    }

    /// 収入から支出を引いた額。負なら赤字である。
    pub fn net(&self) -> Money {
        self.income() - self.expenses()
    }
}

fn mul(a: i64, b: i64) -> i64 {
    a.checked_mul(b).expect("収支の計算があふれた")
}

fn add(a: i64, b: i64) -> i64 {
    a.checked_add(b).expect("収支の計算があふれた")
}

impl World {
    /// いまの街のままで 1 日たったときの収支の見込み。
    pub fn finances(&self) -> Finances {
        let p = &self.population;
        // 区画の種類ごとの、税をかける前の額（住宅は賃金、ほかは利益）。
        let mut base = [0_i64; 4];
        for id in p.citizen_ids() {
            let row = p.citizen(id).expect("いる住民の行がない");
            let Some(b) = row.workplace.and_then(|w| self.buildings.get(w)) else {
                continue;
            };
            let r = ZoneKind::Residential.index();
            base[r] = add(base[r], wage_per_day(b.level));
            let z = b.zone.kind().index();
            base[z] = add(base[z], PROFIT_PER_WORKER_PER_DAY[z]);
        }
        let mut f = Finances::default();
        for zone in ZoneKind::ALL {
            let i = zone.index();
            let taxed = mul(base[i], i64::from(self.tax_permille[i]));
            f.taxes[i] = Money(round::div_floor(taxed, 1_000));
        }
        let rented = self
            .car_places
            .iter()
            .filter(|(_, place)| matches!(place, CarPlace::Shared(_)))
            .count();
        let rented = i64::try_from(rented).expect("借りられた枠の数があふれた");
        f.parking_fees = Money(mul(rented, i64::from(self.parking_fee)));
        // 1 マスあたりの建設費 × 長さ（mm）を足してから割り、端数を区間ごとに落とさない。
        let mut road = 0_i64;
        for (id, s) in self.network.segments() {
            let SegmentKind::Road(kind) = s.kind else {
                continue;
            };
            let line = self.network.segment_line(id).expect("区間の線がない");
            road = add(road, mul(road_cost_per_tile(kind).0, line.length().0));
        }
        f.road_upkeep = Money(round::div_ceil(
            mul(road, ROAD_UPKEEP_PERMILLE_PER_DAY),
            mul(TILE_MM, 1_000),
        ));
        for facility in &self.facilities {
            let upkeep = facility.kind.upkeep_per_day();
            match facility.kind.utility_slot() {
                Some(i) => f.facility_upkeep[i] = f.facility_upkeep[i] + upkeep,
                None => f.parking_upkeep = f.parking_upkeep + upkeep,
            }
        }
        f
    }

    /// 1 日あたりの見込みの 1/24 を所持金に足す。端数は次の時間に持ち越す。
    fn settle_hour(&mut self) {
        let hours = i64::try_from(HOURS_PER_DAY).expect("時間の数があふれた");
        let total = add(self.finances().net().0, self.ledger_carry);
        let this_hour = round::div_floor(total, hours);
        self.ledger_carry = total - this_hour * hours;
        self.money = self.money + Money(this_hour);
    }

    /// 届いた人口のマイルストーンの段階。0 ならまだどこにも届いていない。
    pub fn milestone(&self) -> u32 {
        self.milestone
    }

    /// いまの段階で買えるチャンクの数の合計。最初から買ってあるチャンクは数えない。
    pub fn chunk_slots(&self) -> u32 {
        if self.unlimited_land {
            u32::MAX
        } else {
            chunk_slots(self.milestone)
        }
    }

    /// 家のある住民の数。人口のマイルストーンはこの数で見る。
    pub fn housed_citizens(&self) -> u64 {
        let p = &self.population;
        p.household_ids()
            .filter(|h| {
                p.home(*h)
                    .flatten()
                    .is_some_and(|b| self.buildings.get(b).is_some())
            })
            .map(|h| p.members(h).map_or(0, |m| m.len() as u64))
            .sum()
    }

    fn update_milestone(&mut self) {
        let citizens = self.housed_citizens();
        while citizens >= milestone_population(self.milestone + 1) {
            self.milestone += 1;
        }
    }

    /// ゲーム内の 1 時間ごとの処理。`step` から呼ぶ。
    pub(super) fn update_economy(&mut self) {
        if self.tick.0.is_multiple_of(TICKS_PER_HOUR) {
            self.settle_hour();
            self.update_milestone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{Command, RoadKind, ServiceKind};
    use crate::tile::{ChunkCoord, TilePos};
    use crate::world::people::tests::{city, run_days};
    use crate::world::{ApplyOutcome, DenyReason};

    #[test]
    fn milestones_grow_without_limit() {
        assert_eq!(chunk_slots(0), INITIAL_CHUNK_SLOTS);
        let mut prev = (0, chunk_slots(0));
        for k in 1..30 {
            let now = (milestone_population(k), chunk_slots(k));
            assert!(now.0 > prev.0 && now.1 > prev.1, "段階 {k}");
            prev = now;
        }
        assert_eq!(milestone_population(11), 150_000);
        assert_eq!(chunk_slots(11), 93);
    }

    #[test]
    fn roads_and_facilities_cost_upkeep_every_hour() {
        let mut w = World::new(3);
        let road = Command::BuildRoad {
            from: TilePos::new(0, 0),
            to: TilePos::new(40, 0),
            kind: RoadKind::Medium,
        };
        assert_eq!(w.apply(&road), ApplyOutcome::Applied);
        let plant = Command::PlaceBuilding {
            kind: ServiceKind::PowerPlant,
            at: TilePos::new(6, -1),
            rotation: 2,
        };
        assert_eq!(w.apply(&plant), ApplyOutcome::Applied);
        let f = w.finances();
        // 40 マスの中くらいの道路は 18,000 で、その 1% が 1 日の維持費になる。
        assert_eq!(f.road_upkeep, Money(180));
        assert_eq!(f.facility_upkeep, [Money(1_200), Money::ZERO]);
        assert_eq!(f.income(), Money::ZERO);
        assert_eq!(f.net(), Money(-1_380));
        let start = w.money();
        while w.tick().0 < TICKS_PER_HOUR + 1 {
            w.step();
        }
        // k 回精算すると、端数を持ち越すので、ちょうど 1 日の見込みの k/24 だけ減る。
        let after = |k: i64| start + Money(round::div_floor(-1_380 * k, 24));
        // tick 0 と 1 時間めの 2 回の精算が済んでいる。
        assert_eq!(w.money(), after(2));
        run_days(&mut w, 1);
        assert_eq!(w.money(), after(26));
    }

    #[test]
    fn money_keeps_going_below_zero() {
        let mut w = World::new_with_money(3, Money(62_600));
        let road = Command::BuildRoad {
            from: TilePos::new(0, 0),
            to: TilePos::new(10, 0),
            kind: RoadKind::Small,
        };
        assert_eq!(w.apply(&road), ApplyOutcome::Applied);
        let plant = Command::PlaceBuilding {
            kind: ServiceKind::PowerPlant,
            at: TilePos::new(6, -1),
            rotation: 2,
        };
        assert_eq!(w.apply(&plant), ApplyOutcome::Applied);
        assert_eq!(w.money(), Money(100));
        run_days(&mut w, 2);
        assert!(w.money() < Money::ZERO);
        assert_eq!(w.tick().0, 2 * TICKS_PER_DAY);
    }

    /// 人の住む街は税を納め、維持費の大きい街では所持金が減っていく。
    #[test]
    fn taxes_come_in_and_upkeep_drains_money() {
        let mut w = city(&[ZoneKind::Residential, ZoneKind::Industrial]);
        run_days(&mut w, 1);
        let f = w.finances();
        assert!(f.taxes[ZoneKind::Residential.index()] > Money::ZERO);
        assert!(f.taxes[ZoneKind::Industrial.index()] > Money::ZERO);
        assert_eq!(f.taxes[ZoneKind::Office.index()], Money::ZERO);
        assert!(f.net() < Money::ZERO, "{f:?}");
        let before = w.money();
        run_days(&mut w, 1);
        assert!(w.money() < before);
        // 住宅の税率を倍にすると、住宅の税収が倍近くになる。
        let tax = |w: &World| w.finances().taxes[ZoneKind::Residential.index()].0;
        let low = tax(&w);
        let cmd = Command::SetTaxRate {
            zone: ZoneKind::Residential,
            permille: 200,
        };
        assert_eq!(w.apply(&cmd), ApplyOutcome::Applied);
        assert!(
            (2 * low..=2 * low + 1).contains(&tax(&w)),
            "{low} → {}",
            tax(&w)
        );
    }

    /// 人口がマイルストーンに届くまでは枠を超えて買えず、届くと買えるようになる。
    #[test]
    fn milestones_open_more_chunk_slots() {
        let mut w = city(&[ZoneKind::Residential, ZoneKind::Industrial]);
        assert_eq!(w.milestone(), 0);
        let buy = |w: &mut World, cx: i32| {
            w.apply(&Command::BuyChunk {
                coord: ChunkCoord::new(cx, 0),
            })
        };
        for cx in 2..2 + INITIAL_CHUNK_SLOTS as i32 {
            assert_eq!(buy(&mut w, cx), ApplyOutcome::Applied);
        }
        let next = 2 + INITIAL_CHUNK_SLOTS as i32;
        let before = w.clone();
        assert_eq!(
            buy(&mut w, next),
            ApplyOutcome::Denied(DenyReason::NoChunkSlot)
        );
        assert_eq!(w, before);
        while w.milestone() == 0 {
            w.step();
            assert!(
                w.tick().0 <= 4 * TICKS_PER_DAY,
                "4 日で {} 人",
                w.housed_citizens()
            );
        }
        // 精算の時刻に段階を上げるので、上がった直後は最初の段階の人口をまだ大きくは超えていない。
        assert_eq!(w.milestone(), 1);
        assert!(w.housed_citizens() >= MILESTONES[0].0);
        assert_eq!(w.chunk_slots(), MILESTONES[0].1);
        assert_eq!(buy(&mut w, next), ApplyOutcome::Applied);
    }

    /// 一度届いた段階は、人口が減っても戻らない。
    #[test]
    fn milestones_stay_after_the_population_falls() {
        let mut w = city(&[ZoneKind::Residential, ZoneKind::Industrial]);
        while w.milestone() == 0 {
            w.step();
        }
        let (reached, slots) = (w.milestone(), w.chunk_slots());
        // 区画を消して建物をなくすと、住民はみな家を失う。
        let area = crate::tile::TileRect::from_corners(TilePos::new(-59, 4), TilePos::new(19, 46));
        let clear = Command::PaintZone { area, zone: None };
        assert_eq!(w.apply(&clear), ApplyOutcome::Applied);
        for _ in 0..2 * TICKS_PER_HOUR {
            w.step();
        }
        assert_eq!(w.housed_citizens(), 0);
        assert_eq!((w.milestone(), w.chunk_slots()), (reached, slots));
    }

    #[test]
    #[should_panic(expected = "始めたあとでは決められない")]
    fn unlimited_land_cannot_be_set_after_starting() {
        let mut w = World::new(3);
        w.step();
        let _ = w.with_unlimited_land();
    }
}
