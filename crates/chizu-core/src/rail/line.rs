//! 路線（設計 3.10 の 3）。
//!
//! 路線は駅の並びと、そのあいだの線路の並び（進路）を持つ。進路は（線路、進む向き）の組を頂点にし、
//! ノードで 45 度までの曲がりだけを辺にした図の上で、最短の道のりを探して選ぶ。列車は途中の駅で向きを変えないので、
//! 向きを変えなければ届かない駅は路線に入れられない（[`DenyReason::RailNoRoute`]）。
//!
//! 進路は 1 周ぶんの（線路、向き）の並び（[`Step`]）で持ち、列車はこの並びを先頭から順にたどって、
//! 最後の次は先頭に戻る。往復する路線は、行きの進路のあとに、行きを逆にたどった帰りの進路を続ける。
//! 終点の駅では、同じ駅の線路を逆の向きにした次の [`Step`] へ、走らずに移る（折り返し）。
//! 環状の路線は、最初の駅に戻る進路で 1 周を閉じる。

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

use serde::{Deserialize, Serialize};

use super::train::{
    Block, DWELL_TICKS, TRAIN_ACCEL, TRAIN_BLOCK_LEN_MM, TRAIN_DECEL, TRAIN_MAX_SPEED,
    TURNBACK_TICKS,
};
use super::{LineId, Rail, RailNodeId, StationId, TrackId, heading_of, spread};
use crate::network::Dir;
use crate::round;
use crate::state_hash::{StateHash, StateHasher};
use crate::world::DenyReason;

/// 街に置ける路線の数。
pub const MAX_LINES: usize = 64;

/// 1 つの路線に入れられる駅の数。
pub const MAX_LINE_STATIONS: usize = 32;

/// 1 つの路線を走らせられる列車の本数の上限。行き詰まりを防ぐ上限（[`RailLine::cap`]）がこれより小さければ、そちらに抑える。
pub const MAX_LINE_TRAINS: u8 = 16;

/// 駅に止まって出るまでに、止まる時間のほかに減速と加速で余分にかかる時間（tick）の見込み。
/// 最高の速さから止まるまでと、止まってから最高の速さに戻るまでに、最高の速さのまま走るより遅れる分である。
const STOP_LOSS_TICKS: u64 = {
    let decel = (TRAIN_MAX_SPEED + 2 * TRAIN_DECEL - 1) / (2 * TRAIN_DECEL);
    let accel = (TRAIN_MAX_SPEED + 2 * TRAIN_ACCEL - 1) / (2 * TRAIN_ACCEL);
    (decel + accel) as u64
};

/// 進路の 1 つの要素での列車のふるまい。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum StepKind {
    /// 止まらずに通る。
    Pass = 0,
    /// 路線の途中の駅。先の端で止まり、乗り降りしてから同じ向きに出る。
    Stop = 1,
    /// 往復する路線の終点。先の端で止まり、次の要素（同じ駅の線路の逆の向き）へ折り返す。
    Terminal = 2,
}

/// 進路の 1 つの要素。線路 1 本を 1 つの向きに走る。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub track: TrackId,
    pub dir: Dir,
    /// 線路の長さ（mm）。
    pub len: i64,
    pub kind: StepKind,
    /// 止まる駅か、折り返して入る駅。通り過ぎる駅の線路では `None` である。
    pub station: Option<StationId>,
    /// 入るところのノードが分岐なら、そのノード。分岐は上りと下りをまとめて 1 つの閉塞にする。
    /// 折り返して入る要素では、ノードを通らないので `None` である。
    pub junction: Option<RailNodeId>,
    /// 入るところで、曲がるか分岐を通るので時速 40km まで落とす。
    pub slow: bool,
    /// 列車が駅を出る要素か。途中の駅（`Stop`）と、折り返して入る要素である。
    pub depart: bool,
    /// 出る要素から次に止まる駅のホームまでに、ほかの路線と共有する閉塞があるか。`depart` のときだけ意味がある。
    /// 共有するなら、次の駅のホームまでをまとめて押さえてから出る（行き詰まりを防ぐ決まりの 3）。
    /// ほかの路線の進路が変わると変わるので、路線を作る、変える、なくすたびに、すべての路線で決め直す。
    pub shared: bool,
}

