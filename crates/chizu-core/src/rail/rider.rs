//! 乗る人（設計 3.10 の 4）。
//!
//! 駅まで歩き終えた人は、歩く移動（`Trip`）を休ませて、駅の待ち行列に入る（[`Rider`]）。移動の ID は休ませた
//! あいだも持ち続け、駅から歩き出すときに同じ ID で歩く移動を作り直す。住民と移動の結び付きを変えずに済む。
//!
//! - 待ち行列は（駅、路線、向き）ごとに、着いた順の並びで持つ。
//! - 列車が駅に着いたら、降りる駅がここの人を降ろし、待ち行列の先頭から定員まで乗せる。駅を出るときも、
//!   着いたあとに並んだ人を定員まで乗せる。
//! - 乗り換える人は、最初の路線を降りた駅で、乗り換え先の路線の待ち行列に入る。乗り換えたときに乗り換え先の
//!   路線で行けなくなっていれば、その駅から歩く。
//! - 降りた人と、待ちきれない人と、路線や駅や列車がなくなって乗れなくなった人は、[`Exit`] として呼ぶ側
//!   （[`World`](crate::world::World)）に渡し、駅の出入口から行き先へ歩く移動に戻してもらう。
//!
//! 路線の向きは、往復する路線では、最初の駅から終点へ向かう側を 0、戻る側を 1 とする。進路の前半が 0、
//! 後半が 1 である。環状の路線は 0 だけである。

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};

use super::line::{RailLine, StepKind};
use super::train::{DWELL_TICKS, Train};
use super::{LineId, Rail, RiderId, StationId, TrainId};
use crate::id::TripId;
use crate::movement::SidewalkPos;
use crate::service::Facility;
use crate::state_hash::{StateHash, StateHasher};
use crate::units::{Money, Tick};

/// 列車の定員（人）。1 両 150 人の 4 両である。
pub const TRAIN_CAPACITY: usize = 600;

/// 駅に止まる時間を延ばすときの、1 tick に乗り降りできる人数。
pub const BOARDINGS_PER_TICK: usize = 20;

/// 待ち行列の先頭でこれより長く待った人は、歩きに切り替える（tick）。
pub const RAIL_GIVE_UP: u64 = 600;

/// 運賃の初期値。
pub const INITIAL_FARE: u16 = 5;

/// 運賃の上限。
pub const MAX_FARE: u16 = 20;

/// 運賃 1 円を、手段を選ぶ費用の秒に直す重み。運賃 5 で 25 秒になる。chizu の街は数百 m の大きさなので、
/// 駅の近くどうしなら 500m 前後から鉄道が車に勝つ重さにした（設計 3.10 の「作る順の 6 で決めた細部」）。
pub const FARE_SECONDS: u64 = 5;

/// 乗り換え。最初の路線を [`RidePlan::alight`] で降り、路線 `line` に向き `dir` で乗り換えて、駅 `alight` で降りる。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transfer {
    pub line: LineId,
    pub dir: u8,
    pub alight: StationId,
}

/// 鉄道で向かう移動の計画。駅 `board` から路線 `line` に向き `dir` で乗り、駅 `alight` で降りて、`to` まで歩く。
/// 乗り換えるときは、`alight` で `transfer` の路線に乗り換え、`transfer` の `alight` で降りて歩く。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RidePlan {
    pub board: StationId,
    pub line: LineId,
    pub dir: u8,
    pub alight: StationId,
    /// 乗り換え。乗り換えは 1 回までである。
    pub transfer: Option<Transfer>,
    /// 最後に歩いて向かう行き先の出入口。
    pub to: SidewalkPos,
}

