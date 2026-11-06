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
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, VecDeque};

use serde::{Deserialize, Serialize};

use super::train::{
    Block, DWELL_TICKS, TRAIN_ACCEL, TRAIN_BLOCK_LEN_MM, TRAIN_DECEL, TRAIN_MAX_SPEED,
    TURNBACK_TICKS,
};
use super::{LineId, Rail, RailNodeId, StationId, TrackId, heading_of, spread};
use crate::network::Dir;
use crate::round;
use crate::state_hash::{StateHash, StateHasher};
use crate::tile::TilePos;
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
    /// 列車が先頭をこの要素の先の端に置いて待てるか（待てる所）。列車が押さえる長さ（[`TRAIN_BLOCK_LEN_MM`]）
    /// 以上の長さがあり、終点（折り返す前の要素）でない要素である。ほかの路線と共有する要素も待てる所になる。
    /// 列車は、待てる所から次の待てる所までをまとめて押さえて進む（行き詰まりを防ぐ決まりの 3）。
    pub wait: bool,
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

    /// 最高の速さで走る時間の見込み（tick）。
    fn run_ticks(&self) -> u64 {
        u64::try_from(round::div_ceil(self.len, TRAIN_MAX_SPEED)).expect("線路の長さが負")
    }

    /// 止まる時間と、減速と加速の遅れと、折り返しの時間の見込み（tick）。止まらない要素は 0 である。
    fn stop_ticks(&self) -> u64 {
        match self.kind {
            StepKind::Pass => 0,
            StepKind::Stop => u64::from(DWELL_TICKS) + STOP_LOSS_TICKS,
            StepKind::Terminal => {
                u64::from(DWELL_TICKS) + STOP_LOSS_TICKS + u64::from(TURNBACK_TICKS)
            }
        }
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
    /// 止まる要素ごとの、そこに着いてから次に止まる要素に着くまでの時間（tick）。進路から見積もった値に、
    /// 列車が走って測った値を 1/4 の重みで混ぜる。進路を選び直したら見積もりに戻す。乗る時間の見積もりに使う。
    pub(crate) seg_ticks: Vec<u32>,
    /// 待てる所の区分けと本数の上限。すべての路線の進路から作り直せるので、書き出さず、状態ハッシュにも入れない。
    #[serde(skip)]
    pub(crate) plan: LinePlan,
}

/// 路線の待てる所の区分けと、本数の上限（行き詰まりを防ぐ決まりの 1 と 4）。
///
/// 待てる所は、ほかの路線か、同じ路線のほかの要素と共有するもの（共有の待てる所）と、そうでないもの
/// （自分だけの待てる所）に分かれる。共有の待てる所は、列車が続けて押さえて移る向きをたどって互いに行き来
/// できるものどうしをまとめて、共有の区画にする。路線の輪の上で、共有の待てる所に挟まれて続く自分だけの待てる所の並びを、自分だけの区間と呼ぶ。
/// すべての路線の進路から決まるので、進路が変わるたびと、書き出しから読み戻したときに作り直す。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct LinePlan {
    /// 本数の上限（[`RailLine::cap`]）。
    pub(crate) cap: u8,
    /// 要素ごとの、共有の区画の番号。共有の待てる所の要素だけが `Some` である。
    pub(crate) zone: Vec<Option<u16>>,
    /// 要素ごとの、受け持つ自分だけの区間の番号。その要素から先で最初の、自分だけの待てる所が入る区間である。
    /// 共有の待てる所を持たない路線と、自分だけの待てる所を持たない路線では `None` である。
    pub(crate) bin: Vec<Option<u16>>,
    /// 自分だけの区間ごとの、待てる所の数。
    pub(crate) runs: Vec<u32>,
    /// 通る共有の区画ごとの、この路線の列車が押さえた範囲の終わりを置ける数（区画の枠のうち、この路線の分）。
    /// 本数 0 の路線は、枠に上限のある区画では 0 である。
    pub(crate) quota: BTreeMap<u16, u32>,
    /// 終点の駅で折り返せる本数（`turn_caps`）。走らせる本数（[`RailLine::allowed`]）をこれまでに抑える。
    /// 終点のない路線は [`MAX_LINE_TRAINS`] である。
    pub(crate) turn_cap: u8,
    /// 要素ごとの、終点の駅のもう 1 つのホーム（折り返して出る向きのホーム）を使えるか。終点の駅の 2 つの要素
    /// （折り返す前と、折り返して入る要素）だけが真になりうる。駅の線路をほかの路線も、同じ路線のほかの要素も
    /// 通らないときだけ真である（`make_plans`）。
    pub(crate) alt: Vec<bool>,
}

impl LinePlan {
    /// 終点で折り返せる本数（`turn_cap`）のほかが `other` と同じか。
    fn same_zones(&self, other: &LinePlan) -> bool {
        let LinePlan {
            cap,
            zone,
            bin,
            runs,
            quota,
            turn_cap: _,
            alt,
        } = self;
        *cap == other.cap
            && *zone == other.zone
            && *bin == other.bin
            && *runs == other.runs
            && *quota == other.quota
            && *alt == other.alt
    }
}

impl RailLine {
    /// 行き詰まりを防ぐ本数の上限（決まりの 1）。16 と次の値の小さいほうである（`LinePlan`）。
    ///
    /// - 共有の待てる所を持たない路線は、「待てる所の数 − 1」。路線の中では、待てる所が列車より 1 つ多ければ、
    ///   空いた待てる所の手前の列車がいつも進める。終点の駅は、列車が着いたホームで折り返すので 1 つに数え、
    ///   もう 1 つのホームも使える終点（`LinePlan::alt`）は 2 つに数える。
    /// - 自分だけの区間が 1 つの路線は、その区間の待てる所の数。区間が 2 つ以上なら、区間の待てる所の数の和 − 1。
    ///   さらに、通る共有の区画の枠のうち、この路線の見込みの分で運べる本数までにする。
    /// - 自分だけの待てる所を持たない路線（区間運転など）は、通る共有の区画の枠のうち、この路線の見込みの分。
    ///
    /// 区画の枠は、本数 1 以上の路線で分ける。見込みの分は、本数 1 以上の路線ではいまの分で、本数 0 の路線では、
    /// 本数を 1 以上にしたとしたら配られる分である。走らせる前に、何本まで走るかを見せるためである。
    ///
    /// 待てる所が 2 つ未満の路線と、止まった路線と、通る区画の枠の見込みの分が 0 の路線は 0 である。
    pub fn cap(&self) -> u8 {
        self.plan.cap
    }

    /// 止まっておらず、本数が 1 以上か。ほかの路線のコマンドで上限が 0 になるのを断るとき、守るのはこの路線である。
    /// 本数 0 の路線は、共有の区画の枠を使わない（`LinePlan`）。
    pub(crate) fn wants_trains(&self) -> bool {
        !self.stopped && self.trains > 0
    }

    /// 列車を走らせ続けられないか。止まった路線と、上限が 0 の路線と、通る共有の区画のどれかで分が 0 の路線である。
    /// 分が 0 の区画には入れないので、残した列車は区画の手前で止まり続ける。分が 0 になるのは、本数 0 の路線が
    /// 枠に上限のある区画を通るときと、見込みの分が 0 で上限も 0 の路線だけである。
    pub(crate) fn cannot_run(&self) -> bool {
        self.stopped || self.cap() == 0 || self.plan.quota.values().any(|q| *q == 0)
    }

    /// 走らせる本数。遊ぶ人が決めた本数を、行き詰まりを防ぐ上限と、終点の駅で折り返せる本数
    /// （[`RailLine::turn_cap`]）に抑えたものである。
    pub fn allowed(&self) -> u8 {
        self.trains.min(self.cap()).min(self.turn_cap())
    }

    /// 終点の駅で折り返せる本数（`turn_caps`）。これより多く走らせても、列車は終点の手前に並ぶだけで、運べる量は
    /// 増えず、1 周が延びて乗る人の時間が延びる（#369）。終点のない環状の路線は [`MAX_LINE_TRAINS`] である。
    pub fn turn_cap(&self) -> u8 {
        self.plan.turn_cap
    }

