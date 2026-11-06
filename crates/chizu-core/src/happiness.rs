//! 住民の幸福度。
//!
//! 住民ごとに通勤・生活サービス・駐車場・暮らし向きの 4 つの点数（`0..=HAPPINESS_MAX`）を持ち、組ごとの更新で
//! [`update_citizen`] が書き直す。4 つを重みを付けて足し、生活サービスに応じた上限で抑えたものを幸福度とする
//! （[`total`]）。
//! - 通勤：家を出てから職場に着くまでにかかった時間を均した値（[`CitizenRow::commute`]）。歩きも車も、
//!   待ちや渋滞を含めて同じ物差しで測る。いまの家と職場でまだ着いたことがなければ、[`commute_seconds`] で
//!   直線距離から見積もる。
//! - 生活サービス：家に電気と水道が届いているか。
//! - 駐車場：世帯が車を置く枠を持てたか、借りた共同の駐車場が遠くないか、料金が高くないか（[`parking_score`]）。
//! - 暮らし向き：職があるか（働く年齢でなければ問わない）、世帯の蓄え、住宅の税率。

use crate::building::{Building, Buildings};
use crate::car_place::{CarPlace, CarPlaces, fee_cap};
use crate::citizens::{
    CitizenChange, CitizenRow, HAPPINESS_MAX, HappinessParts, Population, WORKING_AGES,
};
use crate::command::ZoneKind;
use crate::id::{CitizenId, HouseholdId};
use crate::network::TILE_MM;
use crate::round;
use crate::service::Facility;
use crate::tile::TilePos;
use crate::units::{Money, Tick};

/// 直線距離で見積もるときの、家から職場までの速さ（mm/秒）。道なりの遠回り、歩き、待ち、渋滞を込みにした値で、
/// 測った街の住民ごとの「直線距離 ÷ 測った通勤の時間」の中央値に合わせてある（`docs/design.md` の 1.6）。
pub const STRAIGHT_LINE_MM_PER_SECOND: u64 = 1_000;

/// この時間（移動の秒）までの通勤は満点にする。時計では 60 分にあたる（1 tick が移動の 1 秒、時計の 12 秒）。
/// 測った街では、混んでいない街の通勤の中央値がこのあたりにある（`docs/design.md` の 1.6）。
pub const COMMUTE_FULL_SECONDS: u64 = 300;

/// この時間（移動の秒）以上の通勤は 0 点にする。時計では 3 時間にあたり、2 時間で半分になる。
/// 混んだ街では、通勤の長い 1 割の人がこのあたりにいる。
pub const COMMUTE_ZERO_SECONDS: u64 = 900;

/// 暮らし向きの基準になる住宅の税率（‰）。これより高いと点数が下がり、低いと上がる。
pub const REFERENCE_TAX_PERMILLE: i64 = 100;

/// 暮らし向きの点数の、職のある住民の値。基準の税率で、蓄えのないときの値である。
pub const WEALTH_EMPLOYED: i64 = 60;

/// 暮らし向きの点数の、働く年齢でない住民の値。
pub const WEALTH_NOT_WORKING_AGE: i64 = 50;

/// 暮らし向きの点数の、職のない働く年齢の住民の値。
pub const WEALTH_JOBLESS: i64 = 0;

/// 世帯の蓄えで暮らし向きの点数に足す値の上限。蓄えに比例して足し、[`SAVINGS_FOR_FULL_BONUS`] で上限に届く。
/// 蓄えで点数がばらつくので、税率を上げたときに全員が同時に出ていく境目を越えない。
pub const SAVINGS_BONUS_MAX: i64 = 40;

/// 暮らし向きの点数に [`SAVINGS_BONUS_MAX`] を足す蓄え。
pub const SAVINGS_FOR_FULL_BONUS: Money = Money(100_000);

/// 幸福度を足すときの、通勤・生活サービス・駐車場・暮らし向きの重み。
pub const WEIGHTS: [u64; 4] = [1, 1, 1, 4];

/// 共同の駐車場を借りたいのに枠がない（車を持てない）世帯の駐車場の点数。0 にしないのは、歩くことで
/// 通勤の点がすでに下がるためである。
pub const PARKING_CARLESS: u8 = 30;

