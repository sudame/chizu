//! 移動と鉄道のつなぎ（設計 3.10 の 4）。
//!
//! - 手段を選ぶときは、駅の組の表（[`RailPlanner`]）といまの路線の様子から鉄道の候補を作る。表は路線か線路か
//!   道路が変わったときに捨て、次に使うときに作り直す。表は状態から作り直せるので、書き出さない。
//! - 駅の出入口に着いて休ませた移動を、駅の待ち行列に入れる。
//! - 鉄道を離れる人は、駅の出入口から行き先へ歩く移動に戻す。歩き出せる駅がなければ、着いたことにする。
//! - 集めた運賃を所持金に入れる。

use serde::{Deserialize, Serialize};

use super::World;
use crate::id::TripId;
use crate::movement::{Via, facility_spot};
use crate::rail::ExitFrom;
use crate::rail::choice::{RailPlanner, RideChoice};

/// 駅の組の表を置く所。状態に入れず、`World` どうしを比べるときも見ない。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct PlannerSlot {
    #[serde(skip)]
    planner: Option<RailPlanner>,
}

impl PartialEq for PlannerSlot {
    fn eq(&self, _: &PlannerSlot) -> bool {
        true
    }
}

impl Eq for PlannerSlot {}

impl PlannerSlot {
    /// 表を捨てる。路線か線路か道路が変わったときに呼ぶ。
    pub(super) fn forget(&mut self) {
        self.planner = None;
    }
}

impl World {
    /// 駅の組の表を、路線があれば作っておく。
    fn ensure_planner(&mut self) {
        if self.rail.lines().next().is_none() {
            self.planner.planner = None;
            return;
        }
        if self.planner.planner.is_none() {
            self.planner.planner = Some(RailPlanner::build(&self.rail, &self.network));
        }
    }

    /// 駅の組の表。路線がなければ `None`。
    pub fn rail_planner(&mut self) -> Option<&RailPlanner> {
        self.ensure_planner();
        self.planner.planner.as_ref()
    }

    /// 移動の選び方に使う鉄道の候補を作る準備をして、`f` に渡す。路線がなければ `None` を渡す。
    pub(super) fn with_rides<T>(
        &mut self,
        f: impl FnOnce(&mut World, Option<&RideChoice>) -> T,
    ) -> T {
        self.ensure_planner();
        let planner = self.planner.planner.take();
        let choice = planner.as_ref().map(|p| RideChoice::new(p, &self.rail));
        let out = f(self, choice.as_ref());
        drop(choice);
        if self.planner.planner.is_none() {
            self.planner.planner = planner;
        }
        out
    }

    /// 駅の出入口に着いた人を、駅の待ち行列に入れる。
    pub(super) fn board_riders(&mut self) {
        let now = self.tick.0;
        for b in self.trips.take_boarding() {
            self.rail.add_rider(b.trip, b.started, b.plan, b.door, now);
        }
    }

    /// 移動 `trip` を取りやめる。駅で待っているか列車に乗っているなら、鉄道からも外す。
    pub(super) fn cancel_trip(&mut self, trip: TripId) {
        self.trips.cancel(trip);
        self.rail.drop_rider(trip);
    }

    /// 集めた運賃を所持金に入れ、鉄道を離れる人を歩く移動に戻す。
    pub(super) fn release_rail_exits(&mut self) {
        let fares = self.rail.take_fares();
        self.money = self.money + fares;
        let exits = self.rail.take_exits();
        if exits.is_empty() {
            return;
        }
        self.sync_router();
        let mut walks = Vec::new();
        let mut ended = Vec::new();
        for e in exits {
            let via = if e.rode { Via::Rail } else { Via::Walk };
            let from = match e.from {
                ExitFrom::Door(p) => Some(p),
                ExitFrom::Station(s) => self
                    .rail
                    .station(s)
                    .and_then(|st| facility_spot(&self.network, &st.site())),
                ExitFrom::Nowhere => None,
            };
            match from {
                Some(from) => walks.push((e.trip, from, e.to, e.started, via)),
                // 歩き出せる駅がない。3.6 の救済と同じく、行き先に着いたことにする。
                None => {
                    self.trips.finish_suspended(e.trip, true, e.started, via);
                    ended.push(e.trip);
                }
            }
        }
        let ok = self
            .trips
            .resume_walks(&self.network, &mut self.router.router, &walks);
        for ((trip, _, _, started, via), ok) in walks.iter().zip(ok) {
            // 行き先へ歩けなければ、途中で消えた移動と同じく、着かずに終える。
            if !ok && self.trips.is_suspended(*trip) {
                self.trips.finish_suspended(*trip, false, *started, *via);
                ended.push(*trip);
            }
        }
        ended.sort_unstable();
        self.end_commutes(ended);
    }
}

#[cfg(test)]
mod tests {
    use crate::rail::rider_tests::rail_town;

    /// 住民の移動を取りやめると（予定を立て直す、世帯が街を出る）、駅で待つ人と列車に乗る人も鉄道から消える。
    #[test]
    fn cancelling_a_trip_removes_its_rider() {
        let (mut w, _, _, _) = rail_town(5, 3);
        // 待つ人（0）と乗る人（1）を、それぞれ 1 回ずつ取りやめる。
        let mut seen = [false, false];
        for _ in 0..3 * crate::TICKS_PER_DAY {
            w.step();
            if seen.iter().all(|s| *s) {
                break;
            }
            let pick = w.rail().riders().find_map(|(_, r)| {
                let k = usize::from(r.train.is_some());
                (!seen[k]).then_some((k, r.trip))
            });
            let Some((k, trip)) = pick else {
                continue;
            };
            let riders = w.rail().rider_count();
            w.cancel_trip(trip);
            let rail = w.rail();
            assert_eq!(
                rail.rider_of(trip),
                None,
                "取りやめた移動の人が鉄道に残った"
            );
            assert_eq!(rail.rider_count(), riders - 1);
            let listed = rail.queues().map(|(_, q)| q.len()).sum::<usize>()
                + rail
                    .trains()
                    .map(|(t, _)| rail.aboard(t).len())
                    .sum::<usize>();
            assert_eq!(
                listed,
                rail.rider_count(),
                "待ち行列か列車の並びに消えた人が残った"
            );
            assert!(!w.trips().is_suspended(trip));
            seen[k] = true;
        }
        assert!(seen.iter().all(|s| *s), "待つ人か乗る人がいない: {seen:?}");
    }
}
