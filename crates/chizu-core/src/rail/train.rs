//! 列車と閉塞（設計 3.10 の 3）。
//!
//! 列車は路線の進路（[`RailLine::path`]）の上を、どの速さでも 1 tick ずつ動く。位置は先頭がいる進路の要素と、
//! その要素に入ってからの距離で持ち、尾は全長ぶん後ろにある（閉塞を押さえる長さは [`TRAIN_BLOCK_LEN_MM`]）。
//! 乗る人はまだいない。
//!
//! 閉塞は、線路の向きごとのノードとノードのあいだと、分岐のノード（上りと下りをまとめて 1 つ）である
//! （[`Block`]）。列車は先の閉塞を押さえてから入り、尾が抜けたら返す。押さえている閉塞は、列車の体がかかる
//! 要素と、先頭より先に押さえた要素の数（[`Train::ahead`]）から求まるので、状態には持たない。
//!
//! 1 tick は次の順に進める。どれも列車か路線の ID の順で、結果は実行順に左右されない。
//! 1. 駅に止まる時間と折り返しの時間を数える。
//! 2. 先の閉塞を押さえる。同じ tick に押さえようとする列車は、待った長さの長い順、同じなら ID の順にする。
//! 3. 押さえた範囲の中で速さを決めて進める。前の tick の状態と押さえた範囲だけを読む。中に車か人がいる踏切の
//!    手前で止まりきれる列車は、その手前までしか進まない（[`super::crossing`]）。
//! 4. 路線の本数に足りなければ、最初の駅のホームが空いているときに列車を 1 本出す。
//! 5. 押さえられずに止まった列車が互いを待つ輪を見つけたら、輪の中で ID のいちばん大きい列車を消す。
//!
//! 行き詰まりは次の 3 つの決まりで防ぐ。
//! 1. 本数の上限（[`RailLine::cap`]）。待てる所（[`Step::wait`]）の数 − 1 までにする。
//! 2. 終点の駅には、両方のホーム（着く側と、折り返して出る側）をまとめて押さえられるときだけ入る。
//!    終点は待てる所にしないので、3 の押さえ方で、折り返した先まで続けて押さえる。
//! 3. 列車は、待てる所から次の待てる所までをまとめて押さえて進む。待てる所は、列車がまるごと入り、
//!    ほかの路線が使わない要素である。分岐のすぐ先の短い線路や、ほかの路線と共有する駅と線路では待たずに、
//!    抜けた先の自分の路線だけの待てる所まで押さえてから入る。途中の駅に止まるのと間を空けて待つのは
//!    時間で終わるので、待てる所の外で押さえたまま待つ列車は、いずれ進んで閉塞を返す。

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::line::{RailLine, Step, StepKind};
use super::{LineId, Rail, RailNodeId, TrackId, TrainId};
use crate::network::Dir;
use crate::state_hash::{StateHash, StateHasher};
use crate::tile::TilePos;
use crate::units::{Money, Tick};

/// 編成の全長（mm）。1 両 20m の 4 両である。
pub const TRAIN_LEN_MM: i64 = 80_000;

/// 列車が閉塞を押さえる長さ（mm）。全長から 1 マスを引いた 72m である。
/// 閉塞はマスの中心のノードで区切るが、駅のホーム（80m）は駅の線路の両端のノードから半マスずつ外へ延びる。
/// 先頭を駅の線路の先の端に止めた列車が、手前の閉塞にかからないよう、両端の半マスずつはノードのマスに収まるとみなす。
pub const TRAIN_BLOCK_LEN_MM: i64 = TRAIN_LEN_MM - crate::network::TILE_MM;

/// 最高の速さ（mm/tick）。時速 90km である。
pub const TRAIN_MAX_SPEED: i64 = 25_000;

/// 加速（mm/tick²）。
pub const TRAIN_ACCEL: i64 = 800;

/// 減速（mm/tick²）。
pub const TRAIN_DECEL: i64 = 1_000;

/// 45 度の曲がりと分岐を通るときの速さの上限（mm/tick）。時速 40km である。
pub const TRAIN_SLOW_SPEED: i64 = 11_111;

/// 駅に止まる時間（tick）。乗る人がまだいないので、いつもこの長さである。
pub const DWELL_TICKS: u16 = 20;