/// 借りた共同の駐車場がこのマス数より近ければ、遠さで点数を下げない。家の出入口のマスから駐車場の正面のマスまでの直線である。
pub const PARKING_NEAR_TILES: u64 = 8;

/// 借りた共同の駐車場が借りる範囲の端（[`crate::car_place::SHARED_LOT_RADIUS`]）にあるときに下げる点数。
pub const PARKING_FAR_PENALTY: u64 = 40;

/// 料金が世帯の払える上限（[`fee_cap`]）に届いたときに下げる点数。
pub const PARKING_FEE_PENALTY: u64 = 30;

/// 電気も水道も届かない住民の幸福度の上限。出ていく境目（[`crate::world::people::LEAVE_BELOW`]）より低くしてある。
pub const SERVICELESS_CAP: u8 = 40;

/// 組ごとの更新で、住民 1 人の点数を求めるのに読むもの。
pub struct CitizenContext<'a> {
    pub seed: u64,
    pub tick: Tick,
    pub population: &'a Population,
    pub buildings: &'a Buildings,
    /// 区画の種類ごとの税率（‰）。添え字は [`ZoneKind::index`]。
    pub tax_permille: [u16; 4],
    /// 世帯ごとの車を置く枠。
    pub car_places: &'a CarPlaces,
    /// 置いてある施設。借りた共同の駐車場の正面のマスを引く。
    pub facilities: &'a [Facility],
    /// 共同の駐車場の料金（1 日）。
    pub parking_fee: u16,
}

/// 家から職場までの所要時間（秒）の見積もり。まだ通勤していない住民に使う。出入口のマスの直線距離を [`STRAIGHT_LINE_MM_PER_SECOND`] で割る。
pub fn commute_seconds(home: &Building, work: &Building) -> u64 {
    let dx = u128::from(home.front.x.abs_diff(work.front.x));
    let dy = u128::from(home.front.y.abs_diff(work.front.y));
    let tiles = (dx * dx + dy * dy).isqrt();
    let mm = tiles * u128::from(TILE_MM.unsigned_abs());
    u64::try_from(mm / u128::from(STRAIGHT_LINE_MM_PER_SECOND)).unwrap_or(u64::MAX)
}

/// 住民 `row` の通勤の時間（移動の秒）。測った値を均したものがあればそれを、なければ [`commute_seconds`] の
/// 見積もりを使う。`home` と `work` は、その住民の家と職場である。
pub fn commute_time(row: &CitizenRow, home: &Building, work: &Building) -> u64 {
    row.commute
        .map_or_else(|| commute_seconds(home, work), u64::from)
}

/// 通勤の点数。[`COMMUTE_FULL_SECONDS`] までは満点で、[`COMMUTE_ZERO_SECONDS`] で 0 点になるまで一様に下がる。
pub fn commute_score(seconds: u64) -> u8 {
    let max = u64::from(HAPPINESS_MAX);
    let over = seconds.saturating_sub(COMMUTE_FULL_SECONDS);
    let span = COMMUTE_ZERO_SECONDS - COMMUTE_FULL_SECONDS;
    (max - (over.min(span) * max / span)) as u8
}

/// 暮らし向きの点数。職と世帯の蓄えで決め、基準より高い税率は 1‰ ごとに 0.6 点、低い税率は
/// 1‰ ごとに 0.2 点動かす。
pub fn wealth_score(age: u8, employed: bool, savings: Money, tax_permille: u16) -> u8 {
    let base = if !WORKING_AGES.contains(&age) {
        WEALTH_NOT_WORKING_AGE
    } else if employed {
        WEALTH_EMPLOYED
    } else {
        WEALTH_JOBLESS
    };
    let saved = savings.0.clamp(0, SAVINGS_FOR_FULL_BONUS.0);
    let bonus = round::div_floor(saved * SAVINGS_BONUS_MAX, SAVINGS_FOR_FULL_BONUS.0);
    let diff = REFERENCE_TAX_PERMILLE - i64::from(tax_permille);
    let shift = if diff < 0 {
        round::div_floor(diff * 3, 5)
    } else {
        round::div_floor(diff, 5)
    };
    (base + bonus + shift).clamp(0, i64::from(HAPPINESS_MAX)) as u8
}