    /// 1 周の見込みの時間（tick）。進路を最高の速さで走る時間に、止まる駅ごとの止まる時間と減速と加速の遅れ、
    /// 終点ごとの折り返しの時間を足す。
    pub fn round_trip_ticks(&self) -> u64 {
        (0..self.len())
            .map(|i| self.run_ticks(i) + self.stop_ticks(i))
            .sum()
    }

    /// 要素 `i` を最高の速さで走る時間の見込み（tick）。
    pub(crate) fn run_ticks(&self, i: u32) -> u64 {
        self.step(i).run_ticks()
    }

    /// 要素 `i` で止まる時間と、減速と加速の遅れと、折り返しの時間の見込み（tick）。止まらない要素は 0 である。
    pub(crate) fn stop_ticks(&self, i: u32) -> u64 {
        self.step(i).stop_ticks()
    }

    /// 測った値を混ぜた 1 周の時間（tick）。
    pub fn measured_round_trip_ticks(&self) -> u64 {
        self.seg_ticks.iter().map(|t| u64::from(*t)).sum()
    }

    /// 止まる要素ごとの、次に止まる要素に着くまでの時間（tick）。
    pub fn segment_ticks(&self) -> &[u32] {
        &self.seg_ticks
    }

    /// 運転間隔の見込み（tick）。1 周の見込みの時間を走らせる本数で割る。列車がなければ `None`。
    pub fn headway_ticks(&self) -> Option<u64> {
        let n = u64::from(self.allowed());
        (n > 0).then(|| self.round_trip_ticks() / n)
    }

    /// 要素 `i` から駅を出る列車が、同じ要素から前の列車が出てから空ける tick。途中の駅では運転間隔の見込みの
    /// 半分、終点の駅（折り返して入る要素）では、測った 1 周の時間を走らせる本数で割った値の 9 割である。
    /// 終点でも、運転間隔の見込みより長くは空けない。混んで測った 1 周が延びたときに、空ける時間で列車を長く
    /// 止め、測った 1 周がさらに延びるのを防ぐためである。
    ///
    /// 終点の駅にホーム 2 つに列車が入れると、続いて着いた列車も待たずに折り返し終えるので、終点で間隔がならされ
    /// なくなる。半分しか空けないと、列車が半分の間隔で続いて走る組ができ、残りの 1 つの間隔だけが長く空く。待つ人には
    /// 運転間隔が延びたのと同じになる（#371）。測った 1 周に合わせて空けると、列車は 1 周に等しく並び直す。
    /// 9 割にするのは、空ける時間の和を 1 周より短くして、列車をそれで止めないためである。
    pub(crate) fn departure_gap(&self, i: u32) -> u64 {
        if self.pocket(i).is_none() {
            return self.headway_ticks().map_or(0, |h| h / 2);
        }
        let n = u64::from(self.allowed());
        if n == 0 {
            return 0;
        }
        (self.measured_round_trip_ticks() * 9 / (10 * n)).min(self.round_trip_ticks() / n)
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

    /// `from` から先で最初の待てる所の要素。待てる所がなければ `None`。
    pub(crate) fn next_wait(&self, from: u32) -> Option<u32> {
        let mut i = from;
        for _ in 0..self.len() {
            if self.step(i).wait {
                return Some(i);
            }
            i = self.next(i);
        }
        None
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

    /// 要素 `i` が終点の駅の要素（折り返す前か、折り返して入る要素）なら、その終点の折り返す前の要素の番号。
    pub fn pocket(&self, i: u32) -> Option<u32> {
        pocket_term(&self.path, i as usize).map(|k| u32::try_from(k).expect("進路が長すぎる"))
    }

    /// 終点 `term`（折り返す前の要素の番号）の、列車の [`super::Train::bays`] のビット。往復する路線の終点は 2 つで、
    /// 進路の最後の要素で折り返す最初の駅が 1、もう 1 つの終点が 2 である。
    pub(crate) fn bay_bit(&self, term: u32) -> u8 {
        if term + 1 == self.len() { 1 } else { 2 }
    }

    /// 要素 `i` の終点の駅で、もう 1 つのホームを使えるか（`LinePlan::alt`）。
    pub(crate) fn alt_ok(&self, i: u32) -> bool {
        self.plan.alt.get(i as usize).copied().unwrap_or(false)
    }

    /// 終点ごとのホームの選び方が `bays`（[`super::Train::bays`]）の列車が、要素 `i` で押さえるホームの線路と向き。
    ///
    /// 終点の駅では、列車は着いたホームで止まって折り返し、同じホームから出る（その場で折り返す）。そのため、
    /// 終点の駅の 2 つの要素は、どちらも着いたホーム（折り返す前の要素の線路と向き）を押さえる。もう 1 つの
    /// ホームを使える終点（[`RailLine::alt_ok`]）で、列車がそちらに入ったなら、どちらの要素もそのホームを押さえる。
    /// 終点の駅でない要素は、要素の線路と向きである。
    pub fn lane(&self, i: u32, bays: u8) -> (TrackId, Dir) {
        let s = self.step(i);
        let Some(k) = self.pocket(i) else {
            return (s.track, s.dir);
        };
        let t = self.step(k);
        if self.alt_ok(i) && bays & self.bay_bit(k) != 0 {
            (t.track, flip(t.dir))
        } else {
            (t.track, t.dir)
        }
    }

    /// ホームの選び方が `bays` の列車が、要素 `i` に入るときに押さえる閉塞。分岐のノード、線路の向き（ホーム）の順である。
    pub(crate) fn blocks_at(&self, i: u32, bays: u8) -> impl Iterator<Item = Block> {
        let (t, d) = self.lane(i, bays);
        self.step(i)
            .junction
            .map(Block::Junction)
            .into_iter()
            .chain([Block::Track(t, d)])
    }
}

/// 要素 `i` が終点の駅の要素（折り返す前か、折り返して入る要素）なら、その終点の折り返す前の要素の番号。
fn pocket_term(path: &[Step], i: usize) -> Option<usize> {
    if path[i].kind == StepKind::Terminal {
        return Some(i);
    }
    let p = (i + path.len() - 1) % path.len();
    (path[p].kind == StepKind::Terminal).then_some(p)
}

/// 要素 `i` を待てる所として数えるときの（線路、向き）。終点の駅の 2 つの要素は、列車がその場で折り返すので、
/// どちらも着くホーム（折り返す前の要素）に数える（[`RailLine::lane`]）。
fn place(path: &[Step], i: usize) -> Vertex {
    vertex(&path[pocket_term(path, i).unwrap_or(i)])
}

/// 待てる所の数。同じ（線路、向き）は 1 つに数え、もう 1 つのホームを使える終点は 2 つに数える。
fn place_count(path: &[Step], alt: &[bool]) -> usize {
    let places: BTreeSet<Vertex> = (0..path.len())
        .filter(|i| path[*i].wait)
        .map(|i| place(path, i))
        .collect();
    places.len() + (0..path.len()).filter(|i| path[*i].wait && alt[*i]).count()
}

/// （線路、向き）の頂点。
type Vertex = (TrackId, Dir);

fn flip(d: Dir) -> Dir {
    match d {
        Dir::Forward => Dir::Backward,
        Dir::Backward => Dir::Forward,
    }
}

/// 要素が待てる所（[`Step::wait`]）になるか。
fn is_wait(s: &Step) -> bool {
    s.kind != StepKind::Terminal && s.len >= TRAIN_BLOCK_LEN_MM
}

/// 待てる所の要素から、待てる所の数を数えるための（線路、向き）の組。同じ線路の同じ向きは 1 つにまとまる。
fn wait_places<'a>(steps: impl Iterator<Item = &'a Step>) -> BTreeSet<Vertex> {
    steps.map(|s| (s.track, s.dir)).collect()
}

/// 本数の上限を 16 までに抑える。
fn clamp_cap(n: u64) -> u8 {
    u8::try_from(n.min(u64::from(MAX_LINE_TRAINS))).expect("上限は 16 以下")
}

fn vertex(s: &Step) -> Vertex {
    (s.track, s.dir)
}

/// 路線が走れるか。待てる所が 2 つ未満の路線と、止まった路線は走らない。
fn runnable(path: &[Step], stopped: bool) -> bool {
    !stopped && wait_places(path.iter().filter(|s| s.wait)).len() >= 2
}

/// 向きのある図の、強く連結な成分の番号（Kosaraju の方法）。`edges` は頂点ごとの出ていく辺、`into` は入る辺である。
/// 番号は、成分の中でいちばん小さい頂点の順に 0 から振る。
fn strong_components(edges: &[BTreeSet<usize>], into: &[Vec<usize>]) -> Vec<u16> {
    let n = edges.len();
    // 1 回目：辺をたどり、たどり終えた順に頂点を並べる。
    let mut seen = vec![false; n];
    let mut finished = Vec::with_capacity(n);
    for s in 0..n {
        if seen[s] {
            continue;
        }
        seen[s] = true;
        let mut stack = vec![(s, edges[s].iter())];
        while let Some((a, it)) = stack.last_mut() {
            let a = *a;
            match it.next().copied() {
                Some(b) => {
                    if !seen[b] {
                        seen[b] = true;
                        stack.push((b, edges[b].iter()));
                    }
                }
                None => {
                    finished.push(a);
                    stack.pop();
                }
            }
        }
    }
    // 2 回目：たどり終えた順の逆に、入る辺を逆向きにたどる。1 回にたどれる頂点が 1 つの成分である。
    let mut comp = vec![usize::MAX; n];
    let mut count = 0;
    for s in finished.into_iter().rev() {
        if comp[s] != usize::MAX {
            continue;
        }
        comp[s] = count;
        let mut stack = vec![s];
        while let Some(a) = stack.pop() {
            for b in &into[a] {
                if comp[*b] == usize::MAX {
                    comp[*b] = count;
                    stack.push(*b);
                }
            }
        }
        count += 1;
    }
    // 番号を、成分の中でいちばん小さい頂点の順に振り直す。
    let mut renumber = BTreeMap::<usize, u16>::new();
    comp.iter()
        .map(|c| {
            let next = u16::try_from(renumber.len()).expect("共有の区画が多すぎる");
            *renumber.entry(*c).or_insert(next)
        })
        .collect()
}

/// 路線のコマンドで変える路線 1 本の、変えたあとの形。断るかを、状態を変えずに確かめるのに使う。
struct LineChange {
    id: LineId,
    path: Vec<Step>,
    stopped: bool,
    trains: u8,
}

/// `make_plans` に渡す路線 1 本。
#[derive(Clone, Copy)]
struct PlanLine<'a> {
    id: LineId,
    path: &'a [Step],
    stopped: bool,
    /// 遊ぶ人が決めた本数（[`RailLine::trains`]）。
    trains: u8,
}

