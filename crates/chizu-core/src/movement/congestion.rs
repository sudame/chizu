//! 道路の区間ごとの混み具合。画面のデータレイヤーと `chizu-cli agent map --layer traffic` が使う。
//!
//! 読み取りのための値で、状態は変えず、状態ハッシュにも入れない。
//! 区間の向きごとに、車線に出ている車の台数から、車線の長さを車で埋めたときの割合（占有率）を求め、
//! 止まっている車の割合と合わせて段階にする。車の台数を数えるだけなので、費用は走っている車の数に比例する。
//! 車のいない区間は表に載せず、空いているものとして扱う。

use std::collections::BTreeMap;

use super::{CAR_LENGTH, MIN_GAP, Trips};
use crate::network::{Dir, LaneMode, LinkId, Network, SegmentId};

/// 車 1 台が車線の上でふさぐ長さ（mm）。車の長さと、止まったときの最小の車間である。
const SPACING_MM: u64 = (CAR_LENGTH + MIN_GAP) as u64;

/// 占有率の下限が小さすぎる短い区間で、車 1〜2 台が「詰まっている」に見えないよう、
/// 区間の長さをこの台数ぶんの長さより短くは数えない。
const MIN_CAPACITY_CARS: u64 = 6;

/// 段階の区切り（占有率の千分率）。
const BUSY_PERMILLE: u64 = 120;
const HEAVY_PERMILLE: u64 = 300;
const JAMMED_PERMILLE: u64 = 500;

/// 区間の混み具合の段階。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Congestion {
    /// 空いている。
    #[default]
    Free = 0,
    /// やや混んでいる。
    Busy = 1,
    /// 混んでいる。
    Heavy = 2,
    /// 詰まっている。車が多く、半分以上が止まっている。
    Jammed = 3,
}

impl Congestion {
    pub const ALL: [Congestion; 4] = [
        Congestion::Free,
        Congestion::Busy,
        Congestion::Heavy,
        Congestion::Jammed,
    ];
}

/// 区間の向きひとつぶんの、車線の上の車の数え上げ。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Count {
    cars: u64,
    stopped: u64,
}

/// 車の台数、止まっている台数、車線の長さ（mm）から段階を決める。
pub fn classify(cars: u64, stopped: u64, lane_mm: u64) -> Congestion {
    // 車 1 台だけでは、詰まりも混雑も言えない
    if cars < 2 {
        return Congestion::Free;
    }
    let capacity = lane_mm.max(MIN_CAPACITY_CARS * SPACING_MM);
    let occupancy = cars * SPACING_MM * 1000 / capacity;
    if occupancy >= JAMMED_PERMILLE && stopped * 2 >= cars {
        Congestion::Jammed
    } else if occupancy >= HEAVY_PERMILLE {
        Congestion::Heavy
    } else if occupancy >= BUSY_PERMILLE {
        Congestion::Busy
    } else {
        Congestion::Free
    }
}

impl Trips {
    /// 車のいる区間の混み具合。区間の 2 つの向きのうち、混んでいるほうの段階を取る。
    /// 空いている区間は載せない。
    pub fn congestion(&self, net: &Network) -> BTreeMap<SegmentId, Congestion> {
        let mut counts = BTreeMap::<(SegmentId, Dir), Count>::new();
        for (_, t) in self.iter() {
            if t.mode != LaneMode::Car || t.waiting {
                continue;
            }
            let Some(lane) = net.lane(t.lane) else {
                continue;
            };
            let c = counts.entry((lane.segment, lane.dir)).or_default();
            c.cars += 1;
            c.stopped += u64::from(t.speed.0 == 0);
        }
        let mut out = BTreeMap::<SegmentId, Congestion>::new();
        for ((segment, dir), c) in counts {
            let link = LinkId {
                segment,
                dir,
                mode: LaneMode::Car,
            };
            let lane_mm: u64 = net
                .link_lanes(link)
                .filter_map(|l| net.lane(l))
                .map(|l| l.length.0.unsigned_abs())
                .sum();
            let level = classify(c.cars, c.stopped, lane_mm);
            if level > Congestion::Free {
                let e = out.entry(segment).or_default();
                *e = (*e).max(level);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TILE: u64 = 8_000;

    #[test]
    fn a_lone_car_never_makes_a_road_busy() {
        assert_eq!(classify(1, 1, TILE), Congestion::Free);
        assert_eq!(classify(0, 0, TILE), Congestion::Free);
    }

    #[test]
    fn levels_follow_occupancy_and_stopped_share() {
        // 長さ 100 マスの 1 車線。車 1 台が 7m ふさぐので、100 マスなら 114 台で満杯である
        let len = 100 * TILE;
        assert_eq!(classify(10, 0, len), Congestion::Free);
        assert_eq!(classify(20, 0, len), Congestion::Busy);
        assert_eq!(classify(40, 0, len), Congestion::Heavy);
        // 占有率が高くても、動いている車が半分を超えるなら詰まりとは呼ばない
        assert_eq!(classify(70, 10, len), Congestion::Heavy);
        assert_eq!(classify(70, 35, len), Congestion::Jammed);
        // 短い区間は、数えられる長さに下限がある
        assert_eq!(classify(2, 2, TILE), Congestion::Heavy);
        assert_eq!(classify(3, 3, TILE), Congestion::Jammed);
    }
}
