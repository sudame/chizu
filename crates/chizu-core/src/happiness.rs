//! 住民の幸福度。
//!
//! 住民ごとに通勤・生活サービス・暮らし向きの 3 つの点数（`0..=HAPPINESS_MAX`）を持ち、組ごとの更新で
//! [`update_citizen`] が書き直す。3 つを重みを付けて足したものを幸福度とする（[`total`]）。
//! - 通勤：家から職場までの所要時間。いまは移動がないので、[`commute_seconds`] で直線距離から見積もる。
//!   移動の実時間が測れるようになったら、この関数を差し替える。
//! - 生活サービス：家に電気と水道が届いているか。
//! - 暮らし向き：職があるか（働く年齢でなければ問わない）と、住宅の税率。

use crate::building::{Building, Buildings};
use crate::citizens::{CitizenChange, CitizenRow, HAPPINESS_MAX, HappinessParts, Population, WORKING_AGES};
use crate::command::ZoneKind;
use crate::id::CitizenId;
use crate::network::TILE_MM;
use crate::round;
use crate::units::Tick;

/// 直線距離で見積もるときの、家から職場までの速さ（mm/秒）。道なりの遠回りと待ち時間を込みにした値である。
pub const STRAIGHT_LINE_MM_PER_SECOND: u64 = 6_000;

/// この時間（秒）までの通勤は満点にする。
pub const COMMUTE_FULL_SECONDS: u64 = 300;

/// この時間（秒）以上の通勤は 0 点にする。
pub const COMMUTE_ZERO_SECONDS: u64 = 3_600;

/// 暮らし向きの基準になる住宅の税率（‰）。これより高いと点数が下がり、低いと上がる。
pub const REFERENCE_TAX_PERMILLE: i64 = 100;

/// 暮らし向きの点数の、職のある住民の値。基準の税率のときの値である。
pub const WEALTH_EMPLOYED: i64 = 80;

/// 暮らし向きの点数の、働く年齢でない住民の値。
pub const WEALTH_NOT_WORKING_AGE: i64 = 70;

/// 暮らし向きの点数の、職のない働く年齢の住民の値。
pub const WEALTH_JOBLESS: i64 = 20;

/// 幸福度を足すときの、通勤・生活サービス・暮らし向きの重み。
pub const WEIGHTS: [u64; 3] = [1, 1, 3];

/// 組ごとの更新で、住民 1 人の点数を求めるのに読むもの。
pub struct CitizenContext<'a> {
    pub seed: u64,
    pub tick: Tick,
    pub population: &'a Population,
    pub buildings: &'a Buildings,
    /// 区画の種類ごとの税率（‰）。添え字は [`ZoneKind::index`]。
    pub tax_permille: [u16; 4],
}

/// 家から職場までの所要時間（秒）の見積もり。出入口のマスの直線距離を [`STRAIGHT_LINE_MM_PER_SECOND`] で割る。
pub fn commute_seconds(home: &Building, work: &Building) -> u64 {
    let dx = u128::from(home.front.x.abs_diff(work.front.x));
    let dy = u128::from(home.front.y.abs_diff(work.front.y));
    let tiles = (dx * dx + dy * dy).isqrt();
    let mm = tiles * u128::from(TILE_MM.unsigned_abs());
    u64::try_from(mm / u128::from(STRAIGHT_LINE_MM_PER_SECOND)).unwrap_or(u64::MAX)
}

/// 通勤の点数。[`COMMUTE_FULL_SECONDS`] までは満点で、[`COMMUTE_ZERO_SECONDS`] で 0 点になるまで一様に下がる。
pub fn commute_score(seconds: u64) -> u8 {
    let max = u64::from(HAPPINESS_MAX);
    let over = seconds.saturating_sub(COMMUTE_FULL_SECONDS);
    let span = COMMUTE_ZERO_SECONDS - COMMUTE_FULL_SECONDS;
    (max - (over.min(span) * max / span)) as u8
}

/// 暮らし向きの点数。基準より高い税率は 1‰ ごとに 0.6 点、低い税率は 1‰ ごとに 0.2 点動かす。
pub fn wealth_score(age: u8, employed: bool, tax_permille: u16) -> u8 {
    let base = if !WORKING_AGES.contains(&age) {
        WEALTH_NOT_WORKING_AGE
    } else if employed {
        WEALTH_EMPLOYED
    } else {
        WEALTH_JOBLESS
    };
    let diff = REFERENCE_TAX_PERMILLE - i64::from(tax_permille);
    let shift = if diff < 0 {
        round::div_floor(diff * 3, 5)
    } else {
        round::div_floor(diff, 5)
    };
    (base + shift).clamp(0, i64::from(HAPPINESS_MAX)) as u8
}

/// 3 つの点数を [`WEIGHTS`] で重みを付けて平均した幸福度（`0..=HAPPINESS_MAX`）。
pub fn total(h: HappinessParts) -> u8 {
    let parts = [h.commute, h.services, h.wealth];
    let sum: u64 = parts
        .iter()
        .zip(WEIGHTS)
        .map(|(p, w)| u64::from(*p) * w)
        .sum();
    (sum / WEIGHTS.iter().sum::<u64>()) as u8
}

/// 住民 1 人の 3 つの点数。
pub fn parts_of(ctx: &CitizenContext, row: &CitizenRow) -> HappinessParts {
    let home = ctx
        .population
        .home(row.household)
        .flatten()
        .and_then(|b| ctx.buildings.get(b));
    let work = row.workplace.and_then(|b| ctx.buildings.get(b));
    let commute = match (home, work) {
        (Some(h), Some(w)) => commute_score(commute_seconds(h, w)),
        _ => HAPPINESS_MAX,
    };
    let half = HAPPINESS_MAX / 2;
    let services = home.map_or(0, |b| {
        u8::from(b.supply.power) * half + u8::from(b.supply.water) * (HAPPINESS_MAX - half)
    });
    let tax = ctx.tax_permille[ZoneKind::Residential.index()];
    HappinessParts {
        commute,
        services,
        wealth: wealth_score(row.age, work.is_some(), tax),
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
        assert_eq!(commute_score(1_950), 50);
        assert_eq!(commute_score(COMMUTE_ZERO_SECONDS), 0);
        assert_eq!(commute_score(u64::MAX), 0);
    }

    #[test]
    fn wealth_follows_the_job_and_the_tax() {
        assert_eq!(wealth_score(30, true, 100), 80);
        assert_eq!(wealth_score(30, false, 100), 20);
        assert_eq!(wealth_score(8, false, 100), 70);
        assert_eq!(wealth_score(30, true, 200), 20);
        assert_eq!(wealth_score(30, true, 300), 0);
        assert_eq!(wealth_score(30, true, 0), 100);
    }

    #[test]
    fn total_weights_wealth_most() {
        let h = |commute, services, wealth| HappinessParts {
            commute,
            services,
            wealth,
        };
        assert_eq!(total(h(100, 100, 100)), 100);
        assert_eq!(total(h(100, 100, 0)), 40);
        assert_eq!(total(h(0, 0, 100)), 60);
        assert_eq!(total(HappinessParts::NEUTRAL), 50);
    }
}