/// 進路の 1 周の時間の見込みと、そのうち押さえた範囲の終わりを共有の区画ごとに置く時間（tick）。
///
/// 時間は [`RailLine::round_trip_ticks`] と同じ見込みである。列車は、先頭がいる要素から先で最初の待てる所まで
/// 押さえるので、要素の時間は、その要素から先で最初の待てる所の区画に数える。
fn zone_ticks(p: &[Step], zone: &[Option<u16>]) -> (u64, BTreeMap<u16, u64>) {
    let n = p.len();
    let ticks = |s: &Step| s.run_ticks() + s.stop_ticks();
    let total: u64 = p.iter().map(ticks).sum();
    let mut in_zone = BTreeMap::<u16, u64>::new();
    // 後ろから 2 周たどり、要素ごとに、そこから先で最初の待てる所の区画を求める。
    let mut next: Option<Option<u16>> = None;
    for d in (0..2 * n).rev() {
        let i = d % n;
        if p[i].wait {
            next = Some(zone[i]);
        }
        if d < n
            && let Some(Some(z)) = next
        {
            *in_zone.entry(z).or_default() += ticks(&p[i]);
        }
    }
    (total, in_zone)
}

/// 区画の枠 `room` を、区画を使う路線 `users`（（自分だけの待てる所を持つか、路線）の順）で等しく分けたときの、
/// `user` の分。並びの順に 1 つずつ配り、余りは並びの先の路線に行く。
///
/// 余りを自分だけの待てる所を持たない路線（区間運転など）から先に配るのは、これらの路線の上限は分そのもので、
/// 余りの 1 つが列車 1 本になるためである。自分だけの区間を持つ路線は、分が 1 でも区間の数まで列車を置ける。
///
/// 路線の分は `limit`（終点のホームで折り返せる本数、`turn_caps`）を超えない。路線は、これより多くの列車を
/// 走らせないので、超える分は使われずに残る。残る分は、ほかの路線に配る（#377）。
fn split_zone(
    room: u32,
    users: &[(bool, LineId)],
    user: (bool, LineId),
    limit: impl Fn(LineId) -> u32,
) -> u32 {
    let mut got = vec![0_u32; users.len()];
    let mut left = room;
    while left > 0 {
        let mut dealt = false;
        for (i, u) in users.iter().enumerate() {
            if left > 0 && got[i] < limit(u.1) {
                got[i] += 1;
                left -= 1;
                dealt = true;
            }
        }
        if !dealt {
            break;
        }
    }
    got[users
        .iter()
        .position(|u| *u == user)
        .expect("区画を使う路線にない")]
}