/// 駅で待っているか、列車に乗っている人。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rider {
    /// 休ませている歩く移動。降りたら同じ ID で歩く移動を作り直す。
    pub trip: TripId,
    /// 最初の区切りの出発の tick。
    pub started: Tick,
    pub plan: RidePlan,
    /// 何回目の乗車か（0 から数える）。
    pub leg: u8,
    /// 最初に待った駅の出入口。1 回目の乗車を待つあいだに乗れなくなったときは、ここから歩く。乗り換える駅で
    /// 待つ人は、その駅の出入口から歩く（[`Rider::waiting_exit`]）。
    pub door: SidewalkPos,
    /// 待ち行列に入った tick。
    pub since: u64,
    /// 乗っている列車。待っているあいだは `None`。
    pub train: Option<TrainId>,
}

impl Rider {
    /// いまの乗車の（乗る駅、路線、向き、降りる駅）。2 回目の乗車は、乗り換える駅から乗り換え先の路線に乗る。
    pub fn leg_route(&self) -> (StationId, LineId, u8, StationId) {
        match (self.leg, self.plan.transfer) {
            (0, _) | (_, None) => (
                self.plan.board,
                self.plan.line,
                self.plan.dir,
                self.plan.alight,
            ),
            (_, Some(t)) => (self.plan.alight, t.line, t.dir, t.alight),
        }
    }

    /// いま並ぶか並んでいる待ち行列。
    pub fn queue_key(&self) -> QueueKey {
        let (s, l, d, _) = self.leg_route();
        (s, l, d)
    }

    /// 待っているあいだに乗れなくなったときに、どこから歩くか。乗り換える駅で待つ人は、その駅から歩く。
    pub fn waiting_exit(&self) -> ExitFrom {
        if self.leg == 0 {
            ExitFrom::Door(self.door)
        } else {
            ExitFrom::Station(self.plan.alight)
        }
    }
}

/// 歩きに戻る人が、どこから歩くか。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitFrom {
    /// この出入口から歩く。
    Door(SidewalkPos),
    /// この駅の出入口から歩く。
    Station(StationId),
    /// この敷地の駅の出入口から歩く。取り壊した駅で乗り換えを待っていた人に使う。
    Site(Facility),
    /// 歩き出せる駅がない。行き先に着いたことにする（3.6 の救済と同じ）。
    Nowhere,
}

/// 鉄道を離れて歩きに戻る人。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Exit {
    pub trip: TripId,
    pub started: Tick,
    pub to: SidewalkPos,
    pub from: ExitFrom,
    /// 列車に乗ったか。待ちきれずに歩いた人と、乗る前に路線がなくなった人は偽である。
    pub rode: bool,
}

/// 乗る人の読み取り用の数。始まりからの積み上げで、状態ハッシュには入れない。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RideTally {
    /// 乗った回数。乗り換えも 1 回に数える。
    pub boarded: u64,
    /// 降りた回数。
    pub alighted: u64,
    /// 待ち行列の先頭で [`RAIL_GIVE_UP`] tick より長く待ち、歩きに切り替えた人。
    pub gave_up: u64,
    /// 路線、駅、列車がなくなって、乗る前か途中で歩きに戻った人。
    pub evicted: u64,
    /// 運賃の収入の合計。
    pub fares: i64,
    /// 路線ごとの乗った回数。
    pub line_boarded: BTreeMap<LineId, u64>,
    /// 駅ごとの、乗った回数と降りた回数。
    pub station_counts: BTreeMap<StationId, (u64, u64)>,
    /// 駅で乗るまでに待った tick の合計と、その人数。
    pub wait_ticks: u64,
    /// 日ごとの運賃の収入。（日、その日、前の日）。
    pub fare_days: (u64, i64, i64),
    /// 乗り換えた回数。最初の路線を降りて、乗り換え先の路線の待ち行列に入った数である。
    pub transfers: u64,
    /// 駅ごとの、その駅で乗り換えた回数。
    pub station_transfers: BTreeMap<StationId, u64>,
    /// 路線ごとの、乗り換えてその路線の待ち行列に入った回数。
    pub line_transfers: BTreeMap<LineId, u64>,
}

