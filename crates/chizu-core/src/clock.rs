//! ゲーム内の時計。
//!
//! 1 日は 7,200 tick で、1 tick はゲーム内の 12 秒にあたる。tick 0 は 0 日目の 0 時 0 分である。

use crate::units::Tick;

/// 1 日の tick 数。
pub const TICKS_PER_DAY: u64 = 7_200;

/// 1 tick のゲーム内の秒数。
pub const GAME_SECONDS_PER_TICK: u32 = 12;

/// 1 日のゲーム内の秒数。
pub const GAME_SECONDS_PER_DAY: u32 = 86_400;

const _: () = assert!(TICKS_PER_DAY * GAME_SECONDS_PER_TICK as u64 == GAME_SECONDS_PER_DAY as u64);

/// 等速のときに実時間の 1 秒で進める tick 数。
pub const TICKS_PER_SECOND_AT_NORMAL_SPEED: u32 = 30;

/// ゲーム内の日と時刻。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct GameTime {
    /// 0 から数えた日。
    pub day: u64,
    /// その日の 0 時からの秒数（`0..86_400`）。
    pub second_of_day: u32,
}

impl GameTime {
    /// `tick` の頭の時刻。
    pub fn at(tick: Tick) -> GameTime {
        let tick_of_day = tick.0 % TICKS_PER_DAY;
        GameTime {
            day: tick.0 / TICKS_PER_DAY,
            // tick_of_day < 7,200 なので u32 に収まり、12 倍しても 86,400 未満である。
            second_of_day: tick_of_day as u32 * GAME_SECONDS_PER_TICK,
        }
    }

    pub fn hour(self) -> u32 {
        self.second_of_day / 3_600
    }

    pub fn minute(self) -> u32 {
        self.second_of_day / 60 % 60
    }

    pub fn second(self) -> u32 {
        self.second_of_day % 60
    }

    /// その日の何 tick 目か。
    pub fn tick_of_day(self) -> u64 {
        u64::from(self.second_of_day / GAME_SECONDS_PER_TICK)
    }
}

impl Tick {
    /// この tick の頭のゲーム内の時刻。
    pub fn game_time(self) -> GameTime {
        GameTime::at(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn known_times() {
        let t = GameTime::at(Tick::ZERO);
        assert_eq!((t.day, t.hour(), t.minute(), t.second()), (0, 0, 0, 0));

        // 5 tick = 60 秒。
        let t = Tick(5).game_time();
        assert_eq!((t.day, t.hour(), t.minute(), t.second()), (0, 0, 1, 0));

        // 300 tick = 1 時間。
        let t = Tick(300 * 7 + 6).game_time();
        assert_eq!((t.day, t.hour(), t.minute(), t.second()), (0, 7, 1, 12));

        let t = Tick(TICKS_PER_DAY - 1).game_time();
        assert_eq!((t.day, t.hour(), t.minute(), t.second()), (0, 23, 59, 48));

        let t = Tick(TICKS_PER_DAY * 3 + 3_600).game_time();
        assert_eq!((t.day, t.hour(), t.minute(), t.second()), (3, 12, 0, 0));
    }

    #[test]
    fn last_tick_does_not_overflow() {
        let t = Tick(u64::MAX).game_time();
        assert_eq!(t.day, u64::MAX / TICKS_PER_DAY);
        assert_eq!(t.tick_of_day(), u64::MAX % TICKS_PER_DAY);
    }

    proptest! {
        #[test]
        fn game_time_roundtrips_to_tick(tick in 0_u64..(1_u64 << 50)) {
            let t = Tick(tick).game_time();
            prop_assert!(t.second_of_day < GAME_SECONDS_PER_DAY);
            prop_assert_eq!(t.day * TICKS_PER_DAY + t.tick_of_day(), tick);
            prop_assert_eq!(t.hour() * 3_600 + t.minute() * 60 + t.second(), t.second_of_day);
        }
    }
}