/// 終点で折り返す時間（tick）。このあいだは終点の駅の両方のホームを押さえる。
pub const TURNBACK_TICKS: u16 = 30;

/// 列車 1 本の 1 日の維持費。
pub const TRAIN_UPKEEP_PER_DAY: Money = Money(600);

/// 閉塞。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Block {
    /// 分岐のノード。上りと下りをまとめて 1 つにする。
    Junction(RailNodeId),
    /// 線路の 1 つの向き。
    Track(TrackId, Dir),
}

/// 列車の状態。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrainState {
    /// 走っているか、先の閉塞を待って止まっている。
    Running,
    /// 駅に止まっている。`left` は残りの tick で、終点で 0 なら折り返す先のホームが空くのを待っている。
    Dwell { left: u16 },
    /// 終点で折り返している。`left` は残りの tick である。
    Turnback { left: u16 },
    /// 駅を出るのを待っている（間を空ける決まりか、先の閉塞を待つ）。
    Ready,
}

/// 列車。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Train {
    pub line: LineId,
    /// 先頭がいる進路の要素の番号。
    pub pos: u32,
    /// 先頭がいる線路と向き。進路の `pos` の要素と同じで、線路が変わって進路を選び直したときに、列車を
    /// 新しい進路の上に置き直すのに使う。
    pub track: TrackId,
    pub dir: Dir,
    /// 先頭が要素に入ってから進んだ距離（mm）。
    pub offset: i64,
    /// 速さ（mm/tick）。直前の tick に進んだ距離である。
    pub speed: i64,
    /// 先頭がいる要素より先に押さえた要素の数。
    pub ahead: u32,
    pub state: TrainState,
    /// 先の閉塞を押さえられずに待った tick。押さえる順を決めるのに使う。
    pub waited: u32,
}

/// 読み取り用の数。状態ハッシュには入れない。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RailTally {
    /// 列車が互いを待つ輪を見つけて、列車を消した回数。
    pub deadlock: u64,
}

fn isqrt(v: i64) -> i64 {
    let r = u64::try_from(v).expect("負の数の平方根").isqrt();
    i64::try_from(r).expect("平方根があふれた")
}

/// 先の閉塞を押さえようとした結果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reserve {
    /// 何も要らなかったか、押さえられた。
    Done,
    /// 押さえられなかった。押さえている列車がいれば、その列車。
    Blocked(Option<TrainId>),
}

impl RailLine {
    /// 先頭が `pos` の要素の `offset` にいる列車の、尾がいる要素の番号。
    pub(crate) fn tail(&self, pos: u32, offset: i64) -> u32 {
        let mut rest = TRAIN_BLOCK_LEN_MM - offset;
        let mut i = pos;
        for _ in 0..self.len() {
            if rest <= 0 {
                break;
            }
            i = self.prev(i);
            rest -= self.step(i).len;
        }
        i
    }

    /// 列車が押さえている閉塞。体がかかる要素（尾の要素の入るところの分岐は除く）と、先に押さえた要素の閉塞である。
    pub(crate) fn held_blocks(&self, t: &Train) -> Vec<Block> {
        let tail = self.tail(t.pos, t.offset);
        let s = self.step(tail);
        let mut out = vec![Block::Track(s.track, s.dir)];
        let mut i = tail;
        while i != t.pos {
            i = self.next(i);
            out.extend(self.step(i).blocks());
        }
        for _ in 0..t.ahead {
            i = self.next(i);
            out.extend(self.step(i).blocks());
        }
        out
    }

    /// 先頭から、`k` 個先の要素の先の端までの距離。
    pub(super) fn distance_to_end(&self, t: &Train, k: u32) -> i64 {
        let mut d = self.step(t.pos).len - t.offset;
        let mut i = t.pos;
        for _ in 0..k {
            i = self.next(i);
            d += self.step(i).len;
        }
        d
    }

    /// 先頭が `pos` の要素にいる列車が、`k` 個先の要素から押さえ始めるときに、まとめて押さえる終わりの
    /// 要素が先頭から何個先か。`k` 個先から先で最初の待てる所までである（決まりの 3）。
    /// 待てる所がないか 1 周しても届かなければ、1 周の手前までにする。上限が 0 の路線の列車は消すので、
    /// ふだんは起きない。
    pub(crate) fn run_ahead(&self, pos: u32, k: u32) -> u32 {
        let last = self.len() - 1;
        if k > last {
            return last;
        }
        self.next_wait((pos + k) % self.len())
            .map(|j| self.distance(pos, j))
            .filter(|d| *d >= k)
            .unwrap_or(last)
    }