impl RideTally {
    /// 日 `day` の、前の日の運賃の収入。
    pub fn fares_yesterday(&self, day: u64) -> i64 {
        let (d, today, yesterday) = self.fare_days;
        if d == day {
            yesterday
        } else if d + 1 == day {
            today
        } else {
            0
        }
    }

    fn add_fare(&mut self, day: u64, fare: i64) {
        let (d, today, _) = self.fare_days;
        if d != day {
            let yesterday = if d + 1 == day { today } else { 0 };
            self.fare_days = (day, 0, yesterday);
        }
        self.fare_days.1 += fare;
        self.fares += fare;
    }
}

/// 待ち行列の鍵。（駅、路線、向き）。
pub type QueueKey = (StationId, LineId, u8);

impl RailLine {
    /// 進路の要素 `i` が、往復の行き（0）と帰り（1）のどちらにあるか。環状の路線は 0 である。
    pub(crate) fn half(&self, i: u32) -> u8 {
        if self.looped {
            0
        } else {
            u8::from(i >= self.len() / 2)
        }
    }

    /// 要素 `i` に着いた列車に乗る人の向き。終点では折り返した先の向きになる。
    pub(crate) fn board_dir_at_arrival(&self, i: u32) -> u8 {
        let h = self.half(i);
        if self.step(i).kind == StepKind::Terminal {
            1 - h
        } else {
            h
        }
    }

    /// 止まる要素の番号を、進路の順に並べたもの。
    pub(crate) fn stops(&self) -> Vec<u32> {
        (0..self.len())
            .filter(|i| self.step(*i).is_stop())
            .collect()
    }

    /// 要素 `i` が、止まる要素の何番目か。
    pub(crate) fn stop_order(&self, i: u32) -> Option<usize> {
        if !self.step(i).is_stop() {
            return None;
        }
        Some((0..i).filter(|j| self.step(*j).is_stop()).count())
    }

    /// 駅 `from` で向き `dir` に乗り、駅 `to` で降りられるなら、乗る所と降りる所の、止まる要素の順の番号。
    pub(crate) fn direct(&self, from: StationId, to: StationId, dir: u8) -> Option<(usize, usize)> {
        if from == to {
            return None;
        }
        let stops = self.stops();
        let m = stops.len();
        let k = (0..m).find(|k| {
            let i = stops[*k];
            self.step(i).station == Some(from) && self.board_dir_at_arrival(i) == dir
        })?;
        for d in 1..m {
            let j = (k + d) % m;
            let s = self.step(stops[j]);
            if s.station == Some(to) {
                return Some((k, j));
            }
            if s.kind == StepKind::Terminal {
                return None;
            }
        }
        None
    }
}

/// 止まる要素ごとの、そこに着いてから次に止まる要素に着くまでの時間（tick）の見込み。止まる時間と減速と加速の遅れ、
/// 終点の折り返しの時間を含む（[`RailLine::round_trip_ticks`] を止まる要素ごとに分けたもの）。
pub(crate) fn estimate_segments(line: &RailLine) -> Vec<u32> {
    let stops = line.stops();
    let m = stops.len();
    let mut out = Vec::with_capacity(m);
    for k in 0..m {
        let (a, b) = (stops[k], stops[(k + 1) % m]);
        let mut t = line.stop_ticks(a);
        let mut i = a;
        loop {
            i = line.next(i);
            t += line.run_ticks(i);
            if i == b {
                break;
            }
        }
        out.push(u32::try_from(t).unwrap_or(u32::MAX));
    }
    out
}

impl Rail {
    /// 乗る人を ID の順に返す。
    pub fn riders(&self) -> impl Iterator<Item = (RiderId, &Rider)> {
        self.riders.iter()
    }

    pub fn rider(&self, id: RiderId) -> Option<&Rider> {
        self.riders.get(id)
    }

    /// 乗る人の数。
    pub fn rider_count(&self) -> usize {
        self.riders.iter().count()
    }

    /// 休ませている移動 `trip` の乗る人。
    pub fn rider_of(&self, trip: TripId) -> Option<RiderId> {
        self.by_trip.get(&trip).copied()
    }