impl Step {
    /// 要素に入るときに押さえる閉塞。分岐のノード、線路の向きの順である。
    pub fn blocks(&self) -> impl Iterator<Item = Block> {
        self.junction
            .map(Block::Junction)
            .into_iter()
            .chain([Block::Track(self.track, self.dir)])
    }

    pub fn is_stop(&self) -> bool {
        self.kind != StepKind::Pass
    }
}

/// 路線。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RailLine {
    /// 止まる駅の順。環状の路線では、最後の駅の次は最初の駅である。
    pub stations: Vec<StationId>,
    pub looped: bool,
    /// 遊ぶ人が決めた本数（0〜16）。走らせるのは [`RailLine::allowed`] までである。
    pub trains: u8,
    /// 1 周の進路。先頭は最初の駅の、列車が現れて出る要素である。
    pub path: Vec<Step>,
    /// 線路が変わって進路を選べない区間ができた。止まった路線の列車は駅を出ず、新しい列車も現れない。
    /// 進路はそれまでのものを残す。
    pub stopped: bool,
    /// 出る要素の番号ごとの、最後に列車が出た tick。進路を選び直したら消す。
    pub(crate) departed: BTreeMap<u32, u64>,
}

impl RailLine {
    /// 行き詰まりを防ぐ本数の上限（決まりの 1）。16 と「輪の中の閉塞の数 − 1」の小さいほうである。
    ///
    /// 輪の中の閉塞は、列車がまるごと入って待てる閉塞（同じものは 1 つに数える）である。終点の両方のホームは、
    /// まとめて押さえて入る（決まりの 2）ので 1 つに数える。次のものは数えない。
    /// - 列車が押さえる長さ（[`TRAIN_BLOCK_LEN_MM`]）より短い線路。列車がまるごと入れない。
    /// - ほかの路線と共有する区間（[`Step::shared`]）の途中の閉塞。駅のホームまでをまとめて押さえて走り抜けるので、
    ///   列車はそこで待たない（決まりの 3）。
    /// - 分岐のノード。列車は止まれない。
    pub fn cap(&self) -> u8 {
        let mut through = vec![false; self.path.len()];
        for i in 0..self.len() {
            let s = self.step(i);
            if s.depart && s.shared {
                let stop = self.next_stop(self.next(i));
                let mut j = self.next(i);
                while j != stop {
                    through[j as usize] = true;
                    j = self.next(j);
                }
            }
        }
        let places: BTreeSet<(TrackId, Dir)> = self
            .path
            .iter()
            .zip(&through)
            .filter(|(s, through)| !**through && s.len >= TRAIN_BLOCK_LEN_MM)
            .map(|(s, _)| (s.track, s.dir))
            .collect();
        let terminals = self
            .path
            .iter()
            .filter(|s| s.kind == StepKind::Terminal)
            .count();
        let room = places.len().saturating_sub(terminals).saturating_sub(1);
        u8::try_from(room.min(usize::from(MAX_LINE_TRAINS))).expect("上限は 16 以下")
    }

    /// 走らせる本数。遊ぶ人が決めた本数を、行き詰まりを防ぐ上限に抑えたものである。
    pub fn allowed(&self) -> u8 {
        self.trains.min(self.cap())
    }

    /// 1 周の見込みの時間（tick）。進路を最高の速さで走る時間に、止まる駅ごとの止まる時間と減速と加速の遅れ、
    /// 終点ごとの折り返しの時間を足す。
    pub fn round_trip_ticks(&self) -> u64 {
        self.path
            .iter()
            .map(|s| {
                let run =
                    u64::try_from(round::div_ceil(s.len, TRAIN_MAX_SPEED)).expect("線路の長さが負");
                let stop = match s.kind {
                    StepKind::Pass => 0,
                    StepKind::Stop => u64::from(DWELL_TICKS) + STOP_LOSS_TICKS,
                    StepKind::Terminal => {
                        u64::from(DWELL_TICKS) + STOP_LOSS_TICKS + u64::from(TURNBACK_TICKS)
                    }
                };
                run + stop
            })
            .sum()
    }

