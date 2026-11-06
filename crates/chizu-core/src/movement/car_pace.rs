//! 時間帯ごとの、車で行く時間の見込み。
//!
//! 鉄道と比べるときの車の費用は、直線の距離に、出発の時間帯の見込み（直線 1km あたりの移動の秒）を掛けて求める。
//! 見込みは、車で出た通勤の移動のうち鉄道と比べる移動と同じ条件のものについて、車に乗って走った時間（車の区切りを
//! 始めてから駐車場に着くまで。車線に出られずに歩きに切り替えた移動は、着くまで）を測り、出発の時間帯ごとに「走った時間の和 ÷ 直線の距離の和」で 1 日分まとめる。
//! 日が変わったら、その値に既定の見込み（[`DEFAULT_PACE`]）を直線 [`PRIOR_MM`] 分の重みで混ぜ、前の見込みと
//! 半分ずつ混ぜる。朝の混む時間帯は見込みが長くなり、混む時間帯に車で出る人の一部が鉄道に移る。
//! 測る移動のない時間帯は、日ごとに既定の見込みへ半分ずつ戻る。駐車場まで歩く分は測らず、比べるときに
//! 停める手間（`Trips::parking_seconds`）として足す。
//!
//! 見込みは状態に入れ、状態ハッシュにも入れる。手段の選び方に使うので、読み戻した街でも同じ値でなければならない。
//! 足し合わせる順は着いた移動の順（同じ tick なら移動の ID の順）で、整数だけで計算する。

use serde::{Deserialize, Serialize};

use crate::clock::TICKS_PER_DAY;
use crate::rail::choice::CAR_STRAIGHT_LINE_MM_PER_SECOND;
use crate::state_hash::{StateHash, StateHasher};
use crate::units::Tick;

/// 見込みを分ける時間帯の数。1 時間ごとである。
pub const PACE_SLOTS: usize = 24;

/// 1 つの時間帯の tick 数。
const SLOT_TICKS: u64 = TICKS_PER_DAY / PACE_SLOTS as u64;

/// 直線 1km あたりの移動の秒の、測る前の見込み。[`CAR_STRAIGHT_LINE_MM_PER_SECOND`] から求める。
pub const DEFAULT_PACE: u64 = 1_000_000 / CAR_STRAIGHT_LINE_MM_PER_SECOND;

/// 1 日分を見込みに混ぜるときに、既定の見込みを何 mm 分の直線の距離として足すか。20km 分である。
/// 測った移動が少ない日に、数件で見込みが跳ねないようにする。朝の山は 1 日に数百 km 分を測るので、ほとんど効かない。
pub const PRIOR_MM: u64 = 20_000_000;

/// 1 件の移動から測る時間の上限。直線の距離を [`DEFAULT_PACE`] の何倍で走った時間までにするか。詰まりの救済で
/// 長く止まった移動など、1 件で和を大きく動かす値を抑える。
const MAX_PACE_FACTOR: u64 = 10;

/// 日が変わったのにまとめなかった日が続いたとき、測る移動のない日として戻す日数の上限。戻し続けると既定の見込みに
/// 十分近づくので、それより先は回さない。
const MAX_IDLE_DAYS: u64 = 32;

/// 時間帯ごとの、車で行く時間の見込みと、その日に測った値。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CarPace {
    /// 測っている日。
    day: u64,
    /// 時間帯ごとの見込み（直線 1km あたりの移動の秒）。
    expected: [u64; PACE_SLOTS],
    /// その日に測った、走った時間（移動の秒）の和と、直線の距離（mm）の和。
    took: [u64; PACE_SLOTS],
    mm: [u64; PACE_SLOTS],
}

impl Default for CarPace {
    fn default() -> CarPace {
        CarPace {
            day: 0,
            expected: [DEFAULT_PACE; PACE_SLOTS],
            took: [0; PACE_SLOTS],
            mm: [0; PACE_SLOTS],
        }
    }
}

/// tick `t` の時間帯。
fn slot(t: Tick) -> usize {
    usize::try_from(t.0 % TICKS_PER_DAY / SLOT_TICKS).expect("時間帯があふれた")
}

impl CarPace {
    /// 日が変わっていれば、前の日に測った値を時間帯ごとにまとめて見込みに混ぜる。まとめなかった日が挟まって
    /// いれば、その日は測る移動のない日として混ぜる。
    pub fn roll(&mut self, now: Tick) {
        let day = now.0 / TICKS_PER_DAY;
        if day <= self.day {
            return;
        }
        let days = (day - self.day).min(MAX_IDLE_DAYS);
        for _ in 0..days {
            for i in 0..PACE_SLOTS {
                // 既定の見込みを PRIOR_MM 分の距離として足した、距離で重みづけした平均。
                let took = self.took[i] * 1_000_000 + DEFAULT_PACE * PRIOR_MM;
                let mean = took / (self.mm[i] + PRIOR_MM);
                self.expected[i] = (self.expected[i] + mean) / 2;
                self.took[i] = 0;
                self.mm[i] = 0;
            }
        }
        self.day = day;
    }