    /// 駅の待ち行列。（駅、路線、向き）の順に返す。
    pub fn queues(&self) -> impl Iterator<Item = (&QueueKey, &VecDeque<RiderId>)> {
        self.queues.iter()
    }

    /// 列車 `id` に乗っている人。
    pub fn aboard(&self, id: TrainId) -> &[RiderId] {
        self.aboard.get(&id).map_or(&[], Vec::as_slice)
    }

    /// 直前に乗りきれずに人が残った（駅、路線、向き）。
    pub fn left_behind(&self) -> &BTreeSet<QueueKey> {
        &self.left_behind
    }

    /// 運賃。
    pub fn fare(&self) -> u16 {
        self.fare
    }

    pub(crate) fn set_fare(&mut self, fare: u16) {
        self.fare = fare.min(MAX_FARE);
    }

    /// 乗る人の読み取り用の数。
    pub fn ride_tally(&self) -> &RideTally {
        &self.ride_tally
    }

    /// 歩きに戻る人を取り出す。
    pub(crate) fn take_exits(&mut self) -> Vec<Exit> {
        std::mem::take(&mut self.exits)
    }

    /// この前に取り出してから集めた運賃を取り出す。
    pub(crate) fn take_fares(&mut self) -> Money {
        Money(std::mem::take(&mut self.fare_due))
    }

    /// 駅 `from` から路線 `line` に向き `dir` で乗り、駅 `to` で降りられるか。駅と路線があり、路線が止まっておらず、
    /// 同じ向きで 2 つの駅を結ぶときである。
    fn can_ride(&self, from: StationId, line: LineId, dir: u8, to: StationId) -> bool {
        self.stations.get(from).is_some()
            && self
                .lines
                .get(line)
                .is_some_and(|l| !l.stopped && l.direct(from, to, dir).is_some())
    }

    /// 乗り換え `t` を、駅 `at` で乗り換えて使えるか。
    fn can_transfer(&self, at: StationId, t: &Transfer) -> bool {
        self.can_ride(at, t.line, t.dir, t.alight)
    }

    /// 駅まで歩き終えた人を、駅の待ち行列に入れる。乗れない計画（駅、路線がない、止まった路線、向きの合わない駅）
    /// なら、その場で歩きに戻す。乗り換える計画は、乗り換え先の路線にも乗れなければ歩きに戻す。
    pub(crate) fn add_rider(
        &mut self,
        trip: TripId,
        started: Tick,
        plan: RidePlan,
        door: SidewalkPos,
        now: u64,
    ) {
        let rider = Rider {
            trip,
            started,
            plan,
            leg: 0,
            door,
            since: now,
            train: None,
        };
        let id = self.riders.insert(rider);
        self.by_trip.insert(trip, id);
        let ok = self.can_ride(plan.board, plan.line, plan.dir, plan.alight)
            && plan
                .transfer
                .is_none_or(|t| self.can_transfer(plan.alight, &t));
        if ok {
            self.queues
                .entry((plan.board, plan.line, plan.dir))
                .or_default()
                .push_back(id);
        } else {
            self.ride_tally.evicted += 1;
            self.evict(id, ExitFrom::Door(door), false);
        }
    }

    /// 休ませている移動 `trip` の乗る人を、歩きに戻さずに消す。住民の移動を取りやめるときに使う。
    pub(crate) fn drop_rider(&mut self, trip: TripId) {
        let Some(id) = self.by_trip.remove(&trip) else {
            return;
        };
        let Some(r) = self.riders.remove(id) else {
            return;
        };
        match r.train {
            Some(t) => {
                if let Some(list) = self.aboard.get_mut(&t) {
                    list.retain(|x| *x != id);
                    if list.is_empty() {
                        self.aboard.remove(&t);
                    }
                }
            }
            None => {
                let key = r.queue_key();
                if let Some(q) = self.queues.get_mut(&key) {
                    q.retain(|x| *x != id);
                    if q.is_empty() {
                        self.queues.remove(&key);
                    }
                }
            }
        }
    }