/// 路線ごとの待てる所の区分けと本数の上限（`LinePlan`）を求める。
///
/// 共有の区画の枠（押さえた範囲の終わりを区画に置ける列車の数）は、区画の中の待てる所だけを通る輪のうち、
/// いちばん短いものの長さ − 1 である。区画の中に輪がなければ上限はない（`u32::MAX`）。走らない路線は、共有の数えに入れない。
/// `limits` は、路線ごとの区画の分の上限（終点のホームで折り返せる本数）で、ない路線は上限なしである（`split_zone`）。
fn make_plans(lines: &[PlanLine], limits: &BTreeMap<LineId, u8>) -> BTreeMap<LineId, LinePlan> {
    let running = || lines.iter().filter(|l| runnable(l.path, l.stopped));
    let mut uses = BTreeMap::<Vertex, u32>::new();
    let mut waits = BTreeSet::<Vertex>::new();
    // 線路と向きを、終点の駅をホームに読み替えずに数えたもの。もう 1 つのホームを使えるかを決めるのに使う。
    let mut raw = BTreeMap::<Vertex, u32>::new();
    for l in running() {
        for (i, s) in l.path.iter().enumerate() {
            *raw.entry(vertex(s)).or_default() += 1;
            // 終点の折り返す前の要素は、折り返して入る要素と同じホームなので、そちらで 1 度だけ数える。
            if s.kind == StepKind::Terminal {
                continue;
            }
            *uses.entry(place(l.path, i)).or_default() += 1;
            if s.wait {
                waits.insert(place(l.path, i));
            }
        }
    }
    // 共有の待てる所に、（線路の ID、向き）の順で番号を振る。
    let nodes: Vec<Vertex> = waits.into_iter().filter(|v| uses[v] >= 2).collect();
    let index: BTreeMap<Vertex, usize> = nodes.iter().enumerate().map(|(i, v)| (*v, i)).collect();
    // 共有の待てる所 a にいる列車が、次の待てる所まで押さえるときに通る、ほかの共有の待てる所 b への辺。
    // b に止まっている列車がいれば、a の列車はその列車を待つ。
    let mut edges = vec![BTreeSet::<usize>::new(); nodes.len()];
    for l in running() {
        let p = l.path;
        let n = p.len();
        for i in 0..n {
            let Some(&a) = index.get(&place(p, i)).filter(|_| p[i].wait) else {
                continue;
            };
            // 待てる所が 2 つ以上あるので、1 周するまでに次の待てる所に着く。
            let mut j = (i + 1) % n;
            loop {
                if let Some(&b) = index.get(&place(p, j))
                    && b != a
                {
                    edges[a].insert(b);
                }
                if p[j].wait {
                    break;
                }
                j = (j + 1) % n;
            }
        }
    }
    let mut into = vec![Vec::<usize>::new(); nodes.len()];
    for (a, out) in edges.iter().enumerate() {
        for b in out {
            into[*b].push(a);
        }
    }
    // 辺をたどって互いに行き来できるものを、1 つの区画にする（強く連結な成分）。輪は必ず 1 つの区画に収まる。
    // 辺が入るだけか出るだけの待てる所は、ほかの待てる所と輪を作らないので、それだけで区画になり、枠に上限はない。
    // 向きなしにつながるものまでまとめると、ある路線の輪の区画の枠を、輪に入らない待てる所で待つだけの路線
    // （ほかの路線が終点で折り返すホームから出る路線など）にまで分けることになり、その路線の分け前が 0 になって
    // 走れなくなる（#356）。
    let zone_of = strong_components(&edges, &into);
    let zones = zone_of.iter().max().map_or(0, |z| usize::from(*z) + 1);
    // 区画の中のいちばん短い輪。待てる所ごとに、辺をたどってそこへ戻るまでの最短の辺の数を求める。
    let mut girth = vec![u32::MAX; zones];
    for s in 0..nodes.len() {
        let mut dist = vec![u32::MAX; nodes.len()];
        dist[s] = 0;
        let mut queue = VecDeque::from([s]);
        while let Some(a) = queue.pop_front() {
            for b in &edges[a] {
                if dist[*b] == u32::MAX {
                    dist[*b] = dist[a] + 1;
                    queue.push_back(*b);
                }
            }
        }
        let z = usize::from(zone_of[s]);
        for u in &into[s] {
            if dist[*u] != u32::MAX {
                girth[z] = girth[z].min(dist[*u] + 1);
            }
        }
    }
    let zone_caps: Vec<u32> = girth
        .iter()
        .map(|g| if *g == u32::MAX { u32::MAX } else { g - 1 })
        .collect();
    let mut plans = BTreeMap::new();
    // 自分だけの区間を持つ路線の、1 周の時間と区画ごとの時間。
    let mut times = BTreeMap::<LineId, (u64, BTreeMap<u16, u64>)>::new();
    for l in lines {
        let (k, p) = (l.id, l.path);
        let n = p.len();
        if !runnable(p, l.stopped) {
            plans.insert(
                k,
                LinePlan {
                    cap: 0,
                    zone: vec![None; n],
                    bin: vec![None; n],
                    runs: Vec::new(),
                    quota: BTreeMap::new(),
                    turn_cap: MAX_LINE_TRAINS,
                    alt: vec![false; n],
                },
            );
            continue;
        }
        // 終点の駅の線路を、ほかの路線も、同じ路線のほかの要素も通らなければ、もう 1 つのホームも使える。
        // その終点は、列車 2 本が入れる待てる所になる。
        let alt: Vec<bool> = (0..n)
            .map(|i| {
                pocket_term(p, i).is_some_and(|t| {
                    let (track, dir) = vertex(&p[t]);
                    raw[&(track, dir)] == 1 && raw.get(&(track, flip(dir))) == Some(&1)
                })
            })
            .collect();
        let zone: Vec<Option<u16>> = (0..n)
            .map(|i| {
                index
                    .get(&place(p, i))
                    .filter(|_| p[i].wait)
                    .map(|a| zone_of[*a])
            })
            .collect();
        let Some(z0) = zone.iter().position(Option::is_some) else {
            // 共有の待てる所がなければ、路線の輪だけを見る。
            let places = place_count(p, &alt) as u64;
            plans.insert(
                k,
                LinePlan {
                    cap: clamp_cap(places - 1),
                    zone,
                    bin: vec![None; n],
                    runs: Vec::new(),
                    quota: BTreeMap::new(),
                    turn_cap: MAX_LINE_TRAINS,
                    alt,
                },
            );
            continue;
        };
        // 共有の待てる所の次から 1 周たどり、続く自分だけの待てる所を 1 つの区間にまとめる。
        let mut runs: Vec<u32> = Vec::new();
        let mut run_at: Vec<Option<u16>> = vec![None; n];
        let mut cur: Option<u16> = None;
        for d in 1..=n {
            let i = (z0 + d) % n;
            if !p[i].wait {
                continue;
            }
            if zone[i].is_some() {
                cur = None;
                continue;
            }
            let r = *cur.get_or_insert_with(|| {
                runs.push(0);
                u16::try_from(runs.len() - 1).expect("区間が多すぎる")
            });
            runs[usize::from(r)] += 1 + u32::from(alt[i]);
            run_at[i] = Some(r);
        }
        // 要素ごとに、そこから先で最初の自分だけの待てる所の区間。
        let mut bin = vec![None; n];
        let mut next = None;
        for d in (0..2 * n).rev() {
            let i = d % n;
            if run_at[i].is_some() {
                next = run_at[i];
            }
            if d < n {
                bin[i] = next;
            }
        }
        // 自分だけの待てる所を持たない路線の上限は、区画ごとに分けたあとで決める。
        let cap = match runs.as_slice() {
            [] => 0,
            [only] => clamp_cap(u64::from(*only)),
            many => clamp_cap(many.iter().map(|r| u64::from(*r)).sum::<u64>() - 1),
        };
        if !runs.is_empty() {
            times.insert(k, zone_ticks(p, &zone));
        }
        plans.insert(
            k,
            LinePlan {
                cap,
                zone,
                bin,
                runs,
                quota: BTreeMap::new(),
                turn_cap: MAX_LINE_TRAINS,
                alt,
            },
        );
    }
    let trains: BTreeMap<LineId, u8> = lines.iter().map(|l| (l.id, l.trains)).collect();
    let limit = |k: LineId| limits.get(&k).map_or(u32::MAX, |n| u32::from(*n).max(1));
    // 区画の枠を、区画を使う路線のうち本数が 1 以上の路線で等しく分ける（`split_zone`）。路線の列車は、自分の
    // 路線の分の枠だけを使う。本数 0 の路線（使っていない路線）の分は 0 である。上限は、その路線の本数を 1 以上に
    // したとしたら配られる分（見込みの分）から決める。本数 1 以上の路線では、いまの分と同じである。
    // 路線の分は `limits` を超えず、超える分はほかの路線に配る。
    let mut potential = BTreeMap::<(LineId, u16), u32>::new();
    for (z, zone_cap) in zone_caps.iter().enumerate() {
        let z16 = u16::try_from(z).expect("共有の区画が多すぎる");
        let mut users: Vec<(bool, LineId)> = plans
            .iter()
            .filter(|(_, p)| p.zone.contains(&Some(z16)))
            .map(|(k, p)| (!p.runs.is_empty(), *k))
            .collect();
        users.sort();
        let active: Vec<(bool, LineId)> = users
            .iter()
            .filter(|(_, k)| trains[k] > 0)
            .copied()
            .collect();
        for user in &users {
            let k = user.1;
            let (share, seen) = if *zone_cap == u32::MAX {
                (u32::MAX, u32::MAX)
            } else if trains[&k] > 0 {
                let share = split_zone(*zone_cap, &active, *user, limit);
                (share, share)
            } else {
                let mut with = active.clone();
                with.push(*user);
                with.sort();
                (0, split_zone(*zone_cap, &with, *user, limit))
            };
            plans
                .get_mut(&k)
                .expect("路線がない")
                .quota
                .insert(z16, share);
            potential.insert((k, z16), seen);
        }
    }
    for l in lines {
        let plan = plans.get_mut(&l.id).expect("路線がない");
        let mine = potential.range((l.id, 0)..=(l.id, u16::MAX));
        match times.get(&l.id) {
            // 自分だけの待てる所を持たない路線の列車は、いつも区画の中にいて枠を持ち続けるので、見込みの分が
            // そのまま上限になる。進路がすべて 1 つの輪の上にあるので、通る区画は 1 つである。
            None if plan.runs.is_empty() && !plan.quota.is_empty() => {
                plan.cap = mine
                    .map(|(_, q)| clamp_cap(u64::from(*q)))
                    .min()
                    .unwrap_or(0);
            }
            None => {}
            // 自分だけの区間を持つ路線は、通る区画の枠のうち自分の見込みの分で運べる本数までにする。区画 z の分が q で、
            // 列車が 1 周のうち押さえた範囲の終わりを z に置く時間が t_z、1 周の時間が t なら、z を通れるのは t_z の
            // あいだに q 本で、1 周で運べるのは q × t ÷ t_z 本の分である。これより多く走らせても、増えた列車は区画の
            // 手前の自分だけの区間で枠を待つだけで、運べる量は増えず、維持費だけがかかる（#356）。
            Some((total, in_zone)) => {
                let mut cap = plan.cap;
                for ((_, z), q) in mine {
                    if *q == 0 {
                        cap = 0;
                    } else if *q != u32::MAX {
                        cap = cap.min(clamp_cap((u64::from(*q) * total).div_ceil(in_zone[z])));
                    }
                }
                plan.cap = cap;
            }
        }
    }
    plans
}