    /// 走っている列車が次に止まる要素。先頭が要素の先の端にいるなら、その要素は通り過ぎたものとする。
    pub(super) fn upcoming_stop(&self, t: &Train) -> u32 {
        let from = if t.offset < self.step(t.pos).len {
            t.pos
        } else {
            self.next(t.pos)
        };
        self.next_stop(from)
    }
}

impl Rail {
    /// 列車を ID の順に返す。
    pub fn trains(&self) -> impl Iterator<Item = (TrainId, &Train)> {
        self.trains.iter()
    }

    pub fn train(&self, id: TrainId) -> Option<&Train> {
        self.trains.get(id)
    }

    /// 走っている列車の数。
    pub fn train_count(&self) -> usize {
        self.trains.iter().count()
    }

    pub fn tally(&self) -> RailTally {
        self.tally
    }

    /// 列車の体がかかる要素の番号。尾の要素から先頭の要素までである。
    pub(crate) fn body_steps(&self, line: &RailLine, t: &Train) -> Vec<u32> {
        let mut i = line.tail(t.pos, t.offset);
        let mut out = vec![i];
        while i != t.pos {
            i = line.next(i);
            out.push(i);
        }
        out
    }

    /// 列車が押さえている要素の番号。体がかかる要素と、先に押さえた要素である。
    pub(crate) fn reserved_steps(&self, line: &RailLine, t: &Train) -> Vec<u32> {
        let mut out = self.body_steps(line, t);
        let mut i = t.pos;
        for _ in 0..t.ahead {
            i = line.next(i);
            out.push(i);
        }
        out
    }

    /// 列車が押さえている閉塞と、押さえている列車。
    pub fn occupancy(&self) -> BTreeMap<Block, TrainId> {
        let mut held = BTreeMap::new();
        for (id, t) in self.trains.iter() {
            if let Some(line) = self.lines.get(t.line) {
                for b in line.held_blocks(t) {
                    held.entry(b).or_insert(id);
                }
            }
        }
        held
    }

    pub(crate) fn remove_trains_where(&mut self, pred: impl Fn(&Train) -> bool) {
        let gone: Vec<TrainId> = self
            .trains
            .iter()
            .filter(|(_, t)| pred(t))
            .map(|(id, _)| id)
            .collect();
        for id in gone {
            self.trains.remove(id);
        }
    }

    /// 線路 `old` を 2 本に分けたときに、その上の列車を同じ位置の分けた線路へ移す。
    /// `first` は線路の `a` の側、`second` は `b` の側で、それぞれの長さを添える。
    pub(crate) fn move_trains_off(
        &mut self,
        old: TrackId,
        first: (TrackId, i64),
        second: (TrackId, i64),
    ) {
        let ids: Vec<TrainId> = self
            .trains
            .iter()
            .filter(|(_, t)| t.track == old)
            .map(|(id, _)| id)
            .collect();
        for id in ids {
            let t = self.trains.get_mut(id).expect("列車がない");
            // 進む向きに先に通る線路と、あとに通る線路。
            let (near, far) = match t.dir {
                Dir::Forward => (first, second),
                Dir::Backward => (second, first),
            };
            if t.offset <= near.1 {
                t.track = near.0;
            } else {
                t.track = far.0;
                t.offset -= near.1;
            }
        }
    }