    /// `departed` に出た、直線で `mm` の移動が、車に乗って `took` 秒走ったことを、`now` に測った値に足す。
    pub fn learn(&mut self, departed: Tick, now: Tick, (mm, took): (u64, u64)) {
        self.roll(now);
        if mm == 0 {
            return;
        }
        let i = slot(departed);
        self.took[i] += took.min(mm * DEFAULT_PACE * MAX_PACE_FACTOR / 1_000_000);
        self.mm[i] += mm;
    }

    /// `at` に出て、直線で `mm` を車で走る時間の見込み（移動の秒）。
    pub fn seconds(&self, mm: u64, at: Tick) -> u64 {
        mm * self.expected[slot(at)] / 1_000_000
    }

    /// 時間帯 `slot` の見込み（直線 1km あたりの移動の秒）。テストで見る。
    #[cfg(test)]
    pub(super) fn expected(&self, slot: usize) -> u64 {
        self.expected[slot]
    }
}

impl StateHash for CarPace {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.day);
        h.write(&self.expected);
        h.write(&self.took);
        h.write(&self.mm);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: u64 = SLOT_TICKS;
    const KM: u64 = 1_000_000;

    /// 既定の見込みを混ぜた、距離の和 `mm` と時間の和 `took` の 1 日分を、前の見込み `before` と混ぜた値。
    fn blended(before: u64, mm: u64, took: u64) -> u64 {
        (before + (took * 1_000_000 + DEFAULT_PACE * PRIOR_MM) / (mm + PRIOR_MM)) / 2
    }

    #[test]
    fn before_measuring_it_matches_the_straight_line_speed() {
        let p = CarPace::default();
        assert_eq!(p.seconds(KM, Tick(7 * HOUR)), DEFAULT_PACE);
        assert_eq!(p.seconds(2_800, Tick(0)), 0);
    }

    #[test]
    fn the_mean_is_weighted_by_distance() {
        let mut p = CarPace::default();
        let day = TICKS_PER_DAY;
        // 7 時台に、0.3km を 150 秒と、3km を 960 秒。
        p.learn(Tick(day + 7 * HOUR), Tick(day + 8 * HOUR), (300_000, 150));
        p.learn(Tick(day + 7 * HOUR), Tick(day + 8 * HOUR), (3 * KM, 960));
        // 同じ日のうちは見込みを変えない。
        assert_eq!(p.seconds(KM, Tick(day + 9 * HOUR)), DEFAULT_PACE);
        p.roll(Tick(2 * day));
        assert_eq!(p.expected(7), blended(DEFAULT_PACE, 3_300_000, 1_110));
        // 測っていない時間帯は既定の見込みのまま。
        assert_eq!(p.expected(12), DEFAULT_PACE);
    }

    #[test]
    fn a_busy_hour_follows_the_measured_pace() {
        let mut p = CarPace::default();
        // 毎日 7 時台に、直線 1km を 800 秒で 500 件走ると、見込みは 800 秒に近づく。
        for d in 0..10 {
            for _ in 0..500 {
                let at = Tick(d * TICKS_PER_DAY + 7 * HOUR);
                p.learn(at, at, (KM, 800));
            }
        }
        p.roll(Tick(10 * TICKS_PER_DAY));
        assert!((780..=800).contains(&p.expected(7)), "{}", p.expected(7));
    }

    #[test]
    fn one_trip_on_a_quiet_day_moves_the_pace_little() {
        let mut p = CarPace::default();
        // 5 時台に、150m を詰まって長く止まった 1 件だけ。
        p.learn(Tick(5 * HOUR), Tick(5 * HOUR + 1_000), (150_000, 100_000));
        p.roll(Tick(TICKS_PER_DAY));
        let capped = 150_000 * DEFAULT_PACE * MAX_PACE_FACTOR / 1_000_000;
        assert_eq!(p.expected(5), blended(DEFAULT_PACE, 150_000, capped));
        assert!(p.expected(5) < DEFAULT_PACE * 2, "{}", p.expected(5));
    }

    #[test]
    fn an_hour_without_drives_drifts_back_to_the_default() {
        let mut p = CarPace::default();
        for _ in 0..200 {
            p.learn(Tick(7 * HOUR), Tick(7 * HOUR), (KM, 1_500));
        }
        p.roll(Tick(TICKS_PER_DAY));
        let high = p.expected(7);
        assert!(high > 2 * DEFAULT_PACE, "{high}");
        // 次の日から 7 時台に車で出る人がいなくなると、日ごとに既定へ半分ずつ戻る。
        p.roll(Tick(2 * TICKS_PER_DAY));
        assert_eq!(p.expected(7), (high + DEFAULT_PACE) / 2);
        // まとめなかった日が挟まっても、挟まった日の分だけ戻る。
        p.roll(Tick(12 * TICKS_PER_DAY));
        assert!(p.expected(7) <= DEFAULT_PACE + 2, "{}", p.expected(7));
    }

    #[test]
    fn the_slot_is_taken_from_the_departure() {
        let mut p = CarPace::default();
        // 23 時台に出て、次の日に測った移動は 23 時台に入る。
        let day = TICKS_PER_DAY;
        p.roll(Tick(day));
        p.learn(Tick(day - 10), Tick(day + 290), (KM, 300));
        p.roll(Tick(2 * day));
        assert_eq!(p.expected(23), blended(DEFAULT_PACE, KM, 300));
        assert_eq!(p.expected(0), DEFAULT_PACE);
    }
}