/// 終点の駅で、列車 1 本がホーム 1 つを押さえる時間の見込み（tick）。止まっている状態から駅の線路
/// （長さ `len`）へ入って止まる時間、止まる時間、折り返す時間、出た列車の尾が駅の線路を抜ける時間の和である。
/// 列車は着いたホームで折り返して同じホームから出る（行き詰まりを防ぐ決まりの 2）ので、ホーム 1 つが列車を
/// 通せるのは、この時間に 1 本までである。
pub(crate) fn turn_ticks(len: i64) -> u64 {
    let enter = isqrt_ceil(round::div_ceil(2 * len, TRAIN_DECEL));
    let clear = isqrt_ceil(round::div_ceil(2 * TRAIN_BLOCK_LEN_MM, TRAIN_ACCEL));
    enter + u64::from(DWELL_TICKS) + u64::from(TURNBACK_TICKS) + clear
}

/// `v` の平方根を切り上げる。
fn isqrt_ceil(v: i64) -> u64 {
    let v = u64::try_from(v).expect("負の数の平方根");
    let r = v.isqrt();
    if r * r == v { r } else { r + 1 }
}

/// 終点の駅の使い方の割合を表す、1 の大きさ。
pub(crate) const TURN_FULL: u64 = 1_000_000;

/// ほかの路線が折り返すホームを、通り抜ける列車 1 本が押さえる時間の見込み（tick）。止まる駅なら、入って止まる
/// 時間、止まる時間、尾が抜ける時間の和（[`turn_ticks`] から折り返す時間を除いたもの）、止まらずに通るなら、
/// 最高の速さで先頭が入ってから尾が抜けるまでの時間である。
fn through_ticks(s: &Step) -> u64 {
    if s.is_stop() {
        turn_ticks(s.len) - u64::from(TURNBACK_TICKS)
    } else {
        u64::try_from(round::div_ceil(s.len + TRAIN_BLOCK_LEN_MM, TRAIN_MAX_SPEED))
            .expect("線路の長さが負")
    }
}

/// 路線ごとの、終点の着くホームを使う割合と、ホームごとの使える割合の和（`platform_use`）。
type PlatformUse = (
    BTreeMap<LineId, BTreeMap<Vertex, u64>>,
    BTreeMap<Vertex, u64>,
);

/// 終点の着くホームを使う割合。路線ごとの、ホームと、列車 1 本がそのホームの時間を使う割合（[`TURN_FULL`] を 1 とする）
/// と、ホームごとの使える割合の和（ホームの数 × 1）である。上限（`cap`）が 0 の路線は数えない。
///
/// 往復する路線の列車は、1 周に終点ごとに 1 度、着いたホームを [`turn_ticks`] のあいだ押さえる。1 周の見込みの
/// 時間を `t` とすると、列車 1 本はそのホームの時間の `turn_ticks ÷ t` を使う。ほかの路線が折り返すホームを
/// 通り抜ける路線（そこに止まる路線と、止まらずに通る路線）も、列車 1 本が [`through_ticks`] ÷ `t` を使う（#377）。
/// 折り返す列車がホームにいるあいだ、通り抜ける列車はそのホームに入れず、通り抜ける列車がいるあいだは、折り返す
/// 列車が入れないためである。
fn platform_use(lines: &[PlanLine], plans: &BTreeMap<LineId, LinePlan>) -> PlatformUse {
    let mut uses = BTreeMap::<LineId, BTreeMap<Vertex, u64>>::new();
    let mut room = BTreeMap::<Vertex, u64>::new();
    let total = |l: &PlanLine| -> u64 {
        l.path
            .iter()
            .map(|s| s.run_ticks() + s.stop_ticks())
            .sum::<u64>()
            .max(1)
    };
    for l in lines {
        let plan = &plans[&l.id];
        if plan.cap == 0 {
            continue;
        }
        let t = total(l);
        let mut u = BTreeMap::new();
        for (i, s) in l.path.iter().enumerate() {
            if s.kind != StepKind::Terminal {
                continue;
            }
            *u.entry(vertex(s)).or_default() += (turn_ticks(s.len) * TURN_FULL).div_ceil(t);
            let r = (1 + u64::from(plan.alt[i])) * TURN_FULL;
            room.entry(vertex(s))
                .and_modify(|x| *x = (*x).min(r))
                .or_insert(r);
        }
        uses.insert(l.id, u);
    }
    // 終点の要素（折り返す前と、折り返して入る要素）でない要素が、どれかの路線の終点の着くホームを通るなら、
    // 通り抜ける分を足す。
    for l in lines {
        let Some(u) = uses.get_mut(&l.id) else {
            continue;
        };
        let t = total(l);
        for (i, s) in l.path.iter().enumerate() {
            if pocket_term(l.path, i).is_some() || !room.contains_key(&vertex(s)) {
                continue;
            }
            *u.entry(vertex(s)).or_default() += (through_ticks(s) * TURN_FULL).div_ceil(t);
        }
    }
    (uses, room)
}

/// 路線ごとの、終点の駅で折り返せる本数（[`LinePlan::turn_cap`]）。
///
/// 終点の着くホームを使う割合（`platform_use`）の和がホームの数を超えると、列車は終点の手前の線路に並び、1 周が
/// 延びるだけで運べる量は増えない（#369）。そこで、本数が 1 以上の路線に、ID の順に 1 本ずつ配り、どの終点も
/// 使う割合の和がホームの数を超えない所まで配る。どの路線も 1 本は走らせる。本数 0 の路線は、本数を 1 以上に
/// したとしたら配られる本数にする（区画の枠の見込みの分と同じ考え）。配るのは、ほかの上限（`cap`）までである。
/// 終点がなく、ほかの路線の終点のホームも通らない路線は、ほかの上限のままになる。
fn turn_caps(lines: &[PlanLine], plans: &BTreeMap<LineId, LinePlan>) -> BTreeMap<LineId, u8> {
    let (uses, room) = platform_use(lines, plans);
    let deal = |active: &[LineId]| -> BTreeMap<LineId, u8> {
        let mut n = BTreeMap::<LineId, u8>::new();
        let mut load = BTreeMap::<Vertex, u64>::new();
        loop {
            let mut dealt = false;
            for id in active {
                let have = n.get(id).copied().unwrap_or(0);
                if have >= plans[id].cap {
                    continue;
                }
                let u = &uses[id];
                let fits = u
                    .iter()
                    .all(|(k, x)| load.get(k).copied().unwrap_or(0) + x <= room[k]);
                if have > 0 && !fits {
                    continue;
                }
                for (k, x) in u {
                    *load.entry(*k).or_default() += x;
                }
                n.insert(*id, have + 1);
                dealt = true;
            }
            if !dealt {
                return n;
            }
        }
    };
    let mut active: Vec<LineId> = lines
        .iter()
        .filter(|l| l.trains > 0 && uses.contains_key(&l.id))
        .map(|l| l.id)
        .collect();
    active.sort();
    let mut out = deal(&active);
    for l in lines {
        if l.trains > 0 || !uses.contains_key(&l.id) {
            continue;
        }
        let mut with = active.clone();
        with.push(l.id);
        with.sort();
        out.insert(l.id, deal(&with)[&l.id]);
    }
    out.retain(|id, _| !uses[id].is_empty());
    out
}