    /// 乗る人 `id` を消して、`from` から歩きに戻す。待ち行列と列車の並びからは、呼ぶ側が外しておく。
    /// 読み取り用の数は呼ぶ側で数える。
    fn evict(&mut self, id: RiderId, from: ExitFrom, rode: bool) {
        let r = self.riders.remove(id).expect("乗る人がいない");
        self.by_trip.remove(&r.trip);
        self.exits.push(Exit {
            trip: r.trip,
            started: r.started,
            to: r.plan.to,
            from,
            rode,
        });
    }

    /// 列車を消す。乗っていた人は、列車が止まっている駅か次に止まる駅（なければ列車のいる線路にいちばん近い駅）で
    /// 降りたことにする。
    pub(crate) fn drop_train(&mut self, id: TrainId) {
        self.drop_train_on(id, None);
    }

    /// [`Rail::drop_train`] と同じだが、`old` があれば、乗っていた人の降りる駅をその路線の進路で探す。進路を替えた
    /// 直後に新しい進路に置けずに消える列車は、位置が前の進路の番号のままなので、前の進路を渡す。
    pub(crate) fn drop_train_on(&mut self, id: TrainId, old: Option<&RailLine>) {
        let Some(t) = self.trains.get(id).copied() else {
            return;
        };
        self.last_stop.remove(&id);
        let riders = self.aboard.remove(&id).unwrap_or_default();
        if !riders.is_empty() {
            let from = self
                .drop_station(&t, old)
                .map_or(ExitFrom::Nowhere, ExitFrom::Station);
            for r in riders {
                self.ride_tally.evicted += 1;
                self.evict(r, from, true);
            }
        }
        self.trains.remove(id);
    }

    /// 消える列車の乗客が降りる駅。いま止まっている駅、次に止まる駅、列車のいる線路にいちばん近い駅の順に探す。
    /// 進路は `old` があればそれを、なければ列車の路線のいまの進路を使う。
    fn drop_station(&self, t: &Train, old: Option<&RailLine>) -> Option<StationId> {
        if let Some(line) = old.or_else(|| self.lines.get(t.line))
            && (t.pos as usize) < line.path.len()
        {
            let here = line.step(t.pos);
            let alive = |s: Option<StationId>| s.filter(|s| self.stations.get(*s).is_some());
            // 要素の先の端にいて駅があれば、その駅に止まっている。終点で折り返したあとの要素（`depart` の
            // `Pass`）も駅を持つ。
            if t.offset >= here.len
                && let Some(s) = alive(here.station)
            {
                return Some(s);
            }
            for i in stops_ahead(line, t) {
                if let Some(s) = alive(line.step(i).station) {
                    return Some(s);
                }
            }
        }
        let at = self
            .tracks
            .get(t.track)
            .and_then(|tr| self.nodes.get(tr.a))
            .map(|n| n.tile)?;
        self.stations
            .iter()
            .map(|(id, s)| {
                let c = s.area.min;
                let (dx, dy) = (i128::from(c.x - at.x), i128::from(c.y - at.y));
                (dx * dx + dy * dy, id)
            })
            .min()
            .map(|(_, id)| id)
    }