    /// 運転間隔の見込み（tick）。1 周の見込みの時間を走らせる本数で割る。列車がなければ `None`。
    pub fn headway_ticks(&self) -> Option<u64> {
        let n = u64::from(self.allowed());
        (n > 0).then(|| self.round_trip_ticks() / n)
    }

    /// 駅を出る列車が、同じ要素から前の列車が出てから空ける tick。運転間隔の半分である。
    pub(crate) fn departure_gap(&self) -> u64 {
        self.headway_ticks().map_or(0, |h| h / 2)
    }

    /// 進路の要素の数。
    pub(crate) fn len(&self) -> u32 {
        u32::try_from(self.path.len()).expect("進路が長すぎる")
    }

    pub(crate) fn next(&self, i: u32) -> u32 {
        (i + 1) % self.len()
    }

    pub(crate) fn prev(&self, i: u32) -> u32 {
        (i + self.len() - 1) % self.len()
    }

    /// `from` から `to` へ進む要素の数（0 から 要素の数 − 1）。
    pub(crate) fn distance(&self, from: u32, to: u32) -> u32 {
        (to + self.len() - from) % self.len()
    }

    pub(crate) fn step(&self, i: u32) -> &Step {
        &self.path[i as usize]
    }

    /// `from` から先で最初に止まる要素。
    pub(crate) fn next_stop(&self, from: u32) -> u32 {
        let mut i = from;
        for _ in 0..self.len() {
            if self.step(i).is_stop() {
                return i;
            }
            i = self.next(i);
        }
        panic!("止まる駅のない路線");
    }
}

/// （線路、向き）の頂点。
type Vertex = (TrackId, Dir);

fn flip(d: Dir) -> Dir {
    match d {
        Dir::Forward => Dir::Backward,
        Dir::Backward => Dir::Forward,
    }
}

impl Rail {
    /// 路線を ID の順に返す。
    pub fn lines(&self) -> impl Iterator<Item = (LineId, &RailLine)> {
        self.lines.iter()
    }

    pub fn line(&self, id: LineId) -> Option<&RailLine> {
        self.lines.get(id)
    }

    /// 線路 `t` を向き `d` に進むときに入るノードと出るノード。
    fn ends(&self, (t, d): Vertex) -> (RailNodeId, RailNodeId) {
        let tr = &self.tracks[t];
        match d {
            Dir::Forward => (tr.a, tr.b),
            Dir::Backward => (tr.b, tr.a),
        }
    }

    /// ノード `n` から線路 `t` が出ていく向き。
    fn arm(&self, n: RailNodeId, t: TrackId) -> crate::command::Heading {
        let tr = &self.tracks[t];
        let other = if tr.a == n { tr.b } else { tr.a };
        let (p, o) = (self.nodes[n].tile, self.nodes[other].tile);
        heading_of(((o.x - p.x).signum(), (o.y - p.y).signum()))
    }

    /// 頂点 `v` の先に続けて進める頂点。出るノードで、45 度までの曲がりで入れる線路である。ID の順に返す。
    fn successors(&self, v: Vertex) -> Vec<Vertex> {
        let (_, n) = self.ends(v);
        let came = self.arm(n, v.0);
        self.nodes[n]
            .tracks
            .iter()
            .filter(|t| **t != v.0)
            .filter(|t| spread(came, self.arm(n, **t)) >= 3)
            .map(|t| {
                let d = if self.tracks[*t].a == n {
                    Dir::Forward
                } else {
                    Dir::Backward
                };
                (*t, d)
            })
            .collect()
    }

    fn length(&self, t: TrackId) -> i64 {
        self.track_line(t).expect("線路がない").length().0
    }