/// 路線の本数の上限と、その内訳。読み取り用で、状態には持たない。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineCapacity {
    /// 行き詰まりを防ぐ本数の上限（[`RailLine::cap`]）。
    pub cap: u8,
    /// 待てる所の数。ほかの路線と共有する待てる所も数え、もう 1 つのホームを使える終点は 2 つに数える。
    /// 2 つ未満なら上限は 0 で、列車は走らない。
    /// ほかの路線と待てる所を共有しなければ、上限は 16 と「これ − 1」の小さいほうである。共有すれば、上限は
    /// 自分だけの区間の待てる所の数と、共有の区画の枠のうち自分の路線の分から決まる（`LinePlan`）。
    /// 2 つ以上あって上限が 0 なのは、区画の枠が回ってこないときである。
    pub wait_places: usize,
    /// 進路の線路を同じ向きに使う、ほかの路線（ID の順）。共有する待てる所は、区画の枠をこれらの路線と分ける。
    pub shared_with: Vec<LineId>,
    /// 進路の中で、待てる所になれない短い線路（長さが [`TRAIN_BLOCK_LEN_MM`] 未満で、終点の要素でないもの）。
    /// 線路を分岐で割ると、割れた短い線路がここに出る。進路に出てくる順で、同じ線路は 1 つにまとめる。
    pub short_spans: Vec<ShortSpan>,
    /// 終点の駅で折り返せる本数（[`RailLine::turn_cap`]）。走らせる本数はこれまでに抑える。
    pub turn_cap: u8,
    /// 終点の駅で折り返せる本数が上限より小さいとき、その本数を決めている終点。そうでなければ `None`。
    pub turn_limit: Option<TurnLimit>,
}

/// 終点の駅で折り返せる本数（[`RailLine::turn_cap`]）を決めている終点。読み取り用で、状態には持たない。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnLimit {
    /// 終点の駅。
    pub station: StationId,
    /// 列車が折り返せるホームの数（1 か 2）。駅の線路をほかの路線も通る終点は 1 である。
    pub platforms: u8,
    /// 同じホームを使う、本数 1 以上のほかの路線（ID の順）。このホームの時間を分け合う。同じホームに着いて折り返す
    /// 路線と、そのホームを通り抜ける路線（#377）である。
    pub shared_with: Vec<LineId>,
}

/// 待てる所になれない短い線路 1 本。端のマスは線路のノードのマスである。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShortSpan {
    pub from: TilePos,
    pub to: TilePos,
    /// 線路の長さ（mm）。
    pub len_mm: i64,
}

impl Rail {
    /// 路線を ID の順に返す。
    pub fn lines(&self) -> impl Iterator<Item = (LineId, &RailLine)> {
        self.lines.iter()
    }

    pub fn line(&self, id: LineId) -> Option<&RailLine> {
        self.lines.get(id)
    }

    /// 路線の本数の上限と、その内訳。上限が 0 の理由（待てる所が足りない、どの路線と共有しているか）を見せるのに使う。
    pub fn line_capacity(&self, id: LineId) -> Option<LineCapacity> {
        let line = self.lines.get(id)?;
        let lanes =
            |l: &RailLine| -> BTreeSet<Vertex> { (0..l.len()).map(|i| l.lane(i, 0)).collect() };
        let mine = lanes(line);
        let shared_with = self
            .lines
            .iter()
            .filter(|(other, l)| *other != id && !lanes(l).is_disjoint(&mine))
            .map(|(other, _)| other)
            .collect();
        Some(LineCapacity {
            cap: line.cap(),
            wait_places: if line.plan.alt.len() == line.path.len() {
                place_count(&line.path, &line.plan.alt)
            } else {
                wait_places(line.path.iter().filter(|s| s.wait)).len()
            },
            shared_with,
            short_spans: self.short_spans(line),
            turn_cap: line.turn_cap(),
            turn_limit: self.turn_limit(id),
        })
    }

    /// 路線 `id` の終点の駅で折り返せる本数（[`RailLine::turn_cap`]）が上限より小さいとき、その本数を決めている終点。
    /// 列車をもう 1 本走らせると、ホームを使う割合の和がホームの数を超える終点のホームのうち、進路で先に出てくる
    /// ものである（`turn_caps` と同じ数え方）。ほかの路線が折り返すホームを通り抜ける路線では、そのホームの駅になる。
    fn turn_limit(&self, id: LineId) -> Option<TurnLimit> {
        let line = self.lines.get(id)?;
        if line.cap() == 0 || line.turn_cap() >= line.cap() {
            return None;
        }
        let input: Vec<PlanLine> = self
            .lines
            .iter()
            .map(|(id, l)| PlanLine {
                id,
                path: &l.path,
                stopped: l.stopped,
                trains: l.trains,
            })
            .collect();
        let plans: BTreeMap<LineId, LinePlan> = self
            .lines
            .iter()
            .map(|(k, l)| (k, l.plan.clone()))
            .collect();
        let (uses, room) = platform_use(&input, &plans);
        let mine = uses.get(&id)?;
        line.path.iter().enumerate().find_map(|(i, s)| {
            let v = vertex(s);
            let x = mine.get(&v)?;
            // 終点の折り返して入る要素は、折り返す前の要素と同じホームなので、そちらで見る。
            if pocket_term(&line.path, i).is_some_and(|k| k != i) {
                return None;
            }
            let mut load = x * (u64::from(line.turn_cap()) + 1);
            let mut shared_with = Vec::new();
            for (other, l) in self.lines.iter() {
                if other == id || l.trains == 0 {
                    continue;
                }
                if let Some(y) = uses.get(&other).and_then(|u| u.get(&v)) {
                    load += y * u64::from(l.turn_cap());
                    shared_with.push(other);
                }
            }
            let platforms = u8::try_from(room[&v] / TURN_FULL).expect("ホームは 2 つまで");
            let station = self.tracks.get(v.0)?.station?;
            (load > room[&v]).then_some(TurnLimit {
                station,
                platforms,
                shared_with,
            })
        })
    }

    /// 路線の進路にある、待てる所になれない短い線路。
    fn short_spans(&self, line: &RailLine) -> Vec<ShortSpan> {
        let mut seen = BTreeSet::new();
        line.path
            .iter()
            .filter(|s| s.kind != StepKind::Terminal && s.len < TRAIN_BLOCK_LEN_MM)
            .filter(|s| seen.insert(s.track))
            .filter_map(|s| {
                let l = self.track_line(s.track)?;
                Some(ShortSpan {
                    from: l.start,
                    to: l.end(),
                    len_mm: s.len,
                })
            })
            .collect()
    }

    /// コマンド `command`（`CreateLine`、`ExtendLine`、`InsertLineStation`、`SetLineTrains`）が `LineStarvesOther` で
    /// 断られるなら、そのせいで上限が 0 になる路線のうち、ID のいちばん小さいもの。断られないコマンドと、路線の
    /// コマンドでないものは `None`。状態を変えない。
    pub fn starved_by_command(&self, command: &crate::command::Command) -> Option<LineId> {
        let change = self.command_change(command)?;
        self.starved_in(change.id, &self.plans_with(&change))
    }

    /// コマンド `command`（`CreateLine`、`ExtendLine`、`InsertLineStation`、`SetLineTrains`）を受け付けたとしたら
    /// 上限が下がる、ほかの路線と、その前と後の上限（ID の順）。止まった路線と本数 0 の路線は数えない。`LineStarvesSelf` で断った
    /// 理由を見せるのに使う。路線のコマンドでないものと、進路を選べないものは空である。状態を変えない。
    pub fn lowered_by_command(&self, command: &crate::command::Command) -> Vec<(LineId, u8, u8)> {
        let Some(change) = self.command_change(command) else {
            return Vec::new();
        };
        let plans = self.plans_with(&change);
        self.lines
            .iter()
            .filter(|(l, line)| *l != change.id && line.wants_trains() && plans[l].cap < line.cap())
            .map(|(l, line)| (l, line.cap(), plans[&l].cap))
            .collect()
    }

