//! 踏切（設計 3.10 の 1 と 3）。
//!
//! 踏切は、道路の区間の途中のマスを線路が横切る所である。マスの一覧だけを持ち、道路の区間も線路も ID では
//! 持たない。道路を分けたり種類を変えたりすると区間の ID が変わり、線路を分けると線路の ID が変わるので、
//! 使うたびにマスから引き直す。
//!
//! 踏切のマスの両隣（線路に沿って）にはノードを置く。踏切の線路は 2 マスの短い区間になり、待てる所
//! （[`super::Step::wait`]）にならない。列車は待てる所から次の待てる所までをまとめて押さえて進むので、
//! 踏切の線路を押さえた列車は、その先の待てる所まで押さえてある。列車が止まって先を待つのは待てる所の先の端で、
//! 体は踏切にかからない。
//!
//! 踏切を閉じるかは、tick の頭に列車の位置だけから決め、状態に持たない。閉じるのは次のどちらかのときである。
//! - 列車の体が踏切にかかっている。
//! - 走っている列車が、押さえた範囲の中で尾が踏切を抜けきるまで進めて、いまの速さから最高の速さまで加速して
//!   進んだとして [`CROSSING_WARN_TICKS`] tick 以内に先頭が踏切に着く。駅に止まっている列車と、駅を出るのを
//!   待つ列車は閉じない。
//!
//! 列車の体は、先頭（進路の上の位置）から前へ半マス、後ろへ全長から半マスを引いた長さにかかるとみなす。
//! 駅の線路の先の端のノードに先頭を止めた列車が、両端のノードから半マスずつ外へ延びるホームにちょうど収まる形である。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use super::train::{TRAIN_ACCEL, TRAIN_DECEL, TRAIN_LEN_MM, TRAIN_MAX_SPEED, TrainState};
use super::{Rail, RailLine, Train, TrainId};
use crate::network::{DIAGONAL_TILE_MM, Dir, TILE_MM};
use crate::tile::TilePos;

/// 踏切を閉じる時間（tick）。先頭がこの tick 数のうちに踏切に着く見込みになったら閉じる。
pub const CROSSING_WARN_TICKS: i64 = 16;

/// 列車の先頭（進路の上の位置）から、体の前の端までの長さ（mm）。
const NOSE_MM: i64 = TILE_MM / 2;

/// 列車の先頭から、体の後ろの端までの長さ（mm）。
const BODY_BACK_MM: i64 = TRAIN_LEN_MM - NOSE_MM;

/// 踏切の読み取り用の数。状態ハッシュには入れない。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossingTally {
    /// 列車の体と、車か歩く人が、同じ踏切の中にいた tick の数（踏切ごとに数える）。止まりきれない所で
    /// 踏切に入った列車はそのまま進むので、閉じる前に入った車が抜けきれないときにだけ起きる。
    pub overrun: u64,
    /// 閉じた踏切の手前で止まっていた車の延べ tick。
    pub wait: u64,
    /// 踏切が閉じていた延べ tick（踏切ごとに数える）。
    pub closed_ticks: u64,
    /// 踏切が閉じた回数。
    pub closures: u64,
    /// 1 回に続けて閉じていた tick のいちばん長い値。
    pub longest_closed: u64,
}

/// 線路の上の踏切。
#[derive(Clone, Copy, Debug)]
struct OnTrack {
    /// 線路の `a` のノードから踏切のマスの中心までの距離（mm）。
    from_a: i64,
    /// 線路に沿った、踏切のマスの半分の長さ（mm）。
    half: i64,
    tile: TilePos,
}

/// 列車から見た踏切。先頭から手前の端と向こうの端までの距離（mm）である。後ろにあれば負になる。
#[derive(Clone, Copy, Debug)]
struct Ahead {
    near: i64,
    far: i64,
    tile: TilePos,
}

/// いまの速さ `v` から最高の速さまで加速して、`CROSSING_WARN_TICKS` tick で進む距離。
fn warn_reach(v: i64) -> i64 {
    (1..=CROSSING_WARN_TICKS)
        .map(|k| (v + k * TRAIN_ACCEL).min(TRAIN_MAX_SPEED))
        .sum()
}

fn isqrt(v: i64) -> i64 {
    let r = u64::try_from(v.max(0)).expect("負の数の平方根").isqrt();
    i64::try_from(r).expect("平方根があふれた")
}

