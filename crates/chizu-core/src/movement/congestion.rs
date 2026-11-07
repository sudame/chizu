//! 道路の区間ごとの混み具合。画面のデータレイヤーと `chizu-cli agent map --layer traffic` が使う。
//!
//! 混み具合の段階は読み取りのための値で、状態は変えず、状態ハッシュにも入れない。
//! 経路の費用に足す遅れ（[`Trips::queue_delays`]）も、車線に並んでいる車を数えて求める。
//! 区間の向きごとに、車線に出ている車の台数から、その向きに並べる台数に対する割合（占有率）を求め、
//! 止まっている車の割合と合わせて段階にする。車の台数を数えるだけなので、費用は走っている車の数に比例する。
//! 車のいない区間は表に載せず、空いているものとして扱う。

use std::collections::BTreeMap;

use super::{CAR_LENGTH, MIN_GAP, Trips, signal};
use crate::network::{Dir, LaneMode, LinkId, Network, SegmentId};
use crate::routing::{BAND_TICKS, Delay, Delays};
use crate::units::Tick;

/// 車 1 台が車線の上でふさぐ長さ（mm）。車の長さと、止まったときの最小の車間である。
const SPACING_MM: u64 = (CAR_LENGTH + MIN_GAP) as u64;

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

/// 車の台数、止まっている台数、並べる台数の上限から段階を決める。
/// 並べる上限は `capacity_cars` で求める。短い区間では上限が 1〜2 台になり、それが埋まって止まっていれば詰まりである。
pub fn classify(cars: u64, stopped: u64, capacity: u64) -> Congestion {
    let capacity = capacity.max(1);
    // 車 1 台だけで混雑や詰まりとは言えない。ただし 1 台しか並べない区間では、その 1 台が止まっていれば詰まりである
    if cars < 2 && !(capacity == 1 && cars == 1 && stopped == 1) {
        return Congestion::Free;
    }
    let occupancy = cars * 1000 / capacity;
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

/// リンク（区間の片側）に並べる車の台数の上限。交差点の出口は最後尾まで車 2 台ぶんの空きがないと入れないので、
/// 車線ごとに車線の長さを車 1 台がふさぐ長さで割った台数（切り捨て）を足す。最低 1 台とする。
pub(super) fn capacity_cars(net: &Network, link: LinkId) -> u64 {
    net.link_lanes(link)
        .filter_map(|l| net.lane(l))
        .map(|l| l.length.0.unsigned_abs() / SPACING_MM)
        .sum::<u64>()
        .max(1)
}

/// この速さ（mm/tick）より遅い車を、車線に並んでいる車とみなす。時速 7.2km である。
pub(super) const QUEUED_BELOW: i32 = 2_000;

/// 並んでいる車 1 台が、1 車線のリンクを抜けるまでに足す遅れ（ms）の下限。
/// 交差する流れのある交差点や信号では、車線の先頭から 4 tick に 1 台ほど抜けるので、その見積もりにする。
pub(super) const QUEUE_MS_PER_CAR: u64 = 4_000;

/// 経路の費用で、1 tick にあたる時間（ms）。道路の最高速度（mm/tick）で進む時間を ms で数えるため、
/// 1 tick を 1 秒とみなしている（`routing::graph`）。
pub(super) const MS_PER_TICK: u64 = 1_000;

/// 車線 1 本あたりこの台数より長い列は、1 回の青でさばききれない。信号の青の長さに、下限の見積もりの
/// 1 台あたりの時間で抜けられる台数である。
pub(super) const SATURATED_PER_LANE: u64 = signal::GREEN * MS_PER_TICK / QUEUE_MS_PER_CAR;

/// リンク `link`（車線 `lanes` 本）で、並んでいる車がこの台数より多ければ、さばききれていないとみなす。
/// 1 回の青でさばけない台数（[`SATURATED_PER_LANE`] × 車線）にする。ただし短いリンクにはそれだけ並べないので、
/// 並べる台数の上限が埋まった列もさばききれていないとみなす。止まった 1 台の偶然で決めないよう、2 台より少ない列は数えない。
fn saturated_queue(net: &Network, link: LinkId, lanes: u64) -> u64 {
    (SATURATED_PER_LANE * lanes)
        .min(capacity_cars(net, link) - 1)
        .max(1)
}

impl Trips {
    /// 車のリンクごとの、並んでいる車から見積もった遅れ。時間帯（[`BAND_TICKS`]）ごとに tick `now` で 1 回呼ぶ。
    ///
    /// 並んでいる車 1 台あたりの遅れは、ふつうは [`QUEUE_MS_PER_CAR`] を車線の数で割った長さにする。
    /// ただし、1 回の青でさばききれない長さの列（[`saturated_queue`] 台より多い）があるリンクでは、
    /// 前に見積もってから 1 台がリンクの終わりを抜けて次のリンクへ移るのにかかった時間（数えた時間 ÷ 抜けた台数）が
    /// それより長ければ、その時間にする。抜けた車がいなければ 1 台抜けたものとする。リンクの先の交差点で
    /// 待たされるほど、抜ける台数が減り、遅れが長くなる。列が短いリンクで抜けた台数が少ないのは、さばけないからではなく
    /// 通る車が少ないからなので、測った時間は使わない。追い越しはしないので、行き先の手前で速さを落とした車も、
    /// 後ろの車をふさぐ車として数える。
    ///
    /// 数えた時間は、前に見積もった tick（まだなければ街の始まり）から `now` までである。それが時間帯の半分に
    /// 満たないときと、数え始めたあとに足した区間（道路を分けた直後の区間など）では、測った時間を使わない。
    /// 抜けた台数は見積もるたびに 0 に戻す。遅れの上限は、経路の費用に入れるときに決める（`routing::Graph::set_delays`）。
    ///
    /// 建物の前から出発を待つ車はまだ車線にいないので数えないが、道路の種類が変わって車線に戻るのを
    /// 待つ車は、区間の途中で止まっているので数える。
    pub fn queue_delays(&mut self, net: &Network, now: Tick) -> Delays {
        let exits = core::mem::take(&mut self.exits);
        let since = core::mem::replace(&mut self.counted_since, now);
        let version = core::mem::replace(&mut self.counted_version, net.version());
        let counted_ms = now
            .0
            .checked_sub(since.0)
            .filter(|t| t * 2 >= BAND_TICKS)
            .map(|t| t * MS_PER_TICK);
        let mut queued = BTreeMap::<LinkId, u64>::new();
        for t in self.rows.iter().flatten() {
            let parked = t.waiting && !t.rejoining;
            if t.mode != LaneMode::Car || parked || t.speed.0 >= QUEUED_BELOW {
                continue;
            }
            let Some(lane) = net.lane(t.lane) else {
                continue;
            };
            let link = LinkId {
                segment: lane.segment,
                dir: lane.dir,
                mode: LaneMode::Car,
            };
            *queued.entry(link).or_insert(0) += 1;
        }
        let counted = |link: LinkId| net.segment(link.segment).is_some_and(|s| s.built < version);
        // 忙しさは、抜けた台数を数えた時間のある、数え始める前からある区間だけで求める。
        let busy: Vec<(LinkId, u64)> = counted_ms
            .map(|ms| {
                exits
                    .iter()
                    .filter(|(link, _)| counted(**link))
                    .map(|(link, n)| {
                        let lanes = net.link_lanes(*link).count().max(1) as u64;
                        (
                            *link,
                            u64::from(*n) * QUEUE_MS_PER_CAR * 1_000 / (lanes * ms),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        Delays::from_rows(queued.into_iter().map(|(link, n)| {
            let lanes = net.link_lanes(link).count().max(1) as u64;
            let floor = n * QUEUE_MS_PER_CAR / lanes;
            let counted = counted(link);
            let measured = counted_ms
                .filter(|_| counted && n > saturated_queue(net, link, lanes))
                .map_or(0, |ms| {
                    let left = u64::from(exits.get(&link).copied().unwrap_or(0)).max(1);
                    n * (ms / left)
                });
            let delay = Delay {
                ms: floor.max(measured),
                measured: measured > floor,
            };
            (link, delay)
        }))
        .with_busy(busy)
    }

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
            let level = classify(c.cars, c.stopped, capacity_cars(net, link));
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
    use crate::command::RoadKind;
    use crate::network::Line;
    use crate::tile::TilePos;

    fn link_of(n: &mut Network, from: (i64, i64), to: (i64, i64), kind: RoadKind) -> LinkId {
        let line = Line::between(TilePos::new(from.0, from.1), TilePos::new(to.0, to.1)).unwrap();
        n.build_road(&line, kind).unwrap();
        let segment = n.segments_through(TilePos::new(from.0, from.1))[0];
        LinkId {
            segment,
            dir: Dir::Forward,
            mode: LaneMode::Car,
        }
    }

    #[test]
    fn one_car_is_free_unless_it_fills_a_one_car_road() {
        assert_eq!(classify(1, 1, 22), Congestion::Free);
        assert_eq!(classify(0, 0, 22), Congestion::Free);
        assert_eq!(classify(1, 0, 1), Congestion::Free);
        assert_eq!(classify(1, 1, 1), Congestion::Jammed);
    }

    #[test]
    fn levels_follow_occupancy_and_stopped_share() {
        // 100 マスの 1 車線には 114 台並べる
        let cap = 114;
        assert_eq!(classify(10, 0, cap), Congestion::Free);
        assert_eq!(classify(20, 0, cap), Congestion::Busy);
        assert_eq!(classify(40, 0, cap), Congestion::Heavy);
        // 占有率が高くても、動いている車が半分を超えるなら詰まりとは呼ばない
        assert_eq!(classify(70, 10, cap), Congestion::Heavy);
        assert_eq!(classify(70, 35, cap), Congestion::Jammed);
    }

    #[test]
    fn short_roads_jam_when_filled_and_stopped() {
        // 2 マスの道路には 2 台並べる。埋まって止まれば詰まり、動いていれば混んでいるまで
        assert_eq!(classify(2, 2, 2), Congestion::Jammed);
        assert_eq!(classify(2, 0, 2), Congestion::Heavy);
        assert_eq!(classify(1, 1, 2), Congestion::Free);
    }

    #[test]
    fn capacity_counts_every_lane_and_diagonals() {
        let mut n = Network::new();
        // Small は片側 1 車線、Medium は 2、Large は 3。10 マスは 80,000mm で 11 台
        for (y, kind, want) in [
            (0, RoadKind::Small, 11),
            (10, RoadKind::Medium, 22),
            (20, RoadKind::Large, 33),
        ] {
            let link = link_of(&mut n, (0, y), (10, y), kind);
            assert_eq!(capacity_cars(&n, link), want, "{kind:?}");
        }
        // 斜めの 3 マスは 33,942mm で 4 台
        let link = link_of(&mut n, (0, 40), (3, 43), RoadKind::Small);
        assert_eq!(capacity_cars(&n, link), 4);
        // 1 マスと斜め 1 マスは 1 台
        let link = link_of(&mut n, (0, 60), (1, 60), RoadKind::Small);
        assert_eq!(capacity_cars(&n, link), 1);
        let link = link_of(&mut n, (0, 70), (1, 71), RoadKind::Small);
        assert_eq!(capacity_cars(&n, link), 1);
    }

    /// 遅れを見積もる。`now` は見積もる tick である。
    fn estimate(trips: &mut Trips, n: &Network, now: u64) -> Delays {
        trips.queue_delays(n, crate::units::Tick(now))
    }

    /// 行 `y` の道路のマス `xs` から、同じ行のマス `to` へ向かって出発したばかりの、止まった車を置く。
    fn stopped_cars(trips: &mut Trips, n: &Network, y: i64, xs: core::ops::Range<i64>, to: i64) {
        use crate::SidewalkPos;
        use crate::routing::AstarRouter;
        use crate::units::{LaneOffset, Tick};
        let mut router = AstarRouter::new(n);
        let at = |x: i64| SidewalkPos {
            tile: TilePos::new(x, y),
            step: (1, 0),
            along: LaneOffset(1),
        };
        let drives: Vec<_> = xs.map(|x| (at(x), at(to))).collect();
        let ids = trips.spawn_drives(n, &mut router, &drives, Tick::ZERO);
        assert!(ids.iter().all(Option::is_some));
    }

    #[test]
    fn queued_cars_add_delay_per_lane_and_moving_cars_do_not() {
        use crate::units::MmPerTick;
        let mut n = Network::new();
        let small = link_of(&mut n, (0, 0), (20, 0), RoadKind::Small);
        let medium = link_of(&mut n, (0, 10), (20, 10), RoadKind::Medium);
        let mut trips = Trips::default();
        // 出発したばかりの車は止まっている。小さい道路に 3 台、中くらいの道路（2 車線）に 4 台置く。
        stopped_cars(&mut trips, &n, 0, 16..19, 19);
        stopped_cars(&mut trips, &n, 10, 15..19, 19);
        // 1 回の青でさばける列なので、抜けた台数によらず 1 台あたり 4 秒を車線の数で割る。
        let d = estimate(&mut trips, &n, 2 * BAND_TICKS);
        assert_eq!(d.get(small), 3 * QUEUE_MS_PER_CAR);
        assert_eq!(d.get(medium), 4 * QUEUE_MS_PER_CAR / 2);
        // 走っている車は並んでいるとみなさない。
        for t in trips.rows.iter_mut().flatten() {
            t.speed = MmPerTick(QUEUED_BELOW);
        }
        assert!(estimate(&mut trips, &n, 3 * BAND_TICKS).is_empty());
    }

    #[test]
    fn a_lone_stopped_car_on_a_quiet_link_adds_only_the_floor() {
        let mut n = Network::new();
        let small = link_of(&mut n, (0, 0), (20, 0), RoadKind::Small);
        let mut trips = Trips::default();
        estimate(&mut trips, &n, BAND_TICKS);
        // 時間帯のうちに 1 台も抜けなかった空いた道で、赤信号や譲り合いで 1 台だけ止まっていても、
        // 1 台が抜けるのに時間帯まるごとかかったとはみなさない。
        stopped_cars(&mut trips, &n, 0, 18..19, 19);
        let d = estimate(&mut trips, &n, 2 * BAND_TICKS);
        assert_eq!(d.get(small), QUEUE_MS_PER_CAR);
        // 数台が抜けた道でも同じである。
        trips.exits.insert(small, 2);
        let d = estimate(&mut trips, &n, 3 * BAND_TICKS);
        assert_eq!(d.get(small), QUEUE_MS_PER_CAR);
    }

    #[test]
    fn a_queue_one_green_cannot_clear_adds_the_measured_time_per_car() {
        let mut n = Network::new();
        let small = link_of(&mut n, (0, 0), (20, 0), RoadKind::Small);
        let mut trips = Trips::default();
        estimate(&mut trips, &n, BAND_TICKS);
        // 1 回の青でさばけない長さの列なら、1 台が抜けるのにかかった時間を並んでいる車 1 台ごとに足す。
        let queued = SATURATED_PER_LANE + 1;
        stopped_cars(&mut trips, &n, 0, (19 - queued as i64)..19, 19);
        trips.exits.insert(small, 5);
        let d = estimate(&mut trips, &n, 2 * BAND_TICKS);
        let band_ms = BAND_TICKS * MS_PER_TICK;
        assert_eq!(d.get(small), queued * (band_ms / 5));
        // 抜けた台数は見積もるたびに 0 に戻す。抜けた車がいなければ、時間帯に 1 台抜けたものとして数える。
        assert!(trips.exits.is_empty());
        let d = estimate(&mut trips, &n, 3 * BAND_TICKS);
        assert_eq!(d.get(small), queued * band_ms);
        // 1 秒に 1 台より速く抜けたリンクでは、下限の見積もりになる。
        let many = u32::try_from(2 * BAND_TICKS).expect("台数が u32 に収まらない");
        trips.exits.insert(small, many);
        let d = estimate(&mut trips, &n, 4 * BAND_TICKS);
        assert_eq!(d.get(small), queued * QUEUE_MS_PER_CAR);
    }

    #[test]
    fn the_first_estimate_of_a_new_city_uses_the_floor() {
        let mut n = Network::new();
        let small = link_of(&mut n, (0, 0), (20, 0), RoadKind::Small);
        let mut trips = Trips::default();
        let queued = SATURATED_PER_LANE + 1;
        stopped_cars(&mut trips, &n, 0, (19 - queued as i64)..19, 19);
        // 街の始まりから時間帯の半分しかたっていなければ、抜けた台数を数えた時間が短すぎるので、下限の見積もりにする。
        let d = estimate(&mut trips, &n, BAND_TICKS / 2);
        assert_eq!(d.get(small), queued * QUEUE_MS_PER_CAR);
        // 次の見積もりは、前の見積もりからの時間で割る。
        let d = estimate(&mut trips, &n, BAND_TICKS / 2 + BAND_TICKS);
        assert_eq!(d.get(small), queued * BAND_TICKS * MS_PER_TICK);
    }

    #[test]
    fn links_split_since_the_last_estimate_use_the_floor() {
        let mut n = Network::new();
        link_of(&mut n, (0, 0), (20, 0), RoadKind::Small);
        let mut trips = Trips::default();
        estimate(&mut trips, &n, BAND_TICKS);
        // 交差する道路を足して区間を分けると、分けた 2 本は抜けた台数を 0 から数え直す。
        let line = Line::between(TilePos::new(10, -5), TilePos::new(10, 5)).unwrap();
        n.build_road(&line, RoadKind::Small).unwrap();
        let west = n.segments_through(TilePos::new(5, 0))[0];
        let west = LinkId {
            segment: west,
            dir: Dir::Forward,
            mode: LaneMode::Car,
        };
        let queued = SATURATED_PER_LANE + 1;
        stopped_cars(&mut trips, &n, 0, (10 - queued as i64)..10, 15);
        assert_eq!(n.link_lanes(west).count(), 1);
        // 数え始めてからの時間がわからないので、時間帯まるごとで割らずに下限の見積もりにする。
        let d = estimate(&mut trips, &n, 2 * BAND_TICKS);
        assert_eq!(d.get(west), queued * QUEUE_MS_PER_CAR);
    }

    #[test]
    fn a_short_link_filled_with_a_slow_queue_adds_the_measured_time_per_car() {
        let mut n = Network::new();
        // 4 マスの 1 車線には 4 台しか並べないので、1 回の青でさばけない列の長さには届かない。
        let short = link_of(&mut n, (0, 0), (4, 0), RoadKind::Small);
        link_of(&mut n, (4, 0), (10, 0), RoadKind::Small);
        assert_eq!(capacity_cars(&n, short), 4);
        let mut trips = Trips::default();
        estimate(&mut trips, &n, BAND_TICKS);
        // 並べるだけ並んで抜けが遅ければ、さばききれていないとみなし、測った時間を足す。
        stopped_cars(&mut trips, &n, 0, 0..4, 8);
        trips.exits.insert(short, 5);
        let d = estimate(&mut trips, &n, 2 * BAND_TICKS);
        assert_eq!(d.get(short), 4 * (BAND_TICKS * MS_PER_TICK / 5));
    }

    #[test]
    fn a_short_quiet_link_with_a_stopped_car_adds_only_the_floor() {
        let mut n = Network::new();
        let short = link_of(&mut n, (0, 0), (2, 0), RoadKind::Small);
        link_of(&mut n, (2, 0), (10, 0), RoadKind::Small);
        assert_eq!(capacity_cars(&n, short), 2);
        let mut trips = Trips::default();
        estimate(&mut trips, &n, BAND_TICKS);
        // 2 台しか並べない道でも、1 台止まっているだけでは、抜けた車が少なくても測った時間は使わない。
        stopped_cars(&mut trips, &n, 0, 1..2, 8);
        let d = estimate(&mut trips, &n, 2 * BAND_TICKS);
        assert_eq!(d.get(short), QUEUE_MS_PER_CAR);
    }
}