    /// 路線のコマンドで変わる路線（新しく作る路線なら、作ったときに付く ID）と、変えたあとの進路と本数。
    /// 路線のコマンドでないものと、駅か路線がないもの、進路を選べないものは `None`。
    fn command_change(&self, command: &crate::command::Command) -> Option<LineChange> {
        use crate::command::Command;
        let keep = |id: LineId, path: Vec<Step>| {
            let l = self.lines.get(id).expect("路線がない");
            LineChange {
                id,
                path,
                stopped: false,
                trains: l.trains,
            }
        };
        match command {
            Command::CreateLine { from, to } => {
                self.stations.get(*from)?;
                self.stations.get(*to)?;
                let path = self.plan(&[*from, *to], false)?;
                Some(LineChange {
                    id: self.lines.next_id(),
                    path,
                    stopped: false,
                    trains: 0,
                })
            }
            Command::ExtendLine { line, station } => {
                let l = self.lines.get(*line)?;
                self.stations.get(*station)?;
                let closes = l.stations[0] == *station;
                let mut stations = l.stations.clone();
                if !closes {
                    stations.push(*station);
                }
                let path = self.plan(&stations, closes)?;
                Some(keep(*line, path))
            }
            Command::InsertLineStation {
                line,
                after_index,
                station,
            } => {
                let l = self.lines.get(*line)?;
                self.stations.get(*station)?;
                let at = usize::from(*after_index) + 1;
                if at > l.stations.len() {
                    return None;
                }
                let mut stations = l.stations.clone();
                stations.insert(at, *station);
                let path = self.plan(&stations, l.looped)?;
                Some(keep(*line, path))
            }
            Command::SetLineTrains { line, trains } => {
                let l = self.lines.get(*line)?;
                Some(LineChange {
                    id: *line,
                    path: l.path.clone(),
                    stopped: l.stopped,
                    trains: (*trains).min(MAX_LINE_TRAINS),
                })
            }
            _ => None,
        }
    }

    /// 路線を `change` のとおりに変えたとしたときの、すべての路線の区分け。
    fn plans_with(&self, change: &LineChange) -> BTreeMap<LineId, LinePlan> {
        let input: Vec<PlanLine> = self
            .lines
            .iter()
            .filter(|(l, _)| *l != change.id)
            .map(|(id, l)| PlanLine {
                id,
                path: &l.path,
                stopped: l.stopped,
                trains: l.trains,
            })
            .chain([PlanLine {
                id: change.id,
                path: &change.path,
                stopped: change.stopped,
                trains: change.trains,
            }])
            .collect();
        self.plans_for(&input)
    }

    /// 区分けを `plans` にしたとき、上限が 0 でなかったのに 0 になる、路線 `id` のほかの路線のうち、ID のいちばん
    /// 小さいもの。止まった路線と、本数 0 の路線（使っていない路線）は数えない。
    fn starved_in(&self, id: LineId, plans: &BTreeMap<LineId, LinePlan>) -> Option<LineId> {
        self.lines
            .iter()
            .filter(|(l, _)| *l != id)
            .find(|(l, line)| line.wants_trains() && line.cap() > 0 && plans[l].cap == 0)
            .map(|(l, _)| l)
    }

    /// 路線を `change` のとおりに変えるのを断る理由。ほかの路線の上限を 0 にするなら `LineStarvesOther`、
    /// 変える路線そのものの上限が 0 で、ほかの路線の上限を下げるなら `LineStarvesSelf` である。後者は、作っても
    /// 走らない路線のために、ほかの路線の列車が消えるだけで、遊ぶ人に得がない。
    /// 断らなければ、変えたあとの区分けを返す。
    fn deny_change(&self, change: &LineChange) -> Result<BTreeMap<LineId, LinePlan>, DenyReason> {
        let plans = self.plans_with(change);
        if self.starved_in(change.id, &plans).is_some() {
            return Err(DenyReason::LineStarvesOther);
        }
        let lowers_other = self
            .lines
            .iter()
            .filter(|(l, _)| *l != change.id)
            .any(|(l, line)| line.wants_trains() && plans[&l].cap < line.cap());
        if plans[&change.id].cap == 0 && lowers_other {
            return Err(DenyReason::LineStarvesSelf);
        }
        Ok(plans)
    }

    /// 路線 `id` の進路を `path` にするのを断る理由（[`Rail::deny_change`]）。本数は変えない。
    fn deny_path(&self, id: LineId, path: &[Step]) -> Result<(), DenyReason> {
        let trains = self.lines.get(id).map_or(0, |l| l.trains);
        self.deny_change(&LineChange {
            id,
            path: path.to_vec(),
            stopped: false,
            trains,
        })
        .map(|_| ())
    }

    /// すべての路線の待てる所の印と区分け（`LinePlan`）を作り直す。
    fn replan(&mut self) {
        let ids: Vec<LineId> = self.lines.iter().map(|(id, _)| id).collect();
        for id in &ids {
            for s in &mut self.lines.get_mut(*id).expect("路線がない").path {
                s.wait = is_wait(s);
            }
        }
        for (id, plan) in self.current_plans() {
            self.lines.get_mut(id).expect("路線がない").plan = plan;
        }
    }

    /// いまの路線の進路と本数から作った、すべての路線の区分け。終点で折り返せる本数（`turn_caps`）も入れる。
    /// 待てる所の印は、いまの印のまま使う。
    fn current_plans(&self) -> BTreeMap<LineId, LinePlan> {
        let input: Vec<PlanLine> = self
            .lines
            .iter()
            .map(|(id, l)| PlanLine {
                id,
                path: &l.path,
                stopped: l.stopped,
                trains: l.trains,
            })
            .collect();
        self.plans_for(&input)
    }

    /// 路線 `input` の区分け。終点で折り返せる本数（`turn_caps`）も入れ、区画の分をその本数までにする。
    ///
    /// 区画の分は、まず折り返せる本数を見ずに分け、そこから折り返せる本数を求め、その本数を分の上限にして分け直す
    /// （`split_zone`）。分け直すと、上限で余った分がほかの路線に回り、その路線の上限（`cap`）が上がることがある。
    /// 分が折り返せる本数まで下がった路線の `cap` は、分け直す前の値のままにする。`cap` は行き詰まりを防ぐ上限で、
    /// 走らせる本数（[`RailLine::allowed`]）は折り返せる本数でも抑えるので、分を超える列車は走らない。`cap` まで
    /// 下げると、折り返せる本数で抑えている理由（[`LineCapacity::turn_limit`]）が見えなくなる。
    /// 折り返せる本数は、分け直したあとの `cap` から求め直す。
    fn plans_for(&self, input: &[PlanLine]) -> BTreeMap<LineId, LinePlan> {
        let mut plans = make_plans(input, &BTreeMap::new());
        #[cfg(test)]
        let lifted = self.lift_turn_caps;
        #[cfg(not(test))]
        let lifted = false;
        if !lifted {
            let first = plans;
            plans = make_plans(input, &turn_caps(input, &first));
            for (id, p) in &mut plans {
                p.cap = p.cap.max(first[id].cap);
            }
            for (id, n) in turn_caps(input, &plans) {
                plans.get_mut(&id).expect("路線がない").turn_cap = n;
            }
        }
        plans
    }

    /// 書き出しから読み戻したあとに、待てる所の区分けを作り直す。
    ///
    /// 前の規則で書いた保存データでは、作り直した区分けで走らせ続けられない路線に列車が残ることがある。
    /// その列車は、読み戻した状態を保存したときと同じに保つため、ここでは消さず、次の tick に
    /// [`Rail::step_with`] が消す。
    pub(crate) fn restore_plans(&mut self) {
        self.replan();
    }