    /// `starts` のどれかから、線路 `goal` へ（`goal_dir` があればその向きで）入るまでの最短の進路を探す。
    /// 進路は選んだ出発の頂点から、着いた頂点までを両端を含めて返す。道のりは入った線路の長さの和で、
    /// 同じなら（線路の ID、向き）の小さい順に決める。出発の頂点そのものは着いたことにしない。
    fn search(
        &self,
        starts: &[Vertex],
        goal: TrackId,
        goal_dir: Option<Dir>,
    ) -> Option<Vec<Vertex>> {
        let mut dist = BTreeMap::<Vertex, i64>::new();
        let mut prev = BTreeMap::<Vertex, Vertex>::new();
        let mut heap = BinaryHeap::new();
        for s in starts {
            dist.insert(*s, 0);
            heap.push(Reverse((0_i64, s.0, s.1, true)));
        }
        while let Some(Reverse((d, t, dir, start))) = heap.pop() {
            let v = (t, dir);
            if dist.get(&v).is_some_and(|best| *best < d) {
                continue;
            }
            if !start && t == goal && goal_dir.is_none_or(|g| g == dir) {
                let mut path = vec![v];
                let mut cur = v;
                while let Some(p) = prev.get(&cur) {
                    path.push(*p);
                    cur = *p;
                }
                path.reverse();
                return Some(path);
            }
            for w in self.successors(v) {
                let nd = d.checked_add(self.length(w.0)).expect("道のりがあふれた");
                if dist.get(&w).is_none_or(|best| nd < *best) {
                    dist.insert(w, nd);
                    prev.insert(w, v);
                    heap.push(Reverse((nd, w.0, w.1, false)));
                }
            }
        }
        None
    }

    /// 駅の並びから 1 周の進路を作る。どこかで向きを変えなければ届かなければ `None`。
    fn plan(&self, stations: &[StationId], looped: bool) -> Option<Vec<Step>> {
        let track = |s: StationId| self.stations.get(s).map(|st| st.track);
        let first = track(stations[0])?;
        let mut seq: Vec<Vertex> = Vec::new();
        let mut stops: Vec<usize> = Vec::new();
        for (i, s) in stations.iter().enumerate().skip(1) {
            let goal = track(*s)?;
            let starts: Vec<Vertex> = if i == 1 {
                vec![(first, Dir::Forward), (first, Dir::Backward)]
            } else {
                vec![*seq.last().expect("進路が空")]
            };
            let leg = self.search(&starts, goal, None)?;
            if seq.is_empty() {
                seq.push(leg[0]);
            }
            seq.extend_from_slice(&leg[1..]);
            stops.push(seq.len() - 1);
        }
        let steps: Vec<(Vertex, StepKind, bool)> = if looped {
            let start = seq[0];
            let leg = self.search(&[*seq.last().expect("進路が空")], first, Some(start.1))?;
            seq.extend_from_slice(&leg[1..]);
            // 最後の要素は最初の駅に戻った所で、先頭と同じなので除く。先頭が最初の駅の止まる所になる。
            seq.pop();
            let mut out: Vec<(Vertex, StepKind, bool)> =
                seq.iter().map(|v| (*v, StepKind::Pass, false)).collect();
            out[0].1 = StepKind::Stop;
            out[0].2 = true;
            for i in stops {
                out[i].1 = StepKind::Stop;
                out[i].2 = true;
            }
            out
        } else {
            let n = seq.len();
            let mut out: Vec<(Vertex, StepKind, bool)> =
                seq.iter().map(|v| (*v, StepKind::Pass, false)).collect();
            out.extend(
                seq.iter()
                    .rev()
                    .map(|(t, d)| ((*t, flip(*d)), StepKind::Pass, false)),
            );
            let last = *stops.last().expect("駅が 2 つ未満");
            for i in stops {
                if i == last {
                    out[i].1 = StepKind::Terminal;
                } else {
                    out[i].1 = StepKind::Stop;
                    out[i].2 = true;
                    // 帰りの同じ駅。
                    out[2 * n - 1 - i].1 = StepKind::Stop;
                    out[2 * n - 1 - i].2 = true;
                }
            }
            out[2 * n - 1].1 = StepKind::Terminal;
            // 折り返して入る要素から出る。
            out[0].2 = true;
            out[n].2 = true;
            out
        };
        let len = steps.len();
        Some(
            (0..len)
                .map(|i| {
                    let ((t, d), kind, depart) = steps[i];
                    let ((pt, pd), pkind, _) = steps[(i + len - 1) % len];
                    let turned = pkind == StepKind::Terminal;
                    let (entry, _) = self.ends((t, d));
                    let junction =
                        (!turned && self.nodes[entry].tracks.len() >= 3).then_some(entry);
                    let bend = !turned && {
                        let (_, exit) = self.ends((pt, pd));
                        debug_assert_eq!(exit, entry);
                        spread(self.arm(entry, pt), self.arm(entry, t)) != 4
                    };
                    let station = if kind != StepKind::Pass || turned || depart {
                        self.tracks[t].station
                    } else {
                        None
                    };
                    Step {
                        track: t,
                        dir: d,
                        len: self.length(t),
                        kind,
                        station,
                        junction,
                        slow: junction.is_some() || bend,
                        depart,
                        shared: false,
                    }
                })
                .collect(),
        )
    }

