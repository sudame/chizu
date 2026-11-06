//! 時間帯ごとの、車で行く時間の見込み。
//!
//! 鉄道と比べるときの車の費用は、直線の距離に、出発の時間帯の見込み（直線 1km あたりの移動の秒）を掛けて求める。
//! 見込みは、車で出た通勤の移動が着いたときに、かかった時間を直線の距離で割った値を、出発の時間帯ごとに 1 日分
//! 平均し、日が変わったら前の見込みと半分ずつ混ぜる。朝の混む時間帯は見込みが長くなり、混む時間帯に車で出る人の
//! 一部が鉄道に移る。CS2 の住民が、いまの道路の混み具合を経路の費用に入れて手段を選ぶのに寄せたものである。
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

/// 1 件の移動から測る値の上限。[`DEFAULT_PACE`] の何倍までにするか。詰まりの救済で長く止まった移動など、
/// 1 件で平均を大きく動かす値を抑える。
const MAX_PACE_FACTOR: u64 = 10;

/// 時間帯ごとの、車で行く時間の見込みと、その日に測った値。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CarPace {
    /// 測っている日。
    day: u64,
    /// 時間帯ごとの見込み（直線 1km あたりの移動の秒）。
    expected: [u64; PACE_SLOTS],
    /// その日に測った値の和と件数。
    sum: [u64; PACE_SLOTS],
    count: [u64; PACE_SLOTS],
}

impl Default for CarPace {
    fn default() -> CarPace {
        CarPace {
            day: 0,
            expected: [DEFAULT_PACE; PACE_SLOTS],
            sum: [0; PACE_SLOTS],
            count: [0; PACE_SLOTS],
        }
    }
}

/// tick `t` の時間帯。
fn slot(t: Tick) -> usize {
    usize::try_from(t.0 % TICKS_PER_DAY / SLOT_TICKS).expect("時間帯があふれた")
}

impl CarPace {
    /// 日が変わっていれば、前の日に測った値を時間帯ごとに平均し、見込みと半分ずつ混ぜる。
    /// 測った値のない時間帯の見込みは変えない。
    pub fn roll(&mut self, now: Tick) {
        let day = now.0 / TICKS_PER_DAY;
        if day <= self.day {
            return;
        }
        for i in 0..PACE_SLOTS {
            if let Some(mean) = self.sum[i].checked_div(self.count[i]) {
                self.expected[i] = (self.expected[i] + mean) / 2;
            }
            self.sum[i] = 0;
            self.count[i] = 0;
        }
        self.day = day;
    }

    /// `departed` に出て `arrived` に着いた、直線で `mm` の車の移動を測った値に足す。
    pub fn learn(&mut self, departed: Tick, arrived: Tick, mm: u64) {
        self.roll(arrived);
        if mm == 0 {
            return;
        }
        let took = arrived.0.saturating_sub(departed.0);
        let pace = (took * 1_000_000 / mm).min(DEFAULT_PACE * MAX_PACE_FACTOR);
        let i = slot(departed);
        self.sum[i] += pace;
        self.count[i] += 1;
    }

    /// `at` に出て、直線で `mm` を車で行く時間の見込み（移動の秒）。
    pub fn seconds(&self, mm: u64, at: Tick) -> u64 {
        mm * self.expected[slot(at)] / 1_000_000
    }
}

impl StateHash for CarPace {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.day);
        h.write(&self.expected);
        h.write(&self.sum);
        h.write(&self.count);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: u64 = SLOT_TICKS;

    #[test]
    fn before_measuring_it_matches_the_straight_line_speed() {
        let p = CarPace::default();
        assert_eq!(p.seconds(1_000_000, Tick(7 * HOUR)), DEFAULT_PACE);
        assert_eq!(p.seconds(2_800, Tick(0)), 0);
    }

    #[test]
    fn a_slow_morning_raises_only_the_morning_from_the_next_day() {
        let mut p = CarPace::default();
        let day = TICKS_PER_DAY;
        // 1 日目の 7 時台に、直線 1km を 900 秒かかった移動が 2 件、1km を 700 秒が 1 件。
        p.learn(Tick(day + 7 * HOUR), Tick(day + 7 * HOUR + 900), 1_000_000);
        p.learn(
            Tick(day + 7 * HOUR + 10),
            Tick(day + 7 * HOUR + 910),
            1_000_000,
        );
        p.learn(
            Tick(day + 7 * HOUR + 20),
            Tick(day + 7 * HOUR + 720),
            1_000_000,
        );
        // 同じ日のうちは見込みを変えない。
        assert_eq!(p.seconds(1_000_000, Tick(day + 8 * HOUR)), DEFAULT_PACE);
        p.roll(Tick(2 * day));
        let mean = (900 + 900 + 700) / 3;
        assert_eq!(
            p.seconds(1_000_000, Tick(2 * day + 7 * HOUR)),
            (DEFAULT_PACE + mean) / 2
        );
        assert_eq!(
            p.seconds(1_000_000, Tick(2 * day + 12 * HOUR)),
            DEFAULT_PACE
        );
        // 測った値は日ごとに捨てるので、次の日に測らなければそのまま残る。
        p.roll(Tick(3 * day));
        assert_eq!(
            p.seconds(1_000_000, Tick(3 * day + 7 * HOUR)),
            (DEFAULT_PACE + mean) / 2
        );
    }

    #[test]
    fn one_stuck_trip_moves_the_mean_by_a_bounded_amount() {
        let mut p = CarPace::default();
        p.learn(Tick(7 * HOUR), Tick(7 * HOUR + 2_000), 150_000);
        p.roll(Tick(TICKS_PER_DAY));
        assert_eq!(
            p.expected[7],
            (DEFAULT_PACE + DEFAULT_PACE * MAX_PACE_FACTOR) / 2
        );
    }

    #[test]
    fn the_slot_is_taken_from_the_departure() {
        let mut p = CarPace::default();
        // 23 時台に出て、次の日に着いた移動は 23 時台に入る。
        let day = TICKS_PER_DAY;
        p.learn(Tick(day - 10), Tick(day + 290), 1_000_000);
        p.roll(Tick(2 * day));
        assert_eq!(p.expected[23], (DEFAULT_PACE + 300) / 2);
        assert_eq!(p.expected[0], DEFAULT_PACE);
    }
}