    /// 走らせ続けられない路線（[`RailLine::cannot_run`]）の列車を消す。
    pub(crate) fn drop_unrunnable_trains(&mut self) {
        let lines = &self.lines;
        let gone: Vec<_> = self
            .trains
            .iter()
            .filter(|(_, t)| lines.get(t.line).is_none_or(RailLine::cannot_run))
            .map(|(id, _)| id)
            .collect();
        for id in gone {
            self.drop_train(id);
        }
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
                        wait: false,
                    }
                })
                .map(|mut s| {
                    s.wait = is_wait(&s);
                    s
                })
                .collect(),
        )
    }

    /// すべての路線の待てる所の区分けと上限（`LinePlan`）を決め直し、走らせ続けられない路線
    /// （[`RailLine::cannot_run`]）の列車を消す。消えた列車の押さえていた閉塞は、それで返る。残った列車は
    /// [`Rail::settle_trains`] で続けられる形に直す。
    fn mark_waits(&mut self) {
        self.replan();
        self.drop_unrunnable_trains();
        self.settle_trains();
        self.settle_riders();
    }

    /// 駅 `from` から駅 `to` への路線を作る。ほかの路線の上限を 0 にする（走れなくする）か、作る路線の上限が
    /// 0 なのにほかの路線の上限を下げるなら断る（[`Rail::deny_path`]）。
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
        self.deny_path(self.lines.next_id(), &path)?;
        let mut line = RailLine {
            stations,
            looped: false,
            trains: 0,
            path,
            stopped: false,
            departed: BTreeMap::new(),
            seg_ticks: Vec::new(),
            plan: LinePlan::default(),
        };
        line.seg_ticks = super::rider::estimate_segments(&line);
        let id = self.lines.insert(line);
        self.mark_waits();
        Ok(id)
    }

    /// 路線の終点の先に駅を 1 つ足す。最初の駅を指せば環状にする。ほかの路線の上限を 0 にするなら断る（[`Rail::deny_path`]）。
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
        self.deny_path(id, &path)?;
        let line = self.lines.get_mut(id).expect("路線がない");
        line.stations = stations;
        line.looped = closes;
        self.replace_path(id, path);
        self.mark_waits();
        Ok(())
    }

    /// 路線の `after_index` 番目の駅の次に駅を挿す。`ExtendLine` と同じく、進路がなければ `RailNoRoute`、
    /// ほかの路線の上限を 0 にするなら `LineStarvesOther`、上限が 0 になる路線でほかの路線の上限を下げるなら
    /// `LineStarvesSelf`、駅が上限なら `LineLimit`、すでに入っている駅は `StationInLine` で断る。
    /// `after_index` が駅の並びの外なら `NoSuchStation` で断る。環状の路線にも挿せる。
    pub(crate) fn insert_line_station(
        &mut self,
        id: LineId,
        after_index: u8,
        station: StationId,
    ) -> Result<(), DenyReason> {
        let line = self.lines.get(id).ok_or(DenyReason::NoSuchLine)?;
        if self.stations.get(station).is_none() {
            return Err(DenyReason::NoSuchStation);
        }
        let at = usize::from(after_index) + 1;
        if at > line.stations.len() {
            return Err(DenyReason::NoSuchStation);
        }
        if line.stations.len() >= MAX_LINE_STATIONS {
            return Err(DenyReason::LineLimit);
        }
        if line.stations.contains(&station) {
            return Err(DenyReason::StationInLine);
        }
        let mut stations = line.stations.clone();
        stations.insert(at, station);
        let looped = line.looped;
        let path = self
            .plan(&stations, looped)
            .ok_or(DenyReason::RailNoRoute)?;
        self.deny_path(id, &path)?;
        let line = self.lines.get_mut(id).expect("路線がない");
        line.stations = stations;
        self.replace_path(id, path);
        self.mark_waits();
        Ok(())
    }

    /// 路線をなくす。走っている列車も消す。
    pub(crate) fn remove_line(&mut self, id: LineId) -> Result<(), DenyReason> {
        self.lines.get(id).ok_or(DenyReason::NoSuchLine)?;
        // 乗っている人が次に止まる駅で降りられるよう、路線を消す前に列車を消す。
        self.remove_trains_where(|t| t.line == id);
        self.lines.remove(id);
        self.mark_waits();
        Ok(())
    }

    /// 路線を走らせる列車の本数を決める。減らした分は、終点で折り返す列車から消える。
    ///
    /// 共有の区画の枠は本数で分けるので、本数を変えると、区画を一緒に使うほかの路線の上限と枠の分が変わる。
    /// ほかの路線の上限を 0 にするなら `LineStarvesOther`、この路線の上限が 0 なのにほかの路線の上限を下げるなら
    /// `LineStarvesSelf` で断る（[`Rail::deny_change`]）。区分けが変わったら、枠を超えた列車をすぐに消す（[`Rail::mark_waits`]）。
    pub(crate) fn set_line_trains(&mut self, id: LineId, trains: u8) -> Result<(), DenyReason> {
        let line = self.lines.get(id).ok_or(DenyReason::NoSuchLine)?;
        let trains = trains.min(MAX_LINE_TRAINS);
        if trains == line.trains {
            return Ok(());
        }
        let change = LineChange {
            id,
            path: line.path.clone(),
            stopped: line.stopped,
            trains,
        };
        self.deny_change(&change)?;
        self.lines.get_mut(id).expect("路線がない").trains = trains;
        // 終点で折り返せる本数は、本数を 0 と 1 以上のあいだで変えると、同じ駅を終点にするほかの路線でも変わる。
        // 区分けは変わらず、これだけが変わったときは、減らした分を終点まで走らせてから消す（`retire_if_extra`）。
        // ほかの路線で下がった分は、作り直したあとに [`Rail::settle_trains`] がその場で消す。
        let plans = self.current_plans();
        let same = self.lines.iter().all(|(l, line)| line.plan == plans[&l]);
        let same_zones = self
            .lines
            .iter()
            .all(|(l, line)| line.plan.same_zones(&plans[&l]));
        if !same_zones {
            // 枠の分を本数に合わせて分け直したので、本数を超えた列車を終点まで残すと、減った分の枠を待って
            // 止まる（本数 0 なら分が 0 で、区画に入れない）。ID の大きい列車から、すぐに消す。
            let allowed = usize::from(plans[&id].cap.min(trains));
            let extra: Vec<_> = self
                .trains()
                .filter(|(_, t)| t.line == id)
                .map(|(tid, _)| tid)
                .skip(allowed)
                .collect();
            for tid in extra {
                self.drop_train(tid);
            }
            self.mark_waits();
        } else if !same {
            self.mark_waits();
        }
        Ok(())
    }

    /// 路線の進路を替え、列車をその上の同じ線路へ移す。
    fn replace_path(&mut self, id: LineId, path: Vec<Step>) {
        let line = self.lines.get_mut(id).expect("路線がない");
        line.stopped = false;
        // 待てる所の印は、進路を選び直したあとに決め直すので、比べない。
        let same = line.path.len() == path.len()
            && line
                .path
                .iter()
                .zip(&path)
                .all(|(a, b)| Step { wait: b.wait, ..*a } == *b);
        if same {
            return;
        }
        let old = line.clone();
        line.path = path;
        line.departed.clear();
        line.seg_ticks = super::rider::estimate_segments(line);
        self.relocate_trains(id, &old);
    }

    /// 線路を引くか取り壊したあとに、路線の駅と進路を選び直す。消えた駅は路線から外し、駅が 2 つ未満に
    /// なった路線はなくす。進路を選べない路線は止まり、それまでの進路を残して、列車を消す。取り壊した線路
    /// `removed` に体がかかるか、先に押さえていた列車は消す。線路を分けただけなら、列車は分けた線路へ移してあるので消さない。
    pub(crate) fn refresh_lines(&mut self, removed: &BTreeSet<TrackId>) {
        // 取り壊した線路に体がかかるか、取り壊した線路を先に押さえた列車は、進路を選び直す前の進路で
        // 押さえた要素を求めて消す。
        let gone: Vec<_> = self
            .trains
            .iter()
            .filter(|(_, t)| {
                removed.contains(&t.track)
                    || self.lines.get(t.line).is_none_or(|l| {
                        self.reserved_steps(l, t)
                            .into_iter()
                            .any(|i| removed.contains(&l.step(i).track))
                    })
            })
            .map(|(id, _)| id)
            .collect();
        for id in gone {
            self.drop_train(id);
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
                self.remove_trains_where(|t| t.line == id);
                self.lines.remove(id);
                continue;
            }
            let plan = self.plan(&stations, looped);
            let line = self.lines.get_mut(id).expect("路線がない");
            line.stations = stations;
            line.looped = looped;
            match plan {
                Some(path) => self.replace_path(id, path),
                // 止まった路線の列車は、mark_waits で消える。
                None => line.stopped = true,
            }
        }
        self.mark_waits();
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
        h.write(&self.wait);
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
        h.write(&self.seg_ticks);
    }
}