    /// 路線の進路が替わったか、線路が分かれたあとに、路線の列車を進路の上の同じ線路と向きの要素へ置き直す。
    /// 置ける要素がなければ列車を消す。先に押さえた要素は、新しい進路では別の要素を指すので、どの列車も返す。
    /// 新しい進路で続けられない状態は直す。駅でない所で止まっている列車は走る状態に戻し、折り返している
    /// 列車は、折り返す先のホームを押さえ直してから折り返すよう、止まって待つ状態に戻す。
    pub(crate) fn relocate_trains(&mut self, line_id: LineId) {
        let Some(line) = self.lines.get(line_id) else {
            return;
        };
        let ids: Vec<TrainId> = self
            .trains
            .iter()
            .filter(|(_, t)| t.line == line_id)
            .map(|(id, _)| id)
            .collect();
        let mut gone = Vec::new();
        for id in ids {
            let t = self.trains.get_mut(id).expect("列車がない");
            let at = |i: u32| {
                let s = line.step(i);
                s.track == t.track && s.dir == t.dir
            };
            let i = if (t.pos as usize) < line.path.len() && at(t.pos) {
                t.pos
            } else if let Some(i) = (0..line.len()).find(|i| at(*i)) {
                i
            } else {
                gone.push(id);
                continue;
            };
            let s = line.step(i);
            t.pos = i;
            t.ahead = 0;
            t.offset = t.offset.min(s.len);
            let end = t.offset == s.len;
            t.state = match t.state {
                TrainState::Dwell { left } if s.is_stop() && end => TrainState::Dwell { left },
                TrainState::Turnback { .. } if s.kind == StepKind::Terminal && end => {
                    TrainState::Dwell { left: 0 }
                }
                TrainState::Ready if s.depart && end => TrainState::Ready,
                TrainState::Running => TrainState::Running,
                _ => {
                    t.speed = 0;
                    TrainState::Running
                }
            };
        }
        for id in gone {
            self.trains.remove(id);
        }
    }