    /// 列車 `train` に、待ち行列 `key` の先頭から定員まで乗せ、乗せた人数を返す。
    fn board(&mut self, train: TrainId, key: QueueKey, now: u64) -> usize {
        let Some(q) = self.queues.get_mut(&key) else {
            return 0;
        };
        let list = self.aboard.entry(train).or_default();
        let mut n = 0;
        let day = now / crate::clock::TICKS_PER_DAY;
        while list.len() < TRAIN_CAPACITY {
            let Some(id) = q.pop_front() else {
                break;
            };
            let r = self.riders.get_mut(id).expect("乗る人がいない");
            r.train = Some(train);
            list.push(id);
            n += 1;
            self.ride_tally.boarded += 1;
            self.ride_tally.wait_ticks += now.saturating_sub(r.since);
            *self.ride_tally.line_boarded.entry(key.1).or_default() += 1;
            self.ride_tally.station_counts.entry(key.0).or_default().0 += 1;
            // 乗り換えても運賃は 1 回分である。
            if r.leg == 0 {
                let fare = i64::from(self.fare);
                self.fare_due += fare;
                self.ride_tally.add_fare(day, fare);
            }
        }
        if list.is_empty() {
            self.aboard.remove(&train);
        }
        if q.is_empty() {
            self.queues.remove(&key);
        }
        n
    }

    /// 列車 `id` が止まる要素に着いたときの乗り降り。降りる駅がここの人を降ろし、待っている人を乗せ、
    /// 止まる時間（tick）を返す。乗り降りが多ければ、1 tick に [`BOARDINGS_PER_TICK`] 人ずつで延ばす。
    /// 終点では、乗っている人をみな降ろす。区間の時間も測る。
    pub(crate) fn serve_arrival(&mut self, id: TrainId, now: u64, board: bool) -> u16 {
        let t = *self.trains.get(id).expect("列車がない");
        let line = self.lines.get(t.line).expect("列車の路線がない");
        let step = *line.step(t.pos);
        let k = line.stop_order(t.pos);
        let board_dir = line.board_dir_at_arrival(t.pos);
        let stops = line.stops().len();
        // 前に止まった所から、ここまでの時間を 1/4 の重みで混ぜる。
        if let Some(k) = k {
            if let Some((prev, at)) = self.last_stop.get(&id).copied()
                && (prev + 1) % stops == k
            {
                let took = u32::try_from(now.saturating_sub(at)).unwrap_or(u32::MAX);
                let line = self.lines.get_mut(t.line).expect("列車の路線がない");
                if let Some(seg) = line.seg_ticks.get_mut(prev) {
                    *seg = mix(*seg, took);
                }
            }
            self.last_stop.insert(id, (k, now));
        }
        let Some(s) = step.station else {
            return DWELL_TICKS;
        };
        let terminal = step.kind == StepKind::Terminal;
        let aboard = self.aboard.remove(&id).unwrap_or_default();
        let (out, stay): (Vec<RiderId>, Vec<RiderId>) = aboard
            .into_iter()
            .partition(|r| terminal || self.riders.get(*r).is_some_and(|x| x.leg_route().3 == s));
        let alighted = out.len();
        for r in out {
            self.ride_tally.alighted += 1;
            self.ride_tally.station_counts.entry(s).or_default().1 += 1;
            let x = *self.riders.get(r).expect("乗る人がいない");
            match x.plan.transfer {
                Some(t) if x.leg == 0 && x.plan.alight == s => self.transfer(r, s, &t, now),
                _ => self.evict(r, ExitFrom::Station(s), true),
            }
        }
        if !stay.is_empty() {
            self.aboard.insert(id, stay);
        }
        let boarded = if board {
            self.board(id, (s, t.line, board_dir), now)
        } else {
            0
        };
        let busy = (alighted + boarded).div_ceil(BOARDINGS_PER_TICK);
        DWELL_TICKS.max(u16::try_from(busy).unwrap_or(u16::MAX))
    }

    /// 乗る人 `id` を、駅 `at` で乗り換え `t` の路線の待ち行列に入れる。乗り換え先の路線で行けなくなっていれば、
    /// その駅から歩く。
    fn transfer(&mut self, id: RiderId, at: StationId, t: &Transfer, now: u64) {
        if !self.can_transfer(at, t) {
            self.ride_tally.evicted += 1;
            self.evict(id, ExitFrom::Station(at), true);
            return;
        }
        let r = self.riders.get_mut(id).expect("乗る人がいない");
        r.leg = 1;
        r.train = None;
        r.since = now;
        self.queues
            .entry((at, t.line, t.dir))
            .or_default()
            .push_back(id);
        let tally = &mut self.ride_tally;
        tally.transfers += 1;
        *tally.station_transfers.entry(at).or_default() += 1;
        *tally.line_transfers.entry(t.line).or_default() += 1;
    }