    /// 出る要素ごとに、次に止まる駅のホームまでの閉塞をほかの路線と共有するかを決め直す。
    fn mark_shared(&mut self) {
        let mut users = BTreeMap::<Block, BTreeSet<LineId>>::new();
        for (id, l) in self.lines.iter() {
            for s in &l.path {
                for b in s.blocks() {
                    users.entry(b).or_default().insert(id);
                }
            }
        }
        let ids: Vec<LineId> = self.lines.iter().map(|(id, _)| id).collect();
        for id in ids {
            let line = self.lines.get(id).expect("路線がない");
            let flags: Vec<bool> = (0..line.len())
                .map(|i| {
                    if !line.step(i).depart {
                        return false;
                    }
                    let stop = line.next_stop(line.next(i));
                    let mut j = i;
                    loop {
                        j = line.next(j);
                        if line
                            .step(j)
                            .blocks()
                            .any(|b| users.get(&b).is_some_and(|u| u.len() > 1))
                        {
                            return true;
                        }
                        if j == stop {
                            return false;
                        }
                    }
                })
                .collect();
            let line = self.lines.get_mut(id).expect("路線がない");
            for (s, f) in line.path.iter_mut().zip(flags) {
                s.shared = f;
            }
        }
    }

    /// 駅 `from` から駅 `to` への路線を作る。
    pub(crate) fn create_line(
        &mut self,
        from: StationId,
        to: StationId,
    ) -> Result<LineId, DenyReason> {
        if self.stations.get(from).is_none() || self.stations.get(to).is_none() {
            return Err(DenyReason::NoSuchStation);
        }
        if from == to {
            return Err(DenyReason::StationInLine);
        }
        if self.lines.iter().count() >= MAX_LINES {
            return Err(DenyReason::LineLimit);
        }
        let stations = vec![from, to];
        let path = self.plan(&stations, false).ok_or(DenyReason::RailNoRoute)?;
        let id = self.lines.insert(RailLine {
            stations,
            looped: false,
            trains: 0,
            path,
            stopped: false,
            departed: BTreeMap::new(),
        });
        self.mark_shared();
        Ok(id)
    }

    /// 路線の終点の先に駅を 1 つ足す。最初の駅を指せば環状にする。
    pub(crate) fn extend_line(&mut self, id: LineId, station: StationId) -> Result<(), DenyReason> {
        let line = self.lines.get(id).ok_or(DenyReason::NoSuchLine)?;
        if self.stations.get(station).is_none() {
            return Err(DenyReason::NoSuchStation);
        }
        if line.looped || line.stations.len() >= MAX_LINE_STATIONS {
            return Err(DenyReason::LineLimit);
        }
        let closes = line.stations[0] == station;
        if !closes && line.stations.contains(&station) {
            return Err(DenyReason::StationInLine);
        }
        let mut stations = line.stations.clone();
        if !closes {
            stations.push(station);
        }
        let path = self
            .plan(&stations, closes)
            .ok_or(DenyReason::RailNoRoute)?;
        let line = self.lines.get_mut(id).expect("路線がない");
        line.stations = stations;
        line.looped = closes;
        self.replace_path(id, path);
        self.mark_shared();
        Ok(())
    }