/// 借りた共同の駐車場の遠さで下げる点数。家の出入口のマス `home` から駐車場の正面のマス `lot` までの直線が
/// [`PARKING_NEAR_TILES`] までは 0 で、借りる範囲の端で [`PARKING_FAR_PENALTY`] になるまで一様に増やす。
pub fn parking_far_penalty(home: TilePos, lot: TilePos) -> u64 {
    let d = |p: i64, q: i64| u128::from(p.abs_diff(q));
    let (dx, dy) = (d(home.x, lot.x), d(home.y, lot.y));
    let tiles = u64::try_from((dx * dx + dy * dy).isqrt()).unwrap_or(u64::MAX);
    let radius = crate::car_place::SHARED_LOT_RADIUS;
    let over = tiles.saturating_sub(PARKING_NEAR_TILES);
    let span = radius - PARKING_NEAR_TILES;
    over.min(span) * PARKING_FAR_PENALTY / span
}

/// 共同の駐車場の料金の高さで下げる点数。料金 `fee` が 0 なら 0 で、蓄え `savings` の世帯が払える上限
/// （[`fee_cap`]）で [`PARKING_FEE_PENALTY`] になるまで一様に増やす。
/// 上限を超えた世帯は 1 日 1 回の見直しで枠を返すが、それまでは上限と同じだけ下げる。
pub fn parking_fee_penalty(fee: u16, savings: Money) -> u64 {
    let fee = u64::from(fee);
    let cap = u64::try_from(fee_cap(savings)).expect("払える上限は正の値");
    if fee >= cap {
        return PARKING_FEE_PENALTY;
    }
    fee * PARKING_FEE_PENALTY / cap
}

/// 駐車場の点数。自宅の枠があれば満点、共同の駐車場を借りていれば満点から遠さと高さを引き、
/// 枠がなければ（車を持てなければ） [`PARKING_CARLESS`] である。`place` は世帯の枠、`home` は家の出入口のマス、
/// `lot` は借りた共同の駐車場の正面のマス（借りていなければ `None`）である。
pub fn parking_score(
    place: Option<CarPlace>,
    home: TilePos,
    lot: Option<TilePos>,
    fee: u16,
    savings: Money,
) -> u8 {
    match place {
        Some(CarPlace::Home) => HAPPINESS_MAX,
        Some(CarPlace::Shared(_)) => {
            // 借りた駐車場は取り壊すと枠も返すので、`lot` はいつもある。なければ遠さは数えない。
            let far = lot.map_or(0, |l| parking_far_penalty(home, l));
            let down = far + parking_fee_penalty(fee, savings);
            u64::from(HAPPINESS_MAX).saturating_sub(down) as u8
        }
        None => PARKING_CARLESS,
    }
}

/// 4 つの点数を [`WEIGHTS`] で重みを付けて平均した幸福度（`0..=HAPPINESS_MAX`）。
///
/// 生活サービスの点数に応じて上限を設け、0 点なら [`SERVICELESS_CAP`]、満点なら上限なしとし、そのあいだは
/// 一様に上げる。重みを付けて足すだけでは、電気も水道もない街でも職のある世帯が出ていかないためである。
pub fn total(h: HappinessParts) -> u8 {
    let parts = [h.commute, h.services, h.parking, h.wealth];
    let sum: u64 = parts
        .iter()
        .zip(WEIGHTS)
        .map(|(p, w)| u64::from(*p) * w)
        .sum();
    let max = u64::from(HAPPINESS_MAX);
    let floor = u64::from(SERVICELESS_CAP);
    let cap = floor + (max - floor) * u64::from(h.services) / max;
    (sum / WEIGHTS.iter().sum::<u64>()).min(cap) as u8
}

/// 職を得て、電気と水道の届く家に住み、通勤が短い、蓄えが上限の半分の働く年齢の人が越してきたときの幸福度。
/// 移住する人は、この見込みで街を選ぶ。駐車場の点は、街の住宅の世帯の駐車場の点の平均 `parking`
/// （[`crate::world::World::newcomer_parking`]）を見込む。車を持てない世帯の多い街ほど、越してくる人が減る。
pub fn newcomer_happiness(tax_permille: u16, parking: u8) -> u8 {
    total(HappinessParts {
        commute: HAPPINESS_MAX,
        services: HAPPINESS_MAX,
        parking,
        wealth: wealth_score(30, true, Money(SAVINGS_FOR_FULL_BONUS.0 / 2), tax_permille),
    })
}