    /// 取り壊す駅 `id`（敷地 `site`）で乗り換えを待っている人を、駅を消す前に、その敷地の出入口から歩かせる。
    /// 駅を消したあとでは、乗り換える駅の出入口を求められないためである。
    pub(crate) fn evict_transfer_waiters(&mut self, id: StationId, site: Facility) {
        let keys: Vec<QueueKey> = self.queues.keys().filter(|k| k.0 == id).copied().collect();
        for key in keys {
            let q = self.queues.remove(&key).unwrap_or_default();
            let (gone, keep): (VecDeque<RiderId>, VecDeque<RiderId>) = q
                .into_iter()
                .partition(|r| self.riders.get(*r).is_some_and(|x| x.leg > 0));
            for r in gone {
                self.ride_tally.evicted += 1;
                self.evict(r, ExitFrom::Site(site), false);
            }
            if !keep.is_empty() {
                self.queues.insert(key, keep);
            }
        }
    }

    /// 列車 `id` が駅を出るときに、着いたあとに並んだ人を定員まで乗せる。乗りきれずに人が残ったかを覚える。
    pub(crate) fn serve_departure(&mut self, id: TrainId, now: u64) {
        let t = *self.trains.get(id).expect("列車がない");
        let line = self.lines.get(t.line).expect("列車の路線がない");
        let Some(s) = line.step(t.pos).station else {
            return;
        };
        let key = (s, t.line, line.half(t.pos));
        self.board(id, key, now);
        if self.queues.contains_key(&key) {
            self.left_behind.insert(key);
        } else {
            self.left_behind.remove(&key);
        }
    }

    /// 待ち行列の先頭で [`RAIL_GIVE_UP`] tick より長く待った人を、歩きに切り替える。
    pub(crate) fn give_up(&mut self, now: u64) {
        let keys: Vec<QueueKey> = self.queues.keys().copied().collect();
        for key in keys {
            while let Some(q) = self.queues.get_mut(&key) {
                let Some(&head) = q.front() else {
                    self.queues.remove(&key);
                    break;
                };
                let r = self.riders.get(head).expect("乗る人がいない");
                if now.saturating_sub(r.since) <= RAIL_GIVE_UP {
                    break;
                }
                q.pop_front();
                let from = r.waiting_exit();
                self.ride_tally.gave_up += 1;
                self.evict(head, from, false);
            }
        }
    }