    /// 路線をなくす。走っている列車も消す。
    pub(crate) fn remove_line(&mut self, id: LineId) -> Result<(), DenyReason> {
        self.lines.remove(id).ok_or(DenyReason::NoSuchLine)?;
        self.remove_trains_where(|t| t.line == id);
        self.mark_shared();
        Ok(())
    }

    /// 路線を走らせる列車の本数を決める。減らした分は、終点で折り返す列車から消える。
    pub(crate) fn set_line_trains(&mut self, id: LineId, trains: u8) -> Result<(), DenyReason> {
        let line = self.lines.get_mut(id).ok_or(DenyReason::NoSuchLine)?;
        line.trains = trains.min(MAX_LINE_TRAINS);
        Ok(())
    }

    /// 路線の進路を替え、列車をその上の同じ線路へ移す。
    fn replace_path(&mut self, id: LineId, path: Vec<Step>) {
        let line = self.lines.get_mut(id).expect("路線がない");
        line.stopped = false;
        if line.path == path {
            return;
        }
        line.path = path;
        line.departed.clear();
        self.relocate_trains(id);
    }

    /// 線路を引くか取り壊したあとに、路線の駅と進路を選び直す。消えた駅は路線から外し、駅が 2 つ未満に
    /// なった路線はなくす。進路を選べない路線は止まり、それまでの進路を残す。取り壊した線路 `removed` の上に
    /// いた列車は消す。線路を分けただけなら、列車は分けた線路へ移してあるので消さない。
    pub(crate) fn refresh_lines(&mut self, removed: &BTreeSet<TrackId>) {
        // 取り壊した線路の上にいる列車は、進路を選び直す前の進路で体の位置を求めて消す。
        let gone: Vec<_> = self
            .trains
            .iter()
            .filter(|(_, t)| {
                removed.contains(&t.track)
                    || self.lines.get(t.line).is_none_or(|l| {
                        self.body_steps(l, t)
                            .into_iter()
                            .any(|i| removed.contains(&l.step(i).track))
                    })
            })
            .map(|(id, _)| id)
            .collect();
        for id in gone {
            self.trains.remove(id);
        }
        let ids: Vec<LineId> = self.lines.iter().map(|(id, _)| id).collect();
        for id in ids {
            let line = self.lines.get(id).expect("路線がない");
            let mut stations: Vec<StationId> = line
                .stations
                .iter()
                .copied()
                .filter(|s| self.stations.get(*s).is_some())
                .collect();
            stations.dedup();
            let looped = line.looped && stations.len() >= 2;
            if stations.len() < 2 {
                self.lines.remove(id);
                self.remove_trains_where(|t| t.line == id);
                continue;
            }
            let plan = self.plan(&stations, looped);
            let line = self.lines.get_mut(id).expect("路線がない");
            line.stations = stations;
            line.looped = looped;
            match plan {
                Some(path) => self.replace_path(id, path),
                None => {
                    line.stopped = true;
                    self.relocate_trains(id);
                }
            }
        }
        self.mark_shared();
    }
}

impl StateHash for Step {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.track);
        h.write(&(self.dir as u8));
        h.write(&self.len);
        h.write(&(self.kind as u8));
        h.write(&self.station);
        h.write(&self.junction);
        h.write(&self.slow);
        h.write(&self.depart);
        h.write(&self.shared);
    }
}

impl StateHash for RailLine {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.stations);
        h.write(&self.looped);
        h.write(&self.trains);
        h.write(&self.path);
        h.write(&self.stopped);
        h.write(&self.departed);
    }
}