/// 家 `home` に住む世帯 `household` の駐車場の点数（[`parking_score`]）。世帯の全員が同じ点になる。
pub fn household_parking(ctx: &CitizenContext, household: HouseholdId, home: &Building) -> u8 {
    let place = ctx.car_places.get(household);
    let lot = match place {
        Some(CarPlace::Shared(corner)) => ctx
            .facilities
            .iter()
            .find(|f| f.kind.is_parking() && f.area.min == corner)
            .map(|f| f.front),
        _ => None,
    };
    let savings = ctx
        .population
        .household_money(household)
        .unwrap_or(Money::ZERO);
    parking_score(place, home.front, lot, ctx.parking_fee, savings)
}

/// 住民 1 人の 4 つの点数。
pub fn parts_of(ctx: &CitizenContext, row: &CitizenRow) -> HappinessParts {
    let home = ctx
        .population
        .home(row.household)
        .flatten()
        .and_then(|b| ctx.buildings.get(b));
    let work = row.workplace.and_then(|b| ctx.buildings.get(b));
    let commute = match (home, work) {
        (Some(h), Some(w)) => commute_score(commute_time(row, h, w)),
        _ => HAPPINESS_MAX,
    };
    let half = HAPPINESS_MAX / 2;
    let services = home.map_or(0, |b| {
        u8::from(b.supply.power) * half + u8::from(b.supply.water) * (HAPPINESS_MAX - half)
    });
    let tax = ctx.tax_permille[ZoneKind::Residential.index()];
    let savings = ctx
        .population
        .household_money(row.household)
        .unwrap_or(Money::ZERO);
    // 家のない世帯は車を持てない世帯に数えないので、駐車場の点は満点にする。
    let parking = home.map_or(HAPPINESS_MAX, |b| household_parking(ctx, row.household, b));
    HappinessParts {
        commute,
        services,
        parking,
        wealth: wealth_score(row.age, work.is_some(), savings, tax),
    }
}