impl Rail {
    /// 踏切のマスを、マスの順に返す。
    pub fn crossings(&self) -> impl Iterator<Item = TilePos> + '_ {
        self.crossings.keys().copied()
    }

    /// マス `t` が踏切か。
    pub fn is_crossing(&self, t: TilePos) -> bool {
        self.crossings.contains_key(&t)
    }

    /// 踏切の数。
    pub fn crossing_count(&self) -> usize {
        self.crossings.len()
    }

    /// 踏切 `t` が閉じていれば、閉じた tick。
    pub fn closed_since(&self, t: TilePos) -> Option<u64> {
        self.crossings.get(&t).copied().flatten()
    }

    pub fn crossing_tally(&self) -> CrossingTally {
        self.crossing_tally
    }

    /// マス `t` を踏切にするか、踏切でなくする。道路と線路の網に合わせて、呼ぶ側が決める。
    pub(crate) fn set_crossing(&mut self, t: TilePos, on: bool) {
        if on {
            self.crossings.entry(t).or_insert(None);
        } else {
            self.crossings.remove(&t);
        }
    }

    /// マス `t` を通る線路の 1 歩の向き。線路が通っていなければ `None`。
    pub fn rail_step_at(&self, t: TilePos) -> Option<(i64, i64)> {
        let id = self.tracks_through(t).first()?;
        self.track_line(*id).map(|l| l.step)
    }

    /// マス `t` を踏切にできる線路の形か。駅の外の線路の途中のマスか、2 本の線路をまっすぐつなぐだけのノード
    /// （長い線路の区切りのノードなど）のマスなら、踏切にできる。線路の端、分岐、曲がり、駅のマスにはできない。
    pub fn crossing_fits(&self, t: TilePos) -> bool {
        let through = self.tracks_through(t);
        if through.is_empty() || through.iter().any(|id| self.tracks[*id].station.is_some()) {
            return false;
        }
        match self.node_at(t) {
            None => true,
            Some(_) => {
                let arms = self.arms_at(t);
                arms.len() == 2 && super::spread(arms[0], arms[1]) == 4
            }
        }
    }

    /// 踏切 `t` の両隣（線路に沿って）にノードを置く。線路の途中なら、そこで線路を分ける。
    /// 分けた線路の上の列車は同じ位置へ移し、路線の進路を選び直す。
    pub(crate) fn split_for_crossing(&mut self, t: TilePos) {
        let Some(step) = self.rail_step_at(t) else {
            return;
        };
        let mut split = false;
        for d in [1, -1] {
            let n = TilePos::new(t.x + d * step.0, t.y + d * step.1);
            if self.node_at(n).is_some() {
                continue;
            }
            if let Some(id) = self.tracks_through(n).first().copied() {
                self.split_track(id, n);
                split = true;
            }
        }
        if split {
            self.refresh_lines(&BTreeSet::new());
        }
    }

    /// 線路ごとの踏切。
    fn crossings_by_track(&self) -> BTreeMap<super::TrackId, SmallVec<[OnTrack; 2]>> {
        let mut out = BTreeMap::<_, SmallVec<[OnTrack; 2]>>::new();
        for t in self.crossings.keys() {
            for id in self.tracks_through(*t) {
                let Some(line) = self.track_line(*id) else {
                    continue;
                };
                let Some(k) = line.position_of(*t) else {
                    continue;
                };
                let per = if line.is_diagonal() {
                    DIAGONAL_TILE_MM
                } else {
                    TILE_MM
                };
                let k = i64::try_from(k).expect("歩数があふれた");
                out.entry(*id).or_default().push(OnTrack {
                    from_a: k * per,
                    half: per / 2,
                    tile: *t,
                });
            }
        }
        out
    }

    /// 列車の尾の要素から、先に押さえた要素までにある踏切。近い順に並ぶ。
    /// ノードのマスの踏切は、そのノードから出ていく要素の始まりにあるものとして 1 度だけ数える。
    fn crossings_near(
        by_track: &BTreeMap<super::TrackId, SmallVec<[OnTrack; 2]>>,
        line: &RailLine,
        t: &Train,
    ) -> SmallVec<[Ahead; 2]> {
        let mut out = SmallVec::new();
        let tail = line.tail(t.pos, t.offset);
        let mut i = t.pos;
        let mut d = -t.offset;
        while i != tail {
            i = line.prev(i);
            d -= line.step(i).len;
        }
        let count = (line.distance(tail, t.pos) + t.ahead).min(line.len() - 1);
        for _ in 0..=count {
            let s = line.step(i);
            if let Some(list) = by_track.get(&s.track) {
                for c in list {
                    let x = match s.dir {
                        Dir::Forward => c.from_a,
                        Dir::Backward => s.len - c.from_a,
                    };
                    if (0..s.len).contains(&x) {
                        out.push(Ahead {
                            near: d + x - c.half,
                            far: d + x + c.half,
                            tile: c.tile,
                        });
                    }
                }
            }
            d += s.len;
            i = line.next(i);
        }
        out
    }

    /// いま閉じている踏切。列車の位置だけから決める（先頭の説明）。
    pub fn closed_crossings(&self) -> BTreeSet<TilePos> {
        let mut closed = BTreeSet::new();
        if self.crossings.is_empty() {
            return closed;
        }
        let by_track = self.crossings_by_track();
        for (_, t) in self.trains.iter() {
            let Some(line) = self.lines.get(t.line) else {
                continue;
            };
            let running = t.state == TrainState::Running;
            // 押さえた範囲の中で、止まらずに進める先頭の位置。
            let limit = running.then(|| {
                let s = line.upcoming_stop(t);
                line.distance_to_end(t, t.ahead.min(line.distance(t.pos, s)))
            });
            let reach = warn_reach(t.speed);
            for c in Self::crossings_near(&by_track, line, t) {
                let body = NOSE_MM > c.near && -BODY_BACK_MM < c.far;
                let coming = limit.is_some_and(|limit| {
                    c.near >= NOSE_MM && c.near - NOSE_MM <= reach && limit >= c.far + BODY_BACK_MM
                });
                if body || coming {
                    closed.insert(c.tile);
                }
            }
        }
        closed
    }

    /// 中に車か人がいる踏切 `occupied` の手前で止まりきれる、走っている列車と、先頭から止まる所までの距離。
    /// 止まりきれない踏切はそのまま進み、その先の踏切で止まりきれれば、そこで止まる。
    pub(super) fn crossing_stops(&self, occupied: &BTreeSet<TilePos>) -> BTreeMap<TrainId, i64> {
        let mut stops = BTreeMap::new();
        if occupied.is_empty() {
            return stops;
        }
        let by_track = self.crossings_by_track();
        for (id, t) in self.trains.iter() {
            if t.state != TrainState::Running {
                continue;
            }
            let Some(line) = self.lines.get(t.line) else {
                continue;
            };
            let stop = Self::crossings_near(&by_track, line, t)
                .into_iter()
                .filter(|c| c.near >= NOSE_MM && occupied.contains(&c.tile))
                .map(|c| c.near - NOSE_MM)
                .find(|room| isqrt(2 * TRAIN_DECEL * room) >= t.speed - TRAIN_DECEL);
            if let Some(room) = stop {
                stops.insert(id, room);
            }
        }
        stops
    }

    /// tick `now` の踏切を数える。`closed` はこの tick の頭に閉じた踏切、`occupied` は中に車か人がいる踏切、
    /// `waits` は閉じた踏切の手前で止まっていた車の数である。列車を進めたあとに呼ぶ。
    pub(crate) fn note_crossings(
        &mut self,
        now: u64,
        closed: &BTreeSet<TilePos>,
        occupied: &BTreeSet<TilePos>,
        waits: u64,
    ) {
        if self.crossings.is_empty() {
            return;
        }
        let tally = &mut self.crossing_tally;
        tally.wait = tally.wait.saturating_add(waits);
        for (tile, since) in &mut self.crossings {
            if closed.contains(tile) {
                tally.closed_ticks = tally.closed_ticks.saturating_add(1);
                let start = *since.get_or_insert_with(|| {
                    tally.closures = tally.closures.saturating_add(1);
                    now
                });
                tally.longest_closed = tally.longest_closed.max(now - start + 1);
            } else {
                *since = None;
            }
        }
        if occupied.is_empty() {
            return;
        }
        let by_track = self.crossings_by_track();
        let mut hit = BTreeSet::new();
        for (_, t) in self.trains.iter() {
            let Some(line) = self.lines.get(t.line) else {
                continue;
            };
            for c in Self::crossings_near(&by_track, line, t) {
                if NOSE_MM > c.near && -BODY_BACK_MM < c.far && occupied.contains(&c.tile) {
                    hit.insert(c.tile);
                }
            }
        }
        let tally = &mut self.crossing_tally;
        tally.overrun = tally.overrun.saturating_add(hit.len() as u64);
    }
}