    /// 路線か線路が変わったあとに、乗れなくなった人を歩きに戻す。待っている人は、駅か路線がなくなったか、
    /// 路線が止まったか、乗る駅と降りる駅を同じ向きで結ばなくなったら、待っていた駅の出入口から歩く。
    /// 乗り換える計画の人は、最初の路線を待つあいだに乗り換え先の路線で行けなくなっても歩く。
    /// 乗っている人は、乗っている列車が降りる駅に着かなくなったら（降りる駅が路線になくなったか、終点より先になった）、
    /// 列車が止まっている駅か次に止まる駅で降りたことにする。乗り換え先の路線で行けなくなった人も、乗り換える駅に
    /// 着くなら乗り続け、乗り換える駅から歩く（[`Rail::transfer`]）。
    pub(crate) fn settle_riders(&mut self) {
        let keys: Vec<QueueKey> = self.queues.keys().copied().collect();
        for key in keys {
            let (s, l, d) = key;
            let q = self.queues.remove(&key).unwrap_or_default();
            let mut keep = VecDeque::new();
            let mut gone = Vec::new();
            for r in q {
                let x = self.riders.get(r).expect("乗る人がいない");
                let ok = self.can_ride(s, l, d, x.leg_route().3)
                    && (x.leg > 0
                        || x.plan
                            .transfer
                            .is_none_or(|t| self.can_transfer(x.plan.alight, &t)));
                if ok {
                    keep.push_back(r);
                } else {
                    gone.push(r);
                }
            }
            for r in gone {
                let from = self.riders.get(r).expect("乗る人がいない").waiting_exit();
                self.ride_tally.evicted += 1;
                self.evict(r, from, false);
            }
            if !keep.is_empty() {
                self.queues.insert(key, keep);
            }
        }
        let trains: Vec<TrainId> = self.aboard.keys().copied().collect();
        for id in trains {
            let Some(t) = self.trains.get(id).copied() else {
                continue;
            };
            let Some(line) = self.lines.get(t.line) else {
                continue;
            };
            let list = self.aboard.remove(&id).unwrap_or_default();
            let (stay, out): (Vec<RiderId>, Vec<RiderId>) = list.into_iter().partition(|r| {
                let alight = self.riders.get(*r).expect("乗る人がいない").leg_route().3;
                reaches(line, &t, alight)
            });
            if !out.is_empty() {
                let from = self
                    .drop_station(&t, None)
                    .map_or(ExitFrom::Nowhere, ExitFrom::Station);
                for r in out {
                    self.ride_tally.evicted += 1;
                    self.evict(r, from, true);
                }
            }
            if !stay.is_empty() {
                self.aboard.insert(id, stay);
            }
        }
        let lines = &self.lines;
        self.left_behind.retain(|(_, l, _)| lines.get(*l).is_some());
        let trains = &self.trains;
        self.last_stop.retain(|t, _| trains.get(*t).is_some());
    }
}

/// 列車 `t` がこの先に止まる要素の番号を、進路の順に 1 周まで返す。止まっている要素は含めず、走っていて
/// まだ先の端に着いていない要素は含める。
fn stops_ahead<'a>(line: &'a RailLine, t: &Train) -> impl Iterator<Item = u32> + 'a {
    let n = line.len();
    let start = if t.offset < line.step(t.pos).len {
        t.pos
    } else {
        line.next(t.pos)
    };
    (0..n)
        .map(move |k| (start + k) % n)
        .filter(move |i| line.step(*i).is_stop())
}

/// 列車 `t` に乗ったまま、駅 `to` で降りられるか。終点ではみな降りるので、終点の先の駅には着かない。
fn reaches(line: &RailLine, t: &Train, to: StationId) -> bool {
    if (t.pos as usize) >= line.path.len() {
        return false;
    }
    for i in stops_ahead(line, t) {
        let s = line.step(i);
        if s.station == Some(to) {
            return true;
        }
        if s.kind == StepKind::Terminal {
            return false;
        }
    }
    false
}

/// 前の値 `old` に、測った値 `new` を 1/4 の重みで混ぜる。端数は四捨五入する。
fn mix(old: u32, new: u32) -> u32 {
    let v = (u64::from(old) * 3 + u64::from(new) + 2) / 4;
    u32::try_from(v).unwrap_or(u32::MAX)
}

impl StateHash for Transfer {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.line);
        h.write(&self.dir);
        h.write(&self.alight);
    }
}

impl StateHash for RidePlan {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.board);
        h.write(&self.line);
        h.write(&self.dir);
        h.write(&self.alight);
        h.write(&self.transfer);
        h.write(&self.to);
    }
}

impl StateHash for Rider {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.trip);
        h.write(&self.started);
        h.write(&self.plan);
        h.write(&self.leg);
        h.write(&self.door);
        h.write(&self.since);
        h.write(&self.train);
    }
}

/// 待ち行列を状態ハッシュに書く。
pub(crate) fn write_queues(q: &BTreeMap<QueueKey, VecDeque<RiderId>>, h: &mut StateHasher) {
    h.write_len(q.len());
    for ((s, l, d), ids) in q {
        h.write(s);
        h.write(l);
        h.write(d);
        h.write_len(ids.len());
        for id in ids {
            h.write(id);
        }
    }
}