/// 組ごとの更新で 1 人の住民に行う処理。点数が変わったときだけ書き戻す。
pub fn update_citizen(
    ctx: &CitizenContext,
    _id: CitizenId,
    row: CitizenRow,
) -> Option<CitizenChange> {
    let happiness = parts_of(ctx, &row);
    (happiness != row.happiness).then_some(CitizenChange {
        schedule: row.schedule,
        happiness,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commute_score_falls_between_the_limits() {
        assert_eq!(commute_score(0), 100);
        assert_eq!(commute_score(COMMUTE_FULL_SECONDS), 100);
        assert_eq!(commute_score(600), 50);
        assert_eq!(commute_score(COMMUTE_ZERO_SECONDS), 0);
        assert_eq!(commute_score(u64::MAX), 0);
    }

    #[test]
    fn wealth_follows_the_job_and_the_tax() {
        let half = Money(50_000);
        assert_eq!(wealth_score(30, true, half, 100), 80);
        assert_eq!(wealth_score(30, false, half, 100), 20);
        assert_eq!(wealth_score(8, false, half, 100), 70);
        assert_eq!(wealth_score(30, true, half, 200), 20);
        assert_eq!(wealth_score(30, true, half, 300), 0);
        assert_eq!(wealth_score(30, true, half, 0), 100);
        // 蓄えは 0 から上限までで 40 点の幅になり、上限より多くても増えない。
        assert_eq!(wealth_score(30, true, Money::ZERO, 100), 60);
        assert_eq!(wealth_score(30, true, Money(250_000), 100), 100);
        assert_eq!(wealth_score(30, true, Money(-5), 100), 60);
    }

    #[test]
    fn newcomers_expect_less_under_a_high_tax() {
        assert_eq!(newcomer_happiness(100, HAPPINESS_MAX), 88);
        assert_eq!(newcomer_happiness(200, HAPPINESS_MAX), 54);
        assert_eq!(newcomer_happiness(300, HAPPINESS_MAX), 42);
    }

    #[test]
    fn newcomers_expect_less_where_households_cannot_keep_a_car() {
        // 街の世帯がみな車を持てなければ、見込みは 10 点下がる。
        assert_eq!(newcomer_happiness(100, PARKING_CARLESS), 78);
        assert_eq!(newcomer_happiness(100, 65), 83);
        assert!(newcomer_happiness(200, PARKING_CARLESS) < newcomer_happiness(200, HAPPINESS_MAX));
    }

    #[test]
    fn total_weights_wealth_most() {
        let h = |commute, services, parking, wealth| HappinessParts {
            commute,
            services,
            parking,
            wealth,
        };
        assert_eq!(total(h(100, 100, 100, 100)), 100);
        assert_eq!(total(h(100, 100, 100, 0)), 42);
        assert_eq!(total(h(0, 100, 100, 100)), 85);
        // 駐車場だけで下がるのは、通勤と同じく最大で 7 分の 1 である。
        assert_eq!(total(h(100, 100, 0, 100)), 85);
        assert_eq!(total(h(100, 100, PARKING_CARLESS, 100)), 90);
        // 生活サービスが 0 点なら 40 点、50 点なら 70 点が上限になる。
        assert_eq!(total(h(100, 0, 100, 100)), 40);
        assert_eq!(total(h(100, 50, 100, 100)), 70);
        assert_eq!(total(h(0, 50, 0, 0)), 7);
        assert_eq!(total(HappinessParts::NEUTRAL), 50);
    }

    #[test]
    fn the_far_penalty_grows_from_near_to_the_edge_of_the_range() {
        let home = TilePos::new(0, 0);
        assert_eq!(parking_far_penalty(home, home), 0);
        assert_eq!(parking_far_penalty(home, TilePos::new(8, 0)), 0);
        assert_eq!(parking_far_penalty(home, TilePos::new(-20, 0)), 20);
        assert_eq!(parking_far_penalty(home, TilePos::new(0, 32)), 40);
        assert_eq!(parking_far_penalty(home, TilePos::new(100, 100)), 40);
        // 斜めは直線の距離で測る（6・8・10 の直角三角形）
        assert_eq!(parking_far_penalty(home, TilePos::new(12, 16)), 20);
    }

    #[test]
    fn the_fee_penalty_grows_from_zero_to_the_cap() {
        let half = Money(SAVINGS_FOR_FULL_BONUS.0 / 2);
        let cap = fee_cap(half);
        assert_eq!(cap, 30);
        // 料金 0 なら下げない。初期値の 10 から下がる
        assert_eq!(parking_fee_penalty(0, half), 0);
        assert_eq!(parking_fee_penalty(10, half), 10);
        assert_eq!(parking_fee_penalty(15, half), 15);
        assert_eq!(parking_fee_penalty(cap as u16, half), 30);
        assert_eq!(parking_fee_penalty(50, half), 30);
        // 料金を上げるほど下がり、下げるほど上がる
        for fee in 0..50 {
            assert!(parking_fee_penalty(fee, half) <= parking_fee_penalty(fee + 1, half));
        }
        // 蓄えの多い世帯ほど、同じ料金で下がる点が小さい
        assert!(parking_fee_penalty(20, Money(90_000)) < parking_fee_penalty(20, Money(10_000)));
        // 蓄えのない世帯は 10 までしか払えず、10 で上限と同じだけ下がる
        assert_eq!(parking_fee_penalty(5, Money::ZERO), 15);
        assert_eq!(parking_fee_penalty(10, Money::ZERO), 30);
        assert_eq!(parking_fee_penalty(11, Money::ZERO), 30);
    }

    #[test]
    fn the_parking_score_follows_the_place() {
        let home = TilePos::new(0, 0);
        let lot = Some(TilePos::new(20, 0));
        let half = Money(SAVINGS_FOR_FULL_BONUS.0 / 2);
        let shared = Some(CarPlace::Shared(TilePos::new(19, 0)));
        assert_eq!(
            parking_score(Some(CarPlace::Home), home, None, 50, half),
            100
        );
        assert_eq!(parking_score(None, home, None, 10, half), PARKING_CARLESS);
        assert_eq!(parking_score(shared, home, lot, 0, half), 80);
        assert_eq!(parking_score(shared, home, lot, 10, half), 70);
        assert_eq!(parking_score(shared, home, lot, 50, half), 50);
        // 範囲の端で上限の料金なら、車を持てない世帯と同じ点になる
        let edge = Some(TilePos::new(32, 0));
        assert_eq!(parking_score(shared, home, edge, 50, half), PARKING_CARLESS);
    }
}