    /// 進路か待てる所が変わった直後に、続けられない列車を消す（止まった路線の列車と同じ扱い）。
    ///
    /// 行き詰まりを防ぐ決まりの 3 では、列車が押さえた範囲の終わりは、いつも待てる所にある。進路が替わると
    /// 先に押さえた要素を返し（[`Rail::relocate_trains`]）、待てる所が変わると前の終わりが待てる所でなくなるので、
    /// この形が崩れる。そこで、押さえた範囲の終わりが待てる所でない列車は、ID の順に、次の待てる所までを
    /// まとめて押さえ直す。ほかの列車が押さえていて押さえ直せなければ、その列車を消す。
    /// 待てる所の外で先を待つ列車を残すと、ほかの列車と待ち合う輪ができるためである。
    /// 押さえ直せない列車が複数いるときは、ID の最も大きい 1 本だけを消して最初からやり直す。ID の小さい列車が
    /// 押さえ直せないのは、まだ判定していない大きい列車の閉塞のせいかもしれず、その列車が消えれば通るためである。
    ///
    /// その前に、次の 2 つを消す。
    /// - 上限（[`RailLine::cap`]）を超えた本数。ID の大きい列車から消す。上限を超えた列車を次の駅まで残すと、
    ///   駅に着く前に路線の中で待ち合う輪ができることがあるためである。
    /// - ほかの列車と同じ閉塞を押さえる列車。ID の小さい列車から残す。上りと下りにすれ違う 2 本の下で
    ///   線路を分けると、分けた所の分岐（上りと下りで 1 つの閉塞）に 2 本がかかるためである。
    pub(crate) fn settle_trains(&mut self) {
        let mut counts = BTreeMap::<LineId, u8>::new();
        let mut gone = Vec::new();
        for (id, t) in self.trains.iter() {
            let cap = self.lines.get(t.line).map_or(0, RailLine::cap);
            let n = counts.entry(t.line).or_default();
            if *n >= cap {
                gone.push(id);
            } else {
                *n += 1;
            }
        }
        // ID の小さい列車から数えて残すので、消えるのは ID の大きい列車である。
        for id in gone {
            self.trains.remove(id);
        }
        let mut held = BTreeMap::<Block, TrainId>::new();
        let mut gone = Vec::new();
        for (id, t) in self.trains.iter() {
            let line = self.lines.get(t.line).expect("列車の路線がない");
            let blocks = line.held_blocks(t);
            if blocks.iter().any(|b| held.contains_key(b)) {
                gone.push(id);
            } else {
                for b in blocks {
                    held.insert(b, id);
                }
            }
        }
        for id in gone {
            self.trains.remove(id);
        }
        // 押さえ直しは、消す列車がなくなるまで繰り返す。1 回の走査で、ID の小さい列車が、まだ判定していない
        // ID の大きい列車の閉塞に阻まれて消えても、その大きい列車があとで消えれば阻まれなかったことになる。
        // そこで、押さえ直せない列車のうち ID の最も大きい 1 本だけを消し、最初からやり直す。
        let mut base = held;
        let mut alive: Vec<TrainId> = self.trains.iter().map(|(id, _)| id).collect();
        #[cfg(test)]
        let mut removed = Vec::new();
        loop {
            let mut held = base.clone();
            let mut ahead = Vec::new();
            let mut failed = None;
            for id in &alive {
                let t = self.trains.get(*id).expect("列車がない");
                let line = self.lines.get(t.line).expect("列車の路線がない");
                let end = (t.pos + t.ahead) % line.len();
                if line.step(end).wait {
                    continue;
                }
                let from = t.ahead + 1;
                let to = line.run_ahead(t.pos, from);
                let steps: Vec<u32> = (from..=to).map(|k| (t.pos + k) % line.len()).collect();
                let free = from <= to
                    && line.step((t.pos + to) % line.len()).wait
                    && steps.iter().all(|i| {
                        let s = line.step(*i);
                        self.tracks.get(s.track).is_some()
                            && s.blocks().all(|b| held.get(&b).is_none_or(|h| h == id))
                    });
                if free {
                    for i in &steps {
                        for b in line.step(*i).blocks() {
                            held.insert(b, *id);
                        }
                    }
                    ahead.push((*id, to));
                } else {
                    held.retain(|_, h| h != id);
                    failed = Some(*id);
                }
            }
            let Some(id) = failed else {
                for (id, to) in ahead {
                    self.trains.get_mut(id).expect("列車がない").ahead = to;
                }
                #[cfg(test)]
                for t in &removed {
                    if self.could_hold_after_settle(t, &held) {
                        OVERDELETED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                break;
            };
            base.retain(|_, h| *h != id);
            alive.retain(|a| *a != id);
            #[cfg(test)]
            removed.push(*self.trains.get(id).expect("列車がない"));
            self.trains.remove(id);
        }
    }

    /// テスト用。消した列車の押さえ直す先と体の閉塞が、最後の `held` ですべて空いていたかを返す。
    #[cfg(test)]
    fn could_hold_after_settle(&self, t: &Train, held: &BTreeMap<Block, TrainId>) -> bool {
        let line = self.lines.get(t.line).expect("列車の路線がない");
        let end = (t.pos + t.ahead) % line.len();
        if line.step(end).wait {
            return false;
        }
        let from = t.ahead + 1;
        let to = line.run_ahead(t.pos, from);
        from <= to
            && line.step((t.pos + to) % line.len()).wait
            && (from..=to).all(|k| {
                let s = line.step((t.pos + k) % line.len());
                self.tracks.get(s.track).is_some() && s.blocks().all(|b| !held.contains_key(&b))
            })
            && line.held_blocks(t).iter().all(|b| !held.contains_key(b))
    }

    /// 列車を 1 tick 進める。踏切のない街の列車を進めるのに使う。
    pub fn step(&mut self, tick: Tick) {
        self.step_with(tick, &BTreeSet::new());
    }

    /// 列車を 1 tick 進める。`occupied` は、車か人が中にいる踏切である。止まりきれる列車はその手前で止まる。
    /// 閉じてから [`super::CROSSING_WARN_TICKS`] tick たっていない踏切にも、列車は入らない。
    pub fn step_with(&mut self, tick: Tick, occupied: &BTreeSet<TilePos>) {
        if self.lines.is_unused() {
            return;
        }
        self.count_down();
        let mut held = self.occupancy();
        let blocked = self.reserve(tick.0, &mut held);
        let stops = self.crossing_stops(tick.0, occupied);
        self.run(&stops);
        // 新しい列車は、走っている列車が閉塞を押さえ終えてから出す。行き詰まりの輪から列車を消したとき、
        // 空いたホームを待っていた列車が、出直す列車より先に入れるようにするためである。
        let mut held = self.occupancy();
        self.spawn_trains(&mut held);
        self.break_deadlocks(&blocked);
    }

    /// 本数に足りない路線の最初の駅に、ホームが空いていれば列車を 1 本出す。
    fn spawn_trains(&mut self, held: &mut BTreeMap<Block, TrainId>) {
        let mut counts = BTreeMap::<LineId, u8>::new();
        for (_, t) in self.trains.iter() {
            *counts.entry(t.line).or_default() += 1;
        }
        let Rail {
            lines,
            trains,
            tracks: self_tracks,
            ..
        } = self;
        for (id, line) in lines.iter() {
            if line.stopped || counts.get(&id).copied().unwrap_or(0) >= line.allowed() {
                continue;
            }
            let first: &Step = line.step(0);
            // 最初の駅が待てる所でなければ、次の待てる所までをまとめて押さえて現れる（決まりの 3）。
            let ahead = if first.wait { 0 } else { line.run_ahead(0, 1) };
            let t = Train {
                line: id,
                pos: 0,
                track: first.track,
                dir: first.dir,
                offset: first.len,
                speed: 0,
                ahead,
                state: TrainState::Ready,
                waited: 0,
            };
            let blocks = line.held_blocks(&t);
            let tracks_ok = (0..=ahead).all(|k| self_tracks.get(line.step(k).track).is_some());
            if tracks_ok && blocks.iter().all(|b| !held.contains_key(b)) {
                let tid = trains.insert(t);
                for b in blocks {
                    held.insert(b, tid);
                }
            }
        }
    }

    /// 駅に止まる時間と折り返しの時間を数える。折り返し終えた列車は、同じ駅の線路の逆の向きへ移る。
    fn count_down(&mut self) {
        let Rail { lines, trains, .. } = self;
        let ids: Vec<TrainId> = trains.iter().map(|(id, _)| id).collect();
        for id in ids {
            let t = trains.get_mut(id).expect("列車がない");
            let line = lines.get(t.line).expect("列車の路線がない");
            match t.state {
                TrainState::Dwell { left } if left > 1 => {
                    t.state = TrainState::Dwell { left: left - 1 };
                }
                TrainState::Dwell { .. } => {
                    t.state = if line.step(t.pos).kind != StepKind::Terminal {
                        TrainState::Ready
                    } else if t.ahead > 0 {
                        // 折り返す先のホームを押さえてから折り返す。
                        TrainState::Turnback {
                            left: TURNBACK_TICKS,
                        }
                    } else {
                        TrainState::Dwell { left: 0 }
                    };
                }
                TrainState::Turnback { left } if left > 1 => {
                    t.state = TrainState::Turnback { left: left - 1 };
                }
                TrainState::Turnback { .. } => {
                    t.pos = line.next(t.pos);
                    t.ahead -= 1;
                    let s = line.step(t.pos);
                    t.track = s.track;
                    t.dir = s.dir;
                    t.offset = s.len;
                    t.state = TrainState::Ready;
                }
                TrainState::Running | TrainState::Ready => {}
            }
        }
    }

    /// 先の閉塞を押さえる。押さえられなかった列車と、その閉塞を押さえている列車を返す。
    fn reserve(
        &mut self,
        now: u64,
        held: &mut BTreeMap<Block, TrainId>,
    ) -> BTreeMap<TrainId, Option<TrainId>> {
        let mut order: Vec<(Reverse<u32>, TrainId)> = self
            .trains
            .iter()
            .map(|(id, t)| (Reverse(t.waited), id))
            .collect();
        order.sort_unstable();
        let mut blocked = BTreeMap::new();
        let mut departed = Vec::new();
        let Rail {
            lines,
            trains,
            tracks,
            ..
        } = self;
        for (_, id) in order {
            let t = trains.get_mut(id).expect("列車がない");
            let line = lines.get(t.line).expect("列車の路線がない");
            let mut grant = |t: &mut Train, k_from: u32, k_to: u32| -> Reserve {
                // 先頭から k_from 個先から k_to 個先までの要素を、まとめて押さえる。
                let k_to = k_to.min(line.len() - 1);
                if k_from > k_to {
                    return Reserve::Done;
                }
                let steps: Vec<u32> = (k_from..=k_to).map(|k| (t.pos + k) % line.len()).collect();
                for i in &steps {
                    let s = line.step(*i);
                    if tracks.get(s.track).is_none() {
                        return Reserve::Blocked(None);
                    }
                    for b in s.blocks() {
                        if let Some(h) = held.get(&b)
                            && *h != id
                        {
                            return Reserve::Blocked(Some(*h));
                        }
                    }
                }
                for i in &steps {
                    for b in line.step(*i).blocks() {
                        held.insert(b, id);
                    }
                }
                t.ahead = k_to;
                Reserve::Done
            };
            let result = match t.state {
                TrainState::Ready => {
                    let gap_ok = line
                        .departed
                        .get(&t.pos)
                        .is_none_or(|last| now >= last + line.departure_gap());
                    if line.stopped || !gap_ok {
                        None
                    } else {
                        // 決まりの 3：次の待てる所までをまとめて押さえて出る。待てる所でない駅には、
                        // 先まで押さえて着いているので、押さえ足さない。
                        let r = if t.ahead == 0 {
                            grant(t, 1, line.run_ahead(t.pos, 1))
                        } else {
                            Reserve::Done
                        };
                        if r == Reserve::Done {
                            t.state = TrainState::Running;
                            departed.push((t.line, t.pos));
                        }
                        Some(r)
                    }
                }
                TrainState::Dwell { .. } | TrainState::Turnback { .. } => {
                    // 決まりの 2 の両方のホームを押さえ直す（線路が変わって先に押さえた要素を返したとき）。
                    if line.step(t.pos).kind == StepKind::Terminal && t.ahead < 1 {
                        Some(grant(t, 1, line.run_ahead(t.pos, 1)))
                    } else {
                        None
                    }
                }
                TrainState::Running => {
                    let s = line.upcoming_stop(t);
                    let ds = line.distance(t.pos, s);
                    let mut r = None;
                    loop {
                        // 次に止まる駅より先は、駅を出るまで押さえない。待てる所でない駅や終点には、
                        // その先の待てる所までまとめて押さえて入るので、ここで押さえ終えている。
                        if t.ahead >= ds {
                            break;
                        }
                        let room = line.distance_to_end(t, t.ahead);
                        let need =
                            t.speed * t.speed / (2 * TRAIN_DECEL) + 2 * (t.speed + TRAIN_ACCEL);
                        if room >= need {
                            break;
                        }
                        let next = t.ahead + 1;
                        let got = grant(t, next, line.run_ahead(t.pos, next));
                        r = Some(got);
                        if got != Reserve::Done {
                            break;
                        }
                    }
                    r
                }
            };
            match result {
                Some(Reserve::Blocked(by)) => {
                    t.waited = t.waited.saturating_add(1);
                    blocked.insert(id, by);
                }
                Some(Reserve::Done) => t.waited = 0,
                None => {}
            }
        }
        for (line, pos) in departed {
            if let Some(l) = self.lines.get_mut(line) {
                l.departed.insert(pos, now);
            }
        }
        blocked
    }

    /// 走っている列車を、押さえた範囲の中で進める。次に止まる駅の先の端に着いた列車は止まる。
    /// `stops` の列車は、先頭からその距離より先へは進まない（中に車か人がいる踏切の手前）。
    fn run(&mut self, stops: &BTreeMap<TrainId, i64>) {
        let ids: Vec<TrainId> = self
            .trains
            .iter()
            .filter(|(_, t)| t.state == TrainState::Running)
            .map(|(id, _)| id)
            .collect();
        let mut arrived = Vec::new();
        {
            let Rail { lines, trains, .. } = self;
            for id in ids {
                let t = trains.get_mut(id).expect("列車がない");
                let line = lines.get(t.line).expect("列車の路線がない");
                let moved = advance(line, t, stops.get(&id).copied());
                if moved > 0 && t.offset == line.step(t.pos).len && line.step(t.pos).is_stop() {
                    t.state = TrainState::Dwell { left: DWELL_TICKS };
                    t.speed = 0;
                    arrived.push(id);
                }
            }
        }
        for id in arrived {
            self.retire_if_extra(id);
        }
    }

    /// 駅に着いた列車が、路線の本数を超えた分なら消す。決めた本数を減らした分は終点（環状なら次の駅）で消す。
    /// 行き詰まりを防ぐ上限を超えた分は、線路や路線を変えた直後に [`Rail::settle_trains`] がその場で消す。
    /// ここでの上限の判定は、走っているうちに本数が上限を超えた場合の備えである。
    fn retire_if_extra(&mut self, id: TrainId) {
        let t = *self.trains.get(id).expect("列車がない");
        let line = self.lines.get(t.line).expect("列車の路線がない");
        let count = self.trains.iter().filter(|(_, o)| o.line == t.line).count();
        let count = u8::try_from(count).unwrap_or(u8::MAX);
        let here = line.step(t.pos).kind == StepKind::Terminal || line.looped;
        if count > line.allowed() && (here || count > line.cap()) {
            self.trains.remove(id);
        }
    }

    /// 押さえられずに止まった列車の「待つ相手」を ID の順にたどり、輪になっていれば、輪の中で ID の
    /// いちばん大きい列車を消して数える。路線の本数は変えないので、消えた列車は最初の駅が空いたら出直す。
    fn break_deadlocks(&mut self, blocked: &BTreeMap<TrainId, Option<TrainId>>) {
        let halted = |t: &Train| match t.state {
            TrainState::Ready | TrainState::Dwell { left: 0 } => true,
            TrainState::Running => t.speed == 0 && t.ahead == 0,
            TrainState::Dwell { .. } | TrainState::Turnback { .. } => false,
        };
        let waits: BTreeMap<TrainId, TrainId> = blocked
            .iter()
            .filter_map(|(w, by)| Some((*w, (*by)?)))
            .filter(|(w, h)| {
                self.trains.get(*w).is_some_and(halted) && self.trains.get(*h).is_some()
            })
            .collect();
        let mut done = BTreeMap::<TrainId, ()>::new();
        let mut victims = Vec::new();
        for start in waits.keys() {
            let mut chain: Vec<TrainId> = Vec::new();
            let mut cur = *start;
            loop {
                if done.contains_key(&cur) {
                    break;
                }
                if let Some(at) = chain.iter().position(|c| *c == cur) {
                    let victim = *chain[at..].iter().max().expect("輪が空");
                    victims.push(victim);
                    break;
                }
                chain.push(cur);
                match waits.get(&cur) {
                    Some(next) => cur = *next,
                    None => break,
                }
            }
            for c in chain {
                done.insert(c, ());
            }
        }
        for v in victims {
            self.trains.remove(v);
            self.tally.deadlock += 1;
        }
    }
}

/// 列車を押さえた範囲の中で 1 tick 進め、進んだ距離を返す。`stop` があれば、先頭からその距離より先へは進まない。
fn advance(line: &RailLine, t: &mut Train, stop: Option<i64>) -> i64 {
    let tail = line.tail(t.pos, t.offset);
    // 体が曲がりか分岐にかかっていれば、遅い速さに抑える。
    let mut i = tail;
    let mut cap = TRAIN_MAX_SPEED;
    while i != t.pos {
        i = line.next(i);
        if line.step(i).slow {
            cap = TRAIN_SLOW_SPEED;
        }
    }
    let s = line.upcoming_stop(t);
    let ds = line.distance(t.pos, s);
    let reach = t.ahead.min(ds);
    let limit = line.distance_to_end(t, reach);
    let limit = stop.map_or(limit, |s| limit.min(s.max(0)));
    let mut v = (t.speed + TRAIN_ACCEL)
        .min(cap)
        .min(isqrt(2 * TRAIN_DECEL * limit));
    // 先の曲がりと分岐の手前で、遅い速さまで落とせるようにする。
    let mut d = line.step(t.pos).len - t.offset;
    let mut j = t.pos;
    for _ in 0..reach {
        j = line.next(j);
        if line.step(j).slow {
            v = v.min(isqrt(
                TRAIN_SLOW_SPEED * TRAIN_SLOW_SPEED + 2 * TRAIN_DECEL * d,
            ));
        }
        d += line.step(j).len;
    }
    let moved = v.min(limit).max(0);
    let mut rest = moved;
    while rest > 0 {
        let room = line.step(t.pos).len - t.offset;
        if rest <= room {
            t.offset += rest;
            rest = 0;
        } else {
            rest -= room;
            t.pos = line.next(t.pos);
            t.ahead -= 1;
            t.offset = 0;
        }
    }
    let s = line.step(t.pos);
    t.track = s.track;
    t.dir = s.dir;
    t.speed = moved;
    moved
}

impl StateHash for TrainState {
    fn write_state(&self, h: &mut StateHasher) {
        match *self {
            TrainState::Running => h.write(&0_u8),
            TrainState::Dwell { left } => {
                h.write(&1_u8);
                h.write(&left);
            }
            TrainState::Turnback { left } => {
                h.write(&2_u8);
                h.write(&left);
            }
            TrainState::Ready => h.write(&3_u8),
        }
    }
}

impl StateHash for Train {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.line);
        h.write(&self.pos);
        h.write(&self.track);
        h.write(&(self.dir as u8));
        h.write(&self.offset);
        h.write(&self.speed);
        h.write(&self.ahead);
        h.write(&self.state);
        h.write(&self.waited);
    }
}

#[cfg(test)]
pub(super) static OVERDELETED: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
